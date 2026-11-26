//! Isolated cross-language test server. Generated demo only, no app window, credentials,
//! import/export/save operations or live workspace. Parent owns this short-lived process.
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::sync::Arc;
use std::time::Duration;

use filmcraft_engine::Session;
use serde::Deserialize;
use serde_json::{Value, json};

// Bounded lifetime for cold large-original read/restore/Undo/Redo acceptance.
const REQUEST_LIMIT: usize = 65_536;

// Explicit generated-file metadata only; never opens/imports/saves a workspace.
// The parent test owns these scratch files and the original Poema data owner.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct MappingFixture {
    project_id: String,
    camera_path: String,
    audio_path: String,
    #[serde(default)]
    linked_audio: bool,
    #[serde(default)]
    second_passage: bool,
    #[serde(default)]
    cross_passage_tail: bool,
}
fn prepare_mapping(session: &mut Session, sequence: filmcraft_project::ItemId, config: &MappingFixture) -> Result<(u64, u64), Box<dyn std::error::Error>> {
    use filmcraft_media::AudioStreamInfo;
    use filmcraft_project::MediaRef;
    use filmcraft_time::{FrameRate, TICKS_PER_SECOND, Tick};
    if config.project_id.is_empty()
        || config.project_id.len() > 256
        || !config.project_id.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'-')
        || [&config.camera_path, &config.audio_path].iter().any(|p| p.len() > 4096 || !std::path::Path::new(p).is_absolute() || p.chars().any(char::is_control))
    {
        return Err("invalid generated mapping fixture scope".into());
    }
    let project = Arc::make_mut(&mut session.project);
    let q = project.sequence(sequence).ok_or("missing generated sequence")?;
    let video = q.video_tracks.first().and_then(|t| t.items.first()).cloned().ok_or("missing generated picture")?;
    let mut audio = q.audio_tracks.first().and_then(|t| t.items.first()).cloned().ok_or("missing generated sound")?;
    if video.item == audio.item {
        let original = project.item(audio.item).cloned().ok_or("missing generated recorder prototype")?;
        audio.item = project.add_item("Generated recorder", original.label, original.kind, None);
    }
    let q = project.sequence_mut(sequence).ok_or("missing generated sequence")?;
    q.settings.frame_rate = FrameRate::new(25, 1);
    q.video_tracks.truncate(1);
    q.audio_tracks.truncate(1);
    q.caption_tracks.clear();
    q.markers.clear();
    for track in q.all_tracks_mut() {
        track.items.clear();
        track.transitions.clear();
        track.locked = false;
        track.enabled = true;
        track.muted = false;
        track.solo = false;
    }
    let mut v = video;
    v.start = Tick(5 * TICKS_PER_SECOND);
    v.duration = Tick(8 * TICKS_PER_SECOND);
    v.source_in = Tick(10 * TICKS_PER_SECOND);
    v.link = Some(900);
    let mut a = audio;
    a.start = Tick(4 * TICKS_PER_SECOND);
    a.duration = Tick(10 * TICKS_PER_SECOND);
    a.source_in = Tick(14 * TICKS_PER_SECOND);
    a.link = Some(900);
    a.audio_stream = 1;
    a.source_channels = vec![2];
    let ids = (v.item.0, a.item.0);
    q.video_tracks.first_mut().ok_or("missing generated video track")?.items.push(v);
    q.audio_tracks.first_mut().ok_or("missing generated audio track")?.items.push(a);
    let cam = project.item_mut(filmcraft_project::ItemId(ids.0)).and_then(|i| i.as_media_mut()).ok_or("missing camera")?;
    cam.media = MediaRef::File { path: config.camera_path.clone() };
    cam.offline = false;
    cam.info.duration = Tick(40 * TICKS_PER_SECOND);
    cam.info.video.as_mut().ok_or("missing generated video info")?.frame_rate = FrameRate::new(25, 1);
    let mic = project.item_mut(filmcraft_project::ItemId(ids.1)).and_then(|i| i.as_media_mut()).ok_or("missing recorder")?;
    mic.media = MediaRef::File { path: config.audio_path.clone() };
    mic.offline = false;
    mic.info.duration = Tick(45 * TICKS_PER_SECOND);
    mic.info.video = None;
    mic.info.audio_streams = vec![
        AudioStreamInfo { sample_rate: 48000, channels: 2, codec: "generated".into(), bits_per_sample: None },
        AudioStreamInfo { sample_rate: 96000, channels: 6, codec: "generated".into(), bits_per_sample: None },
    ];
    if config.linked_audio {
        let q = session.project.sequence(sequence).ok_or("missing generated linked sequence")?;
        let video_id = q.video_tracks.first().and_then(|t| t.items.first()).ok_or("missing generated linked picture")?.id.0;
        let audio_id = q.audio_tracks.first().and_then(|t| t.items.first()).ok_or("missing generated linked sound")?.id.0;
        session.execute("sequence.addTracks", json!({"video":0,"audio":1}))?;
        let placed = session.execute(
            "timeline.place",
            json!({"item":ids.1,"track":"A2","frame":125,"sourceIn":20*TICKS_PER_SECOND,
                "duration":8*TICKS_PER_SECOND,"video":false,"audio":true,"insert":false}),
        )?;
        let placed_ids: Vec<u64> = placed
            .get("clips")
            .and_then(Value::as_array)
            .ok_or("missing generated extra sound")?
            .iter()
            .map(|v| v.as_u64().ok_or("invalid generated extra sound"))
            .collect::<Result<_, _>>()?;
        let extra_id = *placed_ids.first().ok_or("missing generated extra sound")?;
        // Human placement expands every container stream. This setup intends
        // only stream zero beside the existing stream-one primary. Remove the
        // automatically placed duplicates through the native human command,
        // without following their placement link back to the intended member.
        if placed_ids.len() > 1 {
            if session.state.linked_selection {
                session.execute("sequence.linkedSelection", json!({}))?;
            }
            session.execute("edit.clear", json!({"clips":&placed_ids[1..]}))?;
        }
        let linked = session.execute("clip.link", json!({"clips":[video_id,audio_id,extra_id]}))?;
        if !linked.get("linked").and_then(Value::as_bool).ok_or("missing generated link result")? {
            session.execute("clip.link", json!({"clips":[video_id,audio_id,extra_id]}))?;
        }
    }
    if config.cross_passage_tail && !config.second_passage {
        return Err("cross-passage tail requires the generated second passage".into());
    }
    if config.second_passage {
        if !config.linked_audio {
            session.execute("sequence.addTracks", json!({"video":0,"audio":1}))?;
        }
        let placed = session.execute(
            "timeline.place",
            json!({"item":ids.0,"track":"V1","frame":375,
            "sourceIn":10*TICKS_PER_SECOND,"duration":8*TICKS_PER_SECOND,"video":true,"audio":false,"insert":false}),
        )?;
        let video = placed.get("clips").and_then(Value::as_array).and_then(|v| v.first()).and_then(Value::as_u64).ok_or("missing second generated picture")?;
        let placed = session.execute(
            "timeline.place",
            json!({"item":ids.1,"track":"A2","frame":350,
            "sourceIn":14*TICKS_PER_SECOND,"duration":10*TICKS_PER_SECOND,"video":false,"audio":true,"insert":false}),
        )?;
        let clips: Vec<u64> = placed
            .get("clips")
            .and_then(Value::as_array)
            .ok_or("missing second generated sound")?
            .iter()
            .map(|v| v.as_u64().ok_or("invalid second generated sound"))
            .collect::<Result<_, _>>()?;
        let audio = *clips.first().ok_or("missing second generated sound")?;
        if session.state.linked_selection {
            session.execute("sequence.linkedSelection", json!({}))?;
        }
        if let Some(extra) = clips.get(1..).filter(|v| !v.is_empty()) {
            session.execute("edit.clear", json!({"clips":extra}))?;
        }
        let q = Arc::make_mut(&mut session.project).sequence_mut(sequence).ok_or("missing generated second sequence")?;
        for track in &mut q.audio_tracks {
            for c in &mut track.items {
                if c.id.0 == audio {
                    c.audio_stream = 1;
                    c.source_channels = vec![2];
                }
            }
        }
        if config.cross_passage_tail {
            q.audio_tracks.first_mut().and_then(|t| t.items.first_mut()).ok_or("missing primary generated sound")?.duration = Tick(26 * TICKS_PER_SECOND);
        }
        session.execute("clip.link", json!({"clips":[video,audio]}))?;
    }
    session.execute("poema.trackSequence", json!({}))?;
    session.state.linked_selection = false;
    session.history.undo.clear();
    session.history.redo.clear();
    session.journal.clear();
    session.events.clear();
    Ok(ids)
}

// Optional generated large-timeline acceptance mode. The default remains the
// original read-only inspection fixture. This never opens or saves a real film.
fn prepare_timeline(session: &mut Session, sequence: filmcraft_project::ItemId) -> Result<(), Box<dyn std::error::Error>> {
    let item = session
        .project
        .items
        .values()
        .find(|i| i.as_media().is_some_and(|m| m.info.has_video() && m.info.has_audio()))
        .ok_or("generated A/V source missing")?
        .id;
    let q = Arc::make_mut(&mut session.project).sequence_mut(sequence).ok_or("generated sequence missing")?;
    q.video_tracks.truncate(1);
    q.audio_tracks.truncate(1);
    q.caption_tracks.clear();
    q.markers.clear();
    for track in q.all_tracks_mut() {
        track.items.clear();
        track.transitions.clear();
        track.locked = false;
        track.sync_lock = true;
    }
    let duration = session.sequence_rate().tick_of(12).0;
    for i in 0..130 {
        let mut clips = Vec::new();
        for (track, video) in [("V1", true), ("A1", false)] {
            let placed = session.execute(
                "timeline.place",
                json!({"item":item.0,"track":track,"frame":i*24,"sourceIn":0,"duration":duration,"video":video,"audio":!video,"insert":false}),
            )?;
            let clip =
                placed.get("clips").and_then(Value::as_array).and_then(|a| a.first()).and_then(Value::as_u64).ok_or("generated placement missing clip")?;
            clips.push(clip);
        }
        session.execute("clip.link", json!({"clips":clips}))?;
        // Fixture construction is not the history under test. Avoid retaining
        // hundreds of synthetic setup snapshots; agent/human edits below are real.
        session.history.undo.clear();
        session.history.redo.clear();
        session.journal.clear();
        session.events.clear();
    }
    session.execute("poema.trackSequence", json!({}))?;
    session.state.linked_selection = true;
    session.history.undo.clear();
    session.history.redo.clear();
    session.journal.clear();
    session.events.clear();
    Ok(())
}

// Short adjacent generated clips with original source handles for finishing.
// Unlike the large paging fixture, these retain the demo's actual edit geometry.
fn prepare_finish_workflow(session: &mut Session, sequence: filmcraft_project::ItemId) -> Result<(), Box<dyn std::error::Error>> {
    let q = Arc::make_mut(&mut session.project).sequence_mut(sequence).ok_or("missing finishing sequence")?;
    q.video_tracks.truncate(1);
    q.audio_tracks.truncate(1);
    q.caption_tracks.clear();
    q.markers.clear();
    for track in q.all_tracks_mut() {
        track.items.truncate(2);
        track.transitions.clear();
        track.locked = false;
    }
    session.execute("poema.trackSequence", json!({}))?;
    session.execute("poema.trackEffects", json!({}))?;
    session.state.linked_selection = true;
    session.history.undo.clear();
    session.history.redo.clear();
    session.journal.clear();
    session.events.clear();
    Ok(())
}
pub fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Explicit parent-owned generated-file mode, never a normal workspace.
    // Only this mode can save/reopen or create private history companions.
    let mapping_history = std::env::args().skip(1).any(|arg| arg == "--mapping-history-fixture");
    let identity_recovery = std::env::args().skip(1).any(|arg| arg == "--identity-recovery-fixture");
    let update_fixture = std::env::args().skip(1).any(|arg| arg == "--workspace-update-fixture");
    let history_reopen = std::env::args().skip(1).any(|arg| arg == "--history-reopen");
    let history_fixture =
        history_reopen || mapping_history || update_fixture || identity_recovery || std::env::args().skip(1).any(|arg| arg == "--history-fixture");
    let history_path = if history_fixture {
        let root = std::path::PathBuf::from(std::env::var("POEMA_NATIVE_HISTORY_DIRECTORY")?);
        let marker = root.join("generated-fixture.txt");
        if !root.is_absolute()
            || std::fs::canonicalize(&root)? != root
            || root.file_name().and_then(|s| s.to_str()).is_none_or(|s| !s.starts_with("poema-native-history-"))
            || !std::fs::symlink_metadata(&marker)?.is_file()
            || std::fs::metadata(&marker)?.len() > 128
            || std::fs::read_to_string(marker)? != "poema-generated-history-fixture-v1\n"
        {
            return Err("history fixture requires an explicit unredirected parent-owned generated directory".into());
        }
        Some(root.join("generated.fcproj"))
    } else {
        None
    };
    let finishing_fixture = std::env::args().skip(1).any(|arg| arg == "--finishing-fixture");
    let finish_workflow = std::env::args().skip(1).any(|arg| arg == "--finish-workflow-fixture");
    let timeline_fixture =
        history_fixture && !mapping_history || finishing_fixture || finish_workflow || std::env::args().skip(1).any(|arg| arg == "--timeline-fixture");
    let mapping_fixture = mapping_history || std::env::args().skip(1).any(|arg| arg == "--mapping-fixture");
    if timeline_fixture && mapping_fixture {
        return Err("choose one generated fixture mode".into());
    }
    let mapping = if mapping_fixture {
        let text = std::env::var("POEMA_NATIVE_MAPPING_FIXTURE")?;
        if text.len() > 16384 {
            return Err("generated mapping fixture exceeds its bound".into());
        }
        Some(serde_json::from_str::<MappingFixture>(&text)?)
    } else {
        None
    };
    let mut session = Session::default();
    if update_fixture || identity_recovery {
        if update_fixture && identity_recovery {
            return Err("choose one generated startup mode".into());
        }
        let raw = std::env::var(if identity_recovery { "POEMA_NATIVE_IDENTITY_RECOVERY" } else { "POEMA_NATIVE_WORKSPACE_UPDATE" })?;
        if raw.len() > 16384 {
            return Err("generated update descriptor exceeds its bound".into());
        }
        let descriptor: Value = serde_json::from_str(&raw)?;
        let path = history_path.as_ref().ok_or("generated update root missing")?;
        if descriptor["projectPath"].as_str() != path.to_str() || descriptor["scope"]["projectId"] != "p_inspection_fixture" {
            return Err("generated update descriptor must target its marked fixture".into());
        }
        let recovery = if identity_recovery {
            Some(session.open_identity_publication(&descriptor)?)
        } else {
            session.open_workspace_update(&descriptor)?;
            None
        };
        // Exercise the production launcher's fixed port and deferred persistence
        // without opening a GUI. These options remain confined to this marked,
        // generated-project fixture; normal inspection cannot use them.
        let fixed = std::env::var("POEMA_NATIVE_FIXTURE_UPDATE_PORT").ok();
        let data = std::env::var("POEMA_NATIVE_FIXTURE_UPDATE_DATA").ok();
        let port = match (fixed, data) {
            (None, None) => 0,
            (Some(port), Some(data)) => {
                let port = port.parse::<u16>()?;
                let data = std::path::PathBuf::from(data);
                let root = path.parent().ok_or("generated update root missing")?;
                if port < 1024 || port == 4519 || !data.is_absolute() || !data.starts_with(root) || data == root {
                    return Err("fixed update transport/persistence must stay inside its generated fixture".into());
                }
                session.configure_workspace_update_persistence(filmcraft_engine::autosave::AutosaveConfig::new(data))?;
                port
            }
            _ => return Err("fixed generated update transport and persistence must be supplied together".into()),
        };
        let sequence = session.state.active_sequence.ok_or("generated candidate sequence missing")?;
        let listener = TcpListener::bind(("127.0.0.1", port))?;
        let mut ready = json!({"port":listener.local_addr()?.port(),"projectId":"p_inspection_fixture","sequenceId":sequence.0,"projectPath":session.path});
        if let Some(recovery) = recovery {
            ready["identityRecovery"] = recovery;
        }
        println!("{}", ready);
        std::io::stdout().flush()?;
        return cancellable_fixture(listener, &mut session, true, true, false, true, port != 0);
    }
    session.execute("file.openDemoProject", json!({}))?;
    let sequence = session.state.active_sequence.ok_or("fixture sequence missing")?;
    let mapping_items = mapping.as_ref().map(|config| prepare_mapping(&mut session, sequence, config)).transpose()?;
    let project_id = mapping.as_ref().map_or("p_inspection_fixture", |c| c.project_id.as_str());
    for item in Arc::make_mut(&mut session.project).items.values_mut().filter(|i| i.as_media().is_some()) {
        item.metadata.insert(
            "poema.binding.v1".into(),
            json!({"projectId":project_id,"assetId":if mapping_items.is_some_and(|(v,_)| v == item.id.0) { "a_camera".into() }
                else if mapping_items.is_some_and(|(_,a)| a == item.id.0) { "a_recorder".into() } else { format!("a_{}", item.id.0) },
            "mediaId":"original","role":"original"})
            .to_string(),
        );
    }
    if finish_workflow {
        prepare_finish_workflow(&mut session, sequence)?;
    } else if timeline_fixture {
        prepare_timeline(&mut session, sequence)?;
    }
    if finishing_fixture {
        let clip = session.active_sequence().and_then(|q| q.video_tracks.first()).and_then(|t| t.items.first()).ok_or("missing generated finishing clip")?.id.0;
        for _ in 0..2 {
            session.execute("effects.apply", json!({"clips":[clip],"effect":"gaussian_blur"}))?;
        }
        session.execute("effects.setParam", json!({"clip":clip,"effect":0,"param":"blurriness","value":17}))?;
        session.execute("effects.setParam", json!({"clip":clip,"effect":1,"param":"blurriness","value":42}))?;
        let time = session.sequence_rate().tick_of(5).0;
        session.execute("effects.setParam", json!({"clip":clip,"effect":"motion","param":"scale","value":50,"keyframe":true,"time":0}))?;
        session.execute("effects.setParam", json!({"clip":clip,"effect":"motion","param":"scale","value":150,"keyframe":true,"time":time}))?;
        session.execute("masks.add", json!({"clip":clip,"effect":"opacity","shape":"rectangle","center":[32,24],"size":[24,16]}))?;
        session.execute("masks.set", json!({"clip":clip,"effect":"opacity","mask":0,"name":"Human foreground","inverted":true,"opacity":65,"feather":7}))?;
        session.execute("poema.trackEffects", json!({}))?;
    }
    let admission_item = if std::env::args().skip(1).any(|arg| arg == "--admission-fixture") {
        let root = history_path.as_ref().and_then(|p| p.parent()).ok_or("admission fixture requires generated history mode")?;
        let file = std::path::PathBuf::from(std::env::var("POEMA_NATIVE_ADMISSION_FILE")?);
        if history_reopen
            || !file.is_absolute()
            || std::fs::canonicalize(&file)? != file
            || !file.starts_with(root)
            || !std::fs::symlink_metadata(&file)?.is_file()
            || std::fs::metadata(&file)?.len() > 4 * 1024 * 1024
        {
            return Err("admission fixture requires a small generated original inside its marked directory".into());
        }
        let item = filmcraft_engine::commands::import_streamed(&mut session, file.to_str().ok_or("generated path must be UTF-8")?, None)?.0;
        // prepare_timeline replaces the demo clips. Begin this acceptance with
        // a valid human clip selection so saved reopen can preserve that view.
        session.fix_state();
        if let Some(clip) = session.active_sequence().and_then(|q| q.video_tracks.first()).and_then(|t| t.items.first()).map(|c| c.id) {
            session.state.selection = vec![clip];
        }
        Some(item)
    } else {
        None
    };
    // Exercise the native six-channel extensible float-WAV path as well as the stereo
    // scripted contract fixtures in Poema; no test-only alternate audio encoder.
    Arc::make_mut(&mut session.project).sequence_mut(sequence).ok_or("fixture sequence missing")?.settings.audio_master =
        filmcraft_project::AudioChannels::Surround51;
    session.path = Some("/fixture/inspection.fcproj".into());
    if let Some(path) = history_path {
        let path = path.to_str().ok_or("generated history path must be UTF-8")?;
        if history_reopen {
            if !std::fs::symlink_metadata(path)?.is_file() || std::fs::canonicalize(path)? != std::path::Path::new(path) {
                return Err("history fixture may reopen only its generated regular project".into());
            }
            session.execute("file.open", json!({"path":path}))?;
        } else {
            if std::fs::symlink_metadata(path).is_ok() {
                return Err("generated project already exists; nothing overwritten".into());
            }
            session.path = Some(path.into());
            session.execute("file.save", json!({}))?;
        }
    }
    // Recovery acceptance uses the real native journal, inside this already
    // validated parent-owned fixture only. No production precondition is relaxed.
    if std::env::args().skip(1).any(|arg| arg == "--save-recovery-fixture") {
        if !history_fixture || update_fixture || identity_recovery {
            return Err("Save recovery fixture requires ordinary generated history mode".into());
        }
        let root = session.path.as_ref().and_then(|p| std::path::Path::new(p).parent()).ok_or("generated recovery root missing")?;
        let data = root.join(format!("save-recovery-native-data-{}", std::process::id()));
        std::fs::create_dir(&data)?;
        session.start_autosave(filmcraft_engine::autosave::AutosaveConfig::new(data))?;
    }
    let sequence = session.state.active_sequence.ok_or("opened generated sequence missing")?;
    if let Ok(raw) = std::env::var("POEMA_NATIVE_SAVE_STARTUP_ROOT") {
        let root = session.path.as_ref().and_then(|p| std::path::Path::new(p).parent()).ok_or("generated history root required for Save startup")?;
        let manifest = std::path::Path::new(&raw);
        if !history_reopen || !history_fixture || !manifest.starts_with(root) || manifest.parent() != Some(root) {
            return Err("Save startup only accepts original controls in the marked generated reopen root".into());
        }
        #[cfg(any(target_vendor = "apple", target_os = "linux"))]
        session.install_managed_save_root(manifest)?;
        #[cfg(not(any(target_vendor = "apple", target_os = "linux")))]
        return Err("generated managed Save unavailable on this platform".into());
    }
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let mut ready = json!({"port":listener.local_addr()?.port(),"projectId":project_id,"sequenceId":sequence.0,"projectPath":session.path});
    if let Some(item) = admission_item {
        ready["admissionItem"] = json!(item);
    }
    println!("{ready}");
    std::io::stdout().flush()?;
    let writer_fixture = std::env::args().skip(1).any(|arg| arg == "--writer-fixture") || std::env::var("POEMA_NATIVE_REQUIRE_WRITER").as_deref() == Ok("1");
    if writer_fixture || std::env::args().skip(1).any(|arg| arg == "--cancel-fixture") {
        return cancellable_fixture(listener, &mut session, history_fixture, timeline_fixture, mapping_fixture, writer_fixture, false);
    }
    // Multi-Send correction/inspection acceptance needs more than one short
    // operation's requests. Still bounded; the test parent owns termination.
    for stream in listener.incoming().take(REQUEST_LIMIT) {
        let mut stream = stream?;
        stream.set_read_timeout(Some(Duration::from_secs(5)))?;
        let mut line = String::new();
        BufReader::new((&stream).take(4 * 1024 * 1024 + 1)).read_line(&mut line)?;
        if line.len() > 4 * 1024 * 1024 {
            return Err("fixture request too large".into());
        }
        let request: Value = serde_json::from_str(&line)?;
        let reply = execute_fixture_request(&mut session, &request, history_fixture, timeline_fixture, mapping_fixture);
        writeln!(stream, "{reply}")?;
    }
    Ok(())
}

fn execute_fixture_request(session: &mut Session, request: &Value, history_fixture: bool, timeline_fixture: bool, mapping_fixture: bool) -> Value {
    let command = request["params"]["command"].as_str().unwrap_or("");
    if !export_fixture_path(session, request) {
        return json!({"id":request["id"],"ok":false,"error":"export fixture paths must stay inside the generated project's Exports directory","outcome":"rejected"});
    }
    let allowed = request["method"] == "engine.execute"
        && (matches!(
            command,
            "agent.snapshot"
                | "agent.snapshotStatus"
                | "agent.cancelSnapshot"
                | "agent.inspect"
                | "agent.inspection"
                | "agent.cancelInspection"
                | "agent.validateSnapshot"
                | "agent.readBasis"
                | "agent.readSourceCatalog"
                | "agent.readSourceSelection"
                | "agent.projectDocument"
                | "agent.qualifiedDocument"
                | "agent.prepareApply"
                | "agent.prepareSourceAdmission"
                | "agent.sourceAdmissionPreparation"
                | "agent.cancelSourceAdmissionPreparation"
                | "agent.validationStatus"
                | "agent.cancelValidation"
                | "agent.sourceFrames"
                | "agent.readFinishing"
                | "agent.readMusicDestination"
                | "agent.readCaptions"
                | "agent.readMarkers"
                | "agent.readTracks"
                | "agent.readTrackInserts"
                | "agent.readMixerAutomation"
                | "agent.readBins"
                | "agent.readSearchBins"
                | "agent.readEffectCatalog"
                | "agent.readEffectStack"
                | "agent.readGraphics"
                | "agent.planProjectSave"
                | "agent.beginProjectSaveOperation"
                | "agent.projectSaveOperationStatus"
                | "agent.cancelProjectSaveOperation"
                | "agent.dispatchProjectSaveOperation"
                | "agent.prepareProjectSaveAdmission"
                | "agent.readProjectSaveAdmission"
                | "agent.readProjectSaveEffects"
                | "agent.prepareProjectSaveRecovery"
                | "agent.prepareProjectSaveRecoveryRecord"
                | "agent.readProjectSaveRecoveryRecord"
                | "agent.publishProjectSaveRecoveryRecord"
                | "agent.startProjectSavePlan"
                | "agent.projectSavePlanStatus"
                | "agent.cancelProjectSavePlan"
                | "agent.readFontCatalog"
                | "agent.readFontsUsed"
                | "agent.readCaptionFontResources"
                | "agent.planProjectFontReplacement"
                | "agent.planProjectFontResources"
                | "agent.readProjectFontResources"
                | "agent.startFontResources"
                | "agent.fontResourcesStatus"
                | "agent.cancelFontResources"
                | "agent.planGraphicsFonts"
                | "agent.readGraphicsLayout"
                | "agent.readCaptionDestination"
                | "agent.condenseInputs"
                | "agent.pictureInputs"
                | "agent.reactionReferenceInputs"
                | "agent.sourceEventInputs"
                | "agent.exportOptions"
                | "agent.resolveExport"
                | "agent.exportStatus"
                | "agent.cancelExport"
                | "agent.exportReceipt"
                | "agent.prepareExportDelivery"
        ) || history_fixture
            && matches!(
                command,
                "agent.saveHistoryCheckpoint"
                    | "agent.readProject"
                    | "agent.validateProjectRead"
                    | "agent.readProjectSequenceScope"
                    | "agent.selectProjectSequence"
                    | "sequence.open"
                    | "sequence.close"
                    | "project.inspect"
                    | "state.inspect"
                    | "history.list"
                    | "agent.restoreHistoryCheckpoint"
                    | "agent.validateRetainedHistory"
                    | "agent.inspectPausedWorkspaceHistory"
                    | "agent.inspectHistoryCheckpoint"
                    | "agent.inspectIdentityMigration"
                    | "agent.publishIdentityMigration"
                    | "agent.readIdentityPublication"
                    | "agent.readHistoryCheckpointBasis"
                    | "agent.selectSequence"
                    | "file.save"
                    | "agent.beginWorkspaceUpdate"
                    | "agent.workspaceUpdateStatus"
                    | "agent.cancelWorkspaceUpdate"
                    | "agent.stageWorkspaceUpdate"
                    | "agent.activateWorkspaceUpdate"
                    | "agent.retireWorkspaceUpdate"
                    | "presets.list"
            )
            || (timeline_fixture || mapping_fixture)
                && matches!(
                    command,
                    "agent.apply"
                        | "agent.beginSequencePayload"
                        | "agent.appendSequencePayload"
                        | "agent.sequencePayloadStatus"
                        | "agent.discardSequencePayload"
                        | "agent.readSequenceAssembly"
                        | "agent.startExport"
                        | "agent.publishExport"
                        | "file.exportMedia"
                        | "agent.prepareCondense"
                        | "agent.commitCondense"
                        | "agent.discardCondense"
                        | "agent.undo"
                        | "agent.redo"
                        | "agent.readHistory"
                        | "agent.receipt"
                        | "timeline.trim"
                        | "timeline.razor"
                        | "timeline.place"
                        | "markers.add"
                        | "markers.addRange"
                        | "markers.addChapter"
                        | "markers.addFlashCue"
                        | "markers.edit"
                        | "markers.remove"
                        | "source.open"
                        | "project.setMarks"
                        | "file.newBin"
                        | "file.newBinFromSelection"
                        | "file.newSearchBin"
                        | "project.editSearchBin"
                        | "project.deleteSearchBin"
                        | "project.searchBinItems"
                        | "project.renameBin"
                        | "project.moveToBin"
                        | "prefs.set"
                        | "clip.replaceFromBin"
                        | "clip.makeSubclip"
                        | "source.setPlayhead"
                        | "timeline.move"
                        | "timeline.roll"
                        | "timeline.slip"
                        | "timeline.slide"
                        | "timeline.select"
                        | "project.select"
                        | "timeline.setTargeting"
                        | "timeline.setTrack"
                        | "sequence.addTracks"
                        | "sequence.removeTracks"
                        | "sequence.reorderTrack"
                        | "mixer.addSubmix"
                        | "mixer.setStrip"
                        | "mixer.addSend"
                        | "mixer.setKeyframe"
                        | "captions.importData"
                        | "captions.newTrack"
                        | "captions.add"
                        | "captions.setTrack"
                        | "sequence.extract"
                        | "sequence.lift"
                        | "markers.markIn"
                        | "markers.markOut"
                        | "playhead.set"
                        | "edit.copy"
                        | "edit.pasteInsert"
                        | "clip.speedDuration"
                        | "clip.frameHoldOptions"
                        | "clip.audioGain"
                        | "clip.enable"
                        | "clip.audioChannels"
                        | "clip.fitToFrame"
                        | "sequence.applyVideoTransition"
                        | "sequence.applyAudioTransition"
                        | "effects.setParam"
                        | "effects.apply"
                        | "effects.remove"
                        | "effects.toggleEnabled"
                        | "effects.reorder"
                        | "graphics.newText"
                        | "graphics.upgradeToSourceGraphic"
                        | "graphics.newShape"
                        | "graphics.set"
                        | "graphics.setCharStyle"
                        | "graphics.pin"
                        | "graphics.align"
                        | "graphics.distribute"
                        | "graphics.deleteLayer"
                        | "graphics.arrangeLayer"
                        | "masks.set"
                        | "masks.add"
                        | "poema.annotateClips"
                        | "poema.trackEffects"
                        | "poema.protectClips"
                        | "sequence.linkedSelection"
                        | "edit.rippleDelete"
                        | "edit.undo"
                        | "edit.redo"
                ));
    let result = if allowed {
        session.execute(command, request["params"]["params"].clone()).map_err(|e| e.to_string())
    } else {
        Err("fixture supports only scoped inspection commands".into())
    };
    match result {
        Ok(result) => json!({"id":request["id"],"ok":true,"result":result}),
        Err(error) => json!({"id":request["id"],"ok":false,"error":error}),
    }
}

// Test-only file commands can touch only this explicitly marked generated project root.
// No production project, arbitrary destination or model raw-command escape is introduced.
fn export_fixture_path(session: &Session, request: &Value) -> bool {
    let command = request["params"]["command"].as_str().unwrap_or("");
    if !matches!(command, "agent.startExport" | "agent.exportReceipt" | "file.exportMedia") {
        return true;
    }
    let Some(parent) = session.path.as_deref().and_then(|p| std::path::Path::new(p).parent()) else {
        return false;
    };
    let root = parent.join("Exports");
    let params = &request["params"]["params"];
    if command == "file.exportMedia" {
        return params["path"].as_str().is_some_and(|path| {
            let path = std::path::Path::new(path);
            path.parent() == Some(root.as_path())
                && path.file_name().and_then(|s| s.to_str()).is_some_and(|n| n.starts_with("reference-") && !n.chars().any(char::is_control))
        });
    }
    params["root"].as_str().is_some_and(|p| std::path::Path::new(p) == root)
}

// Optional generated transport acceptance. Only this mode advertises an
// independent cancellation door; the original fixture remains serial.
fn cancellable_fixture(
    listener: TcpListener,
    session: &mut Session,
    history: bool,
    timeline: bool,
    mapping: bool,
    writer: bool,
    close_candidate: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    use std::sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        mpsc,
    };
    struct Slot(Arc<AtomicUsize>);
    impl Drop for Slot {
        fn drop(&mut self) {
            self.0.fetch_sub(1, Ordering::SeqCst);
        }
    }
    let (tx, rx) = mpsc::sync_channel::<(Value, mpsc::Sender<Value>)>(16);
    let operations = session.agent_operation_control();
    let retirement_delivered = Arc::new(AtomicBool::new(false));
    let retired = retirement_delivered.clone();
    std::thread::Builder::new().name("generated-cancel-listener".into()).spawn(move || {
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            operations.enable_transport();
            let open = Arc::new(AtomicUsize::new(0));
            for mut stream in listener.incoming().take(REQUEST_LIMIT).flatten() {
                if open.fetch_add(1, Ordering::SeqCst) >= 8 {
                    open.fetch_sub(1, Ordering::SeqCst);
                    let _ = writeln!(stream, "{}", json!({"ok":false,"error":"generated fixture busy"}));
                    continue;
                }
                let slot = Slot(open.clone());
                let tx = tx.clone();
                let operations = operations.clone();
                let retired = retired.clone();
                let _ = std::thread::Builder::new().name("generated-cancel-connection".into()).spawn(move || {
                    let _slot = slot;
                    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        let handle = || -> Result<Value, Box<dyn std::error::Error>> {
                            stream.set_read_timeout(Some(Duration::from_secs(5)))?;
                            let mut line = String::new();
                            BufReader::new((&stream).take(4 * 1024 * 1024 + 1)).read_line(&mut line)?;
                            if line.len() > 4 * 1024 * 1024 {
                                return Err("generated request too large".into());
                            }
                            let request: Value = serde_json::from_str(&line)?;
                            if request["method"] == "engine.cancelSaveOperationWait" {
                                return Ok(match operations.cancel_save_wait(&request["params"]) {
                                    Ok(result) => json!({"id":request["id"],"ok":true,"result":result}),
                                    Err(e) => json!({"id":request["id"],"ok":false,"error":e.to_string()}),
                                });
                            }
                            if request["method"] == "engine.cancelOperation" {
                                return Ok(match operations.cancel(&request["params"]) {
                                    Ok(result) => json!({"id":request["id"],"ok":true,"result":result}),
                                    Err(e) => json!({"id":request["id"],"ok":false,"error":e.to_string()}),
                                });
                            }
                            let wait = Duration::from_millis(filmcraft_engine::session_worker::control_reply_wait_ms(
                                request["method"].as_str().unwrap_or(""),
                                &request["params"],
                            ));
                            let (reply_tx, reply_rx) = mpsc::channel();
                            tx.send((request, reply_tx))?;
                            Ok(reply_rx.recv_timeout(wait)?)
                        };
                        if let Ok(reply) = handle()
                            && writeln!(stream, "{reply}").and_then(|_| stream.flush()).is_ok()
                            && (matches!(reply["result"]["status"].as_str(), Some("retiring" | "fixture_closed"))
                                || close_candidate && reply["result"]["status"] == "candidate_cancelled")
                        {
                            retired.store(true, Ordering::Release);
                        }
                    }));
                });
            }
        }));
    })?;
    if !writer {
        for (request, reply) in rx.iter().take(REQUEST_LIMIT) {
            let _ = reply.send(execute_fixture_request(session, &request, history, timeline, mapping));
        }
        return Ok(());
    }
    use filmcraft_engine::session_worker::{Pending, SessionWorker, supports};
    let worker = SessionWorker::new()?;
    worker.install(session)?;
    let mut pending: Option<(Pending, mpsc::Sender<Value>, Value)> = None;
    let mut save_cancel: Option<(Value, mpsc::Sender<Value>, Value)> = None;
    let mut received = 0usize;
    loop {
        if retirement_delivered.load(Ordering::Acquire) {
            session.shutdown();
            break;
        }
        if let Some((work, reply, id)) = pending.as_mut()
            && let Some(done) = work.try_finish()
        {
            *session = done.session;
            let mut envelope = done.reply.envelope();
            envelope["id"] = id.clone();
            let _ = reply.send(envelope);
            pending = None;
            if let Some((params, reply, id)) = save_cancel.take() {
                match worker.start(session, "agent.cancelProjectSaveOperation", params, None) {
                    Ok(work) => pending = Some((work, reply, id)),
                    Err(e) => {
                        let _ = reply.send(json!({"id":id,"ok":false,"error":e.to_string(),"outcome":"rejected"}));
                    }
                }
            }
        }
        if received >= REQUEST_LIMIT && pending.is_none() {
            break;
        }
        let timeout = Duration::from_millis(if pending.is_some() { 1 } else { 50 });
        let (request, reply) = match rx.recv_timeout(timeout) {
            Ok(request) => request,
            Err(mpsc::RecvTimeoutError::Timeout) => continue,
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        };
        received = received.saturating_add(1);
        if request["method"] == "fixture.writerStatus" {
            // Generated-test diagnostics can inspect the completed operation's
            // counters as well as a running writer. This never supplies a ticket.
            let progress = pending.as_ref().map(|(work, _, _)| work.progress()).or_else(|| {
                request["params"]["token"]
                    .as_str()
                    .filter(|token| (32..=128).contains(&token.len()) && token.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_'))
                    .map(|token| session.agent_operation_control().progress(token))
            });
            let _ = reply.send(json!({"id":request["id"],"ok":true,"result":{"active":pending.is_some(),"displayOnly":session.is_display_only(),"progress":progress,"headlessFixture":true}}));
            continue;
        }
        if let Some((work, _, _)) = &pending {
            if request["method"] == "engine.execute"
                && request["params"]["command"] == "agent.cancelProjectSaveOperation"
                && work.matches_save_operation(&request["params"]["params"])
                && save_cancel.is_none()
            {
                let _ = work.cancel();
                save_cancel = Some((request["params"]["params"].clone(), reply, request["id"].clone()));
                continue;
            }
        }
        if pending.is_some() {
            let _ = reply.send(json!({"id":request["id"],"ok":false,"error":"native write in progress; no command queued","outcome":"rejected"}));
            continue;
        }
        if request["method"] == "fixture.close"
            && history
            && request["params"]["projectId"] == "p_inspection_fixture"
            && request["params"]["projectPath"].as_str() == session.path.as_deref()
            && session.path.as_deref().is_some_and(|p| std::path::Path::new(p).file_name().is_some_and(|n| n == "generated.fcproj"))
        {
            session.shutdown();
            let _ = reply.send(json!({"id":request["id"],"ok":true,"result":{"status":"fixture_closed"}}));
            continue;
        }
        let command = request["params"]["command"].as_str().unwrap_or("");
        if !export_fixture_path(session, &request) {
            let _ = reply.send(json!({"id":request["id"],"ok":false,"error":"export fixture root refused","outcome":"rejected"}));
            continue;
        }
        if (timeline || mapping) && request["method"] == "engine.execute" && supports(command) {
            match worker.start(session, command, request["params"]["params"].clone(), None) {
                Ok(work) => pending = Some((work, reply, request["id"].clone())),
                Err(error) => {
                    let _ = reply.send(json!({"id":request["id"],"ok":false,"error":error.to_string(),"outcome":"rejected"}));
                }
            }
        } else {
            let _ = reply.send(execute_fixture_request(session, &request, history, timeline, mapping));
        }
    }
    Ok(())
}
