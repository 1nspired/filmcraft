//! Exact renderer observations, never a font-file grant or edit authorization.
use super::{characters, error, project_audit};
use crate::{EngineError, Result, Session};
use filmcraft_project::{
    ItemKind,
    graphic::{self, LayerContent, TextProps},
};
use filmcraft_render::graphic_clip;
use filmcraft_text::{
    Layout, TextStyle,
    fonts::{self, Face, FaceData, FaceId, Resolved},
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

pub(super) const MAX_GLYPHS: usize = 4_000_000;
const MAX_FACES: usize = 4096;
struct Resource {
    row: Value,
    key: String,
    loaded: bool,
    parsed: bool,
}
impl Resource {
    fn observe(face: &Face) -> Result<Self> {
        if !super::text(&face.info.family, 16 * 1024) || !super::text(&face.info.style, 16 * 1024) {
            return Err(error("native font description exceeds its bound"));
        }
        // Do not call data(): it clones and can load a different disk generation.
        let bytes = face.loaded_data();
        let loaded = bytes.is_some();
        // Once bytes exist font() only parses that retained generation, without I/O.
        let parsed = loaded && face.font().is_some();
        let kind = match &face.info.data {
            FaceData::Static(_) => "embedded",
            FaceData::Shared(_) => "registered_memory",
            FaceData::File(_) => "file",
        };
        let mut row = json!({"family":face.info.family,"style":face.info.style,"weight":face.info.weight,"italic":face.info.italic,
            "faceIndex":face.info.index,"origin":face.info.origin,"storage":kind,"loaded":loaded,"parsed":parsed,
            "bytes":bytes.map(<[u8]>::len),"bytesSha256":bytes.map(|v|format!("{:x}",Sha256::digest(v)))});
        let key = format!("{:x}", Sha256::digest(project_audit::value_bytes(&row)?));
        row["resourceSha256"] = json!(key);
        row["nativeFace"] = json!(face.id);
        Ok(Self { row, key, loaded, parsed })
    }
}
fn finite_style(style: &TextStyle) -> bool {
    super::text(&style.family, 16 * 1024)
        && super::text(&style.style, 16 * 1024)
        && [style.size, style.tracking, style.baseline_shift].iter().all(|n| n.is_finite())
        && style.size > 0.0
}
fn style_row(style: &TextStyle) -> Value {
    json!({"family":style.family,"style":style.style,"size":style.size,"tracking":style.tracking,"kerning":style.kerning,
        "ligatures":style.ligatures,"baselineShift":style.baseline_shift,"fauxBold":style.faux_bold,"fauxItalic":style.faux_italic,
        "caps":match style.caps {filmcraft_text::Caps::Normal=>"normal",filmcraft_text::Caps::All=>"all",filmcraft_text::Caps::Small=>"small"},"underline":style.underline})
}
fn feed(hash: &mut Sha256, value: &Value) -> Result<()> {
    let bytes = project_audit::value_bytes(value)?;
    hash.update((bytes.len() as u64).to_le_bytes());
    hash.update(bytes);
    Ok(())
}
struct Snapshot {
    layout: Arc<Layout>,
    styles: Vec<TextStyle>,
    resolutions: Vec<Resolved>,
    ranges: Vec<std::ops::Range<usize>>,
    resources: BTreeMap<FaceId, Resource>,
    state: Value,
}
impl Snapshot {
    fn request(&self, index: usize, local_ids: bool) -> Result<Value> {
        let style = self.styles.get(index).ok_or_else(|| error("native requested style missing"))?;
        let resolution = self.resolutions.get(index).ok_or_else(|| error("native requested resolution missing"))?;
        let resource = self.resources.get(&resolution.face).ok_or_else(|| error("native primary resource missing"))?;
        let mut row = json!({"position":index,"style":style_row(style),"primaryResourceSha256":resource.key,
            "missingFamily":resolution.missing,"synthBold":resolution.synth_bold,"synthItalic":resolution.synth_italic,
            "sourceBytes":if index==0 {None}else{self.ranges.get(index-1).map(|r|[r.start,r.end])}});
        if local_ids {
            row["nativePrimaryFace"] = json!(resolution.face);
        }
        Ok(row)
    }
    fn glyph(&self, index: usize, local_ids: bool) -> Result<Value> {
        let g = self.layout.glyphs.get(index).ok_or_else(|| error("native glyph missing"))?;
        let resolved = self.resolutions.get(usize::from(g.run)).ok_or_else(|| error("native glyph style missing"))?;
        let resource = self.resources.get(&g.face).ok_or_else(|| error("native glyph resource missing"))?;
        let mut row = json!({"position":index,"resourceSha256":resource.key,"glyph":g.id,"clusterByte":g.cluster,
            "x":g.x,"y":g.y,"size":g.size,"styleIndex":g.run,"synthBold":g.synth_bold,"synthItalic":g.synth_italic,
            "fallback":g.face!=resolved.face});
        if local_ids {
            row["nativeFace"] = json!(g.face);
        }
        Ok(row)
    }
}
fn snapshot(t: &TextProps) -> Result<Snapshot> {
    if t.text.chars().count() > characters::MAX_CHARACTERS || t.runs.len() > characters::MAX_CHARACTERS {
        return Err(error("font observation exceeds bounded complete text and run collection"));
    }
    if ![t.leading, t.box_width, t.box_height].iter().all(|v| v.is_finite()) {
        return Err(error("native paragraph style is not finite"));
    }
    let (base, paragraph) = graphic_clip::text_styles(t);
    let runs = graphic_clip::text_runs(t, &base);
    let mut styles = vec![base];
    styles.extend(runs.iter().map(|r| r.style.clone()));
    layout_snapshot(&t.text, &styles, &runs.iter().map(|r| r.range.clone()).collect::<Vec<_>>(), &paragraph, || graphic_clip::text_layout(t))
}
fn layout_snapshot(
    source_text: &str,
    styles: &[TextStyle],
    ranges: &[std::ops::Range<usize>],
    paragraph: &filmcraft_text::ParagraphStyle,
    make_layout: impl Fn() -> Arc<Layout>,
) -> Result<Snapshot> {
    if source_text.chars().count() > characters::MAX_CHARACTERS || styles.len() > characters::MAX_CHARACTERS.saturating_add(1) {
        return Err(error("font observation exceeds bounded complete text and style collection"));
    }
    if styles.iter().any(|s| !finite_style(s)) {
        return Err(error("native font style is not finite or bounded"));
    }
    // A first original resolution can scan native system fonts. Retry that
    // immutable input once; never mix registries silently within one proof.
    for _ in 0..2 {
        let generation = fonts::registry_revision();
        let layout = make_layout();
        if layout.glyphs.len() > MAX_GLYPHS
            || !layout.bounds.iter().all(|v| v.is_finite())
            || layout.glyphs.iter().any(|g| ![g.x, g.y, g.size].iter().all(|v| v.is_finite()) || g.size <= 0.0 || g.cluster > source_text.len())
        {
            return Err(error("native glyph result is not finite or exceeds its bound"));
        }
        let mut seen = BTreeMap::new();
        let resolutions = styles
            .iter()
            .map(|style| *seen.entry((&style.family, &style.style)).or_insert_with(|| fonts::resolve(&style.family, &style.style)))
            .collect::<Vec<_>>();
        let used = resolutions.iter().map(|r| r.face).chain(layout.glyphs.iter().map(|g| g.face)).collect::<BTreeSet<_>>();
        if used.len() > MAX_FACES {
            return Err(error("native complete font resource set exceeds its bound"));
        }
        let faces = fonts::all_faces();
        let mut resources = BTreeMap::new();
        for id in used {
            let face = faces.get(id).filter(|f| f.id == id).ok_or_else(|| error("native face identity is missing; refresh"))?;
            resources.insert(id, Resource::observe(face)?);
        }
        let mut value = Snapshot { layout, styles: styles.to_vec(), resolutions, ranges: ranges.to_vec(), resources, state: Value::Null };
        let mut request_hash = Sha256::new();
        request_hash.update(b"poema-native-font-requests-v1");
        for i in 0..value.styles.len() {
            feed(&mut request_hash, &value.request(i, false)?)?;
        }
        let requests = format!("{:x}", request_hash.finalize());
        let mut resource_hash = Sha256::new();
        resource_hash.update(b"poema-native-font-resources-v1");
        let mut resource_keys = value.resources.values().map(|r| r.key.as_str()).collect::<Vec<_>>();
        resource_keys.sort_unstable();
        for key in resource_keys {
            resource_hash.update(key.as_bytes());
        }
        let resources = format!("{:x}", resource_hash.finalize());
        let mut glyph_hash = Sha256::new();
        glyph_hash.update(b"poema-native-font-glyphs-v1");
        for i in 0..value.layout.glyphs.len() {
            feed(&mut glyph_hash, &value.glyph(i, false)?)?;
        }
        let glyphs = format!("{:x}", glyph_hash.finalize());
        let text = format!("{:x}", Sha256::digest(source_text.as_bytes()));
        let mut layout_hash = Sha256::new();
        layout_hash.update(b"poema-native-font-layout-v1");
        feed(
            &mut layout_hash,
            &json!({"text":text,"requests":requests,"resources":resources,"glyphs":glyphs,
            "bounds":value.layout.bounds,"overflow":value.layout.overflow,"missingFont":value.layout.missing_font,
            "underlines":value.layout.underlines,"underlineRuns":value.layout.underline_runs,
            "paragraph":{"align":match paragraph.align {filmcraft_text::Align::Left=>"left",filmcraft_text::Align::Center=>"center",filmcraft_text::Align::Right=>"right",filmcraft_text::Align::Justify=>"justify"},
            "leading":paragraph.leading,"width":paragraph.width,"height":paragraph.height,"rtl":paragraph.rtl,"vertical":paragraph.vertical}}),
        )?;
        let parsed = value.layout.glyphs.iter().all(|g| value.resources.get(&g.face).is_some_and(|r| r.parsed));
        let loaded = value.resolutions.iter().all(|r| value.resources.get(&r.face).is_some_and(|r| r.loaded && r.parsed));
        let mut fallback = 0usize;
        for g in &value.layout.glyphs {
            if value.resolutions.get(usize::from(g.run)).is_some_and(|r| r.face != g.face) {
                fallback = fallback.saturating_add(1);
            }
        }
        value.state = json!({"kind":"native_font_resolution","version":1,"textSha256":text,"textCharacters":source_text.chars().count(),
            "requestsSha256":requests,"resourcesSha256":resources,"glyphsSha256":glyphs,"layoutSha256":format!("{:x}",layout_hash.finalize()),
            "requestCount":value.styles.len(),"resourceCount":value.resources.len(),"glyphCount":value.layout.glyphs.len(),
            "missingRequestedFamilies":value.resolutions.iter().filter(|r|r.missing).count(),"zeroGlyphCount":value.layout.glyphs.iter().filter(|g|g.id==0).count(),
            "fallbackGlyphCount":fallback,"syntheticGlyphCount":value.layout.glyphs.iter().filter(|g|g.synth_bold||g.synth_italic).count(),
            "bounds":value.layout.bounds,"overflow":value.layout.overflow,"missingFont":value.layout.missing_font,
            "allRenderedGlyphResourcesParsed":parsed,"allPrimaryResourcesLoadedAndParsed":loaded,
            "retainedBytesOnly":true,"fontFileState":"not-inspected","embeddingRights":"not-established","writeAuthority":false});
        if fonts::registry_revision() == generation {
            return Ok(value);
        }
    }
    Err(error("native font registry changed during observation; refresh the read"))
}
fn evaluated(s: &Session, clip: &filmcraft_project::TrackItem, effect: &filmcraft_project::EffectInstance, frame: i64) -> Result<Snapshot> {
    if !(0..=1_000_000).contains(&frame) {
        return Err(error("explicit bounded native font evaluation frame required"));
    }
    let at = super::clock(s, frame)?;
    let mut value = evaluated_at(&s.project, clip, effect, at)?;
    value.state["frame"] = json!(frame);
    Ok(value)
}
fn evaluated_at(
    project: &filmcraft_project::Project,
    clip: &filmcraft_project::TrackItem,
    effect: &filmcraft_project::EffectInstance,
    at: filmcraft_time::Tick,
) -> Result<Snapshot> {
    let end = clip.start.0.checked_add(clip.duration.0).filter(|end| *end > clip.start.0).ok_or_else(|| error("graphic clock is invalid"))?;
    if at < clip.start || at.0 >= end {
        return Err(error("explicit font frame is outside the graphic"));
    }
    let media = crate::graphics::layout_source_time(clip, at)?;
    if let Some(meta) = clip.graphic.as_deref().filter(|m| m.has_responsive_time()) {
        let u = media.0.checked_sub(clip.source_in.0).ok_or_else(|| error("responsive graphic clock exceeds its bound"))?;
        let duration = (clip.duration.0 as f64 * clip.speed.abs().max(1e-9)).round();
        if !duration.is_finite() || !(0.0..9_223_372_036_854_775_808.0).contains(&duration) {
            return Err(error("responsive duration exceeds its bound"));
        }
        let d = duration as i64;
        let remaining = d.checked_sub(u).ok_or_else(|| error("responsive graphic endpoint exceeds its bound"))?;
        if meta.design_duration.0.checked_sub(remaining).is_none()
            || d.checked_sub(meta.outro.0.max(0)).is_none()
            || meta.design_duration.0.checked_sub(meta.outro.0.max(0)).is_none()
        {
            return Err(error("responsive graphic remapping exceeds its bound"));
        }
    }
    let native_time = graphic_clip::graphic_time(clip, media).0;
    let canvas = match project.item(clip.item).map(|i| &i.kind) {
        Some(ItemKind::Graphic { width, height, .. }) => (*width, *height),
        _ => return Err(EngineError::NoSequence),
    };
    let evaluated = graphic::eval_layer(effect, native_time, canvas).ok_or_else(|| error("native text layer cannot be evaluated"))?;
    let LayerContent::Text(t) = &evaluated.content else {
        return Err(error("native resource target is not text"));
    };
    let mut value = snapshot(t)?;
    value.state["identity"] = json!(clip.identity.as_ref().map(|i| &i.id));
    value.state["layerId"] = json!(effect.instance_id);
    value.state["nativeGraphicTimeTicks"] = json!(native_time.0.to_string());
    value.state["enabled"] = json!(effect.enabled);
    Ok(value)
}
pub(super) fn observe_effect(s: &Session, clip: &filmcraft_project::TrackItem, effect: &filmcraft_project::EffectInstance, frame: i64) -> Result<Value> {
    Ok(evaluated(s, clip, effect, frame)?.state)
}
pub(super) fn page(s: &Session, identity: &str, layer_id: &str, frame: i64, offset: usize, limit: usize, kind: &str) -> Result<(usize, Vec<Value>, Value)> {
    let clip = characters::selected_clip(s, identity)?;
    let effect = characters::text_layer(clip, layer_id)?;
    let value = evaluated(s, clip, effect, frame)?;
    let total = match kind {
        "font_glyphs" => value.layout.glyphs.len(),
        "font_requests" => value.styles.len(),
        _ => value.resources.len(),
    };
    if offset > total {
        return Err(error("font page is outside the complete evaluated collection"));
    }
    let end = offset.saturating_add(limit).min(total);
    let rows = match kind {
        "font_glyphs" => (offset..end).map(|i| value.glyph(i, true)).collect::<Result<Vec<_>>>()?,
        "font_requests" => (offset..end).map(|i| value.request(i, true)).collect::<Result<Vec<_>>>()?,
        _ => value
            .resources
            .values()
            .skip(offset)
            .take(end - offset)
            .enumerate()
            .map(|(i, r)| {
                let mut row = r.row.clone();
                row["position"] = json!(offset + i);
                row
            })
            .collect(),
    };
    Ok((total, rows, value.state))
}

pub(super) fn observe_caption(caption: &filmcraft_project::Caption, style: &filmcraft_project::CaptionStyle, width: usize, height: usize) -> Result<Value> {
    validate_caption_style(style, width, height)?;
    if caption.text.len() > 4 * characters::MAX_CHARACTERS || caption.text.chars().count() > characters::MAX_CHARACTERS {
        return Err(error("caption text exceeds complete native resource bound"));
    }
    for _ in 0..2 {
        let generation = fonts::registry_revision();
        let prepared = filmcraft_captions::burn::layout_caption(caption, style, width, height);
        let mut lines_sha = Sha256::new();
        lines_sha.update(b"poema-native-caption-font-lines-v1");
        let mut descriptors = BTreeSet::new();
        let mut counts = [0usize; 6];
        let mut parsed = true;
        let mut loaded = true;
        let mut observe = |text: &str, ts: &TextStyle, layout: Arc<Layout>, placement: Value| -> Result<()> {
            let value = layout_snapshot(text, std::slice::from_ref(ts), &[], &filmcraft_text::ParagraphStyle::default(), || layout.clone())?;
            for resource in value.resources.values() {
                descriptors.insert(resource.key.clone());
            }
            for (index, key) in
                ["requestCount", "glyphCount", "missingRequestedFamilies", "zeroGlyphCount", "fallbackGlyphCount", "syntheticGlyphCount"].iter().enumerate()
            {
                let n = value.state[key].as_u64().and_then(|n| usize::try_from(n).ok()).ok_or_else(|| error("native caption resource count missing"))?;
                let count = counts.get_mut(index).ok_or_else(|| error("native caption count index missing"))?;
                *count = count.checked_add(n).filter(|n| *n <= MAX_GLYPHS).ok_or_else(|| error("complete caption resource count exceeds native bound"))?;
            }
            parsed &= value.state["allRenderedGlyphResourcesParsed"] == true;
            loaded &= value.state["allPrimaryResourcesLoadedAndParsed"] == true;
            feed(&mut lines_sha, &json!({"placement":placement,"font":value.state}))?;
            Ok(())
        };
        let line_count = prepared.as_ref().map_or(0, |p| p.lines.len());
        let placement = if let Some(plan) = &prepared {
            if ![plan.top, plan.line_height, plan.ascent, plan.descent, plan.pad_x, plan.block_height, plan.scale].iter().all(|n| n.is_finite())
                || plan.xs.iter().chain(&plan.widths).any(|n| !n.is_finite())
                || plan.lines.len() != plan.xs.len()
            {
                return Err(error("caption layout is not finite or complete"));
            }
            for (index, line) in plan.lines.iter().enumerate() {
                let layout = plan.line_layout(index).ok_or_else(|| error("caption shaped line missing"))?.clone();
                let x = plan.xs.get(index).ok_or_else(|| error("caption placement missing"))?;
                let top = plan.top + plan.line_height * index as f32;
                let baseline = (top + (plan.line_height - (plan.ascent + plan.descent)) / 2.0 + plan.ascent).round();
                if !baseline.is_finite() {
                    return Err(error("caption baseline is not finite"));
                }
                observe(line, &plan.style, layout, json!({"line":index,"x":x,"baseline":baseline}))?;
            }
            json!({"top":plan.top,"lineHeight":plan.line_height,"ascent":plan.ascent,"descent":plan.descent,"padX":plan.pad_x,"blockHeight":plan.block_height,"scale":plan.scale,"widths":plan.widths,"xs":plan.xs})
        } else {
            // No visible lines: inspect the actual declared primary style without inventing glyphs.
            let ts = filmcraft_captions::burn::caption_style(&style.font, (style.size * height as f32 / 1080.0).max(4.0));
            observe("", &ts, filmcraft_text::layout("", &ts, &filmcraft_text::ParagraphStyle::default()), json!({"emptyDeclaredStyle":true}))?;
            Value::Null
        };
        let mut resources_sha = Sha256::new();
        resources_sha.update(b"poema-native-caption-font-resources-v1");
        for key in &descriptors {
            resources_sha.update(key.as_bytes());
        }
        let lines = format!("{:x}", lines_sha.finalize());
        let state = json!({"kind":"native_caption_font_resolution","version":1,"captionSha256":format!("{:x}",Sha256::digest(project_audit::value_bytes(caption)?)),"styleSha256":format!("{:x}",Sha256::digest(project_audit::value_bytes(style)?)),"canvas":[width,height],
            "lineCount":line_count,"shown":line_count>0,"requestCount":counts[0],"resourceDescriptorCount":descriptors.len(),"glyphCount":counts[1],"missingRequestedFamilies":counts[2],"zeroGlyphCount":counts[3],"fallbackGlyphCount":counts[4],"syntheticGlyphCount":counts[5],
            "linesSha256":lines,"resourcesSha256":format!("{:x}",resources_sha.finalize()),"layoutSha256":format!("{:x}",Sha256::digest(project_audit::value_bytes(&json!({"caption":caption,"style":style,"canvas":[width,height],"placement":placement,"lines":lines}))?)),
            "allRenderedGlyphResourcesParsed":parsed,"allPrimaryResourcesLoadedAndParsed":loaded,"renderer":"original_caption_layout_and_font_observer","rendererSampling":"one_exact_caption_canvas","retainedBytesOnly":true,"fontFileState":"not-inspected","embeddingRights":"not-established","writeAuthority":false});
        if fonts::registry_revision() == generation {
            return Ok(state);
        }
    }
    Err(error("font registry changed across complete caption observation; refresh"))
}

pub(super) fn observe_effect_at(
    project: &filmcraft_project::Project,
    clip: &filmcraft_project::TrackItem,
    effect: &filmcraft_project::EffectInstance,
    at: filmcraft_time::Tick,
) -> Result<Value> {
    let mut value = evaluated_at(project, clip, effect, at)?.state;
    value["timelineTicks"] = json!(at.0.to_string());
    Ok(value)
}
pub(super) fn observe_master(
    project: &filmcraft_project::Project,
    item: filmcraft_project::ItemId,
    effect: &filmcraft_project::EffectInstance,
    at: filmcraft_time::Tick,
) -> Result<Value> {
    let canvas = match project.item(item).map(|i| &i.kind) {
        Some(ItemKind::Graphic { width, height, .. }) => (*width, *height),
        _ => return Err(error("source graphic canvas missing")),
    };
    let evaluated = graphic::eval_layer(effect, at, canvas).ok_or_else(|| error("source master text layer cannot be evaluated"))?;
    let LayerContent::Text(t) = &evaluated.content else {
        return Err(error("source master layer is not text"));
    };
    let mut value = snapshot(t)?.state;
    value["nativeGraphicTimeTicks"] = json!(at.0.to_string());
    value["enabled"] = json!(effect.enabled);
    Ok(value)
}

fn validate_caption_style(style: &filmcraft_project::CaptionStyle, width: usize, height: usize) -> Result<()> {
    // The original renderer permits an empty family as Inter fallback. Validate
    // numeric style bounds through the existing service without rewriting it.
    let mut checked = style.clone();
    checked.font = "Inter".into();
    if !(1..=32768).contains(&width)
        || !(1..=32768).contains(&height)
        || !crate::captions::valid_style(&checked)
        || style.font.len() > 16 * 1024
        || style.font.contains('\0')
    {
        return Err(error("finite bounded native caption style and explicit canvas required"));
    }
    Ok(())
}

pub(super) fn observe_caption_style(style: &filmcraft_project::CaptionStyle, width: usize, height: usize) -> Result<Value> {
    validate_caption_style(style, width, height)?;
    for _ in 0..2 {
        let generation = fonts::registry_revision();
        let ts = filmcraft_captions::burn::caption_style(&style.font, (style.size * height as f32 / 1080.0).max(4.0));
        let value = layout_snapshot("", std::slice::from_ref(&ts), &[], &filmcraft_text::ParagraphStyle::default(), || {
            filmcraft_text::layout("", &ts, &filmcraft_text::ParagraphStyle::default())
        })?;
        if fonts::registry_revision() == generation {
            return Ok(
                json!({"kind":"native_caption_declared_font_resolution","version":1,"styleSha256":format!("{:x}",Sha256::digest(project_audit::value_bytes(style)?)),"canvas":[width,height],"caption":null,"shown":false,"fontState":value.state,"rendererSampling":"declared_empty_caption_track_style_only","retainedBytesOnly":true,"fontFileState":"not-inspected","embeddingRights":"not-established","writeAuthority":false}),
            );
        }
    }
    Err(error("font registry changed during empty caption track observation"))
}

#[cfg(test)]
mod native_clock_tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn explicit_project_tick_font_observation_preserves_original_frame_clock_header_without_live_seek() {
        let mut s = Session::default();
        s.execute("file.newSequence", json!({"name":"Clock oracle","fps":24,"width":128,"height":128,"video":1,"audio":1})).unwrap();
        s.execute("graphics.newText", json!({"text":"A😀é clock","font":"Inter","fontStyle":"Regular"})).unwrap();
        let q = s.active_sequence().unwrap();
        let c = q.video_tracks[0].items[0].clone();
        let e = c.effects.iter().find(|e| e.effect == graphic::TEXT_LAYER).unwrap();
        let before = json!({"project":s.project,"state":s.state,"revision":s.revision,"history":s.history.undo});
        let mut frame = observe_effect(&s, &c, e, 0).unwrap();
        let mut tick = observe_effect_at(&s.project, &c, e, filmcraft_time::Tick(0)).unwrap();
        frame.as_object_mut().unwrap().remove("frame");
        tick.as_object_mut().unwrap().remove("timelineTicks");
        assert_eq!(frame, tick);
        assert_eq!(json!({"project":s.project,"state":s.state,"revision":s.revision,"history":s.history.undo}), before);
    }
}
