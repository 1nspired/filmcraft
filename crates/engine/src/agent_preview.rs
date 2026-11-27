//! Native previews are bounded, short-lived, session-owned prepared projects.
//! A client may approve a prepared result, never replace it with client JSON.
use crate::{EditorState, Result, Session, agent_bridge, commands::bad, native_document};
use filmcraft_project::{ItemId, Project};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    sync::{Arc, Weak},
    time::{Duration, Instant},
};

const MAX_PREVIEWS: usize = 8;
const LIFETIME: Duration = Duration::from_secs(60);
struct Prepared {
    basis: Weak<Project>,
    revision: u64,
    sequence: Option<ItemId>,
    editor: Vec<u8>,
    created: Instant,
    project: Arc<Project>,
    state: EditorState,
    results: Vec<Value>,
    label: String,
    changed: bool,
}
#[derive(Default)]
pub(crate) struct PreviewState {
    serial: u64,
    pending: BTreeMap<String, Prepared>,
}
impl PreviewState {
    pub(crate) fn discard_pending(&mut self) {
        // Never recycle an old preview identity in the same Session.
        self.pending.clear();
    }
}
fn editor_basis(s: &Session) -> Result<Vec<u8>> {
    // EditorState's persisted representation omits the clipboard. Guard it too:
    // committing a delayed preview must not overwrite intervening human state.
    native_document::bounded_json(&(&s.state, &s.state.clipboard, &s.prefs))
}
fn current(p: &Prepared, s: &Session) -> bool {
    p.created.elapsed() < LIFETIME
        && p.revision == s.revision
        && p.sequence == s.state.active_sequence
        && p.basis.upgrade().is_some_and(|project| Arc::ptr_eq(&project, &s.project))
}
pub(crate) fn preview(s: &mut Session, p: &Value) -> Result<Value> {
    agent_bridge::guard(s, p, "agent.preview")?;
    let project = s.project.clone();
    let revision = s.revision;
    let sequence = s.state.active_sequence;
    s.agent_bridge.previews.pending.retain(|_, p| {
        p.created.elapsed() < LIFETIME
            && p.revision == revision
            && p.sequence == sequence
            && p.basis.upgrade().is_some_and(|basis| Arc::ptr_eq(&basis, &project))
    });
    if s.agent_bridge.previews.pending.len() >= MAX_PREVIEWS {
        return Err(bad("agent.preview", "eight previews pending; discard one or wait for expiry"));
    }
    let editor = editor_basis(s)?;
    let (mut staged, results, label) = agent_bridge::stage(s, p)?;
    let changed = native_document::bounded_json(&project)? != native_document::bounded_json(&staged.project)?
        && !crate::poema_audio_fades::same_project_up_to_envelope_layout(&project, &staged.project)?;
    let document = native_document::read(&mut staged, &Value::Null)?;
    let store = &mut s.agent_bridge.previews;
    store.serial = store.serial.checked_add(1).ok_or_else(|| bad("agent.preview", "preview counter exhausted"))?;
    let id = format!("preview-{}-{}", s.agent_bridge.owner, store.serial);
    let reply = json!({"protocol":1,"preview":id,"expiresInMs":60000,"document":document,"changed":changed,"results":results});
    // Validate the entire envelope before retaining a prepared project.
    native_document::bounded_json(&reply)?;
    store.pending.insert(
        id,
        Prepared {
            basis: Arc::downgrade(&project),
            revision,
            sequence,
            editor,
            created: Instant::now(),
            project: staged.project,
            state: staged.state,
            results,
            label,
            changed,
        },
    );
    Ok(reply)
}
pub(crate) fn commit(s: &mut Session, p: &Value) -> Result<Value> {
    agent_bridge::guard(s, p, "agent.commitPreview")?;
    let id = p["preview"].as_str().ok_or_else(|| bad("agent.commitPreview", "need preview id"))?;
    // No replacement operations/project may be submitted after policy approval.
    if p.as_object().is_none_or(|p| p.keys().any(|k| k != "preview" && k != "token")) {
        return Err(bad("agent.commitPreview", "commit accepts only preview id and original read ticket"));
    }
    let prepared = s.agent_bridge.previews.pending.get(id).ok_or_else(|| bad("agent.commitPreview", "unknown, discarded or consumed preview"))?;
    if !current(prepared, s) || prepared.editor != editor_basis(s)? {
        return Err(bad("agent.commitPreview", "preview expired or project/editor state changed; prepare again"));
    }
    let prepared = s.agent_bridge.previews.pending.remove(id).ok_or_else(|| bad("agent.commitPreview", "preview no longer available"))?;
    if !prepared.changed {
        return Ok(json!({"changed":false,"revision":s.revision,"results":prepared.results}));
    }
    let mut result = agent_bridge::commit_prepared(s, p, &prepared.label, prepared.project, prepared.state, prepared.results)?;
    result["changed"] = json!(true);
    Ok(result)
}
pub(crate) fn discard(s: &mut Session, p: &Value) -> Result<Value> {
    let id = p["preview"].as_str().ok_or_else(|| bad("agent.discardPreview", "need preview id"))?;
    Ok(json!({"discarded":s.agent_bridge.previews.pending.remove(id).is_some()}))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn demo() -> Session {
        let mut s = Session::default();
        let (project, sequence) = crate::demo::demo_project(&s.media);
        s.project = Arc::new(project);
        s.state.active_sequence = Some(sequence);
        s
    }
    fn prepare(s: &mut Session) -> (Value, Value) {
        let basis = s.execute("agent.snapshot", json!({})).unwrap();
        let c = s.active_sequence().unwrap().video_tracks[0].items[0].id.0;
        let preview = s
            .execute(
                "agent.preview",
                json!({"token":basis["token"],"operations":[{"command":"timeline.trim","params":{"clip":c,"edge":"out","deltaFrames":-1}}]}),
            )
            .unwrap();
        (basis, preview)
    }
    #[test]
    fn preview_is_invisible_commit_is_one_edit_and_replay_is_refused() {
        let mut s = demo();
        let before = s.project.clone();
        let depth = s.history.undo.len();
        let revision = s.revision;
        let (_, events, _lease) = s.command_journal.subscribe().unwrap();
        let (basis, preview) = prepare(&mut s);
        assert!(Arc::ptr_eq(&before, &s.project));
        assert_eq!(s.revision, revision);
        assert_eq!(s.history.undo.len(), depth);
        assert!(events.try_recv().is_err());
        let params = json!({"token":basis["token"],"preview":preview["preview"]});
        let result = s.execute("agent.commitPreview", params.clone()).unwrap();
        assert_eq!(result["changed"], true);
        assert_eq!(s.history.undo.len(), depth + 1);
        assert_eq!(events.try_recv().unwrap()["command"], "agent.commitPreview");
        assert!(events.try_recv().is_err());
        assert!(s.execute("agent.commitPreview", params).is_err());
        s.execute("edit.undo", json!({})).unwrap();
        assert!(Arc::ptr_eq(&before, &s.project));
    }
    #[test]
    fn state_expiry_capacity_foreign_and_replacement_guards_are_atomic() {
        let mut s = demo();
        let before = s.project.clone();
        let revision = s.revision;
        let (basis, preview) = prepare(&mut s);
        let params = json!({"token":basis["token"],"preview":preview["preview"]});
        let mut replacement = params.clone();
        replacement["operations"] = json!([]);
        assert!(s.execute("agent.commitPreview", replacement).is_err());
        let mut other = demo();
        assert!(other.execute("agent.commitPreview", params.clone()).is_err());
        s.state.linked_selection = !s.state.linked_selection;
        assert!(s.execute("agent.commitPreview", params.clone()).is_err());
        s.state.linked_selection = !s.state.linked_selection;
        let id = preview["preview"].as_str().unwrap();
        s.agent_bridge.previews.pending.get_mut(id).unwrap().created = Instant::now() - LIFETIME;
        assert!(s.execute("agent.commitPreview", params).is_err());
        for _ in 0..MAX_PREVIEWS {
            prepare(&mut s);
        }
        let c = s.active_sequence().unwrap().video_tracks[0].items[0].id.0;
        assert!(
            s.execute(
                "agent.preview",
                json!({"token":basis["token"],"operations":[{"command":"timeline.trim","params":{"clip":c,"edge":"out","deltaFrames":-1}}]})
            )
            .is_err()
        );
        assert!(Arc::ptr_eq(&before, &s.project));
        assert_eq!(s.revision, revision);
    }

    #[test]
    fn discard_noop_and_failed_stage_have_no_edit_footprint() {
        let mut s = demo();
        let before = s.project.clone();
        let revision = s.revision;
        let depth = s.history.undo.len();
        let (_, events, _lease) = s.command_journal.subscribe().unwrap();
        let (basis, preview) = prepare(&mut s);
        assert_eq!(s.execute("agent.discardPreview", json!({"preview":preview["preview"]})).unwrap()["discarded"], true);
        assert!(s.execute("agent.commitPreview", json!({"token":basis["token"],"preview":preview["preview"]})).is_err());
        let c = s.active_sequence().unwrap().video_tracks[0].items[0].id.0;
        let unchanged =
            s.execute("agent.preview", json!({"token":basis["token"],"operations":[{"command":"poema.configureClip","params":{"clip":c}}]})).unwrap();
        assert_eq!(unchanged["changed"], false);
        assert_eq!(s.execute("agent.commitPreview", json!({"token":basis["token"],"preview":unchanged["preview"]})).unwrap()["changed"], false);
        for command in ["file.saveAs", "clip.updateMetadata", "does.not.exist"] {
            assert!(s.execute("agent.preview", json!({"token":basis["token"],"operations":[{"command":command,"params":{}}]})).is_err());
        }
        assert!(s.execute("agent.preview", json!({"token":basis["token"],"operations":[{"command":"timeline.trim","params":{"clip":c,"edge":"out","deltaFrames":-1}},{"command":"timeline.trim","params":{"clip":u64::MAX,"edge":"out","deltaFrames":-1}}]})).is_err());
        assert!(Arc::ptr_eq(&before, &s.project));
        assert_eq!(s.revision, revision);
        assert_eq!(s.history.undo.len(), depth);
        assert!(events.try_recv().is_err());
    }

    #[test]
    fn human_edit_or_document_change_invalidates_a_prepared_result() {
        let mut s = demo();
        let (basis, preview) = prepare(&mut s);
        let c = s.active_sequence().unwrap().video_tracks[0].items[0].id.0;
        s.execute("timeline.trim", json!({"clip":c,"edge":"out","deltaFrames":-1})).unwrap();
        let edited = s.project.clone();
        let depth = s.history.undo.len();
        assert!(s.execute("agent.commitPreview", json!({"token":basis["token"],"preview":preview["preview"]})).is_err());
        assert!(Arc::ptr_eq(&edited, &s.project));
        assert_eq!(s.history.undo.len(), depth);
        let (basis, preview) = prepare(&mut s);
        s.project = Arc::new((*s.project).clone());
        assert!(s.execute("agent.commitPreview", json!({"token":basis["token"],"preview":preview["preview"]})).is_err());
    }
}
