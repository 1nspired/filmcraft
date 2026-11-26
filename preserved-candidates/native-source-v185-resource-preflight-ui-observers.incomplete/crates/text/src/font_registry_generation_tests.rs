//! Regression for font registration after a shared native layout was cached.
//! Uses only existing bundled font bytes; no system scan or external asset.
use crate::{ParagraphStyle, StyleRun, TextStyle, fonts, layout, layout_rich};

struct RegisteredStyle;
const STYLE: &str = "Poema Cache Registry Test Italic";
impl fonts::FontSource for RegisteredStyle {
    fn faces(&self) -> Vec<fonts::FaceInfo> {
        vec![fonts::FaceInfo {
            family: "Inter".into(),
            style: STYLE.into(),
            weight: 400,
            italic: true,
            index: 0,
            data: fonts::FaceData::Static(fonts::INTER_ITALIC),
            origin: "generated-test",
        }]
    }
}

#[test]
fn cached_plain_and_rich_layouts_use_newly_registered_native_faces() {
    let style = TextStyle { family: "Inter".into(), style: STYLE.into(), ..Default::default() };
    let base = TextStyle { family: "Inter".into(), style: "Regular".into(), ..Default::default() };
    let paragraph = ParagraphStyle::default();
    let runs = [StyleRun { range: 0..1, style: style.clone() }];
    // The original resolver chooses an existing Inter face for this absent
    // style; using a present family avoids the lazy system font scan.
    let before = layout("Poema", &style, &paragraph);
    let rich_before = layout_rich("Pa", &base, &runs, &paragraph);
    assert!(!before.glyphs.is_empty());
    let old_face = before.glyphs[0].face;
    assert_eq!(fonts::add_source(&RegisteredStyle), 1);
    let registered = fonts::all_faces().into_iter().find(|face| face.info.family == "Inter" && face.info.style == STYLE).unwrap().id;
    assert_ne!(old_face, registered);
    let after = layout("Poema", &style, &paragraph);
    assert!(after.glyphs.iter().all(|glyph| glyph.face == registered));
    assert!(!std::sync::Arc::ptr_eq(&before, &after));
    let rich_after = layout_rich("Pa", &base, &runs, &paragraph);
    assert_eq!(rich_after.glyphs.iter().find(|g| g.cluster == 0).unwrap().face, registered);
    assert_eq!(rich_after.glyphs.iter().find(|g| g.cluster == 1).unwrap().face, rich_before.glyphs.iter().find(|g| g.cluster == 1).unwrap().face);
    // Already returned immutable layouts keep their original face references.
    assert!(before.glyphs.iter().all(|glyph| glyph.face == old_face));
    assert_ne!(rich_before.glyphs.iter().find(|g| g.cluster == 0).unwrap().face, registered);
}
