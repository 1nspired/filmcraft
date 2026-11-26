use super::*;
use crate::agent_bridge::tests::{read, setup, token};

use super::TEST_SERIAL as SERIAL;
fn fixture() -> Session {
    let mut s = setup();
    for item in Arc::make_mut(&mut s.project).items.values_mut().filter(|i| i.as_media().is_some()) {
        item.metadata
            .insert(BINDING_KEY.into(), json!({"projectId":"p_test","assetId":format!("a_{}",item.id.0),"mediaId":"original","role":"original"}).to_string());
    }
    s
}
fn start_read(s: &mut Session, n: usize, request: Value) -> Value {
    let d = read(s, n);
    let p = json!({"token":token(n),"job":token(n+10000),"scope":{"projectId":"p_test","anchorSequence":d["sequence"]["id"]},"request":request});
    s.execute(START, p.clone()).unwrap();
    p
}
fn wait(s: &mut Session, id: &Value) -> Value {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let result = s.execute("agent.inspection", json!({"job":id})).unwrap();
        if result["status"] != "running" {
            return result;
        }
        assert!(Instant::now() < deadline, "worker did not finish");
        std::thread::sleep(Duration::from_millis(2));
    }
}
fn unchanged(s: &Session) -> Value {
    json!({"project":s.project,"state":s.state,"revision":s.revision,"saved":s.saved_revision,"dirty":s.is_dirty(),
        "undo":s.history.undo.len(),"redo":s.history.redo.len(),"journal":s.journal,"events":s.events.len(),"jobs":s.jobs.len()})
}

#[test]
fn frame_is_native_composite_with_effects_and_captions_without_seeking_or_history_changes() {
    let _guard = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let mut s = fixture();
    let seq = s.state.active_sequence.unwrap();
    let time = s.sequence_rate().tick_of(30);
    s.execute("captions.add", json!({"time":time.0,"text":"Native evidence caption","durationSeconds":3})).unwrap();
    let before = unchanged(&s);
    let p = start_read(&mut s, 1, json!({"kind":"frame","frame":30,"maxSide":96}));
    let result = wait(&mut s, &p["job"]);
    assert_eq!(result["status"], "complete", "{result}");
    let png = filmcraft_project::gtemplate::base64_decode(result["result"]["data"].as_str().unwrap()).unwrap();
    let image = image::load_from_memory(&png).unwrap().to_rgba8();
    let q = s.active_sequence().unwrap();
    let scale = 96.0 / q.settings.width.max(q.settings.height) as f32;
    let reference = filmcraft_render::render_sequence(
        &s.project,
        seq,
        q.settings.frame_rate.tick_of(30),
        RenderOptions { scale, captions: true, ..Default::default() },
        &s.media.full_res_provider(s.project.clone(), s.services.clone()),
    );
    assert_eq!(image.into_raw(), reference.over_black_rgba8());
    let no_captions = filmcraft_render::render_sequence(
        &s.project,
        seq,
        time,
        RenderOptions { scale, captions: false, ..Default::default() },
        &s.media.full_res_provider(s.project.clone(), s.services.clone()),
    );
    assert_ne!(reference.over_black_rgba8(), no_captions.over_black_rgba8(), "caption pixels actually reach the evidence");
    assert_eq!(unchanged(&s), before);
    assert_eq!(s.execute(START, p.clone()).unwrap(), result, "lost read replies never start duplicate jobs");
    let mut conflict = p;
    conflict["request"]["frame"] = json!(31);
    assert!(s.execute(START, conflict).is_err());
}

struct Gate {
    open: Mutex<bool>,
    wake: std::sync::Condvar,
}
impl Gate {
    fn release(&self) {
        *self.open.lock().unwrap_or_else(|e| e.into_inner()) = true;
        self.wake.notify_all();
    }
}
impl Drop for Gate {
    fn drop(&mut self) {
        self.release();
    }
}
struct Blocked(SharedSource, Arc<Gate>);
impl MediaSource for Blocked {
    fn info(&self) -> &MediaInfo {
        self.0.info()
    }
    fn video_frame(&self, r: FrameRequest) -> filmcraft_media::Result<Arc<VideoFrame>> {
        let mut ready = self.1.open.lock().unwrap_or_else(|e| e.into_inner());
        while !*ready {
            ready = self.1.wake.wait(ready).unwrap_or_else(|e| e.into_inner());
        }
        self.0.video_frame(r)
    }
    fn audio(&self, start: i64, n: usize, sr: u32) -> filmcraft_media::Result<AudioBuffer> {
        self.0.audio(start, n, sr)
    }
}
#[test]
fn cancelled_decoders_keep_their_permit_until_exit_and_cannot_spawn_unbounded_workers() {
    let _guard = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let mut s = fixture();
    let item = s.active_sequence().unwrap().video_tracks[0].items[0].item;
    let gate = Arc::new(Gate { open: Mutex::new(false), wake: std::sync::Condvar::new() });
    // A separate release guard prevents a failed assertion from stranding worker threads.
    struct Release(Arc<Gate>);
    impl Drop for Release {
        fn drop(&mut self) {
            self.0.release();
        }
    }
    let _release = Release(gate.clone());
    s.media.insert(item, Arc::new(Blocked(s.source(item).unwrap(), gate.clone())));
    let first = start_read(&mut s, 30, json!({"kind":"frame","frame":30,"maxSide":64}));
    let second = start_read(&mut s, 31, json!({"kind":"frame","frame":30,"maxSide":64}));
    assert_eq!(WORKERS.load(Ordering::SeqCst), 2);
    s.execute("agent.cancelInspection", json!({"job":first["job"]})).unwrap();
    let mut third = second.clone();
    third["job"] = json!(token(10032));
    assert!(s.execute(START, third).is_err(), "cancellation is not decoder termination");
    gate.release();
    assert_eq!(wait(&mut s, &second["job"])["status"], "complete");
    let deadline = Instant::now() + Duration::from_secs(5);
    while WORKERS.load(Ordering::SeqCst) != 0 {
        assert!(Instant::now() < deadline);
        std::thread::yield_now();
    }
}

#[test]
fn mixed_audio_is_exact_float_pcm_from_native_graph_and_does_not_touch_editor_state() {
    let _guard = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let mut s = fixture();
    let before = unchanged(&s);
    let p = start_read(&mut s, 2, json!({"kind":"audio","startSample":2400,"samples":2400}));
    let result = wait(&mut s, &p["job"]);
    assert_eq!(result["status"], "complete", "{result}");
    let provider = StrictSources {
        witnesses: Arc::default(),
        base: s.media.full_res_provider(s.project.clone(), s.services.clone()).recording(),
        project_id: "p_test".into(),
        stop: Arc::new(AtomicBool::new(false)),
        error: Arc::new(AtomicBool::new(false)),
        deadline: Instant::now(),
        budget: DEADLINE,
    };
    let expected = filmcraft_render::mixer::mix_graph(&s.project, s.active_sequence().unwrap(), 2400, 2400, &provider, None);
    let wav = filmcraft_project::gtemplate::base64_decode(result["result"]["data"].as_str().unwrap()).unwrap();
    let src = filmcraft_media::open_bytes("mix.wav", wav.into(), &[]).unwrap();
    let decoded = src.audio(0, 2400, expected.sample_rate).unwrap();
    assert_eq!(decoded, expected);
    assert_eq!(unchanged(&s), before);
    assert!(!provider.reuse_audio_state());
}

#[test]
fn stale_scope_and_hostile_ranges_refuse_without_project_or_history_publication() {
    let _guard = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    for request in [
        json!({"kind":"frame","frame":-1,"maxSide":96}),
        json!({"kind":"frame","frame":0,"maxSide":100000}),
        json!({"kind":"frame","frame":i64::MAX,"maxSide":96}),
        json!({"kind":"audio","startSample":i64::MAX,"samples":1}),
        json!({"kind":"audio","startSample":0,"samples":240001}),
        json!({"kind":"audio","startSample":0,"samples":0}),
        json!({"kind":"frame","frame":0,"maxSide":96,"path":"/tmp/not-allowed"}),
    ] {
        let mut s = fixture();
        let d = read(&mut s, 3);
        let before = unchanged(&s);
        let p = json!({"token":token(3),"job":token(10003),"scope":{"projectId":"p_test","anchorSequence":d["sequence"]["id"]},"request":request});
        assert!(s.execute(START, p).is_err());
        assert_eq!(unchanged(&s), before);
    }
    let mut s = fixture();
    let d = read(&mut s, 4);
    let p = json!({"token":token(4),"job":token(10004),"scope":{"projectId":"p_test","anchorSequence":d["sequence"]["id"]},"request":{"kind":"frame","frame":0,"maxSide":96}});
    let mut foreign = p.clone();
    foreign["scope"]["projectId"] = json!("p_foreign");
    let before = unchanged(&s);
    assert!(s.execute(START, foreign).is_err());
    assert_eq!(unchanged(&s), before);
    s.revision += 1;
    assert!(s.execute(START, p).is_err());
}

#[test]
fn completed_evidence_cannot_cross_a_project_change_but_playhead_motion_is_not_an_edit() {
    let _guard = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let mut s = fixture();
    let p = start_read(&mut s, 5, json!({"kind":"frame","frame":0,"maxSide":96}));
    s.set_playhead(s.sequence_rate().tick_of(6));
    let r = wait(&mut s, &p["job"]);
    assert_eq!(r["status"], "complete");
    // Even a same-path/same-revision replacement project must not receive old evidence.
    s.project = Arc::new((*s.project).clone());
    assert!(s.execute("agent.inspection", json!({"job":p["job"]})).is_err());
    assert_eq!(s.execute("agent.cancelInspection", json!({"job":p["job"]})).unwrap()["status"], "cancelled");
}

struct Broken(SharedSource, bool);
impl MediaSource for Broken {
    fn info(&self) -> &MediaInfo {
        self.0.info()
    }
    fn video_frame(&self, _: FrameRequest) -> filmcraft_media::Result<Arc<VideoFrame>> {
        assert!(!self.1, "synthetic worker panic");
        Err(filmcraft_media::MediaError::Decode("synthetic failure".into()))
    }
    fn audio(&self, _: i64, _: usize, _: u32) -> filmcraft_media::Result<AudioBuffer> {
        Err(filmcraft_media::MediaError::Decode("synthetic audio failure".into()))
    }
}
#[test]
fn missing_bindings_decode_errors_and_panics_never_become_valid_black_or_silence_evidence() {
    let _guard = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    for mode in 0..4 {
        let mut s = fixture();
        let item = s.active_sequence().unwrap().video_tracks[0].items[0].item;
        if mode == 0 {
            Arc::make_mut(&mut s.project).item_mut(item).unwrap().metadata.remove(BINDING_KEY);
        } else {
            let ids: Vec<_> = s.project.items.values().filter(|i| i.as_media().is_some()).map(|i| i.id).collect();
            for id in ids {
                let source = s.source(id).unwrap();
                s.media.insert(id, Arc::new(Broken(source, mode == 2)));
            }
        }
        let before = unchanged(&s);
        let request = if mode == 3 { json!({"kind":"audio","startSample":0,"samples":1024}) } else { json!({"kind":"frame","frame":0,"maxSide":96}) };
        let p = start_read(&mut s, 6, request);
        let result = wait(&mut s, &p["job"]);
        assert_eq!(result["status"], "failed", "mode {mode}: {result}");
        assert!(result.get("result").is_none());
        assert_eq!(unchanged(&s), before);
    }
}

#[test]
fn cancellation_discards_even_completed_bytes_and_cache_is_bounded() {
    let _guard = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let mut s = fixture();
    for n in 10..17 {
        let p = start_read(&mut s, n, json!({"kind":"frame","frame":0,"maxSide":64}));
        assert_eq!(wait(&mut s, &p["job"])["status"], "complete");
        s.execute("agent.cancelInspection", json!({"job":p["job"]})).unwrap();
        assert_eq!(wait(&mut s, &p["job"])["status"], "cancelled");
        assert!(s.agent_bridge.inspections.len() <= MAX_JOBS);
    }
}

struct Original(std::path::PathBuf);
impl Drop for Original {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
struct Release(Arc<Gate>);
impl Drop for Release {
    fn drop(&mut self) {
        self.0.release();
    }
}
fn background(s: &mut Session, n: usize) -> Value {
    let d = read(s, n);
    let p = json!({"token":token(n),"job":token(n+10000),"scope":{"projectId":"p_test","anchorSequence":d["sequence"]["id"]},"request":{"kind":"frame","frame":30,"maxSide":64},"background":true});
    s.execute(START, p.clone()).unwrap();
    p
}
fn blocked_background(s: &mut Session, n: usize) -> (Original, Value, Release) {
    let item = s.active_sequence().unwrap().video_tracks[0].items[0].item;
    let source = s.source(item).unwrap();
    let original = Original(std::env::temp_dir().join(format!(
        "filmcraft-inspection-original-{}-{}",
        std::process::id(),
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
    )));
    std::fs::create_dir(&original.0).unwrap();
    let file = original.0.join("original.bin");
    std::fs::write(&file, b"original bytes for witness").unwrap();
    Arc::make_mut(&mut s.project).item_mut(item).unwrap().as_media_mut().unwrap().media =
        filmcraft_project::MediaRef::File { path: file.to_string_lossy().into_owned() };
    let d = read(s, n);
    let gate = Arc::new(Gate { open: Mutex::new(false), wake: std::sync::Condvar::new() });
    s.media.insert(item, Arc::new(Blocked(source, gate.clone())));
    let p = json!({"token":token(n),"job":token(n+10000),"scope":{"projectId":"p_test","anchorSequence":d["sequence"]["id"]},"request":{"kind":"frame","frame":30,"maxSide":64},"background":true});
    s.execute(START, p.clone()).unwrap();
    let until = Instant::now() + Duration::from_secs(3);
    while s.execute("agent.inspection", json!({"job":p["job"]})).unwrap()["phase"] != "rendering" {
        assert!(Instant::now() < until);
        std::thread::sleep(Duration::from_millis(1));
    }
    (original, p, Release(gate))
}
fn workers_finished() {
    let until = Instant::now() + Duration::from_secs(3);
    while WORKERS.load(Ordering::SeqCst) > 0 {
        assert!(Instant::now() < until);
        std::thread::sleep(Duration::from_millis(1));
    }
}

#[test]
fn background_inspection_leaves_engine_available_and_returns_only_twice_verified_output() {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let mut s = fixture();
    let (_original, p, release) = blocked_background(&mut s, 29000);
    let before = unchanged(&s);
    let tickets = s.agent_bridge.tickets.len();
    let reading = s.execute("agent.inspection", json!({"job":p["job"]})).unwrap();
    assert_eq!(reading["status"], "running");
    assert_eq!(reading["progress"]["completed_checks"], 1);
    assert_eq!(reading["progress"]["total_checks"], 2);
    assert!(reading.get("result").is_none());
    assert!(s.execute("sequence.inspect", json!({})).is_ok());
    assert_eq!(unchanged(&s), before);
    assert_eq!(s.agent_bridge.tickets.len(), tickets);
    release.0.release();
    let done = wait(&mut s, &p["job"]);
    assert_eq!(done["status"], "complete", "{done}");
    assert_eq!(done["phase"], "complete");
    assert_eq!(done["requiresPostflight"], true);
    assert_eq!(done["progress"]["completed_checks"], 2);
    let value = filmcraft_project::gtemplate::base64_decode(done["result"]["data"].as_str().unwrap()).unwrap();
    assert!(image::load_from_memory(&value).is_ok());
    assert_eq!(unchanged(&s), before);
    workers_finished();
}
#[test]
fn background_inspection_rejects_replaced_originals_during_decode_before_exposing_pixels() {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let mut s = fixture();
    let (original, p, release) = blocked_background(&mut s, 29100);
    let before = unchanged(&s);
    let file = original.0.join("original.bin");
    let meta = std::fs::metadata(&file).unwrap();
    std::fs::write(&file, b"replaced bytes for witness").unwrap();
    assert_eq!(std::fs::metadata(&file).unwrap().len(), meta.len());
    std::fs::OpenOptions::new().write(true).open(&file).unwrap().set_times(std::fs::FileTimes::new().set_modified(meta.modified().unwrap())).unwrap();
    release.0.release();
    let out = wait(&mut s, &p["job"]);
    assert_eq!(out["status"], "failed", "{out}");
    assert!(out.get("result").is_none());
    assert_eq!(unchanged(&s), before);
    workers_finished();
}
#[test]
fn background_inspection_cancellation_stops_shared_byte_checks_and_tombstones_delayed_starts() {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let mut s = fixture();
    let (_original, p, release) = blocked_background(&mut s, 29200);
    let before = unchanged(&s);
    s.execute("agent.cancelInspection", json!({"job":p["job"]})).unwrap();
    assert_eq!(s.execute(START, p.clone()).unwrap()["status"], "cancelled");
    let j = s.agent_bridge.inspections.back().unwrap();
    assert!(j.verification.as_ref().unwrap().check.check().is_err());
    assert_eq!(WORKERS.load(Ordering::SeqCst), 1);
    release.0.release();
    workers_finished();
    assert_eq!(unchanged(&s), before);
    let id = token(39203);
    s.execute("agent.cancelInspection", json!({"job":id})).unwrap();
    let mut late = p;
    late["job"] = json!(id);
    assert_eq!(s.execute(START, late).unwrap()["status"], "cancelled");
    assert_eq!(WORKERS.load(Ordering::SeqCst), 0);
}

fn unbound_audio_fixture() -> (Session, Original, u64, Value) {
    use sha2::{Digest, Sha256};
    let mut entropy = [0u8; 16];
    getrandom::fill(&mut entropy).unwrap();
    let directory = std::env::temp_dir().join(format!("poema-source-audio-{:x}", u128::from_le_bytes(entropy)));
    std::fs::create_dir(&directory).unwrap();
    let directory = std::fs::canonicalize(directory).unwrap();
    let file = directory.join("original.wav");
    let bytes = filmcraft_export::write_wav(&vec![0.0; 960], 2, 48000, filmcraft_export::SampleFormat::Float32).unwrap();
    std::fs::write(&file, &bytes).unwrap();
    let mut s = setup();
    let item = crate::commands::import_streamed(&mut s, file.to_str().unwrap(), None).unwrap().0;
    let generation = json!({"kind":"poema-media-generation","version":1,"sha256":format!("{:x}",Sha256::digest(&bytes)),"bytes":bytes.len()});
    (s, Original(directory), item, generation)
}
fn source_audio_request(s: &mut Session, item: u64, generation: Value, n: usize) -> Value {
    let doc = read(s, n);
    json!({"token":token(n),"job":token(n+10000),"scope":{"projectId":"p_test","anchorSequence":doc["sequence"]["id"]},
        "background":true,"request":{"kind":"source_audio_layout","item":item,"generation":generation}})
}
fn wait_validation(s: &mut Session, job: &Value) -> Value {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let reply = s.execute("agent.validationStatus", json!({"job":job})).unwrap();
        if reply["status"] != "running" {
            return reply;
        }
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(2));
    }
}
#[test]
fn source_layout_reads_unbound_originals_without_mutating_history_and_requires_exact_generation() {
    let _guard = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let (mut s, _original, item, generation) = unbound_audio_fixture();
    let before = unchanged(&s);
    let request = source_audio_request(&mut s, item, generation.clone(), 41001);
    s.execute(START, request.clone()).unwrap();
    let reply = wait(&mut s, &request["job"]);
    assert_eq!(reply["status"], "complete", "{reply}");
    assert_eq!(reply["view"], "source_audio_layout");
    assert_eq!(reply["result"]["generation"], generation);
    assert!(reply["result"]["layout"].is_null(), "WAV has no MXF layout");
    let validation = json!({"token":request["token"],"inspection":request["job"],"job":token(51002)});
    s.execute("agent.validateSnapshot", validation.clone()).unwrap();
    assert_eq!(wait_validation(&mut s, &validation["job"])["status"], "complete");
    assert_eq!(unchanged(&s), before);
    let mut wrong = source_audio_request(&mut s, item, generation, 41002);
    wrong["request"]["generation"]["sha256"] = json!("0".repeat(64));
    s.execute(START, wrong.clone()).unwrap();
    assert_eq!(wait(&mut s, &wrong["job"])["status"], "failed");
    let mut unsafe_request = source_audio_request(&mut s, item, request["request"]["generation"].clone(), 41003);
    unsafe_request["background"] = json!(false);
    assert!(s.execute(START, unsafe_request).is_err());
    assert_eq!(unchanged(&s), before);
}
#[test]
fn source_layout_postflight_rechecks_unbound_files_and_cancelled_reads_cannot_qualify() {
    let _guard = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let (mut s, original, item, generation) = unbound_audio_fixture();
    let before = unchanged(&s);
    let request = source_audio_request(&mut s, item, generation, 42001);
    s.execute(START, request.clone()).unwrap();
    assert_eq!(wait(&mut s, &request["job"])["status"], "complete");
    s.agent_bridge.inspections.back_mut().unwrap().started = Instant::now() - Duration::from_secs(1);
    assert_eq!(s.execute("agent.inspection", json!({"job":request["job"]})).unwrap()["status"], "complete");
    let file = original.0.join("original.wav");
    let bytes = std::fs::read(&file).unwrap();
    let mut changed = bytes.clone();
    *changed.last_mut().unwrap() ^= 1;
    std::fs::write(&file, changed).unwrap();
    let postflight = json!({"token":request["token"],"inspection":request["job"],"job":token(52001)});
    s.execute("agent.validateSnapshot", postflight.clone()).unwrap();
    assert_eq!(wait_validation(&mut s, &postflight["job"])["status"], "failed");
    std::fs::write(&file, bytes).unwrap();
    s.execute("agent.cancelInspection", json!({"job":request["job"]})).unwrap();
    assert!(s.execute("agent.validateSnapshot", json!({"token":request["token"],"inspection":request["job"],"job":token(52002)})).is_err());
    assert_eq!(unchanged(&s), before);
}
#[test]
fn background_inspection_refuses_changed_native_basis_without_waiting_for_the_decoder() {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    for mode in ["cow", "path", "sequence"] {
        let mut s = fixture();
        let (_original, p, release) = blocked_background(&mut s, 29300);
        match mode {
            "cow" => s.project = Arc::new((*s.project).clone()),
            "path" => s.path = Some("/generated/another.fcproj".into()),
            _ => s.state.active_sequence = None,
        };
        let before = unchanged(&s);
        assert!(s.execute("agent.inspection", json!({"job":p["job"]})).is_err());
        assert_eq!(unchanged(&s), before);
        release.0.release();
        workers_finished();
    }
}
#[test]
fn background_inspection_retains_completed_output_until_its_job_deadline_and_still_requires_postflight() {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let mut s = fixture();
    let p = background(&mut s, 29400);
    assert_eq!(wait(&mut s, &p["job"])["status"], "complete");
    workers_finished();
    let before = unchanged(&s);
    s.agent_bridge.inspections.back_mut().unwrap().started = Instant::now() - Duration::from_secs(1);
    let out = s.execute("agent.inspection", json!({"job":p["job"]})).unwrap();
    assert_eq!(out["status"], "complete");
    assert_eq!(out["background"], true);
    assert_eq!(out["requiresPostflight"], true);
    assert!(out.get("result").is_some());
    assert_eq!(unchanged(&s), before);
    assert_eq!(s.execute("agent.receipt", json!({"token":p["job"]})).unwrap()["status"], "unknown");
}
#[test]
fn background_inspection_deadline_cancels_retained_output_and_revokes_its_postflight_basis() {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let mut s = fixture();
    let p = background(&mut s, 29401);
    assert_eq!(wait(&mut s, &p["job"])["status"], "complete");
    workers_finished();
    let before = unchanged(&s);
    s.agent_bridge.inspections.back_mut().unwrap().started = Instant::now() - DEADLINE - Duration::from_secs(1);
    let out = s.execute("agent.inspection", json!({"job":p["job"]})).unwrap();
    assert_eq!(out["status"], "cancelled");
    assert!(out.get("result").is_none());
    assert!(read_basis(&s, p["job"].as_str().unwrap(), p["token"].as_str().unwrap(), None).is_err());
    assert_eq!(unchanged(&s), before);
    assert_eq!(s.execute("agent.receipt", json!({"token":p["job"]})).unwrap()["status"], "unknown");
}
#[test]
fn background_inspection_hostile_flags_and_foreign_candidates_refuse_before_work() {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let mut s = fixture();
    let d = read(&mut s, 29500);
    let p = json!({"token":token(29500),"job":token(39500),"scope":{"projectId":"p_test","anchorSequence":d["sequence"]["id"]},"request":{"kind":"frame","frame":30,"maxSide":64},"background":true});
    let before = unchanged(&s);
    for flag in [Value::Null, json!("true"), json!(1)] {
        let mut bad = p.clone();
        bad["background"] = flag;
        assert!(s.execute(START, bad).is_err());
    }
    let mut bad = p;
    bad["candidate"] = json!(token(39502));
    assert!(s.execute(START, bad).is_err());
    assert_eq!(unchanged(&s), before);
    assert_eq!(WORKERS.load(Ordering::SeqCst), 0);
}
