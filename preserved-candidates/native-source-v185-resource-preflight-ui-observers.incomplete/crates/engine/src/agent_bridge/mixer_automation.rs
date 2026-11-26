//! Complete, bounded authored strip lanes and separately paged native keyframes.
//! This is document metadata. It neither evaluates media nor authorizes an edit.
use super::{finishing, parse, project_audit, sequences, tracks};
use crate::{EngineError, Result, Session, commands::bad};
use filmcraft_project::mixer::{MASTER_STRIP, PAN51_LANES, fx_lane, lane_info, parse_fx_lane, send_lane};
use filmcraft_project::{Param, ParamDef, ParamValue, Sequence, Track, TrackId};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{borrow::Cow, collections::BTreeSet};
const COMMAND: &str = "agent.readMixerAutomation";
const MAX_LANES: usize = 4096;
const MAX_KEYS: usize = 65536;
const SAFE: i64 = 9_007_199_254_740_991;
fn error(message: &str) -> EngineError {
    bad(COMMAND, message)
}
fn text(s: &str) -> Result<()> {
    if s.is_empty() || s.len() > 1024 || s.contains('\0') {
        return Err(error("invalid or overlong native lane key"));
    }
    Ok(())
}
struct Lane<'a> {
    key: String,
    storage: &'static str,
    origin: &'static str,
    param: Cow<'a, Param>,
    definition: Option<&'a ParamDef>,
    native: bool,
}
fn push<'a>(rows: &mut Vec<Lane<'a>>, row: Lane<'a>) -> Result<()> {
    text(&row.key)?;
    if rows.len() >= MAX_LANES {
        return Err(error("strip lane inventory exceeds 4096; nothing was omitted"));
    }
    rows.push(row);
    Ok(())
}
fn inventory<'a>(q: &Sequence, tr: &'a Track, id: TrackId) -> Result<Vec<Lane<'a>>> {
    if tr.mixer.sends.len() > 256 || tr.effects.len() > 4096 || tr.mixer.lanes.len() > MAX_LANES {
        return Err(error("strip authored inventory exceeds its bounded read"));
    }
    let mut rows = Vec::new();
    let mut seen = BTreeSet::new();
    let mut controls = vec!["volume".to_string()];
    if id != MASTER_STRIP {
        controls.extend(["pan".to_string(), "mute".to_string()]);
        controls.extend(PAN51_LANES.iter().map(|k| k.to_string()));
        controls.extend((0..tr.mixer.sends.len()).map(send_lane));
    }
    for key in controls {
        seen.insert(key.clone());
        let (param, origin) = match tr.mixer.lanes.get(&key) {
            Some(p) => (Cow::Borrowed(p), "authored"),
            None => (Cow::Owned(Param::new(ParamValue::Float(tr.lane_static(&key).ok_or_else(|| error("missing native lane static"))?))), "native_default"),
        };
        push(&mut rows, Lane { key, storage: "strip_lane", origin, param, definition: None, native: true })?;
    }
    // Keep every saved lane, including unknown/orphan keys and saved fx.* map
    // entries shadowed by real insert Params. Never merge different storage.
    for (key, p) in &tr.mixer.lanes {
        if seen.contains(key) {
            continue;
        }
        let native = parse_fx_lane(key).is_none() && crate::mixer::automation::validate_lane(q, id, key, COMMAND).is_ok();
        push(
            &mut rows,
            Lane {
                key: key.clone(),
                storage: if native { "strip_lane" } else { "orphan_strip_lane" },
                origin: "authored",
                param: Cow::Borrowed(p),
                definition: None,
                native,
            },
        )?;
    }
    for (slot, e) in tr.effects.iter().enumerate() {
        if e.params.len() > 256 || e.def().is_some_and(|d| d.params.len() > 256) {
            return Err(error("insert parameter inventory exceeds its bound"));
        }
        let def = e.def();
        let mut present = BTreeSet::new();
        for (key, p) in &e.params {
            text(key)?;
            present.insert(key.as_str());
            let lane = fx_lane(slot, key);
            let native = crate::mixer::automation::validate_lane(q, id, &lane, COMMAND).is_ok();
            push(
                &mut rows,
                Lane {
                    key: lane,
                    storage: "insert_parameter",
                    origin: "authored",
                    param: Cow::Borrowed(p),
                    definition: def.and_then(|d| d.param(key)),
                    native,
                },
            )?;
        }
        if let Some(d) = def {
            for pd in &d.params {
                if present.contains(pd.id) {
                    continue;
                }
                push(
                    &mut rows,
                    Lane {
                        key: fx_lane(slot, pd.id),
                        storage: "insert_parameter",
                        origin: "definition_default",
                        param: Cow::Owned(Param::new(pd.default.clone())),
                        definition: Some(pd),
                        native: false,
                    },
                )?;
            }
        }
    }
    rows.sort_by(|a, b| a.key.cmp(&b.key).then(a.storage.cmp(b.storage)));
    let mut budget = 8 * 1024 * 1024usize;
    let mut count = 0usize;
    for row in &rows {
        finishing::value(&row.param.value, &mut budget)?;
        count = count.checked_add(row.param.keyframes.len()).ok_or_else(|| error("strip key count overflow"))?;
        if count > MAX_KEYS {
            return Err(error("complete strip automation exceeds 65536 keys; nothing was truncated"));
        }
        let mut previous = None;
        for k in &row.param.keyframes {
            finishing::keyframe(k, &row.param.value, previous, &mut budget)?;
            previous = Some(k.time);
        }
    }
    Ok(rows)
}
fn descriptor(row: &Lane<'_>, tr: &Track, index: usize, budget: &mut usize) -> Result<Value> {
    let info = lane_info(&row.key).map(|i| json!({"min":i.min,"max":i.max,"hold":i.hold,"unit":i.unit}));
    let definition = finishing::definition(row.definition, budget)?;
    Ok(json!({"position":index+1,"key":row.key,"storage":row.storage,"origin":row.origin,"base":row.param.value,
        "staticValue":if row.native {tr.lane_static(&row.key)}else{None},"keyframes":row.param.keyframes.len(),
        "nativeControlAvailable":row.native,"humanWrittenKeyframeEncoding":if row.native {Some("Float")}else{None},
        "laneInfo":info,"definition":definition}))
}
pub(super) fn validate_after(q: &Sequence, strip: Option<u64>) -> Result<()> {
    let id = strip.map(TrackId).unwrap_or(MASTER_STRIP);
    let tr = q.strip(id).ok_or_else(|| error("resulting automation strip is missing"))?;
    inventory(q, &tr, id)?;
    Ok(())
}
pub(super) struct EditTarget {
    pub(super) strip: TrackId,
    pub(super) key: String,
    pub(super) storage: &'static str,
    pub(super) param: Param,
    pub(super) range: (f64, f64, bool),
}
pub(super) fn edit_target(q: &Sequence, strip: Option<u64>, index: usize) -> Result<EditTarget> {
    let id = strip.map(TrackId).unwrap_or(MASTER_STRIP);
    let tr = q.strip(id).ok_or_else(|| error("read an existing mixer strip"))?;
    if tr.locked {
        return Err(error("unlock this strip explicitly before automation edits"));
    }
    let rows = inventory(q, &tr, id)?;
    let row = rows.get(index).ok_or_else(|| error("read this exact lane in the current complete inventory"))?;
    if !row.native || row.storage == "orphan_strip_lane" || row.origin == "definition_default" || !matches!(row.param.value, ParamValue::Float(_)) {
        return Err(error("exact automation requires a present native Float lane; typed static insert controls remain separate"));
    }
    let range = if row.storage == "insert_parameter" {
        let d = row.definition.filter(|d| d.animatable).ok_or_else(|| error("the human timeline exposes only known animatable Float insert lanes"))?;
        match d.kind {
            filmcraft_project::ParamKind::Float { min, max, .. } if min.is_finite() && max.is_finite() && min <= max => (min, max, false),
            _ => return Err(error("use the original typed insert settings for Bool/Choice/non-Float controls")),
        }
    } else {
        let i = lane_info(&row.key).ok_or_else(|| error("unknown native automation lane"))?;
        (i.min, i.max, i.hold)
    };
    Ok(EditTarget { strip: id, key: row.key.clone(), storage: row.storage, param: row.param.clone().into_owned(), range })
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Read {
    token: String,
    scope: sequences::Scope,
    strip: Option<u64>,
    lane: Option<usize>,
    offset: usize,
    limit: usize,
    #[serde(default)]
    projection: bool,
}
pub(super) fn read(s: &mut Session, p: &Value) -> Result<Value> {
    let r: Read = parse(COMMAND, p)?;
    if p.get("strip").is_none()
        || p.get("lane").is_none()
        || r.strip.is_some_and(|v| v == 0 || v > SAFE as u64)
        || r.lane.is_some_and(|v| v >= MAX_LANES)
        || r.offset > MAX_KEYS
        || r.limit == 0
        || r.limit > if r.lane.is_none() { 16 } else { 64 }
    {
        return Err(error("explicit strip/lane choice and bounded authored page required"));
    }
    super::validate_metadata_read(s, &r.token, r.projection, COMMAND)?;
    let sid = s.state.active_sequence.ok_or(EngineError::NoSequence)?;
    if !sequences::belongs(&s.project, sid, &r.scope)? {
        return Err(error("mixer automation is outside its original workspace"));
    }
    let q = s.active_sequence().ok_or(EngineError::NoSequence)?;
    let id = r.strip.map(TrackId).unwrap_or(MASTER_STRIP);
    let tr = q.strip(id).ok_or_else(|| error("read a current audio/submix strip or explicit null master"))?;
    let playhead = s.playhead();
    if !(-SAFE..=SAFE).contains(&playhead.0) {
        return Err(error("current playhead cannot be represented as exact native ticks"));
    }
    let rows = inventory(q, &tr, id)?;
    let sha = tracks::digest(q)?;
    let mut budget = 96 * 1024usize;
    let (total, lanes, keys, selected) = if let Some(index) = r.lane {
        let row = rows.get(index).ok_or_else(|| error("lane position is outside the current complete inventory"))?;
        let total = row.param.keyframes.len();
        if r.offset > total {
            return Err(error("keyframe offset exceeds the complete observed lane"));
        }
        let end = r.offset.saturating_add(r.limit).min(total);
        (
            total,
            Vec::new(),
            json!(row.param.keyframes.get(r.offset..end).ok_or_else(|| error("invalid keyframe page"))?),
            descriptor(row, &tr, index, &mut budget)?,
        )
    } else {
        if r.offset > rows.len() {
            return Err(error("lane offset exceeds the complete strip inventory"));
        }
        let end = r.offset.saturating_add(r.limit).min(rows.len());
        let mut page = Vec::new();
        for (index, row) in rows.iter().enumerate().take(end).skip(r.offset) {
            page.push(descriptor(row, &tr, index, &mut budget)?);
        }
        (rows.len(), page, json!([]), Value::Null)
    };
    let end = r.offset.saturating_add(r.limit).min(total);
    let result = json!({"status":"read","token":r.token,"scope":r.scope,"revision":s.revision,"sequence":sid.0,"sha256":sha,"strip":r.strip,"lane":r.lane,
        "playhead":playhead.0,"sampleRate":q.settings.sample_rate,"mode":tr.mixer.mode,"offset":r.offset,"total":total,"nextOffset":if end<total{Some(end)}else{None},
        "lanes":lanes,"keyframes":keys,"descriptor":selected,"timeDomain":"native_sequence_ticks","referenceScope":"exact_read_ticket_only","requiresSourceValidation":r.projection});
    if project_audit::value_bytes(&result)?.len() > 128 * 1024 {
        return Err(error("mixer automation page exceeds 128KiB; reduce the page size"));
    }
    Ok(result)
}
#[cfg(test)]
mod tests;
