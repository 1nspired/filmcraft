//! Strict request admission; edit math and history stay in the human service.
use super::{error, project_audit};
use crate::{Result, graphics::layout_source_time};
use filmcraft_project::{EffectInstance, TrackItem, graphic, graphic_design::CharStyle};
use filmcraft_time::Tick;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
fn canonical(k: &str) -> Option<&'static str> {
    Some(match k {
        "font" | "family" => "font",
        "fontStyle" | "font_style" | "style" => "font_style",
        "size" | "fontSize" => "size",
        "fill" | "color" | "fillColor" | "fill_color" => "fill_color",
        "bold" | "fauxBold" | "faux_bold" => "faux_bold",
        "italic" | "fauxItalic" | "faux_italic" => "faux_italic",
        "underline" => "underline",
        "tracking" => "tracking",
        "baselineShift" | "baseline_shift" => "baseline_shift",
        "caps" => "caps",
        _ => return None,
    })
}
pub(super) fn validate_style(v: &Value, clear: bool, admit_fonts: bool) -> Result<CharStyle> {
    let o = v.as_object().ok_or_else(|| error("explicit character style object required"))?;
    if (clear && !o.is_empty()) || (!clear && o.is_empty()) || o.len() > 10 {
        return Err(error("choose one explicit style merge or complete override clear"));
    }
    let mut seen = BTreeSet::new();
    for (key, value) in o {
        let id = canonical(key).ok_or_else(|| error("unknown character style property"))?;
        if !seen.insert(id) {
            return Err(error("duplicate aliases for one character property"));
        }
        if ["font", "font_style"].contains(&id) {
            if !admit_fonts {
                return Err(error("font resources require their explicit admission contract"));
            }
            if !value.as_str().is_some_and(|s| super::text(s, 16 * 1024)) {
                return Err(error("font reference must be an exact bounded native string"));
            }
            continue;
        }
        if id == "caps" {
            if !(value.as_u64().is_some_and(|v| v <= 2)
                || value.as_str().is_some_and(|v| graphic::CAPS_OPTS.iter().any(|s| s.eq_ignore_ascii_case(&v.replace('_', " ")))))
            {
                return Err(error("caps must be an exact native choice"));
            }
        } else if id == "fill_color" {
            match value {
                Value::String(s) if s.len() <= 32 && filmcraft_color::parse_hex(s).is_some() => {}
                Value::Array(a)
                    if (a.len() == 3 || a.len() == 4) && a.iter().all(|v| v.as_f64().is_some_and(|v| v.is_finite() && (0.0..=1.0).contains(&v))) => {}
                _ => return Err(error("color requires a native hex or exact bounded RGB/RGBA value")),
            }
        } else if ["faux_bold", "faux_italic", "underline"].contains(&id) {
            if !value.is_boolean() {
                return Err(error("character flag must be a boolean"));
            }
        } else {
            let value = value.as_f64().filter(|v| v.is_finite() && (*v as f32).is_finite()).ok_or_else(|| error("finite native character number required"))?;
            let definition =
                filmcraft_project::find_effect(graphic::TEXT_LAYER).and_then(|d| d.param(id)).ok_or_else(|| error("native character parameter missing"))?;
            if let filmcraft_project::ParamKind::Float { min, max, .. } = &definition.kind
                && !(*min..=*max).contains(&value)
            {
                return Err(error("character value is outside the native human control bounds"));
            }
        }
    }
    crate::graphic_templates::char_style_p(v)
}
pub(super) fn prepare(
    c: &TrackItem,
    e: &EffectInstance,
    at: Tick,
    start: usize,
    end: usize,
    text_sha256: &str,
    runs_sha256: &str,
    range_text_sha256: &str,
    clear: bool,
    style: &Value,
    admit_fonts: bool,
) -> Result<(Tick, CharStyle, bool)> {
    let last = c.start.0.checked_add(c.duration.0).filter(|n| *n > c.start.0).ok_or_else(|| error("graphic clock invalid"))?;
    if at < c.start || at.0 >= last || e.effect != graphic::TEXT_LAYER {
        return Err(error("exact frame inside the observed text clip required"));
    }
    let mt = layout_source_time(c, at)?;
    let text = graphic::text_at(e, mt);
    let count = text.chars().count();
    let runs = e.layer.as_ref().map_or(&[][..], |x| &x.runs[..]);
    if count > 1_000_000 || start >= end || end > count || !filmcraft_project::graphic_design::style_runs_are_disjoint(runs) {
        return Err(error("exact nonempty scalar range and ordered native style evidence required"));
    }
    if format!("{:x}", Sha256::digest(text.as_bytes())) != text_sha256 || format!("{:x}", Sha256::digest(project_audit::value_bytes(&runs)?)) != runs_sha256 {
        return Err(error("native text or saved styles changed at the requested frame"));
    }
    let selected: String = text.chars().skip(start).take(end - start).collect();
    if format!("{:x}", Sha256::digest(selected.as_bytes())) != range_text_sha256 {
        return Err(error("observed scalar text differs from the requested native range"));
    }
    let parsed = validate_style(style, clear, admit_fonts)?;
    let fonts = clear && runs.iter().any(|r| r.start < end && r.end > start && (r.style.font.is_some() || r.style.font_style.is_some()));
    Ok((mt, parsed, fonts))
}
