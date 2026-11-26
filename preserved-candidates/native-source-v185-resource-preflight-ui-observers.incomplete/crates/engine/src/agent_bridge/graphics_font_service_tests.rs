use super::*;
use filmcraft_project::{
    CaptionFormat, CaptionTrack, ItemId, ParamValue, TrackId, graphic,
    graphic_design::{CharStyle, StyleRun},
};
fn fixture() -> (Session, ItemId) {
    let mut s = populated();
    let c = target(&s)["clip"].clone();
    s.execute("graphics.set", json!({"clip":c,"layer":0,"time":0,"props":{"font":"Noto Serif","font_style":"Regular"}})).unwrap();
    let source = ItemId(s.execute("graphics.upgradeToSourceGraphic", json!({"clip":c})).unwrap()["item"].as_u64().unwrap());
    let sid = s.state.active_sequence.unwrap();
    let rate = s.sequence_rate();
    let update = |e: &mut filmcraft_project::EffectInstance| {
        e.params.get_mut("font").unwrap().put_keyframe(rate.tick_of(10), ParamValue::Text("Only an animated font".into()));
        e.params.get_mut("font_style").unwrap().put_keyframe(rate.tick_of(10), ParamValue::Text("Italic".into()));
        e.layer.get_or_insert_with(Default::default).runs = vec![
            StyleRun { start: 0, end: 1, style: CharStyle { font: Some("Noto Serif".into()), font_style: Some("Regular".into()), ..Default::default() } },
            StyleRun { start: 1, end: 2, style: CharStyle { font_style: Some("Italic".into()), ..Default::default() } },
            StyleRun { start: 2, end: 3, style: CharStyle { font: Some("Inter".into()), ..Default::default() } },
        ];
    };
    let p = Arc::make_mut(&mut s.project);
    update(p.sequence_mut(sid).unwrap().video_tracks[0].items[0].effects.iter_mut().find(|e| e.effect == graphic::TEXT_LAYER).unwrap());
    update(&mut p.source_graphics.get_mut(&source).unwrap().layers[0]);
    let track = TrackId(p.alloc_id());
    let mut captions = CaptionTrack::new(track, "Project captions".into(), CaptionFormat::Subtitle);
    captions.style.font = "Noto Serif".into();
    p.sequence_mut(sid).unwrap().caption_tracks.push(captions);
    (s, source)
}
#[test]
fn original_global_font_helper_preserves_exact_human_caption_run_master_and_complete_parameter_replacement_semantics() {
    for (i, request) in [
        json!({"from":"noto serif","to":"JetBrains Mono","toStyle":"Bold"}),
        json!({"from":{"family":"noto serif","style":"regular"},"to":{"family":"JetBrains Mono","style":"Bold"}}),
    ]
    .into_iter()
    .enumerate()
    {
        let (mut s, source) = fixture();
        let before = s.project.clone();
        let mut predicted = (*before).clone();
        let spec = crate::graphic_templates::prepare_font_replacement(&request).unwrap();
        let count = crate::graphic_templates::apply_font_replacement(&mut predicted, &spec);
        let r = s.execute("file.replaceFonts", request).unwrap();
        assert_eq!(r["replaced"], count);
        assert_eq!(*s.project, predicted);
        assert_eq!(count, if i == 0 { 5 } else { 4 });
        let q = s.active_sequence().unwrap();
        let e = q.video_tracks[0].items[0].effects.iter().find(|e| e.effect == graphic::TEXT_LAYER).unwrap();
        assert!(e.params["font"].keyframes.is_empty());
        assert!(e.params["font_style"].keyframes.is_empty());
        let runs = &e.layer.as_ref().unwrap().runs;
        assert_eq!(runs[0].style.font.as_deref(), Some("JetBrains Mono"));
        assert!(runs[1].style.font.is_none());
        assert_eq!(runs[1].style.font_style.as_deref(), Some("Italic"));
        assert_eq!(runs[2].style.font.as_deref(), Some("Inter"));
        assert_eq!(q.caption_tracks[0].style.font, if i == 0 { "JetBrains Mono" } else { "Noto Serif" });
        assert_eq!(s.project.source_graphics[&source].layers[0], *e);
        s.execute("edit.undo", json!({})).unwrap();
        assert_eq!(*s.project, *before);
    }
}
#[test]
fn original_global_usage_is_authored_declaration_inventory_and_bad_replacement_does_not_change_human_state() {
    let (mut s, _) = fixture();
    let before = state(&s);
    let usage = crate::graphic_templates::fonts_used_in_project(&s.project);
    assert!(!usage.iter().any(|(f, _, _)| f == "Only an animated font"));
    assert!(usage.iter().any(|(f, st, _)| f == "Noto Serif" && st == "SemiBold"));
    assert_eq!(usage, crate::graphic_templates::fonts_used(&s));
    for request in [json!({"from":"","to":"Inter"}), json!({"from":"Inter","to":5}), json!({"from":{"family":3},"to":"Inter"})] {
        assert!(s.execute("file.replaceFonts", request).is_err());
        assert_eq!(state(&s), before);
    }
}
