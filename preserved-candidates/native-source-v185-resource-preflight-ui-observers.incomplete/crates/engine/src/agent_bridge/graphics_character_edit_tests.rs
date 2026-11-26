use super::*;
use filmcraft_project::graphic_design::{CharStyle, StyleRun, apply_char_style, clear_char_style};
use filmcraft_project::{ClipId, ParamValue};
use filmcraft_time::Tick;
fn range_request(s: &mut Session, n: usize, frame: i64, start: usize, end: usize, clear: bool, style: Value) -> Value {
    read(s, n);
    let target = target(s);
    let id = layer_ids(s)[0].clone();
    let p = page(s, n, json!({"kind":"character_styles","identity":target["identity"],"layer_id":id,"frame":frame}), 0, 1);
    let t = json!({"kind":"character_text","identity":target["identity"],"layer_id":id,"frame":frame});
    let text_page = page(s, n, t, 0, 16);
    let text: String = text_page["graphics"].as_array().unwrap().iter().filter_map(|r| r["text"].as_str()).collect();
    let selected: String = text.chars().skip(start).take(end.saturating_sub(start)).collect();
    use sha2::{Digest, Sha256};
    let selected_sha = format!("{:x}", Sha256::digest(selected.as_bytes()));
    json!({"token":token(n),"label":"Exact character range","exact":true,"authority":"document_state","operations":[{"command":graphics::COMMAND,"params":{"scope":scope(s),"beforeGraphicsSha256":p["collectionSha256"],"edit":{"type":"style_range","target":target,"layerId":id,"frame":frame,"start":start,"end":end,"textSha256":p["characterState"]["textSha256"],"runsSha256":p["characterState"]["runsSha256"],"rangeTextSha256":selected_sha,"clear":clear,"style":style}}}]})
}
#[test]
fn sparse_character_range_matches_frozen_reference_and_large_font_ranges_without_character_expansion() {
    fn reference(runs: &[StyleRun], n: usize, a: usize, b: usize, over: Option<&CharStyle>) -> Vec<StyleRun> {
        let mut values = vec![CharStyle::default(); n];
        for r in runs {
            for value in values.iter_mut().take(r.end.min(n)).skip(r.start.min(n)) {
                *value = value.merged(&r.style);
            }
        }
        for value in values.iter_mut().take(b.min(n)).skip(a.min(n)) {
            *value = over.map_or_else(CharStyle::default, |s| value.merged(s));
        }
        let mut out: Vec<StyleRun> = Vec::new();
        for (i, value) in values.into_iter().enumerate() {
            if value.is_empty() {
                continue;
            }
            if let Some(last) = out.last_mut()
                && last.end == i
                && last.style == value
            {
                last.end = i + 1;
            } else {
                out.push(StyleRun { start: i, end: i + 1, style: value });
            }
        }
        out
    }
    for seed in 0..512usize {
        let n = seed % 33;
        let mut runs = Vec::new();
        for i in 0..n {
            if (i + seed) % 3 == 0 {
                runs.push(StyleRun {
                    start: i,
                    end: i + 1,
                    style: CharStyle { font: Some(format!("font-{}", i % 2)), faux_bold: Some(i % 2 == 0), ..Default::default() },
                });
            }
        }
        if seed % 5 == 0 {
            runs.push(StyleRun { start: 0, end: n.saturating_add(3), style: CharStyle { underline: Some(true), ..Default::default() } });
        }
        let a = seed % 40;
        let b = (seed * 7) % 40;
        let style = CharStyle { faux_italic: Some(false), tracking: Some((seed % 10) as f32), ..Default::default() };
        assert_eq!(apply_char_style(&runs, n, a, b, &style), reference(&runs, n, a, b, Some(&style)), "{seed}");
        assert_eq!(clear_char_style(&runs, n, a, b), reference(&runs, n, a, b, None), "{seed}");
    }
    let long = vec![StyleRun { start: 0, end: 1_000_000, style: CharStyle { font: Some("x".repeat(16000)), ..Default::default() } }];
    let out = apply_char_style(&long, 1_000_000, 1, 2, &CharStyle { underline: Some(true), ..Default::default() });
    assert_eq!(out.len(), 3);
    assert_eq!((out[1].start, out[1].end), (1, 2));
    assert_eq!(out[2].end, 1_000_000);
    assert_eq!(clear_char_style(&long, 1_000_000, 1, 2).len(), 2);
}
#[test]
fn guarded_character_aliases_match_human_services_preserve_view_and_exact_owned_undo_redo() {
    let cases = vec![
        ("size", json!(36.5)),
        ("fontSize", json!(36.5)),
        ("fill", json!("#ff8000")),
        ("color", json!([0.1, 0.2, 0.3])),
        ("fillColor", json!([0.1, 0.2, 0.3, 0.4])),
        ("fill_color", json!("11223344")),
        ("bold", json!(true)),
        ("fauxBold", json!(true)),
        ("faux_bold", json!(true)),
        ("italic", json!(true)),
        ("fauxItalic", json!(true)),
        ("faux_italic", json!(true)),
        ("underline", json!(true)),
        ("tracking", json!(2.5)),
        ("baselineShift", json!(-3)),
        ("baseline_shift", json!(-3)),
        ("caps", json!("SMALL_CAPS")),
    ];
    for (n, (key, value)) in cases.into_iter().enumerate() {
        let mut s = populated();
        let t = target(&s);
        let c = t["clip"].as_u64().unwrap();
        s.execute("graphics.set", json!({"clip":c,"layer":0,"time":0,"props":{"text":"A😀e\u{301} more"}})).unwrap();
        let mut h = human(&s);
        let before = s.project.clone();
        let view = serde_json::to_value(&s.state).unwrap();
        let undo = s.history.undo.len();
        let req = range_request(&mut s, 81000 + n, 5, 1, 3, false, json!({key:value}));
        let receipt = s.execute("agent.apply", req.clone()).unwrap();
        let at = h.sequence_rate().tick_of(5);
        h.execute(
            "graphics.setCharStyle",
            json!({"clip":c,"layer":0,"time":at.0,"start":1,"end":3,"clear":false,"style":{key:value},"merge":false,"begin":false}),
        )
        .unwrap();
        assert_eq!(*s.project, *h.project, "{key}");
        assert_eq!(serde_json::to_value(&s.state).unwrap(), view);
        assert_eq!(s.history.undo.len(), undo + 1);
        assert_eq!(receipt["results"][0]["humanCommand"], "graphics.setCharStyle");
        assert_eq!(receipt["results"][0]["characterRange"]["clearedFontOverrides"], false);
        assert_eq!(s.execute("agent.apply", req).unwrap(), receipt);
        assert_eq!(s.history.undo.len(), undo + 1);
        let after = s.project.clone();
        read(&mut s, 81100 + n);
        s.execute("agent.undo", json!({"token":token(81100+n),"commit":token(81000+n)})).unwrap();
        assert_eq!(*s.project, *before);
        read(&mut s, 81200 + n);
        s.execute("agent.redo", json!({"token":token(81200+n),"commit":token(81000+n),"undo":token(81100+n)})).unwrap();
        assert_eq!(*s.project, *after);
    }
}
#[test]
fn guarded_character_clear_discloses_font_removal_and_preserves_saved_runs_beyond_a_shorter_animated_frame() {
    let mut s = populated();
    let t = target(&s);
    let c = ClipId(t["clip"].as_u64().unwrap());
    s.execute(
        "graphics.setCharStyle",
        json!({"clip":c.0,"layer":0,"start":0,"end":9,"style":{"font":"Existing authored reference","fontStyle":"Bold","underline":true}}),
    )
    .unwrap();
    let at = s.sequence_rate().tick_of(10);
    let sid = s.state.active_sequence.unwrap();
    let e =
        Arc::make_mut(&mut s.project).sequence_mut(sid).unwrap().find_item_mut(c).unwrap().1.effects.iter_mut().find(|e| e.effect == "graphic_text").unwrap();
    let p = e.params.get_mut("text").unwrap();
    p.put_keyframe(Tick::ZERO, ParamValue::Text("Duplicate".into()));
    p.put_keyframe(at, ParamValue::Text("A😀".into()));
    let mut h = human(&s);
    let req = range_request(&mut s, 81300, 10, 0, 1, true, json!({}));
    let before = s.project.clone();
    let receipt = s.execute("agent.apply", req).unwrap();
    h.execute("graphics.setCharStyle", json!({"clip":c.0,"layer":0,"time":at.0,"start":0,"end":1,"clear":true,"style":{},"merge":false})).unwrap();
    assert_eq!(*s.project, *h.project);
    assert_eq!(receipt["results"][0]["characterRange"]["clearedFontOverrides"], true);
    let c = s.active_sequence().unwrap().find_item(c).unwrap().1;
    let runs = &c.effects.iter().find(|e| e.effect == "graphic_text").unwrap().layer.as_ref().unwrap().runs;
    assert_eq!(runs.len(), 1);
    assert_eq!((runs[0].start, runs[0].end), (1, 9));
    assert_eq!(runs[0].style.font.as_deref(), Some("Existing authored reference"));
    assert_ne!(*s.project, *before);
}
#[test]
fn guarded_character_requests_refuse_bad_values_alias_collisions_ranges_and_fresh_foreign_or_locked_targets_without_writes() {
    let bad = vec![
        json!({}),
        json!({"size":1e39}),
        json!({"size":2001}),
        json!({"tracking":10001}),
        json!({"baselineShift":5001}),
        json!({"size":null}),
        json!({"bold":1}),
        json!({"size":10,"fontSize":11}),
        json!({"caps":3}),
        json!({"caps":"surprise"}),
        json!({"color":[0,0,0,0,0]}),
        json!({"color":[0,0,2]}),
        json!({"unknown":true}),
        json!({"font":"Unadmitted"}),
        json!({"family":"Unadmitted"}),
        json!({"fontStyle":"Unadmitted"}),
        json!({"font_style":"Unadmitted"}),
        json!({"style":"Unadmitted"}),
    ];
    for (n, style) in bad.into_iter().enumerate() {
        let mut s = populated();
        let req = range_request(&mut s, 81400 + n, 0, 0, 1, false, style);
        let before = state(&s);
        assert!(s.execute("agent.apply", req).is_err());
        assert_eq!(state(&s), before);
    }
    for (n, case) in
        ["empty", "long", "foreign", "layer", "frame", "text", "runs", "clear_style", "locked", "protected", "shared", "noop"].into_iter().enumerate()
    {
        let mut s = populated();
        let sid = s.state.active_sequence.unwrap();
        if case == "locked" {
            Arc::make_mut(&mut s.project).sequence_mut(sid).unwrap().video_tracks[0].locked = true;
        }
        if case == "protected" {
            Arc::make_mut(&mut s.project).sequence_mut(sid).unwrap().video_tracks[0].items[0].identity.as_mut().unwrap().protection = Some("human".into());
        }
        if case == "shared" {
            let id = s.active_sequence().unwrap().video_tracks[0].items[0].item;
            Arc::make_mut(&mut s.project).source_graphics.insert(id, Default::default());
        }
        let mut req = range_request(&mut s, 81500 + n, 0, 0, 1, false, json!({"underline":true}));
        let edit = &mut req["operations"][0]["params"]["edit"];
        match case {
            "empty" => edit["end"] = json!(0),
            "long" => edit["end"] = json!(1000),
            "foreign" => edit["target"]["identity"] = json!("f".repeat(32)),
            "layer" => edit["layerId"] = json!("f".repeat(32)),
            "frame" => edit["frame"] = json!(48),
            "text" => edit["textSha256"] = json!("0".repeat(64)),
            "runs" => edit["runsSha256"] = json!("0".repeat(64)),
            "clear_style" => edit["clear"] = json!(true),
            "noop" => {
                edit["clear"] = json!(true);
                edit["style"] = json!({});
            }
            _ => {}
        }
        let before = state(&s);
        assert!(s.execute("agent.apply", req).is_err(), "{case}");
        assert_eq!(state(&s), before, "{case}");
    }
}
