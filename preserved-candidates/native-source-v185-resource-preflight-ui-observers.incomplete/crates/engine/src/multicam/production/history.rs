//! Exact, data-only validation of an already computed native outcome.
//! Shared native metadata/geometry/materializers and edit algebra; no Session::execute,
//! Session::edit, decoder, audio analysis, new identities, journal or publication.
use super::*;
use serde::Deserialize;
fn native_bytes(v: &impl serde::Serialize) -> Result<Vec<u8>> {
    crate::agent_bridge::canonical_native_value(v)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct Row {
    item: u64,
    name: String,
    start: i64,
    start_frame: i64,
    anchor: i64,
    audio: Option<Audio>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Audio {
    lag_samples: i64,
    lag_fine: f64,
    confidence: f32,
    distinct: f32,
    reliable: bool,
    inverted: bool,
}
pub(super) fn anchors(clips: &[SyncClip], reference: usize, method: &Method, offset: Tick, rows: &[Row], rate: FrameRate) -> Result<Vec<sync::Anchor>> {
    if rows.len() != clips.len() {
        return Err(bad("multicam.history", "complete original computed observations required"));
    }
    for (c, row) in clips.iter().zip(rows) {
        if rate.frame_at(Tick(row.start)) != row.start_frame || c.item.0 != row.item || c.name != row.name {
            return Err(bad("multicam.history", "computed observation source/order differs"));
        }
    }
    let out = sync::anchor_values(clips, reference, method, offset, |i, _| {
        let a = rows.get(i).and_then(|row| row.audio.as_ref()).ok_or_else(|| bad("multicam.history", "retained audio observation missing"))?;
        if !a.lag_fine.is_finite() || !a.confidence.is_finite() || !a.distinct.is_finite() {
            return Err(bad("multicam.history", "nonfinite retained audio observation"));
        }
        let r = filmcraft_audio_dsp::sync::SyncResult {
            lag: a.lag_samples,
            lag_fine: a.lag_fine,
            confidence: a.confidence,
            distinct: a.distinct,
            inverted: a.inverted,
        };
        if r.reliable() != a.reliable {
            return Err(bad("multicam.history", "retained audio reliability differs"));
        }
        Ok(r)
    })?;
    for (a, row) in out.iter().zip(rows) {
        if a.anchor.0 != row.anchor || a.audio.is_some() != row.audio.is_some() {
            return Err(bad("multicam.history", "computed anchors differ from original method and retained observations"));
        }
    }
    Ok(out)
}
/// Check hostile native arithmetic before shared geometry or overwrite can inspect it.
pub(super) fn numeric(project: &Project, rows: usize) -> Result<()> {
    if project.items.len() > 100_000 || rows > 2048 {
        return Err(bad("multicam.history", "bounded complete native inventory required"));
    }
    let mut clips = 0usize;
    for item in project.items.values() {
        if let Some(media) = item.as_media() {
            if media.mark_out.is_some_and(|t| t.0.checked_add(media.frame_rate().frame_duration().0).is_none()) {
                return Err(bad("multicam.history", "source Out overflow"));
            }
        }
        if let ItemKind::Subclip { range, .. } = &item.kind {
            if range.start.0.checked_add(range.duration.0).is_none() {
                return Err(bad("multicam.history", "subclip range overflow"));
            }
        }
        if let Some(q) = item.as_sequence() {
            for c in q.all_tracks().flat_map(|t| &t.items) {
                clips = clips.checked_add(1).ok_or_else(|| bad("multicam.history", "clip count overflow"))?;
                let source = finite_tick((c.duration.0 as f64 * c.speed.abs()).round())?;
                if clips > 100_000 || c.duration < Tick::ZERO || c.start.0.checked_add(c.duration.0).is_none() || c.source_in.0.checked_add(source.0).is_none()
                {
                    return Err(bad("multicam.history", "native clip range or inventory overflow"));
                }
            }
        }
    }
    let need = clips
        .checked_mul(4)
        .and_then(|v| rows.checked_mul(8).and_then(|r| v.checked_add(r)))
        .and_then(|v| v.checked_add(16))
        .ok_or_else(|| bad("multicam.history", "native allocation budget overflow"))?;
    if project.next_id.checked_add(need as u64).is_none() {
        return Err(bad("multicam.history", "native allocator exhausted"));
    }
    Ok(())
}
pub(crate) fn validate_computed_transition(
    before: &Project,
    after: &Project,
    before_editor: &crate::EditorState,
    after_editor: &crate::EditorState,
    command: &str,
    params: &Value,
    result: &Value,
) -> Result<()> {
    let values = result["clips"]
        .as_array()
        .filter(|r| !r.is_empty() && r.len() <= 2048)
        .ok_or_else(|| bad("multicam.history", "complete original native report required"))?;
    numeric(before, values.len())?;
    let rows =
        values.iter().map(|v| serde_json::from_value::<Row>(v.clone()).map_err(|e| bad("multicam.history", e.to_string()))).collect::<Result<Vec<_>>>()?;
    // The original parameter readers need only this captured data context. No read
    // ticket, media witness, worker, current epoch or operation is constructed.
    let mut data = Session::default();
    data.project = Arc::new(before.clone());
    data.state = before_editor.clone();
    let mut project = before.clone();
    let mut editor = before_editor.clone();
    let expected;
    let changed;
    match command {
        "clip.synchronize" => {
            let plan = prepare_synchronize_observed(&data, params, Some(&rows))?;
            expected = synchronize_result(&plan);
            changed = plan.moved > 0;
            if changed {
                materialize_synchronize(&mut project, &mut editor, plan)?;
            }
        }
        "clip.mergeClips" => {
            let plan = prepare_merge_observed(&data, params, Some(&rows))?;
            let report = plan.report.clone();
            let id = materialize_merge(&mut project, &mut editor, plan)?;
            expected = json!({"item":id.0,"clips":report});
            changed = true;
        }
        "clip.createMulticam" => {
            let plan = prepare_multicam_observed(&data, params, Some(&rows))?;
            let report = plan.report.clone();
            let names = plan.names.clone();
            let id = materialize_multicam(&mut project, &mut editor, plan)?;
            expected = json!({"sequence":id.0,"cameras":names,"clips":report});
            changed = true;
        }
        _ => return Err(bad("multicam.history", "unsupported computed native gesture")),
    }
    if &expected != result {
        return Err(bad("multicam.history", "computed native result differs from exact original geometry"));
    }
    if changed {
        crate::clip_identity::validate_reconciliation(before, &mut project, after)?;
        crate::editor_state::remap_committed_mask(before, &project, before_editor, &mut editor);
        crate::editor_state::drop_stale_gap(&project, &mut editor);
    }
    if native_bytes(&project)? != native_bytes(after)? || native_bytes(&editor)? != native_bytes(after_editor)? {
        return Err(bad("multicam.history", "historical native placement, trim, link, overwrite, identity, effects or editor state differs"));
    }
    Ok(())
}
