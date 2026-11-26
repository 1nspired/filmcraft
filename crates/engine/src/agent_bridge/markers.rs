//! Scoped authored marker reads and exact gestures through shared human commands.
//! Native presentation is not Poema chapter evidence or human margin-note resolution.
use super::{BINDING_KEY, Binding, Operation, parse, project_audit, sequences};
use crate::{EngineError, Result, Session, commands::bad};
use filmcraft_project::{ItemId, ItemKind, Label, Marker, MarkerId, MarkerKind, Project};
use filmcraft_time::{FrameRate, Tick};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;

pub(super) const COMMAND: &str = "agent.marker";
const SAFE: i64 = 9_007_199_254_740_991;
fn error(message: impl Into<String>) -> EngineError {
    bad(COMMAND, message)
}
fn text(value: &str, max: usize) -> bool {
    value.len() <= max && !value.contains('\0')
}
fn id(value: u64) -> bool {
    value > 0 && value <= SAFE as u64
}
fn hash(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
}

#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(super) enum Target {
    Sequence,
    Source { item: u64 },
}
#[derive(Clone, Deserialize, Serialize)]
struct Bounds {
    start: i64,
    end: i64,
}
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct Context {
    target: Target,
    root: u64,
    bounds: Option<Bounds>,
    frame_rate: FrameRate,
    open: bool,
    playhead: Option<i64>,
}

fn context(s: &Session, target: &Target, scope: &sequences::Scope) -> Result<Context> {
    let sid = s.state.active_sequence.ok_or(EngineError::NoSequence)?;
    if !sequences::belongs(&s.project, sid, scope)? {
        return Err(error("marker sequence is outside this workspace"));
    }
    let result = match target {
        Target::Sequence => {
            Context { target: target.clone(), root: sid.0, bounds: None, frame_rate: s.sequence_rate(), open: true, playhead: Some(s.playhead().0) }
        }
        Target::Source { item } => {
            if !id(*item) {
                return Err(error("bounded explicit Source item required"));
            }
            let view = crate::clip_ops::source_view(s, ItemId(*item)).ok_or_else(|| error("Source marker target is unavailable"))?;
            let root = s.project.item(view.media).ok_or_else(|| error("Source marker owner is missing"))?;
            match &root.kind {
                ItemKind::Media(_) => {
                    let binding: Binding = serde_json::from_str(root.metadata.get(BINDING_KEY).ok_or_else(|| error("Source is not explicitly bound"))?)
                        .map_err(|e| error(e.to_string()))?;
                    if binding.project_id != scope.project_id || binding.role != "original" || binding.asset_id.is_empty() || binding.media_id.is_empty() {
                        return Err(error("Source belongs to another original owner"));
                    }
                }
                ItemKind::Sequence(_) if sequences::belongs(&s.project, view.media, scope)? => {}
                _ => return Err(error("Source markers are outside this workspace")),
            }
            if !(0..=SAFE).contains(&view.start.0) || !(view.start.0..=SAFE).contains(&view.end.0) {
                return Err(error("Source bounds exceed exact marker transport"));
            }
            let open = s.state.source_item == Some(ItemId(*item));
            Context {
                target: target.clone(),
                root: view.media.0,
                bounds: Some(Bounds { start: view.start.0, end: view.end.0 }),
                frame_rate: view.rate,
                open,
                playhead: if open { Some(s.state.source_playhead.0) } else { None },
            }
        }
    };
    if !id(result.root) || result.frame_rate.num <= 0 || result.frame_rate.den <= 0 {
        return Err(error("invalid marker owner or clock"));
    }
    Ok(result)
}
fn list(project: &Project, root: u64) -> Result<&[Marker]> {
    match &project.item(ItemId(root)).ok_or_else(|| error("marker owner missing"))?.kind {
        ItemKind::Media(media) => Ok(&media.markers),
        ItemKind::Sequence(q) => Ok(&q.markers),
        _ => Err(error("unsupported marker collection")),
    }
}
fn list_mut(project: &mut Project, root: u64) -> Result<&mut Vec<Marker>> {
    match &mut project.item_mut(ItemId(root)).ok_or_else(|| error("marker owner missing"))?.kind {
        ItemKind::Media(media) => Ok(&mut media.markers),
        ItemKind::Sequence(q) => Ok(&mut std::sync::Arc::make_mut(q).markers),
        _ => Err(error("unsupported marker collection")),
    }
}
pub(super) fn digest(markers: &[Marker]) -> Result<String> {
    if markers.len() > 100_000 {
        return Err(error("marker inventory exceeds 100000"));
    }
    let mut ids = BTreeSet::new();
    let mut budget = 4 * 1024 * 1024usize;
    for marker in markers {
        budget = budget
            .checked_sub(marker.name.len().saturating_add(marker.comment.len()).saturating_mul(6).saturating_add(512))
            .ok_or_else(|| error("complete marker state exceeds 4 MiB"))?;
        if !id(marker.id.0)
            || !ids.insert(marker.id.0)
            || !text(&marker.name, 1024)
            || !text(&marker.comment, 65536)
            || !(0..=SAFE).contains(&marker.start.0)
            || !(0..=SAFE).contains(&marker.duration.0)
            || marker.start.0.checked_add(marker.duration.0).is_none_or(|v| v > SAFE)
        {
            return Err(error("invalid, duplicated or oversized native marker"));
        }
    }
    Ok(format!("{:x}", Sha256::digest(serde_json::to_vec(markers).map_err(|e| error(e.to_string()))?)))
}
fn visible(marker: &Marker, context: &Context) -> bool {
    context.bounds.as_ref().is_none_or(|b| marker.start.0 >= b.start && marker.start.0 < b.end)
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Read {
    token: String,
    scope: sequences::Scope,
    target: Target,
    offset: usize,
    limit: usize,
    #[serde(default)]
    projection: bool,
}
pub(super) fn read(s: &mut Session, p: &Value) -> Result<Value> {
    let r: Read = parse("agent.readMarkers", p)?;
    if r.offset > 100_000 || r.limit == 0 || r.limit > 16 {
        return Err(error("bounded explicit marker page required"));
    }
    if r.projection {
        #[cfg(target_arch = "wasm32")]
        return Err(error("marker projection requires native source qualification"));
        #[cfg(not(target_arch = "wasm32"))]
        super::validate_read_basis(s, &r.token)?;
    } else {
        super::validate_snapshot(s, &json!({"token":r.token}))?;
    }
    let context = context(s, &r.target, &r.scope)?;
    let all = list(&s.project, context.root)?;
    let sha = digest(all)?;
    let rows: Vec<_> = all.iter().filter(|m| visible(m, &context)).collect();
    if r.offset > rows.len() {
        return Err(error("marker offset exceeds visible inventory"));
    }
    let end = r.offset.saturating_add(r.limit).min(rows.len());
    let page = rows.get(r.offset..end).ok_or_else(|| error("invalid marker page"))?;
    let result = json!({"status":"read","token":r.token,"scope":r.scope,"revision":s.revision,"sequence":s.state.active_sequence.map(|id|id.0),
        "context":context,"sha256":sha,"rootTotal":all.len(),"offset":r.offset,"total":rows.len(),"nextOffset":if end<rows.len(){Some(end)}else{None},
        "markers":page,"requiresSourceValidation":r.projection});
    if serde_json::to_vec(&result).map_err(|e| error(e.to_string()))?.len() > 512 * 1024 {
        return Err(error("marker page exceeds 512 KiB; reduce page size"));
    }
    Ok(result)
}

#[derive(Deserialize, Serialize)]
#[serde(untagged, deny_unknown_fields)]
enum Time {
    Frame { frame: i64 },
    Ticks { ticks: i64 },
}
impl Time {
    fn valid(&self) -> bool {
        match self {
            Self::Frame { frame } => (0..=1_000_000).contains(frame),
            Self::Ticks { ticks } => (0..=SAFE).contains(ticks),
        }
    }
    fn tick(&self, rate: FrameRate) -> Result<Tick> {
        let value = match self {
            Self::Frame { frame } => crate::marker_ops::position(&json!({"frame":frame}), rate, COMMAND)?.ok_or_else(|| error("missing frame position"))?,
            Self::Ticks { ticks } => Tick(*ticks),
        };
        if !(0..=SAFE).contains(&value.0) {
            return Err(error("marker time exceeds exact transport"));
        }
        Ok(value)
    }
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Changes {
    position: Option<Time>,
    duration: Option<Time>,
    name: Option<String>,
    comment: Option<String>,
    color: Option<String>,
}
#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
enum Edit {
    Add { kind: String, position: Time, duration: Time, name: String, comment: String, color: String },
    Set { marker: u64, changes: Changes },
    Remove { marker: u64 },
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Request {
    scope: sequences::Scope,
    target: Target,
    before_sha256: String,
    edit: Edit,
}
pub(super) fn validate(op: &Operation) -> Result<()> {
    let r: Request = parse(COMMAND, &op.params)?;
    if !hash(&r.before_sha256) {
        return Err(error("current marker digest required"));
    }
    let valid = match r.edit {
        Edit::Add { kind, position, duration, name, comment, color } => {
            ["comment", "chapter", "flash_cue"].contains(&kind.as_str())
                && position.valid()
                && duration.valid()
                && text(&name, 1024)
                && text(&comment, 65536)
                && Label::from_name(&color).is_some()
        }
        Edit::Remove { marker } => id(marker),
        Edit::Set { marker, changes: c } => {
            id(marker)
                && c.position.as_ref().is_none_or(Time::valid)
                && c.duration.as_ref().is_none_or(Time::valid)
                && c.name.as_ref().is_none_or(|s| text(s, 1024))
                && c.comment.as_ref().is_none_or(|s| text(s, 65536))
                && c.color.as_ref().is_none_or(|s| Label::from_name(s).is_some())
                && op
                    .params
                    .get("edit")
                    .and_then(|e| e.get("changes"))
                    .and_then(Value::as_object)
                    .is_some_and(|o| !o.is_empty() && o.values().all(|v| !v.is_null()))
        }
    };
    if !valid {
        return Err(error("explicit bounded marker gesture required"));
    }
    Ok(())
}
pub(super) fn run(s: &mut Session, op: &Operation) -> Result<Value> {
    validate(op)?;
    let r: Request = parse(COMMAND, &op.params)?;
    let context = context(s, &r.target, &r.scope)?;
    if !context.open {
        return Err(error("open the exact Source target and take a fresh read before editing"));
    }
    let before = list(&s.project, context.root)?;
    if digest(before)? != r.before_sha256 {
        return Err(error("marker inventory changed since its read"));
    }
    let mut expected = (*s.project).clone();
    let source = matches!(r.target, Target::Source { .. });
    let target = if source { "source" } else { "program" };
    let (command, params, marker) = match r.edit {
        Edit::Add { kind, position, duration, name, comment, color } => {
            let start = position.tick(context.frame_rate)?;
            let duration = duration.tick(context.frame_rate)?;
            if start.0.checked_add(duration.0).is_none_or(|v| v > SAFE)
                || context.bounds.as_ref().is_some_and(|b| start.0 < b.start || start.0 >= b.end || start.0 + duration.0 > b.end)
            {
                return Err(error("marker range lies outside its exact target"));
            }
            if source && kind != "comment" || kind == "chapter" && duration != Tick::ZERO {
                return Err(error("this marker kind/duration has no matching human target gesture"));
            }
            let (command, kind) = match kind.as_str() {
                "chapter" => ("markers.addChapter", MarkerKind::Chapter),
                "flash_cue" => ("markers.addFlashCue", MarkerKind::FlashCue),
                _ => ("markers.add", MarkerKind::Comment),
            };
            let next = crate::project_ids::allocation_start(&expected, 1)?;
            if !id(next) || next >= SAFE as u64 {
                return Err(error("new marker identity exceeds exact transport"));
            }
            expected.next_id = next + 1;
            let rows = list_mut(&mut expected, context.root)?;
            rows.push(Marker {
                id: MarkerId(next),
                start,
                duration,
                name: name.clone(),
                comment: comment.clone(),
                kind,
                color: Label::from_name(&color).ok_or_else(|| error("unknown marker color"))?,
            });
            rows.sort_by_key(|m| m.start);
            (command, json!({"target":target,"time":start.0,"duration":duration.0,"name":name,"comment":comment,"color":color}), next)
        }
        Edit::Set { marker, changes: c } => {
            let i = crate::marker_ops::index(before, MarkerId(marker), COMMAND)?;
            if !before.get(i).is_some_and(|m| visible(m, &context)) {
                return Err(error("marker is outside the current Source view"));
            }
            let mut params = json!({"target":target,"marker":marker});
            let rows = list_mut(&mut expected, context.root)?;
            let m = rows.get_mut(i).ok_or_else(|| error("marker missing"))?;
            if let Some(t) = c.position {
                m.start = t.tick(context.frame_rate)?;
                params["time"] = json!(m.start.0);
            }
            if let Some(t) = c.duration {
                m.duration = t.tick(context.frame_rate)?;
                params["duration"] = json!(m.duration.0);
            }
            if m.start.0.checked_add(m.duration.0).is_none_or(|v| v > SAFE)
                || context.bounds.as_ref().is_some_and(|b| m.start.0 < b.start || m.start.0 >= b.end || m.start.0 + m.duration.0 > b.end)
            {
                return Err(error("marker range would clamp outside its exact target"));
            }
            if let Some(name) = c.name {
                m.name = name.clone();
                params["name"] = json!(name);
            }
            if let Some(comment) = c.comment {
                m.comment = comment.clone();
                params["comment"] = json!(comment);
            }
            if let Some(color) = c.color {
                m.color = Label::from_name(&color).ok_or_else(|| error("unknown marker color"))?;
                params["color"] = json!(color);
            }
            rows.sort_by_key(|m| m.start);
            ("markers.edit", params, marker)
        }
        Edit::Remove { marker } => {
            let i = crate::marker_ops::index(before, MarkerId(marker), COMMAND)?;
            if !before.get(i).is_some_and(|m| visible(m, &context)) {
                return Err(error("marker is outside the current Source view"));
            }
            list_mut(&mut expected, context.root)?.remove(i);
            ("markers.remove", json!({"target":target,"marker":marker}), marker)
        }
    };
    let after = digest(list(&expected, context.root)?)?;
    if after == r.before_sha256 {
        return Err(error("marker gesture makes no change"));
    }
    let expected_bytes = project_audit::bytes(&expected)?;
    let editor = project_audit::value_bytes(&s.state)?;
    s.execute(command, params)?;
    if project_audit::bytes(&s.project)? != expected_bytes || project_audit::value_bytes(&s.state)? != editor {
        return Err(error("human marker gesture clamped or changed unexpected state; staging discarded"));
    }
    Ok(json!({"target":r.target,"beforeSha256":r.before_sha256,"afterSha256":after,"marker":marker,"humanCommand":command}))
}
