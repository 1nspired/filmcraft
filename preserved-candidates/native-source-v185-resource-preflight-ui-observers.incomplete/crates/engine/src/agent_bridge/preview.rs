//! Source-monitor marking only, staged inside the same guarded batch as timeline edits.
use filmcraft_project::ItemId;
use filmcraft_time::{FrameRate, TICKS_PER_SECOND, Tick};
use serde::Deserialize;
use serde_json::{Value, json};

use super::{BINDING_KEY, Binding, Operation, parse};
use crate::commands::bad;
use crate::{Result, Session};

pub(super) const COMMAND: &str = "agent.previewSource";
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Preview {
    item: u64,
    project_id: String,
    source_in_frame: i64,
    duration_frames: i64,
}
fn frame_duration(rate: FrameRate) -> Result<i64> {
    if rate.num <= 0 || rate.den <= 0 {
        return Err(bad(COMMAND, "invalid frame rate"));
    }
    TICKS_PER_SECOND
        .checked_mul(rate.den)
        .filter(|n| n % rate.num == 0)
        .map(|n| n / rate.num)
        .filter(|n| *n > 0)
        .ok_or_else(|| bad(COMMAND, "frame rate has no bounded exact tick representation"))
}
pub(super) fn validate(op: &Operation) -> Result<()> {
    let p: Preview = parse(COMMAND, &op.params)?;
    if !(1..=9_007_199_254_740_991).contains(&p.item)
        || p.project_id.is_empty()
        || p.project_id.len() > 256
        || p.project_id.chars().any(char::is_control)
        || !(0..=1_000_000).contains(&p.source_in_frame)
        || !(1..=1_000_000).contains(&p.duration_frames)
    {
        return Err(bad(COMMAND, "need an original source, explicit project and bounded sequence-rate source range"));
    }
    Ok(())
}
pub(super) fn run(s: &mut Session, op: &Operation) -> Result<Value> {
    validate(op)?;
    let p: Preview = parse(COMMAND, &op.params)?;
    let id = ItemId(p.item);
    let item = s.project.item(id).ok_or_else(|| bad(COMMAND, "missing original source"))?;
    let b: Binding =
        serde_json::from_str(item.metadata.get(BINDING_KEY).ok_or_else(|| bad(COMMAND, "source is not bound"))?).map_err(|e| bad(COMMAND, e.to_string()))?;
    let media = item.as_media().ok_or_else(|| bad(COMMAND, "only imported original media, not nests/subclips"))?;
    if b.project_id != p.project_id
        || b.role != "original"
        || b.asset_id.is_empty()
        || b.media_id.is_empty()
        || media.offline
        || s.media.offline_status(id).is_some()
    {
        return Err(bad(COMMAND, "need one online original bound to this Poema project"));
    }
    if media.interpret.frame_rate.is_some() || !media.info.has_video() {
        return Err(bad(COMMAND, "source preview currently requires uninterpreted video media"));
    }
    let sequence_rate = s.sequence_rate();
    let source_rate = media.frame_rate();
    if sequence_rate.num <= 0 || sequence_rate.den <= 0 || source_rate.num <= 0 || source_rate.den <= 0 {
        return Err(bad(COMMAND, "invalid source or sequence frame rate"));
    }
    let unit = frame_duration(sequence_rate)?;
    let fd = frame_duration(source_rate)?;
    let end_frame = p.source_in_frame.checked_add(p.duration_frames).ok_or_else(|| bad(COMMAND, "range overflow"))?;
    let start = unit.checked_mul(p.source_in_frame).ok_or_else(|| bad(COMMAND, "time overflow"))?;
    let end = unit
        .checked_mul(end_frame)
        .filter(|t| *t <= media.info.duration.0 && *t <= 9_007_199_254_740_991)
        .ok_or_else(|| bad(COMMAND, "range exceeds original source; no clamping"))?;
    // Source Out is INCLUSIVE. Use only complete camera frames contained in the requested
    // sequence-rate source-time interval; HFR source frames are not sequence frame ordinals.
    let first = start.checked_add(fd - 1).ok_or_else(|| bad(COMMAND, "source-grid overflow"))? / fd;
    let exclusive = end / fd;
    if exclusive <= first {
        return Err(bad(COMMAND, "range contains no complete source frame"));
    }
    let mark_in = first.checked_mul(fd).ok_or_else(|| bad(COMMAND, "source-grid overflow"))?;
    let mark_out = exclusive.checked_sub(1).and_then(|f| f.checked_mul(fd)).ok_or_else(|| bad(COMMAND, "source-grid overflow"))?;
    let mut expected = (*s.project).clone();
    let expected_item = expected.item_mut(id).ok_or_else(|| bad(COMMAND, "missing source"))?;
    expected_item.split.video_in = None;
    expected_item.split.audio_in = None;
    expected_item.split.video_out = None;
    expected_item.split.audio_out = None;
    let expected_media = expected_item.as_media_mut().ok_or_else(|| bad(COMMAND, "missing source"))?;
    expected_media.mark_in = Some(Tick(mark_in));
    expected_media.mark_out = Some(Tick(mark_out));
    s.execute("project.setMarks", json!({"item": p.item, "in": mark_in, "out": mark_out}))?;
    s.execute("source.open", json!({"item": p.item}))?;
    s.execute("source.setPlayhead", json!({"time": mark_in}))?;
    if *s.project != expected || s.state.source_item != Some(id) || s.state.source_playhead.0 != mark_in {
        return Err(bad(COMMAND, "source preview postcondition mismatch; nothing published"));
    }
    Ok(json!({"markIn":mark_in,"markOut":mark_out,"exclusiveEnd":exclusive * fd,"sourceFrameRate":source_rate,
        "requestedStart":start,"requestedEnd":end,"timelineUnchanged":true,"quantization":"complete source frames contained within requested source-time interval; Out inclusive"}))
}
