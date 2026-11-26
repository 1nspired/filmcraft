use super::*;
use filmcraft_engine::Session;

fn app() -> FilmcraftApp {
    let mut s = Session::default();
    s.execute("file.openDemoProject", json!({})).unwrap();
    FilmcraftApp::new(s)
}
fn clip(app: &FilmcraftApp) -> ClipId {
    app.session.active_sequence().unwrap().video_tracks[0].items[0].id
}

#[test]
fn notes_protection_and_unlock_are_independent_undoable_actions() {
    let mut a = app();
    let c = clip(&a);
    open(&mut a, &json!({"clip": c.0})).unwrap();
    let mut d = a.poema.take().unwrap();
    action(&mut a, &mut d, "poema.trackSequence").unwrap();
    a.session.execute("poema.annotateClips", json!({"clips":[c.0], "annotations":{"selectId":"sel_keep"}})).unwrap();
    d = capture(&a, c).unwrap();
    d.note = "Keep this reaction".into();
    d.reason = "My choice".into();
    let h = a.session.history.undo.len();
    action(&mut a, &mut d, "poema.annotateClips").unwrap();
    assert_eq!(a.session.history.undo.len(), h + 1);
    assert_eq!(d.annotations["selectId"], "sel_keep");
    assert_eq!(d.annotations["editor_note"], "Keep this reaction");
    assert!(d.protection.is_none());
    assert_eq!(d.reason, "My choice");
    action(&mut a, &mut d, "poema.protectClips").unwrap();
    assert_eq!(d.protection.as_deref(), Some("My choice"));
    action(&mut a, &mut d, "unlock").unwrap();
    assert!(d.protection.is_none());
    a.session.execute("edit.undo", json!({})).unwrap();
    assert_eq!(capture(&a, c).unwrap().protection.as_deref(), Some("My choice"));
}

#[test]
fn stale_draft_never_saves_over_an_intervening_edit_or_replacement_project() {
    let mut a = app();
    let c = clip(&a);
    let mut d = capture(&a, c).unwrap();
    action(&mut a, &mut d, "poema.trackSequence").unwrap();
    d.note = "Unsaved".into();
    a.session.execute("timeline.trim", json!({"clip":c.0,"edge":"out","deltaFrames":-1})).unwrap();
    let before = a.session.project.clone();
    assert!(action(&mut a, &mut d, "poema.annotateClips").is_err());
    assert!(Arc::ptr_eq(&before, &a.session.project));
    a.session.execute("file.openDemoProject", json!({})).unwrap();
    assert!(action(&mut a, &mut d, "poema.trackSequence").is_err());
}

#[test]
fn draft_automation_cannot_retarget_or_commit_and_rejects_bad_input_atomically() {
    let mut a = app();
    let c = clip(&a);
    open(&mut a, &json!({"clip": c.0})).unwrap();
    let before = a.session.project.clone();
    assert!(patch(&mut a, &json!({"note":"do not accept", "clip":9999})).is_err());
    assert_eq!(a.poema.as_ref().unwrap().note, "");
    assert!(patch(&mut a, &json!({"reason":"🟠".repeat(300)})).is_err());
    patch(&mut a, &json!({"note":"hello","reason":"keep"})).unwrap();
    assert!(Arc::ptr_eq(&before, &a.session.project));
    a.poema = None; // Close/Cancel drops the ephemeral draft, not a project mutation.
    assert!(Arc::ptr_eq(&before, &a.session.project));
}

#[test]
fn menu_selection_accepts_one_linked_picture_group_but_not_unrelated_clips() {
    let mut a = app();
    let c = clip(&a);
    a.session.execute("timeline.select", json!({"clips":[c.0],"linked":true})).unwrap();
    open(&mut a, &json!({})).unwrap();
    assert_eq!(a.poema.as_ref().unwrap().clip, c);
    a.session.state.selection.clear();
    assert!(open(&mut a, &json!({})).is_err());
    assert!(open(&mut a, &json!({"clip":c.0,"reason":"no"})).is_err());
}
