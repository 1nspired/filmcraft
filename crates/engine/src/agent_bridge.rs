//! Four-door subset of native v185's guarded read/stage/check/commit boundary.
//! No lifecycle, recovery, UI or provider code. All writes use Session's native undo.
use crate::commands::{CommandSpec, always, bad};
use crate::{Result, Session};
use filmcraft_project::{ClipId, ClipIdentity, ItemKind};
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::{BTreeMap, hash_map::RandomState};
use std::hash::BuildHasher;
use std::sync::{
    Arc, Mutex, Weak,
    atomic::{AtomicU64, Ordering},
    mpsc::{Receiver, SyncSender, sync_channel},
};

static NEXT_SESSION: AtomicU64 = AtomicU64::new(1);
/// Session counters restart in a new process. Two independently seeded standard
/// hashers namespace them across restarts, including otherwise identical projects.
/// These tickets are optimistic concurrency identities, never authentication.
fn session_namespace(serial: u64) -> String {
    let a = RandomState::new().hash_one(("filmcraft-session-a", serial));
    let b = RandomState::new().hash_one(("filmcraft-session-b", serial));
    format!("{a:016x}{b:016x}")
}
pub(crate) fn fresh_owner() -> String {
    session_namespace(NEXT_SESSION.fetch_add(1, Ordering::Relaxed))
}
const MAX_READ_TICKETS: usize = 128;
#[derive(Default)]
struct JournalState {
    ordinal: u64,
    next_subscriber: u64,
    subscribers: Vec<(u64, SyncSender<Value>)>,
}
/// Bounded push fanout, fed only after a successful outer command commits.
#[derive(Clone, Default)]
pub struct CommandJournal(Arc<Mutex<JournalState>>);
/// Either the socket reader or writer ending cancels the subscription.
#[derive(Clone)]
pub struct JournalCancellation {
    state: Weak<Mutex<JournalState>>,
    id: u64,
}
impl Drop for JournalCancellation {
    fn drop(&mut self) {
        if let Some(state) = self.state.upgrade() {
            let mut state = state.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            state.subscribers.retain(|(id, _)| *id != self.id);
        }
    }
}
impl CommandJournal {
    pub fn subscribe(&self) -> Result<(u64, Receiver<Value>, JournalCancellation)> {
        let (tx, rx) = sync_channel(256);
        let mut state = self.0.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        state.next_subscriber = state.next_subscriber.checked_add(1).ok_or_else(|| bad("engine.journal.subscribe", "subscription counter exhausted"))?;
        let id = state.next_subscriber;
        state.subscribers.push((id, tx));
        Ok((state.ordinal, rx, JournalCancellation { state: Arc::downgrade(&self.0), id }))
    }

    pub(crate) fn publish(&self, command: &str, revision: u64, sequence: Option<u64>) {
        let mut state = self.0.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some(next) = state.ordinal.checked_add(1) else {
            state.subscribers.clear();
            return;
        };
        state.ordinal = next;
        let event = json!({"event":"command","ordinal":next,"command":command,"revision":revision,"sequence":sequence});
        // A slow subscriber loses the connection, never silently loses individual events.
        state.subscribers.retain(|(_, tx)| tx.try_send(event.clone()).is_ok());
    }
}

pub(crate) struct BridgeState {
    pub(crate) owner: String,
    pub(crate) previews: crate::agent_preview::PreviewState,
    serial: u64,
    tickets: BTreeMap<Option<filmcraft_project::ItemId>, String>,
    read_revision: u64,
    read_project: Weak<filmcraft_project::Project>,
    owned: Option<(String, u64, usize, bool, Weak<filmcraft_project::Project>)>,
}
impl Default for BridgeState {
    fn default() -> Self {
        Self {
            owner: fresh_owner(),
            previews: Default::default(),
            serial: 0,
            tickets: BTreeMap::new(),
            read_revision: 0,
            read_project: Weak::new(),
            owned: None,
        }
    }
}
pub(crate) fn read_token(s: &mut Session, sequence: Option<filmcraft_project::ItemId>) -> Result<String> {
    let b = &mut s.agent_bridge;
    let same_project = b.read_project.upgrade().is_some_and(|p| Arc::ptr_eq(&p, &s.project));
    if b.read_revision != s.revision || !same_project {
        b.tickets.clear();
        b.read_revision = s.revision;
        b.read_project = Arc::downgrade(&s.project);
    }
    // A read of an unchanged basis must not invalidate another reader's plan.
    if let Some(token) = b.tickets.get(&sequence) {
        return Ok(token.clone());
    }
    // Refuse excess readers, rather than silently evicting a pending ticket.
    if b.tickets.len() >= MAX_READ_TICKETS {
        return Err(bad("agent.snapshot", "too many sequence read tickets for this revision"));
    }
    b.serial = b.serial.checked_add(1).ok_or_else(|| bad("agent.snapshot", "read counter exhausted"))?;
    // An optimistic concurrency ticket, not an authentication credential.
    let token = format!("{}-{:016x}-{:016x}", b.owner, s.revision, b.serial);
    b.tickets.insert(sequence, token.clone());
    Ok(token)
}
pub(crate) fn guard(s: &Session, p: &Value, command: &str) -> Result<()> {
    if p["token"].as_str().is_none()
        || p["token"].as_str() != s.agent_bridge.tickets.get(&s.state.active_sequence).map(String::as_str)
        || s.revision != s.agent_bridge.read_revision
        || !s.agent_bridge.read_project.upgrade().is_some_and(|p| Arc::ptr_eq(&p, &s.project))
    {
        return Err(bad(command, "stale or foreign native read ticket; read again, do not replay the edit"));
    }
    Ok(())
}
fn snapshot(s: &mut Session, params: &Value) -> Result<Value> {
    let sequence_id = params["sequence"].as_u64().map(filmcraft_project::ItemId).or(s.state.active_sequence);
    let token = read_token(s, sequence_id)?;
    let sequence = sequence_id.and_then(|id| s.project.sequence(id)).map(|q| {
        let track = |t: &filmcraft_project::Track| {
            let items: Vec<_> = t.items.iter().map(|c| {
                let path = s.project.resolve_media(c.item).map(|(_,m,_)| match &m.media { filmcraft_project::MediaRef::File { path, .. } => Some(path.clone()), _ => None }).flatten();
                let mut identity = serde_json::to_value(&c.identity).unwrap_or(Value::Null);
                if let Some(spans) = identity.get_mut("handoffSpans").and_then(Value::as_array_mut) {
                    for span in spans {
                        for k in ["sourceIn", "sourceOut"] { if let Some(v) = span[k].as_i64() { span[k] = json!(v.to_string()); } }
                    }
                }
                json!({"clip":c.id.0,"item":c.item.0,"name":c.name,"start":c.start.0.to_string(),"duration":c.duration.0.to_string(),"sourceIn":c.source_in.0.to_string(),"sourceOut":c.source_out().0.to_string(),"speed":c.speed,"reverse":c.reverse,"enabled":c.enabled,"path":path,"identity":identity,"link":c.link,"gainDb":c.gain_db})
            }).collect();
            json!({"id":t.id.0,"name":t.name,"enabled":!t.muted,"items":items})
        };
        json!({"id":sequence_id.map(|id|id.0),"name":sequence_id.and_then(|id|s.project.item(id)).map(|i|i.name.as_str()),"frameRate":{"num":q.settings.frame_rate.num,"den":q.settings.frame_rate.den},"width":q.settings.width,"height":q.settings.height,"video":q.video_tracks.iter().map(track).collect::<Vec<_>>(),"audio":q.audio_tracks.iter().map(track).collect::<Vec<_>>(),"markers":q.markers})
    });
    Ok(
        json!({"protocol":1,"token":token,"revision":s.revision,"project":s.project.name,"sequence":sequence,"undoDepth":s.history.undo.len(),"bins":s.project.root,"catalog":s.project.items.iter().map(|(id,item)|json!({"id":id.0,"name":item.name,"kind":if matches!(item.kind,ItemKind::Sequence(_)){"sequence"}else{"media"}})).collect::<Vec<_>>()}),
    )
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Operation {
    command: String,
    #[serde(default)]
    params: Value,
}
fn allowed(id: &str) -> bool {
    matches!(
        id,
        "timeline.trim"
            | "timeline.razor"
            | "timeline.place"
            | "edit.clear"
            | "poema.setCarrier"
            | "poema.setSequenceMetadata"
            | "poema.setImportedSourceRange"
            | "poema.configureClip"
            | "clip.link"
            | "clip.fitToFrame"
            | "clip.audioChannels"
            | "markers.add"
    )
}
fn apply(s: &mut Session, p: &Value) -> Result<Value> {
    guard(s, p, "agent.apply")?;
    let (staged, results, label) = stage(s, p)?;
    commit_prepared(s, p, &label, staged.project, staged.state, results)
}
pub(crate) fn stage(s: &Session, p: &Value) -> Result<(Session, Vec<Value>, String)> {
    let ops: Vec<Operation> = serde_json::from_value(p["operations"].clone()).map_err(|e| bad("agent.apply", e.to_string()))?;
    if ops.is_empty() || ops.len() > 256 || ops.iter().any(|o| !allowed(&o.command)) {
        return Err(bad("agent.apply", "batch needs 1..256 audited, in-memory edit operations"));
    }
    let label = p["label"].as_str().unwrap_or("Poema edit");
    if label.is_empty() || label.len() > 256 {
        return Err(bad("agent.apply", "invalid label"));
    }
    // Stage in a separate Session. Neither preview publication nor persistence in the
    // live Session can observe intermediate states. This is the v185 batch pattern.
    let mut staged = Session::new(s.services.clone());
    staged.project = s.project.clone();
    staged.state = s.state.clone();
    staged.media = s.media.clone();
    staged.prefs = s.prefs.clone();
    let mut results = Vec::new();
    for op in ops {
        results.push(staged.execute(&op.command, op.params)?);
    }
    for item in staged.project.items.values() {
        if let ItemKind::Sequence(q) = &item.kind {
            q.check().map_err(crate::EngineError::Other)?;
        }
    }
    Ok((staged, results, label.to_owned()))
}
pub(crate) fn commit_prepared(
    s: &mut Session,
    p: &Value,
    label: &str,
    project: Arc<filmcraft_project::Project>,
    state: crate::EditorState,
    results: Vec<Value>,
) -> Result<Value> {
    s.edit(label, |p, st| {
        *p = (*project).clone();
        *st = state;
        Ok(())
    })?;
    let commit = format!("{}-{}", p["token"].as_str().unwrap_or_default(), s.revision);
    s.agent_bridge.owned = Some((commit.clone(), s.revision, s.history.undo.len(), false, Arc::downgrade(&s.project)));
    Ok(json!({"commit":commit,"revision":s.revision,"results":results}))
}
fn history(s: &mut Session, p: &Value, redo: bool) -> Result<Value> {
    let command = if redo { "agent.redo" } else { "agent.undo" };
    guard(s, p, command)?;
    let Some((commit, rev, depth, undone, project)) = s.agent_bridge.owned.clone() else { return Err(bad(command, "no agent-owned history")) };
    if p["commit"].as_str() != Some(&commit)
        || rev != s.revision
        || undone != redo
        || s.history.undo.len() != depth
        || !project.upgrade().is_some_and(|p| Arc::ptr_eq(&p, &s.project))
    {
        return Err(bad(command, "human edit/undo or another document change intervened; refusing another owner's history"));
    }
    let label = if redo { s.redo() } else { s.undo() }.ok_or_else(|| bad(command, "owned history is unavailable"))?;
    s.agent_bridge.owned = Some((commit.clone(), s.revision, s.history.undo.len(), !redo, Arc::downgrade(&s.project)));
    Ok(json!({"commit":commit,"revision":s.revision,"label":label}))
}
fn configure(s: &mut Session, p: &Value) -> Result<Value> {
    let clip = p["clip"].as_u64().map(ClipId).ok_or_else(|| bad("poema.configureClip", "need clip"))?;
    let speed = p["speed"].as_f64().unwrap_or(1.0);
    if !speed.is_finite() || speed <= 0.0 || speed > 100.0 {
        return Err(bad("poema.configureClip", "speed must be finite and in(0,100]"));
    }
    let enabled = p["enabled"].as_bool().unwrap_or(true);
    let name = p["name"].as_str().map(str::to_owned);
    if name.as_ref().is_some_and(|s| s.len() > 1024) {
        return Err(bad("poema.configureClip", "clip name too long"));
    }
    let track_enabled = p["trackEnabled"].as_bool().unwrap_or(true);
    s.edit_sequence("Configure delivered clip", |q, _, _| {
        let track_id = q.find_item(clip).map(|(t, _)| t).ok_or_else(|| bad("poema.configureClip", "unknown clip"))?;
        let track = q.track_mut(track_id).ok_or_else(|| bad("poema.configureClip", "unknown track"))?;
        track.muted = !track_enabled;
        let c = track.items.iter_mut().find(|c| c.id == clip).ok_or_else(|| bad("poema.configureClip", "unknown clip"))?;
        c.speed = speed;
        c.enabled = enabled;
        if let Some(name) = name {
            c.name = name;
        }
        Ok(())
    })?;
    Ok(Value::Null)
}
fn carrier(s: &mut Session, p: &Value) -> Result<Value> {
    let clip = p["clip"].as_u64().map(ClipId).ok_or_else(|| bad("poema.setCarrier", "need clip"))?;
    let mut value = p["identity"].clone();
    if let Some(spans) = value.get_mut("handoffSpans").and_then(Value::as_array_mut) {
        for span in spans {
            for key in ["sourceIn", "sourceOut"] {
                if let Some(text) = span[key].as_str() {
                    let tick = text.parse::<i64>().map_err(|_| bad("poema.setCarrier", "invalid source tick"))?;
                    span[key] = json!(tick);
                }
            }
        }
    }
    let identity: ClipIdentity = serde_json::from_value(value).map_err(|e| bad("poema.setCarrier", e.to_string()))?;
    identity.validate().map_err(|e| bad("poema.setCarrier", e))?;
    s.edit_sequence("Stamp Poema provenance", |q, _, _| {
        let track = q.find_item(clip).map(|(track, _)| track).ok_or_else(|| bad("poema.setCarrier", "unknown clip"))?;
        let item = q.track_mut(track).and_then(|t| t.items.iter_mut().find(|c| c.id == clip)).ok_or_else(|| bad("poema.setCarrier", "unknown clip"))?;
        item.identity = Some(Box::new(identity));
        Ok(())
    })?;
    Ok(Value::Null)
}
fn spec(id: &'static str, label: &'static str, run: fn(&mut Session, &Value) -> Result<Value>, journal: bool) -> CommandSpec {
    CommandSpec { id, label, menu: &[], shortcut: None, params: "{…}", enabled: always, run, journal }
}
pub(crate) fn commands() -> Vec<CommandSpec> {
    vec![
        spec("poema.configureClip", "Configure delivered clip", configure, true),
        spec("poema.owner", "Read managed native project owner", crate::project_owner::status, false),
        spec("poema.checkpoint", "Checkpoint the guarded native owner", crate::project_owner::checkpoint, false),
        spec("agent.snapshot", "Read guarded native sequence", snapshot, false),
        spec("agent.document", "Read lossless native project", crate::native_document::read, false),
        spec("agent.apply", "Apply guarded Poema batch", apply, true),
        spec("agent.preview", "Preview isolated Poema batch", crate::agent_preview::preview, false),
        spec("agent.commitPreview", "Commit checked Poema preview", crate::agent_preview::commit, true),
        spec("agent.discardPreview", "Discard Poema preview", crate::agent_preview::discard, false),
        spec("agent.undo", "Undo owned Poema batch", |s, p| history(s, p, false), true),
        spec("agent.redo", "Redo owned Poema batch", |s, p| history(s, p, true), true),
        spec("poema.setImportedSourceRange", "Preserve imported source interval", crate::import_source::set, true),
        spec("poema.setCarrier", "Stamp timed Poema ids", carrier, true),
        spec("poema.setSequenceMetadata", "Preserve Poema editorial metadata", crate::poema_metadata::set, true),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use filmcraft_time::Tick;
    fn demo() -> Session {
        let mut s = Session::default();
        let (project, sequence) = crate::demo::demo_project(&s.media);
        s.project = Arc::new(project);
        s.state.active_sequence = Some(sequence);
        s
    }
    fn clip(s: &Session) -> u64 {
        s.active_sequence().unwrap().video_tracks[0].items[0].id.0
    }
    fn trim(id: u64) -> Value {
        json!({"command":"timeline.trim","params":{"clip":id,"edge":"out","deltaFrames":-1}})
    }
    #[test]
    fn watcher_reads_share_a_basis_without_cancelling_an_agent_plan() {
        let mut s = demo();
        let id = clip(&s);
        let agent = s.execute("agent.snapshot", json!({})).unwrap();
        for _ in 0..256 {
            let watcher = s.execute("agent.snapshot", json!({})).unwrap();
            assert_eq!(agent["token"], watcher["token"]);
        }
        let foreign = s.execute("agent.snapshot", json!({"sequence":u64::MAX})).unwrap();
        assert_ne!(foreign["token"], agent["token"]);
        assert!(s.execute("agent.apply", json!({"token":foreign["token"],"operations":[trim(id)]})).is_err());
        let (_, events, _subscription) = s.command_journal.subscribe().unwrap();
        let applied = s.execute("agent.apply", json!({"token":agent["token"],"operations":[trim(id)]})).unwrap();
        assert_eq!(events.try_recv().unwrap()["command"], "agent.apply");
        assert!(events.try_recv().is_err());
        assert!(s.execute("agent.apply", json!({"token":agent["token"],"operations":[trim(id)]})).is_err());
        let read = s.execute("agent.snapshot", json!({})).unwrap();
        s.execute("agent.snapshot", json!({})).unwrap();
        s.execute("agent.undo", json!({"token":read["token"],"commit":applied["commit"]})).unwrap();
    }
    #[test]
    fn ticket_limit_refuses_new_readers_without_evicting_an_existing_basis() {
        let mut s = demo();
        let id = clip(&s);
        let agent = s.execute("agent.snapshot", json!({})).unwrap();
        for sequence in 0..(MAX_READ_TICKETS - 1) {
            read_token(&mut s, Some(filmcraft_project::ItemId(u64::MAX - sequence as u64))).unwrap();
        }
        assert!(read_token(&mut s, None).is_err());
        s.execute("agent.apply", json!({"token":agent["token"],"operations":[trim(id)]})).unwrap();
        s.execute("agent.snapshot", json!({})).unwrap();
        assert_eq!(s.agent_bridge.tickets.len(), 1);
    }
    #[test]
    fn namespace_does_not_repeat_when_a_process_counter_restarts() {
        let names: std::collections::BTreeSet<_> = (0..128).map(|_| session_namespace(1)).collect();
        assert_eq!(names.len(), 128);
        assert!(names.iter().all(|name| name.len() == 32 && name.bytes().all(|b| b.is_ascii_hexdigit())));
    }
    #[test]
    fn tickets_cannot_cross_sessions_or_the_active_sequence() {
        let mut s = demo();
        let id = clip(&s);
        let read = s.execute("agent.snapshot", json!({})).unwrap();
        let mut other = demo();
        let other_id = clip(&other);
        other.execute("agent.snapshot", json!({})).unwrap();
        assert!(other.execute("agent.apply", json!({"token":read["token"],"operations":[trim(other_id)]})).is_err());
        s.state.active_sequence = None;
        assert!(s.execute("agent.apply", json!({"token":read["token"],"operations":[trim(id)]})).is_err());
    }
    #[test]
    fn a_read_ticket_cannot_cross_a_document_replacement_with_the_same_revision() {
        let mut s = demo();
        let id = clip(&s);
        let read = s.execute("agent.snapshot", json!({})).unwrap();
        s.project = Arc::new((*s.project).clone());
        assert!(s.execute("agent.apply", json!({"token":read["token"],"operations":[trim(id)]})).is_err());
    }
    #[test]
    fn disconnected_subscriber_releases_its_slot_without_an_edit() {
        let journal = CommandJournal::default();
        let (_, rx, cancel) = journal.subscribe().unwrap();
        assert_eq!(journal.0.lock().unwrap().subscribers.len(), 1);
        drop(cancel);
        assert_eq!(journal.0.lock().unwrap().subscribers.len(), 0);
        assert!(matches!(rx.try_recv(), Err(std::sync::mpsc::TryRecvError::Disconnected)));
    }
    #[test]
    fn failed_batch_never_publishes_or_changes_native_history() {
        let mut s = demo();
        let id = clip(&s);
        let doc = s.execute("agent.snapshot", json!({})).unwrap();
        let before = s.project.clone();
        let depth = s.history.undo.len();
        let (_, rx, _subscription) = s.command_journal.subscribe().unwrap();
        assert!(s.execute("agent.apply", json!({"token":doc["token"],"operations":[trim(id),trim(u64::MAX)]})).is_err());
        assert!(Arc::ptr_eq(&before, &s.project));
        assert_eq!(s.history.undo.len(), depth);
        assert!(rx.try_recv().is_err());
    }
    #[test]
    fn guarded_batch_is_one_native_undo_and_rejects_stale_reads() {
        let mut s = demo();
        let id = clip(&s);
        let doc = s.execute("agent.snapshot", json!({})).unwrap();
        let before = s.project.clone();
        let depth = s.history.undo.len();
        let applied = s.execute("agent.apply", json!({"token":doc["token"],"operations":[trim(id),trim(id)]})).unwrap();
        assert_eq!(s.history.undo.len(), depth + 1);
        assert!(s.execute("agent.apply", json!({"token":doc["token"],"operations":[trim(id)]})).is_err());
        let read = s.execute("agent.snapshot", json!({})).unwrap();
        s.execute("agent.undo", json!({"token":read["token"],"commit":applied["commit"]})).unwrap();
        assert!(Arc::ptr_eq(&before, &s.project));
        let read = s.execute("agent.snapshot", json!({})).unwrap();
        s.execute("agent.redo", json!({"token":read["token"],"commit":applied["commit"]})).unwrap();
        assert_eq!(s.history.undo.len(), depth + 1);
    }
    #[test]
    fn agent_undo_cannot_take_a_humans_history() {
        let mut s = demo();
        let id = clip(&s);
        let doc = s.execute("agent.snapshot", json!({})).unwrap();
        let batch = s.execute("agent.apply", json!({"token":doc["token"],"operations":[trim(id)]})).unwrap();
        s.execute("timeline.trim", trim(id)["params"].clone()).unwrap();
        let before = s.project.clone();
        let read = s.execute("agent.snapshot", json!({})).unwrap();
        assert!(s.execute("agent.undo", json!({"token":read["token"],"commit":batch["commit"]})).is_err());
        assert!(Arc::ptr_eq(&before, &s.project));
    }
    #[test]
    fn journal_pushes_once_per_outer_edit_and_undo_never_for_reads_or_refusals() {
        let mut s = demo();
        let id = clip(&s);
        let (cursor, rx, _subscription) = s.command_journal.subscribe().unwrap();
        let doc = s.execute("agent.snapshot", json!({})).unwrap();
        s.execute("agent.apply", json!({"token":doc["token"],"operations":[trim(id),trim(id)]})).unwrap();
        s.execute("edit.undo", json!({})).unwrap();
        s.execute("agent.snapshot", json!({})).unwrap();
        let a = rx.try_recv().unwrap();
        let b = rx.try_recv().unwrap();
        assert_eq!(a["ordinal"], cursor + 1);
        assert_eq!(a["command"], "agent.apply");
        assert_eq!(b["ordinal"], cursor + 2);
        assert_eq!(b["command"], "edit.undo");
        assert!(rx.try_recv().is_err());
    }
    #[test]
    fn timed_handoff_survives_native_split_both_halves_and_undo() {
        let mut s = demo();
        let id = clip(&s);
        let q = s.active_sequence().unwrap();
        let c = q.find_item(ClipId(id)).unwrap().1;
        let original_start = c.source_in;
        let original_out = c.source_out();
        let cut = q.settings.frame_rate.snap(c.start + Tick(c.duration.0 / 2));
        s.execute("poema.setCarrier",json!({"clip":id,"identity":{"id":"clip-original","handoffSpans":[{"carrier":"poema:sel_real poema:bite_real_1","sourceIn":original_start.0.to_string(),"sourceOut":original_out.0.to_string()}]}})).unwrap();
        let before = s.project.clone();
        s.execute("timeline.razor", json!({"clip":id,"time":cut.0})).unwrap();
        let q = s.active_sequence().unwrap();
        let tracked: Vec<_> = q.video_tracks.iter().flat_map(|t| t.items.iter()).filter(|c| c.identity.is_some()).collect();
        assert_eq!(tracked.len(), 2);
        assert_ne!(tracked[0].identity.as_ref().unwrap().id, tracked[1].identity.as_ref().unwrap().id);
        assert_eq!(tracked[0].identity.as_ref().unwrap().handoff_spans, tracked[1].identity.as_ref().unwrap().handoff_spans);
        s.execute("edit.undo", json!({})).unwrap();
        assert!(Arc::ptr_eq(&before, &s.project));
        let encoded = serde_json::to_value(&s.project).unwrap();
        let decoded: filmcraft_project::Project = serde_json::from_value(encoded).unwrap();
        assert_eq!(
            decoded.sequence(s.state.active_sequence.unwrap()).unwrap().find_item(ClipId(id)).unwrap().1.identity.as_ref().unwrap().handoff_spans[0].source_in,
            original_start
        );
    }
    #[test]
    fn legacy_constructor_is_untracked_and_invalid_carrier_is_atomic() {
        let mut s = demo();
        let id = clip(&s);
        assert!(s.active_sequence().unwrap().find_item(ClipId(id)).unwrap().1.identity.is_none());
        let before = s.project.clone();
        assert!(
            s.execute(
                "poema.setCarrier",
                json!({"clip":id,"identity":{"id":"clip","handoffSpans":[{"carrier":"invented","sourceIn":0,"sourceOut":filmcraft_time::TICKS_PER_SECOND}]}})
            )
            .is_err()
        );
        assert!(Arc::ptr_eq(&before, &s.project));
    }
}
