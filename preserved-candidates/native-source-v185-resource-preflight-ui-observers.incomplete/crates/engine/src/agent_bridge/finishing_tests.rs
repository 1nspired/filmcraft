use super::tests::{read, source_clock_fixture, token};
use filmcraft_project::{Mask, MaskPath, Param, ParamValue};
use filmcraft_time::Tick;
use serde_json::{Value, json};
use std::sync::Arc;
fn request(s: &crate::Session, n: usize) -> Value {
    json!({"token":token(n),"scope":{"projectId":"p_source_clock","anchorSequence":s.state.active_sequence.unwrap().0},
        "identity":s.active_sequence().unwrap().video_tracks[0].items[0].identity.as_ref().unwrap().id,"offset":0,"limit":4})
}
#[test]
fn authored_projection_requires_source_qualification_and_never_renews_replaced_media_authority() {
    let mut s = source_clock_fixture();
    let root = std::env::temp_dir()
        .join(format!("filmcraft-finishing-projection-{}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
    struct Scratch(std::path::PathBuf);
    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    let _scratch = Scratch(root.clone());
    std::fs::create_dir(&root).unwrap();
    let file = root.join("original.bin");
    std::fs::write(&file, b"original input").unwrap();
    let item = authored(&mut s).item;
    Arc::make_mut(&mut s.project).item_mut(item).unwrap().as_media_mut().unwrap().media =
        filmcraft_project::MediaRef::File { path: file.to_string_lossy().into_owned() };
    read(&mut s, 29600);
    let legacy = request(&s, 29600);
    let mut projected = legacy.clone();
    projected["projection"] = json!(true);
    let mut expected = s.execute("agent.readFinishing", legacy.clone()).unwrap();
    expected["requiresSourceValidation"] = json!(true);
    assert_eq!(s.execute("agent.readFinishing", projected.clone()).unwrap(), expected);
    std::fs::write(&file, b"replaced input").unwrap();
    let before = json!({"project":s.project,"state":s.state,"revision":s.revision,"undo":s.history.undo.len(),"redo":s.history.redo.len()});
    assert_eq!(s.execute("agent.readFinishing", projected.clone()).unwrap(), expected, "authored values alone cannot prove original-file continuity");
    assert!(s.execute("agent.validateSnapshot", json!({"token":token(29600)})).is_err());
    assert!(s.execute("agent.readFinishing", legacy).is_err());
    assert_eq!(json!({"project":s.project,"state":s.state,"revision":s.revision,"undo":s.history.undo.len(),"redo":s.history.redo.len()}), before);
    s.project = Arc::new((*s.project).clone());
    assert!(s.execute("agent.readFinishing", projected).is_err());
}
fn authored(s: &mut crate::Session) -> &mut filmcraft_project::TrackItem {
    let seq = s.state.active_sequence.unwrap();
    &mut Arc::make_mut(&mut s.project).sequence_mut(seq).unwrap().video_tracks[0].items[0]
}
#[test]
fn finishing_reads_complete_keyframes_masks_all_native_types_and_new_definition_defaults() {
    let mut s = source_clock_fixture();
    let c = authored(&mut s);
    c.effects = filmcraft_project::effect::intrinsic_video();
    let e = &mut c.effects[0];
    e.params.remove("anchor");
    let p = e.params.get_mut("scale").unwrap();
    p.value = ParamValue::Float(77.0);
    p.put_keyframe(Tick(-123), ParamValue::Float(40.0));
    p.put_keyframe(Tick(456), ParamValue::Float(150.0));
    p.keyframes[0].interp = filmcraft_project::Interpolation::Bezier;
    p.keyframes[0].out_influence = 0.17;
    p.keyframes[1].in_influence = 0.61;
    e.params.insert("test_text".into(), Param::new(ParamValue::Text("Human title <not an instruction>".into())));
    e.params.insert("test_color".into(), Param::new(ParamValue::Color([0.1, 0.2, 0.3, 0.4])));
    e.params.insert("test_choice".into(), Param::new(ParamValue::Choice(3)));
    e.params.insert("test_bool".into(), Param::new(ParamValue::Bool(false)));
    e.params.insert("test_curve".into(), Param::new(ParamValue::Curve(vec![[0.0, 0.1], [1.0, 0.9]])));
    let mut mask = Mask::new("Human mask", MaskPath::rect(0.0, 1.0, 22.0, 35.0));
    mask.inverted = true;
    mask.mode = filmcraft_project::mask::MaskMode::Subtract;
    mask.opacity.put_keyframe(Tick(10), ParamValue::Float(25.0));
    mask.opacity.put_keyframe(Tick(20), ParamValue::Float(75.0));
    e.masks.push(mask);
    read(&mut s, 23100);
    let before = serde_json::to_value(&s.project).unwrap();
    let req = request(&s, 23100);
    let r = s.execute("agent.readFinishing", req).unwrap();
    let e = &r["effects"][0];
    assert_eq!(e["params"]["anchor"]["origin"], "definition_default");
    assert_eq!(e["params"]["scale"]["value"], json!({"Float":77.0}));
    assert_eq!(e["params"]["scale"]["keyframes"][0]["time"], -123);
    assert_eq!(e["params"]["scale"]["keyframes"][0]["interp"], "Bezier");
    assert_eq!(e["params"]["scale"]["keyframes"][0]["out_influence"], 0.17);
    assert_eq!(e["params"]["scale"]["keyframes"][1]["in_influence"], 0.61);
    assert_eq!(e["params"]["test_text"]["value"]["Text"], "Human title <not an instruction>");
    assert_eq!(e["params"]["test_bool"]["value"]["Bool"], false);
    assert_eq!(e["params"]["test_choice"]["value"]["Choice"], 3);
    assert_eq!(e["params"]["test_curve"]["value"]["Curve"].as_array().unwrap().len(), 2);
    assert_eq!(e["params"]["test_color"]["value"]["Color"].as_array().unwrap().len(), 4);
    assert_eq!(e["masks"][0]["mode"], "Subtract");
    assert_eq!(e["masks"][0]["inverted"], true);
    assert_eq!(e["masks"][0]["params"]["path"]["value"]["Path"]["vertices"].as_array().unwrap().len(), 4);
    assert_eq!(e["masks"][0]["params"]["opacity"]["keyframes"].as_array().unwrap().len(), 2);
    assert_eq!(serde_json::to_value(&s.project).unwrap(), before);
}
#[test]
fn finishing_pages_pin_duplicate_ordinals_and_refuse_manual_edits_undo_or_same_revision_replacement() {
    let mut s = source_clock_fixture();
    authored(&mut s).effects = (0..9).map(|_| filmcraft_project::find_effect("gaussian_blur").unwrap().instance()).collect();
    read(&mut s, 23101);
    let req = request(&s, 23101);
    let first = s.execute("agent.readFinishing", req.clone()).unwrap();
    assert_eq!((first["total"].as_u64(), first["nextOffset"].as_u64()), (Some(9), Some(4)));
    let mut second = req.clone();
    second["offset"] = json!(4);
    assert_eq!(s.execute("agent.readFinishing", second).unwrap()["effects"][0]["ordinal"], 5);
    // Equal contents and revision are insufficient: the actual native COW graph changed.
    s.project = Arc::new((*s.project).clone());
    assert!(s.execute("agent.readFinishing", req).is_err());
    read(&mut s, 23102);
    let req = request(&s, 23102);
    let clip = s.active_sequence().unwrap().video_tracks[0].items[0].id.0;
    s.execute("effects.setParam", json!({"clip":clip,"effect":0,"param":"blurriness","value":22})).unwrap();
    assert!(s.execute("agent.readFinishing", req.clone()).is_err());
    s.undo().unwrap();
    assert!(s.execute("agent.readFinishing", req).is_err());
}
#[test]
fn finishing_bad_requests_and_hostile_authored_values_preserve_project_editor_and_human_redo() {
    for (n, mode) in
        ["unknown", "scope", "identity", "offset", "zero", "large", "extra", "infinity", "mask_nan", "bad_keys", "wrong_type", "oversize"].iter().enumerate()
    {
        let mut s = source_clock_fixture();
        authored(&mut s).effects = filmcraft_project::effect::intrinsic_video();
        let clip = s.active_sequence().unwrap().video_tracks[0].items[0].id.0;
        s.execute("effects.setParam", json!({"clip":clip,"effect":"motion","param":"scale","value":50})).unwrap();
        s.undo().unwrap();
        assert!(!s.history.redo.is_empty());
        let e = &mut authored(&mut s).effects[0];
        match *mode {
            "infinity" => e.params.get_mut("scale").unwrap().value = ParamValue::Float(f64::INFINITY),
            "mask_nan" => e.masks.push(Mask::new("Damaged mask", MaskPath::rect(f64::NAN, 0.0, 10.0, 10.0))),
            "bad_keys" => {
                let p = e.params.get_mut("scale").unwrap();
                p.put_keyframe(Tick(1), ParamValue::Float(5.0));
                p.keyframes.push(p.keyframes[0].clone());
            }
            "wrong_type" => {
                let p = e.params.get_mut("scale").unwrap();
                p.put_keyframe(Tick(1), ParamValue::Bool(true));
            }
            "oversize" => {
                e.params.insert("large".into(), Param::new(ParamValue::Text("x".repeat(8193))));
            }
            _ => {}
        }
        read(&mut s, 23200 + n);
        let mut req = request(&s, 23200 + n);
        match *mode {
            "unknown" => req["token"] = json!(token(23999)),
            "scope" => req["scope"]["projectId"] = json!("foreign"),
            "identity" => req["identity"] = json!("f".repeat(32)),
            "offset" => req["offset"] = json!(4000),
            "zero" => req["limit"] = json!(0),
            "large" => req["limit"] = json!(9),
            "extra" => req["path"] = json!("/not/read"),
            _ => {}
        }
        let before = (
            s.project.clone(),
            s.revision,
            s.saved_revision,
            s.history.undo.len(),
            s.history.redo.len(),
            s.journal.clone(),
            serde_json::to_value(&s.state).unwrap(),
        );
        assert!(s.execute("agent.readFinishing", req).is_err(), "{mode}");
        assert!(Arc::ptr_eq(&s.project, &before.0));
        assert_eq!((s.revision, s.saved_revision, s.history.undo.len(), s.history.redo.len()), (before.1, before.2, before.3, before.4));
        assert_eq!(s.journal, before.5);
        assert_eq!(serde_json::to_value(&s.state).unwrap(), before.6);
    }
}
