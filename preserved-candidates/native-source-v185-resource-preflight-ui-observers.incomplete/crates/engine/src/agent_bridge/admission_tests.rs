use super::{
    admission, parse, publish_batch, stage_batch,
    tests::{read, token},
};
use crate::Session;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{fs, path::PathBuf};

struct Fixture {
    s: Session,
    directory: PathBuf,
    file: PathBuf,
    item: u64,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.directory);
    }
}
fn wav(channels: u16) -> Vec<u8> {
    let size = 480u32 * u32::from(channels) * 2;
    let mut b = Vec::new();
    b.extend(b"RIFF");
    b.extend((36 + size).to_le_bytes());
    b.extend(b"WAVEfmt ");
    b.extend(16u32.to_le_bytes());
    b.extend(1u16.to_le_bytes());
    b.extend(channels.to_le_bytes());
    b.extend(48000u32.to_le_bytes());
    b.extend((48000u32 * u32::from(channels) * 2).to_le_bytes());
    b.extend((channels * 2).to_le_bytes());
    b.extend(16u16.to_le_bytes());
    b.extend(b"data");
    b.extend(size.to_le_bytes());
    b.resize(44 + size as usize, 0);
    b
}
fn fixture() -> Fixture {
    let mut entropy = [0u8; 16];
    getrandom::fill(&mut entropy).unwrap();
    let directory = std::env::temp_dir().join(format!("filmcraft-admission-{:x}", u128::from_le_bytes(entropy)));
    fs::create_dir(&directory).unwrap();
    let directory = fs::canonicalize(directory).unwrap();
    let file = directory.join("camera.wav");
    fs::write(&file, wav(2)).unwrap();
    let mut s = super::tests::setup();
    let item = crate::commands::import_streamed(&mut s, file.to_str().unwrap(), None).unwrap().0;
    Fixture { s, directory, file, item }
}
fn request(f: &Fixture, n: usize) -> Value {
    let bytes = fs::read(&f.file).unwrap();
    json!({"token":token(n),"label":"Admit this original","exact":true,"operations":[{"command":admission::COMMAND,"params":{
      "item":f.item,"path":f.file,"binding":{"projectId":"p_admission","assetId":"a_camera","mediaId":"a_camera:original","role":"original"},
      "generation":{"kind":"poema-media-generation","version":1,"sha256":format!("{:x}",Sha256::digest(&bytes)),"bytes":bytes.len()},
      "ownerReceiptSha256":"a".repeat(64),"scope":{"projectId":"p_admission","anchorSequence":f.s.state.active_sequence.unwrap().0}}}]})
}
fn state(s: &Session) -> Value {
    json!({"project":s.project,"state":s.state,"revision":s.revision,"saved":s.saved_revision,"undo":s.history.undo.len(),"redo":s.history.redo.len(),"journal":s.journal,"events":s.events.len()})
}
#[test]
fn admission_is_one_native_binding_step_without_changing_the_cut_and_owned_undo_redo_restore_the_marker() {
    let mut f = fixture();
    let req = request(&f, 1);
    read(&mut f.s, 1);
    let cut = f.s.active_sequence().unwrap().clone();
    let before = f.s.project.clone();
    let count = f.s.history.undo.len();
    let result = f.s.execute("agent.apply", req.clone()).unwrap();
    assert_eq!(f.s.history.undo.len(), count + 1);
    assert_eq!(f.s.active_sequence().unwrap(), &cut);
    assert_eq!(result["results"][0]["sourceAdmission"], true);
    assert_eq!(f.s.execute("agent.apply", req).unwrap(), result);
    assert_eq!(f.s.history.undo.len(), count + 1);
    let it = f.s.project.item(filmcraft_project::ItemId(f.item)).unwrap();
    assert!(it.metadata.contains_key(admission::KEY));
    let mut human = Session { project: before.clone(), state: f.s.state.clone(), ..Session::default() };
    human.execute("agent.bindSource", json!({"item":f.item,"binding":result["results"][0]["binding"]})).unwrap();
    assert_eq!(human.project.item(it.id).unwrap().metadata.get(super::BINDING_KEY), it.metadata.get(super::BINDING_KEY));
    read(&mut f.s, 2);
    f.s.execute("agent.undo", json!({"token":token(2),"commit":token(1)})).unwrap();
    assert_eq!(f.s.project, before);
    read(&mut f.s, 3);
    // Availability must remain readable when legitimate Undo removed the
    // film's last binding. It is metadata, not restored-file authorization.
    let history = json!({"token":token(3),"commit":token(1),"undo":token(2),
        "scope":{"projectId":"p_admission","anchorSequence":f.s.state.active_sequence.unwrap().0}});
    let unchanged = state(&f.s);
    for projection in [false, true] {
        let mut request = history.clone();
        request["projection"] = json!(projection);
        let r = f.s.execute("agent.readHistory", request).unwrap();
        assert_eq!(r["undoAvailable"], false);
        assert_eq!(r["redoAvailable"], true);
        assert_eq!(r["writeAuthority"], false);
        assert_eq!(state(&f.s), unchanged);
    }
    f.s.execute("agent.redo", json!({"token":token(3),"commit":token(1),"undo":token(2)})).unwrap();
    assert!(f.s.project.item(filmcraft_project::ItemId(f.item)).unwrap().metadata.contains_key(admission::KEY));
}

#[test]
fn admission_unbound_history_requires_the_exact_owned_redo_branch_and_film_scope() {
    for mode in ["foreign-film", "foreign-sequence", "unknown-commit", "unknown-undo", "human-edit", "replacement"] {
        let mut f = fixture();
        let req = request(&f, 101);
        read(&mut f.s, 101);
        f.s.execute("agent.apply", req).unwrap();
        read(&mut f.s, 102);
        f.s.execute("agent.undo", json!({"token":token(102),"commit":token(101)})).unwrap();
        if mode == "human-edit" {
            f.s.edit("Human label", |p, _| {
                p.item_mut(filmcraft_project::ItemId(f.item)).unwrap().name = "Human camera".into();
                Ok(())
            })
            .unwrap();
        } else if mode == "replacement" {
            f.s.project = std::sync::Arc::new((*f.s.project).clone());
        }
        read(&mut f.s, 103);
        let mut req = json!({"token":token(103),"commit":token(101),"undo":token(102),"projection":true,
            "scope":{"projectId":"p_admission","anchorSequence":f.s.state.active_sequence.unwrap().0}});
        match mode {
            "foreign-film" => req["scope"]["projectId"] = json!("foreign"),
            "foreign-sequence" => req["scope"]["anchorSequence"] = json!(999999),
            "unknown-commit" => req["commit"] = json!(token(999997)),
            "unknown-undo" => req["undo"] = json!(token(999998)),
            _ => {}
        }
        let before = state(&f.s);
        assert!(f.s.execute("agent.readHistory", req).is_err(), "{mode}");
        assert_eq!(state(&f.s), before, "{mode}");
    }
}

#[test]
fn admission_last_binding_undo_checkpoint_reopens_with_owned_redo_without_renewing_source_authority() {
    let mut f = fixture();
    let path = f.directory.join("unbound-history.fcproj");
    f.s.path = Some(path.to_str().unwrap().into());
    filmcraft_format::atomic_write(&path, &filmcraft_format::encode_with_view(&f.s.project, Some(&f.s.project_view()), false)).unwrap();
    f.s.saved_revision = f.s.revision;
    let cut = f.s.active_sequence().unwrap().clone();
    let req = request(&f, 201);
    read(&mut f.s, 201);
    f.s.execute("agent.apply", req).unwrap();
    read(&mut f.s, 202);
    f.s.execute("agent.undo", json!({"token":token(202),"commit":token(201)})).unwrap();
    assert!(!super::history::has_original(&f.s.project, None).unwrap());
    let scope = json!({"projectId":"p_admission","anchorSequence":f.s.state.active_sequence.unwrap().0});
    let workspace = "ca461f8a-ed89-4318-8e99-c18f4589e693";
    let checkpoint = "20fe8b29-c0d8-4d0e-80f3-37e651fd74a8";
    let before = state(&f.s);
    f.s.execute("agent.readHistoryCheckpointBasis", json!({"token":token(203),"scope":scope})).unwrap();
    let saved = f
        .s
        .execute("agent.saveHistoryCheckpoint", json!({"token":token(203),"scope":scope,"workspace":workspace,"checkpoint":checkpoint,"metadataBasis":true}))
        .unwrap();
    assert_eq!(state(&f.s), before);
    filmcraft_format::atomic_write(&path, &filmcraft_format::encode_with_view(&f.s.project, Some(&f.s.project_view()), false)).unwrap();
    let mut reopened = Session::default();
    reopened.execute("file.open", json!({"path":path})).unwrap();
    assert!(!super::history::has_original(&reopened.project, None).unwrap());
    assert!(
        reopened.execute("agent.readHistoryCheckpointBasis", json!({"token":token(204),"scope":scope})).is_err(),
        "a clean unbound project cannot invent scope before verified restoration"
    );
    read(&mut reopened, 205);
    let restore = json!({"token":token(205),"scope":scope,"workspace":workspace,"checkpoint":checkpoint,"sha256":saved["checkpoint"]["sha256"]});
    let mut foreign = restore.clone();
    foreign["scope"]["projectId"] = json!("foreign");
    let before = state(&reopened);
    assert!(reopened.execute("agent.restoreHistoryCheckpoint", foreign).is_err());
    assert_eq!(state(&reopened), before);
    // Reach the deferred scope proof with a checksum-correct but semantically
    // invalid capsule. Neither a matching envelope nor detached bindings may
    // invent an owned Redo branch for a clean unbound reopened project.
    let companion = f.directory.join(format!(".poema-history-{workspace}-{checkpoint}.json"));
    let raw = fs::read(&companion).unwrap();
    for mode in ["foreign-scope", "unowned-head"] {
        let mut capsule: Value = serde_json::from_slice(&raw).unwrap();
        let mut forged = restore.clone();
        if mode == "foreign-scope" {
            capsule["scope"]["projectId"] = json!("foreign");
            forged["scope"]["projectId"] = json!("foreign");
        } else {
            capsule["owned_head"] = json!(false);
        }
        let bytes = serde_json::to_vec(&capsule).unwrap();
        forged["sha256"] = json!(format!("{:x}", Sha256::digest(&bytes)));
        fs::write(&companion, bytes).unwrap();
        let error = reopened.execute("agent.restoreHistoryCheckpoint", forged).unwrap_err().to_string();
        assert!(error.contains("no current or exact owned restored original"), "{mode}: {error}");
        assert_eq!(state(&reopened), before, "{mode}");
    }
    fs::write(&companion, raw).unwrap();
    reopened.execute("agent.restoreHistoryCheckpoint", restore).unwrap();
    assert!(!super::history::has_original(&reopened.project, None).unwrap());
    read(&mut reopened, 206);
    let h = reopened.execute("agent.readHistory", json!({"token":token(206),"commit":token(201),"undo":token(202),"scope":scope,"projection":true})).unwrap();
    assert_eq!(h["redoAvailable"], true);
    assert_eq!(h["requiresSourceValidation"], true);
    reopened.execute("agent.redo", json!({"token":token(206),"commit":token(201),"undo":token(202)})).unwrap();
    assert!(super::history::has_original(&reopened.project, Some("p_admission")).unwrap());
    assert_eq!(reopened.active_sequence().unwrap(), &cut);
}
#[test]
fn replacement_between_staging_and_publication_refuses_the_new_unbound_original() {
    let mut f = fixture();
    let req = request(&f, 4);
    read(&mut f.s, 4);
    let before = state(&f.s);
    let batch = parse("agent.apply", &req).unwrap();
    let stage = stage_batch(&f.s, &batch).unwrap();
    let mut bytes = fs::read(&f.file).unwrap();
    bytes[50] ^= 1;
    fs::write(&f.file, bytes).unwrap();
    assert!(publish_batch(&mut f.s, &batch, stage, &req, None).is_err());
    assert_eq!(state(&f.s), before);
}
#[test]
fn updated_owner_generation_cannot_adopt_stale_native_import_metadata() {
    let mut f = fixture();
    fs::write(&f.file, wav(1)).unwrap();
    let req = request(&f, 5);
    read(&mut f.s, 5);
    let before = state(&f.s);
    let error = f.s.execute("agent.apply", req).unwrap_err().to_string();
    assert!(error.contains("fresh native stream metadata"), "{error}");
    assert_eq!(state(&f.s), before);
}
#[test]
fn admission_rejects_malformed_foreign_duplicate_and_mixed_requests_without_consuming_history() {
    let mut f = fixture();
    let req = request(&f, 6);
    read(&mut f.s, 6);
    let before = state(&f.s);
    let mut bad = Vec::new();
    for (key, value) in [("item", json!(0)), ("path", json!("relative.wav")), ("ownerReceiptSha256", json!("bad")), ("surprise", json!(true))] {
        let mut r = req.clone();
        r["operations"][0]["params"][key] = value;
        bad.push(r);
    }
    let mut r = req.clone();
    r["operations"][0]["params"]["generation"]["sha256"] = json!("0".repeat(64));
    bad.push(r);
    let mut r = req.clone();
    r["operations"][0]["params"]["scope"]["projectId"] = json!("foreign");
    bad.push(r);
    let mut r = req.clone();
    r["exact"] = json!(false);
    bad.push(r);
    let mut r = req.clone();
    r["operations"] = json!([req["operations"][0], req["operations"][0]]);
    bad.push(r);
    for r in bad {
        assert!(f.s.execute("agent.apply", r).is_err());
        assert_eq!(state(&f.s), before);
    }
    f.s.execute("agent.bindSource", json!({"item":f.item,"binding":req["operations"][0]["params"]["binding"]})).unwrap();
    let req = request(&f, 7);
    read(&mut f.s, 7);
    let before = state(&f.s);
    assert!(f.s.execute("agent.apply", req).is_err());
    assert_eq!(state(&f.s), before);
}
#[test]
fn stopped_admission_cannot_publish_a_prepared_binding_or_erase_the_human_edit() {
    let mut f = fixture();
    let req = request(&f, 8);
    read(&mut f.s, 8);
    let control = f.s.agent_operation_control();
    let before = state(&f.s);
    let result = super::operation_control::run(&mut f.s, "agent.apply", &req, |s, p| {
        let batch = parse("agent.apply", p)?;
        let stage = stage_batch(s, &batch)?;
        control.cancel(&json!({"token":token(8)}))?;
        publish_batch(s, &batch, stage, p, None)
    });
    assert!(result.is_err());
    assert_eq!(state(&f.s), before);
}
#[test]
fn rejected_admission_preserves_the_humans_redo_branch() {
    let mut f = fixture();
    let item = f.s.project.item(filmcraft_project::ItemId(f.item)).unwrap().clone();
    f.s.edit("Human label", |p, _| {
        p.item_mut(item.id).unwrap().name = "Human camera".into();
        Ok(())
    })
    .unwrap();
    f.s.undo().unwrap();
    let mut req = request(&f, 9);
    req["operations"][0]["params"]["generation"]["bytes"] = json!(1);
    read(&mut f.s, 9);
    let before = state(&f.s);
    assert!(f.s.execute("agent.apply", req).is_err());
    assert_eq!(state(&f.s), before);
    f.s.redo().unwrap();
    assert_eq!(f.s.project.item(item.id).unwrap().name, "Human camera");
}

#[test]
fn admitted_current_bytes_do_not_inherit_the_unbound_items_old_decoder_cache() {
    let mut f = fixture();
    let id = filmcraft_project::ItemId(f.item);
    let old = f.s.media.full_res_source(&f.s.project, id, f.s.services.as_ref()).unwrap();
    assert_eq!(old.audio(0, 1, 48000).unwrap().channels[0][0], 0.0);
    let mut bytes = fs::read(&f.file).unwrap();
    bytes[44..48].copy_from_slice(&[255, 127, 255, 127]);
    fs::write(&f.file, bytes).unwrap();
    let req = request(&f, 10);
    read(&mut f.s, 10);
    f.s.execute("agent.apply", req).unwrap();
    let fresh = f.s.media.full_res_source(&f.s.project, id, f.s.services.as_ref()).unwrap();
    assert!(!std::sync::Arc::ptr_eq(&old, &fresh));
    assert!(fresh.audio(0, 1, 48000).unwrap().channels[0][0] > 0.99);
}

#[test]
fn owned_redo_rechecks_the_reintroduced_original_and_preserves_history_on_rejection() {
    let mut f = fixture();
    let req = request(&f, 11);
    read(&mut f.s, 11);
    f.s.execute("agent.apply", req).unwrap();
    read(&mut f.s, 12);
    f.s.execute("agent.undo", json!({"token":token(12),"commit":token(11)})).unwrap();
    let original = fs::read(&f.file).unwrap();
    let mut replaced = original.clone();
    replaced[50] ^= 1;
    fs::write(&f.file, replaced).unwrap();
    read(&mut f.s, 13);
    let before = state(&f.s);
    assert!(f.s.execute("agent.redo", json!({"token":token(13),"commit":token(11),"undo":token(12)})).is_err());
    assert_eq!(state(&f.s), before);
    fs::write(&f.file, original).unwrap();
    read(&mut f.s, 14);
    f.s.execute("agent.redo", json!({"token":token(14),"commit":token(11),"undo":token(12)})).unwrap();
    assert!(f.s.project.item(filmcraft_project::ItemId(f.item)).unwrap().metadata.contains_key(admission::KEY));
}

fn group_fixture(count: usize) -> (Fixture, Vec<(u64, PathBuf)>) {
    let mut f = fixture();
    let mut sources = vec![(f.item, f.file.clone())];
    let mut paths = Vec::new();
    for i in 1..count {
        let path = f.directory.join(format!("source-{i:04}.wav"));
        fs::write(&path, wav(2)).unwrap();
        paths.push(path);
    }
    if !paths.is_empty() {
        let imported = f.s.execute("file.import", json!({"paths":paths})).unwrap();
        for (id, path) in imported["items"].as_array().unwrap().iter().zip(paths) {
            sources.push((id.as_u64().unwrap(), path));
        }
    }
    assert_eq!(sources.len(), count);
    (f, sources)
}
fn group_request(f: &Fixture, sources: &[(u64, PathBuf)], n: usize) -> Value {
    let original = request(f, n);
    let operations: Vec<_> = sources
        .iter()
        .enumerate()
        .map(|(i, (item, path))| {
            let mut op = original["operations"][0].clone();
            op["params"]["item"] = json!(item);
            op["params"]["path"] = json!(path);
            op["params"]["binding"]["assetId"] = json!(format!("a_group_{i}"));
            op["params"]["binding"]["mediaId"] = json!(format!("a_group_{i}:original"));
            op["params"]["ownerReceiptSha256"] = json!(format!("{:064x}", i + 1));
            op
        })
        .collect();
    json!({"token":token(n),"label":"Bind the selected originals","exact":true,"operations":operations})
}

#[test]
fn two_thousand_original_bindings_share_one_history_step_and_complete_saved_owned_undo_redo() {
    let (mut f, sources) = group_fixture(2000);
    let path = f.directory.join("binding-group.fcproj");
    f.s.path = Some(path.to_str().unwrap().into());
    f.s.edit("Earlier human label", |p, _| {
        p.item_mut(filmcraft_project::ItemId(f.item)).unwrap().name = "Human camera".into();
        Ok(())
    })
    .unwrap();
    let before = f.s.project.clone();
    let before_bytes = filmcraft_format::encode(&before, false);
    let earlier_history = f.s.history.undo.clone();
    let cut = f.s.active_sequence().unwrap().clone();
    let req = group_request(&f, &sources, 71001);
    let doc = read(&mut f.s, 71001);
    assert_eq!(doc["capabilities"]["sourceAdmissionBatch"], true);
    let result = f.s.execute("agent.apply", req.clone()).unwrap();
    assert_eq!(result["results"].as_array().unwrap().len(), 2000);
    assert_eq!(f.s.history.undo.len(), earlier_history.len() + 1);
    assert_eq!(f.s.active_sequence().unwrap(), &cut);
    for (i, (item, _)) in sources.iter().enumerate() {
        let source = f.s.project.item(filmcraft_project::ItemId(*item)).unwrap();
        let binding: Value = serde_json::from_str(&source.metadata[super::BINDING_KEY]).unwrap();
        assert_eq!(binding["assetId"], format!("a_group_{i}"));
        assert!(source.metadata.contains_key(admission::KEY));
    }
    let committed = f.s.project.clone();
    read(&mut f.s, 71002);
    f.s.execute("agent.undo", json!({"token":token(71002),"commit":token(71001)})).unwrap();
    assert_eq!(filmcraft_format::encode(&f.s.project, false), before_bytes);
    assert_eq!(f.s.history.undo.len(), earlier_history.len());
    for ((label, p), (old_label, old)) in f.s.history.undo.iter().zip(&earlier_history) {
        assert_eq!(label, old_label);
        assert!(std::sync::Arc::ptr_eq(p, old));
    }
    read(&mut f.s, 71003);
    f.s.execute("agent.redo", json!({"token":token(71003),"commit":token(71001),"undo":token(71002)})).unwrap();
    assert_eq!(filmcraft_format::encode(&f.s.project, false), filmcraft_format::encode(&committed, false));
    assert_eq!(f.s.execute("agent.apply", req).unwrap(), result);
    let committed_bytes = filmcraft_format::encode(&f.s.project, false);
    filmcraft_format::atomic_write(&path, &filmcraft_format::encode_with_view(&f.s.project, Some(&f.s.project_view()), false)).unwrap();
    f.s.saved_revision = f.s.revision;
    let scope = json!({"projectId":"p_admission","anchorSequence":f.s.state.active_sequence.unwrap().0});
    let workspace = "3b8c905c-87ac-4f64-ae77-b479f2822064";
    let checkpoint = "69528dfa-f370-48d0-b710-9a7eeb5aa13d";
    f.s.execute("agent.readHistoryCheckpointBasis", json!({"token":token(71004),"scope":scope})).unwrap();
    let saved = f
        .s
        .execute("agent.saveHistoryCheckpoint", json!({"token":token(71004),"scope":scope,"workspace":workspace,"checkpoint":checkpoint,"metadataBasis":true}))
        .unwrap();
    assert!(saved["checkpoint"]["bytes"].as_u64().unwrap() < 32 * 1024 * 1024);
    let mut reopened = Session::default();
    reopened.execute("file.open", json!({"path":path})).unwrap();
    assert_eq!(filmcraft_format::encode(&reopened.project, false), committed_bytes);
    read(&mut reopened, 71005);
    reopened
        .execute(
            "agent.restoreHistoryCheckpoint",
            json!({"token":token(71005),"scope":scope,"workspace":workspace,"checkpoint":checkpoint,"sha256":saved["checkpoint"]["sha256"]}),
        )
        .unwrap();
    read(&mut reopened, 71006);
    reopened.execute("agent.undo", json!({"token":token(71006),"commit":token(71001)})).unwrap();
    assert_eq!(filmcraft_format::encode(&reopened.project, false), before_bytes);
    read(&mut reopened, 71007);
    reopened.execute("agent.redo", json!({"token":token(71007),"commit":token(71001),"undo":token(71006)})).unwrap();
    assert_eq!(filmcraft_format::encode(&reopened.project, false), committed_bytes);
}

#[test]
fn binding_groups_reject_mixed_duplicate_foreign_and_oversized_inputs_and_keep_the_existing_redo_branch() {
    let (mut f, sources) = group_fixture(3);
    f.s.edit("Earlier human label", |p, _| {
        p.item_mut(filmcraft_project::ItemId(f.item)).unwrap().name = "Human camera".into();
        Ok(())
    })
    .unwrap();
    f.s.undo().unwrap();
    let original = group_request(&f, &sources, 72000);
    let mut bad = Vec::new();
    let mut req = original.clone();
    req["exact"] = json!(false);
    bad.push(req);
    for field in ["item", "ownerReceiptSha256"] {
        let mut req = original.clone();
        req["operations"][1]["params"][field] = req["operations"][0]["params"][field].clone();
        bad.push(req);
    }
    let mut req = original.clone();
    req["operations"][1]["params"]["binding"]["assetId"] = req["operations"][0]["params"]["binding"]["assetId"].clone();
    bad.push(req);
    let mut req = original.clone();
    req["operations"][1]["params"]["scope"]["anchorSequence"] = json!(999999);
    bad.push(req);
    let mut req = original.clone();
    req["operations"][1] = json!({"command":"edit.undo","params":{}});
    bad.push(req);
    let mut req = original.clone();
    req["operations"][2]["params"]["generation"]["sha256"] = json!("0".repeat(64));
    bad.push(req);
    let mut req = original.clone();
    req["operations"] = json!(vec![req["operations"][0].clone(); 2001]);
    bad.push(req);
    let mut req = original.clone();
    req["operations"] = json!(vec![json!({"command":"timeline.razor","params":{}}); 33]);
    bad.push(req);
    for (i, mut req) in bad.into_iter().enumerate() {
        let n = 72000 + i;
        req["token"] = json!(token(n));
        read(&mut f.s, n);
        let before = state(&f.s);
        assert!(f.s.execute("agent.apply", req).is_err());
        assert_eq!(state(&f.s), before);
    }
    f.s.redo().unwrap();
    assert_eq!(f.s.project.item(filmcraft_project::ItemId(f.item)).unwrap().name, "Human camera");
}

#[test]
fn binding_group_publication_rechecks_every_original_after_staging() {
    for changed in 0..3 {
        let (mut f, sources) = group_fixture(3);
        let req = group_request(&f, &sources, 73001);
        read(&mut f.s, 73001);
        let before = state(&f.s);
        let batch = parse("agent.apply", &req).unwrap();
        let stage = stage_batch(&f.s, &batch).unwrap();
        assert_eq!(stage.admission.len(), 3);
        let path = &sources[changed].1;
        let mut bytes = fs::read(path).unwrap();
        *bytes.last_mut().unwrap() ^= 1;
        fs::write(path, bytes).unwrap();
        assert!(publish_batch(&mut f.s, &batch, stage, &req, None).is_err());
        assert_eq!(state(&f.s), before);
    }
}

#[test]
fn stopped_binding_group_keeps_all_sources_unbound_and_the_prior_project_history() {
    let (mut f, sources) = group_fixture(3);
    let req = group_request(&f, &sources, 74001);
    read(&mut f.s, 74001);
    let before = state(&f.s);
    let control = f.s.agent_operation_control();
    let result = super::operation_control::run(&mut f.s, "agent.apply", &req, |s, p| {
        let batch = parse("agent.apply", p)?;
        let stage = stage_batch(s, &batch)?;
        control.cancel(&json!({"token":token(74001)}))?;
        publish_batch(s, &batch, stage, p, None)
    });
    assert!(result.is_err());
    assert_eq!(state(&f.s), before);
}

#[test]
fn saved_binding_groups_refuse_detached_or_missing_member_receipts_without_adopting_history() {
    let (mut f, sources) = group_fixture(3);
    let path = f.directory.join("group-receipts.fcproj");
    f.s.path = Some(path.to_str().unwrap().into());
    let req = group_request(&f, &sources, 75001);
    read(&mut f.s, 75001);
    f.s.execute("agent.apply", req).unwrap();
    filmcraft_format::atomic_write(&path, &filmcraft_format::encode_with_view(&f.s.project, Some(&f.s.project_view()), false)).unwrap();
    f.s.saved_revision = f.s.revision;
    let scope = json!({"projectId":"p_admission","anchorSequence":f.s.state.active_sequence.unwrap().0});
    let workspace = "109998ea-0771-4a48-91d2-3dd89a71f6c9";
    let checkpoint = "87b3db39-24b7-4f53-a268-0275f52db27a";
    f.s.execute("agent.readHistoryCheckpointBasis", json!({"token":token(75002),"scope":scope})).unwrap();
    f.s.execute("agent.saveHistoryCheckpoint", json!({"token":token(75002),"scope":scope,"workspace":workspace,"checkpoint":checkpoint,"metadataBasis":true}))
        .unwrap();
    let companion = f.directory.join(format!(".poema-history-{workspace}-{checkpoint}.json"));
    let original: Value = serde_json::from_slice(&fs::read(&companion).unwrap()).unwrap();
    for (i, mode) in ["asset", "generation", "missing", "duplicate", "extra"].into_iter().enumerate() {
        let mut changed = original.clone();
        let result = &mut changed["tickets"][0]["result"]["results"];
        match mode {
            "asset" => result[0]["binding"]["assetId"] = json!("a_foreign"),
            "generation" => result[0]["generation"]["sha256"] = json!("f".repeat(64)),
            "missing" => {
                result.as_array_mut().unwrap().pop();
            }
            "duplicate" => result[1] = result[0].clone(),
            "extra" => result[0]["writeAuthority"] = json!(true),
            _ => panic!("unknown generated receipt mutation"),
        }
        let raw = serde_json::to_vec(&changed).unwrap();
        fs::write(&companion, &raw).unwrap();
        let mut fresh = Session::default();
        fresh.execute("file.open", json!({"path":path})).unwrap();
        let n = 75010 + i;
        read(&mut fresh, n);
        let before = state(&fresh);
        let sha256 = format!("{:x}", Sha256::digest(&raw));
        assert!(
            fresh
                .execute(
                    "agent.restoreHistoryCheckpoint",
                    json!({"token":token(n),"scope":scope,"workspace":workspace,"checkpoint":checkpoint,"sha256":sha256})
                )
                .is_err(),
            "{mode}"
        );
        assert_eq!(state(&fresh), before);
    }
}

fn prepared_request(f: &mut Fixture, n: usize) -> Value {
    f.s.worker_available = true;
    f.s.agent_bridge.operations.enable_transport();
    read(&mut f.s, n);
    let mut request = request(f, n);
    request["validateAtPublication"] = json!(true);
    request["admissionPreparation"] = json!(token(n + 1));
    request
}
fn prepare(f: &mut Fixture, request: &Value) -> Value {
    let id = request["admissionPreparation"].clone();
    f.s.execute("agent.prepareSourceAdmission", json!({"job":id,"batch":request})).unwrap();
    let until = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        let reply = f.s.execute("agent.sourceAdmissionPreparation", json!({"job":id})).unwrap();
        if reply["status"] != "running" {
            assert_eq!(reply["status"], "prepared", "{reply}");
            return reply;
        }
        assert!(std::time::Instant::now() < until);
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
}
fn workers_settled() {
    let until = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while super::snapshot_jobs::worker_count() != 0 {
        assert!(std::time::Instant::now() < until);
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
}
#[test]
fn prepared_admission_hashes_and_probes_without_a_writer_then_rechecks_complete_bytes_once_before_one_publication() {
    let _serial = super::snapshot_jobs::TEST_SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let mut f = fixture();
    let request = prepared_request(&mut f, 81000);
    let before = state(&f.s);
    let bytes = fs::metadata(&f.file).unwrap().len();
    let ready = prepare(&mut f, &request);
    assert_eq!(state(&f.s), before);
    assert_eq!(ready["writeAuthority"], false);
    assert_eq!(ready["requiresFinalValidation"], true);
    assert_eq!(ready["progress"]["bytes_read"], (2 * bytes).to_string());
    assert_eq!(ready["progress"]["completed_checks"], 2);
    let receipt = f.s.execute("agent.apply", request.clone()).unwrap();
    assert_eq!(receipt["results"][0]["sourceAdmission"], true);
    assert_eq!(f.s.agent_bridge.operations.progress(&token(81000))["bytes_read"], bytes.to_string());
    assert_eq!(
        f.s.active_sequence(),
        serde_json::from_value::<filmcraft_project::Project>(before["project"].clone()).unwrap().sequence(f.s.state.active_sequence.unwrap())
    );
    assert_eq!(f.s.history.undo.len() as u64, before["undo"].as_u64().unwrap() + 1);
    f.s.execute("agent.cancelSourceAdmissionPreparation", json!({"job":request["admissionPreparation"]})).unwrap();
    assert_eq!(f.s.execute("agent.apply", request).unwrap(), receipt, "historical receipt never requires the expired preparation or repeats admission");
    read(&mut f.s, 81002);
    f.s.execute("agent.undo", json!({"token":token(81002),"commit":token(81000)})).unwrap();
    read(&mut f.s, 81003);
    f.s.execute("agent.redo", json!({"token":token(81003),"commit":token(81000),"undo":token(81002)})).unwrap();
    assert!(f.s.project.item(filmcraft_project::ItemId(f.item)).unwrap().metadata.contains_key(admission::KEY));
    workers_settled();
}
#[test]
fn prepared_admission_refuses_changed_bytes_forged_batches_and_missing_proofs_without_a_synchronous_fallback() {
    let _serial = super::snapshot_jobs::TEST_SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let mut f = fixture();
    let request = prepared_request(&mut f, 81100);
    prepare(&mut f, &request);
    let before = state(&f.s);
    for mode in ["label", "job", "generation", "authority", "mixed", "no-proof"] {
        let mut bad = request.clone();
        match mode {
            "label" => bad["label"] = json!("Different gesture"),
            "job" => bad["admissionPreparation"] = json!(token(81999)),
            "generation" => bad["operations"][0]["params"]["generation"]["sha256"] = json!("b".repeat(64)),
            "authority" => bad["authority"] = json!("document_state"),
            "mixed" => bad["operations"].as_array_mut().unwrap().push(json!({"command":"timeline.razor","params":{"clip":1,"time":1}})),
            _ => {
                bad.as_object_mut().unwrap().remove("admissionPreparation");
            }
        }
        assert!(f.s.execute("agent.apply", bad).is_err(), "{mode}");
        assert_eq!(state(&f.s), before);
    }
    let mut bytes = fs::read(&f.file).unwrap();
    *bytes.last_mut().unwrap() ^= 1;
    fs::write(&f.file, &bytes).unwrap();
    assert!(f.s.execute("agent.apply", request).is_err());
    assert_eq!(state(&f.s), before);
    workers_settled();
}
#[test]
fn prepared_admission_human_interleave_and_both_stop_doors_expire_unpublished_work_without_touching_human_history() {
    let _serial = super::snapshot_jobs::TEST_SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    for (index, mode) in ["human", "transport-stop", "job-stop", "pre-stop"].iter().enumerate() {
        let mut f = fixture();
        let n = 81200 + index * 10;
        let request = prepared_request(&mut f, n);
        if *mode == "pre-stop" {
            f.s.execute("agent.cancelSourceAdmissionPreparation", json!({"job":request["admissionPreparation"]})).unwrap();
            assert!(f.s.execute("agent.prepareSourceAdmission", json!({"job":request["admissionPreparation"],"batch":request})).is_err());
        } else {
            prepare(&mut f, &request);
        }
        if *mode == "human" {
            f.s.execute("timeline.setTrack", json!({"track":"V1","name":"Later human edit"})).unwrap();
        }
        if *mode == "transport-stop" {
            assert_eq!(f.s.agent_bridge.operations.cancel(&json!({"token":request["token"]})).unwrap()["cancellationAccepted"], true);
        }
        if *mode == "job-stop" {
            f.s.execute("agent.cancelSourceAdmissionPreparation", json!({"job":request["admissionPreparation"]})).unwrap();
        }
        let before = state(&f.s);
        assert!(f.s.execute("agent.apply", request).is_err(), "{mode}");
        assert_eq!(state(&f.s), before);
        workers_settled();
    }
}
#[test]
fn admission_preparation_and_final_verification_have_fixed_separate_budgets_without_extending_ordinary_operations() {
    use super::media_guard::Check;
    use std::sync::{Arc, atomic::AtomicBool};
    let now = web_time::Instant::now();
    let old = now - std::time::Duration::from_secs(46);
    assert!(Check::new(old).check().is_err());
    assert!(Check::for_admission_preparation(old, Arc::new(AtomicBool::new(false))).check().is_ok());
    assert!(Check::for_admission_publication(old).check().is_ok());
    assert!(Check::for_admission_publication(now - std::time::Duration::from_secs(241)).check().is_err());
    assert!(Check::for_admission_preparation(now - std::time::Duration::from_secs(901), Arc::new(AtomicBool::new(false))).check().is_err());
    let mut f = fixture();
    let request = prepared_request(&mut f, 81300);
    let wire = json!({"command":"agent.apply","params":request});
    assert_eq!(crate::session_worker::control_reply_wait_ms("engine.execute", &wire), 242_000);
    let mut ordinary = wire.clone();
    ordinary["params"].as_object_mut().unwrap().remove("admissionPreparation");
    assert_eq!(crate::session_worker::control_reply_wait_ms("engine.execute", &ordinary), 60_000);
    ordinary["command"] = json!("agent.undo");
    assert_eq!(crate::session_worker::control_reply_wait_ms("engine.execute", &ordinary), 60_000);
}
