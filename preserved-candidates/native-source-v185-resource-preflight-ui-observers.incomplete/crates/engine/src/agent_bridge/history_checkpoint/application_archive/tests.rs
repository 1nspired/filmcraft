use super::super::{Capsule, Request, SAVE, pending_capsule, prepare_restore};
use super::*;
use crate::{History, session_worker::SessionWorker};
use serde_json::json;
use std::{
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};
const W: &str = "6cd868fc-9e1c-485b-87aa-9c60e13b00bf";
const C: &str = "901c432f-8c98-4f7e-806a-e53fc4745ee1";
static NEXT: AtomicU64 = AtomicU64::new(1);
struct Root(PathBuf);
impl Drop for Root {
    fn drop(&mut self) {
        if std::fs::read(self.0.join("generated-root.txt")).ok().as_deref() == Some(b"application-worker-v1\n") {
            std::fs::remove_dir_all(&self.0).unwrap();
        }
    }
}
fn scope() -> Value {
    json!({"projectId":"p_application_tabs","workspaceId":W})
}
fn setup() -> (Root, Session, filmcraft_project::ItemId) {
    let path = std::env::temp_dir().join(format!("application-worker-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
    std::fs::create_dir(&path).unwrap();
    std::fs::write(path.join("generated-root.txt"), b"application-worker-v1\n").unwrap();
    let mut s = Session::default();
    s.execute("file.openDemoProject", json!({})).unwrap();
    let main = s.state.active_sequence.unwrap();
    s.execute("project.associateEditorialOwner", json!({"scope":{"projectId":"p_application_tabs","anchorSequence":main.0},"workspaceId":W})).unwrap();
    Arc::make_mut(&mut s.project).metadata.insert(
        "poema.projectWorkspace.v1".into(),
        serde_json::to_string(&json!({"kind":"poema-native-project-workspace","version":1,"scope":scope()})).unwrap(),
    );
    s.execute("file.newSequence", json!({"name":"Generated archive tab","video":0,"audio":0})).unwrap();
    let other = s.state.active_sequence.unwrap();
    s.execute("sequence.open", json!({"item":main.0})).unwrap();
    s.execute("file.save", json!({"path":path.join("generated.fcproj")})).unwrap();
    s.history = History { limit: 128, ..History::default() };
    s.agent_operation_control().enable_transport();
    s.events.clear();
    (Root(path), s, other)
}
fn ui() -> Value {
    let data = serde_json::to_string(&json!({"kind":"generated_engine_only_ui_fixture","version":1})).unwrap();
    json!({"kind":"native_application_tab_ui_proof","version":1,"bytes":data.len(),"sha256":digest(data.as_bytes()),"data":data})
}
fn publish(s: &mut Session, item: filmcraft_project::ItemId, n: u64, unknown: bool) -> (Value, Value, ApplicationTabReceipt) {
    let token = format!("{n:064x}");
    let native = s.prepare_application_tab_scope(&json!({"token":token,"scope":scope(),"projectPath":s.path})).unwrap();
    let d = json!({"kind":"native_application_tab_view","version":1,"status":"read","applicationInstance":"1234567890abcdef1234567890abcdef","readPurpose":"application_tab_preparation","token":token,"native":native.document(),"ui":ui(),"uiSha256":ui()["sha256"]});
    let reference = json!({"token":token,"applicationInstance":d["applicationInstance"],"scope":scope(),"projectPath":s.path,"sha256":digest(&bytes(&d,MAX_BYTES).unwrap())});
    let p = json!({"kind":"native_application_tab_preparation","version":1,"status":"prepared","reference":reference,"document":d});
    let publication = native.prepare_activation(s, item.0).unwrap().enter_publication(s).unwrap();
    s.execute("sequence.open", json!({"item":item.0})).unwrap();
    if unknown {
        s.execute("sequence.addEdit", json!({"time":254016000000_i64})).unwrap();
    }
    let receipt = publication.finish(s);
    let r = receipt.document();
    let o = if receipt.confirmed() {
        json!({"kind":"native_application_tab_change","version":1,"status":"activated","reference":reference,"operation":{"type":"activate_tab","item":item.0},"native":r,"beforeUi":ui(),"afterUi":ui(),"preparedUiSha256":ui()["sha256"],"receiptComplete":true,"canReplay":false})
    } else {
        json!({"kind":"native_application_tab_outcome","version":1,"status":"unknown","reference":reference,"item":item.0,"receiptComplete":false,"publicationStarted":true,"canReplay":false,"publicationEvidence":{"kind":"native_application_tab_publication_evidence","version":1,"native":r,"nativeReceiptConfirmed":false,"preparedUiSha256":ui()["sha256"],"beforeUi":ui(),"expectedAfterUi":ui(),"expectedNativeSha256":digest(&bytes(&r["expected"],MAX_BYTES).unwrap()),"actualUi":ui(),"actualUiError":null,"actualUiMatchesExpected":true,"humanServiceError":null}})
    };
    (p, o, receipt)
}
fn capture(s: &Session, p: &Value, o: &Value, r: &ApplicationTabReceipt) -> Result<ApplicationSnapshotCapture> {
    ApplicationSnapshotCapture::capture(s, &[ApplicationTabArchiveInput { preparation: p, outcome: o, receipt: r }])
}
fn request() -> Request {
    serde_json::from_value(json!({"token":format!("{:064x}",9000),"scope":scope(),"workspace":W,"checkpoint":C})).unwrap()
}
fn capsule(s: &Session) -> Capsule {
    pending_capsule(s, &request(), s.state.active_sequence).unwrap().encode().unwrap()
}
#[test]
fn live_attachment_requires_same_physical_session_even_for_equal_roots_and_epoch() {
    let (_root, mut s, other) = setup();
    let (p, o, r) = publish(&mut s, other, 1, false);
    assert!(capture(&s, &p, &o, &r).is_ok());
    let mut foreign = Session::default();
    foreign.project = s.project.clone();
    foreign.state = s.state.clone();
    foreign.history = s.history.clone();
    foreign.path = s.path.clone();
    foreign.revision = s.revision;
    foreign.saved_revision = s.saved_revision;
    foreign.agent_bridge.epoch = s.agent_bridge.epoch.clone();
    assert!(capture(&foreign, &p, &o, &r).is_err());
    assert!(capture(&s.display_projection(), &p, &o, &r).is_err());
    assert!(s.agent_bridge.application_history.is_empty());
}
#[test]
fn captured_state_revalidates_raw_clipboard_preferences_and_later_human_changes() {
    let (_root, mut s, other) = setup();
    let (p, o, r) = publish(&mut s, other, 2, false);
    let c = capture(&s, &p, &o, &r).unwrap();
    c.validate(&s).unwrap();
    s.prefs.media.enable_proxies = !s.prefs.media.enable_proxies;
    assert!(c.validate(&s).is_err());
    s.prefs.media.enable_proxies = !s.prefs.media.enable_proxies;
    c.validate(&s).unwrap();
    s.state.clipboard_markers.push(filmcraft_project::Marker {
        id: filmcraft_project::MarkerId(1),
        start: filmcraft_time::Tick(0),
        duration: filmcraft_time::Tick(0),
        name: "clipboard marker".into(),
        comment: String::new(),
        kind: filmcraft_project::MarkerKind::Comment,
        color: filmcraft_project::Label::Rose,
    });
    assert!(c.validate(&s).is_err());
}
#[test]
fn original_worker_saves_attachment_once_and_returns_same_owner_without_edit_or_replay() {
    let (_root, mut s, other) = setup();
    let (p, o, r) = publish(&mut s, other, 3, false);
    let worker = SessionWorker::new().unwrap();
    worker.install(&mut s).unwrap();
    let token = format!("{:064x}", 9000);
    s.execute("agent.readHistoryCheckpointBasis", json!({"token":token,"scope":scope()})).unwrap();
    let params = json!({"token":token,"scope":scope(),"workspace":W,"checkpoint":C,"metadataBasis":true});
    let c = capture(&s, &p, &o, &r).unwrap();
    let project = s.project.clone();
    let rev = s.revision;
    let mut pending = worker.start_with_application_capture(&mut s, SAVE, params, None, c).unwrap();
    assert!(s.is_display_only());
    let until = std::time::Instant::now() + std::time::Duration::from_secs(20);
    let done = loop {
        if let Some(v) = pending.try_finish() {
            break v;
        }
        assert!(std::time::Instant::now() < until);
        std::thread::yield_now();
    };
    assert!(done.reply.result.is_ok(), "{:?}", done.reply.result);
    let s = done.session;
    assert!(Arc::ptr_eq(&project, &s.project));
    assert_eq!(rev, s.revision);
    assert_eq!(s.agent_bridge.application_history.len(), 1);
    assert_eq!(capsule(&s).version, 9);
}
#[test]
fn ordinary_worker_gesture_guard_and_non_checkpoint_command_cannot_consume_capture() {
    let (_root, mut s, other) = setup();
    let (p, o, r) = publish(&mut s, other, 4, false);
    let worker = SessionWorker::new().unwrap();
    worker.install(&mut s).unwrap();
    let c = capture(&s, &p, &o, &r).unwrap();
    assert!(worker.start_with_application_capture(&mut s, "agent.apply", json!({}), None, c).is_err());
    assert!(!s.is_display_only());
    assert!(s.agent_bridge.application_history.is_empty());
    // Capture validation is independent of the original worker gesture check.
    let c = capture(&s, &p, &o, &r).unwrap();
    s.history.merge_key = Some("later-human-drag".into());
    assert!(worker.start_with_application_capture(&mut s, SAVE, json!({}), None, c).is_err());
    assert!(!s.is_display_only());
}
#[test]
fn capsule9_cold_pool_keeps_history_and_actual_uncertainty_as_readonly_data() {
    let (_root, mut s, other) = setup();
    let main = s.state.active_sequence.unwrap();
    let (p, o, r) = publish(&mut s, main, 5, true);
    assert!(!r.confirmed());
    capture(&s, &p, &o, &r).unwrap().install(&mut s).unwrap();
    let c = capsule(&s);
    assert_eq!(c.version, 9);
    assert_eq!(c.application.len(), 1);
    let mut cold = Session::default();
    cold.project = Arc::new(super::super::project(&c.projects[0]).unwrap());
    cold.path = s.path.clone();
    cold.state = s.state.clone();
    cold.saved_revision = cold.revision;
    let prepared = prepare_restore(&cold, &request(), &c, "", cold.state.active_sequence).unwrap();
    assert_eq!(prepared.application.len(), 1);
    assert_eq!(prepared.application[0].outcome, o);
    assert_eq!(prepared.application[0].history.proof().unwrap(), s.agent_bridge.application_history[0].history.proof().unwrap());
    for (a, b) in s.agent_bridge.application_history[0].history.projects().iter().zip(prepared.application[0].history.projects()) {
        assert!(!Arc::ptr_eq(a, b));
    }
    assert!(prepared.tickets.is_empty());
    assert!(cold.agent_bridge.tickets.is_empty());
    assert!(cold.agent_bridge.application_history.is_empty());
    let _ = other;
}
#[test]
fn archive_rejects_changed_preparation_outcome_hashes_and_collapsed_snapshot_mapping() {
    let (_root, mut s, other) = setup();
    let main = s.state.active_sequence.unwrap();
    let (p, o, r) = publish(&mut s, main, 6, true);
    for path in ["token", "applicationInstance", "sha256"] {
        let mut wrong = p.clone();
        wrong["reference"][path] = json!("forged");
        assert!(capture(&s, &wrong, &o, &r).is_err());
    }
    let mut wrong = o.clone();
    wrong["publicationEvidence"]["native"]["canReplay"] = json!(true);
    assert!(capture(&s, &p, &wrong, &r).is_err());
    capture(&s, &p, &o, &r).unwrap().install(&mut s).unwrap();
    let c = capsule(&s);
    let pool = s.agent_bridge.application_history[0].history.projects();
    let mut saved = c.application.clone();
    assert!(saved[0].snapshots.len() > 1);
    saved[0].snapshots[1] = saved[0].snapshots[0];
    assert!(decode(&saved, pool, &mut Default::default()).is_err());
    let _ = other;
}
#[test]
fn history_never_drops_retained_records_and_conflicting_repeat_is_not_an_upgrade() {
    let (_root, mut s, other) = setup();
    let (p, o, r) = publish(&mut s, other, 7, false);
    capture(&s, &p, &o, &r).unwrap().install(&mut s).unwrap();
    capture(&s, &p, &o, &r).unwrap().install(&mut s).unwrap();
    assert_eq!(s.agent_bridge.application_history.len(), 1);
    ApplicationSnapshotCapture::capture(&s, &[]).unwrap().install(&mut s).unwrap();
    assert_eq!(s.agent_bridge.application_history.len(), 1);
    let mut wrong = o.clone();
    wrong["extra"] = json!("cannot replace existing evidence");
    assert!(capture(&s, &p, &wrong, &r).is_err());
}
#[test]
fn capsule9_requires_nonempty_data_and_cannot_masquerade_as_legacy_capsule() {
    let (_root, mut s, other) = setup();
    let (p, o, r) = publish(&mut s, other, 8, false);
    capture(&s, &p, &o, &r).unwrap().install(&mut s).unwrap();
    let c = capsule(&s);
    let mut cold = Session::default();
    cold.project = Arc::new(super::super::project(&c.projects[0]).unwrap());
    cold.path = s.path.clone();
    cold.state = s.state.clone();
    for version in [1, 2, 3, 4, 5, 6, 7, 8, 10] {
        let mut bad = c.clone();
        bad.version = version;
        assert!(prepare_restore(&cold, &request(), &bad, "", cold.state.active_sequence).is_err());
    }
    let mut bad = c.clone();
    bad.application.clear();
    assert!(prepare_restore(&cold, &request(), &bad, "", cold.state.active_sequence).is_err());
}

fn readonly_state(s: &Session) -> Value {
    json!({"project":s.project,"editor":s.state,"undo":s.history.undo,"redo":s.history.redo,
        "limit":s.history.limit,"revision":s.revision,"saved":s.saved_revision,"path":s.path,
        "epoch":s.agent_bridge.epoch,"tickets":s.agent_bridge.tickets.len(),
        "records":s.agent_bridge.application_history.len(),"journal":s.journal})
}
#[test]
fn archived_lookup_is_readonly_exact_and_correlates_original_outcome() {
    for unknown in [false, true] {
        let (_root, mut s, other) = setup();
        let (p, o, r) = publish(&mut s, other, 9100, unknown);
        capture(&s, &p, &o, &r).unwrap().install(&mut s).unwrap();
        let before = readonly_state(&s);
        let project = s.project.clone();
        let got = s.archived_application_tab(&p["reference"]).unwrap().unwrap();
        assert_eq!(got["kind"], "native_archived_application_tab");
        assert_eq!(got["outcomeSha256"], digest(&bytes(&o, MAX_BYTES).unwrap()));
        assert_eq!(got["outcomeStatus"], o["status"]);
        assert_eq!(got["nativeSnapshotComplete"], s.agent_bridge.application_history[0].history.native_snapshot_complete());
        assert_eq!(got["actualSnapshotComplete"], s.agent_bridge.application_history[0].history.actual_snapshot_complete());
        assert_eq!(got["current"]["native"], crate::agent_bridge::application_tab_scope::Snapshot::current_metadata(&s).unwrap());
        assert_eq!(got["sourceValidation"], "not-performed");
        for flag in ["mediaEvidence", "writeAuthority", "requestReplay"] {
            assert_eq!(got[flag], false);
        }
        let mut absent = p["reference"].clone();
        absent["token"] = json!(format!("{:064x}", 9101));
        assert!(s.archived_application_tab(&absent).unwrap().is_none());
        assert_eq!(readonly_state(&s), before);
        assert!(Arc::ptr_eq(&project, &s.project));
    }
}
#[test]
fn archived_lookup_decodes_original_capsule_data_without_recreating_live_scopes() {
    let (_root, mut s, other) = setup();
    let (p, o, r) = publish(&mut s, other, 9200, true);
    capture(&s, &p, &o, &r).unwrap().install(&mut s).unwrap();
    let c = capsule(&s);
    let mut cold = Session::default();
    cold.project = Arc::new(super::super::project(&c.projects[0]).unwrap());
    cold.path = s.path.clone();
    cold.state = s.state.clone();
    cold.saved_revision = cold.revision;
    let prepared = prepare_restore(&cold, &request(), &c, "", cold.state.active_sequence).unwrap();
    assert!(prepared.tickets.is_empty());
    // Codec-only unit coverage. Real loader/normal-constructor coverage lives
    // in ui-egui; assigning decoded data here does not assert cold admission.
    cold.history = prepared.history;
    cold.agent_bridge.application_history = prepared.application;
    cold.agent_operation_control().enable_transport();
    cold.execute("agent.readHistoryCheckpointBasis", json!({"token":format!("{:064x}",9201),"scope":scope()})).unwrap();
    assert!(cold.agent_bridge.tickets.is_empty());
    let before = readonly_state(&cold);
    let got = cold.archived_application_tab(&p["reference"]).unwrap().unwrap();
    assert_eq!(got["outcomeSha256"], digest(&bytes(&o, MAX_BYTES).unwrap()));
    assert_ne!(got["current"]["native"]["nativeSession"], p["document"]["native"]["nativeSession"]);
    assert_eq!(got["writeAuthority"], false);
    assert_eq!(readonly_state(&cold), before);
    assert!(cold.agent_bridge.tickets.is_empty());
}
#[test]
fn archived_lookup_refuses_foreign_malformed_and_worker_projection_but_allows_current_save_as_path() {
    let (root, mut s, other) = setup();
    let (p, o, r) = publish(&mut s, other, 9300, false);
    capture(&s, &p, &o, &r).unwrap().install(&mut s).unwrap();
    let before = readonly_state(&s);
    for key in ["token", "applicationInstance", "scope", "projectPath", "sha256"] {
        let mut bad = p["reference"].clone();
        bad[key] = json!("invalid");
        assert!(s.archived_application_tab(&bad).is_err(), "{key}");
    }
    let mut extra = p["reference"].clone();
    extra["unknown"] = json!(true);
    assert!(s.archived_application_tab(&extra).is_err());
    let mut large = p["reference"].clone();
    large["projectPath"] = json!(format!("/{}.fcproj", "x".repeat(4096)));
    assert!(s.archived_application_tab(&large).is_err());
    assert!(s.display_projection().archived_application_tab(&p["reference"]).is_err());
    assert_eq!(readonly_state(&s), before);
    let target = root.0.join("human-save-as.fcproj");
    s.execute("file.saveAs", json!({"path":target})).unwrap();
    let before = readonly_state(&s);
    let got = s.archived_application_tab(&p["reference"]).unwrap().unwrap();
    assert_eq!(got["reference"]["projectPath"], p["reference"]["projectPath"]);
    assert_eq!(got["current"]["native"]["projectPath"], target.to_str().unwrap());
    assert_ne!(got["reference"]["projectPath"], got["current"]["native"]["projectPath"]);
    assert_eq!(readonly_state(&s), before);
}

#[test]
fn archived_record_pages_reassemble_exact_original_pair_and_keep_unknown_uncertain() {
    for uncertain in [false, true] {
        let (_root, mut s, other) = setup();
        let (p, o, r) = publish(&mut s, other, 9700, uncertain);
        capture(&s, &p, &o, &r).unwrap().install(&mut s).unwrap();
        let before = readonly_state(&s);
        let original = bytes(&(&p, &o), MAX_BYTES).unwrap();
        let expected_sha = digest(&original);
        let mut recovered = Vec::new();
        let mut offset = 0;
        loop {
            let v = s.archived_application_tab_record_page(&p["reference"], offset, 257).unwrap().unwrap();
            assert_eq!(v["sha256"], expected_sha);
            assert_eq!(v["bytes"], original.len());
            assert_eq!(v["offset"], offset);
            assert_eq!(v["archive"]["outcomeStatus"], o["status"]);
            assert_eq!(v["archive"]["outcomeSha256"], digest(&bytes(&o, MAX_BYTES).unwrap()));
            for flag in ["writeAuthority", "requestReplay", "mediaEvidence"] {
                assert_eq!(v[flag], false);
            }
            let data = v["data"].as_str().unwrap();
            assert!(data.len() <= 2 * 257);
            for pair in data.as_bytes().chunks_exact(2) {
                recovered.push(u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap());
            }
            match v["nextOffset"].as_u64() {
                Some(next) => {
                    assert_eq!(next as usize, recovered.len());
                    offset = next as usize;
                }
                None => break,
            }
        }
        assert_eq!(recovered, original);
        let pair: Value = serde_json::from_slice(&recovered).unwrap();
        assert_eq!(pair, json!([p, o]));
        assert_eq!(readonly_state(&s), before);
    }
}
#[test]
fn archived_record_page_bounds_identity_and_projection_refusals_leave_owner_unchanged() {
    let (_root, mut s, other) = setup();
    let (p, o, r) = publish(&mut s, other, 9800, false);
    capture(&s, &p, &o, &r).unwrap().install(&mut s).unwrap();
    let before = readonly_state(&s);
    let len = bytes(&(&p, &o), MAX_BYTES).unwrap().len();
    for (offset, limit) in [(0, 0), (0, 1024 * 1024 + 1), (usize::MAX, 1), (len + 1, 1)] {
        assert!(s.archived_application_tab_record_page(&p["reference"], offset, limit).is_err());
    }
    let eof = s.archived_application_tab_record_page(&p["reference"], len, 1).unwrap().unwrap();
    assert_eq!(eof["data"], "");
    assert!(eof["nextOffset"].is_null());
    let mut foreign = p["reference"].clone();
    foreign["scope"]["projectId"] = json!("p_foreign");
    assert!(s.archived_application_tab_record_page(&foreign, 0, 1024).is_err());
    assert!(s.display_projection().archived_application_tab_record_page(&p["reference"], 0, 1024).is_err());
    assert_eq!(readonly_state(&s), before);
}
#[test]
fn archived_record_pages_survive_original_capsule_decode_without_live_scopes() {
    let (_root, mut s, other) = setup();
    let (p, o, r) = publish(&mut s, other, 9900, true);
    capture(&s, &p, &o, &r).unwrap().install(&mut s).unwrap();
    let c = capsule(&s);
    let mut cold = Session::default();
    cold.project = Arc::new(super::super::project(&c.projects[0]).unwrap());
    cold.path = s.path.clone();
    cold.state = s.state.clone();
    cold.saved_revision = cold.revision;
    let prepared = prepare_restore(&cold, &request(), &c, "", cold.state.active_sequence).unwrap();
    assert!(prepared.tickets.is_empty());
    cold.history = prepared.history;
    cold.agent_bridge.application_history = prepared.application;
    cold.agent_operation_control().enable_transport();
    cold.execute("agent.readHistoryCheckpointBasis", json!({"token":format!("{:064x}",9901),"scope":scope()})).unwrap();
    let before = readonly_state(&cold);
    let got = cold.archived_application_tab_record_page(&p["reference"], 0, 1024 * 1024).unwrap().unwrap();
    assert_eq!(got["sha256"], digest(&bytes(&(&p, &o), MAX_BYTES).unwrap()));
    assert_eq!(got["archive"]["outcomeStatus"], "unknown");
    assert_ne!(got["archive"]["current"]["native"]["nativeSession"], p["document"]["native"]["nativeSession"]);
    assert!(cold.agent_bridge.tickets.is_empty());
    assert_eq!(readonly_state(&cold), before);
}

fn save_attestation_checkpoint(s: &mut Session, checkpoint: &str, n: u64) -> (Value, Value) {
    let token = format!("{n:064x}");
    let basis = s.execute("agent.readHistoryCheckpointBasis", json!({"token":token,"scope":scope()})).unwrap();
    let save = s.execute(SAVE, json!({"token":token,"scope":scope(),"workspace":W,"checkpoint":checkpoint,"metadataBasis":true})).unwrap();
    (save["checkpoint"].clone(), json!({"nativeSession":basis["nativeSession"],"revision":basis["revision"],"sequence":basis["sequence"]}))
}
fn attestation_request(s: &mut Session, p: &Value, metadata: &Value, basis: &Value, n: u64) -> Value {
    let token = format!("{n:064x}");
    s.execute("agent.readHistoryCheckpointBasis", json!({"token":token,"scope":scope()})).unwrap();
    json!({"token":token,"scope":scope(),"workspace":W,"checkpoint":metadata["checkpoint"],"sha256":metadata["sha256"],"basis":basis,"reference":p["reference"]})
}
#[test]
fn saved_checkpoint_record_attestation_uses_actual_companion_not_live_archive_and_grants_no_authority() {
    let (_root, mut s, other) = setup();
    let (p, o, receipt) = publish(&mut s, other, 15001, false);
    capture(&s, &p, &o, &receipt).unwrap().install(&mut s).unwrap();
    let (metadata, basis) = save_attestation_checkpoint(&mut s, C, 15002);
    s.agent_bridge.application_history.clear();
    let params = attestation_request(&mut s, &p, &metadata, &basis, 15003);
    let project = s.project.clone();
    let revision = s.revision;
    let state = serde_json::to_value(&s.state).unwrap();
    let reply = s.execute(CHECKPOINT_INSPECTION, params).unwrap();
    let raw = bytes(&(&p, &o), MAX_BYTES).unwrap();
    assert_eq!(reply["checkpoint"], metadata);
    assert_eq!(reply["basis"], basis);
    assert_eq!(reply["record"]["carrier"]["sha256"], digest(&raw));
    assert_eq!(reply["record"]["carrier"]["bytes"], raw.len());
    assert_eq!(reply["record"]["reference"], p["reference"]);
    assert_eq!(reply["record"]["outcomeSha256"], digest(&bytes(&o, MAX_BYTES).unwrap()));
    assert_eq!(reply["record"]["outcomeStatus"], "activated");
    assert!(reply["applicationUi"].is_null());
    assert_eq!(reply["historyValidation"], "validated_archive_only");
    assert_eq!(reply["writeAuthority"], false);
    assert_eq!(reply["requestReplay"], false);
    assert_eq!(reply["mediaEvidence"], false);
    assert_eq!(reply["execution"], "not-applied");
    assert!(Arc::ptr_eq(&project, &s.project));
    assert_eq!(revision, s.revision);
    assert_eq!(state, serde_json::to_value(&s.state).unwrap());
    assert!(s.agent_bridge.application_history.is_empty());
}
#[test]
fn checkpoint_record_attestation_refuses_old_companion_crossed_basis_scope_hash_and_hostile_fields() {
    let (_root, mut s, other) = setup();
    let (old, old_basis) = save_attestation_checkpoint(&mut s, C, 15100);
    let (p, o, receipt) = publish(&mut s, other, 15101, false);
    capture(&s, &p, &o, &receipt).unwrap().install(&mut s).unwrap();
    let c2 = "77777777-7777-4777-8777-777777777777";
    let (saved, basis) = save_attestation_checkpoint(&mut s, c2, 15102);
    let params = attestation_request(&mut s, &p, &old, &old_basis, 15103);
    assert!(s.execute(CHECKPOINT_INSPECTION, params).is_err());
    let before = serde_json::to_value(&s.state).unwrap();
    let project = s.project.clone();
    for i in 0..9 {
        let mut params = attestation_request(&mut s, &p, &saved, &basis, 15110 + i);
        match i {
            0 => params["sha256"] = json!("0".repeat(64)),
            1 => params["basis"]["nativeSession"] = json!("f".repeat(32)),
            2 => {
                params["basis"].as_object_mut().unwrap().remove("sequence");
            }
            3 => params["scope"]["projectId"] = json!("p_foreign"),
            4 => params["reference"]["sha256"] = json!("f".repeat(64)),
            5 => params["reference"]["extra"] = json!(true),
            6 => params["extra"] = json!(true),
            7 => params["reference"]["projectPath"] = json!("../foreign.fcproj"),
            _ => params["reference"]["token"] = json!("x".repeat(40 * 1024)),
        }
        assert!(s.execute(CHECKPOINT_INSPECTION, params).is_err(), "hostile case {i}");
    }
    assert_eq!(before, serde_json::to_value(&s.state).unwrap());
    assert!(Arc::ptr_eq(&project, &s.project));
    assert_eq!(s.agent_bridge.application_history.len(), 1);
}
#[test]
fn checkpoint_record_attestation_keeps_real_native_uncertainty_and_missing_passive_ui_explicit() {
    let (_root, mut s, _other) = setup();
    let main = s.state.active_sequence.unwrap();
    let (p, o, receipt) = publish(&mut s, main, 15201, true);
    assert!(!receipt.confirmed());
    capture(&s, &p, &o, &receipt).unwrap().install(&mut s).unwrap();
    let original = s.archived_application_tab(&p["reference"]).unwrap().unwrap();
    let (metadata, basis) = save_attestation_checkpoint(&mut s, C, 15202);
    let params = attestation_request(&mut s, &p, &metadata, &basis, 15203);
    let reply = s.execute(CHECKPOINT_INSPECTION, params).unwrap();
    assert_eq!(reply["record"]["outcomeStatus"], "unknown");
    for key in ["nativeSnapshotComplete", "actualSnapshotComplete", "actualUiCaptured"] {
        assert_eq!(reply["record"][key], original[key]);
    }
    assert!(reply["applicationUi"].is_null());
    assert_eq!(reply["record"]["outcomeSha256"], digest(&bytes(&o, MAX_BYTES).unwrap()));
    assert_eq!(reply["writeAuthority"], false);
    assert_eq!(reply["requestReplay"], false);
    assert_eq!(s.agent_bridge.application_history[0].outcome, o);
}
#[test]
fn checkpoint_record_attestation_runs_on_original_worker_and_rejects_display_projection() {
    let (_root, mut s, other) = setup();
    let (p, o, receipt) = publish(&mut s, other, 15301, false);
    capture(&s, &p, &o, &receipt).unwrap().install(&mut s).unwrap();
    let (metadata, basis) = save_attestation_checkpoint(&mut s, C, 15302);
    let worker = SessionWorker::new().unwrap();
    worker.install(&mut s).unwrap();
    let params = attestation_request(&mut s, &p, &metadata, &basis, 15303);
    let project = s.project.clone();
    let revision = s.revision;
    assert!(crate::session_worker::supports(CHECKPOINT_INSPECTION));
    let mut pending = worker.start(&mut s, CHECKPOINT_INSPECTION, params.clone(), None).unwrap();
    assert!(s.is_display_only());
    assert!(s.execute(CHECKPOINT_INSPECTION, params).is_err());
    let until = std::time::Instant::now() + std::time::Duration::from_secs(20);
    let done = loop {
        if let Some(v) = pending.try_finish() {
            break v;
        }
        assert!(std::time::Instant::now() < until);
        std::thread::yield_now();
    };
    let reply = done.reply.result.unwrap();
    let s = done.session;
    assert_eq!(reply["checkpoint"], metadata);
    assert_eq!(reply["record"]["outcomeStatus"], "activated");
    assert!(Arc::ptr_eq(&project, &s.project));
    assert_eq!(revision, s.revision);
    assert!(!s.is_display_only());
    assert_eq!(s.agent_bridge.application_history.len(), 1);
}

#[test]
fn checkpoint_record_attestation_on_same_original_paused_root_worker_preserves_lifecycle_fence() {
    let (_root, mut s, other) = setup();
    let worker = SessionWorker::new().unwrap();
    worker.install(&mut s).unwrap();
    let (p, o, receipt) = publish(&mut s, other, 15401, false);
    capture(&s, &p, &o, &receipt).unwrap().install(&mut s).unwrap();
    let (metadata, basis) = save_attestation_checkpoint(&mut s, C, 15402);
    let pause_token = format!("{:064x}", 15403);
    s.execute("agent.readHistoryCheckpointBasis", json!({"token":pause_token,"scope":scope()})).unwrap();
    let held = s
        .execute("agent.beginWorkspaceUpdate", json!({"token":pause_token,"scope":scope(),"workspace":W,"update":"22222222-2222-4222-8222-222222222222"}))
        .unwrap();
    assert_eq!(held["status"], "paused");
    assert_eq!(held["role"], "owner");
    assert!(s.is_display_only());
    assert!(s.workspace_update_allows(CHECKPOINT_INSPECTION));
    assert!(!s.workspace_update_allows("sequence.open"));
    assert!(!s.workspace_update_allows("agent.apply"));
    let params = attestation_request(&mut s, &p, &metadata, &basis, 15404);
    let project = s.project.clone();
    let revision = s.revision;
    let state = serde_json::to_value(&s.state).unwrap();
    let mut pending = worker.start(&mut s, CHECKPOINT_INSPECTION, params, None).unwrap();
    let until = std::time::Instant::now() + std::time::Duration::from_secs(20);
    let done = loop {
        if let Some(v) = pending.try_finish() {
            break v;
        }
        assert!(std::time::Instant::now() < until);
        std::thread::yield_now();
    };
    let reply = done.reply.result.unwrap();
    let s = done.session;
    assert_eq!(reply["checkpoint"], metadata);
    assert_eq!(reply["record"]["outcomeStatus"], "activated");
    assert!(s.is_display_only());
    assert!(!s.workspace_update_allows("sequence.open"));
    assert_eq!(s.workspace_update_status().unwrap()["status"], "paused");
    assert_eq!(s.workspace_update_status().unwrap()["role"], "owner");
    assert!(Arc::ptr_eq(&project, &s.project));
    assert_eq!(revision, s.revision);
    assert_eq!(state, serde_json::to_value(&s.state).unwrap());
}
