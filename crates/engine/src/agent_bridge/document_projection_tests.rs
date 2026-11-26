use super::{
    assembly_tests,
    tests::{exact_batch, read, token},
};
use crate::Session;
use filmcraft_project::{ItemId, MediaRef};
use serde_json::{Value, json};
use std::{
    fs,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};
struct Scratch(PathBuf);
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn setup() -> (Session, Scratch, u64) {
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    let (mut s, item) = assembly_tests::fixture();
    let dir = Scratch(crate::temp_dir().join(format!("filmcraft-document-projection-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed))));
    fs::create_dir(&dir.0).unwrap();
    let file = dir.0.join("original.bin");
    fs::write(&file, b"original complete generation").unwrap();
    Arc::make_mut(&mut s.project).item_mut(ItemId(item)).unwrap().as_media_mut().unwrap().media = MediaRef::File { path: file.to_string_lossy().into_owned() };
    (s, dir, item)
}
fn project(s: &mut Session, n: usize, basis: usize) -> Value {
    s.execute("agent.projectDocument", json!({"token":token(n),"basis":token(basis),"mediaScope":{}})).unwrap()
}
fn state(s: &Session) -> Value {
    json!({"project":s.project,"state":s.state,"revision":s.revision,"saved":s.saved_revision,"undo":s.history.undo,"redo":s.history.redo,"journal":s.journal})
}
fn validate(s: &mut Session, n: usize, job: usize) {
    s.execute("agent.validateSnapshot", json!({"token":token(n),"job":token(job)})).unwrap();
    for _ in 0..500 {
        let r = s.execute("agent.validationStatus", json!({"job":token(job)})).unwrap();
        if r["status"] == "complete" {
            return;
        }
        assert!(matches!(r["status"].as_str(), Some("running" | "ready")), "{r}");
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    panic!("generated qualification did not settle");
}
#[test]
fn document_projection_needs_native_expected_generations_and_never_invents_a_ticket_on_missing_basis() {
    let (mut s, _d, _) = setup();
    let before = state(&s);
    assert_eq!(project(&mut s, 76001, 76000)["status"], "needs_qualification");
    assert_eq!(state(&s), before);
    assert!(s.agent_bridge.tickets.is_empty());
    let full = read(&mut s, 76002);
    assert_eq!(full["capabilities"]["documentProjection"], true);
    let doc = project(&mut s, 76003, 76002);
    assert_eq!(doc["documentProjection"]["sourceValidation"], "not-performed");
    assert_eq!(doc["documentProjection"]["writeAuthority"], false);
    assert_eq!(doc["mediaScope"]["libraryVerified"], false);
    assert!(doc["sources"].as_array().unwrap().iter().all(|s| s["media"].get("File").is_none() || s["fileVerification"] == "unverified"));
    assert_eq!(doc["sequence"], full["sequence"]);
    assert_eq!(state(&s), before);
}
#[test]
fn document_projection_observes_no_files_and_cannot_renew_missing_originals_for_validation_or_publication() {
    let (mut s, d, _) = setup();
    read(&mut s, 76100);
    let observed = s.agent_bridge.media.as_ref().unwrap().clone();
    fs::remove_file(d.0.join("original.bin")).unwrap();
    let before = state(&s);
    let doc = project(&mut s, 76101, 76100);
    assert!(doc.get("protocol").is_some());
    assert!(Arc::ptr_eq(&observed, s.agent_bridge.media.as_ref().unwrap()));
    assert_eq!(state(&s), before);
    assert!(s.execute("agent.validateSnapshot", json!({"token":token(76101)})).is_err());
    let clip = s.active_sequence().unwrap().video_tracks[0].items[0].id.0;
    assert!(
        s.execute(
            "agent.apply",
            exact_batch(76101, vec![json!({"command":"timeline.trim","params":{"clip":clip,"edge":"out","mode":"regular","deltaFrames":-1}})])
        )
        .is_err()
    );
    assert_eq!(state(&s), before);
}
#[test]
fn document_projection_reads_current_human_geometry_and_keeps_existing_native_publication_checks() {
    let (mut s, _d, _) = setup();
    let history_before = s.history.undo.len();
    read(&mut s, 76200);
    let audio = s.active_sequence().unwrap().audio_tracks[0].items[0].id.0;
    s.execute("clip.audioGain", json!({"clips":[audio],"mode":"set","db":-3})).unwrap();
    let doc = project(&mut s, 76201, 76200);
    assert_eq!(doc["revision"], s.revision);
    assert_eq!(doc["sequence"]["audio"][0]["items"][0]["gainDb"], -3.0);
    let clip = s.active_sequence().unwrap().video_tracks[0].items[0].id.0;
    let result = s
        .execute(
            "agent.apply",
            exact_batch(76201, vec![json!({"command":"timeline.trim","params":{"clip":clip,"edge":"out","mode":"regular","deltaFrames":-1}})]),
        )
        .unwrap();
    assert_eq!(result["status"], "committed");
    assert_eq!(s.history.undo.len(), history_before + 2);
}
#[test]
fn document_projection_refuses_rebound_declarations_and_unobserved_scope_expansion_without_an_edit_ticket() {
    for mode in ["path", "new-source"] {
        let (mut s, d, item) = setup();
        read(&mut s, 76300);
        let _keep_old = s.project.clone();
        let count = s.agent_bridge.tickets.len();
        let other = d.0.join("other.bin");
        fs::write(&other, b"other generation").unwrap();
        let mut request = json!({"token":token(76301),"basis":token(76300),"mediaScope":{}});
        if mode == "path" {
            Arc::make_mut(&mut s.project).item_mut(ItemId(item)).unwrap().as_media_mut().unwrap().media =
                MediaRef::File { path: other.to_string_lossy().into_owned() };
        } else {
            let mut source = s.project.item(ItemId(item)).unwrap().clone();
            source.id = ItemId(9000);
            source.as_media_mut().unwrap().media = MediaRef::File { path: other.to_string_lossy().into_owned() };
            source.metadata.insert(
                super::BINDING_KEY.into(),
                json!({"projectId":"p_test","assetId":"a_added","mediaId":"a_added:original","role":"original"}).to_string(),
            );
            Arc::make_mut(&mut s.project).items.insert(source.id, source);
            request["mediaScope"] = json!({"sources":[9000]});
        }
        assert_eq!(s.execute("agent.projectDocument", request).unwrap()["status"], "needs_qualification");
        assert_eq!(s.agent_bridge.tickets.len(), count);
    }
}
#[test]
fn qualified_document_requires_one_fresh_complete_native_delivery_and_exact_current_editor_context() {
    for mode in ["success", "context", "expired"] {
        let (mut s, _d, _) = setup();
        read(&mut s, 76400);
        project(&mut s, 76401, 76400);
        assert!(s.execute("agent.qualifiedDocument", json!({"token":token(76401),"validation":token(76402)})).is_err());
        validate(&mut s, 76401, 76402);
        let request = json!({"token":token(76401),"validation":token(76402)});
        if mode == "context" {
            s.state.linked_selection = !s.state.linked_selection;
        }
        if mode == "expired" {
            std::thread::sleep(std::time::Duration::from_millis(275));
        }
        let before = state(&s);
        let result = s.execute("agent.qualifiedDocument", request.clone());
        assert_eq!(state(&s), before);
        if mode == "success" {
            let doc = result.unwrap();
            assert!(doc.get("documentProjection").is_none());
            assert!(doc["sources"].as_array().unwrap().iter().any(|s| s["fileVerification"] == "verified"));
            assert!(s.execute("agent.qualifiedDocument", request).is_err());
        } else {
            assert!(result.is_err());
        }
    }
}
