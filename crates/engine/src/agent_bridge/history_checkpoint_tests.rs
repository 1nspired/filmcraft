use super::tests::{exact_batch, read, token};
use crate::Session;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

const WORKSPACE: &str = "53c48674-39fe-489a-8c6e-51241af6dccb";
const CHECKPOINT: &str = "901c432f-8c98-4f7e-806a-e53fc4745ee1";
const SAVE: &str = "agent.saveHistoryCheckpoint";
const RESTORE: &str = "agent.restoreHistoryCheckpoint";
struct Scratch(PathBuf);
impl Scratch {
    fn new() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let p = crate::temp_dir().join(format!("filmcraft-history-checkpoint-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
        fs::create_dir(&p).unwrap();
        Self(fs::canonicalize(p).unwrap())
    }
    fn companion(&self) -> PathBuf {
        self.0.join(format!(".poema-history-{WORKSPACE}-{CHECKPOINT}.json"))
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn setup() -> (Scratch, Session) {
    let dir = Scratch::new();
    let mut s = super::condense_tests::setup();
    s.path = Some(dir.0.join("generated.fcproj").to_str().unwrap().into());
    persist(&mut s);
    (dir, s)
}
fn persist(s: &mut Session) {
    filmcraft_format::atomic_write(
        std::path::Path::new(s.path.as_ref().unwrap()),
        &filmcraft_format::encode_with_view(&s.project, Some(&s.project_view()), false),
    )
    .unwrap();
    s.saved_revision = s.revision;
}
fn reopen(s: &Session) -> Session {
    let mut fresh = Session::default();
    fresh.execute("file.open", json!({"path":s.path})).unwrap();
    assert_eq!(fresh.saved_revision, fresh.revision);
    fresh
}
fn request(s: &Session, n: usize) -> Value {
    json!({"token":token(n),"scope":{"projectId":"p_test","anchorSequence":s.state.active_sequence.unwrap().0},"workspace":WORKSPACE,"checkpoint":CHECKPOINT})
}
fn trim(s: &mut Session, n: usize) {
    let c = &s.active_sequence().unwrap().video_tracks[0].items[0];
    let op = json!({"command":"timeline.trim","params":{"clip":c.id.0,"edge":"out","mode":"regular","deltaFrames":-1}});
    read(s, n);
    s.execute("agent.apply", exact_batch(n, vec![op])).unwrap();
}
fn save(s: &mut Session, n: usize) -> Value {
    read(s, n);
    let r = request(s, n);
    let before = state(s);
    let current = s.project.clone();
    let result = s.execute(SAVE, r).unwrap();
    assert_eq!(state(s), before);
    assert!(Arc::ptr_eq(&s.project, &current));
    assert_eq!(result["status"], "saved");
    result
}
fn restore_request(s: &mut Session, n: usize, saved: &Value) -> Value {
    read(s, n);
    let mut r = request(s, n);
    r["sha256"] = saved["checkpoint"]["sha256"].clone();
    r
}
fn state(s: &Session) -> Value {
    json!({"project":s.project,"revision":s.revision,"saved":s.saved_revision,"editor":s.state,"undo":s.history.undo,"redo":s.history.redo,
        "merge":s.history.merge_key,"journal":s.journal,"events":s.events.len(),"epoch":s.agent_bridge.epoch,
        "tickets":s.agent_bridge.tickets.iter().map(|t|json!({"token":t.token,"revision":t.revision,"sequence":t.sequence,"path":t.path,"context":t.context,
            "committed":t.committed,"afterSequence":t.after_sequence,"consumed":t.redo_consumed})).collect::<Vec<_>>()})
}
fn rejected(s: &mut Session, command: &str, r: Value) {
    let before = state(s);
    let project = s.project.clone();
    let undo = s.history.undo.clone();
    let redo = s.history.redo.clone();
    let head = s.agent_bridge.head.clone();
    assert!(s.execute(command, r).is_err());
    assert_eq!(state(s), before);
    assert!(Arc::ptr_eq(&s.project, &project));
    for (a, b) in [(&s.history.undo, &undo), (&s.history.redo, &redo)] {
        assert!(a.iter().zip(b).all(|((al, ap), (bl, bp))| al == bl && Arc::ptr_eq(ap, bp)));
    }
    assert!(match (&head, &s.agent_bridge.head) {
        (None, None) => true,
        (Some((a, p)), Some((b, q))) => a == b && p.ptr_eq(q),
        _ => false,
    });
}

fn metadata_basis(s: &mut Session, n: usize) -> Value {
    s.execute("agent.readHistoryCheckpointBasis", json!({"token":token(n),"scope":request(s,n)["scope"]})).unwrap()
}
fn metadata_request(s: &Session, n: usize) -> Value {
    let mut r = request(s, n);
    r["metadataBasis"] = json!(true);
    r
}

#[test]
fn metadata_checkpoint_preserves_offline_project_without_issuing_any_footage_or_edit_ticket() {
    let (dir, mut s) = setup();
    trim(&mut s, 28001);
    let media = s.project.items.values().find(|it| it.as_media().is_some()).unwrap().id;
    Arc::make_mut(&mut s.project).item_mut(media).unwrap().as_media_mut().unwrap().media =
        filmcraft_project::MediaRef::File { path: dir.0.join("original-is-offline.bin").to_string_lossy().into_owned() };
    let tickets = s.agent_bridge.tickets.len();
    let before = state(&s);
    let file = fs::read(s.path.as_ref().unwrap()).unwrap();
    let basis = metadata_basis(&mut s, 28002);
    assert_eq!(basis["writeAuthority"], false);
    assert_eq!(basis["sourceValidation"], "not-performed");
    assert!(basis.get("sources").is_none());
    assert_eq!(s.agent_bridge.tickets.len(), tickets);
    let result = s.execute(SAVE, metadata_request(&s, 28002)).unwrap();
    assert_eq!(result["status"], "saved");
    assert_eq!(state(&s), before);
    assert_eq!(fs::read(s.path.as_ref().unwrap()).unwrap(), file);
    assert_eq!(s.agent_operation_control().progress(&token(28002))["bytes_read"], "0");
    assert_eq!(s.agent_operation_control().cancel(&json!({"token":token(28002)})).unwrap()["cancellationAccepted"], false);
    rejected(&mut s, "agent.validateSnapshot", json!({"token":token(28002)}));
    rejected(&mut s, "agent.apply", exact_batch(28002, vec![json!({"command":"timeline.razor","params":{"frame":1}})]));
    rejected(&mut s, "agent.undo", json!({"token":token(28002),"commit":token(28001)}));
    let mut r = metadata_request(&s, 28002);
    r["sha256"] = result["checkpoint"]["sha256"].clone();
    rejected(&mut s, RESTORE, r);
    let bytes = fs::read(dir.companion()).unwrap();
    let r = metadata_request(&s, 28002);
    rejected(&mut s, SAVE, r);
    assert_eq!(fs::read(dir.companion()).unwrap(), bytes);
}

#[test]
fn metadata_checkpoint_rejects_stale_document_view_history_scope_and_path_before_creating_file() {
    for mode in ["document", "view", "revision", "path", "history", "limit", "merge", "scope"] {
        let (dir, mut s) = setup();
        trim(&mut s, 28101);
        metadata_basis(&mut s, 28102);
        let mut r = metadata_request(&s, 28102);
        match mode {
            "document" => s.project = Arc::new((*s.project).clone()),
            "view" => s.state.source_playhead += filmcraft_time::Tick(1),
            "revision" => s.revision += 1,
            "path" => s.path = Some(dir.0.join("other.fcproj").to_string_lossy().into_owned()),
            "history" => {
                let p = &mut s.history.undo.last_mut().unwrap().1;
                *p = Arc::new((**p).clone());
            }
            "limit" => s.history.limit += 1,
            "merge" => s.history.merge_key = Some("new human gesture".into()),
            _ => r["scope"]["projectId"] = json!("other"),
        }
        rejected(&mut s, SAVE, r);
        assert!(!dir.companion().exists(), "{mode}");
    }
}

#[test]
fn metadata_checkpoint_tokens_are_bounded_non_reusable_and_never_evict_native_read_tickets() {
    let (_dir, mut s) = setup();
    read(&mut s, 28201);
    let tickets = s.agent_bridge.tickets.len();
    let r = json!({"token":token(28201),"scope":request(&s,28201)["scope"]});
    rejected(&mut s, "agent.readHistoryCheckpointBasis", r);
    for n in 28210..28228 {
        metadata_basis(&mut s, n);
    }
    assert_eq!(s.agent_bridge.history_bases.len(), 16);
    assert_eq!(s.agent_bridge.tickets.len(), tickets);
    assert!(s.agent_bridge.tickets.iter().any(|t| t.token == token(28201)));
    let r = metadata_request(&s, 28210);
    rejected(&mut s, SAVE, r);
    let r = json!({"token":token(28227),"scope":request(&s,28227)["scope"]});
    rejected(&mut s, "agent.readHistoryCheckpointBasis", r);
    let mut r = metadata_request(&s, 28227);
    r["metadataBasis"] = json!(false);
    rejected(&mut s, SAVE, r);
}

#[test]
fn metadata_checkpoint_stop_fences_file_publication_without_undoing_a_prior_edit() {
    let (dir, mut s) = setup();
    trim(&mut s, 28301);
    metadata_basis(&mut s, 28302);
    let before = state(&s);
    let control = s.agent_operation_control();
    let r = metadata_request(&s, 28302);
    let save = super::history_checkpoint::commands().into_iter().find(|c| c.id == SAVE).unwrap().run;
    assert!(
        super::operation_control::run(&mut s, SAVE, &r, |s, p| {
            assert_eq!(control.progress(&token(28302))["phase"], "verifying_inputs");
            assert_eq!(control.cancel(&json!({"token":token(28302)})).unwrap()["cancellationAccepted"], true);
            save(s, p)
        })
        .is_err()
    );
    let r = metadata_request(&s, 28302);
    rejected(&mut s, SAVE, r);
    assert_eq!(state(&s), before);
    assert!(!dir.companion().exists());
}

#[test]
fn metadata_saved_checkpoint_reopens_with_full_file_qualification_and_original_owned_undo() {
    let (_dir, mut s) = setup();
    let original = s.project.clone();
    trim(&mut s, 28401);
    persist(&mut s);
    metadata_basis(&mut s, 28402);
    let result = s.execute(SAVE, metadata_request(&s, 28402)).unwrap();
    let mut fresh = reopen(&s);
    let r = restore_request(&mut fresh, 28403, &result);
    fresh.execute(RESTORE, r).unwrap();
    read(&mut fresh, 28404);
    fresh.execute("agent.undo", json!({"token":token(28404),"commit":token(28401)})).unwrap();
    assert_eq!(serde_json::to_value(&fresh.project).unwrap(), serde_json::to_value(original).unwrap());
}

#[test]
fn checkpoint_stop_before_file_publication_preserves_the_project_history_and_absent_companion() {
    let (dir, mut s) = setup();
    read(&mut s, 27991);
    let request = request(&s, 27991);
    let before = state(&s);
    let control = s.agent_operation_control();
    let save = super::history_checkpoint::commands().into_iter().find(|c| c.id == SAVE).unwrap().run;
    let result = super::operation_control::run(&mut s, SAVE, &request, |s, p| {
        assert_eq!(control.progress(&token(27991))["phase"], "verifying_inputs");
        assert_eq!(control.cancel(&json!({"token":token(27991)})).unwrap()["cancellationAccepted"], true);
        save(s, p)
    });
    assert!(result.is_err());
    assert_eq!(state(&s), before);
    assert!(!dir.companion().exists());
    assert!(s.execute(SAVE, request).is_err());
}

#[test]
fn checkpoint_publication_has_a_late_stop_fence_and_restore_stop_cannot_adopt_history() {
    let (dir, mut s) = setup();
    trim(&mut s, 27992);
    persist(&mut s);
    let saved = save(&mut s, 27993);
    let hash = digest_file(&dir.companion());
    let late = s.agent_operation_control().cancel(&json!({"token":token(27993)})).unwrap();
    assert_eq!(late["status"], "committed");
    assert_eq!(late["cancellationAccepted"], false);
    assert_eq!(digest_file(&dir.companion()), hash);
    let mut fresh = reopen(&s);
    let request = restore_request(&mut fresh, 27994, &saved);
    let before = state(&fresh);
    let control = fresh.agent_operation_control();
    let restore = super::history_checkpoint::commands().into_iter().find(|c| c.id == RESTORE).unwrap().run;
    assert!(
        super::operation_control::run(&mut fresh, RESTORE, &request, |s, p| {
            assert_eq!(control.cancel(&json!({"token":token(27994)})).unwrap()["cancellationAccepted"], true);
            restore(s, p)
        })
        .is_err()
    );
    assert_eq!(state(&fresh), before);
    assert_eq!(digest_file(&dir.companion()), hash);
}

fn digest_file(path: &std::path::Path) -> String {
    format!("{:x}", Sha256::digest(fs::read(path).unwrap()))
}

#[test]
fn native_history_checkpoint_reopen_restores_shared_undo_redo_and_prior_human_work_without_replacing_project_or_view() {
    let (_dir, mut s) = setup();
    let original = serde_json::to_value(&s.project).unwrap();
    s.execute("mixer.setStrip", json!({"strip":"A1","pan":-24})).unwrap();
    let human = serde_json::to_value(&s.project).unwrap();
    trim(&mut s, 27101);
    let edited = serde_json::to_value(&s.project).unwrap();
    persist(&mut s);
    let saved = save(&mut s, 27102);
    let mut fresh = reopen(&s);
    let r = restore_request(&mut fresh, 27103, &saved);
    let current = fresh.project.clone();
    let view = serde_json::to_value(&fresh.state).unwrap();
    let journal = fresh.journal.clone();
    let result = fresh.execute(RESTORE, r.clone()).unwrap();
    assert_eq!(result["action"], "history_restore");
    assert!(Arc::ptr_eq(&fresh.project, &current));
    assert_eq!(serde_json::to_value(&fresh.state).unwrap(), view);
    assert_eq!(fresh.journal, journal);
    assert_eq!(fresh.saved_revision, fresh.revision);
    assert_eq!(fresh.history.undo.len(), 2);
    assert_eq!(fresh.execute(RESTORE, r).unwrap(), result, "lost reply reconciliation is idempotent");
    read(&mut fresh, 27104);
    fresh.execute("agent.undo", json!({"token":token(27104),"commit":token(27101)})).unwrap();
    assert_eq!(serde_json::to_value(&fresh.project).unwrap(), human);
    read(&mut fresh, 27105);
    fresh.execute("agent.redo", json!({"token":token(27105),"commit":token(27101),"undo":token(27104)})).unwrap();
    assert_eq!(serde_json::to_value(&fresh.project).unwrap(), edited);
    fresh.undo().unwrap();
    fresh.undo().unwrap();
    assert_eq!(serde_json::to_value(&fresh.project).unwrap(), original);
}

#[test]
fn native_history_checkpoint_reopen_retains_specific_owned_redo_branch_and_consumed_undo_state() {
    let (_dir, mut s) = setup();
    trim(&mut s, 27201);
    let edited = serde_json::to_value(&s.project).unwrap();
    read(&mut s, 27202);
    s.execute("agent.undo", json!({"token":token(27202),"commit":token(27201)})).unwrap();
    persist(&mut s);
    let saved = save(&mut s, 27203);
    let mut fresh = reopen(&s);
    let r = restore_request(&mut fresh, 27204, &saved);
    fresh.execute(RESTORE, r).unwrap();
    assert_eq!(fresh.history.redo.len(), 1);
    read(&mut fresh, 27205);
    fresh.execute("agent.redo", json!({"token":token(27205),"commit":token(27201),"undo":token(27202)})).unwrap();
    assert_eq!(serde_json::to_value(&fresh.project).unwrap(), edited);
    read(&mut fresh, 27206);
    fresh.execute("agent.undo", json!({"token":token(27206),"commit":token(27201)})).unwrap();
    persist(&mut fresh);
    // A different immutable checkpoint for the next cycle.
    let mut r = request(&fresh, 27207);
    r["checkpoint"] = json!("5a5eb66c-378d-433c-9387-de53297768ae");
    read(&mut fresh, 27207);
    let saved = fresh.execute(SAVE, r.clone()).unwrap();
    let mut again = reopen(&fresh);
    read(&mut again, 27208);
    r["token"] = json!(token(27208));
    r["sha256"] = saved["checkpoint"]["sha256"].clone();
    again.execute(RESTORE, r).unwrap();
    read(&mut again, 27209);
    rejected(&mut again, "agent.redo", json!({"token":token(27209),"commit":token(27201),"undo":token(27202)}));
    again.execute("agent.redo", json!({"token":token(27209),"commit":token(27201),"undo":token(27206)})).unwrap();
    assert_eq!(serde_json::to_value(&again.project).unwrap(), edited);
}

#[test]
fn native_history_checkpoint_live_recovery_rehydrates_expired_tickets_without_replacing_human_history_pointers_or_dirty_state() {
    let (_dir, mut s) = setup();
    trim(&mut s, 27301);
    s.history.merge_key = Some("existing live human gesture".into());
    let saved = save(&mut s, 27302);
    let current = s.project.clone();
    let undo = s.history.undo.clone();
    assert_ne!(s.saved_revision, s.revision);
    for n in 0..super::TICKET_LIMIT + 1 {
        read(&mut s, 27400 + n);
    }
    let r = restore_request(&mut s, 27303, &saved);
    s.execute(RESTORE, r).unwrap();
    assert_ne!(s.saved_revision, s.revision);
    assert!(Arc::ptr_eq(&s.project, &current));
    assert_eq!(s.history.merge_key.as_deref(), Some("existing live human gesture"));
    assert!(s.history.undo.iter().zip(undo).all(|((_, a), (_, b))| Arc::ptr_eq(a, &b)));
    read(&mut s, 27304);
    s.execute("agent.undo", json!({"token":token(27304),"commit":token(27301)})).unwrap();
}

#[test]
fn native_history_checkpoint_live_recovery_refuses_value_identical_document_or_history_pointer_replacement() {
    for mode in ["human-head-document", "history-snapshot", "expired-guard", "merge-ownership"] {
        let (_dir, mut s) = setup();
        trim(&mut s, 27501);
        if mode == "human-head-document" {
            s.execute("mixer.setStrip", json!({"strip":"A1","pan":-12})).unwrap();
        }
        let saved = save(&mut s, 27502);
        if mode == "merge-ownership" {
            s.history.merge_key = Some("different human gesture".into());
        } else if mode == "expired-guard" {
            s.agent_bridge.history_checkpoints.clear();
        } else if mode == "human-head-document" {
            s.project = Arc::new((*s.project).clone());
        } else {
            let p = &mut s.history.undo.last_mut().unwrap().1;
            *p = Arc::new((**p).clone());
        }
        let r = restore_request(&mut s, 27503, &saved);
        rejected(&mut s, RESTORE, r);
    }
}

#[test]
fn native_history_checkpoint_refuses_intervening_live_history_and_nonempty_or_dirty_reopened_sessions() {
    for mode in ["live-edit", "live-undo-redo", "live-replacement", "fresh-dirty", "fresh-history", "fresh-changed", "wrong-path", "overflow"] {
        let (_dir, mut s) = setup();
        trim(&mut s, 27601);
        persist(&mut s);
        let saved = save(&mut s, 27602);
        if mode.starts_with("fresh") {
            s = reopen(&s);
        }
        match mode {
            "live-edit" | "fresh-history" => {
                s.execute("mixer.setStrip", json!({"strip":"A1","pan":10})).unwrap();
            }
            "live-undo-redo" => {
                s.undo().unwrap();
                s.redo().unwrap();
            }
            "live-replacement" => {
                s.project = Arc::new((*s.project).clone());
            }
            "fresh-dirty" => {
                s.saved_revision = 0;
            }
            "fresh-changed" => {
                Arc::make_mut(&mut s.project).name = "Changed current project".into();
            }
            "wrong-path" => {
                s.path = Some("/nonexistent/foreign.fcproj".into());
            }
            "overflow" => {
                s.revision = u64::MAX - 1;
            }
            _ => {}
        }
        let r = restore_request(&mut s, 27603, &saved);
        rejected(&mut s, RESTORE, r);
    }
}

#[test]
fn native_history_checkpoint_hostile_companions_refuse_atomically_without_changing_native_history_or_receipts() {
    let (dir, mut s) = setup();
    trim(&mut s, 27701);
    persist(&mut s);
    let saved = save(&mut s, 27702);
    let raw = fs::read(dir.companion()).unwrap();
    let original: Value = serde_json::from_slice(&raw).unwrap();
    for mode in [
        "hash",
        "schema",
        "scope",
        "sequence",
        "epoch",
        "index",
        "label",
        "detached",
        "empty",
        "token",
        "receipt",
        "operation",
        "results",
        "unknown",
        "project",
    ] {
        let mut c = original.clone();
        match mode {
            "schema" => c["schema"] = json!(0),
            "scope" => c["scope"]["projectId"] = json!("foreign"),
            "sequence" => c["sequence"] = json!(999999),
            "epoch" => c["epoch"] = json!("invalid"),
            "index" => c["undo"][0]["project"] = json!(usize::MAX),
            "label" => c["undo"][0]["label"] = json!("x".repeat(2049)),
            "detached" => {
                let extra = c["projects"][0].clone();
                c["projects"].as_array_mut().unwrap().push(extra);
            }
            "empty" => c["projects"] = json!([]),
            "token" => c["tickets"][0]["token"] = json!("bad"),
            "receipt" => c["tickets"][0]["result"]["revision"] = json!(99999),
            "operation" => c["tickets"][0]["request"]["operations"][0]["command"] = json!("file.open"),
            "results" => c["tickets"][0]["result"]["results"] = json!([]),
            "unknown" => c["force"] = json!(true),
            "project" => {
                let mut p: Value = serde_json::from_str(c["projects"][1].as_str().unwrap()).unwrap();
                p["name"] = json!("x");
                p["unknown_field"] = json!(true);
                c["projects"][1] = json!(p.to_string());
            }
            _ => {}
        }
        let raw = serde_json::to_vec(&c).unwrap();
        fs::write(dir.companion(), &raw).unwrap();
        let mut fresh = reopen(&s);
        let mut r = restore_request(&mut fresh, 27703, &saved);
        r["sha256"] = json!(if mode == "hash" { "0".repeat(64) } else { format!("{:x}", Sha256::digest(&raw)) });
        rejected(&mut fresh, RESTORE, r);
    }
}

#[test]
fn native_history_checkpoint_save_is_private_exclusive_bounded_and_rejects_hostile_requests_without_native_changes() {
    let (dir, mut s) = setup();
    trim(&mut s, 27801);
    let saved = save(&mut s, 27802);
    let raw = fs::read(dir.companion()).unwrap();
    assert_eq!(saved["checkpoint"]["sha256"], format!("{:x}", Sha256::digest(&raw)));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(fs::metadata(dir.companion()).unwrap().permissions().mode() & 0o777, 0o600);
    }
    let valid = request(&s, 27802);
    rejected(&mut s, SAVE, valid.clone());
    assert_eq!(fs::read(dir.companion()).unwrap(), raw);
    for key in ["workspace", "checkpoint", "scope", "sha256", "force"] {
        let mut r = valid.clone();
        r[key] = if key == "scope" { json!({"projectId":"foreign","anchorSequence":1}) } else { json!("../../escape") };
        rejected(&mut s, SAVE, r);
    }
    s.history.limit = 1001;
    rejected(&mut s, SAVE, valid.clone());
    s.history.limit = 200;
    s.path = None;
    rejected(&mut s, SAVE, valid);
}

#[test]
fn native_history_checkpoint_native_serialization_preserves_automatic_points_and_ticks_beyond_javascript_precision() {
    let (_dir, mut s) = setup();
    let item = s.active_sequence().unwrap().video_tracks[0].items[0].item;
    Arc::make_mut(&mut s.project).item_mut(item).unwrap().as_media_mut().unwrap().info.duration = filmcraft_time::Tick(9_007_199_254_741_123);
    trim(&mut s, 27901);
    persist(&mut s);
    let saved = save(&mut s, 27902);
    let mut fresh = reopen(&s);
    let r = restore_request(&mut fresh, 27903, &saved);
    fresh.execute(RESTORE, r).unwrap();
    read(&mut fresh, 27904);
    fresh.execute("agent.undo", json!({"token":token(27904),"commit":token(27901)})).unwrap();
    assert_eq!(fresh.project.item(item).unwrap().as_media().unwrap().info.duration.0, 9_007_199_254_741_123);
    let json = serde_json::to_string(&fresh.project).unwrap();
    assert!(json.contains("9007199254741123"));
    assert!(json.contains("null"));
}

#[test]
fn native_history_checkpoint_reopen_keeps_distinct_native_snapshots_even_when_their_serialized_values_match() {
    let (_dir, mut s) = setup();
    s.edit("First human unchanged snapshot", |_, _| Ok(())).unwrap();
    s.edit("Second human unchanged snapshot", |_, _| Ok(())).unwrap();
    trim(&mut s, 28001);
    persist(&mut s);
    let saved = save(&mut s, 28002);
    let mut fresh = reopen(&s);
    let r = restore_request(&mut fresh, 28003, &saved);
    fresh.execute(RESTORE, r).unwrap();
    assert_eq!(fresh.history.undo.len(), 3);
    assert_eq!(serde_json::to_value(&fresh.history.undo[0].1).unwrap(), serde_json::to_value(&fresh.history.undo[1].1).unwrap());
    assert!(!Arc::ptr_eq(&fresh.history.undo[0].1, &fresh.history.undo[1].1));
    read(&mut fresh, 28004);
    fresh.execute("agent.undo", json!({"token":token(28004),"commit":token(28001)})).unwrap();
    assert_eq!(fresh.undo().unwrap(), "Second human unchanged snapshot");
    assert_eq!(fresh.undo().unwrap(), "First human unchanged snapshot");
}

#[test]
fn native_history_checkpoint_rejects_redirected_public_truncated_or_oversized_files_without_state_changes() {
    let (dir, mut s) = setup();
    trim(&mut s, 28101);
    persist(&mut s);
    let saved = save(&mut s, 28102);
    let original = fs::read(dir.companion()).unwrap();
    for mode in ["truncated", "oversize", "public", "symlink"] {
        fs::write(dir.companion(), &original).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(dir.companion(), fs::Permissions::from_mode(0o600)).unwrap();
        }
        match mode {
            "truncated" => fs::write(dir.companion(), b"{\"kind\":").unwrap(),
            "oversize" => fs::OpenOptions::new().write(true).open(dir.companion()).unwrap().set_len(32 * 1024 * 1024 + 1).unwrap(),
            #[cfg(unix)]
            "public" => {
                use std::os::unix::fs::PermissionsExt;
                fs::set_permissions(dir.companion(), fs::Permissions::from_mode(0o644)).unwrap();
            }
            #[cfg(unix)]
            "symlink" => {
                let target = dir.0.join("not-the-checkpoint.json");
                fs::write(&target, &original).unwrap();
                fs::remove_file(dir.companion()).unwrap();
                std::os::unix::fs::symlink(target, dir.companion()).unwrap();
            }
            _ => continue,
        }
        let mut fresh = reopen(&s);
        let r = restore_request(&mut fresh, 28103, &saved);
        rejected(&mut fresh, RESTORE, r);
        fs::remove_file(dir.companion()).unwrap(); // Only this generated test companion.
    }
}

#[test]
fn selection_group_cold_history_retains_one_original_native_gesture_and_owned_human_undo_redo() {
    let (_dir, mut s) = setup();
    let selected = s.execute("file.newBin", json!({"name":"Selected source bin"})).unwrap()["bin"].as_u64().unwrap();
    let before = s.project.to_json();
    let n = 84980;
    read(&mut s, n);
    let page = s.execute("agent.readBins", json!({"token":token(n),"scope":request(&s,n)["scope"],"offset":0,"limit":1})).unwrap();
    s.execute("agent.apply",json!({"token":token(n),"label":"One original selection group","exact":true,"authority":"document_state","operations":[{"command":"agent.organizeProject","params":{"scope":request(&s,n)["scope"],"beforeSha256":page["sha256"],"edit":{"type":"group","entries":[selected],"name":null}}}]})).unwrap();
    let after = s.project.to_json();
    assert_ne!(before, after);
    persist(&mut s);
    let saved = save(&mut s, n + 1);
    let mut fresh = reopen(&s);
    let restore = restore_request(&mut fresh, n + 2, &saved);
    fresh.execute(RESTORE, restore).unwrap();
    read(&mut fresh, n + 3);
    fresh.execute("agent.undo", json!({"token":token(n+3),"commit":token(n)})).unwrap();
    assert_eq!(fresh.project.to_json(), before);
    read(&mut fresh, n + 4);
    fresh.execute("agent.redo", json!({"token":token(n+4),"commit":token(n),"undo":token(n+3)})).unwrap();
    assert_eq!(fresh.project.to_json(), after);
    fresh.execute("edit.undo", json!({})).unwrap();
    assert_eq!(fresh.project.to_json(), before);
    fresh.execute("edit.redo", json!({})).unwrap();
    assert_eq!(fresh.project.to_json(), after);
}
