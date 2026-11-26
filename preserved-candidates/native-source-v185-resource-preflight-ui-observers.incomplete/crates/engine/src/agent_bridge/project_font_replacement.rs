//! Exact authored collateral for the original project-wide human font command.
//! Documentary prediction only: no font loading, renderer proof, write grant or
//! replayable project recipe. The original helper remains the only mutation math.
use super::{parse, project_audit, project_plane};
use crate::{Result, Session, commands::bad, project_workspace::ProjectScope};
use filmcraft_project::{EffectInstance, ItemKind, Project, graphic};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
pub(super) const PLAN: &str = "agent.planProjectFontReplacement";
const MAX_ROWS: usize = 100_000;
fn error(message: impl Into<String>) -> crate::EngineError {
    bad(PLAN, message)
}
fn hash(v: &impl Serialize) -> Result<String> {
    Ok(format!("{:x}", Sha256::digest(project_audit::value_bytes(v)?)))
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Face {
    family: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    style: Option<String>,
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Replacement {
    from: Face,
    to: Face,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Query {
    token: String,
    scope: ProjectScope,
    before_project_sha256: String,
    replacement: Replacement,
    offset: usize,
    limit: usize,
}
fn valid_text(s: &str) -> bool {
    !s.is_empty() && s.len() <= 16 * 1024 && !s.contains('\0')
}
fn valid_hash(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
fn request(p: &Value) -> Result<Query> {
    let q: Query = parse(PLAN, p)?;
    if !valid_hash(&q.before_project_sha256)
        || q.offset > MAX_ROWS
        || !(1..=16).contains(&q.limit)
        || [&q.replacement.from, &q.replacement.to].iter().any(|f| !valid_text(&f.family) || f.style.as_ref().is_some_and(|s| !valid_text(s)))
        || ["from", "to"].iter().any(|k| p["replacement"][k].get("style").is_some_and(|s| !s.is_string()))
    {
        return Err(error("current project hash, exact nonempty font names and bounded page required; null style cannot widen a match"));
    }
    Ok(q)
}
#[derive(Default, Serialize)]
#[serde(rename_all = "camelCase")]
struct Totals {
    text_layers: usize,
    source_master_layers: usize,
    caption_tracks: usize,
    changed_font_parameters: usize,
    changed_saved_runs: usize,
    removed_font_keyframes: usize,
}
fn add(n: &mut usize, value: usize) -> Result<()> {
    *n = n.checked_add(value).ok_or_else(|| error("font collateral count overflow"))?;
    Ok(())
}
fn push(rows: &mut Vec<Value>, value: Value) -> Result<()> {
    if rows.len() >= MAX_ROWS {
        return Err(error("complete font collateral exceeds100000 rows; nothing truncated"));
    }
    if project_audit::value_bytes(&value)?.len() > 60 * 1024 {
        return Err(error("one complete font collateral row exceeds its bound"));
    }
    rows.push(value);
    Ok(())
}
fn layer(before: &EffectInstance, after: &EffectInstance, locator: Value, master: bool, rows: &mut Vec<Value>, totals: &mut Totals) -> Result<()> {
    let before_sha = hash(before)?;
    let after_sha = hash(after)?;
    if before_sha == after_sha {
        return Ok(());
    }
    let mut params = Vec::new();
    let mut lost = 0usize;
    for name in ["font", "font_style"] {
        let a = before.params.get(name);
        let b = after.params.get(name);
        if hash(&a)? != hash(&b)? {
            let old_keys = a.map_or(0, |p| p.keyframes.len());
            let new_keys = b.map_or(0, |p| p.keyframes.len());
            let removed = old_keys.checked_sub(new_keys).ok_or_else(|| error("original replacement unexpectedly added animation"))?;
            add(&mut lost, removed)?;
            params.push(json!({"property":name,"beforeSha256":hash(&a)?,"afterSha256":hash(&b)?,"beforeKeyframes":old_keys,"afterKeyframes":new_keys,"removedKeyframes":removed}));
        }
    }
    let a = before.layer.as_ref().map(|v| v.runs.as_slice()).unwrap_or_default();
    let b = after.layer.as_ref().map(|v| v.runs.as_slice()).unwrap_or_default();
    if a.len() != b.len() {
        return Err(error("original replacement unexpectedly changed saved run boundaries"));
    }
    let changed_runs = a.iter().zip(b.iter()).try_fold(0usize, |n, (a, b)| -> Result<usize> {
        if hash(a)? == hash(b)? { Ok(n) } else { n.checked_add(1).ok_or_else(|| error("run count overflow")) }
    })?;
    if master {
        add(&mut totals.source_master_layers, 1)?;
    } else {
        add(&mut totals.text_layers, 1)?;
    }
    add(&mut totals.changed_font_parameters, params.len())?;
    add(&mut totals.changed_saved_runs, changed_runs)?;
    add(&mut totals.removed_font_keyframes, lost)?;
    push(
        rows,
        json!({"kind":"text_layer","locator":locator,"beforeSha256":before_sha,"afterSha256":after_sha,"parameters":params,
        "savedRuns":{"beforeSha256":hash(&a)?,"afterSha256":hash(&b)?,"total":a.len(),"changed":changed_runs},"removedFontKeyframes":lost}),
    )
}
fn collateral(before: &Project, after: &Project) -> Result<(Vec<Value>, Totals)> {
    let mut rows = Vec::new();
    let mut totals = Totals::default();
    for item in before.sequences() {
        let ItemKind::Sequence(sq) = &item.kind else { continue };
        let predicted = after.sequence(item.id).ok_or_else(|| error("replacement lost a sequence"))?;
        for (track_index, track) in sq.video_tracks.iter().enumerate() {
            let next_track = predicted
                .video_tracks
                .get(track_index)
                .filter(|t| t.id == track.id && t.items.len() == track.items.len())
                .ok_or_else(|| error("replacement changed video track structure"))?;
            for (clip_index, clip) in track.items.iter().enumerate() {
                let next = next_track
                    .items
                    .get(clip_index)
                    .filter(|c| c.id == clip.id && c.effects.len() == clip.effects.len())
                    .ok_or_else(|| error("replacement changed clip structure"))?;
                for (index, effect) in clip.effects.iter().enumerate().filter(|(_, e)| e.effect == graphic::TEXT_LAYER) {
                    let e = next.effects.get(index).ok_or_else(|| error("replacement lost a text layer"))?;
                    layer(
                        effect,
                        e,
                        json!({"kind":"sequence","sequence":item.id.0,"sequenceName":item.name,"track":track.id.0,"trackName":track.name,"trackPosition":track_index,"clip":clip.id.0,"clipPosition":clip_index,"layerPosition":index,"trackLocked":track.locked}),
                        false,
                        &mut rows,
                        &mut totals,
                    )?;
                }
            }
        }
        if sq.caption_tracks.len() != predicted.caption_tracks.len() {
            return Err(error("replacement changed caption track structure"));
        }
        for (index, track) in sq.caption_tracks.iter().enumerate() {
            let next = predicted.caption_tracks.get(index).filter(|t| t.id == track.id).ok_or_else(|| error("replacement lost a caption track"))?;
            if hash(&track.style)? != hash(&next.style)? {
                add(&mut totals.caption_tracks, 1)?;
                push(
                    &mut rows,
                    json!({"kind":"caption_track","locator":{"kind":"sequence","sequence":item.id.0,"sequenceName":item.name,"track":track.id.0,"trackName":track.name,"trackPosition":index,"trackLocked":track.locked},
                    "beforeSha256":hash(track)?,"afterSha256":hash(next)?,"beforeStyleSha256":hash(&track.style)?,"afterStyleSha256":hash(&next.style)?,"beforeFamily":track.style.font,"afterFamily":next.style.font}),
                )?;
            }
        }
    }
    for (item, source) in &before.source_graphics {
        let next = after
            .source_graphics
            .get(item)
            .filter(|v| v.layers.len() == source.layers.len())
            .ok_or_else(|| error("replacement changed source master structure"))?;
        for (index, effect) in source.layers.iter().enumerate().filter(|(_, e)| e.effect == graphic::TEXT_LAYER) {
            layer(
                effect,
                next.layers.get(index).ok_or_else(|| error("replacement lost a master text layer"))?,
                json!({"kind":"source_master","item":item.0,"sourceName":before.item(*item).map(|i|i.name.as_str()),"layerPosition":index}),
                true,
                &mut rows,
                &mut totals,
            )?;
        }
    }
    Ok((rows, totals))
}
pub(super) fn plan(s: &mut Session, p: &Value) -> Result<Value> {
    let q = request(p)?;
    project_plane::validate_ticket(s, &q.token, &q.scope)?;
    let before = project_plane::basis(s)?;
    if before != q.before_project_sha256 {
        return Err(error("complete owning project changed; read again"));
    }
    let replacement = serde_json::to_value(&q.replacement).map_err(|e| error(e.to_string()))?;
    let (predicted, replaced, rows, totals) = prediction(&s.project, &replacement)?;
    let after = hash(&predicted)?;
    if q.offset > rows.len() {
        return Err(error("font collateral offset outside complete collection"));
    }
    let end = q.offset.saturating_add(q.limit).min(rows.len());
    let out = json!({"status":"planned","kind":"native_project_font_replacement_plan","version":1,"token":q.token,"scope":q.scope,
        "revision":s.revision,"sequence":s.state.active_sequence.map(|id|id.0),"beforeProjectSha256":before,"afterProjectSha256":after,
        "proposalSha256":hash(&("project_font_replacement_authored_plan_v1",&q.token,&q.scope,&before,&q.replacement))?,"replacement":q.replacement,
        "collateralSha256":hash(&rows)?,"offset":q.offset,"total":rows.len(),"nextOffset":if end<rows.len(){Some(end)}else{None},"rows":rows.get(q.offset..end).ok_or_else(||error("missing collateral page"))?,"totals":totals,
        "humanCommand":"file.replaceFonts","humanResult":{"replaced":replaced},"changesAuthoredState":before!=after,
        "referenceScope":"complete_current_project_authored_font_changes","keyframeInventory":"complete_changed_font_parameters_only",
        "rendererSampling":"not-performed","resourceEvidence":false,"sourceValidation":"not-performed","fontFileState":"not-inspected","embeddingRights":"not-established","writeAuthority":false});
    if project_audit::value_bytes(&out)?.len() > 64 * 1024 {
        return Err(error("complete font collateral page exceeds64KiB; request fewer rows"));
    }
    // This metadata plan neither consumes the original ticket nor mutates it.
    project_plane::validate_ticket(s, &q.token, &q.scope)?;
    Ok(out)
}

/// Shared strict original human mutation and complete authored collateral.
pub(super) fn prediction(project: &Project, replacement: &Value) -> Result<(Project, usize, Vec<Value>, Value)> {
    let typed: Replacement = parse(PLAN, replacement)?;
    if [&typed.from, &typed.to].iter().any(|f| !valid_text(&f.family) || f.style.as_ref().is_some_and(|s| !valid_text(s)))
        || ["from", "to"].iter().any(|k| replacement[k].get("style").is_some_and(|s| !s.is_string()))
    {
        return Err(error("exact canonical font replacement required; null style cannot widen matching"));
    }
    let human = serde_json::to_value(&typed).map_err(|e| error(e.to_string()))?;
    let spec = crate::graphic_templates::prepare_font_replacement(&human)?;
    let mut predicted = project.clone();
    let replaced = crate::graphic_templates::apply_font_replacement(&mut predicted, &spec);
    let (rows, totals) = collateral(project, &predicted)?;
    Ok((predicted, replaced, rows, serde_json::to_value(totals).map_err(|e| error(e.to_string()))?))
}
