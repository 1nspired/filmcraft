//! Actual shared native UI/control flow; synthetic model, no client media/window.
use egui_kittest::Harness;
use filmcraft_engine::Session;
use filmcraft_project::{ClipId, ProgramExtentPolicy};
use filmcraft_time::{FrameRate, TICKS_PER_SECOND, Tick};
use filmcraft_ui_egui::{FilmcraftApp, control::ControlRequest};
use serde_json::{Value, json};
use std::sync::{
    Arc,
    mpsc::{Sender, channel},
};
struct Driver {
    harness: Harness<'static, FilmcraftApp>,
    tx: Sender<ControlRequest>,
    tail: ClipId,
}
impl Driver {
    fn build(poema: bool) -> Self {
        let mut session = Session::default();
        session.execute("file.openDemoProject", json!({})).unwrap();
        let id = session.state.active_sequence.unwrap();
        let mut project = (*session.project).clone();
        let q = project.sequence_mut(id).unwrap();
        q.settings.frame_rate = FrameRate::FPS_24;
        q.program_extent = poema.then_some(ProgramExtentPolicy::Poema);
        q.video_tracks.truncate(1);
        q.audio_tracks.truncate(1);
        q.video_tracks[0].items.truncate(1);
        q.audio_tracks[0].items.truncate(1);
        q.markers.clear();
        for track in q.all_tracks_mut() {
            track.transitions.clear();
            for clip in &mut track.items {
                clip.link = None;
                clip.group = None;
                clip.effects.clear();
                clip.markers.clear();
                clip.source_in = Tick::ZERO;
                clip.source_end_offset = Tick::ZERO;
            }
        }
        q.video_tracks[0].items[0].start = Tick::ZERO;
        q.video_tracks[0].items[0].duration = Tick(3 * TICKS_PER_SECOND);
        q.video_tracks[0].items[0].name = "PROGRAM_FIXTURE".into();
        let clip = &mut q.audio_tracks[0].items[0];
        clip.start = Tick(900 * TICKS_PER_SECOND);
        clip.duration = Tick(2 * TICKS_PER_SECOND);
        clip.name = "OUTSIDE_PROGRAM_TAIL".into();
        let tail = clip.id;
        q.check().unwrap();
        session.project = Arc::new(project);
        let (tx, rx) = channel();
        let app = FilmcraftApp::new(session).with_control(rx);
        let mut builder = Harness::builder().with_size(egui::vec2(1600.0, 980.0)).with_max_steps(10000);
        if std::env::var_os("FILMCRAFT_POEMA_CONTENT_SHOT_DIR").is_some() {
            builder = builder.wgpu();
        }
        let harness = builder.build_eframe(move |_cc| app);
        let mut d = Self { harness, tx, tail };
        d.frames(4);
        d
    }
    fn frames(&mut self, n: usize) {
        for _ in 0..n {
            let ctx = self.harness.ctx.clone();
            let mut raw = std::mem::take(self.harness.input_mut());
            eframe::App::raw_input_hook(self.harness.state_mut(), &ctx, &mut raw);
            *self.harness.input_mut() = raw;
            self.harness.step();
        }
    }
    fn ok(&mut self, method: &str, params: Value) -> Value {
        let (req, reply) = ControlRequest::new(method, params.clone());
        self.tx.send(req).unwrap();
        for _ in 0..600 {
            self.frames(1);
            if let Ok(value) = reply.try_recv() {
                assert_eq!(value["ok"], json!(true), "{method} {params}: {value}");
                return value["result"].clone();
            }
        }
        panic!("Missing {method} reply");
    }
    fn rect(&mut self, id: &str) -> [f32; 4] {
        let v = self.ok("ui.elements", json!({"prefix":id}));
        let e = v.as_array().unwrap().iter().find(|e| e["id"] == json!(id)).unwrap_or_else(|| panic!("Missing {id}: {v}"));
        let r = e["rect"].as_array().unwrap();
        [0, 1, 2, 3].map(|i| r[i].as_f64().unwrap() as f32)
    }
    fn settle(&mut self) {
        for _ in 0..200 {
            let v = &self.harness.state().ui.timeline;
            if v.pps == v.target_pps && v.scroll == v.target_scroll {
                return;
            }
            self.frames(1);
        }
    }
    fn shot(&mut self, name: &str) {
        if let Some(dir) = std::env::var_os("FILMCRAFT_POEMA_CONTENT_SHOT_DIR") {
            for _ in 0..20 {
                std::thread::sleep(std::time::Duration::from_millis(40));
                self.frames(1);
            }
            let image = self.harness.render().unwrap();
            let png = filmcraft_ui_egui::control::encode_png(image.as_raw(), image.width(), image.height()).unwrap();
            let dir = std::path::PathBuf::from(dir);
            assert!(dir.is_absolute());
            std::fs::create_dir_all(&dir).unwrap();
            let project_path = dir.join(format!("{name}.fcproj"));
            assert!(!project_path.exists());
            self.ok("engine.execute", json!({"command":"file.saveCopy","params":{"path":project_path}}));
            let path = dir.join(format!("{name}.png"));
            assert!(!path.exists());
            std::fs::write(&path, png).unwrap();
            eprintln!("poema content screenshot: {}", path.display());
        }
    }
}
#[test]
fn fitting_uses_all_actual_content_without_extending_the_program() {
    let mut d = Driver::build(true);
    let before = (*d.harness.state().session.project).clone();
    let depth = d.harness.state().session.history.undo.len();
    d.harness.state_mut().ui.timeline.fit_pending = true;
    d.frames(4);
    d.settle();
    let width = d.rect("timeline.tracks")[2];
    let expected = (width as f64 * 0.94 / 902.0).clamp(0.05, 24000.0);
    assert!(
        (d.harness.state().ui.timeline.target_pps - expected).abs() < 1e-6,
        "Fit must include the actual 902s tail; expected{expected}, got{}",
        d.harness.state().ui.timeline.target_pps
    );
    assert_eq!(d.harness.state().session.active_sequence().unwrap().duration(), Tick(3 * TICKS_PER_SECOND));
    assert_eq!(*d.harness.state().session.project, before);
    assert_eq!(d.harness.state().session.history.undo.len(), depth);
    d.shot("poema-content-fit");
}
#[test]
fn long_tail_is_reachable_and_selectable_through_actual_control_without_a_project_edit() {
    let mut d = Driver::build(true);
    let before = (*d.harness.state().session.project).clone();
    let depth = d.harness.state().session.history.undo.len();
    d.ok("ui.set", json!({"timeline":{"pps":30.0,"scroll":898.0}}));
    d.frames(4);
    d.settle();
    assert_eq!(d.harness.state().ui.timeline.target_scroll, 898.0, "The program end +600s must not hide actual native media");
    let tracks = d.rect("timeline.tracks");
    let id = format!("timeline.clip.{}", d.tail.0);
    let clip = d.rect(&id);
    assert!(clip[2] > 20.0 && clip[0] >= tracks[0] && clip[0] + clip[2] <= tracks[0] + tracks[2], "tail{clip:?}, tracks{tracks:?}");
    d.ok("ui.click", json!({"id":id}));
    d.frames(2);
    assert_eq!(d.harness.state().session.state.selection, vec![d.tail]);
    assert_eq!(d.harness.state().session.active_sequence().unwrap().duration(), Tick(3 * TICKS_PER_SECOND));
    assert_eq!(*d.harness.state().session.project, before);
    assert_eq!(d.harness.state().session.history.undo.len(), depth);
    d.shot("poema-content-tail-selected");
}
#[test]
fn ordinary_native_sequence_keeps_its_existing_full_extent_navigation() {
    let mut d = Driver::build(false);
    assert_eq!(d.harness.state().session.active_sequence().unwrap().duration(), Tick(902 * TICKS_PER_SECOND));
    let before = (*d.harness.state().session.project).clone();
    d.ok("ui.set", json!({"timeline":{"pps":30.0,"scroll":898.0}}));
    d.frames(4);
    d.settle();
    assert_eq!(d.harness.state().ui.timeline.target_scroll, 898.0);
    let id = format!("timeline.clip.{}", d.tail.0);
    assert!(d.rect(&id)[2] > 20.0);
    assert_eq!(*d.harness.state().session.project, before);
}
