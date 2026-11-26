use crate::Session;
use filmcraft_project::ClipId;
use serde_json::{Value, json};

fn sequence() -> Session {
    let mut s = Session::default();
    s.execute("file.newSequence", json!({"name":"Graphics validation","fps":24,"width":128,"height":128,"video":2,"audio":1})).unwrap();
    s
}

fn state(s: &Session) -> Value {
    json!({"project":s.project,"editor":s.state,"revision":s.revision,
        "undo":s.history.undo,"redo":s.history.redo,"merge":s.history.merge_key,"journal":s.journal})
}

fn redo_branch(s: &mut Session) {
    s.execute("graphics.newShape", json!({"shape":"ellipse","frame":240,"track":1})).unwrap();
    s.execute("edit.undo", json!({})).unwrap();
    assert!(!s.history.redo.is_empty());
    assert!(!s.history.undo.is_empty());
}

fn reject_unchanged(s: &mut Session, command: &str, params: Value) {
    let before = state(s);
    let result = s.execute(command, params.clone());
    assert!(result.is_err(), "{command} accepted {params}: {result:?}");
    assert_eq!(state(s), before, "rejected {command} changed state/history for {params}");
}

fn layout_fixture() -> (Session, u64) {
    let mut s = sequence();
    let clip = s.execute("graphics.newShape", json!({"shape":"rectangle","position":[20,20],"size":[12,10],"track":0})).unwrap()["clip"].as_u64().unwrap();
    s.execute("graphics.newShape", json!({"clip":clip,"shape":"rectangle","position":[60,50],"size":[16,18]})).unwrap();
    s.execute("graphics.newShape", json!({"clip":clip,"shape":"ellipse","position":[100,90],"size":[20,24]})).unwrap();
    s.state.graphic_layers = vec![0, 1, 2];
    (s, clip)
}

#[test]
fn graphics_layout_hostile_layer_lists_are_atomic_and_never_drop_entries() {
    for layers in [
        json!([0, -1, 2]),
        json!([0, 0.5, 2]),
        json!([0, "1", 2]),
        json!([0, null, 2]),
        json!([0, true, 2]),
        json!([0, 0, 2]),
        json!([]),
        json!(null),
        json!(false),
        json!([0, 1, 99]),
    ] {
        for command in ["graphics.align", "graphics.distribute"] {
            let (mut s, clip) = layout_fixture();
            redo_branch(&mut s);
            reject_unchanged(&mut s, command, json!({"clip":clip,"layers":layers,"align":"left","axis":"horizontal"}));
        }
    }
}

#[test]
fn graphics_layout_bad_options_and_explicit_clocks_never_default_or_edit() {
    let (mut s, clip) = layout_fixture();
    redo_branch(&mut s);
    for value in [json!(null), json!(false), json!(1), json!("unknown")] {
        reject_unchanged(&mut s, "graphics.align", json!({"clip":clip,"layers":[0,1],"align":"left","to":value}));
        reject_unchanged(&mut s, "graphics.distribute", json!({"clip":clip,"layers":[0,1,2],"axis":value}));
    }
    for value in [json!(null), json!(1), json!("false")] {
        reject_unchanged(&mut s, "graphics.distribute", json!({"clip":clip,"layers":[0,1,2],"axis":"horizontal","space":value}));
    }
    for time in [json!(-1), json!(0.5), json!(null), json!("1"), json!(true)] {
        reject_unchanged(&mut s, "graphics.align", json!({"clip":clip,"layers":[0,1],"align":"left","time":time}));
        reject_unchanged(&mut s, "graphics.distribute", json!({"clip":clip,"layers":[0,1,2],"axis":"horizontal","time":time}));
    }
}

#[test]
fn graphics_layout_locked_targets_preserve_both_history_branches() {
    let (mut s, clip) = layout_fixture();
    s.execute("timeline.setTrack", json!({"track":"V1","locked":true})).unwrap();
    redo_branch(&mut s);
    s.state.selection = vec![ClipId(clip)];
    s.state.graphic_layers = vec![0, 1, 2];
    for command in [
        "graphics.align",
        "graphics.alignFrame.left",
        "graphics.alignGroup.top",
        "graphics.alignSelection.bottom",
        "graphics.distribute",
        "graphics.distributeHorizontally",
    ] {
        reject_unchanged(&mut s, command, json!({"clip":clip,"layers":[0,1,2],"align":"left","axis":"horizontal"}));
    }
}

fn layout_copy(s: &Session) -> Session {
    let mut other = Session::new(s.services.clone());
    other.project = s.project.clone();
    other.state = s.state.clone();
    other.media = s.media.clone();
    other.prefs = s.prefs.clone();
    other
}

fn pin_boxes(s: &Session, clip: u64) -> Vec<[f64; 4]> {
    use filmcraft_render::graphic_clip::{item_layer_specs, layer_bounds};
    let c = s.active_sequence().unwrap().find_item(ClipId(clip)).unwrap().1;
    item_layer_specs(c, filmcraft_time::Tick::ZERO, (128, 128)).iter().map(|(_, spec)| layer_bounds(spec)).collect()
}

fn assert_shift(before: [f64; 4], after: [f64; 4], dx: f64, dy: f64) {
    for edge in 0..4 {
        let expected = before[edge] + if edge % 2 == 0 { dx } else { dy };
        assert!((after[edge] - expected).abs() < 1e-4, "edge{edge}: {before:?} → {after:?}, expected delta({dx},{dy})");
    }
}

#[test]
fn graphics_layout_pinned_group_moves_once_with_exact_owned_human_history() {
    for how in ["left", "hcenter", "right", "top", "vcenter", "bottom"] {
        for order in [vec![0, 1, 2], vec![2, 1, 0]] {
            let (mut s, clip) = layout_fixture();
            s.execute("graphics.pin", json!({"clip":clip,"layer":0,"to":1})).unwrap();
            let before = s.project.clone();
            let view = serde_json::to_value(&s.state).unwrap();
            let boxes = pin_boxes(&s, clip);
            let bounds = boxes.iter().fold([f64::MAX, f64::MAX, f64::MIN, f64::MIN], |a, b| [a[0].min(b[0]), a[1].min(b[1]), a[2].max(b[2]), a[3].max(b[3])]);
            let (dx, dy) = match how {
                "left" => (-bounds[0], 0.0),
                "hcenter" => (64.0 - (bounds[0] + bounds[2]) / 2.0, 0.0),
                "right" => (128.0 - bounds[2], 0.0),
                "top" => (0.0, -bounds[1]),
                "vcenter" => (0.0, 64.0 - (bounds[1] + bounds[3]) / 2.0),
                _ => (0.0, 128.0 - bounds[3]),
            };
            let history = s.history.undo.len();
            s.execute("graphics.align", json!({"clip":clip,"layers":order,"align":how,"to":"group","time":0})).unwrap();
            for (a, b) in boxes.iter().zip(pin_boxes(&s, clip)) {
                assert_shift(*a, b, dx, dy);
            }
            assert_eq!(s.history.undo.len(), history + 1);
            assert_eq!(serde_json::to_value(&s.state).unwrap(), view);
            let after = s.project.clone();
            s.execute("edit.undo", json!({})).unwrap();
            assert_eq!(*s.project, *before);
            s.execute("edit.redo", json!({})).unwrap();
            assert_eq!(*s.project, *after);
        }
    }
}

#[test]
fn graphics_layout_pins_follow_unselected_intermediates_without_double_movement() {
    let (mut s, clip) = layout_fixture();
    s.execute("graphics.pin", json!({"clip":clip,"layer":0,"to":1})).unwrap();
    s.execute("graphics.pin", json!({"clip":clip,"layer":1,"to":2})).unwrap();
    let original = s.project.clone();
    let before = pin_boxes(&s, clip);
    let dx = -before[0][0].min(before[2][0]);
    let item = s.active_sequence().unwrap().find_item(ClipId(clip)).unwrap().1;
    let unselected_index = filmcraft_project::graphic::layer_indices(&item.effects)[1];
    let unselected = item.effects[unselected_index].clone();
    s.execute("graphics.align", json!({"clip":clip,"layers":[0,2],"align":"left","to":"group","time":0})).unwrap();
    for (a, b) in before.iter().zip(pin_boxes(&s, clip)) {
        assert_shift(*a, b, dx, 0.0);
    }
    assert_eq!(s.active_sequence().unwrap().find_item(ClipId(clip)).unwrap().1.effects[unselected_index], unselected);
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(*s.project, *original);
}

#[test]
fn graphics_layout_pinned_individual_alignment_and_distribution_reach_requested_edges() {
    for (mode, how) in ["frame", "selection"].into_iter().flat_map(|mode| ["left", "hcenter", "right", "top", "vcenter", "bottom"].map(|how| (mode, how))) {
        let (mut s, clip) = layout_fixture();
        s.execute("graphics.pin", json!({"clip":clip,"layer":0,"to":1})).unwrap();
        s.execute("graphics.pin", json!({"clip":clip,"layer":1,"to":2,"edges":["right","bottom"]})).unwrap();
        let before = pin_boxes(&s, clip);
        let target = if mode == "frame" {
            [0.0, 0.0, 128.0, 128.0]
        } else {
            before.iter().fold([f64::MAX, f64::MAX, f64::MIN, f64::MIN], |a, b| [a[0].min(b[0]), a[1].min(b[1]), a[2].max(b[2]), a[3].max(b[3])])
        };
        s.execute("graphics.align", json!({"clip":clip,"layers":[2,0,1],"align":how,"to":mode,"time":0})).unwrap();
        for (a, b) in before.iter().zip(pin_boxes(&s, clip)) {
            let (dx, dy) = match how {
                "left" => (target[0] - a[0], 0.0),
                "hcenter" => ((target[0] + target[2] - a[0] - a[2]) / 2.0, 0.0),
                "right" => (target[2] - a[2], 0.0),
                "top" => (0.0, target[1] - a[1]),
                "vcenter" => (0.0, (target[1] + target[3] - a[1] - a[3]) / 2.0),
                _ => (0.0, target[3] - a[3]),
            };
            assert_shift(*a, b, dx, dy);
        }
    }
    for (axis, space) in [("horizontal", false), ("horizontal", true), ("vertical", false), ("vertical", true)] {
        let (mut s, clip) = layout_fixture();
        s.execute("graphics.set", json!({"clip":clip,"layer":1,"props":{"position":[78,60]},"merge":false,"begin":false})).unwrap();
        s.execute("graphics.pin", json!({"clip":clip,"layer":0,"to":1})).unwrap();
        let before = pin_boxes(&s, clip);
        s.execute("graphics.distribute", json!({"clip":clip,"layers":[0,1,2],"axis":axis,"space":space,"time":0})).unwrap();
        let after = pin_boxes(&s, clip);
        let (lo, hi) = if axis == "horizontal" { (0, 2) } else { (1, 3) };
        assert_shift(before[0], after[0], 0.0, 0.0);
        assert_shift(before[2], after[2], 0.0, 0.0);
        if space {
            assert!(((after[1][lo] - after[0][hi]) - (after[2][lo] - after[1][hi])).abs() < 1e-4);
        } else {
            let centers: Vec<_> = after.iter().map(|b| (b[lo] + b[hi]) / 2.0).collect();
            assert!((centers[1] - (centers[0] + centers[2]) / 2.0).abs() < 1e-4);
        }
    }
}

#[test]
fn graphics_pin_malformed_target_and_edges_never_choose_layer_zero_or_change_history() {
    let (mut s, clip) = layout_fixture();
    redo_branch(&mut s);
    for target in [json!(-1), json!(0.5), json!(18446744073709551615_u64), json!(null), json!(false)] {
        reject_unchanged(&mut s, "graphics.pin", json!({"clip":clip,"layer":1,"to":target}));
    }
    for edges in [
        json!(null),
        json!(false),
        json!(2),
        json!(["left", false]),
        json!(["left", "typo"]),
        json!(["top", "TOP"]),
        json!("lef"),
        json!(["left", "top", "right", "bottom", "left"]),
    ] {
        reject_unchanged(&mut s, "graphics.pin", json!({"clip":clip,"layer":1,"to":0,"edges":edges}));
    }
}

#[test]
fn graphics_pin_cycles_locked_tracks_and_ambiguous_ids_refuse_without_borrowing_history() {
    let (mut s, clip) = layout_fixture();
    s.execute("graphics.pin", json!({"clip":clip,"layer":0,"to":1})).unwrap();
    s.execute("graphics.pin", json!({"clip":clip,"layer":1,"to":2})).unwrap();
    redo_branch(&mut s);
    reject_unchanged(&mut s, "graphics.pin", json!({"clip":clip,"layer":2,"to":0}));
    s.execute("timeline.setTrack", json!({"track":"V1","locked":true})).unwrap();
    redo_branch(&mut s);
    reject_unchanged(&mut s, "graphics.pin", json!({"clip":clip,"layer":0,"to":"frame"}));
    let (mut s, clip) = layout_fixture();
    let sid = s.state.active_sequence.unwrap();
    let c = std::sync::Arc::make_mut(&mut s.project).sequence_mut(sid).unwrap().find_item_mut(ClipId(clip)).unwrap().1;
    for e in &mut c.effects {
        e.layer.get_or_insert_with(Default::default).uid = 1;
    }
    redo_branch(&mut s);
    reject_unchanged(&mut s, "graphics.pin", json!({"clip":clip,"layer":0,"to":1}));
}

#[test]
fn graphics_layout_responsive_keyframes_use_the_same_native_design_clock_as_the_picture() {
    use filmcraft_geom::Vec2;
    use filmcraft_project::{ParamValue, graphic_design::GraphicMeta};
    use filmcraft_render::graphic_clip::{item_layer_specs, layer_bounds};
    use filmcraft_time::Tick;
    let (mut s, clip) = layout_fixture();
    let sid = s.state.active_sequence.unwrap();
    let rate = s.sequence_rate();
    let c = std::sync::Arc::make_mut(&mut s.project).sequence_mut(sid).unwrap().find_item_mut(ClipId(clip)).unwrap().1;
    c.duration = rate.tick_of(240);
    c.graphic = Some(Box::new(GraphicMeta { intro: rate.tick_of(24), outro: rate.tick_of(24), design_duration: rate.tick_of(120), ..Default::default() }));
    let effect = filmcraft_project::graphic::layer_indices(&c.effects)[0];
    let p = c.effects[effect].params.get_mut("position").unwrap();
    p.keyframes.clear();
    p.put_keyframe(Tick::ZERO, ParamValue::Vec2(Vec2::new(20.0, 20.0)));
    p.put_keyframe(rate.tick_of(120), ParamValue::Vec2(Vec2::new(110.0, 40.0)));
    let at = rate.tick_of(120);
    let c = s.active_sequence().unwrap().find_item(ClipId(clip)).unwrap().1;
    let (design, _, _) = filmcraft_render::graphic_clip::graphic_time(c, at);
    assert_ne!(design, at);
    let before = serde_json::to_value(&s.state).unwrap();
    let original = s.project.clone();
    s.execute("graphics.align", json!({"clip":clip,"layers":[0],"align":"left","to":"frame","time":at.0})).unwrap();
    let c = s.active_sequence().unwrap().find_item(ClipId(clip)).unwrap().1;
    let b = layer_bounds(&item_layer_specs(c, at, (128, 128))[0].1);
    assert!(b[0].abs() < 1e-4, "{b:?}");
    let p = c.effects[effect].params.get("position").unwrap();
    assert!(p.keyframes.iter().any(|k| k.time == design));
    assert!(
        p.keyframes.iter().any(|k| k.time == at && k.value == ParamValue::Vec2(Vec2::new(110.0, 40.0))),
        "The existing later design keyframe must be preserved."
    );
    assert_eq!(serde_json::to_value(&s.state).unwrap(), before);
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(*s.project, *original);
}

#[test]
fn graphics_pin_preparation_uses_explicit_native_frame_without_seeking_and_refuses_a_replaced_basis() {
    use filmcraft_geom::Vec2;
    use filmcraft_project::ParamValue;
    use filmcraft_render::graphic_clip::{item_layer_specs, layer_bounds};
    use filmcraft_time::Tick;
    let (mut s, clip) = layout_fixture();
    let sid = s.state.active_sequence.unwrap();
    let rate = s.sequence_rate();
    let c = std::sync::Arc::make_mut(&mut s.project).sequence_mut(sid).unwrap().find_item_mut(ClipId(clip)).unwrap().1;
    let effect = filmcraft_project::graphic::layer_indices(&c.effects)[1];
    let p = c.effects[effect].params.get_mut("position").unwrap();
    p.keyframes.clear();
    p.put_keyframe(Tick::ZERO, ParamValue::Vec2(Vec2::new(60.0, 50.0)));
    p.put_keyframe(rate.tick_of(24), ParamValue::Vec2(Vec2::new(80.0, 70.0)));
    let at = rate.tick_of(12);
    let before = state(&s);
    let c = s.active_sequence().unwrap().find_item(ClipId(clip)).unwrap().1;
    let boxes: Vec<_> = item_layer_specs(c, at, (128, 128)).iter().map(|(_, sp)| layer_bounds(sp)).collect();
    let args = json!({"clip":clip,"layer":0,"to":1,"edges":["left","top"],"time":at.0});
    let plan = crate::graphic_templates::prepare_pinning(&s, &args, at).unwrap();
    assert_eq!(state(&s), before);
    let expected = plan.expected.clone();
    s.execute("graphics.pin", args.clone()).unwrap();
    assert_eq!(*s.project, expected);
    let c = s.active_sequence().unwrap().find_item(ClipId(clip)).unwrap().1;
    let effect = filmcraft_project::graphic::layer_indices(&c.effects)[0];
    let pin = c.effects[effect].layer.as_ref().unwrap().pin.as_ref().unwrap();
    assert!((pin.offsets[0] - (boxes[0][0] - boxes[1][0])).abs() < 1e-6);
    assert!((pin.offsets[1] - (boxes[0][1] - boxes[1][1])).abs() < 1e-6);
    let stale = crate::graphic_templates::prepare_pinning(&s, &json!({"clip":clip,"layer":0,"to":"frame"}), at).unwrap();
    s.execute("graphics.set", json!({"clip":clip,"layer":2,"props":{"position":[11,19]},"merge":false,"begin":false})).unwrap();
    let later = state(&s);
    assert!(stale.commit(&mut s).is_err());
    assert_eq!(state(&s), later);
}

#[test]
fn graphics_layout_all_menu_align_and_distribution_variants_share_explicit_time_preparation() {
    let (s, clip) = layout_fixture();
    let at = s.sequence_rate().tick_of(12);
    for (name, mode) in [("Frame", "frame"), ("Group", "group"), ("Selection", "selection")] {
        for how in ["left", "hcenter", "right", "top", "vcenter", "bottom"] {
            let mut explicit = layout_copy(&s);
            let mut menu = layout_copy(&s);
            menu.set_playhead(at);
            let view = serde_json::to_value(&explicit.state).unwrap();
            explicit.execute("graphics.align", json!({"clip":clip,"layers":[0,1,2],"align":how,"to":mode,"time":at.0})).unwrap();
            menu.execute(&format!("graphics.align{name}.{how}"), json!({"clip":clip,"layers":[0,1,2]})).unwrap();
            assert_eq!(*explicit.project, *menu.project, "{name}/{how}");
            assert_eq!(serde_json::to_value(&explicit.state).unwrap(), view);
        }
    }
    for (axis, space, command) in [
        ("horizontal", false, "graphics.distributeHorizontally"),
        ("horizontal", true, "graphics.distributeSpaceHorizontally"),
        ("vertical", false, "graphics.distributeVertically"),
        ("vertical", true, "graphics.distributeSpaceVertically"),
    ] {
        let mut explicit = layout_copy(&s);
        let mut menu = layout_copy(&s);
        menu.set_playhead(at);
        let view = serde_json::to_value(&explicit.state).unwrap();
        explicit.execute("graphics.distribute", json!({"clip":clip,"layers":[0,1,2],"axis":axis,"space":space,"time":at.0})).unwrap();
        menu.execute(command, json!({"clip":clip,"layers":[0,1,2]})).unwrap();
        assert_eq!(*explicit.project, *menu.project, "{command}");
        assert_eq!(serde_json::to_value(&explicit.state).unwrap(), view);
    }
}

#[test]
fn graphics_layout_prepared_basis_cannot_replace_later_human_work_or_history() {
    let (mut s, clip) = layout_fixture();
    let plan =
        crate::graphics::prepare_alignment(&s, &json!({"clip":clip,"layers":[0,1]}), "left", crate::graphics::AlignTo::Frame, filmcraft_time::Tick(0)).unwrap();
    let expected = plan.expected.clone();
    let mut same = layout_copy(&s);
    plan.commit(&mut same).unwrap();
    assert_eq!(*same.project, expected);
    let stale =
        crate::graphics::prepare_alignment(&s, &json!({"clip":clip,"layers":[0,1]}), "left", crate::graphics::AlignTo::Frame, filmcraft_time::Tick(0)).unwrap();
    s.execute("graphics.set", json!({"clip":clip,"layer":2,"time":0,"props":{"position":[17,19]},"merge":false,"begin":false})).unwrap();
    let before = state(&s);
    assert!(stale.commit(&mut s).is_err());
    assert_eq!(state(&s), before);
}

#[test]
fn graphics_layout_video_groups_use_complete_explicit_targets_and_refuse_locked_late_targets() {
    use filmcraft_geom::Vec2;
    use filmcraft_project::ParamValue;
    let mut s = sequence();
    s.execute("file.newOfflineFile", json!({"name":"Video","seconds":4,"fps":24,"video":true,"audio":false,"width":32,"height":24})).unwrap();
    let item = s.project.items.values().find(|i| i.as_media().is_some()).unwrap().id.0;
    for track in ["V1", "V2"] {
        s.execute("timeline.place", json!({"item":item,"track":track,"time":0,"sourceIn":0,"duration":254016000000i64})).unwrap();
    }
    let sid = s.state.active_sequence.unwrap();
    let mut ids = Vec::new();
    for (i, tr) in std::sync::Arc::make_mut(&mut s.project).sequence_mut(sid).unwrap().video_tracks.iter_mut().enumerate() {
        let c = &mut tr.items[0];
        ids.push(c.id);
        c.effects.iter_mut().find(|e| e.effect == "motion").unwrap().params.get_mut("position").unwrap().value =
            ParamValue::Vec2(Vec2::new(20.0 + i as f64 * 60.0, 30.0 + i as f64 * 40.0));
    }
    s.state.selection = ids.clone();
    s.state.graphic_layers.clear();
    assert!(s.is_enabled("graphics.alignGroup.left"), "ordinary video groups must use their native group control");
    let before = s.project.clone();
    let undo = s.history.undo.len();
    s.execute("graphics.align", json!({"clips":ids.iter().map(|c|c.0).collect::<Vec<_>>(),"align":"left","to":"group","time":0})).unwrap();
    let q = s.active_sequence().unwrap();
    let mut positions = Vec::new();
    let mut left = f64::MAX;
    for id in &ids {
        let (_, c) = q.find_item(*id).unwrap();
        positions.push(c.effect("motion").unwrap().vec2_at("position", filmcraft_time::Tick(0)).x);
        let src = s.project.source_size(c.item).unwrap();
        let m = filmcraft_render::motion_matrix(q, c, src, s.project.source_par(c.item), filmcraft_time::Tick(0));
        for (x, y) in [(0.0, 0.0), (src.0 as f64, 0.0), (0.0, src.1 as f64), (src.0 as f64, src.1 as f64)] {
            left = left.min(m.apply(Vec2::new(x, y)).x);
        }
    }
    assert!(left.abs() < 1e-6);
    assert!((positions[1] - positions[0] - 60.0).abs() < 1e-6);
    assert_eq!(s.history.undo.len(), undo + 1);
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(*s.project, *before);
    let mut corrupt = layout_copy(&s);
    let (_, c) = std::sync::Arc::make_mut(&mut corrupt.project).sequence_mut(sid).unwrap().find_item_mut(ids[0]).unwrap();
    c.effects.iter_mut().find(|e| e.effect == "motion").unwrap().params.get_mut("scale").unwrap().value = ParamValue::Float(f64::NAN);
    let corrupted = state(&corrupt);
    let error = corrupt.execute("graphics.align", json!({"clips":[ids[0].0],"align":"left","time":0})).unwrap_err();
    assert!(!matches!(error, crate::EngineError::CommandPanicked { .. }));
    assert_eq!(state(&corrupt), corrupted);
    s.execute("timeline.setTrack", json!({"track":"V2","locked":true})).unwrap();
    s.execute("graphics.newShape", json!({"track":0,"frame":240})).unwrap();
    s.execute("edit.undo", json!({})).unwrap();
    s.state.selection = ids.clone();
    for clips in [json!([ids[0].0, ids[1].0]), json!([ids[0].0, ids[0].0]), json!([ids[0].0, u64::MAX]), json!([ids[0].0, "wrong"]), json!([])] {
        reject_unchanged(&mut s, "graphics.align", json!({"clips":clips,"align":"left","time":0}));
    }
    reject_unchanged(&mut s, "graphics.align", json!({"clip":ids[0].0,"clips":[ids[0].0],"align":"left"}));
}

#[test]
fn graphics_layout_corrupt_source_clocks_fail_before_native_panics_or_publication() {
    use filmcraft_time::Tick;
    for kind in ["endpoint", "source", "speed", "geometry", "scale"] {
        let (mut s, clip) = layout_fixture();
        let sid = s.state.active_sequence.unwrap();
        let (_, c) = std::sync::Arc::make_mut(&mut s.project).sequence_mut(sid).unwrap().find_item_mut(ClipId(clip)).unwrap();
        match kind {
            "endpoint" => {
                c.start = Tick(i64::MAX);
                c.duration = Tick(2);
            }
            "source" => {
                c.source_in = Tick(i64::MAX);
                c.reverse = true;
            }
            "speed" => c.speed = 1e300,
            "scale" => {
                let ei = filmcraft_project::graphic::layer_indices(&c.effects)[0];
                c.effects[ei].params.get_mut("scale").unwrap().value = filmcraft_project::ParamValue::Float(f64::NAN);
            }
            _ => {
                let ei = filmcraft_project::graphic::layer_indices(&c.effects)[0];
                c.effects[ei].params.get_mut("position").unwrap().value = filmcraft_project::ParamValue::Vec2(filmcraft_geom::Vec2::new(f64::INFINITY, 0.0));
            }
        }
        let before = state(&s);
        let pointer = s.project.clone();
        let error = s.execute("graphics.align", json!({"clip":clip,"layers":[0,1,2],"align":"left","time":254016000000i64})).unwrap_err();
        assert!(!matches!(error, crate::EngineError::CommandPanicked { .. }), "{kind}: {error}");
        assert_eq!(state(&s), before);
        assert!(std::sync::Arc::ptr_eq(&s.project, &pointer));
    }
}

#[test]
fn graphics_layout_explicit_time_matches_human_playhead_and_preserves_view() {
    use filmcraft_geom::Vec2;
    use filmcraft_project::ParamValue;
    use filmcraft_time::Tick;
    let (mut s, clip) = layout_fixture();
    let sid = s.state.active_sequence.unwrap();
    let at = s.sequence_rate().tick_of(24);
    let (_, c) = std::sync::Arc::make_mut(&mut s.project).sequence_mut(sid).unwrap().find_item_mut(ClipId(clip)).unwrap();
    let indices = filmcraft_project::graphic::layer_indices(&c.effects);
    for (i, ei) in indices.iter().enumerate() {
        let pos = c.effects[*ei].params.get_mut("position").unwrap();
        pos.put_keyframe(Tick(0), ParamValue::Vec2(Vec2::new(20.0 + i as f64 * 30.0, 20.0)));
        pos.put_keyframe(at, ParamValue::Vec2(Vec2::new(30.0 + i as f64 * 35.0, 80.0)));
    }
    s.set_playhead(s.sequence_rate().tick_of(5));
    let mut human = Session::new(s.services.clone());
    human.project = s.project.clone();
    human.state = s.state.clone();
    human.media = s.media.clone();
    human.set_playhead(at);
    let view = serde_json::to_value(&s.state).unwrap();
    let before = s.project.clone();
    let undo = s.history.undo.len();
    s.execute("graphics.align", json!({"clip":clip,"layers":[0,1,2],"align":"left","time":at.0})).unwrap();
    human.execute("graphics.align", json!({"clip":clip,"layers":[0,1,2],"align":"left"})).unwrap();
    assert_eq!(*s.project, *human.project);
    assert_eq!(serde_json::to_value(&s.state).unwrap(), view);
    assert_eq!(s.history.undo.len(), undo + 1);
    let after = s.project.clone();
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(*s.project, *before);
    s.execute("edit.redo", json!({})).unwrap();
    assert_eq!(*s.project, *after);
    assert_eq!(filmcraft_format::decode(&filmcraft_format::encode(&s.project, false)).unwrap().project, *after);
}

#[test]
fn graphics_hostile_numeric_layers_never_alias_zero_or_selected_layer() {
    for command in ["graphics.setText", "graphics.set", "graphics.deleteLayer"] {
        for layer in [json!(-1), json!(0.5), json!(1e100), json!(true), Value::Null, json!([]), json!({})] {
            let mut s = sequence();
            let clip = s.execute("graphics.newText", json!({"text":"Back","track":0})).unwrap()["clip"].as_u64().unwrap();
            s.execute("graphics.newText", json!({"text":"Front","clip":clip})).unwrap();
            redo_branch(&mut s);
            reject_unchanged(&mut s, command, json!({"clip":clip,"layer":layer,"text":"Wrong","props":{"text":"Wrong"}}));
        }
    }
}

#[test]
fn graphics_explicit_locked_creation_preserves_project_and_both_history_branches() {
    for command in ["graphics.newText", "graphics.newShape", "graphics.newRectangle", "graphics.newPolygon"] {
        let mut s = sequence();
        s.execute("timeline.setTrack", json!({"track":"V1","locked":true})).unwrap();
        redo_branch(&mut s);
        reject_unchanged(&mut s, command, json!({"track":0,"frame":0,"text":"Locked","sides":5}));
    }
}

#[test]
fn graphics_explicit_wrong_clip_does_not_create_a_new_graphic() {
    for command in ["graphics.newText", "graphics.newShape"] {
        for missing in [false, true] {
            let mut s = sequence();
            s.execute("file.newOfflineFile", json!({"name":"Picture","seconds":4,"fps":24,"video":true,"audio":false,"width":128,"height":128})).unwrap();
            let item = s.project.items.values().find(|i| i.as_media().is_some()).unwrap().id.0;
            s.execute("timeline.place", json!({"item":item,"track":"V1","time":0,"sourceIn":0,"duration":254016000000i64})).unwrap();
            let clip = s.active_sequence().unwrap().video_tracks[0].items[0].id.0;
            redo_branch(&mut s);
            reject_unchanged(&mut s, command, json!({"clip":if missing {u64::MAX} else {clip},"text":"Wrong target"}));
        }
    }
}

#[test]
fn graphics_valid_layer_names_indices_and_omitted_selection_keep_human_behavior() {
    let mut s = sequence();
    let clip = s.execute("graphics.newText", json!({"text":"Back","track":0})).unwrap()["clip"].as_u64().unwrap();
    s.execute("graphics.newText", json!({"text":"Front","clip":clip})).unwrap();
    s.execute("graphics.setText", json!({"clip":clip,"layer":0,"text":"Named back"})).unwrap();
    s.execute("graphics.setText", json!({"clip":clip,"layer":"Named back","text":"Back again"})).unwrap();
    s.execute("graphics.setText", json!({"clip":clip,"text":"Selected front"})).unwrap();
    let list = s.execute("graphics.list", json!({"clip":clip})).unwrap();
    assert_eq!(list["layers"][0]["text"], "Back again");
    assert_eq!(list["layers"][1]["text"], "Selected front");
    let after = s.project.clone();
    s.execute("edit.undo", json!({})).unwrap();
    s.execute("edit.redo", json!({})).unwrap();
    assert_eq!(*s.project, *after);
    assert!(s.active_sequence().unwrap().find_item(ClipId(clip)).is_some());
}

#[test]
fn graphics_automatic_creation_skips_locked_tracks_and_explicit_new_clip_stays_available() {
    let mut s = sequence();
    s.execute("timeline.setTrack", json!({"track":"V1","locked":true})).unwrap();
    let result = s.execute("graphics.newText", json!({"text":"Automatic"})).unwrap();
    let clip = ClipId(result["clip"].as_u64().unwrap());
    let q = s.active_sequence().unwrap();
    assert_eq!(q.find_item(clip).unwrap().0, q.video_tracks[1].id);
    let second = s.execute("graphics.newText", json!({"text":"Separate","clip":clip.0,"newClip":true,"frame":240})).unwrap();
    assert_ne!(second["clip"], result["clip"]);
}

#[test]
fn graphics_exact_frame_and_tick_durations_match_human_native_rounding_and_survive_reopen() {
    for fps in [23.976, 24.0, 29.97, 37.516] {
        for command in ["graphics.newText", "graphics.newShape"] {
            for field in ["durationFrames", "durationTicks"] {
                let mut s = sequence();
                let sid = s.state.active_sequence.unwrap();
                let rate = filmcraft_time::FrameRate::from_f64(fps);
                std::sync::Arc::make_mut(&mut s.project).sequence_mut(sid).unwrap().settings.frame_rate = rate;
                let duration = rate.tick_of(37);
                let start = rate.tick_of(72);
                let mut human = Session::new(s.services.clone());
                human.project = s.project.clone();
                human.state = s.state.clone();
                human.media = s.media.clone();
                let mut params = json!({"text":"Exact native clock","shape":"rectangle","time":start.0,"track":0});
                params[field] = json!(if field == "durationFrames" { 37 } else { duration.0 });
                let result = s.execute(command, params).unwrap();
                human.execute(command, json!({"text":"Exact native clock","shape":"rectangle","time":start.0,"track":0,"seconds":duration.seconds()})).unwrap();
                assert_eq!(*s.project, *human.project, "{command}, {field}, {fps}");
                assert_eq!(serde_json::to_value(&s.state).unwrap(), serde_json::to_value(&human.state).unwrap());
                let clip = ClipId(result["clip"].as_u64().unwrap());
                let item = s.active_sequence().unwrap().find_item(clip).unwrap().1;
                assert_eq!((item.start, item.duration), (start, duration));
                let encoded = filmcraft_format::encode(&s.project, false);
                let reopened = filmcraft_format::decode(&encoded).unwrap().project;
                assert_eq!(*s.project, reopened);
                let after = s.project.clone();
                s.execute("edit.undo", json!({})).unwrap();
                assert!(s.active_sequence().unwrap().find_item(clip).is_none());
                s.execute("edit.redo", json!({})).unwrap();
                assert_eq!(*s.project, *after);
            }
        }
    }
}

#[test]
fn graphics_bad_exact_duration_preserves_redo_and_never_defaults() {
    for command in ["graphics.newText", "graphics.newShape"] {
        for field in ["durationFrames", "durationTicks"] {
            for value in [json!(-1), json!(0), json!(0.5), json!(u64::MAX), json!(1e100), json!(true), Value::Null, json!("1")] {
                let mut s = sequence();
                redo_branch(&mut s);
                let mut params = json!({"text":"Invalid duration","track":0});
                params[field] = value;
                reject_unchanged(&mut s, command, params);
            }
        }
        for params in [
            json!({"durationFrames":24,"durationTicks":254016000000i64}),
            json!({"durationFrames":24,"seconds":1}),
            json!({"durationTicks":254016000000i64,"seconds":1}),
            json!({"durationTicks":1}),
        ] {
            let mut s = sequence();
            redo_branch(&mut s);
            reject_unchanged(&mut s, command, params);
        }
    }
}
