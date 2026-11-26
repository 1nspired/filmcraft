use super::*;
use crate::control::{ControlRequest, Outcome};
use filmcraft_engine::Session;
use filmcraft_project::{ItemId, MediaRef};
use std::path::PathBuf;
use std::sync::{
    Mutex,
    atomic::{AtomicU64, Ordering},
    mpsc::{Sender, channel},
};

struct Temp(PathBuf);
impl Temp {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let p = std::env::temp_dir().join(format!("filmcraft-media-intake-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
        std::fs::create_dir_all(&p).unwrap();
        Self(p)
    }
    fn wav(&self, name: &str) -> String {
        let p = self.0.join(name);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, filmcraft_media::wav::write_wav16(&[0.0; 4800], 1, 48_000)).unwrap();
        p.to_string_lossy().into_owned()
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
type Asked = Arc<Mutex<Vec<(FileDialog, Sender<Vec<String>>)>>>;
fn setup() -> (FilmcraftApp, egui::Context, Asked) {
    let mut s = Session::default();
    s.execute("file.newSequence", json!({"name":"Intake fixture","fps":24})).unwrap();
    s.path = Some("/fixture/intake.fcproj".into());
    let (_, rx) = channel();
    let mut app = FilmcraftApp::new(s).with_control(rx);
    let asked: Asked = Arc::default();
    let a = asked.clone();
    app.hooks.run_dialog = Some(Box::new(move |dialog| {
        let (tx, rx) = channel();
        a.lock().unwrap().push((dialog, tx));
        rx
    }));
    (app, egui::Context::default(), asked)
}
fn request(app: &mut FilmcraftApp, n: u32, kind: &str) -> Value {
    let scope = json!({"projectId":"p_intake","anchorSequence":app.session.state.active_sequence.unwrap().0});
    let c = app.session.execute("agent.readSourceCatalog", json!({"scope":scope})).unwrap();
    json!({"operation_id":format!("{n:08x}-0000-4000-8000-{n:012x}"),"native_session":c["nativeSession"],"project_path":c["project"]["path"],"revision":c["revision"],"scope":scope,"kind":kind})
}
fn reference_of(p: &Value) -> Value {
    json!({"operation_id":p["operation_id"],"native_session":p["native_session"],"project_path":p["project_path"]})
}
fn call(app: &mut FilmcraftApp, ctx: &egui::Context, method: &str, p: Value) -> Value {
    let (req, _) = ControlRequest::new(method, p);
    match crate::control::handle(app, ctx, &req) {
        Outcome::Done(v) => v,
        _ => panic!("media intake must return a correlated UI result immediately"),
    }
}
fn read(app: &mut FilmcraftApp, ctx: &egui::Context, q: &Value) -> Value {
    let r = call(app, ctx, "ui.mediaIntake.read", reference_of(q));
    assert_eq!(r["ok"], true, "{r}");
    r["result"].clone()
}
fn answer(app: &mut FilmcraftApp, ctx: &egui::Context, asked: &Asked, paths: Vec<String>) {
    answer_start(app, ctx, asked, paths);
    settle(app, ctx);
}
fn answer_start(app: &mut FilmcraftApp, ctx: &egui::Context, asked: &Asked, paths: Vec<String>) {
    let (_, tx) = asked.lock().unwrap().pop().unwrap();
    tx.send(paths).unwrap();
    app.poll_native_dialog(ctx);
}
fn settle(app: &mut FilmcraftApp, ctx: &egui::Context) {
    let end = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while app.native_writer.pending.is_some() {
        assert!(std::time::Instant::now() < end, "import owner did not return");
        crate::native_writer::poll(app, ctx);
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
}
fn path_of(app: &FilmcraftApp, id: &Value) -> String {
    match &app.session.project.item(ItemId(id.as_u64().unwrap())).unwrap().as_media().unwrap().media {
        MediaRef::File { path } => path.clone(),
        _ => panic!("expected an imported native file"),
    }
}

#[test]
fn asynchronous_selection_correlates_exact_copy_path_and_never_reopens_on_repeated_begin_or_read() {
    let temp = Temp::new();
    let a = temp.wav("drive-a/take.wav");
    let b = temp.wav("drive-b/take.wav");
    assert_eq!(std::fs::read(a).unwrap(), std::fs::read(&b).unwrap());
    let (mut app, ctx, asked) = setup();
    let q = request(&mut app, 1, "files");
    let before = app.session.project.clone();
    let start = call(&mut app, &ctx, "ui.mediaIntake.begin", q.clone());
    assert_eq!(start["ok"], true, "{start}");
    assert_eq!(start["result"]["status"], "selecting");
    assert!(Arc::ptr_eq(&app.session.project, &before));
    assert_eq!(call(&mut app, &ctx, "ui.mediaIntake.begin", q.clone()), start);
    assert_eq!(asked.lock().unwrap().len(), 1);
    answer(&mut app, &ctx, &asked, vec![b.clone()]);
    let receipt = read(&mut app, &ctx, &q);
    assert_eq!(receipt["status"], "complete");
    assert_eq!(receipt["selected_paths"], json!([b]));
    let items = receipt["native_result"]["items"].as_array().unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(path_of(&app, &items[0]), receipt["selected_paths"][0]);
    assert_eq!(receipt["imported_sources"], json!([{"item":items[0],"kind":"file","path":receipt["selected_paths"][0]}]));
    let rev = app.session.revision;
    assert_eq!(call(&mut app, &ctx, "ui.mediaIntake.begin", q.clone())["result"], receipt);
    assert_eq!(read(&mut app, &ctx, &q), receipt);
    assert!(asked.lock().unwrap().is_empty());
    assert_eq!(app.session.revision, rev);
}

#[test]
fn selected_folder_uses_shared_native_tree_import_without_importing_caption_or_project_files() {
    let temp = Temp::new();
    let first = temp.wav("chosen/take.wav");
    let second = temp.wav("chosen/camera-b/take.wav");
    temp.wav("other-drive/chosen/take.wav");
    std::fs::write(temp.0.join("chosen/other.fcpxml"), "not an interchange project").unwrap();
    std::fs::write(temp.0.join("chosen/other.srt"), "1\n00:00:00,000 --> 00:00:01,000\nDo not import\n").unwrap();
    let (mut app, ctx, asked) = setup();
    let sequence = serde_json::to_value(app.session.active_sequence()).unwrap();
    let q = request(&mut app, 2, "folder");
    assert_eq!(call(&mut app, &ctx, "ui.mediaIntake.begin", q.clone())["ok"], true);
    answer(&mut app, &ctx, &asked, vec![temp.0.join("chosen").to_string_lossy().into_owned()]);
    let receipt = read(&mut app, &ctx, &q);
    assert_eq!(receipt["status"], "complete", "{receipt}");
    let mut paths: Vec<_> = receipt["native_result"]["items"].as_array().unwrap().iter().map(|i| path_of(&app, i)).collect();
    paths.sort();
    let mut expected = vec![first, second];
    expected.sort();
    assert_eq!(paths, expected);
    assert_eq!(receipt["native_result"]["bins"].as_array().unwrap().len(), 2);
    assert_eq!(serde_json::to_value(app.session.active_sequence()).unwrap(), sequence);
}

#[test]
fn stop_waits_for_picker_settlement_and_discards_even_a_late_successful_selection() {
    let temp = Temp::new();
    let wav = temp.wav("take.wav");
    let (mut app, ctx, asked) = setup();
    let q = request(&mut app, 3, "files");
    let before = app.session.project.clone();
    let history = app.session.history.undo.len();
    assert_eq!(call(&mut app, &ctx, "ui.mediaIntake.begin", q.clone())["ok"], true);
    let stopped = call(&mut app, &ctx, "ui.mediaIntake.stop", reference_of(&q));
    assert_eq!(stopped["result"]["status"], "stopping");
    assert!(app.native_dialog_open());
    assert_eq!(call(&mut app, &ctx, "ui.mediaIntake.stop", reference_of(&q)), stopped);
    answer(&mut app, &ctx, &asked, vec![wav]);
    assert_eq!(read(&mut app, &ctx, &q)["status"], "cancelled");
    assert!(Arc::ptr_eq(&app.session.project, &before));
    assert_eq!(app.session.history.undo.len(), history);
    assert!(!app.native_dialog_open());
}

#[test]
fn cancelled_and_lost_picker_answers_leave_project_and_history_untouched() {
    for disconnected in [false, true] {
        let (mut app, ctx, asked) = setup();
        let q = request(&mut app, 4, "files");
        let before = app.session.project.clone();
        let revision = app.session.revision;
        assert_eq!(call(&mut app, &ctx, "ui.mediaIntake.begin", q.clone())["ok"], true);
        if disconnected {
            let (_, tx) = asked.lock().unwrap().pop().unwrap();
            drop(tx);
            app.poll_native_dialog(&ctx);
        } else {
            answer(&mut app, &ctx, &asked, vec![]);
        }
        assert_eq!(read(&mut app, &ctx, &q)["status"], "cancelled");
        assert!(Arc::ptr_eq(&app.session.project, &before));
        assert_eq!(app.session.revision, revision);
    }
}

#[test]
fn intervening_human_edit_or_different_project_refuses_before_any_selected_file_is_imported() {
    let temp = Temp::new();
    let wav = temp.wav("take.wav");
    for replace in [false, true] {
        let (mut app, ctx, asked) = setup();
        let q = request(&mut app, 5, "files");
        assert_eq!(call(&mut app, &ctx, "ui.mediaIntake.begin", q.clone())["ok"], true);
        app.session.execute(if replace { "file.newProject" } else { "file.newSequence" }, json!({"name":"Human authoritative work"})).unwrap();
        let human = app.session.project.clone();
        let revision = app.session.revision;
        answer(&mut app, &ctx, &asked, vec![wav.clone()]);
        assert_eq!(read(&mut app, &ctx, &q)["status"], "rejected");
        assert!(Arc::ptr_eq(&app.session.project, &human));
        assert_eq!(app.session.revision, revision);
    }
}

#[test]
fn partial_native_import_retains_exact_successful_items_without_discovering_unrelated_imports() {
    let temp = Temp::new();
    let a = temp.wav("a.wav");
    let b = temp.wav("unrelated.wav");
    let (mut app, ctx, asked) = setup();
    app.session.execute("file.import", json!({"paths":[b]})).unwrap();
    let q = request(&mut app, 6, "files");
    assert_eq!(call(&mut app, &ctx, "ui.mediaIntake.begin", q.clone())["ok"], true);
    answer(&mut app, &ctx, &asked, vec![a.clone(), temp.0.join("missing.wav").to_string_lossy().into_owned()]);
    let receipt = read(&mut app, &ctx, &q);
    assert_eq!(receipt["status"], "partial", "{receipt}");
    let items = receipt["native_result"]["items"].as_array().unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(path_of(&app, &items[0]), a);
    assert_eq!(receipt["native_result"]["errors"].as_array().unwrap().len(), 1);
    assert_eq!(app.session.project.items.values().filter(|i| i.as_media().is_some()).count(), 2);
}

#[test]
fn malformed_stale_foreign_and_oversized_requests_never_open_or_publish() {
    let (mut app, ctx, asked) = setup();
    let q = request(&mut app, 7, "files");
    let before = app.session.project.clone();
    let revision = app.session.revision;
    let mut extra = q.clone();
    extra["paths"] = json!(["/guessed/path.wav"]);
    let mut stale = q.clone();
    stale["revision"] = json!(revision + 1);
    let mut foreign = q.clone();
    foreign["native_session"] = json!("00000000000000000000000000000000");
    let mut path = q.clone();
    path["project_path"] = json!("/different/project.fcproj");
    for p in [extra, stale, foreign, path, json!({}), json!([])] {
        assert_eq!(call(&mut app, &ctx, "ui.mediaIntake.begin", p)["ok"], false);
    }
    assert!(asked.lock().unwrap().is_empty());
    assert!(Arc::ptr_eq(&app.session.project, &before));
    assert_eq!(app.session.revision, revision);
    assert_eq!(call(&mut app, &ctx, "ui.mediaIntake.begin", q.clone())["ok"], true);
    answer(&mut app, &ctx, &asked, vec!["/too-many.wav".into(); MAX_SELECTED + 1]);
    assert_eq!(read(&mut app, &ctx, &q)["status"], "rejected");
    assert!(Arc::ptr_eq(&app.session.project, &before));
}

#[test]
fn unknown_receipt_cannot_restart_and_changed_arguments_cannot_reuse_an_existing_request() {
    let (mut app, ctx, asked) = setup();
    let q = request(&mut app, 8, "files");
    let missing = read(&mut app, &ctx, &q);
    assert_eq!(missing["status"], "not_found");
    assert_eq!(missing["can_restart"], false);
    assert_eq!(call(&mut app, &ctx, "ui.mediaIntake.begin", q.clone())["ok"], true);
    let mut changed = q.clone();
    changed["kind"] = json!("folder");
    assert_eq!(call(&mut app, &ctx, "ui.mediaIntake.begin", changed)["ok"], false);
    assert_eq!(asked.lock().unwrap().len(), 1);
    let mut bad = reference_of(&q);
    bad["project_path"] = json!("/other.fcproj");
    assert_eq!(call(&mut app, &ctx, "ui.mediaIntake.read", bad)["ok"], false);
    answer(&mut app, &ctx, &asked, vec![]);
    assert_eq!(read(&mut app, &ctx, &q)["status"], "cancelled");
}

#[test]
fn stop_before_begin_fences_a_late_control_request_without_opening_a_picker() {
    let (mut app, ctx, asked) = setup();
    let q = request(&mut app, 9, "files");
    let before = app.session.project.clone();
    let revision = app.session.revision;
    let cancelled = call(&mut app, &ctx, "ui.mediaIntake.stop", reference_of(&q));
    assert_eq!(cancelled["ok"], true);
    assert_eq!(cancelled["result"]["status"], "cancelled_before_start");
    assert_eq!(cancelled["result"]["native_scope_verified"], false);
    assert_eq!(call(&mut app, &ctx, "ui.mediaIntake.begin", q.clone()), cancelled);
    assert_eq!(read(&mut app, &ctx, &q), cancelled["result"]);
    assert_eq!(call(&mut app, &ctx, "ui.mediaIntake.stop", reference_of(&q)), cancelled);
    let mut foreign = q.clone();
    foreign["project_path"] = json!("/different.fcproj");
    assert_eq!(call(&mut app, &ctx, "ui.mediaIntake.begin", foreign)["ok"], false);
    assert!(asked.lock().unwrap().is_empty());
    assert!(!app.native_dialog_open());
    assert!(Arc::ptr_eq(&app.session.project, &before));
    assert_eq!(app.session.revision, revision);
}

#[test]
fn unsupported_picker_and_an_already_open_human_dialog_do_not_allocate_an_intake_request() {
    let (mut app, ctx, asked) = setup();
    let q = request(&mut app, 10, "files");
    let hook = app.hooks.run_dialog.take();
    assert_eq!(call(&mut app, &ctx, "ui.mediaIntake.info", json!({}))["result"]["files"], false);
    assert_eq!(call(&mut app, &ctx, "ui.mediaIntake.begin", q.clone())["ok"], false);
    app.hooks.run_dialog = hook;
    app.pick(FileDialog::Folder { at: None }, |_, _| Ok(Value::Null)).unwrap();
    assert_eq!(call(&mut app, &ctx, "ui.mediaIntake.begin", q.clone())["ok"], false);
    assert_eq!(read(&mut app, &ctx, &q)["status"], "not_found");
    answer(&mut app, &ctx, &asked, vec![]);
    assert_eq!(call(&mut app, &ctx, "ui.mediaIntake.begin", q.clone())["ok"], true);
    answer(&mut app, &ctx, &asked, vec![]);
    assert_eq!(read(&mut app, &ctx, &q)["status"], "cancelled");
}

struct ReleaseFile(Arc<(Mutex<bool>, std::sync::Condvar)>);
impl Drop for ReleaseFile {
    fn drop(&mut self) {
        *self.0.0.lock().unwrap() = true;
        self.0.1.notify_all();
    }
}
struct SlowMedia {
    path: String,
    entered: Sender<()>,
    release: Arc<(Mutex<bool>, std::sync::Condvar)>,
}
impl filmcraft_engine::Services for SlowMedia {
    fn read_file(&self, path: &str) -> std::io::Result<Vec<u8>> {
        if path == self.path {
            let _ = self.entered.send(());
            let mut ready = self.release.0.lock().unwrap();
            while !*ready {
                let (next, timeout) = self.release.1.wait_timeout(ready, std::time::Duration::from_secs(5)).unwrap();
                ready = next;
                if timeout.timed_out() {
                    return Err(std::io::Error::other("generated read was not released"));
                }
            }
        }
        std::fs::read(path)
    }
    fn write_file(&self, _: &str, _: &[u8]) -> std::io::Result<()> {
        Err(std::io::Error::other("fixture does not write files"))
    }
    fn list_entries(&self, path: &str) -> Option<std::io::Result<Vec<filmcraft_engine::media_browser::DirEntry>>> {
        Some(filmcraft_engine::media_browser::std_list_entries(path))
    }
}

#[test]
fn slow_import_keeps_control_responsive_and_stop_retains_only_the_settled_prefix_with_exact_locations() {
    let temp = Temp::new();
    let first = temp.wav("selected/first.wav");
    let second = temp.wav("selected/second.wav");
    let third = temp.wav("selected/third.wav");
    let (mut app, ctx, asked) = setup();
    let (entered, entry) = channel();
    let gate = Arc::new((Mutex::new(false), std::sync::Condvar::new()));
    let release = ReleaseFile(gate.clone());
    app.session.services = Arc::new(SlowMedia { path: second.clone(), entered, release: gate });
    let q = request(&mut app, 40, "files");
    let before = app.session.project.clone();
    let earlier_history = app.session.history.undo.clone();
    let sequence = app.session.active_sequence().unwrap().clone();
    assert_eq!(call(&mut app, &ctx, "ui.mediaIntake.begin", q.clone())["ok"], true);
    answer_start(&mut app, &ctx, &asked, vec![first.clone(), second.clone(), third.clone()]);
    entry.recv_timeout(std::time::Duration::from_secs(3)).unwrap();
    assert!(app.session.is_display_only());
    assert!(Arc::ptr_eq(&app.session.project, &before));
    let importing = read(&mut app, &ctx, &q);
    assert_eq!(importing["status"], "importing");
    assert_eq!(importing["receipt_complete"], false);
    assert_eq!(call(&mut app, &ctx, "ui.mediaIntake.begin", q.clone())["result"], importing);
    assert_eq!(call(&mut app, &ctx, "engine.execute", json!({"command":"file.newSequence","params":{"name":"interleaved"}}))["ok"], false);
    assert_eq!(call(&mut app, &ctx, "ui.key", json!({"key":"Z","command":true}))["ok"], false);
    let stopped = call(&mut app, &ctx, "ui.mediaIntake.stop", reference_of(&q));
    assert_eq!(stopped["result"]["status"], "stopping");
    assert_eq!(stopped["result"]["close_picker_to_finish"], false);
    assert!(asked.lock().unwrap().is_empty());
    drop(release);
    settle(&mut app, &ctx);
    let receipt = read(&mut app, &ctx, &q);
    assert_eq!(receipt["status"], "partial");
    assert_eq!(receipt["cancel_requested"], true);
    assert_eq!(receipt["native_result"]["cancelled"], true);
    let sources = receipt["imported_sources"].as_array().unwrap();
    assert_eq!(sources.len(), 2);
    assert_eq!(sources.iter().map(|s| s["path"].as_str().unwrap()).collect::<Vec<_>>(), vec![first.as_str(), second.as_str()]);
    assert_eq!(sources.iter().map(|s| s["item"].clone()).collect::<Vec<_>>(), *receipt["native_result"]["items"].as_array().unwrap());
    assert_eq!(app.session.active_sequence().unwrap(), &sequence);
    assert!(!app.session.is_display_only());
    assert!(!app.session.project.items.values().any(|i| i.as_media().is_some_and(|m| matches!(&m.media,MediaRef::File {path} if path==&third))));
    let imported = app.session.project.clone();
    assert_eq!(app.session.history.undo.len(), earlier_history.len() + 1);
    app.session.execute("edit.undo", json!({})).unwrap();
    assert_eq!(*app.session.project, *before);
    assert_eq!(app.session.active_sequence().unwrap(), &sequence);
    assert_eq!(app.session.history.undo, earlier_history);
    app.session.execute("edit.redo", json!({})).unwrap();
    assert_eq!(*app.session.project, *imported);
    assert_eq!(app.session.active_sequence().unwrap(), &sequence);
    assert_eq!(read(&mut app, &ctx, &q), receipt);
    app.session.execute("edit.undo", json!({})).unwrap();
    assert_eq!(*app.session.project, *before);
    let r = app.session.execute("file.import", json!({"paths":[third]})).unwrap();
    assert_eq!(r["items"].as_array().unwrap().len(), 1);
    assert!(r.get("cancelled").is_none());
}

#[cfg(not(target_arch = "wasm32"))]
fn settle_retirement(app: &mut FilmcraftApp, ctx: &egui::Context) {
    let until = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while app.media_intake.retiring.is_some() {
        assert!(std::time::Instant::now() < until, "retirement worker did not settle");
        poll(app, ctx);
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
}
#[cfg(not(target_arch = "wasm32"))]
fn archived_setup(temp: &Temp) -> (FilmcraftApp, egui::Context, Asked) {
    let (mut app, ctx, asked) = setup();
    app.media_intake = State::new(Some(temp.0.clone()));
    (app, ctx, asked)
}
#[test]
#[cfg(not(target_arch = "wasm32"))]
fn two_hundred_sixty_real_picker_requests_retire_without_forgetting_old_ids_or_changing_project_history() {
    let temp = Temp::new();
    let (mut app, ctx, asked) = archived_setup(&temp);
    let before = app.session.project.clone();
    let revision = app.session.revision;
    let undo = app.session.history.undo.len();
    let mut requests = Vec::new();
    for id in 1000..1260 {
        let q = request(&mut app, id, "files");
        assert_eq!(call(&mut app, &ctx, "ui.mediaIntake.begin", q.clone())["ok"], true);
        assert!(app.media_intake.records.len() + app.media_intake.fences.len() <= MAX_REQUESTS + 1);
        settle_retirement(&mut app, &ctx);
        assert_eq!(asked.lock().unwrap().len(), 1);
        answer(&mut app, &ctx, &asked, Vec::new());
        assert_eq!(read(&mut app, &ctx, &q)["status"], "cancelled");
        requests.push(q);
    }
    assert!(app.media_intake.records.len() <= MAX_REQUESTS);
    assert!(std::fs::read_dir(app.media_intake.archive.as_ref().unwrap().root()).unwrap().count() >= 128);
    for q in &requests {
        let old = read(&mut app, &ctx, q);
        assert_eq!(call(&mut app, &ctx, "ui.mediaIntake.begin", q.clone())["result"], old);
        assert_eq!(call(&mut app, &ctx, "ui.mediaIntake.stop", reference_of(q))["result"], old);
    }
    assert!(asked.lock().unwrap().is_empty());
    assert!(Arc::ptr_eq(&app.session.project, &before));
    assert_eq!(app.session.revision, revision);
    assert_eq!(app.session.history.undo.len(), undo);
}
#[test]
#[cfg(not(target_arch = "wasm32"))]
fn retirement_publication_before_active_removal_reopens_as_the_exact_receipt_and_rejects_foreign_reuse() {
    let temp = Temp::new();
    let (mut app, ctx, asked) = archived_setup(&temp);
    let q = request(&mut app, 1400, "files");
    assert_eq!(call(&mut app, &ctx, "ui.mediaIntake.begin", q.clone())["ok"], true);
    answer(&mut app, &ctx, &asked, Vec::new());
    let old = read(&mut app, &ctx, &q);
    let entry = Entry::Record { record: app.media_intake.records.get(q["operation_id"].as_str().unwrap()).unwrap().clone() };
    app.media_intake.archive.as_ref().unwrap().publish(&[entry]).unwrap();
    assert_eq!(app.media_intake.records.len(), 1);
    // Simulate loss of the native memory after durable publication, before
    // deleting the active entry. The runtime archive remains authoritative.
    app.media_intake = State::new(Some(temp.0.clone()));
    assert_eq!(read(&mut app, &ctx, &q), old);
    assert_eq!(call(&mut app, &ctx, "ui.mediaIntake.begin", q.clone())["result"], old);
    for field in ["native_session", "project_path", "kind", "revision"] {
        let mut changed = q.clone();
        changed[field] = match field {
            "native_session" => json!("0".repeat(32)),
            "project_path" => json!("/foreign.fcproj"),
            "kind" => json!("folder"),
            _ => json!(q["revision"].as_u64().unwrap() + 1),
        };
        assert_eq!(call(&mut app, &ctx, "ui.mediaIntake.begin", changed)["ok"], false);
    }
    assert!(asked.lock().unwrap().is_empty());
    assert_eq!(call(&mut app, &ctx, "ui.mediaIntake.stop", reference_of(&q))["result"], old);
}
#[test]
#[cfg(not(target_arch = "wasm32"))]
fn archive_worker_keeps_ui_and_stop_responsive_and_a_stopped_queued_request_never_opens_its_picker() {
    let temp = Temp::new();
    let (mut app, ctx, asked) = archived_setup(&temp);
    for id in 1500..1628 {
        let q = request(&mut app, id, "files");
        assert_eq!(call(&mut app, &ctx, "ui.mediaIntake.begin", q)["ok"], true);
        answer(&mut app, &ctx, &asked, Vec::new());
    }
    let (entered_tx, entered_rx) = channel();
    let (go_tx, go_rx) = channel();
    app.media_intake.archive.as_mut().unwrap().publish_gate = Some((entered_tx, Arc::new(Mutex::new(go_rx))));
    let before = app.session.project.clone();
    let revision = app.session.revision;
    let q = request(&mut app, 1628, "files");
    let start = std::time::Instant::now();
    let reply = call(&mut app, &ctx, "ui.mediaIntake.begin", q.clone());
    let start_elapsed = start.elapsed();
    entered_rx.recv_timeout(std::time::Duration::from_secs(5)).unwrap();
    let stop_start = std::time::Instant::now();
    let stop = call(&mut app, &ctx, "ui.mediaIntake.stop", reference_of(&q));
    let stop_elapsed = stop_start.elapsed();
    let metadata = app.session.execute("agent.readSourceCatalog", json!({"scope":q["scope"]}));
    let no_picker = asked.lock().unwrap().is_empty();
    go_tx.send(()).unwrap();
    settle_retirement(&mut app, &ctx);
    assert_eq!(reply["ok"], true);
    assert!(start_elapsed < std::time::Duration::from_secs(1));
    assert_eq!(stop["result"]["status"], "cancelled");
    assert!(stop_elapsed < std::time::Duration::from_secs(1));
    assert!(metadata.is_ok());
    assert!(no_picker);
    assert!(asked.lock().unwrap().is_empty());
    assert_eq!(read(&mut app, &ctx, &q)["status"], "cancelled");
    assert!(Arc::ptr_eq(&app.session.project, &before));
    assert_eq!(app.session.revision, revision);
}
#[test]
#[cfg(not(target_arch = "wasm32"))]
fn cancellation_fences_retire_and_stop_overtaking_a_full_store_still_prevents_a_delayed_begin() {
    let temp = Temp::new();
    let (mut app, ctx, asked) = archived_setup(&temp);
    let before = app.session.project.clone();
    let mut requests = Vec::new();
    for id in 1700..1960 {
        let q = request(&mut app, id, "files");
        let stopped = call(&mut app, &ctx, "ui.mediaIntake.stop", reference_of(&q));
        assert_eq!(stopped["result"]["status"], "cancelled_before_start");
        assert_eq!(call(&mut app, &ctx, "ui.mediaIntake.begin", q.clone())["result"], stopped["result"]);
        settle_retirement(&mut app, &ctx);
        requests.push(q);
    }
    for q in requests {
        assert_eq!(call(&mut app, &ctx, "ui.mediaIntake.begin", q)["result"]["status"], "cancelled_before_start");
    }
    assert!(app.media_intake.fences.len() <= MAX_REQUESTS);
    assert!(asked.lock().unwrap().is_empty());
    assert!(Arc::ptr_eq(&app.session.project, &before));
}
#[test]
#[cfg(not(target_arch = "wasm32"))]
fn unresolved_native_receipts_are_never_retired_and_cannot_open_another_picker_at_capacity() {
    let temp = Temp::new();
    let (mut app, ctx, asked) = archived_setup(&temp);
    let base = request(&mut app, 2000, "files");
    assert_eq!(call(&mut app, &ctx, "ui.mediaIntake.begin", base.clone())["ok"], true);
    answer(&mut app, &ctx, &asked, Vec::new());
    let template = app.media_intake.records.values().next().unwrap().clone();
    app.media_intake.records.clear();
    for id in 2000..2128 {
        let q: Begin = serde_json::from_value(request(&mut app, id, "files")).unwrap();
        let mut row = template.clone();
        row.request = q.clone();
        row.value["operation_id"] = json!(q.operation_id);
        row.value["status"] = json!("unknown");
        row.value["receipt_complete"] = json!(false);
        app.media_intake.records.insert(q.operation_id, row);
    }
    let before = app.media_intake.records.clone();
    let q = request(&mut app, 2128, "files");
    assert_eq!(call(&mut app, &ctx, "ui.mediaIntake.begin", q)["ok"], false);
    assert!(app.media_intake.records == before);
    assert!(app.media_intake.retiring.is_none());
    assert!(asked.lock().unwrap().is_empty());
}
#[test]
#[cfg(not(target_arch = "wasm32"))]
fn failed_archive_publication_preserves_active_receipts_and_never_opens_the_queued_picker() {
    let temp = Temp::new();
    let (mut app, ctx, asked) = archived_setup(&temp);
    for id in 2200..2328 {
        let q = request(&mut app, id, "files");
        assert_eq!(call(&mut app, &ctx, "ui.mediaIntake.begin", q)["ok"], true);
        answer(&mut app, &ctx, &asked, Vec::new());
    }
    let root = app.media_intake.archive.as_ref().unwrap().root().to_path_buf();
    std::fs::create_dir(&root).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    let first = app.media_intake.records.keys().next().unwrap().clone();
    let corrupt = root.join(format!("{first}.json"));
    std::fs::write(&corrupt, b"not a receipt").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&corrupt, std::fs::Permissions::from_mode(0o400)).unwrap();
    }
    let old = app.media_intake.records.clone();
    let q = request(&mut app, 2328, "files");
    assert_eq!(call(&mut app, &ctx, "ui.mediaIntake.begin", q.clone())["ok"], true);
    settle_retirement(&mut app, &ctx);
    assert!(app.media_intake.blocked);
    assert_eq!(read(&mut app, &ctx, &q)["status"], "failed");
    assert_eq!(app.media_intake.records.len(), 129);
    assert!(old.iter().all(|(id, r)| app.media_intake.records.get(id) == Some(r)));
    assert!(asked.lock().unwrap().is_empty());
    let another = request(&mut app, 2329, "files");
    assert_eq!(call(&mut app, &ctx, "ui.mediaIntake.begin", another)["ok"], false);
}
#[test]
#[cfg(unix)]
fn altered_archive_files_and_replaced_or_redirected_directories_refuse_without_reopening() {
    use std::os::unix::fs::{PermissionsExt, symlink};
    for mode in ["bytes", "symlink", "directory", "oversized"] {
        let temp = Temp::new();
        let (mut app, ctx, asked) = archived_setup(&temp);
        let q = request(&mut app, 2400, "files");
        assert_eq!(call(&mut app, &ctx, "ui.mediaIntake.begin", q.clone())["ok"], true);
        answer(&mut app, &ctx, &asked, Vec::new());
        let entry = Entry::Record { record: app.media_intake.records.values().next().unwrap().clone() };
        let store = app.media_intake.archive.as_ref().unwrap().clone();
        store.publish(&[entry]).unwrap();
        let root = store.root();
        let file = root.join(format!("{}.json", q["operation_id"].as_str().unwrap()));
        app.media_intake.records.clear();
        match mode {
            "bytes" => {
                std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o600)).unwrap();
                std::fs::write(&file, b"corrupt").unwrap();
                std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o400)).unwrap();
            }
            "symlink" => {
                std::fs::remove_file(&file).unwrap();
                let target = temp.0.join("unrelated.txt");
                std::fs::write(&target, b"keep me").unwrap();
                symlink(&target, &file).unwrap();
            }
            "directory" => {
                std::fs::rename(root, temp.0.join("old-archive")).unwrap();
                std::fs::create_dir(root).unwrap();
                std::fs::set_permissions(root, std::fs::Permissions::from_mode(0o700)).unwrap();
            }
            _ => {
                std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o600)).unwrap();
                std::fs::OpenOptions::new().write(true).open(&file).unwrap().set_len(4 * 1024 * 1024).unwrap();
                std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o400)).unwrap();
            }
        }
        assert_eq!(call(&mut app, &ctx, "ui.mediaIntake.read", reference_of(&q))["ok"], false);
        assert_eq!(call(&mut app, &ctx, "ui.mediaIntake.begin", q)["ok"], false);
        assert!(asked.lock().unwrap().is_empty());
    }
}

#[test]
#[cfg(not(target_arch = "wasm32"))]
fn retirement_preserves_the_existing_bounded_unicode_scope_contract_without_normalizing_identity() {
    let temp = Temp::new();
    let (mut app, ctx, asked) = archived_setup(&temp);
    let mut q = request(&mut app, 2500, "files");
    q["scope"]["projectId"] = json!("p_édit liberté");
    let start = call(&mut app, &ctx, "ui.mediaIntake.begin", q.clone());
    assert_eq!(start["ok"], true, "{start}");
    answer(&mut app, &ctx, &asked, Vec::new());
    let old = read(&mut app, &ctx, &q);
    let entry = Entry::Record { record: app.media_intake.records.values().next().unwrap().clone() };
    app.media_intake.archive.as_ref().unwrap().publish(&[entry]).unwrap();
    app.media_intake = State::new(Some(temp.0.clone()));
    assert_eq!(read(&mut app, &ctx, &q), old);
    assert!(asked.lock().unwrap().is_empty());
}

#[test]
#[cfg(not(target_arch = "wasm32"))]
fn byte_pressure_retires_additional_batches_before_opening_the_same_picker() {
    let temp = Temp::new();
    let (mut app, ctx, asked) = archived_setup(&temp);
    let q = request(&mut app, 2600, "files");
    assert_eq!(call(&mut app, &ctx, "ui.mediaIntake.begin", q)["ok"], true);
    answer(&mut app, &ctx, &asked, Vec::new());
    let template = app.media_intake.records.values().next().unwrap().clone();
    app.media_intake.records.clear();
    for id in 2600..2728 {
        let q: Begin = serde_json::from_value(request(&mut app, id, "files")).unwrap();
        let mut row = template.clone();
        row.request = q.clone();
        row.value["operation_id"] = json!(q.operation_id);
        if id >= 2713 {
            row.value["status"] = json!("failed");
            row.value["error"] = json!("Generated terminal import refusal");
            row.value["selected_paths"] = json!(vec![format!("/generated/{}", "a".repeat(3980)); 256]);
        }
        app.media_intake.records.insert(q.operation_id, row);
    }
    assert!(retained_bytes(&app).unwrap() > MAX_RETAINED_BYTES - MAX_RECEIPT_BYTES);
    let before = app.session.project.clone();
    let q = request(&mut app, 2800, "files");
    let started = call(&mut app, &ctx, "ui.mediaIntake.begin", q.clone());
    assert_eq!(started["ok"], true, "{started}");
    settle_retirement(&mut app, &ctx);
    assert_eq!(asked.lock().unwrap().len(), 1);
    assert_eq!(app.media_intake.records.len(), 1);
    assert!(retained_bytes(&app).unwrap() < MAX_RETAINED_BYTES - MAX_RECEIPT_BYTES);
    answer(&mut app, &ctx, &asked, Vec::new());
    assert_eq!(read(&mut app, &ctx, &q)["status"], "cancelled");
    assert!(Arc::ptr_eq(&app.session.project, &before));
}

#[cfg(not(target_arch = "wasm32"))]
fn release_input(app: &mut FilmcraftApp, ctx: &egui::Context, q: &Value) -> Value {
    call(app, ctx, "ui.mediaIntake.release", json!({"request":q,"action":"keep_imports_release_context"}))
}
#[cfg(not(target_arch = "wasm32"))]
fn settle_release(app: &mut FilmcraftApp, ctx: &egui::Context, q: &Value) -> Value {
    let end = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        poll(app, ctx);
        let r = call(app, ctx, "ui.mediaIntake.resolution", q.clone());
        assert_eq!(r["ok"], true, "{r}");
        if r["result"]["status"] != "running" {
            return r["result"].clone();
        }
        assert!(std::time::Instant::now() < end);
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
}
#[test]
#[cfg(not(target_arch = "wasm32"))]
fn explicit_unknown_release_preserves_import_and_undo_and_recovers_historical_evidence_after_recreation() {
    let temp = Temp::new();
    let (mut app, ctx, asked) = archived_setup(&temp);
    let file = temp.wav("selected/take.wav");
    let q = request(&mut app, 2000, "files");
    assert_eq!(call(&mut app, &ctx, "ui.mediaIntake.begin", q.clone())["ok"], true);
    answer(&mut app, &ctx, &asked, vec![file]);
    let item = read(&mut app, &ctx, &q)["native_result"]["items"][0].clone();
    let original = finish(&mut app, q["operation_id"].as_str().unwrap(), "unknown", vec![], None, Some("Outcome unavailable".into())).unwrap();
    let project = app.session.project.clone();
    let revision = app.session.revision;
    let undo = app.session.history.undo.len();
    assert_eq!(release_input(&mut app, &ctx, &q)["result"]["status"], "running");
    assert!(app.media_intake.records.contains_key(q["operation_id"].as_str().unwrap()));
    let r = settle_release(&mut app, &ctx, &q);
    assert_eq!(r["status"], "released");
    assert_eq!(r["proof"]["outcome"], "unconfirmed");
    assert_eq!(r["proof"]["imports_undone"], false);
    assert_eq!(r["proof"]["source_authority"], false);
    assert!(!app.media_intake.records.contains_key(q["operation_id"].as_str().unwrap()));
    assert!(Arc::ptr_eq(&project, &app.session.project));
    assert_eq!(revision, app.session.revision);
    assert_eq!(undo, app.session.history.undo.len());
    assert_eq!(read(&mut app, &ctx, &q), original);
    app.media_intake = State::new(Some(temp.0.clone()));
    assert_eq!(call(&mut app, &ctx, "ui.mediaIntake.resolution", q.clone())["result"], r);
    assert_eq!(call(&mut app, &ctx, "ui.mediaIntake.begin", q.clone())["result"], original);
    let mut foreign = q.clone();
    foreign["kind"] = json!("folder");
    assert_eq!(call(&mut app, &ctx, "ui.mediaIntake.begin", foreign)["ok"], false);
    assert!(asked.lock().unwrap().is_empty());
    app.session.execute("edit.undo", json!({})).unwrap();
    assert!(app.session.project.item(ItemId(item.as_u64().unwrap())).is_none());
    app.session.execute("edit.redo", json!({})).unwrap();
    assert!(app.session.project.item(ItemId(item.as_u64().unwrap())).is_some());
}
#[test]
#[cfg(not(target_arch = "wasm32"))]
fn missing_release_fences_delayed_begin_before_publication_and_keeps_ui_responsive() {
    let temp = Temp::new();
    let (mut app, ctx, asked) = archived_setup(&temp);
    let q = request(&mut app, 2001, "files");
    let (entered, seen) = channel();
    let (go, gate) = channel();
    app.media_intake.releases.as_mut().unwrap().publish_gate = Some((entered, Arc::new(Mutex::new(gate))));
    let project = app.session.project.clone();
    let revision = app.session.revision;
    assert_eq!(release_input(&mut app, &ctx, &q)["result"]["status"], "running");
    seen.recv_timeout(std::time::Duration::from_secs(3)).unwrap();
    assert_eq!(call(&mut app, &ctx, "ui.mediaIntake.begin", q.clone())["result"]["status"], "cancelled_before_start");
    assert_eq!(call(&mut app, &ctx, "ui.mediaIntake.resolution", q.clone())["result"]["status"], "running");
    assert!(!call(&mut app, &ctx, "ui.mediaIntake.info", json!({})).is_null());
    assert!(asked.lock().unwrap().is_empty());
    go.send(()).unwrap();
    let r = settle_release(&mut app, &ctx, &q);
    assert_eq!(r["status"], "released");
    assert!(Arc::ptr_eq(&project, &app.session.project));
    assert_eq!(revision, app.session.revision);
    app.media_intake = State::new(Some(temp.0.clone()));
    assert_eq!(call(&mut app, &ctx, "ui.mediaIntake.begin", q.clone())["result"]["status"], "cancelled_before_start");
    let mut changed = q.clone();
    changed["scope"]["projectId"] = json!("foreign");
    assert_eq!(call(&mut app, &ctx, "ui.mediaIntake.begin", changed)["ok"], false);
    assert_eq!(call(&mut app, &ctx, "ui.mediaIntake.resolution", q)["result"], r);
}
#[test]
#[cfg(not(target_arch = "wasm32"))]
fn active_selection_refuses_release_and_terminal_selection_returns_exact_known_receipt() {
    let temp = Temp::new();
    let (mut app, ctx, asked) = archived_setup(&temp);
    let q = request(&mut app, 2002, "files");
    assert_eq!(call(&mut app, &ctx, "ui.mediaIntake.begin", q.clone())["ok"], true);
    assert_eq!(release_input(&mut app, &ctx, &q)["ok"], false);
    assert!(app.media_intake.resolution.is_none());
    answer(&mut app, &ctx, &asked, vec![]);
    let old = read(&mut app, &ctx, &q);
    let known = release_input(&mut app, &ctx, &q);
    assert_eq!(known["result"]["status"], "known");
    assert_eq!(known["result"]["receipt"], old);
    assert!(!app.media_intake.releases.as_ref().unwrap().root().exists());
}
#[test]
#[cfg(not(target_arch = "wasm32"))]
fn failed_release_keeps_evidence_and_fence_and_cannot_claim_success_or_reopen_picker() {
    let temp = Temp::new();
    let (mut app, ctx, asked) = archived_setup(&temp);
    let q = request(&mut app, 2003, "files");
    let root = app.media_intake.releases.as_ref().unwrap().root().to_path_buf();
    std::fs::write(root, b"not a directory").unwrap();
    // The archive is checked before publication; restore a nonexistent child,
    // then pause the worker and substitute the invalid storage underneath it.
    std::fs::remove_file(app.media_intake.releases.as_ref().unwrap().root()).unwrap();
    let (entered, seen) = channel();
    let (go, gate) = channel();
    app.media_intake.releases.as_mut().unwrap().publish_gate = Some((entered, Arc::new(Mutex::new(gate))));
    assert_eq!(release_input(&mut app, &ctx, &q)["result"]["status"], "running");
    seen.recv_timeout(std::time::Duration::from_secs(3)).unwrap();
    std::fs::write(app.media_intake.releases.as_ref().unwrap().root(), b"unavailable storage").unwrap();
    go.send(()).unwrap();
    assert_eq!(settle_release(&mut app, &ctx, &q)["status"], "failed");
    assert!(app.media_intake.blocked);
    assert!(app.media_intake.fences.contains_key(q["operation_id"].as_str().unwrap()));
    assert_eq!(call(&mut app, &ctx, "ui.mediaIntake.begin", q.clone())["result"]["status"], "cancelled_before_start");
    assert!(asked.lock().unwrap().is_empty());
}

#[test]
fn independent_project_picker_scope_is_disjoint_from_legacy_sequence_scope_and_never_accepts_a_null_or_dummy_anchor() {
    let root = serde_json::json!({"projectId":"p_picker_root","workspaceId":"11111111-1111-4111-8111-111111111111"});
    let scope: super::Scope = serde_json::from_value(root.clone()).unwrap();
    assert!(scope.valid());
    assert_eq!(serde_json::to_value(&scope).unwrap(), root);
    let legacy = serde_json::json!({"projectId":"p_picker_root","anchorSequence":21});
    let scope: super::Scope = serde_json::from_value(legacy.clone()).unwrap();
    assert!(scope.valid());
    assert_eq!(serde_json::to_value(&scope).unwrap(), legacy);
    for bad in [
        serde_json::json!({"projectId":"p_picker_root","workspaceId":"11111111-1111-4111-8111-111111111111","anchorSequence":21}),
        serde_json::json!({"projectId":"p_picker_root","workspaceId":"11111111-1111-4111-8111-111111111111","anchorSequence":null}),
        serde_json::json!({"projectId":"p_picker_root","anchorSequence":0}),
        serde_json::json!({"projectId":"p_picker_root","workspaceId":"not-a-workspace"}),
    ] {
        assert!(serde_json::from_value::<super::Scope>(bad).map_or(true, |scope| !scope.valid()));
    }
}
