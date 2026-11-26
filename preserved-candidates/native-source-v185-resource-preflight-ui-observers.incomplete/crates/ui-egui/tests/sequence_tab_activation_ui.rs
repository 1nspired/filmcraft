//! Generated headless acceptance for the shared original Timeline-tab service.
use egui::{Event, Modifiers, PointerButton, Rect, pos2, vec2};
use egui_kittest::Harness;
use filmcraft_engine::Session;
use filmcraft_project::ItemId;
use filmcraft_ui_egui::{
    FilmcraftApp,
    control::{self, ControlRequest, Outcome},
    dock::{DockNode, PanelKind},
    menus,
};
use serde_json::{Value, json};
use std::sync::Arc;

fn app() -> (FilmcraftApp, ItemId, ItemId) {
    let mut s = Session::default();
    s.execute("file.openDemoProject", json!({})).unwrap();
    let main = s.state.active_sequence.unwrap();
    s.execute("file.newSequence", json!({"name":"Other tab","video":0,"audio":0})).unwrap();
    let other = s.state.active_sequence.unwrap();
    s.execute("sequence.open", json!({"item":main.0})).unwrap();
    s.events.clear();
    let mut app = FilmcraftApp::new(s);
    app.ui.dock = DockNode::Tabs { panels: vec![PanelKind::Timeline, PanelKind::Program], active: 1 };
    app.ui.floating.clear();
    let clip = app.session.active_sequence().unwrap().video_tracks[0].items[0].id;
    app.session.state.selection = vec![clip];
    app.ui.timeline.pps = 73.;
    app.ui.timeline.target_pps = 73.;
    app.ui.timeline.scroll = 0.5;
    app.ui.timeline.target_scroll = 0.5;
    (app, main, other)
}
fn activate(app: &mut FilmcraftApp, ctx: &egui::Context, item: ItemId) -> Value {
    let (request, _) = ControlRequest::new("engine.execute", json!({"command":"window.activateSequenceTab","params":{"item":item.0}}));
    match control::handle(app, ctx, &request) {
        Outcome::Done(v) => {
            assert_eq!(v["ok"], true, "{v}");
            v["result"].clone()
        }
        _ => panic!("tab activation unexpectedly deferred"),
    }
}
fn history(app: &FilmcraftApp) -> Value {
    json!({"undo":app.session.history.undo,"redo":app.session.history.redo,"limit":app.session.history.limit,"merge_key":app.session.history.merge_key})
}
fn unchanged(app: &FilmcraftApp) -> Value {
    json!({"editor":app.session.state,"project":app.session.project,"history":history(app),"revision":app.session.revision,"saved":app.session.saved_revision,"journal":app.session.journal,"timeline":app.ui.timeline})
}
fn step(h: &mut Harness<'_, FilmcraftApp>) {
    let ctx = h.ctx.clone();
    let mut input = std::mem::take(h.input_mut());
    eframe::App::raw_input_hook(h.state_mut(), &ctx, &mut input);
    *h.input_mut() = input;
    h.step();
}
fn click(h: &mut Harness<'_, FilmcraftApp>, item: ItemId) {
    let e = h.state().auto.find(&format!("timeline.tab.{}", item.0)).unwrap();
    let pos = Rect::from_min_size(pos2(e.rect[0], e.rect[1]), vec2(e.rect[2], e.rect[3])).center();
    h.event(Event::PointerMoved(pos));
    step(h);
    for pressed in [true, false] {
        h.event(Event::PointerButton { pos, button: PointerButton::Primary, pressed, modifiers: Modifiers::NONE });
        step(h);
    }
}
#[test]
fn active_tab_control_keeps_complete_editor_project_history_journal_and_view() {
    let (mut a, main, _) = app();
    let ctx = egui::Context::default();
    let before = unchanged(&a);
    let project = a.session.project.clone();
    let events = a.session.events.len();
    let r = activate(&mut a, &ctx, main);
    assert_eq!(r, json!({"sequence":main.0,"switched":false,"timelineActivationRequested":true,"keyboardFocusRequested":false}));
    assert_eq!(unchanged(&a), before);
    assert!(Arc::ptr_eq(&project, &a.session.project));
    assert_eq!(a.session.events.len(), events);
    assert!(a.ui.dock.is_visible(PanelKind::Timeline));
}
#[test]
fn active_tab_keeps_each_live_trim_and_loop_without_settlement_or_new_history() {
    for mode in ["zero", "moved", "loop"] {
        let (mut a, main, _) = app();
        let ctx = egui::Context::default();
        let clip = a.session.state.selection[0];
        a.session.execute("trim.selectEditPoint", json!({"clip":clip.0,"edge":"out","kind":"roll"})).unwrap();
        if mode == "loop" {
            a.session.execute("trim.playAround", json!({"clock":0})).unwrap();
        } else {
            a.session.execute("trim.shuttle", json!({"direction":"forward","clock":0})).unwrap();
            if mode == "moved" {
                a.session.execute("trim.tick", json!({"clock":0.25})).unwrap();
            }
        }
        assert!(a.session.trim_play.active());
        let before = unchanged(&a);
        let trim = format!("{:?}", a.session.trim_play);
        let events = a.session.events.len();
        assert_eq!(activate(&mut a, &ctx, main)["switched"], false);
        assert_eq!(unchanged(&a), before);
        assert_eq!(format!("{:?}", a.session.trim_play), trim);
        assert_eq!(a.session.events.len(), events);
    }
}
#[test]
fn switching_tab_runs_original_sequence_open_once_and_retains_original_command_behavior() {
    let (mut a, main, other) = app();
    let ctx = egui::Context::default();
    let before = a.session.project.clone();
    let revision = a.session.revision;
    let journal = a.session.journal.len();
    let history_before = history(&a);
    let r = activate(&mut a, &ctx, other);
    assert_eq!(r["switched"], true);
    assert_eq!(a.session.state.active_sequence, Some(other));
    assert!(a.session.state.selection.is_empty());
    assert!(Arc::ptr_eq(&before, &a.session.project));
    assert_eq!(a.session.revision, revision);
    assert_eq!(history(&a), history_before);
    assert_eq!(a.session.journal.len(), journal + 1);
    assert_eq!(a.session.journal.last().unwrap().0, "sequence.open");
    a.session.execute("sequence.open", json!({"item":main.0})).unwrap();
    let clip = a.session.active_sequence().unwrap().video_tracks[0].items[0].id;
    a.session.state.selection = vec![clip];
    activate(&mut a, &ctx, main);
    assert_eq!(a.session.state.selection, vec![clip]);
    a.session.execute("sequence.open", json!({"item":main.0})).unwrap();
    assert!(a.session.state.selection.is_empty());
}
#[test]
fn hostile_closed_and_nonsequence_tabs_refuse_before_docking_or_document_changes() {
    let (mut a, main, other) = app();
    let ctx = egui::Context::default();
    a.session.execute("sequence.close", json!({"item":other.0})).unwrap();
    let media = a.session.active_sequence().unwrap().video_tracks[0].items[0].item;
    for params in [
        json!(null),
        json!([]),
        json!({}),
        json!({"item":0}),
        json!({"item":-1}),
        json!({"item":1.5}),
        json!({"item":"21"}),
        json!({"item":true}),
        json!({"item":u64::MAX}),
        json!({"item":main.0,"extra":true}),
        json!({"item":other.0}),
        json!({"item":media.0}),
    ] {
        let before = unchanged(&a);
        let dock = a.ui.dock.clone();
        let floating = a.ui.floating.clone();
        assert!(menus::invoke(&mut a, &ctx, "window.activateSequenceTab", params).is_err());
        assert_eq!(unchanged(&a), before);
        assert_eq!(a.ui.dock, dock);
        assert_eq!(a.ui.floating, floating);
    }
}
#[test]
fn failed_original_dock_transaction_does_not_switch_sequence_or_clear_selection() {
    let (mut a, _, other) = app();
    let ctx = egui::Context::default();
    a.ui.dock = DockNode::Tabs { panels: vec![PanelKind::Timeline, PanelKind::Timeline], active: 0 };
    let before = unchanged(&a);
    let dock = a.ui.dock.clone();
    assert!(menus::invoke(&mut a, &ctx, "window.activateSequenceTab", json!({"item":other.0})).is_err());
    assert_eq!(unchanged(&a), before);
    assert_eq!(a.ui.dock, dock);
}
#[test]
fn floating_timeline_activation_preserves_geometry_and_one_panel_owner() {
    let (mut a, main, _) = app();
    let ctx = egui::Context::default();
    menus::invoke(&mut a, &ctx, "window.floatPanel", json!({"panel":"Timeline","rect":[45,65,420,320]})).unwrap();
    let floating = a.ui.floating.clone();
    let dock = a.ui.dock.clone();
    let before = unchanged(&a);
    activate(&mut a, &ctx, main);
    assert_eq!(a.ui.floating, floating);
    assert_eq!(a.ui.dock, dock);
    assert_eq!(unchanged(&a), before);
    assert!(!a.ui.dock.contains(PanelKind::Timeline));
    assert_eq!(a.ui.floating.iter().filter(|g| g.panels.contains(&PanelKind::Timeline)).count(), 1);
}
#[test]
fn real_original_tab_pointer_and_control_share_active_and_switch_behavior() {
    for switched in [false, true] {
        let (mut a, main, other) = app();
        a.ui.dock = DockNode::Tabs { panels: vec![PanelKind::Timeline], active: 0 };
        let mut h = Harness::builder().with_size(vec2(1000., 700.)).build_eframe(move |_| a);
        for _ in 0..4 {
            step(&mut h);
        }
        let clip = h.state().session.active_sequence().unwrap().video_tracks[0].items[0].id;
        h.state_mut().session.state.selection = vec![clip];
        let before = h.state().session.journal.len();
        let target = if switched { other } else { main };
        click(&mut h, target);
        assert_eq!(h.state().session.state.active_sequence, Some(target));
        assert_eq!(h.state().session.journal.len(), before + usize::from(switched));
        assert_eq!(h.state().session.state.selection.is_empty(), switched);
        let ctx = h.ctx.clone();
        let before = h.state().session.journal.len();
        assert_eq!(activate(h.state_mut(), &ctx, target)["switched"], false);
        assert_eq!(h.state().session.journal.len(), before);
    }
}
