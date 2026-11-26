use super::*;
use filmcraft_project::{
    ClipId, ParamValue,
    graphic::{self, LayerContent},
};
use filmcraft_render::graphic_clip;
use filmcraft_text::fonts::{self, FaceData, FaceInfo, FontSource};
use sha2::{Digest, Sha256};
use std::{
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};
fn font_target(s: &Session, kind: &str, frame: i64) -> Value {
    json!({"kind":kind,"identity":target(s)["identity"],"layer_id":layer_ids(s)[0],"frame":frame})
}
fn complete(s: &mut Session, n: usize, kind: &str, frame: i64) -> (Vec<Value>, Value) {
    let mut offset = 0;
    let mut rows = Vec::new();
    let mut header = None;
    loop {
        let target = font_target(s, kind, frame);
        let p = page(s, n, target, offset, 16);
        assert!(serde_json::to_vec(&p).unwrap().len() <= 64 * 1024);
        if let Some(h) = &header {
            assert_eq!(&p["fontState"], h);
        } else {
            header = Some(p["fontState"].clone());
        }
        rows.extend(p["graphics"].as_array().unwrap().iter().cloned());
        match p["nextOffset"].as_u64() {
            Some(n) => {
                assert!(n > offset as u64);
                offset = n as usize;
            }
            None => break,
        }
    }
    (rows, header.unwrap())
}
fn props(s: &mut Session, p: Value) {
    let c = target(s)["clip"].clone();
    s.execute("graphics.set", json!({"clip":c,"layer":0,"time":0,"props":p,"merge":false})).unwrap();
}
fn native(s: &Session, frame: i64) -> Arc<filmcraft_text::Layout> {
    let c = &s.active_sequence().unwrap().video_tracks[0].items[0];
    let mt = crate::graphics::layout_source_time(c, s.sequence_rate().tick_of(frame)).unwrap();
    let t = graphic_clip::graphic_time(c, mt).0;
    let e = c.effects.iter().find(|e| e.effect == graphic::TEXT_LAYER).unwrap();
    let LayerContent::Text(p) = graphic::eval_layer(e, t, (128, 128)).unwrap().content else { panic!("text expected") };
    graphic_clip::text_layout(&p)
}
#[test]
fn font_resource_pages_match_original_native_glyphs_rich_unicode_synthesis_wrapping_and_vertical_without_edits() {
    for (j, vertical) in [false, true].into_iter().enumerate() {
        let mut s = populated();
        props(
            &mut s,
            json!({"text":"office A😀e\u{301} אבג\nnext line","font":"Inter","font_style":"Regular","ligatures":true,"vertical":vertical,"box_width":80,"box_height":110}),
        );
        let c = target(&s)["clip"].clone();
        s.execute(
            "graphics.setCharStyle",
            json!({"clip":c,"layer":0,"start":7,"end":12,"style":{"font":"JetBrains Mono","fontStyle":"Bold Italic","underline":true}}),
        )
        .unwrap();
        let snapshot = read(&mut s, 81000 + j);
        assert_eq!(snapshot["capabilities"]["graphicsFontResources"], true);
        let before = state(&s);
        let expected = native(&s, 0);
        let (glyphs, h) = complete(&mut s, 81000 + j, "font_glyphs", 0);
        let (resources, rh) = complete(&mut s, 81000 + j, "font_resources", 0);
        let (requests, qh) = complete(&mut s, 81000 + j, "font_requests", 0);
        assert_eq!(h, rh);
        assert_eq!(h, qh);
        assert_eq!(h["glyphCount"], glyphs.len());
        assert_eq!(glyphs.len(), expected.glyphs.len());
        for (actual, g) in glyphs.iter().zip(&expected.glyphs) {
            assert_eq!(actual["nativeFace"], g.face);
            assert_eq!(actual["glyph"], g.id);
            assert_eq!(actual["clusterByte"], g.cluster);
            assert_eq!(actual["styleIndex"], g.run);
            assert_eq!(actual["x"], json!(g.x));
            assert_eq!(actual["y"], json!(g.y));
            assert_eq!(actual["synthBold"], g.synth_bold);
            assert_eq!(actual["synthItalic"], g.synth_italic);
            let resource = resources.iter().find(|r| r["nativeFace"] == g.face).unwrap();
            let face = fonts::face(g.face);
            assert_eq!(resource["bytesSha256"], format!("{:x}", Sha256::digest(face.loaded_data().unwrap())));
            assert_eq!(actual["resourceSha256"], resource["resourceSha256"]);
        }
        assert!(requests.iter().any(|r| r["style"]["family"] == "JetBrains Mono" && r["synthBold"] == true && r["synthItalic"] == true));
        assert_eq!(h["fontFileState"], "not-inspected");
        assert_eq!(h["embeddingRights"], "not-established");
        assert_eq!(h["writeAuthority"], false);
        assert_eq!(state(&s), before);
    }
}
#[test]
fn font_resource_reads_follow_original_responsive_clock_and_reject_stale_or_foreign_requests_without_changes() {
    let mut s = populated();
    let c = ClipId(target(&s)["clip"].as_u64().unwrap());
    let rate = s.sequence_rate();
    let sid = s.state.active_sequence.unwrap();
    let clip = Arc::make_mut(&mut s.project).sequence_mut(sid).unwrap().find_item_mut(c).unwrap().1;
    clip.graphic = Some(Box::new(filmcraft_project::graphic_design::GraphicMeta {
        intro: rate.tick_of(4),
        outro: rate.tick_of(4),
        design_duration: rate.tick_of(24),
        ..Default::default()
    }));
    let e = clip.effects.iter_mut().find(|e| e.effect == graphic::TEXT_LAYER).unwrap();
    let p = e.params.get_mut("text").unwrap();
    p.put_keyframe(filmcraft_time::Tick::ZERO, ParamValue::Text("first".into()));
    p.put_keyframe(rate.tick_of(12), ParamValue::Text("responsive later".into()));
    read(&mut s, 81010);
    let before = state(&s);
    for frame in [0, 30, 45] {
        let expected = native(&s, frame);
        let (glyphs, h) = complete(&mut s, 81010, "font_glyphs", frame);
        assert_eq!(glyphs.len(), expected.glyphs.len());
        for (a, b) in glyphs.iter().zip(&expected.glyphs) {
            assert_eq!(a["glyph"], b.id);
            assert_eq!(a["clusterByte"], b.cluster);
        }
        assert_eq!(h["frame"], frame);
    }
    let good = font_target(&s, "font_resources", 0);
    for (target, offset, limit) in [
        (font_target(&s, "font_resources", 48), 0, 1),
        (font_target(&s, "font_resources", -1), 0, 1),
        (json!({"kind":"font_resources","identity":good["identity"],"layerId":good["layer_id"],"frame":0}), 0, 1),
        (json!({"kind":"font_resources","identity":good["identity"],"layer_id":layer_ids(&s)[2],"frame":0}), 0, 1),
        (good.clone(), 4097, 1),
        (good.clone(), 0, 0),
        (good.clone(), 0, 17),
    ] {
        assert!(s.execute("agent.readGraphics", json!({"token":token(81010),"scope":scope(&s),"target":target,"offset":offset,"limit":limit})).is_err());
    }
    assert_eq!(state(&s), before);
    Arc::make_mut(&mut s.project).sequence_mut(sid).unwrap().find_item_mut(c).unwrap().1.name = "Later human edit".into();
    let after = state(&s);
    assert!(s.execute("agent.readGraphics", json!({"token":token(81010),"scope":scope(&s),"target":good,"offset":0,"limit":1})).is_err());
    assert_eq!(state(&s), after);
}
struct GeneratedFace {
    family: String,
    file: PathBuf,
}
impl FontSource for GeneratedFace {
    fn faces(&self) -> Vec<FaceInfo> {
        vec![FaceInfo {
            family: self.family.clone(),
            style: "Regular".into(),
            weight: 400,
            italic: false,
            index: 0,
            data: FaceData::File(self.file.clone()),
            origin: "generated-test",
        }]
    }
}
fn unique() -> String {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    format!("Poema Font Reader {} {}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed))
}
#[test]
fn font_resource_identity_uses_retained_renderer_bytes_after_file_replacement_and_removal() {
    let family = unique();
    let root = std::env::temp_dir().join(family.replace(' ', "-"));
    std::fs::create_dir(&root).unwrap();
    let file = root.join("generated.ttf");
    std::fs::write(&file, fonts::INTER_REGULAR).unwrap();
    assert_eq!(fonts::add_source(&GeneratedFace { family: family.clone(), file: file.clone() }), 1);
    let mut s = populated();
    props(&mut s, json!({"text":"Retained font generation","font":family,"font_style":"Regular"}));
    read(&mut s, 81020);
    let before = state(&s);
    let (resources, h) = complete(&mut s, 81020, "font_resources", 0);
    assert!(resources.iter().any(|r| r["bytesSha256"] == format!("{:x}", Sha256::digest(fonts::INTER_REGULAR))));
    std::fs::write(&file, fonts::INTER_ITALIC).unwrap();
    let (_, after_write) = complete(&mut s, 81020, "font_resources", 0);
    assert_eq!(h, after_write);
    std::fs::remove_file(&file).unwrap();
    let (_, after_delete) = complete(&mut s, 81020, "font_resources", 0);
    assert_eq!(h, after_delete);
    assert_eq!(state(&s), before);
}
#[test]
fn font_resource_queries_disclose_missing_family_and_invalid_primary_bytes_without_inventing_parse_proof() {
    let mut s = populated();
    let missing = unique();
    props(&mut s, json!({"text":"ordinary text","font":missing,"font_style":"Regular"}));
    read(&mut s, 81030);
    let (requests, h) = complete(&mut s, 81030, "font_requests", 0);
    assert_eq!(requests[0]["missingFamily"], true);
    assert_eq!(h["missingFont"], true);
    let family = unique();
    let root = std::env::temp_dir().join(family.replace(' ', "-"));
    std::fs::create_dir(&root).unwrap();
    let file = root.join("invalid.ttf");
    std::fs::write(&file, b"generated invalid font").unwrap();
    assert_eq!(fonts::add_source(&GeneratedFace { family: family.clone(), file: file.clone() }), 1);
    props(&mut s, json!({"font":family}));
    read(&mut s, 81031);
    let before = state(&s);
    let (resources, h) = complete(&mut s, 81031, "font_resources", 0);
    let resource = resources.iter().find(|r| r["family"] == family).unwrap();
    assert_eq!(resource["loaded"], true);
    assert_eq!(resource["parsed"], false);
    assert_eq!(h["allPrimaryResourcesLoadedAndParsed"], false);
    assert_eq!(h["writeAuthority"], false);
    assert_eq!(state(&s), before);
    std::fs::remove_file(&file).unwrap();
}
#[test]
fn font_resource_fingerprint_survives_unrelated_registry_additions_but_refreshes_actual_late_family_resolution() {
    let mut s = populated();
    props(&mut s, json!({"text":"ordinary Latin text","font":"Inter","font_style":"Regular"}));
    read(&mut s, 81040);
    let (_, before) = complete(&mut s, 81040, "font_resources", 0);
    let family = unique();
    let root = std::env::temp_dir().join(family.replace(' ', "-"));
    std::fs::create_dir(&root).unwrap();
    let file = root.join("generated.ttf");
    std::fs::write(&file, fonts::INTER_ITALIC).unwrap();
    assert_eq!(fonts::add_source(&GeneratedFace { family: family.clone(), file: file.clone() }), 1);
    let (_, after) = complete(&mut s, 81040, "font_resources", 0);
    assert_eq!(before, after);
    let late = unique();
    props(&mut s, json!({"font":late}));
    read(&mut s, 81041);
    let (_, missing) = complete(&mut s, 81041, "font_resources", 0);
    assert_eq!(missing["missingFont"], true);
    assert_eq!(fonts::add_source(&GeneratedFace { family: late.clone(), file: file.clone() }), 1);
    let (_, found) = complete(&mut s, 81041, "font_resources", 0);
    assert_eq!(found["missingFont"], false);
    assert_ne!(missing["layoutSha256"], found["layoutSha256"]);
    assert_ne!(missing["resourcesSha256"], found["resourcesSha256"]);
    std::fs::remove_file(&file).unwrap();
}
