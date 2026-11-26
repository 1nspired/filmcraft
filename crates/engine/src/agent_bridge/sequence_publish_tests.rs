use super::{
    assembly_tests::fixture,
    sequence_payload,
    tests::{read, token},
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
static NEXT: AtomicUsize = AtomicUsize::new(180000);
struct Scratch(PathBuf);
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn setup() -> (Session, Scratch, Value) {
    let (mut s, item) = fixture();
    let n = NEXT.fetch_add(1, Ordering::Relaxed);
    let dir = crate::temp_dir().join(format!("native-sequence-payload-{}-{n}", std::process::id()));
    fs::create_dir(&dir).unwrap();
    let file = dir.join("original.mov");
    let bytes = b"Generated source byte identity; no decoder/body is substituted by this unit test.";
    fs::write(&file, bytes).unwrap();
    let media = Arc::make_mut(&mut s.project).item_mut(ItemId(item)).unwrap().as_media_mut().unwrap();
    media.media = MediaRef::File { path: file.to_string_lossy().into_owned() };
    media.proxy = None;
    media.interpret = Default::default();
    let scope = json!({"projectId":"p_test","anchorSequence":s.state.active_sequence.unwrap().0});
    let original = json!({"item":item,"assetId":"a_source","mediaId":"m_original","path":file,
        "generation":{"kind":"poema-media-generation","version":1,"sha256":sequence_payload::hash(bytes),"bytes":bytes.len()}});
    let owner = json!({"workspaceId":"12345678-1234-4234-8234-123456789abc","operationId":"22345678-1234-4234-8234-123456789abc",
        "receiptSha256":"a".repeat(64),"planSha256":"b".repeat(64),"basisSha256":"c".repeat(64),"requestSha256":"d".repeat(64)});
    let owner_scope = json!({"project_id":"p_test","workspace_id":owner["workspaceId"],"operation_id":owner["operationId"]});
    let request = json!({"name":"Original native assembly"});
    let archive = json!({"receipt":{"kind":"poema-source-stringout","status":"complete","scope":owner_scope,"request":request,
        "plan":{"sha256":owner["planSha256"]},"basis_sha256":owner["basisSha256"],"request_sha256":owner["requestSha256"]},
        "plan":{"kind":"poema-stringout-plan","version":1,"scope":owner_scope,"request":request,
            "media":[{"asset_id":"a_source","original_path":file,"observation":{"generation":original["generation"]}}]}});
    let u = s.sequence_rate().frame_duration().0;
    let components: Vec<_> = (0..2)
        .map(|track| {
            json!({"key":format!("component-{track}"),"item":item,"track":track,"at":0,"duration":u*12,
        "sourceIn":0,"sourceOut":u*12,"speed":1.0,"timing":"nearestFrame","audioStream":if track==1{Some(0)}else{None},
        "channels":if track==1{vec![0]}else{vec![]},"link":"av","enabled":true,"gainDb":0.0,"fadeIn":0,"fadeOut":0,
        "annotations":{"curation":"Keep the original human decision"},"protection":null})
        })
        .collect();
    let projection = json!({"kind":"poema-native-stringout-projection","version":1,"scope":scope,"owner":owner,"originals":[original],
        "data":{"name":"Original native assembly","settings":s.active_sequence().unwrap().settings,"tracks":[{"kind":"Video","name":"Picture"},{"kind":"Audio","name":"Sound"}],
            "components":components,"markers":[],"metadata":{"poema.stringout.original.v1":archive.to_string()}},"summary":{},"native_edit_requested":false});
    (s, Scratch(dir), projection)
}
fn native_state(s: &Session) -> Value {
    json!({"project":s.project,"state":s.state,"revision":s.revision,"history":[s.history.undo.len(),s.history.redo.len()],"journal":s.journal})
}
fn begin(s: &mut Session, n: usize, raw: &[u8]) -> (String, Value) {
    read(s, n);
    let id = format!("sequence-payload-{n:032}");
    let result=s.execute("agent.beginSequencePayload",json!({"token":token(n),"payload":id,"scope":{"projectId":"p_test","anchorSequence":s.state.active_sequence.unwrap().0},"bytes":raw.len(),"sha256":sequence_payload::hash(raw)})).unwrap();
    (id, result)
}
fn upload(s: &mut Session, n: usize, projection: &Value) -> (String, String) {
    let raw = serde_json::to_vec(projection).unwrap();
    let (id, _) = begin(s, n, &raw);
    for (index, chunk) in raw.chunks(sequence_payload::CHUNK_BYTES).enumerate() {
        let result = s
            .execute(
                "agent.appendSequencePayload",
                json!({"token":token(n),"payload":id,"offset":index*sequence_payload::CHUNK_BYTES,"data":filmcraft_project::gtemplate::base64_encode(chunk)}),
            )
            .unwrap();
        assert_eq!(result["receivedBytes"], (index * sequence_payload::CHUNK_BYTES + chunk.len()) as u64);
        if result["receivedBytes"] == raw.len() {
            assert_eq!(result["status"], "ready");
        }
        assert_eq!(result["writeAuthority"], false);
    }
    (id, sequence_payload::hash(&raw))
}
fn batch(n: usize, id: &str, hash: &str) -> Value {
    json!({"token":token(n),"label":"Build original native stringout","exact":true,"operations":[{"command":"agent.assembleSequence","params":{"payload":id,"payloadSha256":hash}}]})
}

#[test]
fn complete_upload_is_not_an_edit_and_publication_preserves_the_old_project_and_owned_history() {
    let (mut s, _dir, p) = setup();
    let before = s.project.clone();
    let count = s.history.undo.len();
    let before_state = native_state(&s);
    let (id, hash) = upload(&mut s, 180001, &p);
    assert_eq!(native_state(&s), before_state);
    let request = batch(180001, &id, &hash);
    let receipt = s.execute("agent.apply", request.clone()).unwrap();
    assert_eq!(receipt["action"], "sequence_assembly");
    assert_eq!(receipt["results"][0]["oldProjectPreserved"], true);
    let qid = s.state.active_sequence.unwrap();
    assert_eq!(receipt["sequence"], qid.0, "history ownership follows the newly active sequence");
    let after = s.project.clone();
    let q = s.active_sequence().unwrap();
    assert_eq!(q.video_tracks[0].items.len(), 1);
    assert_eq!(q.audio_tracks[0].items.len(), 1);
    assert_eq!(s.history.undo.len(), count + 1);
    assert_eq!(q.video_tracks[0].items[0].link, q.audio_tracks[0].items[0].link);
    assert_eq!(s.execute("agent.apply", request).unwrap(), receipt);
    read(&mut s, 180002);
    let back = s.execute("agent.readSequenceAssembly", json!({"token":token(180002),"scope":p["scope"],"sequence":qid.0})).unwrap();
    assert_eq!(back["projectSha256"], receipt["results"][0]["afterProjectSha256"]);
    assert_eq!(back["sequence"]["sha256"], receipt["results"][0]["sequenceSha256"]);
    s.execute("agent.undo", json!({"token":token(180002),"commit":token(180001)})).unwrap();
    assert_eq!(s.project, before);
    read(&mut s, 180003);
    let missing = s.execute("agent.readSequenceAssembly", json!({"token":token(180003),"scope":p["scope"],"sequence":qid.0})).unwrap();
    assert_eq!(missing["sequence"], Value::Null);
    assert_eq!(missing["projectSha256"], receipt["results"][0]["beforeProjectSha256"]);
    s.execute("agent.redo", json!({"token":token(180003),"commit":token(180001),"undo":token(180002)})).unwrap();
    assert_eq!(s.project, after);
    let status = s.execute("agent.sequencePayloadStatus", json!({"token":token(180001),"payload":id})).unwrap();
    assert_eq!(status["status"], "committed");
}

#[test]
fn chunk_reconciliation_detects_holes_false_bytes_hashes_and_closed_mutation_without_an_edit() {
    let (mut s, _dir, p) = setup();
    let raw = serde_json::to_vec(&p).unwrap();
    let (id, first) = begin(&mut s, 180010, &raw);
    let before = native_state(&s);
    assert_eq!(first["prefixSha256"], sequence_payload::hash(&[]));
    let chunk = &raw[..64];
    let append = json!({"token":token(180010),"payload":id,"offset":0,"data":filmcraft_project::gtemplate::base64_encode(chunk)});
    let ack = s.execute("agent.appendSequencePayload", append.clone()).unwrap();
    assert_eq!(s.execute("agent.appendSequencePayload", append.clone()).unwrap(), ack);
    let mut hole = append.clone();
    hole["offset"] = json!(65);
    assert!(s.execute("agent.appendSequencePayload", hole).is_err());
    let mut wrong = append;
    wrong["data"] = json!(filmcraft_project::gtemplate::base64_encode(&[1; 64]));
    assert!(s.execute("agent.appendSequencePayload", wrong).is_err());
    s.execute("agent.discardSequencePayload", json!({"token":token(180010),"payload":id})).unwrap();
    assert!(s.execute("agent.appendSequencePayload", json!({"token":token(180010),"payload":id,"offset":64,"data":"YQ=="})).is_err());
    assert_eq!(native_state(&s), before);
    let (other, _) = begin(&mut s, 180011, &raw);
    let mut broken = raw.clone();
    broken[0] = b'[';
    assert!(
        s.execute(
            "agent.appendSequencePayload",
            json!({"token":token(180011),"payload":other,"offset":0,"data":filmcraft_project::gtemplate::base64_encode(&broken)})
        )
        .is_err()
    );
    let state = s.execute("agent.sequencePayloadStatus", json!({"token":token(180011),"payload":other})).unwrap();
    assert_eq!(state["status"], "refused");
    assert_eq!(native_state(&s), before);
}

#[test]
fn generations_preferences_scope_and_source_bytes_refuse_without_publication() {
    for change in ["generation", "preferences", "scope", "source", "cancelled"] {
        let (mut s, dir, mut p) = setup();
        if change == "generation" {
            p["originals"][0]["generation"]["sha256"] = json!("e".repeat(64));
            p["data"]["metadata"]["poema.stringout.original.v1"] = {
                let mut a: Value = serde_json::from_str(p["data"]["metadata"]["poema.stringout.original.v1"].as_str().unwrap()).unwrap();
                a["plan"]["media"][0]["observation"]["generation"]["sha256"] = json!("e".repeat(64));
                json!(a.to_string())
            };
        }
        let (id, hash) = upload(&mut s, 180020, &p);
        if change == "preferences" {
            s.execute("prefs.set", json!({"key":"media.defaultMediaScaling","value":"setToFrameSize"})).unwrap();
        }
        if change == "scope" {
            s.execute("timeline.setTrack", json!({"track":"V1","name":"Later human work"})).unwrap();
        }
        if change == "source" {
            fs::write(dir.0.join("original.mov"), b"Changed bytes after source qualification").unwrap();
        }
        if change == "cancelled" {
            s.agent_operation_control().cancel(&json!({"token":token(180020)})).unwrap();
        }
        let before = native_state(&s);
        assert!(s.execute("agent.apply", batch(180020, &id, &hash)).is_err(), "{change}");
        assert_eq!(native_state(&s), before, "{change}");
    }
}

#[test]
fn large_complete_archive_crosses_small_chunks_while_raw_command_limit_remains_unchanged() {
    let (mut s, _dir, mut p) = setup();
    let archive = p["data"]["metadata"]["poema.stringout.original.v1"].as_str().unwrap();
    let mut a: Value = serde_json::from_str(archive).unwrap();
    a["plan"]["complete_original_rationale"] = json!("Original note — ".repeat(300000));
    p["data"]["metadata"]["poema.stringout.original.v1"] = json!(a.to_string());
    let raw = serde_json::to_vec(&p).unwrap();
    assert!(raw.len() > 4 * 1024 * 1024);
    let before = native_state(&s);
    assert!(s.execute("sequence.assemble", p["data"].clone()).is_err());
    assert_eq!(native_state(&s), before);
    let (id, hash) = upload(&mut s, 180030, &p);
    let result = s.execute("agent.apply", batch(180030, &id, &hash)).unwrap();
    assert_eq!(result["results"][0]["components"], 2);
    let item = s.project.item(s.state.active_sequence.unwrap()).unwrap();
    assert_eq!(item.metadata["poema.stringout.original.v1"], p["data"]["metadata"]["poema.stringout.original.v1"].as_str().unwrap());
}

#[test]
fn journal_recovery_restores_complete_payload_and_captured_scaling_without_reusing_read_authority() {
    let (mut s, _dir, p) = setup();
    s.prefs.media.default_media_scaling = "scaleToFrameSize".into();
    let before = s.project.clone();
    let state = s.state.clone();
    let (id, hash) = upload(&mut s, 180040, &p);
    s.execute("agent.apply", batch(180040, &id, &hash)).unwrap();
    let journal = s.journal.last().unwrap().clone();
    assert_eq!(journal.0, "agent.replayBatch");
    assert!(journal.1["operations"][0]["params"]["projectionSource"].is_string());
    let mut reopened = Session::new(s.services.clone());
    reopened.project = before;
    reopened.state = state;
    reopened.media = s.media.clone();
    reopened.prefs.media.default_media_scaling = "none".into();
    reopened.execute(&journal.0, journal.1).unwrap();
    assert_eq!(reopened.prefs.media.default_media_scaling, "none");
    assert_eq!(
        reopened.active_sequence().unwrap().video_tracks[0].items[0].scale_to_frame,
        s.active_sequence().unwrap().video_tracks[0].items[0].scale_to_frame
    );
    assert_eq!(reopened.active_sequence().unwrap().duration(), s.active_sequence().unwrap().duration());
    assert_eq!(reopened.history.undo.len(), 1);
}
