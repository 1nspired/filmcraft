use super::*;
use crate::{
    media_test_util::{make_movie, tmp_dir},
    session_worker::SessionWorker,
};
use filmcraft_media::DemoScene;
use filmcraft_project::TrackKind;
use filmcraft_time::{FrameRate, Tick, TimeRange};
use sha2::{Digest, Sha256};
use std::{fs, path::PathBuf, time::Duration};
const WORKSPACE: &str = "a93d3dc1-50aa-4ba9-b7e9-b670a4ed9c28";
fn scope() -> Value {
    json!({"projectId":"p_production","workspaceId":WORKSPACE})
}
fn token(n: usize) -> String {
    format!("qualified-production-{n:044}")
}
struct Fixture {
    s: Session,
    items: Vec<u64>,
    files: Vec<PathBuf>,
    _dir: PathBuf,
    worker: SessionWorker,
}
fn state(s: &Session) -> Value {
    json!({"project":s.project,"editor":s.state,"revision":s.revision,"saved":s.saved_revision,"path":s.path,"undo":s.history.undo.len(),"redo":s.history.redo.len(),"journal":s.journal,"events":s.events.len()})
}
fn poll(s: &mut Session, id: usize) -> Value {
    for _ in 0..1500 {
        let r = s.execute("agent.snapshotStatus", json!({"job":token(id)})).unwrap();
        if r["status"] == "complete" {
            return r["result"].clone();
        }
        assert_eq!(r["status"], "running", "{r}");
        std::thread::sleep(Duration::from_millis(2));
    }
    panic!("bounded native input verification did not finish")
}
fn fixture() -> Fixture {
    let _root = tmp_dir("qualified-production");
    let dir = fs::canonicalize(_root).unwrap();
    let mut s = Session::default();
    s.execute("file.newProject", json!({"name":"Original generated Root","workspace":scope()})).unwrap();
    let mut files = Vec::new();
    for name in ["first.mov", "second.mov"] {
        let p = dir.join(name);
        make_movie(&p, DemoScene::OceanSunset, 64, 48, 48);
        files.push(p);
    }
    let row = s.execute("file.import", json!({"paths":files})).unwrap();
    let mut items = row["items"].as_array().unwrap().iter().map(|x| x.as_u64().unwrap()).collect::<Vec<_>>();
    let src = s.source(ItemId(items[0])).unwrap();
    let audio = src.audio(0, 96000, 48000).unwrap();
    let mono = filmcraft_audio_dsp::sync::mixdown(&audio.channels.iter().map(|c| c.as_slice()).collect::<Vec<_>>());
    let wav = dir.join("recorder.wav");
    fs::write(&wav, crate::voiceover::write_wav_f32(&mono, 48000)).unwrap();
    let row = s.execute("file.import", json!({"paths":[wav]})).unwrap();
    items.push(row["items"][0].as_u64().unwrap());
    files.push(wav);
    s.execute("file.save", json!({"path":dir.join("owning.fcproj")})).unwrap();
    // Existing original library qualification/admission, genuine bytes and native import.
    s.execute("agent.readSourceLibrary", json!({"token":token(1),"scope":scope(),"projectPath":s.path,"sources":items})).unwrap();
    poll(&mut s, 1);
    let operations=items.iter().zip(&files).enumerate().map(|(i,(id,p))|{let b=fs::read(p).unwrap();json!({"command":"agent.admitSource","params":{"item":id,"path":p,"scope":scope(),"binding":{"projectId":"p_production","assetId":format!("a_{i}"),"mediaId":format!("a_{i}:original"),"role":"original"},"generation":{"kind":"poema-media-generation","version":1,"sha256":format!("{:x}",Sha256::digest(&b)),"bytes":b.len()},"ownerReceiptSha256":format!("{:x}",Sha256::digest(format!("generated owner {i}").as_bytes()))}})}).collect::<Vec<_>>();
    s.execute("agent.apply", json!({"token":token(1),"exact":true,"label":"Original native file admissions","operations":operations})).unwrap();
    let rate = FrameRate::FPS_24;
    let mut p = (*s.project).clone();
    for (k, id) in items.iter().enumerate() {
        let m = p.item_mut(ItemId(*id)).unwrap().as_media_mut().unwrap();
        m.mark_in = Some(rate.tick_of([4, 12, 8][k]));
        m.mark_out = Some(rate.tick_of([35, 31, 39][k]));
        m.markers.push(filmcraft_project::Marker {
            id: filmcraft_project::MarkerId(900 + id),
            start: rate.tick_of([16, 6, 10][k]),
            duration: Tick::ZERO,
            name: "Clap".into(),
            comment: String::new(),
            kind: filmcraft_project::MarkerKind::Comment,
            color: filmcraft_project::Label::Rose,
        });
    }
    s.project = Arc::new(p);
    s.execute("file.save", json!({})).unwrap();
    let worker = SessionWorker::new().unwrap();
    worker.install(&mut s).unwrap();
    s.agent_operation_control().enable_transport();
    Fixture { s, items, files, _dir: dir, worker }
}
fn inputs(f: &mut Fixture, id: usize, targets: Value) -> Value {
    let before = state(&f.s);
    let out = f.s.execute(READ, json!({"token":token(id),"scope":scope(),"projectPath":f.s.path,"targets":targets})).unwrap();
    assert_eq!(out["status"], "running");
    assert!(out.get("token").is_none());
    let doc = poll(&mut f.s, id);
    assert_eq!(doc["capabilities"]["sourceFileGuard"], true);
    assert_eq!(doc["capabilities"]["operationCancellation"], super::super::operation_cancellation_available(&f.s));
    assert_eq!(doc["capabilities"]["guardedOperationDeadlineMs"], 45_000);
    assert_eq!(doc["capabilities"]["largeSourceVerification"], true);
    assert_eq!(doc["capabilities"]["largeSourceVerificationDeadlineMs"], 600_000);
    assert_eq!(doc["capabilities"]["largeSourceVerificationThresholdBytes"], 1_073_741_824);
    assert_eq!(state(&f.s), before);
    assert_eq!(doc["writeAuthority"], false);
    assert_eq!(doc["capabilities"]["multicamProductionPublication"], f.s.worker_available && super::super::operation_cancellation_available(&f.s));
    assert_eq!(f.s.agent_bridge.media.as_ref().unwrap().scope_json(), doc["mediaScope"]);
    doc
}
fn plan(f: &mut Fixture, id: usize, edit: Value) -> crate::Result<Value> {
    let mut pending = f.worker.start(&mut f.s, PLAN, json!({"token":token(id),"scope":scope(),"edit":edit}), None)?;
    for _ in 0..5000 {
        if let Some(c) = pending.try_finish() {
            f.s = c.session;
            assert!(!c.reply.uncertain);
            return c.reply.result;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    panic!("original native planning worker exceeded bounded test wait")
}
fn release(f: &mut Fixture, id: usize) {
    f.s.execute("agent.cancelSnapshot", json!({"job":token(id)})).unwrap();
}
fn timeline(f: &mut Fixture) -> (u64, Vec<String>) {
    let seq =
        f.s.execute("file.newSequence", json!({"name":"Original owning timeline","width":64,"height":48,"fps":{"num":24,"den":1},"video":2,"audio":2}))
            .unwrap()["sequence"]
            .as_u64()
            .unwrap();
    let seq = if seq == 0 { f.s.state.active_sequence.unwrap().0 } else { seq };
    let rate = FrameRate::FPS_24;
    let items = f.items.clone();
    f.s.edit("Generated original source placement", |p, _| {
        for (k, item) in items[..2].iter().enumerate() {
            let link = p.alloc_id();
            let range = TimeRange::new(rate.tick_of([4, 12][k]), rate.tick_of([28, 20][k]));
            let at = rate.tick_of([0, 40][k]);
            let mut v = p.make_track_item(ItemId(*item), TrackKind::Video, at, range, rate).unwrap();
            let mut a = p.make_track_item(ItemId(*item), TrackKind::Audio, at, range, rate).unwrap();
            v.link = Some(link);
            a.link = Some(link);
            let q = p.sequence_mut(ItemId(seq)).unwrap();
            q.video_tracks[k].items.push(v);
            q.audio_tracks[k].items.push(a);
        }
        Ok(())
    })
    .unwrap();
    f.s.execute("poema.trackSequence", json!({})).unwrap();
    f.s.execute("file.save", json!({})).unwrap();
    let identities = f.s.project.sequence(ItemId(seq)).unwrap().video_tracks.iter().map(|t| t.items[0].identity.as_ref().unwrap().id.clone()).collect();
    (seq, identities)
}
#[test]
fn qualified_project_sources_use_original_files_and_worker_for_all_five_merge_and_create_methods_without_publication() {
    let _serial = super::super::snapshot_jobs::TEST_SERIAL.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let mut f = fixture();
    let mut id = 10;
    for gesture in ["merge", "create_multicam"] {
        for method in ["in", "out", "timecode", "marker", "audio"] {
            let items = if gesture == "merge" { vec![f.items[0], f.items[2]] } else { f.items.clone() };
            let doc = inputs(&mut f, id, json!({"kind":"sources","items":items}));
            assert_eq!(doc["fileDependencies"].as_array().unwrap().len(), items.len());
            let before = state(&f.s);
            let mut opts = json!({"method":method,"ignoreHours":true,"marker":"clap","offset":2});
            if gesture == "merge" {
                opts["removeVideoAudio"] = json!(true);
            } else {
                opts["audio"] = json!("switch");
                opts["cameraNames"] = json!("cameraAngle");
                opts["processedBin"] = json!(true);
            }
            let prepared = plan(&mut f, id, json!({"gesture":gesture,"options":opts})).unwrap();
            assert_eq!(prepared["status"], "prepared");
            assert_eq!(prepared["writeAuthority"], false);
            assert_ne!(prepared["beforeProjectSha256"], prepared["afterProjectSha256"]);
            assert_eq!(state(&f.s), before);
            let cached = f.s.execute(STATUS, json!({"token":token(id),"scope":scope()})).unwrap();
            assert_eq!(cached, prepared);
            assert!(plan(&mut f, id, json!({"gesture":gesture,"options":{"method":"in"}})).is_err());
            assert_eq!(state(&f.s), before);
            let owner = f.s.agent_bridge.tickets.iter().find(|t| t.token == token(id)).unwrap().multicam_production.as_ref().unwrap();
            let candidate = owner.prepared.as_ref().unwrap();
            let mut human = Session::new(f.s.services.clone());
            human.project = f.s.project.clone();
            human.state = f.s.state.clone();
            human.media = f.s.media.clone();
            human.prefs = f.s.prefs.clone();
            human.revision = f.s.revision;
            human.saved_revision = f.s.saved_revision;
            human.path = f.s.path.clone();
            let result = human.execute(prepared["humanCommand"].as_str().unwrap(), prepared["humanParams"].clone()).unwrap();
            assert_eq!(result, prepared["result"]);
            assert_eq!(project_audit::bytes(&human.project).unwrap(), project_audit::bytes(&candidate.candidate.project).unwrap());
            assert_eq!(context(&human).unwrap(), serde_json::to_value(&candidate.candidate.state).unwrap());
            eprintln!("qualified original worker {gesture}/{method}: complete owner bytes, retained exact native candidate, no publication");
            release(&mut f, id);
            id += 1;
        }
    }
}
#[test]
fn qualified_timeline_plans_resolve_current_persistent_instances_and_all_linked_sound_for_all_five_methods() {
    let _serial = super::super::snapshot_jobs::TEST_SERIAL.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let mut f = fixture();
    let (seq, identities) = timeline(&mut f);
    for (i, method) in ["in", "out", "timecode", "marker", "audio"].into_iter().enumerate() {
        let id = 30 + i;
        let doc = inputs(&mut f, id, json!({"kind":"timeline","sequence":seq,"identities":identities}));
        let rows = doc["targets"]["instances"].as_array().unwrap();
        assert_eq!(rows.len(), 4);
        assert_eq!(rows.iter().filter(|r| r["requested"] == true).count(), 2);
        assert_eq!(rows.iter().filter(|r| r["clip"]["source_in"] == FrameRate::FPS_24.tick_of(12).0).count(), 2);
        let before = state(&f.s);
        let p = plan(
            &mut f,
            id,
            json!({"gesture":"synchronize","options":{"method":method,"marker":"CLAP","ignoreHours":true,"reference":identities[0],"offset":2}}),
        )
        .unwrap();
        assert_eq!(p["humanCommand"], "clip.synchronize");
        assert_eq!(p["humanParams"]["clips"].as_array().unwrap().len(), 2);
        assert_eq!(p["nativeLinkedSelection"], true);
        assert_eq!(state(&f.s), before);
        release(&mut f, id);
    }
}
#[test]
fn metadata_admission_and_multicam_input_purposes_never_borrow_each_others_writers_or_history() {
    let _serial = super::super::snapshot_jobs::TEST_SERIAL.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let mut f = fixture();
    let items = f.items.clone();
    inputs(&mut f, 50, json!({"kind":"sources","items":items}));
    let before = state(&f.s);
    for (command, params) in [
        (
            "agent.apply",
            json!({"token":token(50),"label":"forged source writer","exact":true,"authority":"document_state","operations":[{"command":"agent.organizeProject","params":{"scope":scope(),"edit":{"type":"create","name":"not permitted","parent":null}}}]}),
        ),
        ("agent.readHistory", json!({"token":token(50),"scope":scope()})),
        ("agent.validateProjectRead", json!({"token":token(50),"scope":scope()})),
        ("agent.read", json!({"token":token(50)})),
    ] {
        assert!(f.s.execute(command, params).is_err());
        assert_eq!(state(&f.s), before);
    }
    f.s.execute("agent.readProject", json!({"token":token(51),"scope":scope(),"projectPath":f.s.path})).unwrap();
    assert!(plan(&mut f, 51, json!({"gesture":"create_multicam","options":{}})).is_err());
    f.s.execute("agent.readSourceLibrary", json!({"token":token(52),"scope":scope(),"projectPath":f.s.path,"sources":items})).unwrap();
    poll(&mut f.s, 52);
    assert!(plan(&mut f, 52, json!({"gesture":"create_multicam","options":{}})).is_err());
    assert_eq!(state(&f.s), before);
}
#[test]
fn original_admission_generation_must_match_fresh_read_bytes_and_cannot_be_replaced_by_newly_observed_bytes() {
    let _serial = super::super::snapshot_jobs::TEST_SERIAL.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let mut f = fixture();
    let mut b = fs::read(&f.files[0]).unwrap();
    let i = b.len() - 1;
    b[i] ^= 1;
    fs::write(&f.files[0], b).unwrap();
    let before = state(&f.s);
    f.s.execute(READ, json!({"token":token(60),"scope":scope(),"projectPath":f.s.path,"targets":{"kind":"sources","items":[f.items[0]]}})).unwrap();
    let mut refused = false;
    for _ in 0..1500 {
        match f.s.execute("agent.snapshotStatus", json!({"job":token(60)})) {
            Err(_) => {
                refused = true;
                break;
            }
            Ok(r) => {
                assert_eq!(r["status"], "running", "{r}");
                std::thread::sleep(Duration::from_millis(2));
            }
        }
    }
    assert!(refused);
    assert!(!is_ticket(&f.s, &token(60)));
    assert_eq!(state(&f.s), before);
}
#[test]
fn source_byte_change_after_qualified_inputs_refuses_preparation_and_never_restages_a_failed_attempt() {
    let _serial = super::super::snapshot_jobs::TEST_SERIAL.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let mut f = fixture();
    let items = f.items.clone();
    inputs(&mut f, 70, json!({"kind":"sources","items":items}));
    let mut b = fs::read(&f.files[0]).unwrap();
    let i = b.len() - 1;
    b[i] ^= 1;
    fs::write(&f.files[0], b).unwrap();
    let before = state(&f.s);
    assert!(plan(&mut f, 70, json!({"gesture":"create_multicam","options":{}})).is_err());
    assert_eq!(state(&f.s), before);
    assert!(plan(&mut f, 70, json!({"gesture":"create_multicam","options":{}})).unwrap_err().to_string().contains("already attempted"));
    assert_eq!(state(&f.s), before);
}
#[test]
fn independent_stop_revokes_queued_or_completed_private_plans_without_touching_original_history() {
    let _serial = super::super::snapshot_jobs::TEST_SERIAL.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let mut f = fixture();
    let items = f.items.clone();
    inputs(&mut f, 80, json!({"kind":"sources","items":items}));
    let before = state(&f.s);
    let mut pending = f
        .worker
        .start(&mut f.s, PLAN, json!({"token":token(80),"scope":scope(),"edit":{"gesture":"create_multicam","options":{"method":"audio"}}}), None)
        .unwrap();
    assert_eq!(pending.cancel().unwrap()["cancellationAccepted"], true);
    let mut finished = false;
    for _ in 0..5000 {
        if let Some(c) = pending.try_finish() {
            f.s = c.session;
            assert!(!c.reply.uncertain);
            finished = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    assert!(finished);
    assert_eq!(state(&f.s), before);
    assert!(f.s.execute(STATUS, json!({"token":token(80),"scope":scope()})).is_err());
    assert!(plan(&mut f, 80, json!({"gesture":"create_multicam","options":{}})).is_err());
    assert_eq!(state(&f.s), before);
}
#[test]
fn generators_remain_intrinsic_and_subclips_use_the_actual_parent_original_without_fake_bindings() {
    let _serial = super::super::snapshot_jobs::TEST_SERIAL.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let mut f = fixture();
    let g = f.s.execute("file.newBarsAndTone", json!({"name":"Intrinsic camera","seconds":2,"width":64,"height":48})).unwrap()["item"].as_u64().unwrap();
    let doc = inputs(&mut f, 90, json!({"kind":"sources","items":[g]}));
    assert_eq!(doc["fileDependencies"], json!([]));
    assert!(!f.s.project.item(ItemId(g)).unwrap().metadata.contains_key(super::super::BINDING_KEY));
    plan(&mut f, 90, json!({"gesture":"create_multicam","options":{}})).unwrap();
    release(&mut f, 90);
    let mut p = (*f.s.project).clone();
    let sub = p.add_item(
        "Original subclip",
        filmcraft_project::Label::Iris,
        filmcraft_project::ItemKind::Subclip {
            parent: ItemId(f.items[0]),
            range: TimeRange::new(FrameRate::FPS_24.tick_of(4), FrameRate::FPS_24.tick_of(20)),
            restrict_trims: true,
        },
        None,
    );
    f.s.project = Arc::new(p);
    let doc = inputs(&mut f, 91, json!({"kind":"sources","items":[sub.0]}));
    assert_eq!(doc["fileDependencies"], json!([f.items[0]]));
    assert!(!f.s.project.item(sub).unwrap().metadata.contains_key(super::super::BINDING_KEY));
    plan(&mut f, 91, json!({"gesture":"create_multicam","options":{}})).unwrap();
}
#[test]
fn later_human_cow_or_selection_and_foreign_root_refuse_the_old_purpose_without_minting_new_authority() {
    let _serial = super::super::snapshot_jobs::TEST_SERIAL.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let mut f = fixture();
    let items = f.items.clone();
    inputs(&mut f, 100, json!({"kind":"sources","items":items}));
    f.s.project = Arc::new((*f.s.project).clone());
    let before = state(&f.s);
    assert!(plan(&mut f, 100, json!({"gesture":"create_multicam","options":{}})).is_err());
    assert_eq!(state(&f.s), before);
    let items = f.items.clone();
    inputs(&mut f, 101, json!({"kind":"sources","items":items}));
    f.s.execute("project.select", json!({"items":[f.items[0]]})).unwrap();
    let before = state(&f.s);
    assert!(f.s.execute(STATUS, json!({"token":token(101),"scope":scope()})).is_err());
    assert_eq!(state(&f.s), before);
    assert!(f.s.execute(READ,json!({"token":token(102),"scope":{"projectId":"p_foreign","workspaceId":WORKSPACE},"projectPath":f.s.path,"targets":{"kind":"sources","items":[f.items[0]]}})).is_err());
    assert_eq!(state(&f.s), before);
}
#[test]
fn two_retained_candidates_are_bounded_and_only_explicit_cancellation_frees_capacity() {
    let _serial = super::super::snapshot_jobs::TEST_SERIAL.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let mut f = fixture();
    for id in 110..113 {
        let items = f.items.clone();
        inputs(&mut f, id, json!({"kind":"sources","items":items}));
        if id < 112 {
            plan(&mut f, id, json!({"gesture":"create_multicam","options":{}})).unwrap();
        }
    }
    let before = state(&f.s);
    assert!(plan(&mut f, 112, json!({"gesture":"create_multicam","options":{}})).unwrap_err().to_string().contains("two retained"));
    assert_eq!(state(&f.s), before);
    release(&mut f, 110);
    plan(&mut f, 112, json!({"gesture":"create_multicam","options":{}})).unwrap();
    assert_eq!(state(&f.s), before);
}

#[test]
fn missing_worker_or_independent_stop_cannot_prepare_or_advertise_an_agent_writer() {
    let _serial = super::super::snapshot_jobs::TEST_SERIAL.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let mut f = fixture();
    let items = f.items.clone();
    inputs(&mut f, 130, json!({"kind":"sources","items":items}));
    f.s.worker_available = false;
    let before = state(&f.s);
    assert!(
        f.s.execute(PLAN, json!({"token":token(130),"scope":scope(),"edit":{"gesture":"create_multicam","options":{}}}))
            .unwrap_err()
            .to_string()
            .contains("installed original worker")
    );
    assert_eq!(state(&f.s), before);
    f.s.worker_available = true;
    plan(&mut f, 130, json!({"gesture":"create_multicam","options":{}})).unwrap();
    let mut other = Session::new(f.s.services.clone());
    other.project = f.s.project.clone();
    other.media = f.s.media.clone();
    other.state = f.s.state.clone();
    other.path = f.s.path.clone();
    other.revision = f.s.revision;
    other.saved_revision = f.s.saved_revision;
    other.worker_available = true;
    other.execute(READ, json!({"token":token(131),"scope":scope(),"projectPath":other.path,"targets":{"kind":"sources","items":f.items}})).unwrap();
    let doc = poll(&mut other, 131);
    assert_eq!(doc["capabilities"]["multicamProductionPreparation"], false);
    let before = state(&other);
    assert!(
        other
            .execute(PLAN, json!({"token":token(131),"scope":scope(),"edit":{"gesture":"create_multicam","options":{}}}))
            .unwrap_err()
            .to_string()
            .contains("independent Stop")
    );
    assert_eq!(state(&other), before);
}

#[test]
fn exact_saved_owner_and_preferences_changes_expire_qualified_preparation_and_retained_plan_readback() {
    let _serial = super::super::snapshot_jobs::TEST_SERIAL.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    for kind in 0..3 {
        let mut f = fixture();
        let items = f.items.clone();
        inputs(&mut f, 140, json!({"kind":"sources","items":items}));
        plan(&mut f, 140, json!({"gesture":"create_multicam","options":{}})).unwrap();
        match kind {
            0 => f.s.prefs.auto_save.enabled = !f.s.prefs.auto_save.enabled,
            1 => f.s.saved_revision += 1,
            _ => f.s.saved_file.as_mut().unwrap().sha256 = "e".repeat(64),
        }
        let before = state(&f.s);
        assert!(f.s.execute(STATUS, json!({"token":token(140),"scope":scope()})).unwrap_err().to_string().contains("saved owner changed"));
        assert_eq!(state(&f.s), before);
        release(&mut f, 140);
        let items = f.items.clone();
        inputs(&mut f, 141, json!({"kind":"sources","items":items}));
        f.s.prefs.auto_save.enabled = !f.s.prefs.auto_save.enabled;
        let before = state(&f.s);
        assert!(plan(&mut f, 141, json!({"gesture":"create_multicam","options":{}})).unwrap_err().to_string().contains("saved owner changed"));
        assert_eq!(state(&f.s), before);
    }
}

#[test]
fn linked_selection_off_retains_all_physical_partners_in_readback_but_uses_exact_human_requested_targets() {
    let _serial = super::super::snapshot_jobs::TEST_SERIAL.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let mut f = fixture();
    let (seq, identities) = timeline(&mut f);
    f.s.execute("sequence.linkedSelection", json!({"on":false})).unwrap();
    let doc = inputs(&mut f, 150, json!({"kind":"timeline","sequence":seq,"identities":identities}));
    let rows = doc["targets"]["instances"].as_array().unwrap();
    assert_eq!(rows.len(), 4);
    assert_eq!(rows.iter().filter(|r| r["requested"] == true).count(), 2);
    assert_eq!(doc["targets"]["nativeLinkedSelection"], false);
    let before = state(&f.s);
    let prepared = plan(&mut f, 150, json!({"gesture":"synchronize","options":{"method":"in","reference":identities[0]}})).unwrap();
    assert_eq!(prepared["nativeLinkedSelection"], false);
    assert_eq!(prepared["humanParams"]["clips"].as_array().unwrap().len(), 2);
    assert_eq!(state(&f.s), before);
    let owner = f.s.agent_bridge.tickets.iter().find(|t| t.token == token(150)).unwrap().multicam_production.as_ref().unwrap();
    let candidate = &owner.prepared.as_ref().unwrap().candidate;
    let old = f.s.project.sequence(ItemId(seq)).unwrap();
    let new = candidate.project.sequence(ItemId(seq)).unwrap();
    assert_eq!(project_audit::value_bytes(&old.audio_tracks).unwrap(), project_audit::value_bytes(&new.audio_tracks).unwrap());
    assert_ne!(project_audit::value_bytes(&old.video_tracks).unwrap(), project_audit::value_bytes(&new.video_tracks).unwrap());
    let mut human = Session::new(f.s.services.clone());
    human.project = f.s.project.clone();
    human.state = f.s.state.clone();
    human.media = f.s.media.clone();
    human.prefs = f.s.prefs.clone();
    human.revision = f.s.revision;
    human.saved_revision = f.s.saved_revision;
    human.path = f.s.path.clone();
    human.execute("clip.synchronize", prepared["humanParams"].clone()).unwrap();
    assert_eq!(project_audit::bytes(&human.project).unwrap(), project_audit::bytes(&candidate.project).unwrap());
    assert_eq!(context(&human).unwrap(), serde_json::to_value(&candidate.state).unwrap());
}

#[test]
fn actual_metadata_history_is_not_borrowed_by_a_distinct_qualified_multicam_purpose_with_the_same_root_shape() {
    let _serial = super::super::snapshot_jobs::TEST_SERIAL.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let mut f = fixture();
    let doc = f.s.execute("agent.readProject", json!({"token":token(160),"scope":scope(),"projectPath":f.s.path})).unwrap();
    let commit=f.s.execute("agent.apply",json!({"token":token(160),"label":"Actual original metadata edit","exact":true,"authority":"document_state","operations":[{"command":"agent.organizeProject","params":{"scope":scope(),"beforeSha256":doc["binState"]["sha256"],"edit":{"type":"create","name":"Original owned metadata bin","parent":null}}}]})).unwrap();
    assert_eq!(commit["status"], "committed");
    f.s.execute("agent.readProject", json!({"token":token(161),"scope":scope(),"projectPath":f.s.path})).unwrap();
    let h = f.s.execute("agent.readHistory", json!({"token":token(161),"scope":scope(),"commit":token(160),"undo":null})).unwrap();
    assert_eq!(h["undoAvailable"], true);
    let items = f.items.clone();
    inputs(&mut f, 162, json!({"kind":"sources","items":items}));
    let before = state(&f.s);
    let published = f.s.agent_bridge.tickets.iter().find(|t| t.token == token(160)).unwrap();
    let metadata = f.s.agent_bridge.tickets.iter().find(|t| t.token == token(161)).unwrap();
    let qualified = f.s.agent_bridge.tickets.iter().find(|t| t.token == token(162)).unwrap();
    assert_eq!(qualified.project_scope, published.project_scope);
    assert_eq!(qualified.source_library.is_some(), published.source_library.is_some());
    assert!(qualified.multicam_production.is_some());
    assert!(published.multicam_production.is_none());
    assert!(super::super::history::owns_undo(&f.s, metadata, published));
    assert!(!super::super::history::owns_undo(&f.s, qualified, published));
    assert_eq!(state(&f.s), before);
    f.s.execute("agent.undo", json!({"token":token(161),"commit":token(160)})).unwrap();
    f.s.execute("agent.readProject", json!({"token":token(163),"scope":scope(),"projectPath":f.s.path})).unwrap();
    let h = f.s.execute("agent.readHistory", json!({"token":token(163),"scope":scope(),"commit":token(160),"undo":token(161)})).unwrap();
    assert_eq!(h["redoAvailable"], true);
    f.s.execute("agent.redo", json!({"token":token(163),"commit":token(160),"undo":token(161)})).unwrap();
}

fn retained(f: &Fixture, id: usize) -> &PreparedPlan {
    f.s.agent_bridge.tickets.iter().find(|t| t.token == token(id)).unwrap().multicam_production.as_ref().unwrap().prepared.as_deref().unwrap()
}
fn captured_valid(f: &Fixture, id: usize, raw: &[u8]) -> crate::Result<()> {
    let owner = f.s.agent_bridge.tickets.iter().find(|t| t.token == token(id)).unwrap().multicam_production.as_ref().unwrap();
    let p = owner.prepared.as_ref().unwrap();
    let historical = publication::Historical::decode(raw)?;
    historical.validate_inputs(&f.s, &owner.request, &owner.projection, &owner.dependencies)?;
    historical.validate_captured(
        &f.s.project,
        &p.candidate.project,
        &f.s.state,
        &p.candidate.state,
        p.summary["humanCommand"].as_str().unwrap(),
        &p.summary["humanParams"],
        &p.summary["result"],
    )
}
#[test]
fn computed_record_roundtrip_binds_actual_source_and_trimmed_linked_timeline_results_without_reanalysis_or_publication() {
    let _serial = super::super::snapshot_jobs::TEST_SERIAL.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let mut f = fixture();
    let items = f.items.clone();
    inputs(&mut f, 180, json!({"kind":"sources","items":items}));
    let before = state(&f.s);
    let doc = plan(&mut f, 180, json!({"gesture":"create_multicam","options":{"method":"audio","audio":"switch","processedBin":true}})).unwrap();
    let raw = retained(&f, 180).record.bytes().unwrap();
    captured_valid(&f, 180, &raw).unwrap();
    let historical = publication::Historical::decode(&raw).unwrap();
    assert_eq!(doc["computedRecord"], historical.metadata().unwrap());
    assert_eq!(doc["computedRecord"]["writeAuthority"], false);
    for _ in 0..3 {
        assert_eq!(f.s.execute(STATUS, json!({"token":token(180),"scope":scope()})).unwrap(), doc);
        assert_eq!(retained(&f, 180).record.bytes().unwrap(), raw);
    }
    assert_eq!(state(&f.s), before);
    release(&mut f, 180);
    let (seq, ids) = timeline(&mut f);
    inputs(&mut f, 181, json!({"kind":"timeline","sequence":seq,"identities":ids}));
    let before = state(&f.s);
    let doc = plan(&mut f, 181, json!({"gesture":"synchronize","options":{"method":"in","reference":ids[0],"offset":2}})).unwrap();
    let raw = retained(&f, 181).record.bytes().unwrap();
    captured_valid(&f, 181, &raw).unwrap();
    assert_eq!(doc["computedRecord"], publication::Historical::decode(&raw).unwrap().metadata().unwrap());
    assert_eq!(state(&f.s), before);
}
#[test]
fn computed_record_parser_refuses_truncation_unknown_fields_noncanonical_encoding_and_corruption_without_panicking() {
    let _serial = super::super::snapshot_jobs::TEST_SERIAL.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let mut f = fixture();
    let items = vec![f.items[0], f.items[2]];
    inputs(&mut f, 182, json!({"kind":"sources","items":items}));
    plan(&mut f, 182, json!({"gesture":"merge","options":{"method":"in"}})).unwrap();
    let raw = retained(&f, 182).record.bytes().unwrap();
    let before = state(&f.s);
    let mut bad = vec![Vec::new(), vec![b' '; 7 * 1024 * 1024 + 1]];
    for end in [1, 2, raw.len() / 3, raw.len() / 2, raw.len() - 1] {
        bad.push(raw[..end].to_vec());
    }
    let v: Value = serde_json::from_slice(&raw).unwrap();
    let mut extra = v.clone();
    extra["unexpected"] = json!(true);
    bad.push(project_audit::value_bytes(&extra).unwrap());
    let mut extra = v.clone();
    extra["payload"]["unexpected"] = json!(true);
    bad.push(project_audit::value_bytes(&extra).unwrap());
    let mut wrong = v.clone();
    wrong["checksum_sha256"] = json!("0".repeat(64));
    bad.push(project_audit::value_bytes(&wrong).unwrap());
    bad.push(serde_json::to_vec_pretty(&v).unwrap());
    let mut flip = raw.clone();
    flip[raw.len() / 2] ^= 1;
    bad.push(flip);
    for bytes in bad {
        let out = std::panic::catch_unwind(|| publication::Historical::decode(&bytes));
        assert!(out.is_ok());
        assert!(out.unwrap().is_err());
    }
    assert_eq!(state(&f.s), before);
}
#[test]
fn recomputed_checksums_do_not_replace_actual_captured_native_inputs_snapshots_editor_or_result() {
    let _serial = super::super::snapshot_jobs::TEST_SERIAL.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let mut f = fixture();
    let items = f.items.clone();
    inputs(&mut f, 183, json!({"kind":"sources","items":items}));
    plan(&mut f, 183, json!({"gesture":"create_multicam","options":{"method":"in"}})).unwrap();
    let raw = retained(&f, 183).record.bytes().unwrap();
    let v: Value = serde_json::from_slice(&raw).unwrap();
    let before = state(&f.s);
    let changes = [
        ("/payload/before_project_sha256", json!("0".repeat(64))),
        ("/payload/after_project_sha256", json!("0".repeat(64))),
        ("/payload/before_editor_sha256", json!("0".repeat(64))),
        ("/payload/after_editor_sha256", json!("0".repeat(64))),
        ("/payload/input_request_sha256", json!("0".repeat(64))),
        ("/payload/input_projection_sha256", json!("0".repeat(64))),
        ("/payload/native_epoch", json!("0".repeat(32))),
        ("/payload/read_token", json!(token(999))),
        ("/payload/revision", json!(99999)),
        ("/payload/project_path", json!("/other/project.fcproj")),
        ("/payload/human_command", json!("clip.mergeClips")),
        ("/payload/human_params", json!({})),
        ("/payload/result", json!({})),
        ("/payload/files/0/generation/sha256", json!("0".repeat(64))),
        ("/payload/files/0/binding_sha256", json!("0".repeat(64))),
        ("/payload/files/0/admission_sha256", json!("0".repeat(64))),
        ("/payload/files", json!([])),
        ("/payload/native_linked_selection", json!(false)),
    ];
    for (pointer, value) in changes {
        let mut changed = v.clone();
        *changed.pointer_mut(pointer).unwrap() = value;
        changed["checksum_sha256"] = json!(format!("{:x}", Sha256::digest(project_audit::value_bytes(&changed["payload"]).unwrap())));
        let bytes = publication::canonical_reseal_for_test(&changed).unwrap();
        assert!(captured_valid(&f, 183, &bytes).is_err(), "{pointer}");
    }
    assert_eq!(state(&f.s), before);
}
#[test]
fn historical_computed_data_cannot_replace_live_cow_identity_epoch_or_create_publication_authority() {
    let _serial = super::super::snapshot_jobs::TEST_SERIAL.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let mut f = fixture();
    let items = f.items.clone();
    inputs(&mut f, 184, json!({"kind":"sources","items":items}));
    plan(&mut f, 184, json!({"gesture":"create_multicam","options":{"method":"in"}})).unwrap();
    let raw = retained(&f, 184).record.bytes().unwrap();
    publication::Historical::decode(&raw).unwrap();
    let mut other = Session::new(f.s.services.clone());
    other.project = f.s.project.clone();
    other.state = f.s.state.clone();
    other.path = f.s.path.clone();
    other.revision = f.s.revision;
    other.saved_revision = f.s.saved_revision;
    assert!(retained(&f, 184).record.validate_live(&other, &retained(&f, 184).candidate).is_err());
    let old = f.s.project.clone();
    f.s.project = Arc::new((*old).clone());
    assert!(retained(&f, 184).record.validate_live(&f.s, &retained(&f, 184).candidate).is_err());
    assert!(f.s.execute(STATUS, json!({"token":token(184),"scope":scope()})).is_err());
    f.s.project = old;
    let before = state(&f.s);
    assert!(
        f.s.execute("agent.apply", json!({"token":token(184),"exact":true,"operations":[{"command":"clip.createMulticam","params":{"items":f.items}}]}))
            .is_err()
    );
    assert_eq!(state(&f.s), before);
    release(&mut f, 184);
    assert!(f.s.execute(STATUS, json!({"token":token(184),"scope":scope()})).is_err());
    publication::Historical::decode(&raw).unwrap();
    assert!(f.s.agent_bridge.tickets.iter().all(|t| t.token != token(184)));
}
#[test]
fn qualified_single_audio_merge_keeps_original_human_disabled_selection_rules_and_live_history() {
    let _serial = super::super::snapshot_jobs::TEST_SERIAL.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let mut f = fixture();
    f.s.execute("project.select", json!({"items":[]})).unwrap();
    let items = vec![f.items[2]];
    inputs(&mut f, 185, json!({"kind":"sources","items":items}));
    let before = state(&f.s);
    let error = plan(&mut f, 185, json!({"gesture":"merge","options":{"method":"in"}})).unwrap_err();
    assert!(error.to_string().contains("original human gesture is disabled"), "{error}");
    assert_eq!(state(&f.s), before);
    let status = f.s.execute(STATUS, json!({"token":token(185),"scope":scope()})).unwrap();
    assert_eq!(status["status"], "failed");
    assert!(status.get("computedRecord").is_none());
    assert!(plan(&mut f, 185, json!({"gesture":"merge","options":{"method":"in"}})).is_err());
    assert_eq!(state(&f.s), before);
}

fn original_origin(f: &Fixture, id: usize) -> (origin::Saved, Vec<Arc<filmcraft_project::Project>>) {
    let pool = vec![f.s.project.clone(), retained(f, id).candidate.project.clone()];
    (retained(f, id).origin.saved(&pool).unwrap(), pool)
}
#[test]
fn retained_origin_roundtrip_uses_original_native_project_codec_and_new_historical_cow_nodes_only() {
    let _serial = super::super::snapshot_jobs::TEST_SERIAL.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let mut f = fixture();
    let items = f.items.clone();
    inputs(&mut f, 190, json!({"kind":"sources","items":items}));
    let before = state(&f.s);
    let doc = plan(&mut f, 190, json!({"gesture":"create_multicam","options":{"method":"audio","audio":"all","processedBin":true}})).unwrap();
    let (saved, pool) = original_origin(&f, 190);
    let raw = saved.encoded().unwrap();
    let independent = pool
        .iter()
        .map(|p| {
            let bytes = super::super::history_checkpoint::project_bytes(p, 16 * 1024 * 1024).unwrap();
            Arc::new(super::super::history_checkpoint::project(std::str::from_utf8(&bytes).unwrap()).unwrap())
        })
        .collect::<Vec<_>>();
    assert!(pool.iter().zip(&independent).all(|(a, b)| !Arc::ptr_eq(a, b)));
    assert!(retained(&f, 190).origin.saved(&independent).is_err());
    let decoded = origin::Saved::decode(&raw).unwrap();
    let historical = decoded.restore(&independent, &serde_json::from_value(scope()).unwrap(), f.s.path.as_deref().unwrap(), &token(190), f.s.revision).unwrap();
    assert_eq!(historical.metadata().unwrap(), doc["retainedOrigin"]);
    assert_eq!(historical.saved(&independent).unwrap().encoded().unwrap(), raw);
    assert_eq!(historical.metadata().unwrap()["writeAuthority"], false);
    assert_eq!(state(&f.s), before);
    release(&mut f, 190);
    let (seq, ids) = timeline(&mut f);
    inputs(&mut f, 191, json!({"kind":"timeline","sequence":seq,"identities":ids}));
    let before = state(&f.s);
    plan(&mut f, 191, json!({"gesture":"synchronize","options":{"method":"marker","marker":"Clap"}})).unwrap();
    let (saved, pool) = original_origin(&f, 191);
    let historical = origin::Saved::decode(&saved.encoded().unwrap())
        .unwrap()
        .restore(&pool, &serde_json::from_value(scope()).unwrap(), f.s.path.as_deref().unwrap(), &token(191), f.s.revision)
        .unwrap();
    assert_eq!(historical.metadata().unwrap(), retained(&f, 191).origin.metadata().unwrap());
    assert_eq!(state(&f.s), before);
}
#[test]
fn retained_origin_missing_crossed_graph_edges_input_context_editor_and_receipt_identity_are_refused() {
    let _serial = super::super::snapshot_jobs::TEST_SERIAL.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let mut f = fixture();
    let items = f.items.clone();
    inputs(&mut f, 192, json!({"kind":"sources","items":items}));
    plan(&mut f, 192, json!({"gesture":"create_multicam","options":{"method":"in"}})).unwrap();
    let (saved, pool) = original_origin(&f, 192);
    let raw = saved.encoded().unwrap();
    let before = state(&f.s);
    let root = serde_json::from_value(scope()).unwrap();
    let path = f.s.path.as_deref().unwrap();
    assert!(retained(&f, 192).origin.saved(&pool[..1]).is_err());
    assert!(saved.restore(&pool[..1], &root, path, &token(192), f.s.revision).is_err());
    let mut crossed = pool.clone();
    crossed.reverse();
    assert!(saved.restore(&crossed, &root, path, &token(192), f.s.revision).is_err());
    assert!(saved.restore(&pool, &root, "/different.fcproj", &token(192), f.s.revision).is_err());
    assert!(saved.restore(&pool, &root, path, &token(999), f.s.revision).is_err());
    assert!(saved.restore(&pool, &root, path, &token(192), f.s.revision + 1).is_err());
    let other = serde_json::from_value(json!({"projectId":"p_other","workspaceId":WORKSPACE})).unwrap();
    assert!(saved.restore(&pool, &other, path, &token(192), f.s.revision).is_err());
    let v: Value = serde_json::from_slice(&raw).unwrap();
    for (pointer, value) in [
        ("/version", json!(2)),
        ("/before", json!(u64::MAX)),
        ("/after", json!(0)),
        ("/evidence/version", json!(2)),
        ("/evidence/kind", json!("foreign")),
        ("/evidence/dependencies", json!([])),
        ("/evidence/projection", json!({})),
        ("/evidence/request/token", json!(token(999))),
        ("/evidence/before_editor/linked_selection", json!(false)),
        ("/evidence/after_editor/project_selection", json!([])),
        ("/evidence/record", json!("{}")),
    ] {
        let mut changed = v.clone();
        *changed.pointer_mut(pointer).unwrap() = value;
        let changed: origin::Saved = serde_json::from_value(changed).unwrap();
        let bytes = changed.encoded().unwrap();
        let check = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            origin::Saved::decode(&bytes).and_then(|s| s.restore(&pool, &root, path, &token(192), f.s.revision))
        }));
        assert!(check.is_ok());
        assert!(check.unwrap().is_err(), "{pointer}");
    }
    let mut extra = v.clone();
    extra["unexpected"] = json!(true);
    assert!(origin::Saved::decode(&project_audit::value_bytes(&extra).unwrap()).is_err());
    for cut in [0, 1, raw.len() / 2, raw.len() - 1] {
        assert!(origin::Saved::decode(&raw[..cut]).is_err());
    }
    assert_eq!(state(&f.s), before);
}
#[test]
fn restored_source_origin_is_historical_data_when_physical_sources_change_and_cannot_refresh_input_qualification() {
    let _serial = super::super::snapshot_jobs::TEST_SERIAL.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let mut f = fixture();
    let items = vec![f.items[0], f.items[2]];
    inputs(&mut f, 193, json!({"kind":"sources","items":items}));
    plan(&mut f, 193, json!({"gesture":"merge","options":{"method":"in"}})).unwrap();
    let (saved, pool) = original_origin(&f, 193);
    let before = state(&f.s);
    {
        use std::io::Write;
        let mut file = fs::OpenOptions::new().append(true).open(&f.files[0]).unwrap();
        file.write_all(b"changed generated source after native capture").unwrap();
        file.sync_all().unwrap();
    }
    let historical = saved.restore(&pool, &serde_json::from_value(scope()).unwrap(), f.s.path.as_deref().unwrap(), &token(193), f.s.revision).unwrap();
    assert_eq!(historical.metadata().unwrap()["sourceValidation"], "not-performed");
    assert!(f.s.execute(STATUS, json!({"token":token(193),"scope":scope()})).is_err());
    assert!(plan(&mut f, 193, json!({"gesture":"merge","options":{"method":"in"}})).is_err());
    assert_eq!(state(&f.s), before);
}
#[test]
fn original_complete_command_journal_preserves_source_origin_as_passive_data_without_replaying_or_creating_tickets() {
    let _serial = super::super::snapshot_jobs::TEST_SERIAL.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let mut f = fixture();
    let items = f.items.clone();
    inputs(&mut f, 194, json!({"kind":"sources","items":items}));
    plan(&mut f, 194, json!({"gesture":"create_multicam","options":{"method":"audio"}})).unwrap();
    let (saved, _pool) = original_origin(&f, 194);
    let before = state(&f.s);
    let mut source = Session::default();
    source.journal = vec![("native.multicamComputed".into(), json!({"origin":String::from_utf8(saved.encoded().unwrap()).unwrap()}))];
    let snapshot = super::super::native_editor::Snapshot::capture_complete(&source).unwrap();
    let bytes = super::super::history_checkpoint::bytes(&snapshot, 80 * 1024 * 1024).unwrap();
    let decoded: super::super::native_editor::Snapshot = serde_json::from_slice(&bytes).unwrap();
    let mut fresh = Session::default();
    let project = fresh.project.clone();
    let revision = fresh.revision;
    decoded.install_cold(&mut fresh).unwrap();
    assert_eq!(fresh.journal, source.journal);
    assert!(Arc::ptr_eq(&project, &fresh.project));
    assert_eq!(fresh.revision, revision);
    assert!(fresh.agent_bridge.tickets.is_empty());
    assert!(fresh.history.undo.is_empty());
    assert!(fresh.history.redo.is_empty());
    assert!(!fresh.worker_available);
    assert!(crate::commands::find("native.multicamComputed").is_none());
    assert_eq!(state(&f.s), before);
}

fn worker_command(f: &mut Fixture, command: &str, params: Value) -> crate::Result<Value> {
    let mut pending = f.worker.start(&mut f.s, command, params, None)?;
    for _ in 0..5000 {
        if let Some(c) = pending.try_finish() {
            f.s = c.session;
            assert!(!c.reply.uncertain);
            return c.reply.result;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    panic!("bounded original native worker did not finish")
}
fn commit_request(id: usize, doc: &Value) -> Value {
    json!({"token":token(id),"label":"Original computed source gesture","exact":true,"validateAtPublication":true,"operations":[{"command":COMMIT,"params":{"scope":scope(),"recordSha256":doc["computedRecord"]["sha256"]}}]})
}
fn metadata(f: &mut Fixture, id: usize) {
    f.s.execute("agent.readProject", json!({"token":token(id),"scope":scope(),"projectPath":f.s.path})).unwrap();
}
fn ready_source(f: &mut Fixture, id: usize) -> Value {
    let items = f.items.clone();
    inputs(f, id, json!({"kind":"sources","items":items}));
    plan(f, id, json!({"gesture":"create_multicam","options":{"method":"in","processedBin":true}})).unwrap()
}
fn checkpoint(id: usize) -> String {
    format!("51364925-203f-4c22-9d7c-{id:012}")
}
fn saved_source(f: &mut Fixture, id: usize) -> (Value, PathBuf) {
    let checkpoint = checkpoint(id);
    f.s.execute("file.save", json!({})).unwrap();
    metadata(f, id);
    let saved = f.s.execute("agent.saveHistoryCheckpoint", json!({"token":token(id),"scope":scope(),"workspace":WORKSPACE,"checkpoint":checkpoint})).unwrap();
    (saved, f._dir.join(format!(".poema-history-{WORKSPACE}-{checkpoint}.json")))
}
fn reopen_source(f: &mut Fixture, id: usize, saved: &Value) {
    let mut fresh = Session::default();
    fresh.execute("file.open", json!({"path":f.s.path})).unwrap();
    f.worker.install(&mut fresh).unwrap();
    fresh.agent_operation_control().enable_transport();
    f.s = fresh;
    metadata(f, id);
    worker_command(
        f,
        "agent.restoreHistoryCheckpoint",
        json!({"token":token(id),"scope":scope(),"workspace":WORKSPACE,"checkpoint":saved["checkpoint"]["checkpoint"],"sha256":saved["checkpoint"]["sha256"]}),
    )
    .unwrap();
}
#[test]
fn owned_source_publication_all_five_methods_use_exact_original_human_results_one_history_step_and_passive_journal() {
    let _serial = super::super::snapshot_jobs::TEST_SERIAL.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    for gesture in ["merge", "create_multicam", "synchronize"] {
        for method in ["in", "out", "timecode", "marker", "audio"] {
            let mut f = fixture();
            let targets = if gesture == "synchronize" {
                let (seq, ids) = timeline(&mut f);
                json!({"kind":"timeline","sequence":seq,"identities":ids})
            } else {
                json!({"kind":"sources","items":if gesture=="merge"{vec![f.items[0],f.items[2]]}else{f.items.clone()}})
            };
            inputs(&mut f, 210, targets);
            let doc = plan(&mut f, 210, json!({"gesture":gesture,"options":{"method":method,"marker":"Clap","ignoreHours":true,"offset":2}})).unwrap();
            let mut human = Session::new(f.s.services.clone());
            human.project = f.s.project.clone();
            human.state = f.s.state.clone();
            human.media = f.s.media.clone();
            human.prefs = f.s.prefs.clone();
            human.path = f.s.path.clone();
            human.revision = f.s.revision;
            human.saved_revision = f.s.saved_revision;
            let result = human.execute(doc["humanCommand"].as_str().unwrap(), doc["humanParams"].clone()).unwrap();
            let rev = f.s.revision;
            let steps = f.s.history.undo.len();
            let request = commit_request(210, &doc);
            let receipt = worker_command(&mut f, "agent.apply", request.clone()).unwrap();
            assert_eq!(receipt["action"], "multicam_production");
            assert_eq!(receipt["results"], json!([result]));
            assert_eq!(f.s.revision, rev + 1);
            assert_eq!(f.s.history.undo.len(), steps + 1);
            assert_eq!(project_audit::bytes(&f.s.project).unwrap(), project_audit::bytes(&human.project).unwrap());
            assert_eq!(context(&f.s).unwrap(), context(&human).unwrap());
            assert_eq!(f.s.journal.last().unwrap().0, "native.multicamComputed");
            assert!(crate::commands::find("native.multicamComputed").is_none());
            let t = f.s.agent_bridge.tickets.iter().find(|t| t.token == token(210)).unwrap();
            assert!(t.multicam_history.is_some());
            assert!(t.multicam_production.is_none());
            let before = state(&f.s);
            assert_eq!(worker_command(&mut f, "agent.apply", request).unwrap(), receipt);
            assert_eq!(state(&f.s), before);
            eprintln!("owned source publication {gesture}/{method}: exact original result, one native history step, passive journal");
        }
    }
}
#[test]
fn owned_source_undo_redo_use_fresh_qualified_reads_and_receipts_survive_later_human_work() {
    let _serial = super::super::snapshot_jobs::TEST_SERIAL.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let mut f = fixture();
    let doc = ready_source(&mut f, 220);
    let original = f.s.project.clone();
    let req = commit_request(220, &doc);
    let receipt = worker_command(&mut f, "agent.apply", req.clone()).unwrap();
    let committed = f.s.project.clone();
    let items = f.items.clone();
    inputs(&mut f, 221, json!({"kind":"sources","items":items}));
    assert_eq!(f.s.execute("agent.readHistory", json!({"token":token(221),"scope":scope(),"commit":token(220),"undo":null})).unwrap()["undoAvailable"], true);
    worker_command(&mut f, "agent.undo", json!({"token":token(221),"commit":token(220)})).unwrap();
    assert!(Arc::ptr_eq(&original, &f.s.project));
    inputs(&mut f, 222, json!({"kind":"sources","items":items}));
    assert_eq!(
        f.s.execute("agent.readHistory", json!({"token":token(222),"scope":scope(),"commit":token(220),"undo":token(221)})).unwrap()["redoAvailable"],
        true
    );
    worker_command(&mut f, "agent.redo", json!({"token":token(222),"commit":token(220),"undo":token(221)})).unwrap();
    assert!(Arc::ptr_eq(&committed, &f.s.project));
    f.s.execute("file.newBin", json!({"name":"Later human work"})).unwrap();
    let later = state(&f.s);
    assert_eq!(worker_command(&mut f, "agent.apply", req).unwrap(), receipt);
    assert_eq!(f.s.execute("agent.receipt", json!({"token":token(220)})).unwrap()["result"], receipt);
    assert_eq!(state(&f.s), later);
    inputs(&mut f, 223, json!({"kind":"sources","items":items}));
    assert_eq!(f.s.execute("agent.readHistory", json!({"token":token(223),"scope":scope(),"commit":token(220),"undo":null})).unwrap()["undoAvailable"], false);
    assert!(worker_command(&mut f, "agent.undo", json!({"token":token(223),"commit":token(220)})).is_err());
    assert_eq!(state(&f.s), later);
}
#[test]
fn source_capsule_eleven_reopens_owned_graph_as_historical_data_and_requires_fresh_source_qualification() {
    let _serial = super::super::snapshot_jobs::TEST_SERIAL.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let mut f = fixture();
    let doc = ready_source(&mut f, 230);
    worker_command(&mut f, "agent.apply", commit_request(230, &doc)).unwrap();
    let expected = project_audit::bytes(&f.s.project).unwrap();
    let (saved, path) = saved_source(&mut f, 231);
    let capsule: Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
    assert_eq!(capsule["version"], 11);
    reopen_source(&mut f, 232, &saved);
    assert_eq!(project_audit::bytes(&f.s.project).unwrap(), expected);
    let t = f.s.agent_bridge.tickets.iter().find(|t| t.token == token(230)).unwrap();
    assert!(t.multicam_history.is_some());
    assert!(t.multicam_production.is_none());
    assert_eq!(t.media.scope_json()["libraryVerified"], false);
    let items = f.items.clone();
    inputs(&mut f, 233, json!({"kind":"sources","items":items}));
    assert_eq!(f.s.execute("agent.readHistory", json!({"token":token(233),"scope":scope(),"commit":token(230),"undo":null})).unwrap()["undoAvailable"], true);
    worker_command(&mut f, "agent.undo", json!({"token":token(233),"commit":token(230)})).unwrap();
    inputs(&mut f, 234, json!({"kind":"sources","items":items}));
    worker_command(&mut f, "agent.redo", json!({"token":token(234),"commit":token(230),"undo":token(233)})).unwrap();
    assert_eq!(project_audit::bytes(&f.s.project).unwrap(), expected);
    let (saved, _) = saved_source(&mut f, 235);
    reopen_source(&mut f, 236, &saved);
    inputs(&mut f, 237, json!({"kind":"sources","items":items}));
    worker_command(&mut f, "agent.undo", json!({"token":token(237),"commit":token(230)})).unwrap();
}
#[test]
fn source_capsule_role_rejects_versions_one_through_ten_missing_origins_and_crossed_graphs_even_with_new_checksums() {
    let _serial = super::super::snapshot_jobs::TEST_SERIAL.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let mut f = fixture();
    let doc = ready_source(&mut f, 240);
    worker_command(&mut f, "agent.apply", commit_request(240, &doc)).unwrap();
    let (saved, path) = saved_source(&mut f, 241);
    let raw: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    let index = raw["tickets"].as_array().unwrap().iter().position(|t| t["token"] == token(240)).unwrap();
    let mut hostile = Vec::new();
    for version in 1..=10 {
        let mut v = raw.clone();
        v["version"] = json!(version);
        hostile.push(v);
    }
    for mode in ["missing", "null", "crossed", "source-library", "foreign-hash", "origin-input"] {
        let mut v = raw.clone();
        match mode {
            "missing" => {
                v["tickets"][index].as_object_mut().unwrap().remove("multicam_history");
            }
            "null" => v["tickets"][index]["multicam_history"] = Value::Null,
            "crossed" => v["tickets"][index]["multicam_history"]["before"] = v["tickets"][index]["after"].clone(),
            "source-library" => v["tickets"][index]["source_library"] = json!(f.items),
            "foreign-hash" => v["tickets"][index]["request"]["operations"][0]["params"]["recordSha256"] = json!("0".repeat(64)),
            "origin-input" => v["tickets"][index]["multicam_history"]["evidence"]["projection"] = json!({}),
            _ => unreachable!(),
        };
        hostile.push(v);
    }
    for v in hostile {
        let bytes = serde_json::to_vec(&v).unwrap();
        fs::write(&path, &bytes).unwrap();
        let mut fresh = Session::default();
        fresh.execute("file.open", json!({"path":f.s.path})).unwrap();
        fresh.execute("agent.readProject", json!({"token":token(242),"scope":scope(),"projectPath":fresh.path})).unwrap();
        let before = state(&fresh);
        assert!(
            fresh
                .execute(
                    "agent.restoreHistoryCheckpoint",
                    json!({"token":token(242),"scope":scope(),"workspace":WORKSPACE,"checkpoint":saved["checkpoint"]["checkpoint"],"sha256":format!("{:x}",Sha256::digest(&bytes))})
                )
                .is_err()
        );
        assert_eq!(state(&fresh), before);
    }
}
#[test]
fn source_publication_stop_changed_bytes_wrong_record_and_missing_worker_never_publish_or_recompute() {
    let _serial = super::super::snapshot_jobs::TEST_SERIAL.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    for mode in ["stop", "bytes", "hash", "worker"] {
        let mut f = fixture();
        let doc = ready_source(&mut f, 250);
        let mut req = commit_request(250, &doc);
        let before = state(&f.s);
        match mode {
            "stop" => {
                assert_eq!(f.s.agent_operation_control().cancel(&json!({"token":token(250)})).unwrap()["cancellationAccepted"], true);
            }
            "bytes" => {
                use std::io::Write;
                fs::OpenOptions::new().append(true).open(&f.files[0]).unwrap().write_all(b"changed generated source").unwrap();
            }
            "hash" => req["operations"][0]["params"]["recordSha256"] = json!("0".repeat(64)),
            "worker" => f.s.worker_available = false,
            _ => unreachable!(),
        }
        assert!(worker_command(&mut f, "agent.apply", req).is_err(), "{mode}");
        assert_eq!(state(&f.s), before, "{mode}");
        assert!(f.s.agent_bridge.tickets.iter().find(|t| t.token == token(250)).unwrap().committed.is_none());
    }
}
#[test]
fn synchronized_native_noop_does_not_invent_an_owned_undo_step() {
    let _serial = super::super::snapshot_jobs::TEST_SERIAL.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let mut f = fixture();
    let (seq, ids) = timeline(&mut f);
    let clips = f.s.project.sequence(ItemId(seq)).unwrap().video_tracks.iter().map(|t| t.items[0].id.0).collect::<Vec<_>>();
    f.s.execute("clip.synchronize", json!({"clips":clips,"method":"in"})).unwrap();
    inputs(&mut f, 260, json!({"kind":"timeline","sequence":seq,"identities":ids}));
    let doc = plan(&mut f, 260, json!({"gesture":"synchronize","options":{"method":"in"}})).unwrap();
    assert_eq!(doc["result"]["moved"], 0);
    let before = state(&f.s);
    let receipt = worker_command(&mut f, "agent.apply", commit_request(260, &doc)).unwrap();
    assert_eq!(receipt["status"], "committed");
    assert_eq!(receipt["outcome"], "unchanged");
    assert_eq!(receipt["timelineCommits"], 0);
    assert_eq!(receipt["undoOwned"], false);
    assert_eq!(state(&f.s), before);
}

#[test]
fn fully_resealed_historical_source_records_cannot_hide_unrelated_global_tree_or_existing_sequence_changes() {
    let _serial = super::super::snapshot_jobs::TEST_SERIAL.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let mut f = fixture();
    let (seq, _) = timeline(&mut f);
    let doc = ready_source(&mut f, 270);
    let (saved, pool) = original_origin(&f, 270);
    let original: Value = serde_json::from_slice(&saved.encoded().unwrap()).unwrap();
    let before = state(&f.s);
    for mode in ["project-name", "root-name", "existing-sequence"] {
        let mut forged = (*pool[0]).clone();
        match mode {
            "project-name" => forged.name = "Unauthored historical title".into(),
            "root-name" => forged.root.name = "Unauthored historical root".into(),
            "existing-sequence" => forged.item_mut(ItemId(seq)).unwrap().name = "Unauthored historical sequence".into(),
            _ => unreachable!(),
        }
        let mut v = original.clone();
        let mut record: Value = serde_json::from_str(v["evidence"]["record"].as_str().unwrap()).unwrap();
        record["payload"]["before_project_sha256"] = json!(project_plane::project_basis(&forged).unwrap());
        let raw = publication::canonical_reseal_for_test(&record).unwrap();
        publication::Historical::decode(&raw).unwrap();
        v["evidence"]["record"] = json!(String::from_utf8(raw).unwrap());
        let forged_saved: origin::Saved = serde_json::from_value(v).unwrap();
        let encoded = forged_saved.encoded().unwrap();
        let canonical = origin::Saved::decode(&encoded).unwrap();
        let forged_pool = vec![Arc::new(forged), pool[1].clone()];
        assert!(
            canonical.restore(&forged_pool, &serde_json::from_value(scope()).unwrap(), f.s.path.as_deref().unwrap(), &token(270), f.s.revision).is_err(),
            "{mode}"
        );
    }
    assert_eq!(state(&f.s), before);
    assert_eq!(doc["writeAuthority"], false);
}
#[test]
fn derived_source_sequences_keep_independent_root_and_only_a_real_human_main_sequence_creates_the_owning_anchor() {
    let _serial = super::super::snapshot_jobs::TEST_SERIAL.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let mut f = fixture();
    let doc = ready_source(&mut f, 280);
    let receipt = worker_command(&mut f, "agent.apply", commit_request(280, &doc)).unwrap();
    let source = receipt["results"][0]["sequence"].as_u64().unwrap();
    assert!(crate::editorial_workspace::association(&f.s.project).unwrap().is_none());
    assert!(crate::project_workspace::ordinary_sequences(&f.s.project).next().is_none());
    metadata(&mut f, 281);
    assert!(!f.s.project.item(ItemId(source)).unwrap().metadata.contains_key(crate::editorial_workspace::KEY));
    let main =
        f.s.execute("file.newSequence", json!({"name":"Actual human owning sequence","width":64,"height":48,"fps":{"num":24,"den":1},"video":1,"audio":1}))
            .unwrap()["sequence"]
            .as_u64()
            .unwrap();
    let main = if main == 0 { f.s.state.active_sequence.unwrap().0 } else { main };
    let association = crate::editorial_workspace::association(&f.s.project).unwrap().unwrap();
    assert_eq!(association.scope.anchor_sequence, main);
    assert_ne!(main, source);
    assert_eq!(association.scope.project_id, "p_production");
    assert_eq!(association.workspace_id, WORKSPACE);
    assert!(!f.s.project.item(ItemId(source)).unwrap().metadata.contains_key(crate::editorial_workspace::KEY));
}

fn overlapping_timeline(f: &mut Fixture) -> (u64, Vec<String>) {
    let (seq, ids) = timeline(f);
    let item = f.items[0];
    let rate = FrameRate::FPS_24;
    f.s.edit("Actual unselected linked overlap and retained tail", |p, _| {
        for (at, duration, track) in [(0, 36, 1), (80, 10, 0)] {
            let link = p.alloc_id();
            let range = TimeRange::new(Tick::ZERO, rate.tick_of(duration));
            let mut v = p.make_track_item(ItemId(item), TrackKind::Video, rate.tick_of(at), range, rate).unwrap();
            let mut a = p.make_track_item(ItemId(item), TrackKind::Audio, rate.tick_of(at), range, rate).unwrap();
            v.link = Some(link);
            a.link = Some(link);
            let q = p.sequence_mut(ItemId(seq)).unwrap();
            q.video_tracks[track].items.push(v);
            q.audio_tracks[track].items.push(a);
            q.video_tracks[track].sort();
            q.audio_tracks[track].sort();
        }
        Ok(())
    })
    .unwrap();
    f.s.execute("file.save", json!({})).unwrap();
    (seq, ids)
}
fn forged_origin_with_project(
    f: &Fixture,
    id: usize,
    before: &filmcraft_project::Project,
    after: &filmcraft_project::Project,
    after_editor: &crate::EditorState,
    change: impl FnOnce(&mut Value),
) -> origin::Saved {
    let (saved, _) = original_origin(f, id);
    let mut v: Value = serde_json::from_slice(&saved.encoded().unwrap()).unwrap();
    let mut record: Value = serde_json::from_str(v["evidence"]["record"].as_str().unwrap()).unwrap();
    record["payload"]["before_project_sha256"] = json!(project_plane::project_basis(before).unwrap());
    record["payload"]["after_project_sha256"] = json!(project_plane::project_basis(after).unwrap());
    record["payload"]["after_editor_sha256"] = json!(format!("{:x}", Sha256::digest(project_audit::value_bytes(after_editor).unwrap())));
    change(&mut record);
    let raw = publication::canonical_reseal_for_test(&record).unwrap();
    publication::Historical::decode(&raw).unwrap();
    v["evidence"]["record"] = json!(String::from_utf8(raw).unwrap());
    v["evidence"]["after_editor"] = serde_json::to_value(after_editor).unwrap();
    let saved: origin::Saved = serde_json::from_value(v).unwrap();
    origin::Saved::decode(&saved.encoded().unwrap()).unwrap()
}
#[test]
fn exact_native_geometry_roundtrips_actual_linked_overwrite_splits_and_retained_tail_for_all_five_methods_without_sources() {
    let _serial = super::super::snapshot_jobs::TEST_SERIAL.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    for method in ["in", "out", "timecode", "marker", "audio"] {
        let mut f = fixture();
        let (seq, ids) = overlapping_timeline(&mut f);
        inputs(&mut f, 300, json!({"kind":"timeline","sequence":seq,"identities":ids}));
        let before = state(&f.s);
        let doc = plan(&mut f, 300, json!({"gesture":"synchronize","options":{"method":method,"marker":"Clap","offset":2}})).unwrap();
        let (saved, pool) = original_origin(&f, 300);
        let historical = saved.restore(&pool, &serde_json::from_value(scope()).unwrap(), f.s.path.as_deref().unwrap(), &token(300), f.s.revision).unwrap();
        assert_eq!(historical.metadata().unwrap(), doc["retainedOrigin"]);
        let q = pool[1].sequence(ItemId(seq)).unwrap();
        assert!(q.all_tracks().flat_map(|t| &t.items).any(|c| c.start == FrameRate::FPS_24.tick_of(80)));
        assert!(q.video_tracks[1].items.iter().any(|c| c.item == ItemId(f.items[0]) && c.duration < FrameRate::FPS_24.tick_of(36)));
        if method == "in" {
            assert!(q.all_tracks().flat_map(|t| &t.items).filter_map(|c| c.identity.as_ref()).any(|i| i.parent.is_some()));
        }
        // The historical path must use the stored observations and original metadata,
        // even after actual generated footage bytes have changed.
        {
            use std::io::Write;
            fs::OpenOptions::new().append(true).open(&f.files[0]).unwrap().write_all(b"changed generated footage after native calculation").unwrap();
        }
        saved.restore(&pool, &serde_json::from_value(scope()).unwrap(), f.s.path.as_deref().unwrap(), &token(300), f.s.revision).unwrap();
        assert!(f.s.execute(STATUS, json!({"token":token(300),"scope":scope()})).is_err());
        assert_eq!(state(&f.s), before);
        eprintln!("exact computed geometry {method}: linked overwrite, native lineage, retained tail, no new source observations");
    }
}
#[test]
fn canonical_resealed_timeline_snapshots_cannot_change_unselected_clips_trims_links_lineage_effects_or_editor_state() {
    let _serial = super::super::snapshot_jobs::TEST_SERIAL.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let mut f = fixture();
    let (seq, ids) = overlapping_timeline(&mut f);
    inputs(&mut f, 301, json!({"kind":"timeline","sequence":seq,"identities":ids}));
    plan(&mut f, 301, json!({"gesture":"synchronize","options":{"method":"in"}})).unwrap();
    let (_, pool) = original_origin(&f, 301);
    let editor = retained(&f, 301).candidate.state.clone();
    let before = state(&f.s);
    let rate = FrameRate::FPS_24;
    for mode in [
        "unselected-position",
        "unselected-before-position",
        "split-source-in",
        "selected-source-in",
        "split-link",
        "split-parent",
        "split-notes",
        "effect-id",
        "editor-selection",
        "report-start",
    ] {
        let mut original = (*pool[0]).clone();
        let mut after = (*pool[1]).clone();
        let mut post = editor.clone();
        let q = after.sequence_mut(ItemId(seq)).unwrap();
        match mode {
            "unselected-position" => {
                let c = q.video_tracks[0].items.iter_mut().find(|c| c.start == rate.tick_of(80)).unwrap();
                c.start += rate.frame_duration();
            }
            "unselected-before-position" => {
                let c = original.sequence_mut(ItemId(seq)).unwrap().video_tracks[0].items.iter_mut().find(|c| c.start == rate.tick_of(80)).unwrap();
                c.start += rate.frame_duration();
            }
            "split-source-in" => {
                let c = q.video_tracks[1].items.iter_mut().find(|c| c.identity.as_ref().is_some_and(|i| i.parent.is_some())).unwrap();
                c.source_in += rate.frame_duration();
            }
            "selected-source-in" => {
                let c = q.video_tracks[1].items.iter_mut().find(|c| c.start == Tick::ZERO).unwrap();
                c.source_in += rate.frame_duration();
            }
            "split-link" => {
                let c = q.video_tracks[1].items.iter_mut().find(|c| c.identity.as_ref().is_some_and(|i| i.parent.is_some())).unwrap();
                c.link = Some(999999);
            }
            "split-parent" => {
                let c = q.video_tracks[1].items.iter_mut().find(|c| c.identity.as_ref().is_some_and(|i| i.parent.is_some())).unwrap();
                c.identity.as_mut().unwrap().parent = Some("0".repeat(32));
            }
            "split-notes" => {
                let c = q.video_tracks[1].items.iter_mut().find(|c| c.identity.as_ref().is_some_and(|i| i.parent.is_some())).unwrap();
                c.identity.as_mut().unwrap().annotations.insert("forged".into(), "not inherited".into());
            }
            "effect-id" => {
                let c = q.video_tracks[1].items.iter_mut().find(|c| c.identity.as_ref().is_some_and(|i| i.parent.is_some())).unwrap();
                c.effects[0].instance_id = Some("0".repeat(32));
            }
            "editor-selection" => post.selection.clear(),
            "report-start" => {}
            _ => unreachable!(),
        }
        let saved = forged_origin_with_project(&f, 301, &original, &after, &post, |record| {
            if mode == "report-start" {
                record["payload"]["result"]["clips"][1]["start"] = json!(rate.tick_of(4).0);
            }
        });
        let forged_pool = vec![Arc::new(original), Arc::new(after)];
        assert!(
            saved.restore(&forged_pool, &serde_json::from_value(scope()).unwrap(), f.s.path.as_deref().unwrap(), &token(301), f.s.revision).is_err(),
            "{mode}"
        );
    }
    assert_eq!(state(&f.s), before);
}
#[test]
fn canonical_resealed_created_sources_cannot_change_components_camera_state_audio_or_native_effects() {
    let _serial = super::super::snapshot_jobs::TEST_SERIAL.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    for gesture in ["merge", "create_multicam"] {
        let mut f = fixture();
        let items = if gesture == "merge" { vec![f.items[0], f.items[2]] } else { f.items.clone() };
        inputs(&mut f, 302, json!({"kind":"sources","items":items}));
        let doc = plan(&mut f, 302, json!({"gesture":gesture,"options":{"method":"audio"}})).unwrap();
        let (_, pool) = original_origin(&f, 302);
        let editor = retained(&f, 302).candidate.state.clone();
        let created = doc["result"][if gesture == "merge" { "item" } else { "sequence" }].as_u64().unwrap();
        for mode in ["source-in", "duration", "link", "effect", "master-volume"] {
            let mut after = (*pool[1]).clone();
            let q = after.sequence_mut(ItemId(created)).unwrap();
            match mode {
                "source-in" => q.video_tracks[0].items[0].source_in += FrameRate::FPS_24.frame_duration(),
                "duration" => q.video_tracks[0].items[0].duration -= FrameRate::FPS_24.frame_duration(),
                "link" => q.video_tracks[0].items[0].link = Some(999999),
                "effect" => {
                    q.video_tracks[0].items[0].effects[0].params.clear();
                }
                "master-volume" => q.master_volume_db = -12.0,
                _ => unreachable!(),
            }
            let saved = forged_origin_with_project(&f, 302, &pool[0], &after, &editor, |_| {});
            assert!(
                saved
                    .restore(
                        &[pool[0].clone(), Arc::new(after)],
                        &serde_json::from_value(scope()).unwrap(),
                        f.s.path.as_deref().unwrap(),
                        &token(302),
                        f.s.revision
                    )
                    .is_err(),
                "{gesture}/{mode}"
            );
        }
    }
}
#[test]
fn malformed_recorded_observations_and_hostile_native_times_return_errors_without_audio_or_panics() {
    let _serial = super::super::snapshot_jobs::TEST_SERIAL.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let mut f = fixture();
    let (seq, ids) = overlapping_timeline(&mut f);
    inputs(&mut f, 303, json!({"kind":"timeline","sequence":seq,"identities":ids}));
    let doc = plan(&mut f, 303, json!({"gesture":"synchronize","options":{"method":"audio"}})).unwrap();
    let (_, pool) = original_origin(&f, 303);
    let editor = retained(&f, 303).candidate.state.clone();
    for mode in ["lag-max", "anchor-min", "start-max", "unknown-row", "wrong-item", "unknown-audio"] {
        let saved = forged_origin_with_project(&f, 303, &pool[0], &pool[1], &editor, |record| {
            let row = &mut record["payload"]["result"]["clips"][1];
            match mode {
                "lag-max" => row["audio"]["lagSamples"] = json!(i64::MAX),
                "anchor-min" => row["anchor"] = json!(i64::MIN),
                "start-max" => row["start"] = json!(i64::MAX),
                "unknown-row" => row["unexpected"] = json!(true),
                "wrong-item" => row["item"] = json!(999999),
                "unknown-audio" => row["audio"]["unexpected"] = json!(true),
                _ => unreachable!(),
            }
        });
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            saved.restore(&pool, &serde_json::from_value(scope()).unwrap(), f.s.path.as_deref().unwrap(), &token(303), f.s.revision)
        }));
        assert!(result.is_ok(), "{mode}");
        assert!(result.unwrap().is_err(), "{mode}");
    }
    let mut hostile = (*pool[0]).clone();
    hostile.sequence_mut(ItemId(seq)).unwrap().video_tracks[0].items[0].start = Tick(i64::MAX);
    let checked = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        crate::multicam::production::validate_computed_transition(
            &hostile,
            &pool[1],
            &f.s.state,
            &editor,
            doc["humanCommand"].as_str().unwrap(),
            &doc["humanParams"],
            &doc["result"],
        )
    }));
    assert!(checked.is_ok());
    assert!(checked.unwrap().is_err());
}

#[path = "tests/mixed.rs"]
mod mixed;

#[path = "tests/unchanged.rs"]
mod unchanged;

#[path = "tests/views.rs"]
mod views;
