use super::tests::{read, token};
use crate::Session;
use serde_json::{Value, json};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

static SCRATCH_ID: AtomicU64 = AtomicU64::new(0);

struct Scratch(std::path::PathBuf);
impl Scratch {
    fn new() -> Self {
        Self::at(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos())
    }
    fn at(clock: u128) -> Self {
        let path = std::env::temp_dir().join(format!("filmcraft-export-{}-{}-{}", std::process::id(), clock, SCRATCH_ID.fetch_add(1, Ordering::Relaxed)));
        std::fs::create_dir(&path).unwrap();
        Self(std::fs::canonicalize(path).unwrap())
    }
}
#[test]
fn export_test_directories_remain_unique_when_parallel_tests_observe_the_same_clock() {
    let workers: Vec<_> = (0..32).map(|_| std::thread::spawn(|| Scratch::at(0))).collect();
    let directories: Vec<_> = workers.into_iter().map(|worker| worker.join().unwrap()).collect();
    let names: std::collections::BTreeSet<_> = directories.iter().map(|directory| directory.0.clone()).collect();
    assert_eq!(names.len(), directories.len());
    assert!(directories.iter().all(|directory| directory.0.is_dir()));
}
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn setup() -> Session {
    let mut s = super::tests::setup();
    s.agent_operation_control().enable_transport();
    let seq = s.state.active_sequence.unwrap();
    let project = Arc::make_mut(&mut s.project);
    let q = project.sequence_mut(seq).unwrap();
    q.settings.width = 64;
    q.settings.height = 64;
    q.mark_in = None;
    q.mark_out = None;
    q.video_tracks.truncate(1);
    q.audio_tracks.truncate(1);
    q.caption_tracks.clear();
    q.markers.clear();
    let duration = q.settings.frame_rate.tick_of(3);
    for track in q.all_tracks_mut() {
        track.items.truncate(1);
        track.transitions.clear();
        for clip in &mut track.items {
            clip.start = filmcraft_time::Tick::ZERO;
            clip.duration = duration;
            clip.source_in = filmcraft_time::Tick::ZERO;
            clip.effects.clear();
        }
    }
    s.history.undo.clear();
    s.history.redo.clear();
    s
}
fn options(s: &Session, n: usize, name: &str) -> Value {
    json!({"token":token(n),"scope":{"projectId":"p_export","anchorSequence":s.state.active_sequence.unwrap().0},
    "sequence":s.state.active_sequence.unwrap().0,"name":name,"range":{"kind":"entire"},"settings":{"format":"prores"}})
}
fn start(s: &mut Session, dir: &Scratch, n: usize, name: &str) -> (Value, Value) {
    read(s, n);
    let options = options(s, n, name);
    let resolved = s.execute("agent.resolveExport", options.clone()).unwrap();
    let request = json!({"token":token(n),"options":options,"job":token(n+1),"root":dir.0,"settingsSha256":resolved["settingsSha256"]});
    let reply = s.execute("agent.startExport", request.clone()).unwrap();
    (request, reply)
}
fn ready(s: &mut Session, job: &Value) -> Value {
    for _ in 0..200 {
        let r = s.execute("agent.exportStatus", json!({"job":job})).unwrap();
        if r["status"] != "running" {
            assert_eq!(r["status"], "ready", "{r}");
            return r;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    panic!("native export did not finish");
}
fn deliver(s: &mut Session, request: &Value, n: usize) -> crate::Result<Value> {
    let params = json!({"token":token(n+1_000_000),"job":request["job"],"scope":request["options"]["scope"]});
    s.execute("agent.prepareExportDelivery", params.clone())?;
    s.execute("agent.publishExport", params)
}
#[test]
fn native_export_uses_human_settings_encoder_without_editing_and_publishes_a_new_complete_file_set() {
    let dir = Scratch::new();
    let mut s = setup();
    let before = serde_json::to_value(&*s.project).unwrap();
    let state = serde_json::to_value(&s.state).unwrap();
    let revision = s.revision;
    let (request, _) = start(&mut s, &dir, 70_000, "test-delivery");
    assert!(!dir.0.join("test-delivery").exists());
    ready(&mut s, &request["job"]);
    let mut human = Session::new(s.services.clone());
    human.project = s.project.clone();
    human.state = s.state.clone();
    human.media = s.media.clone();
    let human_path = dir.0.join("human.mov");
    let native = human.execute("file.exportMedia", json!({"path":human_path,"format":"prores","range":"entire","wait":true})).unwrap();
    assert!(native["result"]["error"].is_null(), "{native}");
    read(&mut s, 70_002);
    let reply = deliver(&mut s, &request, 70_002).unwrap();
    assert_eq!(reply["status"], "delivered");
    assert_eq!(reply["receipt"]["complete"], true);
    assert_eq!(std::fs::read(dir.0.join("test-delivery/test-delivery.mov")).unwrap(), std::fs::read(&human_path).unwrap());
    assert!(dir.0.join("test-delivery/poema-export.json").is_file());
    assert_eq!(serde_json::to_value(&*s.project).unwrap(), before);
    assert_eq!(serde_json::to_value(&s.state).unwrap(), state);
    assert_eq!(s.revision, revision);
    assert!(s.history.undo.is_empty());
    let recovered = s
        .execute(
            "agent.exportReceipt",
            json!({"job":request["job"],"root":dir.0,"name":"test-delivery","scope":request["options"]["scope"],
        "sequence":request["options"]["sequence"],"revision":revision,"settingsSha256":request["settingsSha256"],"requestSha256":reply["receipt"]["requestSha256"]}),
        )
        .unwrap();
    assert_eq!(recovered["receipt"], reply["receipt"]);
    assert_eq!(recovered["currentFileBytesVerified"], false);
    let again = s.execute("agent.startExport", request).unwrap();
    assert_eq!(again["status"], "delivered");
    assert_eq!(s.jobs.len(), 1);
}
#[test]
fn native_export_rejects_hostile_settings_ranges_stale_plans_and_existing_destinations_before_encoding() {
    let dir = Scratch::new();
    let mut s = setup();
    read(&mut s, 71_000);
    let clean = options(&s, 71_000, "test");
    for patch in [json!({"path":"bad"}), json!({"partOfBatch":true}), json!({"typo":1}), json!({"frame_size":[0,0]}), json!({"frameRate":{"num":0,"den":1}})] {
        let mut o = clean.clone();
        o["settings"] = patch;
        assert!(s.execute("agent.resolveExport", o).is_err());
    }
    for range in [json!({"kind":"frames","start":-1,"end":1}), json!({"kind":"frames","start":1,"end":1}), json!({"kind":"frames","start":0,"end":i64::MAX})] {
        let mut o = clean.clone();
        o["range"] = range;
        assert!(s.execute("agent.resolveExport", o).is_err());
    }
    std::fs::create_dir(dir.0.join("test")).unwrap();
    std::fs::write(dir.0.join("test/preserve.txt"), b"keep").unwrap();
    let resolved = s.execute("agent.resolveExport", clean.clone()).unwrap();
    let req = json!({"token":token(71_000),"options":clean,"job":token(71_001),"root":dir.0,"settingsSha256":resolved["settingsSha256"]});
    assert!(s.execute("agent.startExport", req).is_err());
    assert!(s.jobs.is_empty());
    assert_eq!(std::fs::read(dir.0.join("test/preserve.txt")).unwrap(), b"keep");
    s.execute("sequence.setPlayhead", json!({"frame":1})).ok();
    Arc::make_mut(&mut s.project).name = "human change".into();
    assert!(s.execute("agent.resolveExport", clean).is_err());
}
#[test]
fn native_export_stop_tombstones_prevent_late_start_and_captured_cuts_survive_human_edits() {
    let dir = Scratch::new();
    let mut s = setup();
    read(&mut s, 72_000);
    let o = options(&s, 72_000, "stopped");
    let resolved = s.execute("agent.resolveExport", o.clone()).unwrap();
    s.execute("agent.cancelExport", json!({"job":token(72_001)})).unwrap();
    let reply = s
        .execute("agent.startExport", json!({"token":token(72_000),"options":o,"job":token(72_001),"root":dir.0,"settingsSha256":resolved["settingsSha256"]}))
        .unwrap();
    assert_eq!(reply["status"], "cancelled");
    assert!(s.jobs.is_empty());
    let (request, _) = start(&mut s, &dir, 72_010, "stale");
    ready(&mut s, &request["job"]);
    let captured = s.revision;
    let audio = s.active_sequence().unwrap().audio_tracks[0].items[0].id.0;
    s.execute("clip.audioGain", json!({"clips":[audio],"mode":"set","db":-12})).unwrap();
    let changed = serde_json::to_value(&*s.project).unwrap();
    let history = s.history.undo.len();
    read(&mut s, 72_012);
    let delivered = deliver(&mut s, &request, 72_012).unwrap();
    assert_eq!(delivered["status"], "delivered");
    assert_eq!(delivered["receipt"]["revision"], captured);
    assert_eq!(delivered["receipt"]["currentRevisionAtDelivery"], s.revision);
    assert_eq!(delivered["receipt"]["cutChangedAfterRequest"], true);
    assert_eq!(serde_json::to_value(&*s.project).unwrap(), changed);
    assert_eq!(s.history.undo.len(), history);
}
#[test]
fn native_export_corrupt_encoded_output_cannot_earn_a_completion_manifest() {
    let dir = Scratch::new();
    let mut s = setup();
    let (request, _) = start(&mut s, &dir, 73_000, "corrupt");
    ready(&mut s, &request["job"]);
    let stage = dir.0.join(format!(".poema-export-{}", request["job"].as_str().unwrap()));
    std::fs::write(stage.join("files/corrupt.mov"), b"nonempty but invalid encoded output").unwrap();
    read(&mut s, 73_002);
    let result = deliver(&mut s, &request, 73_002);
    assert!(result.is_err());
    assert!(!dir.0.join("corrupt").exists());
    assert!(s.history.undo.is_empty());
}
#[test]
fn native_export_late_destination_collision_never_replaces_foreign_files_or_claims_delivery() {
    let dir = Scratch::new();
    let mut s = setup();
    let (request, _) = start(&mut s, &dir, 74_000, "collision");
    ready(&mut s, &request["job"]);
    std::fs::create_dir(dir.0.join("collision")).unwrap();
    std::fs::write(dir.0.join("collision/keep.mov"), b"human file").unwrap();
    read(&mut s, 74_002);
    assert_eq!(deliver(&mut s, &request, 74_002).unwrap()["status"], "failed");
    assert_eq!(std::fs::read(dir.0.join("collision/keep.mov")).unwrap(), b"human file");
    assert!(!dir.0.join("collision/poema-export.json").exists());
    assert_eq!(s.execute("agent.exportStatus", json!({"job":request["job"]})).unwrap()["status"], "failed");
}
#[test]
fn native_export_cancelled_stage_cannot_publish_and_receipts_survive_fresh_sessions_without_reencoding() {
    let dir = Scratch::new();
    let mut s = setup();
    let (stopped, _) = start(&mut s, &dir, 75_000, "stop");
    s.execute("agent.cancelExport", json!({"job":stopped["job"]})).unwrap();
    read(&mut s, 75_002);
    assert!(deliver(&mut s, &stopped, 75_002).is_err());
    assert!(!dir.0.join("stop").exists());
    let (request, _) = start(&mut s, &dir, 75_010, "reopen");
    ready(&mut s, &request["job"]);
    read(&mut s, 75_012);
    let reply = deliver(&mut s, &request, 75_012).unwrap();
    assert_eq!(reply["receipt"]["nativeDecodeVerified"], true);
    assert!(reply["receipt"]["decode"]["videoSamples"].as_array().unwrap().len() > 1);
    assert!(reply["receipt"]["decode"]["audioSamples"].as_array().unwrap().len() > 1);
    let mut fresh = setup();
    assert_eq!(fresh.execute("agent.exportStatus", json!({"job":request["job"]})).unwrap()["status"], "unknown");
    let recovered = fresh
        .execute(
            "agent.exportReceipt",
            json!({"job":request["job"],"root":dir.0,"name":"reopen","scope":request["options"]["scope"],
        "sequence":request["options"]["sequence"],"revision":reply["receipt"]["revision"],"settingsSha256":request["settingsSha256"],"requestSha256":reply["receipt"]["requestSha256"]}),
        )
        .unwrap();
    assert_eq!(recovered["receipt"], reply["receipt"]);
    assert_eq!(recovered["historical"], true);
    assert!(fresh.jobs.is_empty());
}
#[test]
fn native_export_exact_ranges_audio_only_and_image_sets_use_declared_native_output() {
    for (index, format) in ["wav", "png", "mjpeg"].into_iter().enumerate() {
        let dir = Scratch::new();
        let mut s = setup();
        let n = 76_000 + index * 10;
        read(&mut s, n);
        let mut o = options(&s, n, "range");
        o["range"] = json!({"kind":"frames","start":1,"end":3});
        o["settings"]["format"] = json!(format);
        let resolved = s.execute("agent.resolveExport", o.clone()).unwrap();
        assert_eq!(resolved["resolved"]["range"]["start"], s.active_sequence().unwrap().settings.frame_rate.tick_of(1).0);
        let request = json!({"token":token(n),"options":o,"job":token(n+1),"root":dir.0,"settingsSha256":resolved["settingsSha256"]});
        s.execute("agent.startExport", request.clone()).unwrap();
        ready(&mut s, &request["job"]);
        read(&mut s, n + 2);
        let reply = deliver(&mut s, &request, n + 2).unwrap();
        assert_eq!(reply["status"], "delivered", "{reply}");
        if format == "png" {
            assert_eq!(reply["receipt"]["files"].as_array().unwrap().len(), 2);
        }
    }
}
#[test]
fn native_export_declares_burned_captions_and_sidecar_without_changing_authored_cues() {
    let dir = Scratch::new();
    let mut s = setup();
    s.execute("captions.newTrack", json!({"name":"Exact export words","language":"en","format":"Subtitle"})).unwrap();
    s.execute("captions.add", json!({"text":"Native words","time":0,"duration":s.sequence_rate().tick_of(3).0})).unwrap();
    let before = serde_json::to_value(&*s.project).unwrap();
    let history = s.history.undo.len();
    read(&mut s, 77_000);
    let mut o = options(&s, 77_000, "captioned");
    o["settings"]["captionSidecar"] = json!("srt");
    o["settings"]["burnCaptions"] = json!(true);
    let resolved = s.execute("agent.resolveExport", o.clone()).unwrap();
    let request = json!({"token":token(77_000),"options":o,"job":token(77_001),"root":dir.0,"settingsSha256":resolved["settingsSha256"]});
    s.execute("agent.startExport", request.clone()).unwrap();
    ready(&mut s, &request["job"]);
    read(&mut s, 77_002);
    let result = deliver(&mut s, &request, 77_002).unwrap();
    assert_eq!(result["status"], "delivered");
    assert_eq!(result["receipt"]["files"].as_array().unwrap().len(), 2);
    assert!(std::fs::read_to_string(dir.0.join("captioned/captioned.srt")).unwrap().contains("Native words"));
    assert_eq!(result["receipt"]["resolved"]["settings"]["burnCaptions"], true);
    assert_eq!(serde_json::to_value(&*s.project).unwrap(), before);
    assert_eq!(s.history.undo.len(), history);
}
#[test]
fn native_export_records_source_decode_failures_even_when_the_encoder_produces_a_readable_movie() {
    struct BrokenPicture(filmcraft_media::MediaInfo);
    impl filmcraft_media::MediaSource for BrokenPicture {
        fn info(&self) -> &filmcraft_media::MediaInfo {
            &self.0
        }
        fn video_frame(&self, _: filmcraft_media::FrameRequest) -> filmcraft_media::Result<Arc<filmcraft_frame::VideoFrame>> {
            Err(filmcraft_media::MediaError::Decode("generated damaged picture".into()))
        }
        fn audio(&self, _: i64, n: usize, rate: u32) -> filmcraft_media::Result<filmcraft_frame::AudioBuffer> {
            Ok(filmcraft_frame::AudioBuffer::silence(rate, 2, n))
        }
    }
    let dir = Scratch::new();
    let mut s = setup();
    let item = s.active_sequence().unwrap().video_tracks[0].items[0].item;
    let info = s.project.item(item).unwrap().as_media().unwrap().info.clone();
    s.media.insert(item, Arc::new(BrokenPicture(info)));
    let (request, _) = start(&mut s, &dir, 79_000, "decode-failed");
    let mut result = Value::Null;
    for _ in 0..200 {
        result = s.execute("agent.exportStatus", json!({"job":request["job"]})).unwrap();
        if result["status"] != "running" {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert_eq!(result["status"], "failed", "{result}");
    assert!(result["nativeJob"]["result"]["error"].is_null(), "{result}");
    assert!(
        result["nativeJob"]["offlineMedia"].as_array().unwrap().iter().any(|m| m["error"].as_str().is_some_and(|s| s.contains("generated damaged picture")))
    );
    assert!(deliver(&mut s, &request, 79_002).is_err());
    assert!(!dir.0.join("decode-failed").exists());
    assert!(s.history.undo.is_empty());
}
#[test]
fn native_export_publication_metadata_cannot_authorize_timeline_edits_and_independent_stop_fences_delivery() {
    let dir = Scratch::new();
    let mut s = setup();
    let (request, _) = start(&mut s, &dir, 78_000, "fenced");
    ready(&mut s, &request["job"]);
    let parameters = json!({"token":token(1_078_002),"job":request["job"],"scope":request["options"]["scope"]});
    let basis = s.execute("agent.prepareExportDelivery", parameters.clone()).unwrap();
    assert_eq!(basis["kind"], "native_export_publication_basis");
    assert_eq!(basis["requiresSourceValidation"], true);
    let before = serde_json::to_value(&*s.project).unwrap();
    let clip = s.active_sequence().unwrap().video_tracks[0].items[0].id.0;
    assert!(s.execute("agent.apply",json!({"token":parameters["token"],"label":"Not an edit ticket","exact":true,"operations":[{"command":"timeline.razor","params":{"clip":clip,"time":1}}]})).is_err());
    assert_eq!(serde_json::to_value(&*s.project).unwrap(), before);
    let cancelled = s.agent_operation_control().cancel(&json!({"token":parameters["token"]})).unwrap();
    assert_eq!(cancelled["cancellationAccepted"], true);
    assert!(s.execute("agent.publishExport", parameters).is_err());
    assert!(!dir.0.join("fenced").exists());
    assert!(s.history.undo.is_empty());
}
