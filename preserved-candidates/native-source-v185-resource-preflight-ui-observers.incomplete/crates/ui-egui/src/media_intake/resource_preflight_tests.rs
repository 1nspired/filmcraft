use super::*;
use std::{
    fs,
    sync::atomic::{AtomicUsize, Ordering},
    time::{Duration, Instant},
};
fn app() -> FilmcraftApp {
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    let root = std::env::temp_dir().join(format!("poema-resource-intake-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
    fs::create_dir(&root).unwrap();
    let mut app = FilmcraftApp::new(filmcraft_engine::Session::default());
    app.media_intake = State::new(Some(root));
    app
}
#[test]
fn original_blocked_media_archive_refuses_resource_preflight_without_clearing_its_failure() {
    let mut app = app();
    require_idle_resource_writers(&app).unwrap();
    app.media_intake.blocked = true;
    assert!(require_idle_resource_writers(&app).is_err());
    assert!(app.media_intake.blocked);
    assert!(app.media_intake.retiring.is_none());
}
#[test]
fn real_original_archive_retirement_must_return_to_its_owner_before_resource_preflight_admission() {
    let mut app = app();
    let ctx = egui::Context::default();
    let reference = Reference {
        operation_id: "11111111-1111-4111-8111-111111111111".into(),
        native_session: "a".repeat(32),
        project_path: "/generated/recovery-gate.fcproj".into(),
    };
    app.media_intake.fences.insert(reference.operation_id.clone(), reference.clone());
    app.media_intake.hold_retirement_delivery = true;
    start_retirement(&mut app, &ctx, vec![Entry::Fence { reference }], None).unwrap();
    assert!(app.media_intake.retiring.is_some());
    assert!(require_idle_resource_writers(&app).is_err());
    app.media_intake.hold_retirement_delivery = false;
    let until = Instant::now() + Duration::from_secs(10);
    while app.media_intake.retiring.is_some() {
        assert!(Instant::now() < until);
        poll(&mut app, &ctx);
        std::thread::sleep(Duration::from_millis(1));
    }
    assert!(!app.media_intake.blocked);
    require_idle_resource_writers(&app).unwrap();
}
