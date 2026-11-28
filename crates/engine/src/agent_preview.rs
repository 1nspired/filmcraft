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
    preview_inner(s, p, None)
}
pub(crate) fn preview_sequence(s: &mut Session, p: &Value) -> Result<Value> {
    agent_bridge::guard(s, p, "agent.previewSequence")?;
    if p.as_object().is_none_or(|p| p.keys().any(|k| !matches!(k.as_str(), "token" | "label" | "operations" | "sequence"))) {
        return Err(bad("agent.previewSequence", "need only active read ticket, explicit sequence, label and operations"));
    }
    let sequence =
        p.get("sequence").and_then(Value::as_u64).map(ItemId).ok_or_else(|| bad("agent.previewSequence", "need an exact unsigned native sequence id"))?;
    preview_inner(s, p, Some(sequence))
}
fn preview_inner(s: &mut Session, p: &Value, scope: Option<ItemId>) -> Result<Value> {
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
    let (mut staged, results, label) = match scope {
        Some(sequence) => agent_bridge::stage_sequence(s, p, sequence)?,
        None => agent_bridge::stage(s, p)?,
    };
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

#[cfg(test)]
mod scoped_tests {
    use super::*;
    use filmcraft_project::{ClipId, ItemKind, TrackKind};
    use filmcraft_time::Tick;

    fn fixture() -> (Session, ItemId, ItemId, ClipId, ClipId) {
        let mut s = Session::default();
        let (project, active) = crate::demo::demo_project(&s.media);
        s.project = Arc::new(project);
        s.state.active_sequence = Some(active);
        let q = s.project.sequence(active).unwrap();
        let mut clip = q.video_tracks[0].items[0].clone();
        let first = clip.id;
        let settings = q.settings.clone();
        let project = Arc::make_mut(&mut s.project);
        let other = project.new_sequence("Separate scoped fixture", settings, 1, 1, None);
        clip.id = ClipId(project.alloc_id());
        clip.start = Tick::ZERO;
        clip.identity = Some(serde_json::from_value(json!({"id":"scoped_instance","handoffSpans":[{"carrier":"poema:sel_scoped poema:bite_scoped","sourceIn":clip.source_in.0,"sourceOut":clip.source_out().0}]})).unwrap());
        let second = clip.id;
        Arc::make_mut(match &mut project.items.get_mut(&other).unwrap().kind {
            ItemKind::Sequence(q) => q,
            _ => panic!("fixture sequence"),
        })
        .video_tracks[0]
            .items
            .push(clip.clone());
        s.state.selection = vec![first];
        s.state.project_selection = vec![clip.item];
        s.state.playheads.insert(active, Tick(123));
        s.state.playheads.insert(other, Tick(456));
        s.state.source_item = Some(clip.item);
        s.state.source_playhead = Tick(789);
        s.state.source_history = vec![clip.item];
        s.state.open_sequences = vec![active, other];
        s.state.clipboard = vec![(TrackKind::Video, 0, clip)];
        s.state.linked_selection = false;
        s.execute("agent.snapshot", json!({})).unwrap();
        (s, active, other, first, second)
    }
    fn scoped(s: &mut Session, sequence: ItemId, clip: ClipId) -> (Value, Value) {
        let read = s.execute("agent.snapshot", json!({})).unwrap();
        let p = s.execute("agent.previewSequence", json!({"token":read["token"],"sequence":sequence.0,"operations":[{"command":"timeline.trim","params":{"clip":clip.0,"edge":"out","mode":"regular","deltaFrames":-1}}]})).unwrap();
        (read, p)
    }
    #[test]
    fn scoped_preview_owns_only_the_requested_project_sequence_and_restores_all_editor_state() {
        let (mut s, active, other, _, clip) = fixture();
        let before = s.project.clone();
        let editor = editor_basis(&s).unwrap();
        let depth = s.history.undo.len();
        let (_, events, _lease) = s.command_journal.subscribe().unwrap();
        let (basis, preview) = scoped(&mut s, other, clip);
        assert!(Arc::ptr_eq(&before, &s.project));
        assert_eq!(editor_basis(&s).unwrap(), editor);
        assert_eq!(preview["document"]["activeSequence"], active.0.to_string());
        assert_eq!(preview["changed"], true);
        assert!(events.try_recv().is_err());
        // Independent native reference: alter just the one record/source tail
        // on a separate full project; no preview/output supplies this geometry.
        let mut expected = (*before).clone();
        let q = Arc::make_mut(match &mut expected.items.get_mut(&other).unwrap().kind {
            ItemKind::Sequence(q) => q,
            _ => panic!("fixture sequence"),
        });
        let frame = q.settings.frame_rate.frame_duration();
        q.video_tracks[0].items[0].duration -= frame;
        let result = s.execute("agent.commitPreview", json!({"token":basis["token"],"preview":preview["preview"]})).unwrap();
        assert_eq!(native_document::bounded_json(&s.project).unwrap(), native_document::bounded_json(&expected).unwrap());
        assert_eq!(editor_basis(&s).unwrap(), editor);
        assert_eq!(s.history.undo.len(), depth + 1);
        assert_eq!(events.try_recv().unwrap()["command"], "agent.commitPreview");
        assert!(events.try_recv().is_err());
        let read = s.execute("agent.snapshot", json!({})).unwrap();
        s.execute("agent.undo", json!({"token":read["token"],"commit":result["commit"]})).unwrap();
        assert!(Arc::ptr_eq(&before, &s.project));
        assert_eq!(editor_basis(&s).unwrap(), editor);
        assert_eq!(events.try_recv().unwrap()["command"], "agent.undo");
        assert!(events.try_recv().is_err());
        let read = s.execute("agent.snapshot", json!({})).unwrap();
        s.execute("agent.redo", json!({"token":read["token"],"commit":result["commit"]})).unwrap();
        assert_eq!(native_document::bounded_json(&s.project).unwrap(), native_document::bounded_json(&expected).unwrap());
        assert_eq!(editor_basis(&s).unwrap(), editor);
        assert_eq!(events.try_recv().unwrap()["command"], "agent.redo");
        s.execute("edit.undo", json!({})).unwrap();
        assert!(Arc::ptr_eq(&before, &s.project));
        assert_eq!(events.try_recv().unwrap()["command"], "edit.undo");
        assert!(events.try_recv().is_err());
    }
    #[test]
    fn scoped_preview_retains_active_ticket_guard_while_another_sequence_is_read() {
        let (mut s, active, other, first, clip) = fixture();
        let read = s.execute("agent.snapshot", json!({})).unwrap();
        let watcher = s.execute("agent.snapshot", json!({"sequence":other.0})).unwrap();
        assert_ne!(read["token"], watcher["token"]);
        let ops = json!([{"command":"timeline.trim","params":{"clip":first.0,"edge":"out","deltaFrames":-1}}]);
        assert!(s.execute("agent.apply", json!({"token":watcher["token"],"operations":ops})).is_err());
        assert!(s.execute("agent.previewSequence", json!({"token":watcher["token"],"sequence":active.0,"operations":ops})).is_err());
        let (_, p) = scoped(&mut s, other, clip);
        assert_eq!(s.execute("agent.snapshot", json!({})).unwrap()["token"], read["token"]);
        let reply = s.execute("agent.commitPreview", json!({"token":read["token"],"preview":p["preview"]})).unwrap();
        assert_eq!(reply["changed"], true);
        assert_eq!(s.state.active_sequence, Some(active));
    }
    #[test]
    fn scoped_preview_hostile_scope_and_invalid_suffix_never_publish_a_prefix() {
        let (mut s, _, other, first, clip) = fixture();
        let read = s.execute("agent.snapshot", json!({})).unwrap();
        let before = s.project.clone();
        let editor = editor_basis(&s).unwrap();
        let rev = s.revision;
        let depth = s.history.undo.len();
        let (_, events, _lease) = s.command_journal.subscribe().unwrap();
        let good = json!({"command":"timeline.trim","params":{"clip":clip.0,"edge":"out","deltaFrames":-1}});
        for sequence in [json!(null), json!(-1), json!(1.5), json!("1"), json!(u64::MAX), json!(s.active_sequence().unwrap().video_tracks[0].items[0].item.0)] {
            assert!(s.execute("agent.previewSequence", json!({"token":read["token"],"sequence":sequence,"operations":[good]})).is_err());
        }
        for bad_op in [
            json!({"command":"timeline.trim","params":{"clip":first.0,"edge":"out","deltaFrames":-1}}),
            json!({"command":"file.saveAs","params":{"path":"forbidden"}}),
            json!({"command":"does.not.exist","params":{}}),
        ] {
            assert!(s.execute("agent.previewSequence", json!({"token":read["token"],"sequence":other.0,"operations":[good,bad_op]})).is_err());
        }
        assert!(s.execute("agent.previewSequence", json!({"token":read["token"],"sequence":other.0,"operations":[good],"project":{}})).is_err());
        assert!(s.execute("agent.previewSequence", json!({"token":read["token"],"sequence":other.0,"operations":vec![good;257]})).is_err());
        assert!(Arc::ptr_eq(&before, &s.project));
        assert_eq!(editor_basis(&s).unwrap(), editor);
        assert_eq!(s.revision, rev);
        assert_eq!(s.history.undo.len(), depth);
        assert!(events.try_recv().is_err());
    }
    #[test]
    fn scoped_preview_noop_discard_editor_change_and_replay_keep_existing_guards() {
        let (mut s, _, other, _, clip) = fixture();
        let before = s.project.clone();
        let read = s.execute("agent.snapshot", json!({})).unwrap();
        let noop = s.execute("agent.previewSequence", json!({"token":read["token"],"sequence":other.0,"operations":[{"command":"timeline.trim","params":{"clip":clip.0,"edge":"out","deltaFrames":0}}]})).unwrap();
        assert_eq!(noop["changed"], false);
        assert_eq!(s.execute("agent.commitPreview", json!({"token":read["token"],"preview":noop["preview"]})).unwrap()["changed"], false);
        let (basis, p) = scoped(&mut s, other, clip);
        s.state.selection.clear();
        assert!(s.execute("agent.commitPreview", json!({"token":basis["token"],"preview":p["preview"]})).is_err());
        s.execute("agent.discardPreview", json!({"preview":p["preview"]})).unwrap();
        assert!(s.execute("agent.commitPreview", json!({"token":basis["token"],"preview":p["preview"]})).is_err());
        let (basis, p) = scoped(&mut s, other, clip);
        let params = json!({"token":basis["token"],"preview":p["preview"]});
        s.execute("agent.commitPreview", params.clone()).unwrap();
        assert!(s.execute("agent.commitPreview", params).is_err());
        s.execute("edit.undo", json!({})).unwrap();
        assert!(Arc::ptr_eq(&before, &s.project));
    }
    #[test]
    fn scoped_preview_shares_capacity_expiry_foreign_and_replacement_guards() {
        let (mut s, _, other, _, clip) = fixture();
        let before = s.project.clone();
        let (basis, p) = scoped(&mut s, other, clip);
        let params = json!({"token":basis["token"],"preview":p["preview"]});
        let (mut foreign, _, _, _, _) = fixture();
        assert!(foreign.execute("agent.commitPreview", params.clone()).is_err());
        let mut replacement = params.clone();
        replacement["sequence"] = json!(other.0);
        assert!(s.execute("agent.commitPreview", replacement).is_err());
        let id = p["preview"].as_str().unwrap();
        s.agent_bridge.previews.pending.get_mut(id).unwrap().created = Instant::now() - LIFETIME;
        assert!(s.execute("agent.commitPreview", params).is_err());
        for _ in 0..MAX_PREVIEWS {
            scoped(&mut s, other, clip);
        }
        let request = json!({"token":basis["token"],"sequence":other.0,"operations":[{"command":"timeline.trim","params":{"clip":clip.0,"edge":"out","deltaFrames":-1}}]});
        assert!(s.execute("agent.previewSequence", request.clone()).is_err());
        // The original active preview shares the same global eight-slot bound.
        assert!(s.execute("agent.preview", request).is_err());
        s.agent_bridge.previews.discard_pending();
        assert!(Arc::ptr_eq(&before, &s.project));
    }
    #[test]
    fn scoped_preview_refuses_shared_media_changes_after_a_valid_sequence_prefix() {
        let (mut s, _, other, _, clip) = fixture();
        let audio = s.project.items.iter().find_map(|(id, item)| item.as_media().filter(|m| m.info.has_audio()).map(|_| *id)).unwrap();
        let operation = json!({"command":"clip.audioChannels","params":{"items":[audio.0],"format":"mono"}});
        // Prove the ordinary engine command really changes the shared item;
        // a malformed/no-op command would not exercise this scope boundary.
        let (mut direct, _, _, _, _) = fixture();
        let original_media = native_document::bounded_json(direct.project.item(audio).unwrap()).unwrap();
        direct.execute("clip.audioChannels", operation["params"].clone()).unwrap();
        assert_ne!(native_document::bounded_json(direct.project.item(audio).unwrap()).unwrap(), original_media);
        let before = s.project.clone();
        let editor = editor_basis(&s).unwrap();
        let read = s.execute("agent.snapshot", json!({})).unwrap();
        let depth = s.history.undo.len();
        let (_, events, _lease) = s.command_journal.subscribe().unwrap();
        let result = s.execute("agent.previewSequence", json!({"token":read["token"],"sequence":other.0,"operations":[{"command":"timeline.trim","params":{"clip":clip.0,"edge":"out","deltaFrames":-1}},operation]}));
        assert!(result.unwrap_err().to_string().contains("outside the requested sequence"));
        assert!(Arc::ptr_eq(&before, &s.project));
        assert_eq!(editor_basis(&s).unwrap(), editor);
        assert_eq!(s.history.undo.len(), depth);
        assert!(events.try_recv().is_err());
    }
}
