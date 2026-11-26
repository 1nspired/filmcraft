use super::{project_plane, tests::token};
use crate::Session;
use filmcraft_project::ItemId;
use filmcraft_text::fonts::{self, FaceData, FaceInfo, FontSource};
use serde_json::{Value, json};
fn scope() -> Value {
    json!({"projectId":"p_caption_fonts","workspaceId":"b6e7b453-8457-4dab-ae33-15de4b6a4535"})
}
fn fixture() -> Session {
    let mut s = Session::default();
    s.execute("file.newProject", json!({"name":"Caption font evidence","workspace":scope()})).unwrap();
    s.path = Some("/generated/caption-fonts.fcproj".into());
    s.execute("file.newSequence", json!({"name":"Captions","fps":24,"width":480,"height":270,"video":1,"audio":1})).unwrap();
    s.execute("captions.add", json!({"text":"A😀é שלום wrapped caption words","seconds":0,"durationSeconds":2})).unwrap();
    s
}
fn state(s: &Session) -> Value {
    json!({"project":s.project,"editor":s.state,"revision":s.revision,"undo":s.history.undo,"redo":s.history.redo,"journal":s.journal,"path":s.path})
}
fn read(s: &mut Session, n: usize) -> Value {
    s.execute(project_plane::READ, json!({"token":token(n),"scope":scope(),"projectPath":s.path})).unwrap();
    let seq = s.state.active_sequence.unwrap();
    let track = &s.project.sequence(seq).unwrap().caption_tracks[0];
    json!({"token":token(n),"scope":scope(),"target":{"sequence":seq.0,"track":track.id.0,"caption":track.captions[0].id.0}})
}
#[test]
fn caption_font_resources_reuse_original_prepared_caption_glyphs_and_complete_font_hashes_without_editor_or_history_changes() {
    let mut s = fixture();
    let q = read(&mut s, 87000);
    let before = state(&s);
    let r = s.execute("agent.readCaptionFontResources", q).unwrap();
    assert_eq!(state(&s), before);
    assert_eq!(r["writeAuthority"], false);
    assert_eq!(r["sourceValidation"], "not-performed");
    let f = &r["fontState"];
    assert_eq!(f["fontFileState"], "not-inspected");
    assert_eq!(f["embeddingRights"], "not-established");
    assert_eq!(f["renderer"], "original_caption_layout_and_font_observer");
    let seq = s.project.sequence(ItemId(r["target"]["sequence"].as_u64().unwrap())).unwrap();
    let track = &seq.caption_tracks[0];
    let prepared = filmcraft_captions::burn::layout_caption(&track.captions[0], &track.style, 480, 270).unwrap();
    assert_eq!(f["lineCount"], prepared.lines.len());
    assert_eq!(f["glyphCount"], prepared.lines.iter().enumerate().map(|(i, _)| prepared.line_layout(i).unwrap().glyphs.len()).sum::<usize>());
    assert_eq!(f["shown"], true);
    assert_eq!(f["retainedBytesOnly"], true);
    assert_eq!(f["rendererSampling"], "one_exact_caption_canvas");
    let fresh = read(&mut s, 87001);
    let r2 = s.execute("agent.readCaptionFontResources", fresh).unwrap();
    assert_eq!(r["fontState"], r2["fontState"]);
}
struct Face {
    family: String,
}
impl FontSource for Face {
    fn faces(&self) -> Vec<FaceInfo> {
        vec![FaceInfo {
            family: self.family.clone(),
            style: "SemiBold".into(),
            weight: 600,
            italic: false,
            index: 0,
            data: FaceData::Static(fonts::INTER_SEMIBOLD),
            origin: "generated-test",
        }]
    }
}
#[test]
fn caption_font_resources_refresh_relevant_native_font_registration_but_ignore_unrelated_registry_growth() {
    let mut s = fixture();
    let sid = s.state.active_sequence.unwrap();
    let family = format!("Poema caption late {}", std::process::id());
    std::sync::Arc::make_mut(&mut s.project).sequence_mut(sid).unwrap().caption_tracks[0].style.font = family.clone();
    let q = read(&mut s, 87010);
    let before = state(&s);
    let a = s.execute("agent.readCaptionFontResources", q.clone()).unwrap();
    assert!(a["fontState"]["missingRequestedFamilies"].as_u64().unwrap() > 0);
    assert_eq!(fonts::add_source(&Face { family: family.clone() }), 1);
    let b = s.execute("agent.readCaptionFontResources", q.clone()).unwrap();
    assert_eq!(b["fontState"]["missingRequestedFamilies"], 0);
    assert_ne!(a["fontState"]["resourcesSha256"], b["fontState"]["resourcesSha256"]);
    assert_eq!(a["projectSha256"], b["projectSha256"]);
    fonts::add_source(&Face { family: format!("Unrelated {}", std::process::id()) });
    let c = s.execute("agent.readCaptionFontResources", q).unwrap();
    assert_eq!(b["fontState"], c["fontState"]);
    assert_eq!(state(&s), before);
}
#[test]
fn caption_font_resources_refuse_stale_foreign_unknown_unsafe_and_missing_caption_targets_without_writes() {
    let mut s = fixture();
    let q = read(&mut s, 87020);
    let before = state(&s);
    for i in 0..8 {
        let mut p = q.clone();
        match i {
            0 => p["scope"]["projectId"] = json!("foreign"),
            1 => p["target"]["sequence"] = json!(0),
            2 => p["target"]["track"] = json!(99999),
            3 => p["target"]["caption"] = json!(99999),
            4 => p["target"]["caption"] = json!(9_007_199_254_740_992u64),
            5 => p["target"]["frame"] = json!(0),
            6 => p["projection"] = json!(true),
            _ => p["token"] = json!(token(999999)),
        };
        assert!(s.execute("agent.readCaptionFontResources", p).is_err());
        assert_eq!(state(&s), before);
    }
    s.execute("file.newBin", json!({"name":"Later human"})).unwrap();
    let later = state(&s);
    assert!(s.execute("agent.readCaptionFontResources", q).is_err());
    assert_eq!(state(&s), later);
}

#[test]
fn caption_font_resources_preserve_empty_family_fallback_empty_cues_and_hostile_style_refusal_without_project_changes() {
    let mut s = fixture();
    let sid = s.state.active_sequence.unwrap();
    std::sync::Arc::make_mut(&mut s.project).sequence_mut(sid).unwrap().caption_tracks[0].style.font.clear();
    let q = read(&mut s, 87030);
    let before = state(&s);
    let r = s.execute("agent.readCaptionFontResources", q).unwrap();
    assert_eq!(r["fontState"]["missingRequestedFamilies"], 0);
    assert_eq!(state(&s), before);
    std::sync::Arc::make_mut(&mut s.project).sequence_mut(sid).unwrap().caption_tracks[0].captions[0].text.clear();
    let q = read(&mut s, 87031);
    let empty = state(&s);
    let r = s.execute("agent.readCaptionFontResources", q).unwrap();
    assert_eq!(r["fontState"]["shown"], false);
    assert_eq!(r["fontState"]["glyphCount"], 0);
    assert_eq!(r["fontState"]["lineCount"], 0);
    assert_eq!(state(&s), empty);
    let track = &s.project.sequence(sid).unwrap().caption_tracks[0];
    let c = track.captions[0].clone();
    let style = track.style.clone();
    for i in 0..5 {
        let mut bad = style.clone();
        match i {
            0 => bad.size = f32::NAN,
            1 => bad.margin = f32::INFINITY,
            2 => bad.line_spacing = 0.0,
            3 => bad.font = "bad\0family".into(),
            _ => bad.font = "x".repeat(16 * 1024 + 1),
        };
        assert!(super::graphics::observe_caption_fonts(&c, &bad, 480, 270).is_err());
    }
    assert_eq!(state(&s), empty);
}
