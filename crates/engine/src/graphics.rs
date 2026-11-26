//! Graphics commands (`graphics.*`, `fonts.list`): graphic clips with text and shape layers.
//!
//! A graphic clip's layers are the `graphic_text` / `graphic_shape` effect instances on the clip
//! (see `filmcraft_project::graphic`). Commands address a clip by `clip` (default: the first
//! selected graphic clip, else the topmost graphic clip under the playhead) and a layer by
//! `layer` = index among the clip's graphic layers, 0 = back (default: the selected layer, else
//! the frontmost). Property values are keyframe-aware: on an animated property they set the value
//! at the playhead.

use filmcraft_geom::Vec2;
use filmcraft_project::graphic::{self, LayerContent, SHAPE_OPTS, eval_layer, layer_display_name, layer_indices, new_shape_layer, new_text_layer};
use filmcraft_project::graphic_design::{PinNode, PinTarget, resolve_pins};
use filmcraft_project::{ClipId, EffectInstance, ItemId, ItemKind, Label, Param, ParamValue, Project, Sequence, TrackKind};
use filmcraft_render::graphic_clip::{graphic_specs, graphic_time, item_layer_specs, layer_bounds, layer_local_bounds, layer_quad, text_layout};
use filmcraft_time::{Tick, TimeRange};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

use crate::commands::{CommandSpec, always, bad, bool_p, f64_p, has_seq, str_p, time_p, u64_p};
use crate::{EngineError, Result, Session};

type Run = fn(&mut Session, &Value) -> Result<Value>;
type Enabled = fn(&Session) -> std::result::Result<(), String>;

fn spec(
    id: &'static str,
    label: &'static str,
    menu: &'static [&'static str],
    shortcut: Option<&'static str>,
    params: &'static str,
    enabled: Enabled,
    run: Run,
) -> CommandSpec {
    CommandSpec { id, label, menu, shortcut, params, enabled, run, journal: true }
}
fn query(id: &'static str, label: &'static str, params: &'static str, run: Run) -> CommandSpec {
    CommandSpec { id, label, menu: &[], shortcut: None, params, enabled: always, run, journal: false }
}

/// Whether a track item is a graphic clip.
pub fn is_graphic(s: &Session, seq: &Sequence, c: ClipId) -> bool {
    seq.find_item(c).and_then(|(_, it)| s.project.item(it.item)).is_some_and(|p| matches!(p.kind, ItemKind::Graphic { .. }))
}

/// The graphic clip a command acts on: `clip`, else the first selected graphic clip, else the
/// topmost graphic clip under the playhead.
pub fn target_clip(s: &Session, p: &Value) -> Result<Option<ClipId>> {
    let explicit = u64_p(p, "clip")?;
    let Some(seq) = s.active_sequence() else { return Ok(None) };
    if let Some(c) = explicit.map(ClipId) {
        return Ok(is_graphic(s, seq, c).then_some(c));
    }
    if let Some(c) = s.state.selection.iter().copied().find(|c| is_graphic(s, seq, *c)) {
        return Ok(Some(c));
    }
    let t = s.playhead();
    Ok(seq.video_tracks.iter().rev().filter_map(|tr| tr.item_at(t)).map(|it| it.id).find(|c| is_graphic(s, seq, *c)))
}

fn has_graphic(s: &Session) -> std::result::Result<(), String> {
    has_seq(s)?;
    target_clip(s, &Value::Null).map_err(|e| e.to_string())?.map(|_| ()).ok_or_else(|| "select a graphic clip".into())
}

/// Effect index of graphic layer `layer` (or the selected / frontmost layer) of `clip`.
pub(crate) fn layer_effect_index(s: &Session, clip: ClipId, p: &Value) -> Result<(usize, usize)> {
    let seq = s.active_sequence().ok_or(EngineError::NoSequence)?;
    let (_, it) = seq.find_item(clip).ok_or_else(|| bad("graphics", "no such clip"))?;
    let idx = layer_indices(&it.effects);
    if idx.is_empty() {
        return Err(bad("graphics", "the graphic has no layers"));
    }
    let l = match p.get("layer") {
        Some(Value::Number(n)) => {
            n.as_u64().and_then(|v| usize::try_from(v).ok()).ok_or_else(|| bad("graphics", "layer must be a nonnegative integer index or a layer name"))?
        }
        Some(Value::String(name)) => (0..idx.len())
            .find(|&i| layer_display_name(&it.effects[idx[i]], i).eq_ignore_ascii_case(name))
            .ok_or_else(|| bad("graphics", format!("no layer named `{name}`")))?,
        None => s.state.graphic_layers.first().copied().filter(|l| *l < idx.len()).unwrap_or(idx.len() - 1),
        Some(_) => return Err(bad("graphics", "layer must be a nonnegative integer index or a layer name")),
    };
    let e = *idx.get(l).ok_or_else(|| bad("graphics", format!("no layer {l}")))?;
    Ok((l, e))
}

pub(crate) fn vec2_p(p: &Value, k: &str) -> Option<Vec2> {
    let a = p.get(k)?.as_array()?;
    Some(Vec2::new(a.first()?.as_f64()?, a.get(1)?.as_f64()?))
}

/// Find or create the graphic source item for the active sequence's frame size (source graphics
/// have their own items and are never reused here).
pub(crate) fn graphic_source(p: &mut filmcraft_project::Project, w: u32, h: u32, rate: filmcraft_time::FrameRate) -> ItemId {
    if let Some((id, _)) = p.items.iter().find(|(id, i)| {
        !p.source_graphics.contains_key(id) && matches!(i.kind, ItemKind::Graphic { width, height, rate: r } if width == w && height == h && r == rate)
    }) {
        return *id;
    }
    p.add_item("Graphic", Label::Rose, ItemKind::Graphic { width: w, height: h, rate }, None)
}

/// Place a new graphic clip holding `layer` at the playhead: on the first video track above the
/// topmost clip at the playhead that is free for the duration (a track is added if needed).
fn new_graphic_clip(s: &mut Session, cmd: &str, layer: filmcraft_project::EffectInstance, name: &str, p: &Value) -> Result<ClipId> {
    let duration = graphic_duration(s, cmd, p)?;
    // Here `seconds` is duration, not the generic time parser's placement alias.
    let mut placement = p.clone();
    if let Some(params) = placement.as_object_mut() {
        params.remove("seconds");
    }
    place_video_clip(s, cmd, name, &placement, "New Graphic", vec![layer], move |pr, (w, h, rate)| {
        let src = graphic_source(pr, w, h, rate);
        (src, duration)
    })
}

/// Exact native duration alternatives for callers that already work in frames/ticks.
/// The existing human seconds input keeps its native rounding and minimum.
fn graphic_duration(s: &Session, command: &str, p: &Value) -> Result<Tick> {
    let rate = s.sequence_rate();
    if ["durationFrames", "durationTicks", "seconds"].iter().filter(|key| p.get(**key).is_some()).count() > 1 {
        return Err(bad(command, "choose one duration: durationFrames, durationTicks or seconds"));
    }
    if let Some(value) = p.get("durationFrames") {
        let frames = value.as_i64().filter(|v| *v > 0).ok_or_else(|| bad(command, "durationFrames must be a positive integer"))?;
        let duration = rate.tick_of(frames);
        if duration <= Tick::ZERO || rate.frame_at(duration) != frames {
            return Err(bad(command, "durationFrames exceeds the native clock"));
        }
        return Ok(duration);
    }
    if let Some(value) = p.get("durationTicks") {
        let ticks = value.as_i64().filter(|v| *v > 0).ok_or_else(|| bad(command, "durationTicks must be positive integer ticks"))?;
        let duration = Tick(ticks);
        if rate.snap(duration) != duration {
            return Err(bad(command, "durationTicks must lie on a native frame boundary"));
        }
        return Ok(duration);
    }
    let seconds = f64_p(p, "seconds").unwrap_or(5.0).max(0.01);
    Ok(rate.snap_nearest(Tick::from_seconds_f64(seconds)).max(rate.frame_duration()))
}

/// Place a video clip of the item `source` returns (with its duration) at the playhead (or
/// `time`), above the clips there, as one undo step; selects it. `cmd` is the command that asked,
/// for its errors.
pub(crate) fn place_video_clip(
    s: &mut Session,
    cmd: &str,
    name: &str,
    p: &Value,
    label: &str,
    extra: Vec<filmcraft_project::EffectInstance>,
    source: impl FnOnce(&mut filmcraft_project::Project, (u32, u32, filmcraft_time::FrameRate)) -> (ItemId, Tick),
) -> Result<ClipId> {
    let seq_id = s.state.active_sequence.ok_or(EngineError::NoSequence)?;
    let t = time_p(s, p, "").unwrap_or_else(|| s.playhead());
    let want_track = u64_p(p, "track")?.map(|v| usize::try_from(v).map_err(|_| bad(cmd, "track index exceeds native width"))).transpose()?;
    let name = name.to_string();
    s.edit(label, |pr, st| {
        let (w, h, rate) = {
            let q = pr.sequence(seq_id).ok_or(EngineError::NoSequence)?;
            (q.settings.width, q.settings.height, q.settings.frame_rate)
        };
        let (src, dur) = source(pr, (w, h, rate));
        // an imported picture (`graphics.newFromFile`) keeps its own size: centre its anchor in it
        let src_size = pr.source_size(src).unwrap_or((w, h));
        let t = rate.snap(t);
        let dur = dur.max(rate.frame_duration());
        let mut ti = pr.make_track_item(src, TrackKind::Video, t, TimeRange::new(Tick::ZERO, dur), rate).ok_or_else(|| bad(cmd, "bad item"))?;
        ti.name = name;
        ti.effects.extend(extra);
        for e in &mut ti.effects {
            filmcraft_project::resolve_auto_points(e, (w, h), src_size);
        }
        let id = ti.id;
        let track_id = filmcraft_project::TrackId(pr.alloc_id());
        let q = pr.sequence_mut(seq_id).ok_or(EngineError::NoSequence)?;
        let range = TimeRange::new(t, dur);
        let free = |tr: &filmcraft_project::Track| !tr.items.iter().any(|i| i.range().overlaps(&range));
        let idx = match want_track {
            Some(i) if q.video_tracks.get(i).is_some_and(|tr| tr.locked) => return Err(bad(cmd, format!("V{} is locked", i + 1))),
            Some(i) if i < q.video_tracks.len() && free(&q.video_tracks[i]) => i,
            Some(i) if i < q.video_tracks.len() => return Err(bad(cmd, format!("V{} is not free here", i + 1))),
            _ => {
                let top = q.video_tracks.iter().rposition(|tr| tr.item_at(t).is_some()).map_or(0, |k| k + 1);
                match (top..q.video_tracks.len()).find(|&i| free(&q.video_tracks[i]) && !q.video_tracks[i].locked) {
                    Some(i) => i,
                    None => {
                        let n = q.video_tracks.len() + 1;
                        q.video_tracks.push(filmcraft_project::Track::new(track_id, TrackKind::Video, format!("Video {n}")));
                        q.video_tracks.len() - 1
                    }
                }
            }
        };
        q.video_tracks[idx].items.push(ti);
        q.video_tracks[idx].sort();
        q.check().map_err(EngineError::Other)?;
        st.selection = vec![id];
        st.graphic_layers = vec![0];
        Ok(id)
    })
}

/// Add a layer to an existing graphic clip; returns the new layer index.
fn add_layer(s: &mut Session, clip: ClipId, mut layer: filmcraft_project::EffectInstance) -> Result<usize> {
    let frame = s.active_sequence().map(|q| (q.settings.width, q.settings.height)).unwrap_or((1920, 1080));
    filmcraft_project::resolve_auto_points(&mut layer, frame, frame);
    s.edit_sequence("Add Graphic Layer", |q, _, st| {
        let (_, it) = q.find_item_mut(clip).ok_or(filmcraft_edit::EditError::NoItem(clip))?;
        // new layers go in front: after the last graphic layer
        let pos = layer_indices(&it.effects).last().map_or(it.effects.len(), |l| l + 1);
        it.effects.insert(pos, layer);
        let n = layer_indices(&it.effects).len() - 1;
        st.selection = vec![clip];
        st.graphic_layers = vec![n];
        Ok(n)
    })
}

/// An explicit add-layer target must not silently become a request for a new clip.
fn new_layer_target(s: &Session, p: &Value, command: &str) -> Result<Option<ClipId>> {
    let Some(clip) = u64_p(p, "clip")?.map(ClipId) else { return Ok(None) };
    if s.active_sequence().is_some_and(|q| is_graphic(s, q, clip)) { Ok(Some(clip)) } else { Err(bad(command, "explicit clip is absent or is not a graphic")) }
}

/// Map friendly property names to parameter ids.
fn param_id(k: &str) -> &str {
    match k {
        "fontSize" => "size",
        "fontStyle" | "style" => "font_style",
        "fillColor" | "color" => "fill_color",
        "fillKind" => "fill_kind",
        "gradientStart" => "gradient_start",
        "gradientEnd" => "gradient_end",
        "gradientAngle" => "gradient_angle",
        "strokeColor" => "stroke_color",
        "strokeWidth" => "stroke_width",
        "backgroundColor" => "background_color",
        "shadowColor" => "shadow_color",
        "baselineShift" => "baseline_shift",
        "fauxBold" => "faux_bold",
        "fauxItalic" => "faux_italic",
        "boxWidth" => "box_width",
        "boxHeight" => "box_height",
        "anchorPoint" => "anchor",
        "cornerRadius" => "corner_radius",
        other => other,
    }
}

/// JSON value for a property, accepting names for choices ("center", "small caps", "ellipse"…).
pub(crate) fn to_param(template: &ParamValue, id: &str, v: &Value) -> Option<ParamValue> {
    if let (ParamValue::Choice(_), Value::String(name)) = (template, v) {
        let opts: &[&str] = match id {
            "align" => graphic::ALIGN_OPTS,
            "caps" => graphic::CAPS_OPTS,
            "stroke_type" | "stroke2_type" => graphic::STROKE_OPTS,
            "fill_kind" => graphic::FILL_KIND_OPTS,
            "shape" => SHAPE_OPTS,
            _ => &[],
        };
        let n = name.to_ascii_lowercase().replace('_', " ");
        let n = if n == "centre" { "center".to_string() } else { n };
        return opts.iter().position(|o| o.to_ascii_lowercase() == n).map(|i| ParamValue::Choice(i as u32));
    }
    crate::commands::json_to_param(template, v)
}

/// Parameter `id` of a layer, created from the layer definition when the layer was saved before
/// the parameter existed.
fn param_entry<'a>(e: &'a mut EffectInstance, id: &str) -> Option<&'a mut Param> {
    if !e.params.contains_key(id) {
        let d = filmcraft_project::find_effect(&e.effect)?.param(id)?.default.clone();
        e.params.insert(id.to_string(), Param::new(d));
    }
    e.params.get_mut(id)
}

/// Set properties `props` on layer `e` (keyframe-aware at media time `mt`). Changing the text
/// keeps per-character styles on their characters.
pub(crate) fn apply_props(e: &mut EffectInstance, props: &serde_json::Map<String, Value>, mt: Tick) -> Result<()> {
    for (k, v) in props {
        if k == "enabled" {
            e.enabled = v.as_bool().unwrap_or(true);
            continue;
        }
        let id = param_id(k);
        let prm = param_entry(e, id).ok_or_else(|| bad("graphics.set", format!("no property `{k}`")))?;
        let pv = to_param(&prm.value, id, v).ok_or_else(|| bad("graphics.set", format!("`{k}`: value has the wrong type")))?;
        let old = prm.value_at(mt);
        prm.set_at(mt, pv.clone());
        if let (ParamValue::Text(a), ParamValue::Text(b), "text") = (&old, &pv, id)
            && let Some(x) = e.layer.as_mut()
        {
            x.runs = filmcraft_project::graphic_design::adjust_runs(a, b, &x.runs);
        }
    }
    Ok(())
}

/// Set properties `props` on a layer (keyframe-aware at time `tl`). Changing the text keeps
/// per-character styles on their characters. `merge` folds consecutive calls with the same key
/// into one undo step (a drag is one undoable change, like `effects.setParam`).
pub(crate) fn set_props(
    s: &mut Session,
    clip: ClipId,
    eidx: usize,
    props: &serde_json::Map<String, Value>,
    tl: Tick,
    label: &str,
    merge: Option<&str>,
) -> Result<()> {
    let props = props.clone();
    s.edit_sequence_as(label, merge, |q, _, _| {
        let (_, it) = q.find_item_mut(clip).ok_or(filmcraft_edit::EditError::NoItem(clip))?;
        let mt = it.source_time_at(tl.clamp(it.start, it.end() - Tick(1)));
        let e = it.effects.get_mut(eidx).ok_or_else(|| bad("graphics.set", "no such layer"))?;
        apply_props(e, &props, mt)?;
        Ok(())
    })
}

/// Turn a text layer into point text (no box; the lines the box wrapped become real lines) or
/// paragraph text (a box fitted to the text). The text stays where it is: the layer's origin
/// moves between the first baseline and the box's top-left corner, and the anchor point is
/// renumbered to stay on the same spot.
fn set_text_type(s: &mut Session, p: &Value) -> Result<Value> {
    const ID: &str = "graphics.setTextType";
    let clip = target_clip(s, p)?.ok_or_else(|| bad(ID, "no graphic clip"))?;
    let (layer, ei) = layer_effect_index(s, clip, p)?;
    let to_paragraph = match str_p(p, "type").map(str::to_ascii_lowercase).as_deref() {
        Some("paragraph" | "paragraph text") => true,
        Some("point" | "point text") => false,
        _ => return Err(bad(ID, "need `type`: point or paragraph")),
    };
    let seq = s.active_sequence().ok_or(EngineError::NoSequence)?;
    let (_, it) = seq.find_item(clip).ok_or_else(|| bad(ID, "no such clip"))?;
    let size = match s.project.item(it.item).map(|p| &p.kind) {
        Some(ItemKind::Graphic { width, height, .. }) => (*width, *height),
        _ => (seq.settings.width, seq.settings.height),
    };
    let ph = s.playhead();
    let mt = it.source_time_at(ph.clamp(it.start, it.end() - Tick(1)));
    let spec = it.effects.get(ei).and_then(|e| eval_layer(e, mt, size)).ok_or_else(|| bad(ID, "no such layer"))?;
    let LayerContent::Text(tp) = &spec.content else { return Err(bad(ID, "not a text layer")) };
    if tp.vertical {
        return Err(bad(ID, "vertical text is always point text"));
    }
    let name = |paragraph: bool| if paragraph { "paragraph" } else { "point" };
    if (tp.box_width > 0.0) == to_paragraph {
        return Ok(json!({"clip": clip.0, "layer": layer, "type": name(to_paragraph), "changed": false}));
    }
    // where point text's origin (its alignment point on the first baseline) sits in a box `w` wide
    let origin_x = |w: f32| match tp.align {
        1 => w / 2.0,
        2 => w,
        _ => 0.0,
    };
    let mut props = serde_json::Map::new();
    // what to add to the anchor so that it stays on the same spot of the picture
    let shift = if to_paragraph {
        let widest = text_layout(tp).lines.iter().map(|l| l.width).fold(0.0, f32::max);
        // a little slack, so that no line wraps
        let w = (widest.ceil() + 1.0).clamp(1.0, 100_000.0);
        let boxed = text_layout(&graphic::TextProps { box_width: w, box_height: 0.0, ..tp.clone() });
        let h = (boxed.bounds[3].ceil() + 1.0).clamp(1.0, 100_000.0);
        props.insert("box_width".into(), json!(w));
        props.insert("box_height".into(), json!(h));
        (origin_x(w), boxed.lines.first().map_or(0.0, |l| l.baseline))
    } else {
        // every line, also those the box hides
        let all = text_layout(&graphic::TextProps { box_height: 0.0, ..tp.clone() });
        let mut text = String::with_capacity(tp.text.len());
        for (i, l) in all.lines.iter().enumerate() {
            let line = tp.text.get(l.range.clone()).unwrap_or_default();
            // a wrapped line ends where the next begins: its trailing space becomes the line break
            let wrapped = all.lines.get(i + 1).is_some_and(|n| n.range.start == l.range.end);
            text.push_str(if wrapped { line.trim_end() } else { line });
            if i + 1 < all.lines.len() {
                text.push('\n');
            }
        }
        props.insert("text".into(), json!(text));
        props.insert("box_width".into(), json!(0.0));
        props.insert("box_height".into(), json!(0.0));
        (-origin_x(tp.box_width), -all.lines.first().map_or(0.0, |l| l.baseline))
    };
    s.edit_sequence("Text Layer Type", |q, _, _| {
        let (_, it) = q.find_item_mut(clip).ok_or(filmcraft_edit::EditError::NoItem(clip))?;
        let e = it.effects.get_mut(ei).ok_or_else(|| bad(ID, "no such layer"))?;
        apply_props(e, &props, mt)?;
        if let Some(a) = param_entry(e, "anchor") {
            a.map_values(|v| match v.as_vec2() {
                Some(a) => ParamValue::Vec2(Vec2::new(a.x + shift.0 as f64, a.y + shift.1 as f64)),
                None => v,
            });
        }
        Ok(())
    })?;
    Ok(json!({"clip": clip.0, "layer": layer, "type": name(to_paragraph), "changed": true}))
}

/// Layers of a graphic clip with their evaluated bounds (sequence/canvas pixels).
fn list_layers(s: &Session, clip: ClipId) -> Result<Value> {
    list_layers_at(s, clip, s.playhead())
}

/// Shared human/agent layout evaluation. Explicit time never seeks the editor.
pub(crate) fn list_layers_at(s: &Session, clip: ClipId, at: Tick) -> Result<Value> {
    let seq = s.active_sequence().ok_or(EngineError::NoSequence)?;
    let (_, it) = seq.find_item(clip).ok_or_else(|| bad("graphics.list", "no such clip"))?;
    let size = match s.project.item(it.item).map(|p| &p.kind) {
        Some(ItemKind::Graphic { width, height, .. }) => (*width, *height),
        _ => (seq.settings.width, seq.settings.height),
    };
    let mt = layout_source_time(it, at)?;
    let shown = item_layer_specs(it, mt, size);
    let layers: Vec<Value> = layer_indices(&it.effects)
        .iter()
        .enumerate()
        .filter_map(|(i, &ei)| {
            let e = &it.effects[ei];
            let sp = shown.iter().find(|(x, _)| *x == ei).map(|(_, s)| s.clone()).or_else(|| eval_layer(e, mt, size))?;
            let q = layer_quad(&sp);
            let extra = e.layer.as_deref();
            let (kind, text) = match &sp.content {
                LayerContent::Text(t) => ("text", Some(t.text.clone())),
                LayerContent::Shape(sh) => (SHAPE_OPTS.get(sh.shape as usize).copied().unwrap_or("Shape"), None),
            };
            // point text has no box; paragraph text wraps in one and hides what does not fit
            let (text_type, text_box, overflow) = match &sp.content {
                LayerContent::Text(t) if t.box_width > 0.0 && !t.vertical => {
                    (Some("paragraph"), Some([t.box_width, t.box_height]), Some(text_layout(t).overflow))
                }
                LayerContent::Text(_) => (Some("point"), None, None),
                LayerContent::Shape(_) => (None, None, None),
            };
            Some(json!({
                "layer": i,
                "effectIndex": ei,
                "name": layer_display_name(e, i),
                "kind": kind,
                "text": text,
                "enabled": e.enabled,
                "textType": text_type,
                "box": text_box,
                "overflow": overflow,
                "position": [sp.transform.position.x, sp.transform.position.y],
                "anchor": [sp.transform.anchor.x, sp.transform.anchor.y],
                "scale": sp.transform.scale.y * 100.0,
                "scaleWidth": sp.transform.scale.x * 100.0,
                "uid": extra.map_or(0, |x| x.uid),
                "pin": extra.and_then(|x| x.pin.as_ref()).map(|p| serde_json::to_value(p).unwrap_or_default()),
                "styles": extra.map(|x| serde_json::to_value(&x.runs).unwrap_or_default()).unwrap_or(json!([])),
                "localBounds": layer_local_bounds(&sp),
                "quad": q.iter().map(|p| [p.x, p.y]).collect::<Vec<_>>(),
            }))
        })
        .collect();
    let meta = it.graphic.as_deref().map(|m| serde_json::to_value(m).unwrap_or_default());
    let source = s.project.source_graphics.contains_key(&it.item).then_some(it.item.0);
    Ok(
        json!({"clip": clip.0, "name": it.name, "start": it.start.0, "duration": it.duration.0, "canvas": [size.0, size.1], "layers": layers, "graphic": meta, "sourceGraphic": source}),
    )
}

fn quad_bounds(q: &[Vec2; 4]) -> [f64; 4] {
    let mut b = [f64::MAX, f64::MAX, f64::MIN, f64::MIN];
    for p in q {
        b = [b[0].min(p.x), b[1].min(p.y), b[2].max(p.x), b[3].max(p.y)];
    }
    b
}

/// Layer bounds at explicit native sequence time: (effect index, own position, shown bounds).
fn layer_boxes_at(s: &Session, clip: ClipId, layers: &[usize], at: Tick) -> Result<(Vec<(usize, Vec2, [f64; 4])>, (u32, u32))> {
    let seq = s.active_sequence().ok_or(EngineError::NoSequence)?;
    let (_, it) = seq.find_item(clip).ok_or_else(|| bad("graphics.align", "no such clip"))?;
    let size = match s.project.item(it.item).map(|p| &p.kind) {
        Some(ItemKind::Graphic { width, height, .. }) => (*width, *height),
        _ => (seq.settings.width, seq.settings.height),
    };
    layout_target(s, clip)?;
    let mt = layout_source_time(it, at)?;
    let idx = layer_indices(&it.effects);
    let shown = item_layer_specs(it, mt, size);
    let mut out = Vec::new();
    for &l in layers {
        let ei = *idx.get(l).ok_or_else(|| bad("graphics.align", format!("no layer {l}")))?;
        let sp = shown.iter().find(|(x, _)| *x == ei).map(|(_, s)| s.clone()).ok_or_else(|| bad("graphics.align", "bad layer"))?;
        // the position stored in the layer (what moving it changes)
        let own = eval_layer(&it.effects[ei], graphic_time(it, mt).0, size).map_or(sp.transform.position, |o| o.transform.position);
        // Desired translation of shown bounds; the prepared plan compensates for target pins.
        let quad = layer_quad(&sp);
        if quad.iter().any(|p| !finite_point(*p)) {
            return Err(bad("graphics.layout", "layer geometry is not finite"));
        }
        let bounds = quad_bounds(&quad);
        if !finite_point(own) || !bounds.iter().all(|v| v.is_finite()) {
            return Err(bad("graphics.layout", "layer geometry is not finite"));
        }
        out.push((ei, own, bounds));
    }
    Ok((out, size))
}

fn layers_p(s: &Session, clip: ClipId, p: &Value) -> Result<Vec<usize>> {
    let layers = if let Some(value) = p.get("layers") {
        let a = value.as_array().ok_or_else(|| bad("graphics.layout", "layers must be an array of nonnegative integer indices"))?;
        if a.is_empty() || a.len() > 4096 {
            return Err(bad("graphics.layout", "select 1 to 4096 explicit layers"));
        }
        a.iter()
            .map(|v| v.as_u64().and_then(|n| usize::try_from(n).ok()).ok_or_else(|| bad("graphics.layout", "every layer must be a nonnegative integer index")))
            .collect::<Result<Vec<_>>>()?
    } else if !s.state.graphic_layers.is_empty() {
        s.state.graphic_layers.clone()
    } else {
        vec![layer_effect_index(s, clip, p)?.0]
    };
    if layers.is_empty() || layers.len() > 4096 || layers.iter().collect::<BTreeSet<_>>().len() != layers.len() {
        return Err(bad("graphics.layout", "layer targets must be nonempty, bounded and unique"));
    }
    Ok(layers)
}

fn finite_point(p: Vec2) -> bool {
    p.x.is_finite() && p.y.is_finite()
}

/// Validate the native clock's arithmetic, then reuse its authoritative mapping.
pub(crate) fn layout_source_time(it: &filmcraft_project::TrackItem, at: Tick) -> Result<Tick> {
    let fail = || bad("graphics.layout", "clip duration, endpoint or source clock is invalid");
    let last = it.start.0.checked_add(it.duration.0).and_then(|n| n.checked_sub(1)).filter(|n| *n >= it.start.0).ok_or_else(fail)?;
    if !it.speed.is_finite() {
        return Err(fail());
    }
    let at = at.clamp(it.start, Tick(last));
    if let Some(hold) = it.frame_hold {
        return Ok(hold);
    }
    let rel = at.0.checked_sub(it.start.0).ok_or_else(fail)?;
    let scaled = (rel as f64 * it.speed.abs()).round();
    if !scaled.is_finite() || !(0.0..9_223_372_036_854_775_808.0).contains(&scaled) {
        return Err(fail());
    }
    let scaled = scaled as i64;
    let expected = if it.reverse {
        let span = (it.duration.0 as f64 * it.speed.abs()).round();
        if !span.is_finite() || !(0.0..9_223_372_036_854_775_808.0).contains(&span) {
            return Err(fail());
        }
        it.source_in.0.checked_add(span as i64).and_then(|n| n.checked_sub(scaled)).and_then(|n| n.checked_sub(1))
    } else {
        it.source_in.0.checked_add(scaled)
    }
    .ok_or_else(fail)?;
    let native = it.source_time_at(at);
    if native.0 != expected {
        return Err(fail());
    }
    Ok(native)
}

pub(crate) fn layout_time_p(s: &Session, p: &Value) -> Result<Tick> {
    match p.get("time") {
        None => Ok(s.playhead()),
        Some(value) => value.as_i64().filter(|n| *n >= 0).map(Tick).ok_or_else(|| bad("graphics.layout", "time must be nonnegative integer native ticks")),
    }
}

pub(crate) fn layout_target(s: &Session, clip: ClipId) -> Result<&filmcraft_project::TrackItem> {
    let q = s.active_sequence().ok_or(EngineError::NoSequence)?;
    q.settings.validate().map_err(|e| bad("graphics.layout", e))?;
    let (track, c) = q.find_item(clip).ok_or_else(|| bad("graphics.layout", "explicit clip is missing"))?;
    let tr = q.track(track).ok_or_else(|| bad("graphics.layout", "clip track is missing"))?;
    if tr.kind != TrackKind::Video || tr.locked || c.effects.len() > 4096 {
        return Err(bad("graphics.layout", "layout requires an unlocked video target with a bounded effect stack"));
    }
    Ok(c)
}

/// Native preparation shared by human commands and guarded assistant staging.
/// The caller prepares the complete expected native project for layout or pin design.
/// Shared-source synchronization still belongs to normal graphics command dispatch;
/// guarded adapters must retain the explicit source-graphic admission boundary.
pub(crate) struct LayoutPlan {
    before: Arc<Project>,
    sequence: ItemId,
    pub(crate) expected: Project,
    result: Value,
    label: &'static str,
}
impl LayoutPlan {
    pub(crate) fn prepared(s: &Session, expected: Project, label: &'static str, result: Value) -> Result<Self> {
        let sequence = s.state.active_sequence.ok_or(EngineError::NoSequence)?;
        expected.sequence(sequence).ok_or(EngineError::NoSequence)?.check().map_err(EngineError::Other)?;
        Ok(Self { before: s.project.clone(), sequence, expected, result, label })
    }
    pub(crate) fn commit(self, s: &mut Session) -> Result<Value> {
        if !Arc::ptr_eq(&self.before, &s.project) || s.state.active_sequence != Some(self.sequence) {
            return Err(bad("graphics.layout", "layout basis changed before publication"));
        }
        let expected = self.expected;
        let result = self.result;
        s.edit(self.label, move |p, _| {
            *p = expected;
            Ok(())
        })?;
        Ok(result)
    }
}

type LayerMove = (ClipId, usize, Tick, Vec2, Vec2);
struct PinnedLayout {
    clip: ClipId,
    time: Tick,
    canvas: (u32, u32),
    offsets: BTreeMap<usize, [f64; 4]>,
    requested: BTreeMap<usize, [f64; 4]>,
}

/// Solve target movement against the complete native pin graph, including unselected
/// intermediates. Selected layers are fixed at their requested shown bounds; unselected
/// layers retain their native pins. Each selected pin changes by own-edge delta minus
/// target-edge delta, so a moved target does not move its selected follower twice.
fn prepare_layout_pins(s: &Session, moves: &[LayerMove]) -> Result<Vec<PinnedLayout>> {
    let mut groups: BTreeMap<ClipId, (Tick, BTreeMap<usize, Vec2>)> = BTreeMap::new();
    for &(clip, ei, time, old, new) in moves {
        let c = layout_target(s, clip)?;
        if !c.effects.iter().any(|e| e.layer.as_ref().and_then(|l| l.pin.as_ref()).is_some()) {
            continue;
        }
        let (at, deltas) = groups.entry(clip).or_insert_with(|| (time, BTreeMap::new()));
        if *at != time || deltas.insert(ei, Vec2::new(new.x - old.x, new.y - old.y)).is_some() {
            return Err(bad("graphics.layout", "pin targets require one evaluation time and unique layers"));
        }
    }
    let mut prepared = Vec::new();
    for (clip, (time, deltas)) in groups {
        let c = layout_target(s, clip)?;
        let canvas = match s.project.item(c.item).map(|i| &i.kind) {
            Some(ItemKind::Graphic { width, height, .. }) => (*width, *height),
            _ => {
                let q = s.active_sequence().ok_or(EngineError::NoSequence)?;
                (q.settings.width, q.settings.height)
            }
        };
        let indices = layer_indices(&c.effects);
        let specs = graphic_specs(&c.effects, time, canvas);
        if specs.len() != indices.len() {
            return Err(bad("graphics.layout", "pin graph contains an invalid graphic layer"));
        }
        let mut by_uid = BTreeMap::new();
        let mut nodes = Vec::new();
        let mut before = Vec::new();
        let mut requested = BTreeMap::new();
        for ((&ei, spec), i) in indices.iter().zip(&specs).zip(0..) {
            if spec.uid != 0 && by_uid.insert(spec.uid, i).is_some() {
                return Err(bad("graphics.layout", "pin graph has ambiguous layer identity"));
            }
            let bounds = layer_bounds(spec);
            if !bounds.iter().all(|v| v.is_finite()) {
                return Err(bad("graphics.layout", "pin graph geometry is not finite"));
            }
            let mut node = PinNode { uid: spec.uid, pin: spec.pin.clone(), bounds, resizable: matches!(spec.content, LayerContent::Shape(_)) };
            if let Some(delta) = deltas.get(&ei) {
                for (edge, value) in node.bounds.iter_mut().enumerate() {
                    *value += if edge % 2 == 0 { delta.x } else { delta.y };
                }
                if !node.bounds.iter().all(|v| v.is_finite()) {
                    return Err(bad("graphics.layout", "pin movement is not finite"));
                }
                node.pin = None;
                requested.insert(ei, node.bounds);
            }
            before.push(bounds);
            nodes.push(node);
        }
        let after = resolve_pins(&nodes, [0.0, 0.0, canvas.0 as f64, canvas.1 as f64]);
        let mut offsets = BTreeMap::new();
        for (&ei, spec) in indices.iter().zip(&specs) {
            let (Some(delta), Some(pin)) = (deltas.get(&ei), spec.pin.as_ref()) else { continue };
            let target = match pin.to {
                PinTarget::Layer(uid) if pin.any() && uid != spec.uid => by_uid.get(&uid).copied(),
                _ => None,
            };
            let mut next = pin.offsets;
            for (edge, value) in next.iter_mut().enumerate() {
                let inherited = target.map_or(0.0, |i| after[i][edge] - before[i][edge]);
                *value += if edge % 2 == 0 { delta.x } else { delta.y };
                *value -= inherited;
            }
            if !next.iter().all(|v| v.is_finite()) {
                return Err(bad("graphics.layout", "layout pin offsets are not finite"));
            }
            offsets.insert(ei, next);
        }
        prepared.push(PinnedLayout { clip, time, canvas, offsets, requested });
    }
    Ok(prepared)
}

fn layout_plan(s: &Session, moves: Vec<LayerMove>, label: &'static str, result: Value) -> Result<LayoutPlan> {
    if moves.is_empty() || moves.len() > 4096 {
        return Err(bad("graphics.layout", "layout targets must be nonempty and bounded"));
    }
    let sid = s.state.active_sequence.ok_or(EngineError::NoSequence)?;
    let pins = prepare_layout_pins(s, &moves)?;
    let mut expected = (*s.project).clone();
    let mut seen = BTreeSet::new();
    for (clip, ei, at, old, pos) in moves {
        let c = layout_target(s, clip)?;
        if !seen.insert((clip, ei)) || !finite_point(old) || !finite_point(pos) {
            return Err(bad("graphics.layout", "layout positions must be unique and finite"));
        }
        let effect = c.effects.get(ei).ok_or_else(|| bad("graphics.layout", "layout effect is missing"))?;
        let mut param = effect
            .params
            .get("position")
            .cloned()
            .or_else(|| effect.def().and_then(|d| d.param("position")).map(|d| Param::new(d.default.clone())))
            .ok_or_else(|| bad("graphics.layout", "layout effect has no position parameter"))?;
        if !matches!(param.value, ParamValue::Vec2(_)) || param.keyframes.iter().any(|k| !matches!(k.value, ParamValue::Vec2(_))) {
            return Err(bad("graphics.layout", "layout position parameter has invalid types"));
        }
        param.set_at(at, ParamValue::Vec2(pos));
        let offsets = pins.iter().find(|p| p.clip == clip).and_then(|p| p.offsets.get(&ei));
        let e = expected
            .sequence_mut(sid)
            .and_then(|q| q.find_item_mut(clip))
            .and_then(|(_, c)| c.effects.get_mut(ei))
            .ok_or_else(|| bad("graphics.layout", "expected layout effect is missing"))?;
        e.params.insert("position".into(), param);
        if let Some(offsets) = offsets {
            e.layer.as_mut().and_then(|l| l.pin.as_mut()).ok_or_else(|| bad("graphics.layout", "expected graphic pin is missing"))?.offsets = *offsets;
        }
    }
    for p in pins {
        let c =
            expected.sequence(sid).and_then(|q| q.find_item(p.clip)).map(|(_, c)| c).ok_or_else(|| bad("graphics.layout", "expected pin target is missing"))?;
        let specs = graphic_specs(&c.effects, p.time, p.canvas);
        for (&ei, spec) in layer_indices(&c.effects).iter().zip(&specs) {
            if let Some(wanted) = p.requested.get(&ei) {
                let actual = layer_bounds(spec);
                if actual.iter().zip(wanted).any(|(a, b)| !a.is_finite() || (a - b).abs() > 1e-4 + b.abs() * 1e-12) {
                    return Err(bad("graphics.layout", "pin constraints did not produce the requested complete layout; no edit published"));
                }
            }
        }
    }
    expected.sequence(sid).ok_or(EngineError::NoSequence)?.check().map_err(EngineError::Other)?;
    Ok(LayoutPlan { before: s.project.clone(), sequence: sid, expected, result, label })
}

#[derive(Clone, Copy, PartialEq)]
pub(crate) enum AlignTo {
    /// Each layer to the video frame.
    Frame,
    /// The layers' union to the video frame (they keep their relative positions).
    FrameGroup,
    /// Each layer to the union of the selection.
    Selection,
}

fn align_delta(how: &str, r: [f64; 4], b: [f64; 4]) -> Result<(f64, f64)> {
    Ok(match how {
        "left" => (r[0] - b[0], 0.0),
        "right" => (r[2] - b[2], 0.0),
        "hcenter" | "center" => (r[0] / 2.0 + r[2] / 2.0 - b[0] / 2.0 - b[2] / 2.0, 0.0),
        "top" => (0.0, r[1] - b[1]),
        "bottom" => (0.0, r[3] - b[3]),
        "vcenter" | "middle" => (0.0, r[1] / 2.0 + r[3] / 2.0 - b[1] / 2.0 - b[3] / 2.0),
        o => return Err(bad("graphics.align", format!("unknown alignment `{o}`"))),
    })
}

fn union_box(boxes: &[(usize, Vec2, [f64; 4])]) -> [f64; 4] {
    boxes.iter().fold([f64::MAX, f64::MAX, f64::MIN, f64::MIN], |a, b| [a[0].min(b.2[0]), a[1].min(b.2[1]), a[2].max(b.2[2]), a[3].max(b.2[3])])
}

/// Video clips (not graphics) the align commands move by their Motion ▸ Position (#1081): `clip`,
/// else the selected ones. Empty when a graphic clip is named or selected: its layers align.
fn align_target_clips(s: &Session, p: &Value) -> Result<Vec<ClipId>> {
    if p.get("clip").is_some() && p.get("clips").is_some() {
        return Err(bad("graphics.align", "choose clip or clips, not both"));
    }
    if p.get("clips").is_some() {
        let input = p.get("clips").and_then(Value::as_array).ok_or_else(|| bad("graphics.align", "clips must be a nonempty bounded array"))?;
        if input.is_empty() || input.len() > 4096 {
            return Err(bad("graphics.align", "select 1 to 4096 video clips"));
        }
        let ids = crate::commands::ids_p(p, "clips")?.ok_or_else(|| bad("graphics.align", "explicit clips required"))?;
        if ids.is_empty() || ids.len() > 4096 || ids.iter().collect::<BTreeSet<_>>().len() != ids.len() {
            return Err(bad("graphics.align", "clip targets must be nonempty, bounded and unique"));
        }
        let q = s.active_sequence().ok_or(EngineError::NoSequence)?;
        let clips = ids.into_iter().map(ClipId).collect::<Vec<_>>();
        for c in &clips {
            if !q.find_item(*c).is_some_and(|(t, _)| q.track(t).is_some_and(|t| t.kind == TrackKind::Video)) || is_graphic(s, q, *c) {
                return Err(bad("graphics.align", "every explicit clip must be an ordinary video target"));
            }
        }
        return Ok(clips);
    }
    let explicit = u64_p(p, "clip")?;
    let Some(seq) = s.active_sequence() else { return Ok(Vec::new()) };
    let video = |c: &ClipId| seq.find_item(*c).is_some_and(|(t, _)| seq.track(t).is_some_and(|t| t.kind == TrackKind::Video)) && !is_graphic(s, seq, *c);
    Ok(match explicit.map(ClipId) {
        Some(c) => [c].into_iter().filter(video).collect(),
        None if s.state.selection.iter().any(|c| is_graphic(s, seq, *c)) => Vec::new(),
        None => s.state.selection.iter().copied().filter(video).collect(),
    })
}

fn has_graphic_or_clip(s: &Session) -> std::result::Result<(), String> {
    has_seq(s)?;
    if align_target_clips(s, &Value::Null).map_err(|e| e.to_string())?.is_empty() {
        has_graphic(s).map_err(|_| "select a graphic or video clip".into())
    } else {
        Ok(())
    }
}

/// Align video clips to the frame (each, as a group, or to their union) by moving Motion ▸
/// Position only, from the clip's scaled and rotated bounds at the playhead. Position gets a
/// keyframe there only when it is already animated, as typing a value would.
fn prepare_clip_alignment(s: &Session, clips: &[ClipId], how: &str, mode: AlignTo, at: Tick) -> Result<LayoutPlan> {
    if clips.is_empty() || clips.len() > 4096 {
        return Err(bad("graphics.align", "select 1 to 4096 video clips"));
    }
    let seq = s.active_sequence().ok_or(EngineError::NoSequence)?;
    let frame = [0.0, 0.0, seq.settings.width as f64, seq.settings.height as f64];
    let mut boxes = Vec::new();
    for (i, &c) in clips.iter().enumerate() {
        let it = layout_target(s, c)?;
        let src = s.project.source_size(it.item).filter(|(w, h)| *w > 0 && *h > 0).ok_or_else(|| bad("graphics.align", "video source size is unavailable"))?;
        let motions = it.effects.iter().enumerate().filter(|(_, e)| e.effect == "motion").collect::<Vec<_>>();
        let (_, motion) =
            motions.first().filter(|_| motions.len() == 1).ok_or_else(|| bad("graphics.align", "one unambiguous native Motion effect is required"))?;
        let mt = layout_source_time(it, at)?;
        let m = filmcraft_render::motion_matrix(seq, it, src, s.project.source_par(it.item), mt);
        let (w, h) = (src.0 as f64, src.1 as f64);
        let corners = [(0.0, 0.0), (w, 0.0), (w, h), (0.0, h)].map(|(x, y)| m.apply(Vec2::new(x, y)));
        if corners.iter().any(|p| !finite_point(*p)) {
            return Err(bad("graphics.align", "video geometry is not finite"));
        }
        let b = corners.iter().fold([f64::MAX, f64::MAX, f64::MIN, f64::MIN], |b, p| [b[0].min(p.x), b[1].min(p.y), b[2].max(p.x), b[3].max(p.y)]);
        let pos = motion.vec2_at("position", mt);
        let pos = if pos.x.is_nan() || pos.y.is_nan() { Vec2::new(frame[2] / 2.0, frame[3] / 2.0) } else { pos };
        if !finite_point(pos) || !b.iter().all(|v| v.is_finite()) {
            return Err(bad("graphics.align", "video geometry is not finite"));
        }
        boxes.push((i, pos, b));
    }
    if boxes.is_empty() {
        return Err(bad("graphics.align", "the selected clips have no picture to align"));
    }
    let union = union_box(&boxes);
    let group = align_delta(how, frame, union)?;
    let mut moves = Vec::new();
    for (i, pos, b) in &boxes {
        let (dx, dy) = match mode {
            AlignTo::Frame => align_delta(how, frame, *b)?,
            AlignTo::FrameGroup => group,
            AlignTo::Selection => align_delta(how, union, *b)?,
        };
        let clip = *clips.get(*i).ok_or_else(|| bad("graphics.align", "video target disappeared"))?;
        let item = layout_target(s, clip)?;
        let effect = item.effects.iter().position(|e| e.effect == "motion").ok_or_else(|| bad("graphics.align", "Motion effect disappeared"))?;
        moves.push((clip, effect, layout_source_time(item, at)?, *pos, Vec2::new(pos.x + dx, pos.y + dy)));
    }
    layout_plan(s, moves, "Align Clips", json!({"clips":clips.iter().map(|c|c.0).collect::<Vec<_>>()}))
}

/// Graphics ▸ Align to Video Frame / Align to Video Frame as Group / Align to Selection. With video
/// clips selected instead of a graphic, the clips align (#1081).
fn align_layers(s: &mut Session, p: &Value, how: &str, mode: AlignTo) -> Result<Value> {
    prepare_alignment(s, p, how, mode, layout_time_p(s, p)?)?.commit(s)
}

pub(crate) fn prepare_alignment(s: &Session, p: &Value, how: &str, mode: AlignTo, at: Tick) -> Result<LayoutPlan> {
    let clips = align_target_clips(s, p)?;
    if !clips.is_empty() {
        return prepare_clip_alignment(s, &clips, how, mode, at);
    }
    let clip = target_clip(s, p)?.ok_or_else(|| bad("graphics.align", "no graphic clip"))?;
    let layers = layers_p(s, clip, p)?;
    let (boxes, size) = layer_boxes_at(s, clip, &layers, at)?;
    let frame = [0.0, 0.0, size.0 as f64, size.1 as f64];
    let mode = if mode == AlignTo::Selection && boxes.len() < 2 { AlignTo::Frame } else { mode };
    let union = union_box(&boxes);
    let group = align_delta(how, frame, union)?;
    let mut moves = Vec::new();
    for (ei, pos, b) in &boxes {
        let (dx, dy) = match mode {
            AlignTo::Frame => align_delta(how, frame, *b)?,
            AlignTo::FrameGroup => group,
            AlignTo::Selection => align_delta(how, union, *b)?,
        };
        let c = layout_target(s, clip)?;
        moves.push((clip, *ei, graphic_time(c, layout_source_time(c, at)?).0, *pos, Vec2::new(pos.x + dx, pos.y + dy)));
    }
    layout_plan(s, moves, "Align Layers", json!({"clip":clip.0,"layers":layers}))
}

/// Graphics ▸ Distribute: equal centre spacing, or (`space`) equal gaps between the layers.
fn distribute_layers(s: &mut Session, p: &Value, vertical: bool, space: bool) -> Result<Value> {
    prepare_distribution(s, p, vertical, space, layout_time_p(s, p)?)?.commit(s)
}

pub(crate) fn prepare_distribution(s: &Session, p: &Value, vertical: bool, space: bool, at: Tick) -> Result<LayoutPlan> {
    if p.get("clips").is_some() {
        return Err(bad("graphics.distribute", "distribution targets the layers of one explicit graphic"));
    }
    let clip = target_clip(s, p)?.ok_or_else(|| bad("graphics.distribute", "no graphic clip"))?;
    let layers = layers_p(s, clip, p)?;
    let (mut boxes, _) = layer_boxes_at(s, clip, &layers, at)?;
    if boxes.len() < 3 {
        return Err(bad("graphics.distribute", "select three or more layers"));
    }
    let (lo, hi) = if vertical { (1, 3) } else { (0, 2) };
    let c = |b: &[f64; 4]| (b[lo] + b[hi]) / 2.0;
    boxes.sort_by(|a, b| c(&a.2).total_cmp(&c(&b.2)));
    let n = boxes.len() - 1;
    let deltas: Vec<f64> = if space {
        let span = boxes[n].2[hi] - boxes[0].2[lo];
        let total: f64 = boxes.iter().map(|b| b.2[hi] - b.2[lo]).sum();
        let gap = (span - total) / n as f64;
        let mut at = boxes[0].2[lo];
        boxes
            .iter()
            .map(|b| {
                let d = at - b.2[lo];
                at += b.2[hi] - b.2[lo] + gap;
                d
            })
            .collect()
    } else {
        let (first, last) = (c(&boxes[0].2), c(&boxes[n].2));
        boxes.iter().enumerate().map(|(i, b)| first + (last - first) * i as f64 / n as f64 - c(&b.2)).collect()
    };
    let c = layout_target(s, clip)?;
    let source_time = graphic_time(c, layout_source_time(c, at)?).0;
    let moves = boxes
        .iter()
        .zip(deltas)
        .map(|((ei, pos, _), d)| (clip, *ei, source_time, *pos, if vertical { Vec2::new(pos.x, pos.y + d) } else { Vec2::new(pos.x + d, pos.y) }))
        .collect();
    layout_plan(s, moves, "Distribute Layers", json!({"clip":clip.0,"layers":layers}))
}

/// Move a graphic layer in the paint order (`to`: front / back / forward / backward / index).
/// Shared layer reorder primitive; all callers keep non-layer instances intact.
pub(crate) fn reorder_layers(effects: &mut Vec<EffectInstance>, from: usize, to: usize) -> Result<()> {
    let indices = layer_indices(effects);
    let first = *indices.first().ok_or_else(|| bad("graphics.arrangeLayer", "no graphic layers"))?;
    let source = *indices.get(from).ok_or_else(|| bad("graphics.arrangeLayer", "source layer is absent"))?;
    if to >= indices.len() {
        return Err(bad("graphics.arrangeLayer", "destination layer is absent"));
    }
    let effect = effects.remove(source);
    let mut remaining = layer_indices(effects);
    let at = match remaining.get(to) {
        Some(i) => *i,
        None => remaining.pop().map_or(first, |i| i + 1),
    };
    effects.insert(at, effect);
    Ok(())
}

fn arrange_layer(s: &mut Session, p: &Value, to: Value) -> Result<Value> {
    let clip = target_clip(s, p)?.ok_or_else(|| bad("graphics.arrangeLayer", "no graphic clip"))?;
    let (l, _) = layer_effect_index(s, clip, p)?;
    let dest = s.edit_sequence("Arrange Graphic Layer", |q, _, st| {
        let (_, it) = q.find_item_mut(clip).ok_or(filmcraft_edit::EditError::NoItem(clip))?;
        let idx = layer_indices(&it.effects);
        let last = idx.len() - 1;
        let dest = match &to {
            Value::Number(v) => (v.as_u64().unwrap_or(0) as usize).min(last),
            Value::String(w) => match w.as_str() {
                "front" => last,
                "back" => 0,
                "forward" => (l + 1).min(last),
                "backward" => l.saturating_sub(1),
                o => return Err(bad("graphics.arrangeLayer", format!("unknown `to` `{o}`"))),
            },
            _ => last,
        };
        reorder_layers(&mut it.effects, l, dest)?;
        st.selection = vec![clip];
        st.graphic_layers = vec![dest];
        Ok(dest)
    })?;
    Ok(json!({"clip": clip.0, "layer": dest}))
}

/// Graphic clips of the active sequence in timeline order (start, then track bottom → top).
fn graphic_clips(s: &Session) -> Vec<(Tick, usize, ClipId)> {
    let Some(q) = s.active_sequence() else { return Vec::new() };
    let mut v: Vec<(Tick, usize, ClipId)> = q
        .video_tracks
        .iter()
        .enumerate()
        .flat_map(|(ti, tr)| tr.items.iter().map(move |it| (it.start, ti, it.id)))
        .filter(|(_, _, c)| is_graphic(s, q, *c))
        .collect();
    v.sort();
    v
}

/// Graphics ▸ Select ▸ Select Next / Previous Graphic.
fn select_graphic(s: &mut Session, forward: bool) -> Result<Value> {
    let all = graphic_clips(s);
    let cur = s.state.selection.iter().find_map(|c| all.iter().position(|g| g.2 == *c));
    let ph = s.playhead();
    let pick = match (cur, forward) {
        (Some(i), true) => all.get(i + 1),
        (Some(i), false) => i.checked_sub(1).and_then(|j| all.get(j)),
        (None, true) => all.iter().find(|g| g.0 >= ph).or(all.first()),
        (None, false) => all.iter().rev().find(|g| g.0 < ph).or(all.last()),
    };
    let (start, _, clip) = *pick.ok_or_else(|| bad("graphics.select", if forward { "no next graphic" } else { "no previous graphic" }))?;
    let dur = s.active_sequence().and_then(|q| q.find_item(clip)).map_or(Tick::ZERO, |(_, it)| it.duration);
    s.state.selection = vec![clip];
    s.state.graphic_layers.clear();
    if !(ph >= start && ph < start + dur) {
        s.set_playhead(start);
    }
    Ok(json!({"clip": clip.0}))
}

/// Graphics ▸ Select ▸ Select Next / Previous Layer (cycles through the clip's layers).
fn select_layer_step(s: &mut Session, forward: bool) -> Result<Value> {
    let clip = target_clip(s, &Value::Null)?.ok_or_else(|| bad("graphics.selectLayer", "no graphic clip"))?;
    let n = s.active_sequence().and_then(|q| q.find_item(clip)).map_or(0, |(_, it)| layer_indices(&it.effects).len());
    if n == 0 {
        return Err(bad("graphics.selectLayer", "the graphic has no layers"));
    }
    let l = match (s.state.graphic_layers.first().copied().filter(|l| *l < n), forward) {
        (Some(l), true) => (l + 1) % n,
        (Some(l), false) => (l + n - 1) % n,
        (None, true) => 0,
        (None, false) => n - 1,
    };
    s.state.selection = vec![clip];
    s.state.graphic_layers = vec![l];
    Ok(json!({"clip": clip.0, "layer": l}))
}

fn has_two_layers(s: &Session) -> std::result::Result<(), String> {
    let clips = align_target_clips(s, &Value::Null).map_err(|e| e.to_string())?;
    if !clips.is_empty() {
        return if clips.len() >= 2 { Ok(()) } else { Err("select two or more video clips".into()) };
    }
    has_graphic(s)?;
    if s.state.graphic_layers.len() < 2 { Err("select two or more graphic layers".into()) } else { Ok(()) }
}

fn has_three_layers(s: &Session) -> std::result::Result<(), String> {
    has_graphic(s)?;
    if s.state.graphic_layers.len() < 3 { Err("select three or more graphic layers".into()) } else { Ok(()) }
}

fn has_graphics_in_seq(s: &Session) -> std::result::Result<(), String> {
    has_seq(s)?;
    if graphic_clips(s).is_empty() { Err("the sequence has no graphics".into()) } else { Ok(()) }
}

const ALIGN_PARAMS: &str = r#"{"clip":id?,"clips":[id]? (ordinary video only; exclusive with clip),"layers":[n]? (graphic layers; default: selected layers),"time":ticks? (default: playhead, clamped to each clip)}"#;
const DISTRIBUTE_PARAMS: &str = r#"{"clip":id?,"layers":[n]? (three or more graphic layers; default: selected layers),"time":ticks? (default: playhead)}"#;

macro_rules! align_cmd {
    ($id:literal, $label:literal, $menu:expr, $how:literal, $mode:expr, $en:expr) => {
        spec($id, $label, &["Graphics and Titles", $menu], None, ALIGN_PARAMS, $en, |s, p| align_layers(s, p, $how, $mode))
    };
}

macro_rules! arrange_cmd {
    ($id:literal, $label:literal, $sc:literal, $to:literal) => {
        spec($id, $label, &["Graphics and Titles", "Arrange"], Some($sc), r#"{"clip":id?,"layer":n?}"#, has_graphic, |s, p| arrange_layer(s, p, json!($to)))
    };
}

fn new_shape_cmd(s: &mut Session, p: &Value, shape: &str) -> Result<Value> {
    let mut q = p.clone();
    q["shape"] = json!(shape);
    (find_spec("graphics.newShape")?.run)(s, &q)
}

fn find_spec(id: &str) -> Result<CommandSpec> {
    commands().into_iter().find(|c| c.id == id).ok_or_else(|| EngineError::UnknownCommand(id.to_string()))
}

/// Graphics menu commands (New Layer shapes, Align/Distribute/Arrange/Select submenus, resets).
fn menu_commands() -> Vec<CommandSpec> {
    const F: &str = "Align to Video Frame";
    const G: &str = "Align to Video Frame as Group";
    const S: &str = "Align to Selection";
    const D: &[&str] = &["Graphics and Titles", "Distribute"];
    const SEL: &[&str] = &["Graphics and Titles", "Select"];
    vec![
        spec(
            "graphics.newVerticalText",
            "Vertical Text",
            &["Graphics and Titles", "New Layer"],
            None,
            r#"{"text":str="New Text","position":[x,y]?,"clip":id?,"size":px=100,"seconds":f64=5 (or durationFrames/durationTicks),"durationFrames":positive_int?,"durationTicks":frame_aligned_ticks?,"time":ticks?}"#,
            has_seq,
            |s, p| {
                let mut p = if p.is_object() { p.clone() } else { json!({}) };
                p["vertical"] = json!(true);
                (find_spec("graphics.newText")?.run)(s, &p)
            },
        ),
        spec(
            "graphics.newRectangle",
            "Rectangle",
            &["Graphics and Titles", "New Layer"],
            Some("Cmd+Alt+R"),
            r#"{"position":[x,y]?,"size":[w,h]=[400,200],"clip":id?,"seconds":f64=5 (or durationFrames/durationTicks),"durationFrames":positive_int?,"durationTicks":frame_aligned_ticks?,"track":index?,"time":ticks?}"#,
            has_seq,
            |s, p| new_shape_cmd(s, p, "rectangle"),
        ),
        spec(
            "graphics.newEllipse",
            "Ellipse",
            &["Graphics and Titles", "New Layer"],
            Some("Cmd+Alt+E"),
            r#"{"position":[x,y]?,"size":[w,h]=[400,200],"clip":id?,"seconds":f64=5 (or durationFrames/durationTicks),"durationFrames":positive_int?,"durationTicks":frame_aligned_ticks?,"track":index?,"time":ticks?}"#,
            has_seq,
            |s, p| new_shape_cmd(s, p, "ellipse"),
        ),
        spec(
            "graphics.newPolygon",
            "Polygon",
            &["Graphics and Titles", "New Layer"],
            None,
            r#"{"position":[x,y]?,"size":[w,h]=[300,300],"sides":n=6,"clip":id?,"seconds":f64=5 (or durationFrames/durationTicks),"durationFrames":positive_int?,"durationTicks":frame_aligned_ticks?,"track":index?,"time":ticks?}"#,
            has_seq,
            |s, p| {
                let mut q = if p.is_object() { p.clone() } else { json!({}) };
                if q.get("size").is_none() {
                    q["size"] = json!([300, 300]);
                }
                let r = new_shape_cmd(s, &q, "polygon")?;
                if let Some(n) = f64_p(p, "sides") {
                    // fold the side count into the same undo step
                    let clip = ClipId(r["clip"].as_u64().unwrap_or(0));
                    let (_, ei) = layer_effect_index(s, clip, &json!({"layer": r["layer"]}))?;
                    let mut props = serde_json::Map::new();
                    props.insert("sides".into(), json!(n));
                    let ph = s.playhead();
                    let before = s.history.undo.len();
                    set_props(s, clip, ei, &props, ph, "Change Graphic Property", None)?;
                    if s.history.undo.len() > before {
                        s.history.undo.pop();
                    }
                }
                Ok(r)
            },
        ),
        spec(
            "graphics.newFromFile",
            "From file…",
            &["Graphics and Titles", "New Layer"],
            None,
            r#"{"path":str,"time":ticks?,"track":index?} (imports the image or video and places it above the clips at the playhead)"#,
            has_seq,
            |s, p| {
                let path = str_p(p, "path").ok_or_else(|| bad("graphics.newFromFile", "need `path`"))?.to_string();
                let r = s.execute("file.import", json!({"paths": [path]}))?;
                let item = r["items"].as_array().and_then(|a| a.first()).and_then(Value::as_u64).map(ItemId).ok_or_else(|| {
                    bad(
                        "graphics.newFromFile",
                        r["errors"].as_array().and_then(|e| e.first()).and_then(Value::as_str).unwrap_or("nothing imported").to_string(),
                    )
                })?;
                let pi = s.project.item(item).ok_or_else(|| bad("graphics.newFromFile", "no such item"))?;
                if matches!(&pi.kind, ItemKind::Media(m) if m.info.video.is_none()) {
                    return Err(bad("graphics.newFromFile", "the file has no picture"));
                }
                let (name, dur) = (pi.name.clone(), pi.duration());
                let clip = place_video_clip(s, "graphics.newFromFile", &name, p, "New Layer from File", Vec::new(), move |_, _| (item, dur))?;
                s.state.graphic_layers.clear();
                Ok(json!({"clip": clip.0, "item": item.0}))
            },
        ),
        align_cmd!("graphics.alignFrame.left", "Left", F, "left", AlignTo::Frame, has_graphic_or_clip),
        align_cmd!("graphics.alignFrame.hcenter", "Center Horizontally", F, "hcenter", AlignTo::Frame, has_graphic_or_clip),
        align_cmd!("graphics.alignFrame.right", "Right", F, "right", AlignTo::Frame, has_graphic_or_clip),
        align_cmd!("graphics.alignFrame.top", "Top", F, "top", AlignTo::Frame, has_graphic_or_clip),
        align_cmd!("graphics.alignFrame.vcenter", "Center Vertically", F, "vcenter", AlignTo::Frame, has_graphic_or_clip),
        align_cmd!("graphics.alignFrame.bottom", "Bottom", F, "bottom", AlignTo::Frame, has_graphic_or_clip),
        align_cmd!("graphics.alignGroup.left", "Left", G, "left", AlignTo::FrameGroup, has_two_layers),
        align_cmd!("graphics.alignGroup.hcenter", "Center Horizontally", G, "hcenter", AlignTo::FrameGroup, has_two_layers),
        align_cmd!("graphics.alignGroup.right", "Right", G, "right", AlignTo::FrameGroup, has_two_layers),
        align_cmd!("graphics.alignGroup.top", "Top", G, "top", AlignTo::FrameGroup, has_two_layers),
        align_cmd!("graphics.alignGroup.vcenter", "Center Vertically", G, "vcenter", AlignTo::FrameGroup, has_two_layers),
        align_cmd!("graphics.alignGroup.bottom", "Bottom", G, "bottom", AlignTo::FrameGroup, has_two_layers),
        align_cmd!("graphics.alignSelection.left", "Left", S, "left", AlignTo::Selection, has_two_layers),
        align_cmd!("graphics.alignSelection.hcenter", "Center Horizontally", S, "hcenter", AlignTo::Selection, has_two_layers),
        align_cmd!("graphics.alignSelection.right", "Right", S, "right", AlignTo::Selection, has_two_layers),
        align_cmd!("graphics.alignSelection.top", "Top", S, "top", AlignTo::Selection, has_two_layers),
        align_cmd!("graphics.alignSelection.vcenter", "Center Vertically", S, "vcenter", AlignTo::Selection, has_two_layers),
        align_cmd!("graphics.alignSelection.bottom", "Bottom", S, "bottom", AlignTo::Selection, has_two_layers),
        spec("graphics.distributeVertically", "Distribute Vertically", D, None, DISTRIBUTE_PARAMS, has_three_layers, |s, p| {
            distribute_layers(s, p, true, false)
        }),
        spec("graphics.distributeSpaceVertically", "Distribute Space Vertically", D, None, DISTRIBUTE_PARAMS, has_three_layers, |s, p| {
            distribute_layers(s, p, true, true)
        }),
        spec("graphics.distributeHorizontally", "Distribute Horizontally", D, None, DISTRIBUTE_PARAMS, has_three_layers, |s, p| {
            distribute_layers(s, p, false, false)
        }),
        spec("graphics.distributeSpaceHorizontally", "Distribute Space Horizontally", D, None, DISTRIBUTE_PARAMS, has_three_layers, |s, p| {
            distribute_layers(s, p, false, true)
        }),
        arrange_cmd!("graphics.bringToFront", "Bring to Front", "Cmd+Shift+]", "front"),
        arrange_cmd!("graphics.bringForward", "Bring Forward", "Cmd+]", "forward"),
        arrange_cmd!("graphics.sendBackward", "Send Backward", "Cmd+[", "backward"),
        arrange_cmd!("graphics.sendToBack", "Send to Back", "Cmd+Shift+[", "back"),
        spec("graphics.selectNextGraphic", "Select Next Graphic", SEL, None, "{}", has_graphics_in_seq, |s, _| select_graphic(s, true)),
        spec("graphics.selectPreviousGraphic", "Select Previous Graphic", SEL, None, "{}", has_graphics_in_seq, |s, _| select_graphic(s, false)),
        spec("graphics.selectNextLayer", "Select Next Layer", SEL, Some("Cmd+Alt+]"), "{}", has_graphic, |s, _| select_layer_step(s, true)),
        spec("graphics.selectPreviousLayer", "Select Previous Layer", SEL, Some("Cmd+Alt+["), "{}", has_graphic, |s, _| select_layer_step(s, false)),
        spec(
            "graphics.resetAllParameters",
            "Reset All Parameters",
            &["Graphics and Titles"],
            None,
            r#"{"clip":id?,"layers":[n]? (default: the selected layers, else all)}"#,
            has_graphic,
            |s, p| {
                let clip = target_clip(s, p)?.ok_or_else(|| bad("graphics.resetAllParameters", "no graphic clip"))?;
                let frame = s.active_sequence().map(|q| (q.settings.width, q.settings.height)).unwrap_or((1920, 1080));
                let want: Vec<usize> = match p.get("layers").and_then(Value::as_array) {
                    Some(a) => a.iter().filter_map(Value::as_u64).map(|v| v as usize).collect(),
                    None => s.state.graphic_layers.clone(),
                };
                let n = s.edit_sequence("Reset All Parameters", |q, _, _| {
                    let (_, it) = q.find_item_mut(clip).ok_or(filmcraft_edit::EditError::NoItem(clip))?;
                    let mut n = 0;
                    for (l, ei) in layer_indices(&it.effects).into_iter().enumerate() {
                        if want.is_empty() || want.contains(&l) {
                            graphic::reset_layer_params(&mut it.effects[ei]);
                            filmcraft_project::resolve_auto_points(&mut it.effects[ei], frame, frame);
                            n += 1;
                        }
                    }
                    Ok(n)
                })?;
                Ok(json!({"clip": clip.0, "reset": n}))
            },
        ),
        spec(
            "graphics.resetDuration",
            "Reset Duration",
            &["Graphics and Titles"],
            None,
            r#"{"clip":id?,"seconds":f64=5 (the default graphic duration; limited by the next clip on the track)}"#,
            has_graphic,
            |s, p| {
                let clip = target_clip(s, p)?.ok_or_else(|| bad("graphics.resetDuration", "no graphic clip"))?;
                let seconds = f64_p(p, "seconds").unwrap_or(5.0).max(0.01);
                let d = s.edit_sequence("Reset Duration", |q, _, _| {
                    let rate = q.settings.frame_rate;
                    let want = rate.snap_nearest(Tick::from_seconds_f64(seconds)).max(rate.frame_duration());
                    let tr = q.video_tracks.iter_mut().find(|tr| tr.items.iter().any(|i| i.id == clip)).ok_or(filmcraft_edit::EditError::NoItem(clip))?;
                    let i = tr.items.iter().position(|i| i.id == clip).ok_or(filmcraft_edit::EditError::NoItem(clip))?;
                    let start = tr.items[i].start;
                    let limit = tr.items.get(i + 1).map_or(want, |n| n.start - start);
                    let d = want.min(limit).max(rate.frame_duration());
                    tr.items[i].duration = d;
                    Ok(d)
                })?;
                Ok(json!({"clip": clip.0, "duration": d.0}))
            },
        ),
    ]
}

/// Shared preparation for human creation and the agent's collateral audit.
pub(crate) fn prepare_text_layer(s: &Session, p: &Value) -> Result<(String, EffectInstance)> {
    let text = str_p(p, "text").unwrap_or("New Text").to_string();
    // Settings ▸ Graphics ▸ Text: smart quotes, ligatures, default font
    let gp = s.prefs.graphics.clone();
    let text = if gp.smart_quotes { crate::settings::smart_quotes(&text) } else { text };
    let (w, h) = s.active_sequence().map(|q| (q.settings.width, q.settings.height)).unwrap_or((1920, 1080));
    let pos = vec2_p(p, "position").unwrap_or(Vec2::new(w as f64 / 2.0, h as f64 / 2.0));
    let size = f64_p(p, "size").unwrap_or(100.0);
    let vertical = p.get("vertical").and_then(Value::as_bool) == Some(true);
    let mut layer = if vertical { graphic::new_vertical_text_layer(&text, pos, size) } else { new_text_layer(&text, pos, size) };
    if let Some(b) = vec2_p(p, "box").filter(|_| !vertical) {
        for (id, v) in [("box_width", b.x), ("box_height", b.y)] {
            layer.params.insert(id.into(), Param::new(ParamValue::Float(v.clamp(1.0, 100_000.0))));
        }
    }
    layer.params.insert("ligatures".into(), Param::new(ParamValue::Bool(gp.ligatures)));
    if !gp.default_font.trim().is_empty() {
        layer.params.insert("font".into(), Param::new(ParamValue::Text(gp.default_font.trim().into())));
    }
    for (k, id) in [("font", "font"), ("fontStyle", "font_style")] {
        if let Some(v) = str_p(p, k) {
            layer.params.insert(id.into(), Param::new(ParamValue::Text(v.into())));
        }
    }
    Ok((text, layer))
}

pub(crate) fn prepare_shape_layer(s: &Session, p: &Value) -> Result<EffectInstance> {
    let shape = str_p(p, "shape").unwrap_or("rectangle").to_ascii_lowercase();
    let k = SHAPE_OPTS.iter().position(|o| o.to_ascii_lowercase() == shape).ok_or_else(|| bad("graphics.newShape", format!("unknown shape `{shape}`")))? as u32;
    let (w, h) = s.active_sequence().map(|q| (q.settings.width, q.settings.height)).unwrap_or((1920, 1080));
    let pos = vec2_p(p, "position").unwrap_or(Vec2::new(w as f64 / 2.0, h as f64 / 2.0));
    let size = vec2_p(p, "size").unwrap_or(Vec2::new(400.0, 200.0));
    let points: Vec<[f32; 2]> = p
        .get("points")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(|q| Some([q.get(0)?.as_f64()? as f32, q.get(1)?.as_f64()? as f32])).collect())
        .unwrap_or_default();
    if k == 3 && points.len() < 3 {
        return Err(bad("graphics.newShape", "a path needs at least 3 points"));
    }
    let layer = new_shape_layer(k, pos, size, points);
    Ok(layer)
}

pub fn commands() -> Vec<CommandSpec> {
    let mut v = vec![
        spec(
            "graphics.newText",
            "Text",
            &["Graphics and Titles", "New Layer"],
            Some("Cmd+T"),
            r#"{"text":str="New Text","position":[x,y]? (point text: its alignment point on the first baseline; with `box`: the box's top-left corner),"box":[w,h]? (paragraph text wrapped in a box this size),"clip":id?,"newClip":bool?,"vertical":bool=false,"size":px=100,"font":str?,"fontStyle":str?,"seconds":f64=5 (or durationFrames/durationTicks),"durationFrames":positive_int?,"durationTicks":frame_aligned_ticks?,"track":index?,"time":ticks?}"#,
            has_seq,
            |s, p| {
                let (text, layer) = prepare_text_layer(s, p)?;
                let into = if p.get("newClip").and_then(Value::as_bool) == Some(true) { None } else { new_layer_target(s, p, "graphics.newText")? };
                let (clip, layer_i) = match into {
                    Some(c) => (c, add_layer(s, c, layer)?),
                    None => {
                        let name = text.lines().next().filter(|l| !l.trim().is_empty()).unwrap_or("Graphic").to_string();
                        (new_graphic_clip(s, "graphics.newText", layer, &name, p)?, 0)
                    }
                };
                Ok(json!({"clip": clip.0, "layer": layer_i}))
            },
        ),
        spec(
            "graphics.newShape",
            "Shape",
            &[],
            None,
            r#"{"shape":"rectangle|ellipse|polygon|path","position":[x,y]? (the shape's centre),"size":[w,h]=[400,200],"points":[[x,y],…]?,"clip":id? (add the shape to this graphic),"seconds":f64=5 (or durationFrames/durationTicks),"durationFrames":positive_int?,"durationTicks":frame_aligned_ticks?,"track":index?,"time":ticks?}"#,
            has_seq,
            |s, p| {
                let layer = prepare_shape_layer(s, p)?;
                let into = new_layer_target(s, p, "graphics.newShape")?;
                let (clip, layer_i) = match into {
                    Some(c) => (c, add_layer(s, c, layer)?),
                    None => (new_graphic_clip(s, "graphics.newShape", layer, "Shape", p)?, 0),
                };
                Ok(json!({"clip": clip.0, "layer": layer_i}))
            },
        ),
        spec(
            "graphics.setText",
            "Edit Text",
            &[],
            None,
            r#"{"clip":id?,"layer":n|name?,"text":str,"merge":bool? (coalesce with the previous Edit Text undo step)}"#,
            has_graphic,
            |s, p| {
                let clip = target_clip(s, p)?.ok_or_else(|| bad("graphics.setText", "no graphic clip"))?;
                let (_, ei) = layer_effect_index(s, clip, p)?;
                let text = str_p(p, "text").ok_or_else(|| bad("graphics.setText", "need `text`"))?.to_string();
                let merge = p.get("merge").and_then(Value::as_bool).unwrap_or(false) && s.history.undo.last().is_some_and(|h| h.0 == "Edit Text");
                let mut props = serde_json::Map::new();
                props.insert("text".into(), Value::String(text));
                let ph = s.playhead();
                set_props(s, clip, ei, &props, ph, "Edit Text", None)?;
                if merge && s.history.undo.len() >= 2 {
                    // keep the snapshot from before the typing session
                    s.history.undo.pop();
                }
                Ok(Value::Null)
            },
        ),
        spec(
            "graphics.set",
            "Set Graphic Properties",
            &[],
            None,
            r##"{"clip":id?,"layer":n|name?,"props":{"font":"Inter","font_style":"Bold","size":120,"align":"center","tracking":50,"leading":0,"fill_color":"#ffcc00","stroke":true,"stroke_width":6,"background":true,"shadow":true,"position":[x,y],"scale":100,"rotation":0,"opacity":100,…},"time":ticks?,"merge":bool?,"begin":bool?}"##,
            has_graphic,
            |s, p| {
                let clip = target_clip(s, p)?.ok_or_else(|| bad("graphics.set", "no graphic clip"))?;
                let (_, ei) = layer_effect_index(s, clip, p)?;
                let props = p.get("props").and_then(Value::as_object).ok_or_else(|| bad("graphics.set", "need `props`"))?.clone();
                let tl = time_p(s, p, "").unwrap_or_else(|| s.playhead());
                // `merge`: a drag of a property is one undo step; `begin` starts a new one (#201)
                let merge = bool_p(p, "merge").unwrap_or(false).then(|| format!("graphics.set:{}:{ei}", clip.0));
                if bool_p(p, "begin").unwrap_or(false) {
                    s.history.merge_key = None;
                }
                set_props(s, clip, ei, &props, tl, "Change Graphic Property", merge.as_deref())?;
                Ok(Value::Null)
            },
        ),
        spec(
            "graphics.setTextType",
            "Text Layer Type",
            &[],
            None,
            r#"{"clip":id?,"layer":n|name?,"type":"point|paragraph" (point: no box, handles scale the text; paragraph: the text wraps in a box that handles resize)}"#,
            has_graphic,
            set_text_type,
        ),
        spec("graphics.selectLayer", "Select Graphic Layer", &[], None, r#"{"clip":id?,"layers":[n]}"#, has_graphic, |s, p| {
            let clip = target_clip(s, p)?.ok_or_else(|| bad("graphics.selectLayer", "no graphic clip"))?;
            let layers: Vec<usize> =
                p.get("layers").and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_u64).map(|v| v as usize).collect()).unwrap_or_default();
            s.state.selection = vec![clip];
            s.state.graphic_layers = layers;
            Ok(Value::Null)
        }),
        spec("graphics.deleteLayer", "Delete Graphic Layer", &[], None, r#"{"clip":id?,"layer":n?}"#, has_graphic, |s, p| {
            let clip = target_clip(s, p)?.ok_or_else(|| bad("graphics.deleteLayer", "no graphic clip"))?;
            let (_, ei) = layer_effect_index(s, clip, p)?;
            s.edit_sequence("Delete Graphic Layer", |q, _, st| {
                let (_, it) = q.find_item_mut(clip).ok_or(filmcraft_edit::EditError::NoItem(clip))?;
                it.effects.remove(ei);
                st.graphic_layers.clear();
                Ok(())
            })?;
            Ok(Value::Null)
        }),
        spec(
            "graphics.arrangeLayer",
            "Arrange Graphic Layer",
            &[],
            None,
            r#"{"clip":id?,"layer":n?,"to":"front|back|forward|backward"|index}"#,
            has_graphic,
            |s, p| {
                let to = p.get("to").cloned().unwrap_or(json!("front"));
                arrange_layer(s, p, to)
            },
        ),
        spec(
            "graphics.align",
            "Align Layers",
            &[],
            None,
            r#"{"clip":id?,"clips":[id]? (ordinary video only; exclusive with clip),"layers":[n]?,"align":"left|hcenter|right|top|vcenter|bottom","to":"frame|group|selection"="frame","time":ticks? (default: playhead; one layer aligns to the frame)}"#,
            has_graphic_or_clip,
            |s, p| {
                let how = str_p(p, "align").ok_or_else(|| bad("graphics.align", "need `align`"))?.to_string();
                let mode = match str_p(p, "to") {
                    None if p.get("to").is_none() => AlignTo::Frame,
                    Some("frame") => AlignTo::Frame,
                    Some("selection") => AlignTo::Selection,
                    Some("group") => AlignTo::FrameGroup,
                    _ => return Err(bad("graphics.align", "to must be frame, group or selection")),
                };
                align_layers(s, p, &how, mode)
            },
        ),
        spec(
            "graphics.distribute",
            "Distribute Layers",
            &[],
            None,
            r#"{"clip":id?,"layers":[n] (3 or more)?,"axis":"horizontal|vertical","space":bool=false,"time":ticks? (equal gaps instead of centres; default time: playhead)}"#,
            has_graphic,
            |s, p| {
                let vertical = match p.get("axis") {
                    None => false,
                    Some(Value::String(s)) if s == "horizontal" => false,
                    Some(Value::String(s)) if s == "vertical" => true,
                    _ => return Err(bad("graphics.distribute", "axis must be horizontal or vertical")),
                };
                let space = match p.get("space") {
                    None => false,
                    Some(Value::Bool(v)) => *v,
                    _ => return Err(bad("graphics.distribute", "space must be a boolean")),
                };
                distribute_layers(s, p, vertical, space)
            },
        ),
        query("graphics.list", "List Graphic Layers", r#"{"clip":id?}"#, |s, p| {
            let clip = target_clip(s, p)?.ok_or_else(|| bad("graphics.list", "no graphic clip"))?;
            list_layers(s, clip)
        }),
        query("fonts.list", "List Fonts", r#"{"system":bool=true (scan the system font folders)}"#, |_, p| {
            if p.get("system").and_then(Value::as_bool).unwrap_or(true) {
                filmcraft_text::fonts::scan_system();
            }
            Ok(json!(filmcraft_text::families().into_iter().map(|(f, st)| json!({"family": f, "styles": st})).collect::<Vec<_>>()))
        }),
    ];
    v.extend(menu_commands());
    v
}

#[cfg(test)]
#[path = "graphics_tests.rs"]
mod tests;
