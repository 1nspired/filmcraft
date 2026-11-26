//! Generated saved projects only; graph restoration never replays a recipe.
use super::{project_plane, tests::token};
use crate::Session;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    fs,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};
const W: &str = "f57e1332-1bb3-4cdf-a70a-9ecbe8a4d3ac";
const C: &str = "c52cae9c-7a18-44a1-82d9-5e76e715bf2b";
const SAVE: &str = "agent.saveHistoryCheckpoint";
const RESTORE: &str = "agent.restoreHistoryCheckpoint";
const RETAIN: &str = "agent.validateRetainedHistory";
struct Scratch(PathBuf);
impl Scratch {
    fn new() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let p = crate::temp_dir().join(format!("filmcraft-project-history-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
        fs::create_dir(&p).unwrap();
        Self(fs::canonicalize(p).unwrap())
    }
    fn companion(&self) -> PathBuf {
        self.0.join(format!(".poema-history-{W}-{C}.json"))
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn scope() -> Value {
    json!({"projectId":"p_project_history","workspaceId":W})
}
fn setup() -> (Scratch, Session) {
    let d = Scratch::new();
    let mut s = Session::default();
    s.execute("file.newProject", json!({"name":"Generated project history","workspace":scope()})).unwrap();
    s.path = Some(d.0.join("generated.fcproj").to_str().unwrap().into());
    persist(&mut s);
    (d, s)
}
fn persist(s: &mut Session) {
    filmcraft_format::atomic_write(Path::new(s.path.as_ref().unwrap()), &filmcraft_format::encode_with_view(&s.project, Some(&s.project_view()), false))
        .unwrap();
    s.saved_revision = s.revision;
}
fn reopen(s: &Session) -> Session {
    let mut f = Session::default();
    f.execute("file.open", json!({"path":s.path})).unwrap();
    f
}
fn state(s: &Session) -> Value {
    json!({"project":s.project,"editor":s.state,"path":s.path,"revision":s.revision,"saved":s.saved_revision,"undo":s.history.undo,"redo":s.history.redo,"merge":s.history.merge_key,"events":s.events.len(),"journal":s.journal})
}
fn read(s: &mut Session, n: usize) -> Value {
    s.execute(project_plane::READ, json!({"token":token(n),"scope":scope(),"projectPath":s.path})).unwrap()
}
fn request(n: usize) -> Value {
    json!({"token":token(n),"scope":scope(),"workspace":W,"checkpoint":C})
}
fn batch(n: usize, command: &str, params: Value) -> Value {
    json!({"token":token(n),"label":"Generated exact metadata","exact":true,"authority":"document_state","operations":[{"command":command,"params":params}]})
}
fn bin(s: &mut Session, n: usize) -> Value {
    let r = read(s, n);
    s.execute(
        "agent.apply",
        batch(
            n,
            "agent.organizeProject",
            json!({"scope":scope(),"beforeSha256":r["binState"]["sha256"],"edit":{"type":"create","name":"Generated bin","parent":null}}),
        ),
    )
    .unwrap()
}
fn first(s: &mut Session, n: usize) -> Value {
    let r = read(s, n);
    s.execute("agent.apply",batch(n,project_plane::CREATE,json!({"scope":scope(),"beforeSha256":r["projectState"]["sha256"],"sequence":{"name":"First","width":3840,"height":2160,"frameRate":{"num":48001,"den":2000},"sampleRate":96000,"mix":"Mono","trackType":"Mono","video":2,"audio":4}}))).unwrap()
}
fn save(s: &mut Session, n: usize) -> Value {
    persist(s);
    read(s, n);
    let before = state(s);
    let r = s.execute(SAVE, request(n)).unwrap();
    assert_eq!(state(s), before);
    r
}
fn restore(s: &mut Session, n: usize, saved: &Value) -> Value {
    read(s, n);
    let mut r = request(n);
    r["sha256"] = saved["checkpoint"]["sha256"].clone();
    r
}
fn rejected(s: &mut Session, command: &str, r: Value) {
    let before = state(s);
    let project = s.project.clone();
    let undo = s.history.undo.clone();
    let redo = s.history.redo.clone();
    assert!(s.execute(command, r).is_err(), "{command}");
    assert_eq!(state(s), before);
    assert!(Arc::ptr_eq(&project, &s.project));
    for (a, b) in [(&undo, &s.history.undo), (&redo, &s.history.redo)] {
        assert_eq!(a.len(), b.len());
        assert!(a.iter().zip(b).all(|((l, p), (m, q))| l == m && Arc::ptr_eq(p, q)));
    }
}
fn undo(s: &mut Session, n: usize, commit: usize) {
    read(s, n);
    assert_eq!(s.execute("agent.readHistory", json!({"token":token(n),"scope":scope(),"commit":token(commit),"undo":null})).unwrap()["undoAvailable"], true);
    s.execute("agent.undo", json!({"token":token(n),"commit":token(commit)})).unwrap();
}
fn redo(s: &mut Session, n: usize, commit: usize, u: usize) {
    read(s, n);
    assert_eq!(
        s.execute("agent.readHistory", json!({"token":token(n),"scope":scope(),"commit":token(commit),"undo":token(u)})).unwrap()["redoAvailable"],
        true
    );
    s.execute("agent.redo", json!({"token":token(n),"commit":token(commit),"undo":token(u)})).unwrap();
}

#[test]
fn project_history_cold_restore_before_any_sequence_preserves_owned_bin_and_prior_human_step() {
    let (d, mut s) = setup();
    s.execute("file.newBin", json!({"name":"Earlier human work"})).unwrap();
    let before = s.project.clone();
    bin(&mut s, 92000);
    let after = s.project.clone();
    let saved = save(&mut s, 92001);
    let raw: Value = serde_json::from_slice(&fs::read(d.companion()).unwrap()).unwrap();
    assert_eq!(raw["version"], 2);
    assert_eq!(raw["scope"], scope());
    assert_eq!(raw["sequence"], Value::Null);
    assert_eq!(raw["tickets"][0]["sequence"], Value::Null);
    assert_eq!(raw["tickets"][0]["project_scope"], scope());
    let mut f = reopen(&s);
    let current = f.project.clone();
    let view = f.state.clone();
    let r = restore(&mut f, 92002, &saved);
    f.execute(RESTORE, r.clone()).unwrap();
    assert!(Arc::ptr_eq(&current, &f.project));
    assert_eq!(serde_json::to_value(&f.state).unwrap(), serde_json::to_value(&view).unwrap());
    let stable = state(&f);
    f.execute(RESTORE, r).unwrap();
    assert_eq!(state(&f), stable);
    undo(&mut f, 92003, 92000);
    assert_eq!(f.project, before);
    assert!(f.state.active_sequence.is_none());
    redo(&mut f, 92004, 92000, 92003);
    assert_eq!(f.project, after);
    undo(&mut f, 92005, 92000);
    f.execute("edit.undo", json!({})).unwrap();
    assert!(f.project.root.children.is_empty());
}
#[test]
fn project_history_cold_first_sequence_undo_restores_absence_and_redo_the_original_anchor() {
    let (_d, mut s) = setup();
    let before = s.project.clone();
    let result = first(&mut s, 92100);
    let after = s.project.clone();
    let saved = save(&mut s, 92101);
    let mut f = reopen(&s);
    f.execute("sequence.close", json!({"item":result["sequence"]})).unwrap();
    assert!(f.state.active_sequence.is_none());
    let r = restore(&mut f, 92102, &saved);
    f.execute(RESTORE, r).unwrap();
    assert!(f.state.active_sequence.is_none());
    undo(&mut f, 92103, 92100);
    assert_eq!(f.project, before);
    assert!(f.state.active_sequence.is_none());
    redo(&mut f, 92104, 92100, 92103);
    assert_eq!(f.project, after);
    assert_eq!(f.state.active_sequence.unwrap().0, result["sequence"].as_u64().unwrap());
}
#[test]
fn project_history_cold_saved_redo_without_a_sequence_restores_complete_graph_and_receipts() {
    let (_d, mut s) = setup();
    let created = first(&mut s, 92200);
    let after = s.project.clone();
    undo(&mut s, 92201, 92200);
    let saved = save(&mut s, 92202);
    let mut f = reopen(&s);
    assert!(f.state.active_sequence.is_none());
    let r = restore(&mut f, 92203, &saved);
    f.execute(RESTORE, r).unwrap();
    redo(&mut f, 92204, 92200, 92201);
    assert_eq!(f.project, after);
    assert_eq!(f.state.active_sequence.unwrap().0, created["sequence"].as_u64().unwrap());
}
#[test]
fn project_history_saved_metadata_undo_redo_graph_survives_another_cold_restore() {
    let (_d, mut s) = setup();
    bin(&mut s, 92300);
    undo(&mut s, 92301, 92300);
    redo(&mut s, 92302, 92300, 92301);
    let saved = save(&mut s, 92303);
    let mut f = reopen(&s);
    let r = restore(&mut f, 92304, &saved);
    f.execute(RESTORE, r).unwrap();
    undo(&mut f, 92305, 92300);
    assert!(f.project.root.children.is_empty());
}
#[test]
fn project_history_private_basis_has_no_read_media_edit_or_restore_authority_and_writes_exclusively() {
    let (d, mut s) = setup();
    bin(&mut s, 92400);
    persist(&mut s);
    let count = s.agent_bridge.tickets.len();
    let basis = s.execute("agent.readHistoryCheckpointBasis", json!({"token":token(92401),"scope":scope()})).unwrap();
    assert_eq!(basis["kind"], "native_project_history_checkpoint_basis");
    assert_eq!(basis["sequence"], Value::Null);
    assert_eq!(basis["sourceValidation"], "not-performed");
    assert_eq!(basis["writeAuthority"], false);
    assert_eq!(s.agent_bridge.tickets.len(), count);
    let mut r = request(92401);
    r["metadataBasis"] = json!(true);
    let saved = s.execute(SAVE, r.clone()).unwrap();
    let raw = fs::read(d.companion()).unwrap();
    assert_eq!(s.agent_operation_control().progress(&token(92401))["bytes_read"], "0");
    r["sha256"] = saved["checkpoint"]["sha256"].clone();
    rejected(&mut s, RESTORE, r);
    rejected(&mut s, "agent.validateProjectRead", json!({"token":token(92401),"scope":scope()}));
    read(&mut s, 92402);
    rejected(&mut s, SAVE, request(92402));
    assert_eq!(fs::read(d.companion()).unwrap(), raw);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(fs::metadata(d.companion()).unwrap().permissions().mode() & 0o077, 0);
    }
}
#[test]
fn project_history_same_session_preserves_cow_and_refuses_equal_replacement_or_later_human_history() {
    let (_d, mut s) = setup();
    bin(&mut s, 92500);
    let saved = save(&mut s, 92501);
    let r = restore(&mut s, 92502, &saved);
    assert_eq!(s.execute(RETAIN, r).unwrap()["status"], "retained");
    s.project = Arc::new((*s.project).clone());
    let r = restore(&mut s, 92503, &saved);
    rejected(&mut s, RESTORE, r);
    let mut f = reopen(&s);
    f.execute("file.newBin", json!({"name":"Human intervened"})).unwrap();
    let r = restore(&mut f, 92504, &saved);
    rejected(&mut f, RESTORE, r);
}
#[test]
fn project_history_current_human_tab_is_preserved_instead_of_restoring_a_saved_view() {
    let (_d, mut s) = setup();
    let a = s.execute("file.newSequence", json!({"name":"A"})).unwrap()["sequence"].clone();
    let b = s.execute("file.newSequence", json!({"name":"B"})).unwrap()["sequence"].clone();
    let before = s.project.clone();
    bin(&mut s, 92600);
    let saved = save(&mut s, 92601);
    let mut f = reopen(&s);
    f.execute("sequence.open", json!({"item":a})).unwrap();
    let r = restore(&mut f, 92602, &saved);
    f.execute(RESTORE, r).unwrap();
    assert_eq!(f.state.active_sequence.unwrap().0, a.as_u64().unwrap());
    undo(&mut f, 92603, 92600);
    assert_eq!(f.project, before);
    assert_eq!(f.state.active_sequence.unwrap().0, a.as_u64().unwrap());
    f.execute("sequence.open", json!({"item":b})).unwrap();
    redo(&mut f, 92604, 92600, 92603);
    assert_eq!(f.state.active_sequence.unwrap().0, b.as_u64().unwrap());
}
#[test]
fn project_history_tampered_capsule_versions_scope_receipts_and_nullable_fields_refuse_atomically() {
    let (d, mut s) = setup();
    bin(&mut s, 92700);
    let saved = save(&mut s, 92701);
    let raw = fs::read(d.companion()).unwrap();
    let original: Value = serde_json::from_slice(&raw).unwrap();
    for k in 0..10 {
        let mut v = original.clone();
        match k {
            0 => v["version"] = json!(1),
            1 => v["scope"]["workspaceId"] = json!(C),
            2 => {
                v.as_object_mut().unwrap().remove("sequence");
            }
            3 => {
                v["tickets"][0].as_object_mut().unwrap().remove("after_sequence");
            }
            4 => v["tickets"][0]["project_scope"] = Value::Null,
            5 => v["tickets"][0]["result"]["authority"] = json!("media_inputs"),
            6 => v["tickets"][0]["result"]["action"] = json!("cut"),
            7 => v["tickets"][0]["result"]["results"][0]["afterSha256"] = json!("f".repeat(64)),
            8 => v["tickets"][0]["result"]["results"][0]["humanCommand"] = json!("timeline.trim"),
            _ => v["tickets"][0]["after_sequence"] = json!(0),
        };
        let changed = serde_json::to_vec(&v).unwrap();
        fs::write(d.companion(), &changed).unwrap();
        let mut f = reopen(&s);
        let mut r = restore(&mut f, 92710 + k, &saved);
        r["sha256"] = json!(format!("{:x}", Sha256::digest(&changed)));
        rejected(&mut f, RESTORE, r);
        assert_eq!(fs::read(d.companion()).unwrap(), changed);
    }
}
#[test]
fn project_history_metadata_receipt_cannot_hide_unrelated_project_payload_changes() {
    let (d, mut s) = setup();
    bin(&mut s, 92800);
    let saved = save(&mut s, 92801);
    let mut v: Value = serde_json::from_slice(&fs::read(d.companion()).unwrap()).unwrap();
    let before = v["tickets"][0]["project"].as_u64().unwrap() as usize;
    let mut p: Value = serde_json::from_str(v["projects"][before].as_str().unwrap()).unwrap();
    p["name"] = json!("Forged different earlier project");
    v["projects"][before] = json!(serde_json::to_string(&p).unwrap());
    let raw = serde_json::to_vec(&v).unwrap();
    fs::write(d.companion(), &raw).unwrap();
    let mut f = reopen(&s);
    let mut r = restore(&mut f, 92802, &saved);
    r["sha256"] = json!(format!("{:x}", Sha256::digest(&raw)));
    rejected(&mut f, RESTORE, r);
}
#[test]
fn project_history_stop_before_checkpoint_publication_preserves_edit_and_creates_no_companion() {
    let (d, mut s) = setup();
    bin(&mut s, 92900);
    read(&mut s, 92901);
    let before = state(&s);
    let control = s.agent_operation_control();
    let r = request(92901);
    let run = super::history_checkpoint::commands().into_iter().find(|c| c.id == SAVE).unwrap().run;
    assert!(
        super::operation_control::run(&mut s, SAVE, &r, |s, p| {
            assert_eq!(control.cancel(&json!({"token":token(92901)})).unwrap()["cancellationAccepted"], true);
            run(s, p)
        })
        .is_err()
    );
    assert_eq!(state(&s), before);
    assert!(!d.companion().exists());
    rejected(&mut s, SAVE, r);
}
#[test]
fn project_history_legacy_sequence_scope_cannot_borrow_project_tickets_for_persistence() {
    let (_d, mut s) = setup();
    let c = first(&mut s, 93000);
    read(&mut s, 93001);
    let mut r = request(93001);
    r["scope"] = json!({"projectId":"p_project_history","anchorSequence":c["sequence"]});
    rejected(&mut s, SAVE, r);
}

#[test]
fn project_history_offline_media_stays_metadata_through_save_cold_restore_and_owned_undo() {
    let (_d, mut s) = setup();
    let (demo, _) = crate::demo::demo_project(&s.media);
    let mut item = demo.items.values().find(|i| i.as_media().is_some()).unwrap().clone();
    item.id = filmcraft_project::ItemId(1000);
    if let filmcraft_project::ItemKind::Media(m) = &mut item.kind {
        m.media = filmcraft_project::MediaRef::File { path: "/generated/missing-history-original.mxf".into() };
    }
    let p = Arc::make_mut(&mut s.project);
    p.items.insert(item.id, item.clone());
    p.root.children.push(filmcraft_project::BinEntry::Item(item.id));
    let before = s.project.clone();
    bin(&mut s, 93100);
    let saved = save(&mut s, 93101);
    let mut f = reopen(&s);
    let r = restore(&mut f, 93102, &saved);
    f.execute(RESTORE, r).unwrap();
    undo(&mut f, 93103, 93100);
    assert_eq!(f.project, before);
    assert_eq!(f.project.item(item.id), Some(&item));
    assert_eq!(read(&mut f, 93104)["mediaEvidence"], false);
    assert_eq!(f.agent_operation_control().progress(&token(93102))["bytes_read"], "0");
}
#[test]
fn project_history_retains_mixed_sequence_media_and_project_tickets_without_lending_source_authority() {
    let d = Scratch::new();
    let mut s = super::condense_tests::setup();
    let id = s.state.active_sequence.unwrap();
    let root = json!({"projectId":"p_test","workspaceId":W});
    let sequence = json!({"projectId":"p_test","anchorSequence":id.0});
    // Assemble the generated already-associated saved fixture. The production
    // association command correctly refuses adopting an already-bound project.
    Arc::make_mut(&mut s.project)
        .item_mut(id)
        .unwrap()
        .metadata
        .insert(crate::editorial_workspace::KEY.into(), json!({"kind":"poema-native-workspace","version":1,"workspaceId":W,"scope":sequence}).to_string());
    Arc::make_mut(&mut s.project)
        .metadata
        .insert(crate::project_workspace::KEY.into(), json!({"kind":"poema-native-project-workspace","version":1,"scope":root}).to_string());
    s.history.undo.clear();
    s.history.redo.clear();
    s.history.merge_key = None;
    s.path = Some(d.0.join("generated.fcproj").to_str().unwrap().into());
    persist(&mut s);
    let original = s.project.clone();
    let c = s.active_sequence().unwrap().video_tracks[0].items[0].id;
    super::tests::read(&mut s, 93200);
    s.execute(
        "agent.apply",
        super::tests::exact_batch(93200, vec![json!({"command":"timeline.trim","params":{"clip":c.0,"edge":"out","mode":"regular","deltaFrames":-1}})]),
    )
    .unwrap();
    let trimmed = s.project.clone();
    let r = s.execute(project_plane::READ, json!({"token":token(93201),"scope":root,"projectPath":s.path})).unwrap();
    s.execute(
        "agent.apply",
        batch(
            93201,
            "agent.organizeProject",
            json!({"scope":root,"beforeSha256":r["binState"]["sha256"],"edit":{"type":"create","name":"Metadata after media edit","parent":null}}),
        ),
    )
    .unwrap();
    persist(&mut s);
    s.execute(project_plane::READ, json!({"token":token(93202),"scope":root,"projectPath":s.path})).unwrap();
    let mut r = request(93202);
    r["scope"] = root.clone();
    let saved = s.execute(SAVE, r).unwrap();
    let raw: Value = serde_json::from_slice(&fs::read(d.companion()).unwrap()).unwrap();
    assert_eq!(raw["tickets"].as_array().unwrap().len(), 2);
    assert!(raw["tickets"][0].get("project_scope").is_none());
    assert_eq!(raw["tickets"][1]["project_scope"], root);
    let mut f = reopen(&s);
    f.execute(project_plane::READ, json!({"token":token(93203),"scope":root,"projectPath":f.path})).unwrap();
    let mut r = request(93203);
    r["scope"] = root.clone();
    r["sha256"] = saved["checkpoint"]["sha256"].clone();
    f.execute(RESTORE, r).unwrap();
    f.execute(project_plane::READ, json!({"token":token(93204),"scope":root,"projectPath":f.path})).unwrap();
    f.execute("agent.undo", json!({"token":token(93204),"commit":token(93201)})).unwrap();
    assert_eq!(f.project, trimmed);
    f.execute(project_plane::READ, json!({"token":token(93205),"scope":root,"projectPath":f.path})).unwrap();
    assert_eq!(f.execute("agent.readHistory", json!({"token":token(93205),"scope":root,"commit":token(93200),"undo":null})).unwrap()["undoAvailable"], false);
    rejected(&mut f, "agent.undo", json!({"token":token(93205),"commit":token(93200)}));
    super::tests::read(&mut f, 93206);
    f.execute("agent.undo", json!({"token":token(93206),"commit":token(93200)})).unwrap();
    assert_eq!(serde_json::to_value(&f.project).unwrap(), serde_json::to_value(original).unwrap());
}

#[test]
fn project_history_legacy_capsule_keeps_version_one_numeric_sequences_and_omits_project_plane_fields() {
    let d = Scratch::new();
    let mut s = super::condense_tests::setup();
    s.path = Some(d.0.join("generated.fcproj").to_str().unwrap().into());
    let scope = json!({"projectId":"p_test","anchorSequence":s.state.active_sequence.unwrap().0});
    let before = s.project.clone();
    let clip = s.active_sequence().unwrap().video_tracks[0].items[0].id;
    super::tests::read(&mut s, 93300);
    s.execute(
        "agent.apply",
        super::tests::exact_batch(93300, vec![json!({"command":"timeline.trim","params":{"clip":clip.0,"edge":"out","mode":"regular","deltaFrames":-1}})]),
    )
    .unwrap();
    persist(&mut s);
    super::tests::read(&mut s, 93301);
    let mut r = request(93301);
    r["scope"] = scope.clone();
    let saved = s.execute(SAVE, r).unwrap();
    let raw: Value = serde_json::from_slice(&fs::read(d.companion()).unwrap()).unwrap();
    assert_eq!(raw["version"], 1);
    assert_eq!(raw["scope"], scope);
    assert_eq!(raw["sequence"].as_u64(), Some(s.state.active_sequence.unwrap().0));
    for t in raw["tickets"].as_array().unwrap() {
        assert!(t["sequence"].is_u64());
        assert!(t["after_sequence"].is_u64());
        assert!(!t.as_object().unwrap().contains_key("project_scope"));
    }
    let mut f = reopen(&s);
    super::tests::read(&mut f, 93302);
    let mut r = request(93302);
    r["scope"] = scope;
    r["sha256"] = saved["checkpoint"]["sha256"].clone();
    f.execute(RESTORE, r).unwrap();
    super::tests::read(&mut f, 93303);
    f.execute("agent.undo", json!({"token":token(93303),"commit":token(93300)})).unwrap();
    assert_eq!(serde_json::to_value(&f.project).unwrap(), serde_json::to_value(before).unwrap());
}
#[test]
fn project_history_forged_owned_undo_snapshot_links_refuse_without_adopting_a_redo_branch() {
    let (d, mut s) = setup();
    bin(&mut s, 93400);
    undo(&mut s, 93401, 93400);
    let saved = save(&mut s, 93402);
    let mut raw: Value = serde_json::from_slice(&fs::read(d.companion()).unwrap()).unwrap();
    let undo = raw["tickets"].as_array_mut().unwrap().iter_mut().find(|t| t["token"] == token(93401)).unwrap();
    undo["project"] = undo["after"].clone();
    let bytes = serde_json::to_vec(&raw).unwrap();
    fs::write(d.companion(), &bytes).unwrap();
    let mut f = reopen(&s);
    let mut r = restore(&mut f, 93403, &saved);
    r["sha256"] = json!(format!("{:x}", Sha256::digest(bytes)));
    rejected(&mut f, RESTORE, r);
}

const INSPECT: &str = "agent.inspectHistoryCheckpoint";
fn inspection(s: &mut Session, n: usize, saved: &Value) -> Value {
    read(s, n);
    json!({"token":token(n),"scope":scope(),"workspace":W,"checkpoint":C,"basis":{"nativeSession":saved["checkpoint"]["nativeSession"],"revision":saved["checkpoint"]["nativeRevision"],"sequence":saved["sequence"]}})
}
#[test]
fn project_companion_inspection_is_read_only_complete_and_does_not_renew_source_or_history_authority() {
    let (d, mut s) = setup();
    bin(&mut s, 98000);
    let saved = save(&mut s, 98001);
    let raw = fs::read(d.companion()).unwrap();
    let request = inspection(&mut s, 98002, &saved);
    let before = state(&s);
    let root = s.project.clone();
    let result = s.execute(INSPECT, request).unwrap();
    assert_eq!(result["checkpoint"], saved["checkpoint"]);
    assert_eq!(result["basis"]["sequence"], Value::Null);
    assert_eq!(result["historyValidation"], "validated_for_current_project");
    assert_eq!(result["mediaEvidence"], false);
    assert_eq!(result["writeAuthority"], false);
    assert_eq!(result["sourceValidation"], "not-performed");
    assert_eq!(state(&s), before);
    assert!(Arc::ptr_eq(&root, &s.project));
    assert_eq!(fs::read(d.companion()).unwrap(), raw);
}
#[test]
fn project_companion_inspection_cold_validates_without_installing_undo_or_old_tickets() {
    let (_d, mut s) = setup();
    first(&mut s, 98010);
    let saved = save(&mut s, 98011);
    let mut cold = reopen(&s);
    let request = inspection(&mut cold, 98012, &saved);
    let before = state(&cold);
    cold.execute(INSPECT, request).unwrap();
    assert_eq!(state(&cold), before);
    assert!(cold.history.undo.is_empty());
    assert_eq!(cold.execute("agent.receipt", json!({"token":token(98010)})).unwrap()["status"], "unknown");
}
#[test]
fn project_companion_inspection_rejects_foreign_basis_unknown_fields_and_later_human_work() {
    let (_d, mut s) = setup();
    first(&mut s, 98020);
    let saved = save(&mut s, 98021);
    for (index, field) in ["epoch", "revision", "sequence", "scope", "path", "workspace"].iter().enumerate() {
        let mut r = inspection(&mut s, 98022 + index, &saved);
        match *field {
            "epoch" => r["basis"]["nativeSession"] = json!("f".repeat(32)),
            "revision" => r["basis"]["revision"] = json!(0),
            "sequence" => r["basis"]["sequence"] = Value::Null,
            "scope" => r["scope"]["projectId"] = json!("p_foreign"),
            "path" => r["path"] = json!("/must-not-read"),
            _ => r["workspace"] = json!(C),
        }
        rejected(&mut s, INSPECT, r);
    }
    s.execute("file.newBin", json!({"name":"Later human work"})).unwrap();
    let request = inspection(&mut s, 98030, &saved);
    rejected(&mut s, INSPECT, request);
}
#[test]
fn project_companion_inspection_rejects_truncated_corrupt_and_redirected_companions_without_writing() {
    let (d, mut s) = setup();
    first(&mut s, 98040);
    let saved = save(&mut s, 98041);
    let raw = fs::read(d.companion()).unwrap();
    for (index, corrupted) in [raw[..raw.len() / 2].to_vec(), b"{}".to_vec()].into_iter().enumerate() {
        fs::write(d.companion(), &corrupted).unwrap();
        let request = inspection(&mut s, 98042 + index, &saved);
        rejected(&mut s, INSPECT, request);
        assert_eq!(fs::read(d.companion()).unwrap(), corrupted);
    }
    fs::write(d.companion(), &raw).unwrap();
    #[cfg(unix)]
    {
        let actual = d.0.join("preserved-companion.json");
        fs::rename(d.companion(), &actual).unwrap();
        std::os::unix::fs::symlink(&actual, d.companion()).unwrap();
        let request = inspection(&mut s, 98045, &saved);
        rejected(&mut s, INSPECT, request);
        assert_eq!(fs::read(&actual).unwrap(), raw);
    }
}
#[test]
fn project_companion_inspection_honors_the_independent_stop_fence_and_worker_routes() {
    let (_d, mut s) = setup();
    let saved = save(&mut s, 98050);
    let request = inspection(&mut s, 98051, &saved);
    assert_eq!(s.agent_operation_control().cancel(&json!({"token":token(98051)})).unwrap()["cancellationAccepted"], true);
    rejected(&mut s, INSPECT, request);
    assert!(crate::session_worker::supports(INSPECT));
}

const PROVENANCE: &str = "agent.inspectCheckpointProvenance";
fn provenance(s: &mut Session, n: usize, saved: &Value) -> Value {
    let mut r = inspection(s, n, saved);
    r["sha256"] = saved["checkpoint"]["sha256"].clone();
    r
}
#[test]
fn project_companion_provenance_reads_real_archived_realm_after_undo_and_new_branch_without_adopting_history() {
    let (d, mut s) = setup();
    let creation = first(&mut s, 99000);
    let anchor = creation["results"][0]["sequence"].clone();
    let saved = save(&mut s, 99001);
    let bytes = fs::read(d.companion()).unwrap();
    undo(&mut s, 99002, 99000);
    bin(&mut s, 99003);
    assert!(s.state.active_sequence.is_none());
    assert!(s.history.redo.is_empty());
    let request = inspection(&mut s, 99004, &saved);
    rejected(&mut s, INSPECT, request);
    let request = provenance(&mut s, 99005, &saved);
    let before = state(&s);
    let root = s.project.clone();
    let result = s.execute(PROVENANCE, request).unwrap();
    assert_eq!(result["owningRealms"], json!([{"projectId":scope()["projectId"],"anchorSequence":anchor}]));
    assert_eq!(result["historyValidation"], "validated_archive_only");
    for flag in ["mediaEvidence", "writeAuthority"] {
        assert_eq!(result[flag], false);
    }
    assert_eq!(result["projectUnchanged"], true);
    assert_eq!(result["historyUnchanged"], true);
    assert_eq!(result["sourceValidation"], "not-performed");
    assert_eq!(result["execution"], "not-applied");
    assert_eq!(state(&s), before);
    assert!(Arc::ptr_eq(&root, &s.project));
    assert_eq!(fs::read(d.companion()).unwrap(), bytes);
}
#[test]
fn project_companion_provenance_cold_reads_original_scope_without_restoring_undo_receipts_or_a_sequence() {
    let (_d, mut s) = setup();
    let creation = first(&mut s, 99100);
    let saved = save(&mut s, 99101);
    undo(&mut s, 99102, 99100);
    bin(&mut s, 99103);
    persist(&mut s);
    let mut cold = reopen(&s);
    let request = provenance(&mut cold, 99104, &saved);
    let before = state(&cold);
    let result = cold.execute(PROVENANCE, request).unwrap();
    assert_eq!(result["owningRealms"][0]["anchorSequence"], creation["results"][0]["sequence"]);
    assert_eq!(state(&cold), before);
    assert!(cold.history.undo.is_empty());
    assert!(cold.history.redo.is_empty());
    assert!(cold.state.active_sequence.is_none());
    assert_eq!(cold.execute("agent.receipt", json!({"token":token(99100)})).unwrap()["status"], "unknown");
}
#[test]
fn project_companion_provenance_empty_original_graph_never_invents_a_realm_from_a_later_active_tab() {
    let (_d, mut s) = setup();
    let saved = save(&mut s, 99200);
    first(&mut s, 99201);
    let request = provenance(&mut s, 99202, &saved);
    let before = state(&s);
    let result = s.execute(PROVENANCE, request).unwrap();
    assert_eq!(result["owningRealms"], json!([]));
    assert_eq!(result["basis"]["sequence"], Value::Null);
    assert_eq!(state(&s), before);
}
#[test]
fn project_companion_provenance_recovers_real_realm_from_the_archived_redo_graph_with_no_active_sequence() {
    let (_d, mut s) = setup();
    let creation = first(&mut s, 99300);
    undo(&mut s, 99301, 99300);
    let saved = save(&mut s, 99302);
    assert_eq!(saved["sequence"], Value::Null);
    bin(&mut s, 99303);
    let request = provenance(&mut s, 99304, &saved);
    let before = state(&s);
    let result = s.execute(PROVENANCE, request).unwrap();
    assert_eq!(result["owningRealms"][0]["anchorSequence"], creation["results"][0]["sequence"]);
    assert_eq!(state(&s), before);
    assert!(s.history.redo.is_empty());
}
#[test]
fn project_companion_provenance_refuses_foreign_digest_basis_root_or_request_aliases_without_state_changes() {
    let (_d, mut s) = setup();
    first(&mut s, 99400);
    let saved = save(&mut s, 99401);
    for (i, field) in ["digest", "epoch", "revision", "sequence", "scope", "workspace", "path", "sequence_alias", "missing_sequence"].iter().enumerate() {
        let mut r = provenance(&mut s, 99402 + i, &saved);
        match *field {
            "digest" => r["sha256"] = json!("f".repeat(64)),
            "epoch" => r["basis"]["nativeSession"] = json!("f".repeat(32)),
            "revision" => r["basis"]["revision"] = json!(0),
            "sequence" => r["basis"]["sequence"] = Value::Null,
            "scope" => r["scope"]["projectId"] = json!("p_foreign"),
            "workspace" => r["workspace"] = json!(C),
            "path" => r["path"] = json!("/must-not-read"),
            "missing_sequence" => {
                r["basis"].as_object_mut().unwrap().remove("sequence");
            }
            _ => r["scope"] = json!({"projectId":scope()["projectId"],"anchorSequence":saved["sequence"]}),
        }
        rejected(&mut s, PROVENANCE, r);
    }
}
#[test]
fn project_companion_provenance_validates_rehashed_archive_recipes_and_rejects_corruption_and_symlinks() {
    let (d, mut s) = setup();
    first(&mut s, 99500);
    let saved = save(&mut s, 99501);
    let bytes = fs::read(d.companion()).unwrap();
    let mut forged: Value = serde_json::from_slice(&bytes).unwrap();
    forged["tickets"][0]["result"]["revision"] = json!(0);
    for (i, raw) in [bytes[..bytes.len() / 2].to_vec(), b"{}".to_vec(), serde_json::to_vec(&forged).unwrap()].iter().enumerate() {
        fs::write(d.companion(), raw).unwrap();
        let mut r = provenance(&mut s, 99502 + i, &saved);
        r["sha256"] = json!(format!("{:x}", Sha256::digest(raw)));
        rejected(&mut s, PROVENANCE, r);
        assert_eq!(fs::read(d.companion()).unwrap(), *raw);
    }
    fs::write(d.companion(), &bytes).unwrap();
    #[cfg(unix)]
    {
        let actual = d.0.join("preserved-original-companion.json");
        fs::rename(d.companion(), &actual).unwrap();
        std::os::unix::fs::symlink(&actual, d.companion()).unwrap();
        let r = provenance(&mut s, 99510, &saved);
        rejected(&mut s, PROVENANCE, r);
        assert_eq!(fs::read(actual).unwrap(), bytes);
    }
}
#[test]
fn project_companion_provenance_uses_the_existing_independent_stop_fence_and_real_native_worker() {
    let (_d, mut s) = setup();
    let saved = save(&mut s, 99600);
    let r = provenance(&mut s, 99601, &saved);
    s.agent_operation_control().cancel(&json!({"token":token(99601)})).unwrap();
    rejected(&mut s, PROVENANCE, r);
    assert!(crate::session_worker::supports(PROVENANCE));
}


fn paused_project_companion(n: usize) -> (Scratch, Session, Value) {
    let (d, mut s) = setup();
    s.agent_operation_control().enable_transport();
    first(&mut s, n);
    persist(&mut s);
    s.execute("agent.readHistoryCheckpointBasis", json!({"token":token(n+1),"scope":scope()})).unwrap();
    s.execute("agent.beginWorkspaceUpdate", json!({"token":token(n+1),"scope":scope(),"workspace":W,"update":"bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb"})).unwrap();
    s.execute("agent.readHistoryCheckpointBasis", json!({"token":token(n+2),"scope":scope()})).unwrap();
    let mut r = request(n+2);
    r["metadataBasis"] = json!(true);
    let saved = s.execute(SAVE, r).unwrap();
    (d, s, saved)
}
#[test]
fn paused_project_companion_inspection_keeps_the_original_project_history_and_edit_fence() {
    let (d, mut s, saved) = paused_project_companion(99800);
    let request = inspection(&mut s, 99804, &saved);
    let before = state(&s);
    let root = s.project.clone();
    let bytes = fs::read(d.companion()).unwrap();
    let result = s.execute(INSPECT, request).unwrap();
    assert_eq!(result["checkpoint"], saved["checkpoint"]);
    assert_eq!(result["writeAuthority"], false);
    assert_eq!(result["execution"], "not-applied");
    assert_eq!(result["mediaEvidence"], false);
    assert_eq!(state(&s), before);
    assert!(Arc::ptr_eq(&root, &s.project));
    assert_eq!(fs::read(d.companion()).unwrap(), bytes);
    assert!(s.workspace_update_status().is_some());
    assert!(!s.workspace_update_allows("agent.apply"));
    assert!(!s.workspace_update_allows("agent.prepareMulticamProduction"));
    assert!(!s.workspace_update_allows("agent.undo"));
    assert!(!s.workspace_update_allows(RESTORE));
}
#[test]
fn paused_project_companion_inspection_still_rejects_foreign_basis_scope_and_changed_capsule() {
    let (d, mut s, saved) = paused_project_companion(99900);
    for (index, field) in ["epoch", "revision", "sequence", "scope", "workspace", "extra"].iter().enumerate() {
        let mut r = inspection(&mut s, 99904+index, &saved);
        match *field {
            "epoch" => r["basis"]["nativeSession"] = json!("f".repeat(32)),
            "revision" => r["basis"]["revision"] = json!(0),
            "sequence" => r["basis"]["sequence"] = Value::Null,
            "scope" => r["scope"]["projectId"] = json!("p_foreign"),
            "workspace" => r["workspace"] = json!("aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa"),
            _ => r["extra"] = json!(true),
        }
        rejected(&mut s, INSPECT, r);
    }
    let r = inspection(&mut s, 99920, &saved);
    fs::write(d.companion(), b"corrupted generated companion").unwrap();
    rejected(&mut s, INSPECT, r);
    assert!(s.workspace_update_status().is_some());
}


const OUTCOME: &str = "agent.readHistoryCheckpointOutcome";
fn outcome_request(s: &mut Session, n: usize, saved: &Value) -> Value {
    let mut p = inspection(s,n,saved);p["originalToken"] = saved["token"].clone();p
}
#[test]
fn original_checkpoint_outcome_survives_later_human_work_without_file_read_or_state_change() {
    let (_d, mut s) = setup();first(&mut s, 99000);let saved=save(&mut s,99001);
    s.execute("file.newBin",json!({"name":"Later human work"})).unwrap();
    let p=outcome_request(&mut s,99002,&saved);let before=state(&s);let root=s.project.clone();
    let r=s.execute(OUTCOME,p).unwrap();assert_eq!(r["status"],"recorded");assert_eq!(r["original"],saved);
    assert_eq!(r["fileValidation"],"not-performed");assert_eq!(r["requestReplay"],false);assert_eq!(r["writeAuthority"],false);
    assert_eq!(r["historyAdopted"],false);assert_eq!(state(&s),before);assert!(Arc::ptr_eq(&root,&s.project));
}
#[test]
fn original_checkpoint_outcome_does_not_claim_validation_of_replaced_file() {
    let (d,mut s)=setup();let saved=save(&mut s,99010);fs::write(d.companion(),b"generated damaged companion").unwrap();
    let p=outcome_request(&mut s,99011,&saved);let before=state(&s);let r=s.execute(OUTCOME,p).unwrap();
    assert_eq!(r["original"],saved);assert_eq!(r["fileValidation"],"not-performed");assert_eq!(state(&s),before);
}
#[test]
fn original_checkpoint_outcome_is_unknown_in_cold_or_adopted_guards_even_with_original_bytes() {
    let (_d,mut s)=setup();let saved=save(&mut s,99020);let mut cold=reopen(&s);
    let p=outcome_request(&mut cold,99021,&saved);assert_eq!(cold.execute(OUTCOME,p).unwrap()["status"],"unknown");
    super::history_checkpoint::remember_workspace(&mut cold,W,C,saved["checkpoint"]["sha256"].as_str().unwrap(),saved["checkpoint"]["nativeSession"].as_str().unwrap()).unwrap();
    let p=outcome_request(&mut cold,99022,&saved);let before=state(&cold);let r=cold.execute(OUTCOME,p).unwrap();
    assert_eq!(r["status"],"unknown");assert!(r["original"].is_null());assert_eq!(state(&cold),before);
}
#[test]
fn original_checkpoint_outcome_rejects_hostile_shape_and_never_rebases_reference() {
    let (_d,mut s)=setup();let saved=save(&mut s,99030);
    for (i,field) in ["extra","missing_sequence","zero_revision","bad_original_token","huge_sequence"].iter().enumerate(){
        let mut p=outcome_request(&mut s,99031+i,&saved);
        match *field {"extra"=>p["extra"]=json!(true),"missing_sequence"=>{p["basis"].as_object_mut().unwrap().remove("sequence");},"zero_revision"=>p["basis"]["revision"]=json!(0),"bad_original_token"=>p["originalToken"]=json!("bad"),_=>p["basis"]["sequence"]=json!(9_007_199_254_740_992_u64)}
        rejected(&mut s,OUTCOME,p);
    }
    for (i,field) in ["originalToken","revision","epoch","checkpoint"].iter().enumerate(){
        let mut p=outcome_request(&mut s,99040+i,&saved);
        match *field {"originalToken"=>p["originalToken"]=json!(token(99999)),"revision"=>p["basis"]["revision"]=json!(999),"epoch"=>p["basis"]["nativeSession"]=json!("f".repeat(32)),_=>p["checkpoint"]=json!("aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa")}
        let before=state(&s);let r=s.execute(OUTCOME,p).unwrap();assert_eq!(r["status"],"unknown");assert!(r["original"].is_null());assert_eq!(state(&s),before);
    }
}
#[test]
fn original_checkpoint_outcome_refuses_changed_path_or_epoch() {
    let (d,mut s)=setup();let saved=save(&mut s,99060);
    let old_path=s.path.clone();s.path=Some(d.0.join("later.fcproj").to_str().unwrap().into());persist(&mut s);
    let p=outcome_request(&mut s,99061,&saved);let before=state(&s);assert_eq!(s.execute(OUTCOME,p).unwrap()["status"],"unknown");assert_eq!(state(&s),before);
    s.path=old_path;s.agent_bridge.epoch=Some("a".repeat(32));
    let p=outcome_request(&mut s,99062,&saved);let before=state(&s);assert_eq!(s.execute(OUTCOME,p).unwrap()["status"],"unknown");assert_eq!(state(&s),before);
}
#[test]
fn original_checkpoint_outcome_keeps_existing_sixteen_guard_bound() {
    let (_d,mut s)=setup();let saved=save(&mut s,99050);
    for i in 0..16 {read(&mut s,99100+i);let mut p=request(99100+i);p["checkpoint"]=json!(format!("aaaaaaaa-aaaa-4aaa-8aaa-{i:012x}"));s.execute(SAVE,p).unwrap();}
    assert_eq!(s.agent_bridge.history_checkpoints.len(),16);let p=outcome_request(&mut s,99200,&saved);let before=state(&s);
    let r=s.execute(OUTCOME,p).unwrap();assert_eq!(r["status"],"unknown");assert_eq!(state(&s),before);
}


#[test]
fn paused_checkpoint_outcome_and_provenance_preserve_the_same_owner_and_edit_fence() {
    let (d,mut s,saved)=paused_project_companion(100000);let pause=s.workspace_update_status().unwrap();let root=s.project.clone();let bytes=fs::read(d.companion()).unwrap();
    let p=outcome_request(&mut s,100004,&saved);let before=state(&s);let result=s.execute(OUTCOME,p).unwrap();assert_eq!(result["original"],saved);assert_eq!(result["fileValidation"],"not-performed");assert_eq!(result["writeAuthority"],false);assert_eq!(state(&s),before);
    let p=provenance(&mut s,100005,&saved);let before=state(&s);let proof=s.execute(PROVENANCE,p).unwrap();assert_eq!(proof["checkpoint"],saved["checkpoint"]);assert_eq!(proof["historyValidation"],"validated_archive_only");assert_eq!(proof["execution"],"not-applied");assert_eq!(state(&s),before);assert!(Arc::ptr_eq(&root,&s.project));assert_eq!(fs::read(d.companion()).unwrap(),bytes);assert_eq!(s.workspace_update_status().unwrap(),pause);
    for command in ["file.newBin","agent.apply","agent.undo","agent.redo",RESTORE,"agent.prepareMulticamProduction"] {assert!(!s.workspace_update_allows(command));assert!(s.execute(command,json!({})).is_err());}
    assert_eq!(s.workspace_update_status().unwrap(),pause);
}
#[test]
fn paused_checkpoint_outcome_never_claims_archival_validation_for_a_corrupt_file() {
    let (d,mut s,saved)=paused_project_companion(100020);let pause=s.workspace_update_status().unwrap();fs::write(d.companion(),b"generated damaged companion").unwrap();let p=outcome_request(&mut s,100024,&saved);let result=s.execute(OUTCOME,p).unwrap();assert_eq!(result["original"],saved);assert_eq!(result["fileValidation"],"not-performed");let p=provenance(&mut s,100025,&saved);rejected(&mut s,PROVENANCE,p);assert_eq!(fs::read(d.companion()).unwrap(),b"generated damaged companion");assert_eq!(s.workspace_update_status().unwrap(),pause);
}
#[test]
fn paused_checkpoint_outcome_crossed_references_and_foreign_scope_do_not_settle_the_owner() {
    let (_d,mut s,saved)=paused_project_companion(100040);let pause=s.workspace_update_status().unwrap();
    for (i,field) in ["scope","workspace","extra"].iter().enumerate(){let mut p=outcome_request(&mut s,100044+i,&saved);match *field{"scope"=>p["scope"]["projectId"]=json!("p_foreign"),"workspace"=>p["workspace"]=json!("aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa"),_=>p["extra"]=json!(true)}rejected(&mut s,OUTCOME,p);}
    for (i,field) in ["originalToken","revision","epoch","checkpoint"].iter().enumerate(){let mut p=outcome_request(&mut s,100050+i,&saved);match *field{"originalToken"=>p["originalToken"]=json!(token(100099)),"revision"=>p["basis"]["revision"]=json!(999),"epoch"=>p["basis"]["nativeSession"]=json!("f".repeat(32)),_=>p["checkpoint"]=json!("aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa")}let before=state(&s);let r=s.execute(OUTCOME,p).unwrap();assert_eq!(r["status"],"unknown");assert!(r["original"].is_null());assert_eq!(state(&s),before);}
    assert_eq!(s.workspace_update_status().unwrap(),pause);
}
#[test]
fn paused_checkpoint_outcome_uses_independent_stop_without_cancelling_the_original_publisher() {
    let (_d,mut s,saved)=paused_project_companion(100060);let pause=s.workspace_update_status().unwrap();let p=outcome_request(&mut s,100064,&saved);assert_eq!(s.agent_operation_control().cancel(&json!({"token":token(100064)})).unwrap()["cancellationAccepted"],true);rejected(&mut s,OUTCOME,p);let p=provenance(&mut s,100065,&saved);assert_eq!(s.agent_operation_control().cancel(&json!({"token":token(100065)})).unwrap()["cancellationAccepted"],true);rejected(&mut s,PROVENANCE,p);let p=outcome_request(&mut s,100066,&saved);assert_eq!(s.execute(OUTCOME,p).unwrap()["original"],saved);assert_eq!(s.workspace_update_status().unwrap(),pause);
}
#[test]
fn paused_checkpoint_outcome_and_provenance_run_on_the_original_session_worker_only() {
    let (_d,mut s,saved)=paused_project_companion(100080);let pause=s.workspace_update_status().unwrap();let worker=crate::session_worker::SessionWorker::new().unwrap();worker.install(&mut s).unwrap();
    for (i,command) in [OUTCOME,PROVENANCE].iter().enumerate(){let p=if *command==OUTCOME{outcome_request(&mut s,100084+i,&saved)}else{provenance(&mut s,100084+i,&saved)};let before=state(&s);let root=s.project.clone();let projection_request=p.clone();let mut pending=worker.start(&mut s,command,p,None).unwrap();assert!(s.execute(command,projection_request).is_err());assert!(s.execute("file.newBin",json!({"name":"No projection writer"})).is_err());let began=std::time::Instant::now();let done=loop{if let Some(done)=pending.try_finish(){break done;}assert!(began.elapsed()<std::time::Duration::from_secs(5));std::thread::sleep(std::time::Duration::from_millis(2));};assert!(!done.reply.uncertain);let result=done.reply.result.unwrap();s=done.session;assert_eq!(result["writeAuthority"],false);assert_eq!(result["execution"],"not-applied");if *command==OUTCOME{assert_eq!(result["original"],saved);}else{assert_eq!(result["checkpoint"],saved["checkpoint"]);}assert_eq!(state(&s),before);assert!(Arc::ptr_eq(&root,&s.project));assert_eq!(s.workspace_update_status().unwrap(),pause);}
}
#[test]
fn paused_checkpoint_outcome_keeps_the_original_sixteen_live_guard_bound() {
    let (_d,mut s,saved)=paused_project_companion(100100);let pause=s.workspace_update_status().unwrap();
    for i in 0..16 {s.execute("agent.readHistoryCheckpointBasis",json!({"token":token(100104+i),"scope":scope()})).unwrap();let mut p=request(100104+i);p["metadataBasis"]=json!(true);p["checkpoint"]=json!(format!("aaaaaaaa-aaaa-4aaa-8aaa-{i:012x}"));s.execute(SAVE,p).unwrap();}
    assert_eq!(s.agent_bridge.history_checkpoints.len(),16);let p=outcome_request(&mut s,100122,&saved);assert_eq!(s.execute(OUTCOME,p).unwrap()["status"],"unknown");assert_eq!(s.workspace_update_status().unwrap(),pause);
}
