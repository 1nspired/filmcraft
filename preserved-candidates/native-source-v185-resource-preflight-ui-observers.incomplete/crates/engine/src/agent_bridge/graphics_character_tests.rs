use super::*;
use filmcraft_project::{
    ParamValue,
    graphic_design::{CharStyle, StyleRun},
};
use sha2::{Digest, Sha256};
fn character_target(s: &Session, text: bool, frame: i64) -> Value {
    json!({"kind":if text {"character_text"} else {"character_styles"},"identity":target(s)["identity"],"layer_id":layer_ids(s)[0],"frame":frame})
}
#[test]
fn paged_character_inventory_reads_oversized_layers_all_runs_and_unicode_text_without_edits_or_font_admission() {
    let mut s = populated();
    let t = target(&s);
    let clip = filmcraft_project::ClipId(t["clip"].as_u64().unwrap());
    let text = "A😀e\u{301}".repeat(200);
    s.execute("graphics.set", json!({"clip":clip.0,"layer":0,"time":0,"props":{"text":text},"merge":false})).unwrap();
    let sid = s.state.active_sequence.unwrap();
    let q = Arc::make_mut(&mut s.project).sequence_mut(sid).unwrap();
    let c = q.find_item_mut(clip).unwrap().1;
    let e = c.effects.iter_mut().find(|e| e.effect == "graphic_text").unwrap();
    let runs = (0..500)
        .map(|i| StyleRun {
            start: i,
            end: i + 1,
            style: CharStyle { font: Some(format!("Authored font reference {i} {}", "x".repeat(150))), faux_bold: Some(i % 2 == 0), ..Default::default() },
        })
        .collect::<Vec<_>>();
    e.layer.get_or_insert_with(Default::default).runs = runs.clone();
    read(&mut s, 79001);
    let before = state(&s);
    let scope = scope(&s);
    assert!(
        s.execute("agent.readGraphics", json!({"token":token(79001),"scope":scope,"target":{"kind":"layers","identity":t["identity"]},"offset":0,"limit":1}))
            .is_err()
    );
    let index = page(&mut s, 79001, json!({"kind":"layer_index","identity":t["identity"]}), 0, 16);
    assert_eq!(index["total"], 3);
    assert_eq!(index["graphics"][0]["layerId"], layer_ids(&s)[0]);
    let mut styles = Vec::new();
    let mut offset = 0;
    let mut header = None;
    while offset < runs.len() {
        let target = character_target(&s, false, 0);
        let p = page(&mut s, 79001, target, offset, 16);
        assert!(serde_json::to_vec(&p).unwrap().len() <= 64 * 1024);
        if let Some(h) = &header {
            assert_eq!(&p["characterState"], h);
        } else {
            header = Some(p["characterState"].clone());
        }
        styles.extend(p["graphics"].as_array().unwrap().iter().map(|r| r["run"].clone()));
        offset += p["graphics"].as_array().unwrap().len();
    }
    assert_eq!(styles, serde_json::to_value(&runs).unwrap().as_array().unwrap().clone());
    assert_eq!(header.as_ref().unwrap()["fontResources"], "unverified");
    let mut joined = String::new();
    let chunks = text.chars().count().div_ceil(256);
    for offset in 0..chunks {
        let target = character_target(&s, true, 0);
        let p = page(&mut s, 79001, target, offset, 1);
        assert_eq!(p["characterState"], header.as_ref().unwrap().clone());
        let row = &p["graphics"][0];
        assert_eq!(row["start"], offset * 256);
        assert_eq!(row["end"].as_u64().unwrap() - row["start"].as_u64().unwrap(), row["text"].as_str().unwrap().chars().count() as u64);
        joined.push_str(row["text"].as_str().unwrap());
    }
    assert_eq!(joined, text);
    assert_eq!(header.as_ref().unwrap()["textSha256"], format!("{:x}", Sha256::digest(text.as_bytes())));
    assert_eq!(state(&s), before);
}
#[test]
fn paged_character_reads_use_exact_native_frame_and_refuse_foreign_shape_clock_offsets_and_alias_fields_without_writes() {
    let mut s = populated();
    let t = target(&s);
    let at = s.sequence_rate().tick_of(10);
    let sid = s.state.active_sequence.unwrap();
    let c = Arc::make_mut(&mut s.project).sequence_mut(sid).unwrap().find_item_mut(filmcraft_project::ClipId(t["clip"].as_u64().unwrap())).unwrap().1;
    let e = c.effects.iter_mut().find(|e| e.effect == "graphic_text").unwrap();
    let p = e.params.get_mut("text").unwrap();
    p.put_keyframe(filmcraft_time::Tick::ZERO, ParamValue::Text("A😀".into()));
    p.put_keyframe(at, ParamValue::Text("Different frame".into()));
    read(&mut s, 79002);
    let before = state(&s);
    let a = character_target(&s, true, 0);
    let a = page(&mut s, 79002, a, 0, 1);
    let b = character_target(&s, true, 10);
    let b = page(&mut s, 79002, b, 0, 1);
    assert_eq!(a["graphics"][0]["text"], "A😀");
    assert_ne!(a["characterState"]["textSha256"], b["characterState"]["textSha256"]);
    let good = character_target(&s, true, 0);
    let shape = layer_ids(&s)[2].clone();
    for target in [
        json!({"kind":"character_text","identity":t["identity"],"layer_id":shape,"frame":0}),
        json!({"kind":"character_text","identity":"f".repeat(32),"layer_id":good["layer_id"],"frame":0}),
        json!({"kind":"character_text","identity":t["identity"],"layerId":good["layer_id"],"frame":0}),
        json!({"kind":"character_text","identity":t["identity"],"layer_id":good["layer_id"],"frame":48}),
        json!({"kind":"character_text","identity":t["identity"],"layer_id":good["layer_id"],"frame":0.5}),
    ] {
        assert!(s.execute("agent.readGraphics", json!({"token":token(79002),"scope":scope(&s),"target":target,"offset":0,"limit":1})).is_err());
    }
    assert!(s.execute("agent.readGraphics", json!({"token":token(79002),"scope":scope(&s),"target":good,"offset":2,"limit":1})).is_err());
    assert_eq!(state(&s), before);
}
#[test]
fn paged_character_reads_keep_saved_inheritance_runs_at_shorter_frames_and_refuse_stale_cow_tickets() {
    let mut s = populated();
    let t = target(&s);
    let c = filmcraft_project::ClipId(t["clip"].as_u64().unwrap());
    s.execute("graphics.setCharStyle", json!({"clip":c.0,"layer":0,"start":2,"end":8,"style":{"underline":true}})).unwrap();
    let at = s.sequence_rate().tick_of(10);
    let sid = s.state.active_sequence.unwrap();
    let e =
        Arc::make_mut(&mut s.project).sequence_mut(sid).unwrap().find_item_mut(c).unwrap().1.effects.iter_mut().find(|e| e.effect == "graphic_text").unwrap();
    let p = e.params.get_mut("text").unwrap();
    p.put_keyframe(filmcraft_time::Tick::ZERO, ParamValue::Text("Duplicate".into()));
    p.put_keyframe(at, ParamValue::Text("A".into()));
    read(&mut s, 79003);
    let target = character_target(&s, false, 10);
    let p = page(&mut s, 79003, target.clone(), 0, 1);
    assert_eq!(p["characterState"]["textCharacters"], 1);
    assert_eq!(p["graphics"][0]["run"], json!({"start":2,"end":8,"underline":true}));
    Arc::make_mut(&mut s.project).sequence_mut(sid).unwrap().find_item_mut(c).unwrap().1.effects[0].enabled = false;
    let before = state(&s);
    assert!(s.execute("agent.readGraphics", json!({"token":token(79003),"scope":scope(&s),"target":target,"offset":0,"limit":1})).is_err());
    assert_eq!(state(&s), before);
}
