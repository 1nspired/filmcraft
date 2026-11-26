//! Exact companion publication through the shared human caption import service.
//! Destination is explicit and may be inactive. Read tickets still pin the full
//! document/editor context and original files; no navigation or recipe replay.
use super::{BINDING_KEY, Binding, Operation, captions, condense, parse, project_audit, sequences};
use crate::{Result, Session, caption_import, commands::bad};
use filmcraft_project::{ItemId, TrackId};
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};

pub(super) const COMMAND: &str = "agent.importCaptions";
const READ: &str = "agent.readCaptionDestination";
const SAFE: i64 = 9_007_199_254_740_991;
fn error(s: impl Into<String>) -> crate::EngineError {
    bad(COMMAND, s)
}
fn hash(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
}
fn sha<T: serde::Serialize>(v: &T) -> Result<String> {
    Ok(format!("{:x}", Sha256::digest(serde_json::to_vec(v).map_err(|e| error(e.to_string()))?)))
}
fn archive_sha(s: &Session, id: ItemId) -> Result<Option<String>> {
    let raw = s.project.item(id).and_then(|i| i.metadata.get(caption_import::ARCHIVE));
    if raw.is_some_and(|r| r.len() > 4 * 1024 * 1024) {
        return Err(error("caption archive exceeds 4 MiB"));
    }
    Ok(raw.map(|r| format!("{:x}", Sha256::digest(r.as_bytes()))))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Read {
    token: String,
    scope: sequences::Scope,
    sequence: u64,
    #[serde(default)]
    projection: bool,
    #[serde(default)]
    source_kind: Option<String>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Request {
    scope: sequences::Scope,
    before_layout_sha256: String,
    before_caption_sha256: String,
    before_archive_sha256: Option<String>,
    source_sha256: String,
    data: caption_import::ImportData,
    #[serde(default)]
    source_kind: Option<String>,
    #[serde(default)]
    replace_track: Option<u64>,
}
fn current_mix(kind: &Option<String>) -> Result<bool> {
    match kind.as_deref() {
        None | Some("original_companion") => Ok(false),
        Some("current_mix") => Ok(true),
        _ => Err(error("unsupported caption source kind")),
    }
}
fn current_basis(s: &Session, id: ItemId) -> Result<String> {
    // Caption publication may change only this sequence's captions/archive and
    // ID allocation. All current source, nesting, picture, audio and processing
    // stay in the complete project basis, without original-component assumptions.
    let mut project = (*s.project).clone();
    project.sequence_mut(id).ok_or(crate::EngineError::NoSequence)?.caption_tracks.clear();
    project.item_mut(id).ok_or(crate::EngineError::NoSequence)?.metadata.remove(caption_import::ARCHIVE);
    project.next_id = 0;
    sha(&project)
}
fn target(s: &Session, scope: &sequences::Scope, sequence: u64) -> Result<ItemId> {
    let active = s.state.active_sequence.ok_or(crate::EngineError::NoSequence)?;
    let target = ItemId(sequence);
    if !(1..=SAFE as u64).contains(&sequence) || !sequences::belongs(&s.project, active, scope)? || !sequences::belongs(&s.project, target, scope)? {
        return Err(error("explicit caption destination and active sequence must belong to this workspace"));
    }
    Ok(target)
}
fn basis(s: &Session, scope: &sequences::Scope, sequence: u64, current: bool) -> Result<(String, String)> {
    project_audit::bytes(&s.project)?;
    let id = target(s, scope, sequence)?;
    let q = s.project.sequence(id).ok_or(crate::EngineError::NoSequence)?;
    q.settings.validate().map_err(error)?;
    q.check().map_err(error)?;
    if current {
        if s.state.active_sequence != Some(id) || q.duration().0 > SAFE {
            return Err(error("current mixed captions require the active bounded workspace sequence"));
        }
        return Ok((current_basis(s, id)?, captions::digest(&q.caption_tracks)?));
    }
    if q.video_tracks.len() > 16 || q.audio_tracks.len() > 16 || q.duration().0 > SAFE {
        return Err(error("caption destination exceeds exact transfer bounds"));
    }
    if s.project.items.len() > 100_000 {
        return Err(error("caption source inventory exceeds transfer bounds"));
    }
    let mut original_counts = BTreeMap::new();
    for item in s.project.items.values().filter(|i| i.as_media().is_some()) {
        if let Some(raw) = item.metadata.get(BINDING_KEY) {
            let b: Binding = serde_json::from_str(raw).map_err(|e| error(e.to_string()))?;
            if b.project_id == scope.project_id && b.role == "original" {
                *original_counts.entry(b.asset_id).or_insert(0usize) += 1;
            }
        }
    }
    let mut count = 0usize;
    let mut assets = BTreeMap::new();
    let mut sources = BTreeMap::new();
    for t in q.all_tracks() {
        if t.items.len() > 200 {
            return Err(error("complete caption destination clips exceed transfer bounds"));
        }
        for c in &t.items {
            count = count.saturating_add(1);
            if count > 200
                || !condense::forward_clock(c)
                || [c.start.0, c.source_in.0].iter().any(|n| !(0..=SAFE).contains(n))
                || !(1..=SAFE).contains(&c.duration.0)
                || c.start.0.checked_add(c.duration.0).is_none_or(|n| n > SAFE)
                || c.source_in.0.checked_add(c.duration.0).is_none_or(|n| n > SAFE)
            {
                return Err(error("caption transfer requires complete bounded forward 1x native source clocks"));
            }
            let source = s.project.item(c.item).ok_or_else(|| error("caption source is missing"))?;
            let media = source.as_media().ok_or_else(|| error("caption transfer needs direct original media, not nested graphics"))?;
            let raw = source.metadata.get(BINDING_KEY).ok_or_else(|| error("caption source lacks its explicit original binding"))?;
            let b: Binding = serde_json::from_str(raw).map_err(|e| error(e.to_string()))?;
            if b.project_id != scope.project_id
                || b.role != "original"
                || b.asset_id.is_empty()
                || b.media_id.is_empty()
                || original_counts.get(&b.asset_id) != Some(&1)
                || media.offline
                || c.source_in.0.checked_add(c.duration.0).is_none_or(|n| n > media.info.duration.0)
            {
                return Err(error("caption source binding or native duration differs"));
            }
            if assets.insert(b.asset_id, c.item).is_some_and(|old| old != c.item) {
                return Err(error("caption source correspondence is ambiguous"));
            }
            sources.insert(c.item.0, source);
        }
    }
    let caption = captions::digest(&q.caption_tracks)?;
    // Bound above before constructing either full projection. Authored NaN
    // automatic points serialize exactly as in the existing project audit.
    let mut layout = serde_json::to_value(q).map_err(|e| error(e.to_string()))?;
    layout.as_object_mut().ok_or_else(|| error("invalid native sequence layout"))?.remove("caption_tracks");
    Ok((sha(&(layout, sources))?, caption))
}
pub(super) fn read(s: &mut Session, value: &Value) -> Result<Value> {
    let r: Read = parse(READ, value)?;
    super::validate_metadata_read(s, &r.token, r.projection, READ)?;
    let current = current_mix(&r.source_kind)?;
    let (layout, caption) = basis(s, &r.scope, r.sequence, current)?;
    let id = ItemId(r.sequence);
    let ticket = super::validate_read_basis(s, &r.token)?;
    super::media_scope::require_sequence(&s.project, &ticket.media, id)?;
    let q = s.project.sequence(id).ok_or(crate::EngineError::NoSequence)?;
    let mut result = json!({"status":"read","token":r.token,"scope":r.scope,"revision":s.revision,"sequence":crate::commands::inspect_sequence(s,id,q),
        "layoutSha256":layout,"captionSha256":caption,"archiveSha256":archive_sha(s,id)?,"nativeForwardClocks":true});
    if current {
        let object = result.as_object_mut().ok_or_else(|| error("invalid caption projection"))?;
        object.remove("nativeForwardClocks");
        object.insert("currentMix".into(), json!(true));
        object.insert("captionTemplates".into(), json!(q.caption_tracks.iter().map(|t| json!({"id":t.id.0,"name":t.name,"language":t.language,"format":t.format.label(),"enabled":t.enabled,"locked":t.locked,"syncLock":t.sync_lock,"style":t.style,"blocks":t.vtt_blocks})).collect::<Vec<_>>()));
    }
    if r.projection {
        result["requiresSourceValidation"] = json!(true);
    }
    if serde_json::to_vec(&result).map_err(|e| error(e.to_string()))?.len() > 512 * 1024 {
        return Err(error("complete caption destination read exceeds 512 KiB"));
    }
    Ok(result)
}
pub(super) fn validate(op: &Operation) -> Result<()> {
    let r: Request = parse(COMMAND, &op.params)?;
    r.data.validate()?;
    let current = current_mix(&r.source_kind)?;
    if !current && r.replace_track.is_some()
        || current && (r.data.tracks.len() != 1 || op.params.get("replaceTrack").is_none())
        || r.replace_track.is_some_and(|n| n == 0 || n > SAFE as u64)
    {
        return Err(error("current caption creation/replacement requires one explicit track policy"));
    }
    if current {
        let source: Value = serde_json::from_str(r.data.source_document.as_deref().ok_or_else(|| error("current caption source document required"))?)
            .map_err(|e| error(e.to_string()))?;
        if source.get("kind").and_then(Value::as_str) != Some("poema-current-native-caption-source")
            || source.get("version").and_then(Value::as_u64) != Some(1)
            || source.pointer("/scope/project_id").and_then(Value::as_str) != Some(r.scope.project_id.as_str())
            || source.pointer("/scope/sequence_id").and_then(Value::as_u64) != Some(r.data.sequence)
            || source.pointer("/story/status").and_then(Value::as_str) != Some("complete")
            || source.pointer("/story/speech/producer").and_then(Value::as_str) != Some("original-file-services")
        {
            return Err(error("current mixed captions require their own complete speech source archive"));
        }
    }
    if !hash(&r.before_layout_sha256)
        || !hash(&r.before_caption_sha256)
        || op.params.get("beforeArchiveSha256").is_none()
        || r.before_archive_sha256.as_ref().is_some_and(|s| !hash(s))
        || !hash(&r.source_sha256)
        || r.data.source_document.as_ref().is_none_or(|raw| format!("{:x}", Sha256::digest(raw.as_bytes())) != r.source_sha256)
    {
        return Err(error("exact native layout/caption state and original document SHA required"));
    }
    // Preserve strict explicit CaptionStyle shapes from the human import door.
    caption_import::parse(op.params.get("data").ok_or_else(|| error("missing exact caption data"))?)?;
    Ok(())
}
pub(super) fn run(s: &mut Session, op: &Operation) -> Result<Value> {
    validate(op)?;
    let r: Request = parse(COMMAND, &op.params)?;
    let current = current_mix(&r.source_kind)?;
    let (layout, caption) = basis(s, &r.scope, r.data.sequence, current)?;
    if layout != r.before_layout_sha256 || caption != r.before_caption_sha256 {
        return Err(error("caption destination changed since preparation"));
    }
    let q = s.project.sequence(ItemId(r.data.sequence)).ok_or(crate::EngineError::NoSequence)?;
    if r.data.tracks.iter().flat_map(|t| &t.cues).any(|c| c.start < 0 || c.end > q.duration().0) {
        return Err(error("original captions must remain inside the verified destination duration"));
    }
    let before_archive = archive_sha(s, ItemId(r.data.sequence))?;
    if before_archive != r.before_archive_sha256 {
        return Err(error("caption source archive changed since preparation"));
    }
    let id = ItemId(r.data.sequence);
    let mut expected = (*s.project).clone();
    let mut editor = s.state.clone();
    let mut after_track = None;
    if let Some(replace) = r.replace_track {
        let q = expected.sequence_mut(id).ok_or(crate::EngineError::NoSequence)?;
        let index = q.caption_tracks.iter().position(|t| t.id.0 == replace).ok_or_else(|| error("caption replacement track is absent"))?;
        let old = q.caption_tracks.get(index).ok_or_else(|| error("caption replacement index changed"))?;
        let new = r.data.tracks.first().ok_or_else(|| error("caption replacement track data is absent"))?;
        if old.locked
            || new.name != old.name
            || new.language != old.language
            || new.format != old.format.label()
            || new.enabled != old.enabled
            || new.locked != old.locked
            || new.sync_lock != old.sync_lock
            || serde_json::to_value(&new.style).map_err(|e| error(e.to_string()))? != serde_json::to_value(&old.style).map_err(|e| error(e.to_string()))?
            || new.blocks != old.vtt_blocks
        {
            return Err(error("caption replacement must preserve the chosen unlocked track's complete style and settings"));
        }
        after_track = index.checked_sub(1).and_then(|n| q.caption_tracks.get(n)).map(|t| t.id);
        crate::track_ops::remove_inventory(q, &mut editor, id, &BTreeSet::from([TrackId(replace)]));
    }
    let mut predicted = serde_json::to_value(caption_import::append(&mut expected, &r.data)?).map_err(|e| error(e.to_string()))?;
    if expected.next_id > SAFE as u64 {
        return Err(error("caption IDs exceed exact agent transport"));
    }
    let data = op.params.get("data").cloned().ok_or_else(|| error("missing exact caption data"))?;
    if let Some(replace) = r.replace_track {
        s.execute("sequence.removeTracks", json!({"tracks":[replace]}))?;
    }
    let receipt = s.execute("captions.importData", data)?;
    if receipt != predicted {
        return Err(error("native caption import receipt differs; staged result discarded"));
    }
    if r.replace_track.is_some() && after_track.is_some() {
        let track = predicted.pointer("/tracks/0/track").and_then(Value::as_u64).ok_or_else(|| error("caption replacement omitted its new track"))?;
        let q = expected.sequence_mut(id).ok_or(crate::EngineError::NoSequence)?;
        crate::track_ops::reorder_inventory(q, TrackId(track), after_track, COMMAND)?;
        s.execute("sequence.reorderTrack", json!({"track":track,"after":after_track.map(|t|t.0)}))?;
        predicted["afterCaptionSha256"] = json!(captions::digest(&q.caption_tracks)?);
    }
    if project_audit::bytes(&s.project)? != project_audit::bytes(&expected)? || project_audit::value_bytes(&s.state)? != project_audit::value_bytes(&editor)? {
        return Err(error("native caption import changed unexpected state; staged result discarded"));
    }

    let mut result = json!({"humanCommand":"captions.importData","beforeLayoutSha256":layout,"beforeCaptionSha256":caption,"beforeArchiveSha256":before_archive,"import":predicted});
    if current {
        result["sourceKind"] = json!("current_mix");
        result["replacedTrack"] = json!(r.replace_track);
        result["humanCommands"] = if r.replace_track.is_some() && after_track.is_some() {
            json!(["sequence.removeTracks", "captions.importData", "sequence.reorderTrack"])
        } else if r.replace_track.is_some() {
            json!(["sequence.removeTracks", "captions.importData"])
        } else {
            json!(["captions.importData"])
        };
    }
    Ok(result)
}
