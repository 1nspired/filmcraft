use super::*;
use std::{
    fs,
    sync::Condvar,
    time::{Duration, Instant},
};

fn token(n: usize) -> String {
    format!("native-worker-test-{n:032}")
}
fn fixture(worker: &SessionWorker, n: usize) -> (Session, Value) {
    let mut s = Session::default();
    s.execute("file.openDemoProject", json!({})).unwrap();
    s.execute("file.newSequence", json!({"name":"Native worker fixture","fps":24})).unwrap();
    let source = s.project.items.values().find(|i| i.as_media().is_some_and(|m| m.info.has_video() && m.info.has_audio())).unwrap().id;
    s.execute("agent.bindSource", json!({"item":source.0,"binding":{"projectId":"p_worker","assetId":"a_worker","mediaId":"m_original","role":"original"}}))
        .unwrap();
    let mut clips = Vec::new();
    for (track, video) in [("V1", true), ("A1", false)] {
        let result = s.execute("timeline.place", json!({"item":source.0,"track":track,"frame":0,"sourceIn":0,"duration":s.sequence_rate().tick_of(12).0,"video":video,"audio":!video,"insert":false})).unwrap();
        clips.push(result["clips"][0].as_u64().unwrap());
    }
    s.execute("clip.link", json!({"clips":clips})).unwrap();
    s.execute("poema.trackSequence", json!({})).unwrap();
    worker.install(&mut s).unwrap();
    s.agent_operation_control().enable_transport();
    let read = s.execute("agent.snapshot", json!({"token":token(n)})).unwrap();
    assert_eq!(read["capabilities"]["backgroundPublication"], true);
    let params = json!({"token":token(n),"label":"Worker exact trim","exact":true,
        "operations":[{"command":"timeline.trim","params":{"clip":clips[0],"edge":"out","mode":"regular","deltaFrames":-1}}]});
    (s, params)
}
fn finish(pending: &mut Pending) -> Completion {
    let end = Instant::now() + Duration::from_secs(10);
    loop {
        if let Some(result) = pending.try_finish() {
            return result;
        }
        assert!(Instant::now() < end, "native worker did not return its owner");
        std::thread::sleep(Duration::from_millis(1));
    }
}
struct Release(Arc<(Mutex<bool>, Condvar)>);
impl Drop for Release {
    fn drop(&mut self) {
        *self.0.0.lock().unwrap() = true;
        self.0.1.notify_all();
    }
}
fn blocked() -> (SessionWorker, Receiver<()>, Release) {
    let gate = Arc::new((Mutex::new(false), Condvar::new()));
    let held = gate.clone();
    let (entered, receiver) = mpsc::channel();
    let worker = SessionWorker::spawn(move |s, command, params| {
        entered.send(()).unwrap();
        let mut ready = held.0.lock().unwrap();
        while !*ready {
            ready = held.1.wait(ready).unwrap();
        }
        s.execute(command, params)
    })
    .unwrap();
    (worker, receiver, Release(gate))
}
fn state(s: &Session) -> Value {
    json!({"project":s.project,"state":s.state,"revision":s.revision,"undo":s.history.undo,"redo":s.history.redo,"historyLimit":s.history.limit,"mergeKey":s.history.merge_key,"journal":s.journal,"events":s.events})
}

#[test]
fn recovery_checkpoint_work_uses_the_exclusive_worker_and_stop_precedes_file_publication() {
    let (worker, entered, release) = blocked();
    let (mut s, _) = fixture(&worker, 60);
    let directory = crate::temp_dir().join(format!("native-worker-checkpoint-{}", std::process::id()));
    fs::create_dir(&directory).unwrap();
    let directory = fs::canonicalize(directory).unwrap();
    let file = directory.join("generated.fcproj");
    filmcraft_format::atomic_write(&file, &filmcraft_format::encode_with_view(&s.project, Some(&s.project_view()), false)).unwrap();
    s.path = Some(file.to_str().unwrap().into());
    s.execute("agent.snapshot", json!({"token":token(61)})).unwrap();
    let scope = json!({"projectId":"p_worker","anchorSequence":s.state.active_sequence.unwrap().0});
    let workspace = "2b6f05fb-8caa-44ca-8dbe-4dcb767bdf83";
    let checkpoint = "0109a861-1913-4589-adba-78133929d786";
    let params = json!({"token":token(61),"scope":scope,"workspace":workspace,"checkpoint":checkpoint});
    let before = state(&s);
    let mut pending = worker.start(&mut s, "agent.saveHistoryCheckpoint", params.clone(), None).unwrap();
    entered.recv_timeout(Duration::from_secs(2)).unwrap();
    assert!(s.is_display_only());
    assert!(s.execute("file.save", json!({})).is_err());
    assert_eq!(pending.cancel().unwrap()["cancellationAccepted"], true);
    drop(release);
    let done = finish(&mut pending);
    s = done.session;
    assert!(done.reply.result.is_err());
    assert_eq!(state(&s), before);
    let companion = directory.join(format!(".poema-history-{workspace}-{checkpoint}.json"));
    assert!(!companion.exists());
    s.execute("agent.snapshot", json!({"token":token(62)})).unwrap();
    let mut params = params;
    params["token"] = json!(token(62));
    let mut save = worker.start(&mut s, "agent.saveHistoryCheckpoint", params.clone(), None).unwrap();
    assert!(s.is_display_only());
    let done = finish(&mut save);
    s = done.session;
    let saved = done.reply.result.unwrap();
    assert_eq!(saved["status"], "saved");
    assert!(companion.is_file());
    s.execute("agent.snapshot", json!({"token":token(63)})).unwrap();
    params["token"] = json!(token(63));
    params["sha256"] = saved["checkpoint"]["sha256"].clone();
    let original = s.project.clone();
    let mut restore = worker.start(&mut s, "agent.restoreHistoryCheckpoint", params, None).unwrap();
    assert!(s.is_display_only());
    let done = finish(&mut restore);
    assert_eq!(done.reply.result.unwrap()["action"], "history_restore");
    assert!(Arc::ptr_eq(&done.session.project, &original));
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn exclusive_writer_keeps_display_inert_and_returns_exact_native_history_and_receipt() {
    let (worker, entered, release) = blocked();
    let (mut s, params) = fixture(&worker, 1);
    let original = s.project.clone();
    let history = s.history.undo.len();
    let revision = s.revision;
    let services = s.services.clone();
    let pool = s.media.clone();
    let previews = s.previews.clone();
    let mut pending = worker.start(&mut s, "agent.apply", params.clone(), None).unwrap();
    entered.recv_timeout(Duration::from_secs(2)).unwrap();
    assert!(s.is_display_only());
    assert!(Arc::ptr_eq(&s.project, &original));
    assert!(s.history.undo.is_empty());
    assert!(s.persistence.is_none());
    assert!(s.prefs_path.is_none());
    assert!(s.execute("agent.snapshot", json!({"token":token(2)})).is_err());
    assert!(s.execute("file.save", json!({})).is_err());
    assert!(s.edit::<()>("forbidden", |_, _| panic!("display edit ran")).is_err());
    assert!(s.edit_merged::<()>("forbidden", "key", |_, _| panic!("display gesture ran")).is_err());
    assert!(s.edit_group::<()>("forbidden", |_| panic!("display group ran")).is_err());
    assert!(s.undo().is_none());
    assert!(s.redo().is_none());
    assert!(worker.start(&mut s, "agent.apply", params.clone(), None).is_err());
    drop(release);
    let done = finish(&mut pending);
    s = done.session;
    assert!(!done.reply.uncertain);
    let receipt = done.reply.result.unwrap();
    assert!(!s.is_display_only());
    assert_eq!(s.revision, revision + 1);
    assert_eq!(s.history.undo.len(), history + 1);
    assert!(Arc::ptr_eq(&services, &s.services));
    assert!(Arc::ptr_eq(&pool, &s.media));
    assert!(Arc::ptr_eq(&previews, &s.previews));
    assert_eq!(s.execute("agent.apply", params).unwrap(), receipt, "reconciliation never repeats a write");
    let changed = s.project.clone();
    s.execute("agent.snapshot", json!({"token":token(3)})).unwrap();
    let mut undo = worker.start(&mut s, "agent.undo", json!({"token":token(3),"commit":token(1)}), None).unwrap();
    let done = finish(&mut undo);
    s = done.session;
    done.reply.result.unwrap();
    assert_eq!(s.project, original);
    s.execute("agent.snapshot", json!({"token":token(4)})).unwrap();
    let mut redo = worker.start(&mut s, "agent.redo", json!({"token":token(4),"commit":token(1),"undo":token(3)}), None).unwrap();
    let done = finish(&mut redo);
    s = done.session;
    done.reply.result.unwrap();
    assert_eq!(s.project, changed);
}

#[test]
fn queued_stop_revokes_the_real_ticket_without_publication_or_late_replay() {
    let (worker, entered, release) = blocked();
    let (mut s, params) = fixture(&worker, 10);
    let before = state(&s);
    let mut pending = worker.start(&mut s, "agent.apply", params.clone(), None).unwrap();
    entered.recv_timeout(Duration::from_secs(2)).unwrap();
    assert_eq!(pending.cancel().unwrap()["cancellationAccepted"], true);
    assert_eq!(pending.progress()["phase"], "cancelled");
    drop(release);
    let done = finish(&mut pending);
    s = done.session;
    assert!(done.reply.result.is_err());
    assert!(!done.reply.uncertain);
    // The failed command adds an error-log entry, but does not alter project/history/journal.
    assert_eq!(state(&s), before);
    assert!(s.execute("agent.apply", params).is_err());
    assert_eq!(state(&s), before);
}

#[test]
fn replaced_original_is_still_rejected_by_existing_session_file_guards() {
    let (worker, entered, release) = blocked();
    let (mut s, mut params) = fixture(&worker, 20);
    let file = std::env::temp_dir().join(format!("native-worker-source-{}-{}", std::process::id(), token(20)));
    fs::write(&file, b"original bytes").unwrap();
    let source = s.active_sequence().unwrap().video_tracks[0].items[0].item;
    Arc::make_mut(&mut s.project).item_mut(source).unwrap().as_media_mut().unwrap().media =
        filmcraft_project::MediaRef::File { path: file.to_string_lossy().into_owned() };
    s.execute("agent.snapshot", json!({"token":token(21)})).unwrap();
    params["token"] = json!(token(21));
    let before = state(&s);
    let mut pending = worker.start(&mut s, "agent.apply", params, None).unwrap();
    entered.recv_timeout(Duration::from_secs(2)).unwrap();
    fs::write(&file, b"replacement with different bytes").unwrap();
    drop(release);
    let done = finish(&mut pending);
    s = done.session;
    assert!(done.reply.result.is_err());
    assert!(!done.reply.uncertain);
    assert_eq!(state(&s), before);
    fs::remove_file(file).unwrap();
}

#[test]
fn panic_after_actual_commit_retains_real_receipt_and_reports_unknown_without_undo() {
    let worker = SessionWorker::spawn(|s, command, params| {
        s.execute(command, params)?;
        panic!("generated worker failure after real commit")
    })
    .unwrap();
    let (mut s, params) = fixture(&worker, 30);
    let before = s.project.clone();
    let revision = s.revision;
    let mut pending = worker.start(&mut s, "agent.apply", params.clone(), None).unwrap();
    let done = finish(&mut pending);
    s = done.session;
    assert!(done.reply.uncertain);
    assert_eq!(done.reply.envelope()["outcome"], "unknown");
    assert_eq!(s.revision, revision + 1);
    assert_ne!(s.project, before);
    let receipt = s.execute("agent.receipt", json!({"token":token(30)})).unwrap();
    assert_eq!(receipt["status"], "committed");
    assert_eq!(s.execute("agent.apply", params).unwrap(), receipt["result"]);
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(s.project, before);
}

#[test]
fn bounded_queue_refusal_restores_the_unsent_owner_and_does_not_consume_its_ticket() {
    let (worker, entered, release) = blocked();
    let (mut a, pa) = fixture(&worker, 40);
    let (mut b, pb) = fixture(&worker, 41);
    let (mut c, pc) = fixture(&worker, 42);
    let before = state(&c);
    let owner = c.project.clone();
    let mut first = worker.start(&mut a, "agent.apply", pa, None).unwrap();
    entered.recv_timeout(Duration::from_secs(2)).unwrap();
    let mut second = worker.start(&mut b, "agent.apply", pb, None).unwrap();
    assert!(worker.start(&mut c, "agent.apply", pc.clone(), None).is_err());
    assert!(!c.is_display_only());
    assert!(Arc::ptr_eq(&c.project, &owner));
    assert_eq!(state(&c), before);
    drop(release);
    finish(&mut first).reply.result.unwrap();
    finish(&mut second).reply.result.unwrap();
    assert_eq!(c.execute("agent.apply", pc).unwrap()["status"], "committed");
}

#[test]
fn unsupported_commands_and_bad_tokens_do_not_transfer_or_mutate_the_session() {
    let worker = SessionWorker::new().unwrap();
    let (mut s, params) = fixture(&worker, 50);
    let before = state(&s);
    for command in ["file.save", "file.import", "agent.snapshot", "agent.prepareCondense", "timeline.trim", ""] {
        assert!(worker.start(&mut s, command, params.clone(), None).is_err());
        assert!(!s.is_display_only());
        assert_eq!(state(&s), before);
    }
    for token in [Value::Null, json!("short"), json!("/generated/arbitrary-path"), json!(52)] {
        let mut request = params.clone();
        request["token"] = token;
        assert!(worker.start(&mut s, "agent.apply", request, None).is_err());
        assert_eq!(state(&s), before);
    }
}

#[test]
fn abandoned_pending_owner_flushes_unsaved_recovery_after_worker_exit() {
    let (worker, entered, release) = blocked();
    let (mut s, mut params) = fixture(&worker, 60);
    let data = std::env::temp_dir().join(format!("native-worker-recovery-{}-{}", std::process::id(), token(60)));
    fs::create_dir_all(&data).unwrap();
    s.start_autosave(crate::autosave::AutosaveConfig::new(&data)).unwrap();
    s.execute("agent.snapshot", json!({"token":token(61)})).unwrap();
    params["token"] = json!(token(61));
    let original = s.project.clone();
    let (finished, done) = mpsc::channel();
    let pending = worker
        .start(
            &mut s,
            "agent.apply",
            params,
            Some(Arc::new(move || {
                let _ = finished.send(());
            })),
        )
        .unwrap();
    entered.recv_timeout(Duration::from_secs(2)).unwrap();
    drop(pending);
    drop(release);
    done.recv_timeout(Duration::from_secs(10)).unwrap();
    let mut recovered = Session::default();
    recovered.start_autosave(crate::autosave::AutosaveConfig::new(&data)).unwrap();
    assert_eq!(recovered.recovery_candidates().len(), 1);
    let id = recovered.recovery_candidates()[0].id.clone();
    recovered.execute("file.recover", json!({"id":id})).unwrap();
    assert_eq!(recovered.project, original);
    recovered.shutdown();
    fs::remove_dir_all(data).unwrap();
}

#[test]
fn human_media_import_uses_the_exclusive_owner_and_a_stopped_request_cannot_poison_the_next_import() {
    let (worker, entered, release) = blocked();
    let mut s = Session::default();
    s.execute("file.newSequence", json!({"name":"Preserved cut","fps":24})).unwrap();
    worker.install(&mut s).unwrap();
    let root = crate::temp_dir().join(format!("worker-media-import-{}", std::process::id()));
    fs::create_dir(&root).unwrap();
    let file = root.join("take.wav");
    fs::write(&file, filmcraft_media::wav::write_wav16(&[0.0; 4800], 1, 48000)).unwrap();
    let params = json!({"paths":[file],"imageSequence":false,"mediaOnly":true,"maxFiles":2000});
    let before = state(&s);
    assert!(worker.start(&mut s, "mediaBrowser.import", params.clone(), None).is_err());
    for bad in [json!({"mediaOnly":false}), json!({"maxFiles":2001}), json!({"imageSequence":true}), json!({"paths":["relative.wav"]}), json!({"extra":true})] {
        let mut invalid = params.clone();
        invalid.as_object_mut().unwrap().extend(bad.as_object().unwrap().clone());
        assert!(worker.start_media_import(&mut s, invalid, None).is_err());
        assert_eq!(state(&s), before);
    }
    let mut pending = worker.start_media_import(&mut s, params.clone(), None).unwrap();
    entered.recv_timeout(Duration::from_secs(2)).unwrap();
    assert!(s.is_display_only());
    assert!(s.media_import_control.is_none());
    assert!(s.execute("file.save", json!({})).is_err());
    assert_eq!(pending.progress()["kind"], "native_media_import");
    assert_eq!(pending.cancel().unwrap()["cancellationAccepted"], true);
    drop(release);
    let done = finish(&mut pending);
    s = done.session;
    assert!(done.reply.result.is_err());
    assert_eq!(state(&s), before);
    assert!(s.media_import_control.is_none());
    let mut pending = worker.start_media_import(&mut s, params, None).unwrap();
    entered.recv_timeout(Duration::from_secs(2)).unwrap();
    let done = finish(&mut pending);
    s = done.session;
    assert!(!done.reply.uncertain);
    let result = done.reply.result.unwrap();
    assert_eq!(result["cancelled"], false);
    assert_eq!(result["items"].as_array().unwrap().len(), 1);
    assert_eq!(pending.cancel().unwrap()["cancellationAccepted"], false);
    assert!(s.media_import_control.is_none());
    assert!(s.journal.iter().any(|(id, _)| id == "mediaBrowser.import"));
    assert!(s.journal.iter().any(|(id, _)| id == "file.import"));
    fs::remove_dir_all(root).unwrap();
}
