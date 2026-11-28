//! Bounded, in-memory sequence lifecycle primitives for the owning Poema project.
//! The host initializes editorial aliases inside the SAME guarded preview before publication.
use crate::{Result, Session, commands::bad};
use filmcraft_project::{CaptionFormat, CaptionTrack, ItemId, ItemKind, MonoSourceLaw, ProgramExtentPolicy, SequenceSettings, StereoBalanceLaw, TrackId};
use filmcraft_time::FrameRate;
use serde::Deserialize;
use serde_json::{Value, json};
const CREATE: &str = "poema.createSequence";
// Check depth/nodes/string budget BEFORE cloning or deserializing hostile Values.
fn bounded(params: &Value, command: &'static str) -> Result<()> {
    let mut stack = vec![(params, 0usize)];
    let mut nodes = 0usize;
    let mut bytes = 0usize;
    while let Some((value, depth)) = stack.pop() {
        nodes = nodes.checked_add(1).ok_or_else(|| bad(command, "parameter budget overflow"))?;
        if nodes > 8192 || depth > 64 {
            return Err(bad(command, "parameter nesting/node limit"));
        }
        match value {
            Value::String(text) => {
                bytes = bytes.checked_add(text.len()).ok_or_else(|| bad(command, "parameter budget overflow"))?;
            }
            Value::Array(values) => {
                if values.len() > 8192 || stack.len().saturating_add(values.len()) > 8192 {
                    return Err(bad(command, "parameter node limit"));
                }
                stack.extend(values.iter().map(|v| (v, depth + 1)));
            }
            Value::Object(values) => {
                if values.len() > 8192 || stack.len().saturating_add(values.len()) > 8192 {
                    return Err(bad(command, "parameter node limit"));
                }
                for (key, value) in values {
                    bytes = bytes.checked_add(key.len()).ok_or_else(|| bad(command, "parameter budget overflow"))?;
                    stack.push((value, depth + 1));
                }
            }
            _ => {}
        }
        if bytes > 131072 {
            return Err(bad(command, "parameter string limit"));
        }
    }
    Ok(())
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Frame {
    width: u32,
    height: u32,
    rate: FrameRate,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Create {
    name: String,
    #[serde(default)]
    from: Option<u64>,
    #[serde(default)]
    frame: Option<Frame>,
    #[serde(default)]
    activate: bool,
}
pub(crate) fn create(s: &mut Session, params: &Value) -> Result<Value> {
    crate::project_owner::writable(s)?;
    bounded(params, CREATE)?;
    let r: Create = serde_json::from_value(params.clone()).map_err(|e| bad(CREATE, e.to_string()))?;
    if r.name.len() > 1024 || r.name.encode_utf16().count() > 256 || s.project.sequences().take(64).count() >= 64 {
        return Err(bad(CREATE, "need bounded name and fewer than 64 sequences"));
    }
    let source = match (r.from, r.frame.as_ref()) {
        (Some(from), None) => {
            let item = s
                .project
                .item(ItemId(from))
                .filter(|item| matches!(item.kind, ItemKind::Sequence(_)))
                .ok_or_else(|| bad(CREATE, "from must identify an existing current sequence"))?;
            Some(item.clone())
        }
        (None, Some(_)) => None,
        _ => return Err(bad(CREATE, "provide exactly one of frame or current from sequence")),
    };
    let settings = if let Some(frame) = r.frame {
        let mut settings = SequenceSettings { width: frame.width, height: frame.height, frame_rate: frame.rate, ..Default::default() };
        settings.validate().map_err(|e| bad(CREATE, e))?;
        settings.preset = format!("{}x{} {}", settings.width, settings.height, settings.frame_rate.label());
        Some(settings)
    } else {
        None
    };
    // Project's shared allocator assumes a validated counter. Refuse overflow
    // and item replacement before entering it; no hostile input can wrap an id.
    let needed = if source.is_some() { 1 } else { 4 };
    if s.project.next_id.checked_add(needed).is_none() || s.project.items.keys().next_back().is_some_and(|id| s.project.next_id <= id.0) {
        return Err(bad(CREATE, "native id allocator has no safe capacity"));
    }
    let id = s.edit("Create Poema sequence", move |project, state| {
        let id = if let Some(source) = source {
            let id = project.add_item(&r.name, source.label, source.kind, None);
            let item = project.item_mut(id).ok_or_else(|| bad(CREATE, "created item is missing"))?;
            item.metadata = source.metadata;
            item.split = source.split;
            id
        } else {
            let settings = settings.ok_or_else(|| bad(CREATE, "empty sequence needs settings"))?;
            let id = project.new_sequence(&r.name, settings, 1, 1, None);
            let captions = TrackId(project.alloc_id());
            let q = project.sequence_mut(id).ok_or_else(|| bad(CREATE, "created sequence is missing"))?;
            q.program_extent = Some(ProgramExtentPolicy::Poema);
            if let Some(t) = q.video_tracks.first_mut() {
                t.name = "V1".into();
            }
            if let Some(t) = q.audio_tracks.first_mut() {
                t.name = "A1".into();
                t.mixer.stereo_balance = StereoBalanceLaw::Linear;
                t.mixer.mono_source = MonoSourceLaw::EqualPower;
            }
            q.caption_tracks.push(CaptionTrack::new(captions, "Captions".into(), CaptionFormat::default()));
            id
        };
        project.sequence(id).ok_or_else(|| bad(CREATE, "created sequence is missing"))?.check().map_err(|e| bad(CREATE, e))?;
        if r.activate {
            state.active_sequence = Some(id);
            if !state.open_sequences.contains(&id) {
                state.open_sequences.push(id);
            }
            state.selection.clear();
            state.caption_selection.clear();
        }
        Ok(id)
    })?;
    Ok(json!({"sequence":id.0}))
}

pub(crate) const RECORD_KEY: &str = "poema.sequenceRecord.v1";
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RecordRequest {
    sequence: u64,
    project_id: String,
    sequence_id: String,
    props: serde_json::Map<String, Value>,
    #[serde(default)]
    name: Option<String>,
    /// The legacy document title is distinct from its sequence display name.
    /// Presentation only: numeric cut state stays in the native sequence.
    #[serde(default)]
    document_title: Option<String>,
}
fn identifier(value: &str) -> bool {
    !value.is_empty() && value.len() <= 160 && value.bytes().all(|b| b.is_ascii_alphanumeric() || b"_.:-".contains(&b))
}
pub(crate) fn record(s: &mut Session, params: &Value) -> Result<Value> {
    const CMD: &str = "poema.setSequenceRecord";
    crate::project_owner::writable(s)?;
    bounded(params, CMD)?;
    let r: RecordRequest = serde_json::from_value(params.clone()).map_err(|e| bad(CMD, e.to_string()))?;
    if !identifier(&r.project_id)
        || !identifier(&r.sequence_id)
        || r.props.len() > 64
        || r.name.as_ref().is_some_and(|n| n.len() > 1024 || n.encode_utf16().count() > 256)
        || r.document_title.as_ref().is_some_and(|n| n.len() > 1024 || n.encode_utf16().count() > 256)
    {
        return Err(bad(CMD, "invalid or excessive sequence record"));
    }
    for key in ["id", "name", "currentVersion", "timeline", "tracks", "clips", "settings", "duration", "versions"] {
        if r.props.contains_key(key) {
            return Err(bad(CMD, "native name/version/geometry cannot be stored in sequence record annotations"));
        }
    }
    if r.props.get("held_finish").is_some_and(|v| !v.is_null()) {
        return Err(bad(CMD, "held versions need their persistent native representation before publication"));
    }
    let id = ItemId(r.sequence);
    let item = s.project.item(id).filter(|item| matches!(item.kind, ItemKind::Sequence(_))).ok_or_else(|| bad(CMD, "target is not a sequence"))?;
    let editorial_text = item.metadata.get(crate::poema_metadata::KEY).ok_or_else(|| bad(CMD, "initialize native editorial aliases first"))?;
    if editorial_text.len() > 65536 {
        return Err(bad(CMD, "editorial aliases exceed their bounded size"));
    }
    let editorial: Value = serde_json::from_str(editorial_text).map_err(|e| bad(CMD, e.to_string()))?;
    if editorial.get("project_id").and_then(Value::as_str) != Some(r.project_id.as_str())
        || editorial.get("sequence_id").and_then(Value::as_str) != Some(r.sequence_id.as_str())
    {
        return Err(bad(CMD, "sequence record must match its native editorial identity"));
    }
    let titled = editorial.get("fields").and_then(Value::as_array).is_some_and(|fields| fields.iter().any(|field| field.as_str() == Some("name")));
    if titled != r.document_title.is_some() {
        return Err(bad(CMD, "documentTitle must match the declared document presentation fields"));
    }
    let mut record = json!({"schema":1,"project_id":r.project_id,"sequence_id":r.sequence_id,"props":r.props});
    if let Some(title) = r.document_title {
        record["document_title"] = json!(title);
    }
    let text = serde_json::to_string(&record).map_err(|e| bad(CMD, e.to_string()))?;
    if text.len() > 32768 {
        return Err(bad(CMD, "sequence record exceeds 32 KiB"));
    }
    if item.metadata.get(RECORD_KEY) == Some(&text) && r.name.as_ref().is_none_or(|name| item.name == *name) {
        return Ok(json!({"changed":false}));
    }
    s.edit("Set Poema sequence record", move |project, _| {
        let item = project.item_mut(id).ok_or_else(|| bad(CMD, "target sequence disappeared"))?;
        item.metadata.insert(RECORD_KEY.into(), text);
        if let Some(name) = r.name {
            item.name = name;
        }
        Ok(())
    })?;
    Ok(json!({"changed":true}))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Delete {
    sequence: u64,
    #[serde(default)]
    fallback: Option<u64>,
}
pub(crate) fn delete(s: &mut Session, params: &Value) -> Result<Value> {
    const CMD: &str = "poema.deleteSequence";
    crate::project_owner::writable(s)?;
    bounded(params, CMD)?;
    let r: Delete = serde_json::from_value(params.clone()).map_err(|e| bad(CMD, e.to_string()))?;
    let id = ItemId(r.sequence);
    let item = s.project.item(id).filter(|item| matches!(item.kind, ItemKind::Sequence(_))).ok_or_else(|| bad(CMD, "target is not a sequence"))?;
    if s.project.sequences().take(2).count() < 2 {
        return Err(bad(CMD, "a project keeps at least one sequence"));
    }
    if let Some(text) = item.metadata.get(RECORD_KEY) {
        if text.len() > 32768 {
            return Err(bad(CMD, "invalid sequence record"));
        }
        let record: Value = serde_json::from_str(text).map_err(|e| bad(CMD, e.to_string()))?;
        if record.get("props").and_then(|v| v.get("legacy")).and_then(Value::as_bool) == Some(true) {
            return Err(bad(CMD, "the original chain cannot be deleted"));
        }
    }
    if s.project.items.values().any(|item| matches!(item.kind, ItemKind::Subclip { parent, .. } if parent == id))
        || s.project
            .sequences()
            .filter(|i| i.id != id)
            .filter_map(|i| i.as_sequence())
            .any(|q| q.all_tracks().any(|t| t.items.iter().any(|clip| clip.item == id)))
    {
        return Err(bad(CMD, "sequence is nested in another cut"));
    }
    let fallback = r.fallback.map(ItemId);
    if fallback.is_some_and(|v| v == id || s.project.sequence(v).is_none()) {
        return Err(bad(CMD, "fallback must be another existing sequence"));
    }
    if s.state.active_sequence == Some(id) && fallback.is_none() {
        return Err(bad(CMD, "deleting the active sequence needs an explicit fallback"));
    }
    s.edit("Delete Poema sequence", move |project, state| {
        project.items.remove(&id);
        project.transcripts.remove(&id);
        project.source_graphics.remove(&id);
        project.narrations.remove(&id);
        project.root.remove_item(id);
        state.open_sequences.retain(|v| *v != id);
        state.timeline_views.remove(&id);
        state.playheads.remove(&id);
        state.project_selection.retain(|v| *v != id);
        if state.source_item == Some(id) {
            state.source_item = None;
        }
        if state.active_sequence == Some(id) {
            state.active_sequence = fallback;
            if let Some(next) = fallback
                && !state.open_sequences.contains(&next)
            {
                state.open_sequences.push(next);
            }
            state.selection.clear();
            state.caption_selection.clear();
        }
        Ok(())
    })?;
    Ok(json!({"deleted":id.0}))
}
