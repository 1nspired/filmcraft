//! Real generated source bytes, ordinary native import, one guarded binding writer.
use super::*;
use crate::Session;
use crate::agent_bridge::{project_plane, snapshot_jobs, tests::token};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    fs,
    path::{Path, PathBuf},
};
const W: &str = "c322ff87-815f-4896-9230-588ed57694ba";
const C: &str = "e2428d4a-fc93-43d4-841b-3641bb1337de";
fn scope() -> Value {
    json!({"projectId":"p_library","workspaceId":W})
}
struct Fixture {
    s: Session,
    dir: PathBuf,
    files: Vec<PathBuf>,
    items: Vec<u64>,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.dir);
    }
}
fn fixture() -> Fixture {
    let mut entropy = [0u8; 16];
    getrandom::fill(&mut entropy).unwrap();
    let dir = crate::temp_dir().join(format!("filmcraft-source-library-{:x}", u128::from_le_bytes(entropy)));
    fs::create_dir(&dir).unwrap();
    let dir = fs::canonicalize(dir).unwrap();
    let mut s = Session::default();
    s.execute("file.newProject", json!({"name":"Generated library","workspace":scope()})).unwrap();
    s.path = Some(dir.join("root.fcproj").to_str().unwrap().into());
    let mut files = Vec::new();
    let mut items = Vec::new();
    for i in 0..2 {
        let mut b = Vec::new();
        b.extend(b"RIFF");
        b.extend(1956u32.to_le_bytes());
        b.extend(b"WAVEfmt ");
        b.extend(16u32.to_le_bytes());
        b.extend(1u16.to_le_bytes());
        b.extend(2u16.to_le_bytes());
        b.extend(48000u32.to_le_bytes());
        b.extend(192000u32.to_le_bytes());
        b.extend(4u16.to_le_bytes());
        b.extend(16u16.to_le_bytes());
        b.extend(b"data");
        b.extend(1920u32.to_le_bytes());
        b.resize(1964, 0);
        b[100] = i;
        let file = dir.join(format!("camera{i}.wav"));
        fs::write(&file, b).unwrap();
        items.push(crate::commands::import_streamed(&mut s, file.to_str().unwrap(), None).unwrap().0);
        files.push(file);
    }
    Fixture { s, dir, files, items }
}
fn state(s: &Session) -> Value {
    json!({"project":s.project,"editor":s.state,"revision":s.revision,"saved":s.saved_revision,"undo":s.history.undo.len(),"redo":s.history.redo.len(),"journal":s.journal,"events":s.events.len()})
}
fn read(s: &mut Session, n: usize, items: &[u64]) -> Value {
    let before = state(s);
    let first = s.execute(READ, json!({"token":token(n),"scope":scope(),"projectPath":s.path,"sources":items})).unwrap();
    assert_eq!(first["status"], "running");
    assert!(first.get("token").is_none());
    for _ in 0..1500 {
        let r = s.execute("agent.snapshotStatus", json!({"job":token(n)})).unwrap();
        if r["status"] == "complete" {
            assert_eq!(state(s), before);
            return r["result"].clone();
        }
        assert_eq!(r["status"], "running", "{r}");
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    panic!("library read did not settle")
}
fn validation(s: &mut Session, n: usize) -> Value {
    let job = token(n + 100_000);
    let before = state(s);
    let first = s.execute(VALIDATE, json!({"token":token(n),"scope":scope(),"job":job})).unwrap();
    assert_eq!(first["status"], "running");
    for _ in 0..1500 {
        let r = s.execute("agent.validationStatus", json!({"job":job})).unwrap();
        if r["status"] == "complete" {
            assert_eq!(r["result"]["kind"], "native_source_library_read");
            assert_eq!(r["result"]["scope"], scope());
            assert_eq!(r["result"]["writeAuthority"], false);
            assert_eq!(state(s), before);
            return r["result"].clone();
        }
        assert_eq!(r["status"], "running", "{r}");
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    panic!("Root background validation did not settle")
}
fn batch(f: &Fixture, n: usize, items: &[usize]) -> Value {
    let operations:Vec<_>=items.iter().map(|i|{let bytes=fs::read(&f.files[*i]).unwrap();json!({"command":"agent.admitSource","params":{"item":f.items[*i],"path":f.files[*i],"binding":{"projectId":"p_library","assetId":format!("a_camera{i}"),"mediaId":format!("a_camera{i}:original"),"role":"original"},"generation":{"kind":"poema-media-generation","version":1,"sha256":format!("{:x}",Sha256::digest(&bytes)),"bytes":bytes.len()},"ownerReceiptSha256":format!("{i:064x}"),"scope":scope()}})}).collect();
    json!({"token":token(n),"label":"Admit exact generated library","exact":true,"operations":operations})
}
fn metadata(s: &mut Session, n: usize) -> Value {
    s.execute(project_plane::READ, json!({"token":token(n),"scope":scope(),"projectPath":s.path})).unwrap()
}
fn persist(s: &mut Session) {
    filmcraft_format::atomic_write(Path::new(s.path.as_ref().unwrap()), &filmcraft_format::encode_with_view(&s.project, Some(&s.project_view()), false))
        .unwrap();
    s.saved_revision = s.revision;
}
fn serial() -> std::sync::MutexGuard<'static, ()> {
    snapshot_jobs::TEST_SERIAL.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}
#[test]
fn independent_library_background_read_retains_absence_and_unbound_files_never_borrow_byte_qualification_or_metadata_authority() {
    let _serial = serial();
    let mut f = fixture();
    let d = read(&mut f.s, 100, &f.items);
    assert_eq!(d["kind"], "native_source_library_read");
    assert_eq!(d["sequence"], Value::Null);
    assert_eq!(d["scope"], scope());
    assert_eq!(d["requestedSources"], json!(f.items));
    assert_eq!(d["mediaScope"], json!({"kind":"dependencies","items":[],"libraryVerified":false}));
    assert!(d["sources"].as_array().unwrap().iter().all(|s| s["fileVerification"] == "unverified"));
    assert!(f.s.execute("agent.read", json!({"token":token(100)})).is_err());
    assert!(f.s.execute("agent.validateSnapshot", json!({"token":token(100)})).is_err());
    assert!(f.s.execute("agent.validateProjectRead", json!({"token":token(100),"scope":scope()})).is_err());
    assert_eq!(validation(&mut f.s, 100)["sequence"], Value::Null);
    metadata(&mut f.s, 101);
    assert!(f.s.execute(VALIDATE, json!({"token":token(101),"scope":scope(),"job":token(100_101)})).is_err());
    assert_eq!(f.s.project.sequences().count(), 0);
}
#[test]
fn independent_library_exact_group_is_one_history_step_with_no_timeline_and_lost_apply_reply_is_not_replayed() {
    let _serial = serial();
    let mut f = fixture();
    read(&mut f.s, 110, &f.items);
    let req = batch(&f, 110, &[0, 1]);
    let before = f.s.project.clone();
    let count = f.s.history.undo.len();
    let result = f.s.execute("agent.apply", req.clone()).unwrap();
    assert_eq!(result["results"].as_array().unwrap().len(), 2);
    assert_eq!(f.s.history.undo.len(), count + 1);
    assert_eq!(f.s.execute("agent.apply", req).unwrap(), result);
    let d = read(&mut f.s, 111, &f.items);
    assert_eq!(d["mediaScope"]["items"], json!(f.items));
    assert!(d["sources"].as_array().unwrap().iter().all(|s| s["fileVerification"] == "verified"));
    metadata(&mut f.s, 112);
    assert!(f.s.execute("agent.undo", json!({"token":token(112),"commit":token(110)})).is_err());
    f.s.execute("agent.undo", json!({"token":token(111),"commit":token(110)})).unwrap();
    assert_eq!(f.s.project, before);
    read(&mut f.s, 113, &f.items);
    let h = f.s.execute("agent.readHistory", json!({"token":token(113),"scope":scope(),"commit":token(110),"undo":token(111),"projection":true})).unwrap();
    assert_eq!(h["redoAvailable"], true);
    f.s.execute("agent.redo", json!({"token":token(113),"commit":token(110),"undo":token(111)})).unwrap();
    assert_eq!(f.s.state.active_sequence, None);
    assert_eq!(f.s.project.sequences().count(), 0);
    assert_eq!(f.s.history.undo.len(), count + 1);
}
#[test]
fn independent_library_scope_is_closed_exact_and_an_explicit_empty_selection_cannot_authorize_undeclared_admission() {
    let _serial = serial();
    let mut f = fixture();
    let before = state(&f.s);
    for (i, change) in [
        json!({"scope":{"projectId":"foreign","workspaceId":W}}),
        json!({"scope":{"projectId":"p_library","workspaceId":W,"anchorSequence":1}}),
        json!({"sources":[0]}),
        json!({"sources":[f.items[0],f.items[0]]}),
        json!({"sources":[999999]}),
        json!({"sources":null}),
        json!({"projectPath":"/wrong/root.fcproj"}),
        json!({"background":false}),
    ]
    .into_iter()
    .enumerate()
    {
        let mut q = json!({"token":token(200+i),"scope":scope(),"projectPath":f.s.path,"sources":f.items});
        for (k, v) in change.as_object().unwrap() {
            q[k] = v.clone();
        }
        assert!(f.s.execute(READ, q).is_err());
        assert_eq!(state(&f.s), before);
    }
    read(&mut f.s, 220, &[]);
    assert!(f.s.execute("agent.apply", batch(&f, 220, &[0])).is_err());
    assert_eq!(state(&f.s), before);
}
#[test]
fn independent_library_metadata_cannot_prepare_or_bind_sources_and_source_reads_cannot_publish_document_or_timeline_gestures() {
    let _serial = serial();
    let mut f = fixture();
    metadata(&mut f.s, 230);
    let req = batch(&f, 230, &[0]);
    let before = state(&f.s);
    assert!(f.s.execute("agent.apply", req).is_err());
    assert_eq!(state(&f.s), before);
    read(&mut f.s, 231, &f.items);
    for (command, params, authority) in [
        (
            "agent.bindSource",
            json!({"item":f.items[0],"binding":{"projectId":"p_library","assetId":"bypass","mediaId":"bypass:original","role":"original"}}),
            None,
        ),
        ("timeline.razor", json!({"clip":1,"time":1}), None),
        (
            "agent.organizeProject",
            json!({"scope":scope(),"beforeSha256":"a".repeat(64),"edit":{"type":"create","name":"Unexpected","parent":null}}),
            Some("document_state"),
        ),
    ] {
        let mut q = json!({"token":token(231),"label":"No borrowed hand","exact":true,"operations":[{"command":command,"params":params}]});
        if let Some(a) = authority {
            q["authority"] = json!(a);
        }
        assert!(f.s.execute("agent.apply", q).is_err());
        assert_eq!(state(&f.s), before);
    }
}
#[test]
fn independent_library_changed_bytes_before_publication_and_before_owned_redo_leave_the_whole_project_unchanged() {
    let _serial = serial();
    let mut f = fixture();
    read(&mut f.s, 240, &f.items);
    let old = batch(&f, 240, &[0]);
    let mut bytes = fs::read(&f.files[0]).unwrap();
    bytes[100] ^= 1;
    fs::write(&f.files[0], &bytes).unwrap();
    let before = state(&f.s);
    assert!(f.s.execute("agent.apply", old).is_err());
    assert_eq!(state(&f.s), before);
    read(&mut f.s, 241, &f.items);
    f.s.execute("agent.apply", batch(&f, 241, &[0])).unwrap();
    read(&mut f.s, 242, &f.items);
    f.s.execute("agent.undo", json!({"token":token(242),"commit":token(241)})).unwrap();
    bytes[100] ^= 1;
    fs::write(&f.files[0], bytes).unwrap();
    read(&mut f.s, 243, &f.items);
    let before = state(&f.s);
    assert!(f.s.execute("agent.redo", json!({"token":token(243),"commit":token(241),"undo":token(242)})).is_err());
    assert_eq!(state(&f.s), before);
}
#[test]
fn independent_library_human_context_changes_cancel_background_delivery_and_stop_revokes_an_unused_read_without_restarting_it() {
    let _serial = serial();
    let mut f = fixture();
    let q = json!({"token":token(250),"scope":scope(),"projectPath":f.s.path,"sources":f.items});
    f.s.execute(READ, q).unwrap();
    f.s.state.linked_selection = !f.s.state.linked_selection;
    let before = state(&f.s);
    assert!(f.s.execute("agent.snapshotStatus", json!({"job":token(250)})).is_err());
    assert_eq!(state(&f.s), before);
    let q = json!({"token":token(251),"scope":scope(),"projectPath":f.s.path,"sources":f.items});
    f.s.execute("agent.cancelSnapshot", json!({"job":token(251)})).unwrap();
    assert!(f.s.execute(READ, q).is_err());
    read(&mut f.s, 252, &f.items);
    assert_eq!(f.s.execute("agent.cancelSnapshot", json!({"job":token(252)})).unwrap()["ticketRevoked"], true);
    assert!(f.s.execute(VALIDATE, json!({"token":token(252),"scope":scope(),"job":token(100_252)})).is_err());
}
fn saved_undone() -> (Fixture, Value, PathBuf) {
    let mut f = fixture();
    read(&mut f.s, 300, &f.items);
    f.s.execute("agent.apply", batch(&f, 300, &[0, 1])).unwrap();
    read(&mut f.s, 301, &f.items);
    f.s.execute("agent.undo", json!({"token":token(301),"commit":token(300)})).unwrap();
    persist(&mut f.s);
    metadata(&mut f.s, 302);
    let saved = f.s.execute("agent.saveHistoryCheckpoint", json!({"token":token(302),"scope":scope(),"workspace":W,"checkpoint":C})).unwrap();
    let path = f.dir.join(format!(".poema-history-{W}-{C}.json"));
    (f, saved, path)
}
#[test]
fn independent_library_checkpoint_version_three_reopens_the_same_owned_redo_without_source_or_timeline_authority() {
    let _serial = serial();
    let (f, saved, path) = saved_undone();
    let raw: Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
    assert_eq!(raw["version"], 3);
    assert!(raw["tickets"].as_array().unwrap().iter().all(|t| t["source_library"] == json!(f.items)));
    let mut reopened = Session::default();
    reopened.execute("file.open", json!({"path":f.s.path})).unwrap();
    metadata(&mut reopened, 303);
    let before = reopened.project.clone();
    reopened
        .execute(
            "agent.restoreHistoryCheckpoint",
            json!({"token":token(303),"scope":scope(),"workspace":W,"checkpoint":C,"sha256":saved["checkpoint"]["sha256"]}),
        )
        .unwrap();
    assert_eq!(reopened.project, before);
    assert!(reopened.agent_bridge.tickets.iter().filter(|t| t.source_library.is_some()).all(|t| t.media.scope_json()["libraryVerified"] == false));
    read(&mut reopened, 304, &f.items);
    let h = reopened.execute("agent.readHistory", json!({"token":token(304),"scope":scope(),"commit":token(300),"undo":token(301),"projection":true})).unwrap();
    assert_eq!(h["redoAvailable"], true);
    reopened.execute("agent.redo", json!({"token":token(304),"commit":token(300),"undo":token(301)})).unwrap();
    assert_eq!(reopened.project.sequences().count(), 0);
    assert_eq!(reopened.state.active_sequence, None);
}
#[test]
fn independent_library_forged_checkpoint_purpose_selection_scope_and_collateral_changes_refuse_even_with_recomputed_checksum() {
    let _serial = serial();
    let (f, _saved, path) = saved_undone();
    let raw: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    for mode in ["downgrade", "remove-purpose", "unselected-target", "foreign-scope", "collateral", "undo-selection"] {
        let mut v = raw.clone();
        match mode {
            "downgrade" => v["version"] = json!(2),
            "undo-selection" => v["tickets"][1]["source_library"] = json!([0]),
            "remove-purpose" => {
                for t in v["tickets"].as_array_mut().unwrap() {
                    t.as_object_mut().unwrap().remove("source_library");
                }
            }
            "unselected-target" => v["tickets"][0]["source_library"] = json!([]),
            "foreign-scope" => v["tickets"][0]["request"]["operations"][0]["params"]["scope"]["workspaceId"] = json!("00000000-0000-4000-8000-000000000000"),
            "collateral" => {
                let idx = v["tickets"][0]["after"].as_u64().unwrap() as usize;
                let mut p: Value = serde_json::from_str(v["projects"][idx].as_str().unwrap()).unwrap();
                p["name"] = json!("Forged collateral");
                v["projects"][idx] = json!(serde_json::to_string(&p).unwrap());
            }
            _ => unreachable!(),
        }
        let bytes = serde_json::to_vec(&v).unwrap();
        fs::write(&path, &bytes).unwrap();
        let mut reopened = Session::default();
        reopened.execute("file.open", json!({"path":f.s.path})).unwrap();
        metadata(&mut reopened, 305);
        let before = state(&reopened);
        assert!(
            reopened
                .execute(
                    "agent.restoreHistoryCheckpoint",
                    json!({"token":token(305),"scope":scope(),"workspace":W,"checkpoint":C,"sha256":format!("{:x}",Sha256::digest(&bytes))})
                )
                .is_err(),
            "{mode}"
        );
        assert_eq!(state(&reopened), before, "{mode}");
    }
}
#[test]
fn independent_library_read_does_not_bind_a_foreign_receipt_scope_or_borrow_an_unrequested_source() {
    let _serial = serial();
    let mut f = fixture();
    read(&mut f.s, 320, &[f.items[0]]);
    let before = state(&f.s);
    let q = batch(&f, 320, &[1]);
    assert!(f.s.execute("agent.apply", q).is_err());
    let mut q = batch(&f, 320, &[0]);
    q["operations"][0]["params"]["scope"]["workspaceId"] = json!("00000000-0000-4000-8000-000000000000");
    assert!(f.s.execute("agent.apply", q).is_err());
    assert_eq!(state(&f.s), before);
}
#[test]
fn independent_library_root_read_cannot_be_reused_as_a_sequence_read_when_a_human_opens_a_real_timeline() {
    let _serial = serial();
    let mut f = fixture();
    let id = f.s.execute("file.newSequence", json!({"name":"Human sequence","fps":24})).unwrap()["sequence"].as_u64().unwrap();
    let d = read(&mut f.s, 330, &f.items);
    assert_eq!(d["sequence"], id);
    assert!(f.s.execute("agent.read", json!({"token":token(330)})).is_err());
    let before = f.s.project.clone();
    f.s.execute("agent.apply", batch(&f, 330, &[0])).unwrap();
    assert_eq!(f.s.state.active_sequence.unwrap().0, id);
    assert_eq!(f.s.project.sequence(filmcraft_project::ItemId(id)), before.sequence(filmcraft_project::ItemId(id)));
}

#[test]
fn independent_library_prepared_admission_keeps_metadata_ineligible_and_prepares_exact_group_before_one_native_publication() {
    let _serial = serial();
    let mut f = fixture();
    f.s.worker_available = true;
    f.s.agent_bridge.operations.enable_transport();
    metadata(&mut f.s, 340);
    let mut q = batch(&f, 340, &[0, 1]);
    q["validateAtPublication"] = json!(true);
    q["admissionPreparation"] = json!(token(341));
    let before = state(&f.s);
    let error = f.s.execute("agent.prepareSourceAdmission", json!({"job":token(341),"batch":q})).unwrap_err();
    assert!(error.to_string().contains("project read requires one exact authored publication"), "{error}");
    assert!(f.s.agent_bridge.admission_jobs.is_empty());
    assert_eq!(state(&f.s), before);
    let doc = read(&mut f.s, 342, &f.items);
    assert_eq!(doc["capabilities"]["sourceAdmissionPreparation"], true);
    let mut q = batch(&f, 342, &[0, 1]);
    q["validateAtPublication"] = json!(true);
    q["admissionPreparation"] = json!(token(343));
    let start = f.s.execute("agent.prepareSourceAdmission", json!({"job":token(343),"batch":q})).unwrap();
    assert_eq!(start["sequence"], Value::Null);
    let end = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        let r = f.s.execute("agent.sourceAdmissionPreparation", json!({"job":token(343)})).unwrap();
        assert_eq!(r["sequence"], Value::Null);
        if r["status"] == "prepared" {
            assert_eq!(r["progress"]["completed_checks"], 4);
            break;
        }
        assert_eq!(r["status"], "running", "{r}");
        assert!(std::time::Instant::now() < end);
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    assert_eq!(state(&f.s), before);
    let count = f.s.history.undo.len();
    let result = f.s.execute("agent.apply", q.clone()).unwrap();
    assert_eq!(f.s.history.undo.len(), count + 1);
    assert_eq!(f.s.execute("agent.apply", q).unwrap(), result);
    assert_eq!(f.s.project.sequences().count(), 0);
    assert_eq!(f.s.state.active_sequence, None);
}

#[test]
fn independent_library_checkpoint_refuses_a_missing_original_causal_receipt_before_creating_a_companion() {
    let _serial = serial();
    let mut f = fixture();
    read(&mut f.s, 350, &f.items);
    f.s.execute("agent.apply", batch(&f, 350, &[0, 1])).unwrap();
    read(&mut f.s, 351, &f.items);
    f.s.execute("agent.undo", json!({"token":token(351),"commit":token(350)})).unwrap();
    // The production ticket cache is bounded. Model that exact eviction while
    // retaining the existing native snapshots and the later owned Undo receipt.
    f.s.agent_bridge.tickets.retain(|t| t.token != token(350));
    persist(&mut f.s);
    metadata(&mut f.s, 352);
    let before = state(&f.s);
    let path = f.dir.join(format!(".poema-history-{W}-{C}.json"));
    assert!(!path.exists());
    let error = f.s.execute("agent.saveHistoryCheckpoint", json!({"token":token(352),"scope":scope(),"workspace":W,"checkpoint":C})).unwrap_err();
    assert!(error.to_string().contains("retained exact original commit"), "{error}");
    assert!(!path.exists());
    assert_eq!(state(&f.s), before);
}

#[test]
fn independent_library_postflight_uses_the_shared_background_verifier_without_consuming_a_read_and_refuses_changed_bytes_or_human_interleave() {
    let _serial = serial();
    let mut f = fixture();
    read(&mut f.s, 400, &f.items);
    f.s.execute("agent.apply", batch(&f, 400, &[0])).unwrap();
    read(&mut f.s, 401, &f.items);
    let tickets = f.s.agent_bridge.tickets.len();
    let r = validation(&mut f.s, 401);
    assert_eq!(r["sequence"], Value::Null);
    assert_eq!(f.s.agent_bridge.tickets.len(), tickets);
    // Read-only postflight must leave this same native token available to the
    // existing original binding publisher, rather than consuming its fence.
    f.s.execute("agent.apply", batch(&f, 401, &[1])).unwrap();
    read(&mut f.s, 402, &f.items);
    let job = token(140_402);
    f.s.execute("agent.cancelValidation", json!({"job":job})).unwrap();
    let before = state(&f.s);
    assert_eq!(f.s.execute(VALIDATE, json!({"token":token(402),"scope":scope(),"job":job})).unwrap()["status"], "cancelled");
    assert_eq!(state(&f.s), before);
    let job = token(140_403);
    f.s.execute(VALIDATE, json!({"token":token(402),"scope":scope(),"job":job})).unwrap();
    f.s.edit("Human source name", |p, _| {
        p.item_mut(filmcraft_project::ItemId(f.items[0])).unwrap().name = "Human renamed source".into();
        Ok(())
    })
    .unwrap();
    let before = state(&f.s);
    assert!(f.s.execute("agent.validationStatus", json!({"job":job})).is_err());
    assert_eq!(state(&f.s), before);
    read(&mut f.s, 404, &f.items);
    let mut bytes = fs::read(&f.files[0]).unwrap();
    bytes[100] ^= 1;
    fs::write(&f.files[0], bytes).unwrap();
    let before = state(&f.s);
    let job = token(140_404);
    f.s.execute(VALIDATE, json!({"token":token(404),"scope":scope(),"job":job})).unwrap();
    let end = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        let r = f.s.execute("agent.validationStatus", json!({"job":job})).unwrap();
        if r["status"] != "running" {
            assert_eq!(r["status"], "failed", "{r}");
            assert!(r.get("result").is_none());
            break;
        }
        assert!(std::time::Instant::now() < end);
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    assert_eq!(state(&f.s), before);
}

fn layout_request(f: &Fixture, n: usize, index: usize) -> Value {
    let bytes = fs::read(&f.files[index]).unwrap();
    json!({"token":token(n),"job":token(n+400_000),"scope":scope(),"background":true,"request":{"kind":"source_audio_layout","item":f.items[index],"generation":{"kind":"poema-media-generation","version":1,"sha256":format!("{:x}",Sha256::digest(&bytes)),"bytes":bytes.len()}}})
}
fn layout_result(s: &mut Session, p: &Value) -> Value {
    let mut r = s.execute("agent.inspect", p.clone()).unwrap();
    let end = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while r["status"] == "running" {
        assert!(std::time::Instant::now() < end);
        std::thread::sleep(std::time::Duration::from_millis(2));
        r = s.execute("agent.inspection", json!({"job":p["job"]})).unwrap();
    }
    assert_eq!(r["status"], "complete", "{r}");
    r
}
#[test]
fn independent_library_source_audio_layout_reuses_the_native_worker_and_original_byte_postflight_without_a_sequence_or_history_change() {
    let _serial = serial();
    let _inspection = crate::agent_bridge::inspection::TEST_SERIAL.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let mut f = fixture();
    let doc = read(&mut f.s, 500, &f.items);
    assert_eq!(doc["capabilities"]["sourceAudioLayout"], true);
    assert_eq!(doc["capabilities"]["backgroundInspection"], true);
    let before = state(&f.s);
    let p = layout_request(&f, 500, 0);
    let r = layout_result(&mut f.s, &p);
    assert_eq!(r["sequence"], Value::Null);
    assert_eq!(r["scope"], scope());
    assert_eq!(r["view"], "source_audio_layout");
    assert_eq!(r["result"]["item"], f.items[0]);
    assert_eq!(r["result"]["generation"], p["request"]["generation"]);
    assert_eq!(r["result"]["layout"], Value::Null, "WAV has no MXF-only descriptor");
    assert_eq!(state(&f.s), before);
    let job = token(500_500);
    f.s.execute(VALIDATE, json!({"token":token(500),"scope":scope(),"job":job,"inspection":p["job"]})).unwrap();
    let end = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        let r = f.s.execute("agent.validationStatus", json!({"job":job})).unwrap();
        if r["status"] != "running" {
            assert_eq!(r["status"], "complete", "{r}");
            assert_eq!(r["result"]["inspection"], p["job"]);
            assert_eq!(r["result"]["scope"], scope());
            assert_eq!(r["result"]["sequence"], Value::Null);
            break;
        }
        assert!(std::time::Instant::now() < end);
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    assert_eq!(state(&f.s), before);
}
#[test]
fn independent_library_layout_refuses_metadata_undeclared_sources_ambiguous_scope_program_reads_and_candidates_before_starting_a_worker() {
    let _serial = serial();
    let _inspection = crate::agent_bridge::inspection::TEST_SERIAL.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let mut f = fixture();
    read(&mut f.s, 510, &[f.items[0]]);
    metadata(&mut f.s, 511);
    let p = layout_request(&f, 510, 0);
    let before = state(&f.s);
    for change in [
        json!({"token":token(511)}),
        json!({"scope":{"projectId":"p_library","workspaceId":"00000000-0000-4000-8000-000000000000"}}),
        json!({"scope":{"projectId":"p_library","workspaceId":W,"anchorSequence":1}}),
        json!({"request":{"kind":"frame","frame":0,"maxSide":64}}),
        json!({"request":{"kind":"audio","startSample":0,"samples":1}}),
        json!({"candidate":token(900_001)}),
        json!({"background":false}),
        json!({"request":layout_request(&f,510,1)["request"]}),
    ] {
        let mut q = p.clone();
        for (k, v) in change.as_object().unwrap() {
            q[k] = v.clone();
        }
        assert!(f.s.execute("agent.inspect", q).is_err());
        assert_eq!(state(&f.s), before);
        assert!(f.s.agent_bridge.inspections.is_empty());
    }
}
#[test]
fn independent_library_unbound_layout_postflight_rejects_changed_source_bytes_foreign_inspections_and_human_context_without_promoting_metadata() {
    let _serial = serial();
    let _inspection = crate::agent_bridge::inspection::TEST_SERIAL.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let mut f = fixture();
    read(&mut f.s, 520, &f.items);
    let p = layout_request(&f, 520, 0);
    layout_result(&mut f.s, &p);
    read(&mut f.s, 521, &f.items);
    let before = state(&f.s);
    assert!(f.s.execute(VALIDATE, json!({"token":token(521),"scope":scope(),"job":token(500_521),"inspection":p["job"]})).is_err());
    assert_eq!(state(&f.s), before);
    let mut bytes = fs::read(&f.files[0]).unwrap();
    bytes[100] ^= 1;
    fs::write(&f.files[0], bytes).unwrap();
    let job = token(500_520);
    f.s.execute(VALIDATE, json!({"token":token(520),"scope":scope(),"job":job,"inspection":p["job"]})).unwrap();
    let end = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        let r = f.s.execute("agent.validationStatus", json!({"job":job})).unwrap();
        if r["status"] != "running" {
            assert_eq!(r["status"], "failed", "{r}");
            assert!(r.get("result").is_none());
            break;
        }
        assert!(std::time::Instant::now() < end);
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    assert_eq!(state(&f.s), before);
    f.s.state.linked_selection = !f.s.state.linked_selection;
    let before = state(&f.s);
    assert!(f.s.execute("agent.inspection", json!({"job":p["job"]})).is_err());
    assert_eq!(state(&f.s), before);
    assert_eq!(f.s.project.sequences().count(), 0);
}
