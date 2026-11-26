//! Exact caption data import, shared by human file import and guarded adapters.
use crate::{
    EngineError, Result, Session,
    commands::{CommandSpec, always, bad},
};
use filmcraft_project::{Caption, CaptionFormat, CaptionStyle, ClipId, ItemId, Project, TrackId};
use filmcraft_time::Tick;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

pub(crate) const ARCHIVE: &str = "filmcraft.captionImports.v1";
const COMMAND: &str = "captions.importData";
fn error(message: impl Into<String>) -> EngineError {
    bad(COMMAND, message)
}
fn yes() -> bool {
    true
}
fn text(s: &str, max: usize) -> bool {
    s.len() <= max && !s.contains('\0')
}
fn sha<T: Serialize>(value: &T) -> Result<String> {
    Ok(format!("{:x}", Sha256::digest(serde_json::to_vec(value).map_err(|e| error(e.to_string()))?)))
}
fn caption_inventory(tracks: &[filmcraft_project::CaptionTrack]) -> Result<()> {
    let mut budget = 4 * 1024 * 1024usize;
    let mut count = 0usize;
    if tracks.len() > 32 {
        return Err(error("caption track inventory exceeds 32"));
    }
    for t in tracks {
        if !text(&t.name, 1024) || !text(&t.language, 128) || !crate::captions::valid_style(&t.style) || t.captions.len() > 100_000 || t.vtt_blocks.len() > 1024
        {
            return Err(error("existing caption state exceeds import bounds"));
        }
        budget = budget.checked_sub(8192).ok_or_else(|| error("complete caption inventory exceeds 4 MiB"))?;
        for b in &t.vtt_blocks {
            if !text(b, 65536) {
                return Err(error("caption block exceeds bound"));
            }
            budget = budget.checked_sub(b.len().saturating_mul(6).saturating_add(32)).ok_or_else(|| error("complete caption inventory exceeds 4 MiB"))?;
        }
        for c in &t.captions {
            count = count.saturating_add(1);
            if count > 100_000
                || !text(&c.text, 8192)
                || !text(&c.settings, 2048)
                || c.speaker.as_ref().is_some_and(|s| !text(s, 1024))
                || c.cue_id.as_ref().is_some_and(|s| !text(s, 1024))
            {
                return Err(error("caption cue exceeds import bounds"));
            }
            let bytes = c
                .text
                .len()
                .saturating_add(c.settings.len())
                .saturating_add(c.speaker.as_ref().map_or(0, String::len))
                .saturating_add(c.cue_id.as_ref().map_or(0, String::len));
            budget = budget.checked_sub(bytes.saturating_mul(6).saturating_add(512)).ok_or_else(|| error("complete caption inventory exceeds 4 MiB"))?;
        }
        t.check().map_err(error)?;
    }
    Ok(())
}
use crate::project_ids::allocation_start;

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct CueData {
    pub start: i64,
    pub end: i64,
    pub text: String,
    #[serde(default)]
    pub speaker: Option<String>,
    #[serde(default)]
    pub cue_id: Option<String>,
    #[serde(default)]
    pub settings: String,
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct TrackData {
    pub name: String,
    pub language: String,
    pub format: String,
    #[serde(default = "yes")]
    pub enabled: bool,
    #[serde(default)]
    pub locked: bool,
    #[serde(default = "yes")]
    pub sync_lock: bool,
    #[serde(default)]
    pub style: Option<CaptionStyle>,
    #[serde(default)]
    pub blocks: Vec<String>,
    pub cues: Vec<CueData>,
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ImportData {
    pub sequence: u64,
    #[serde(default)]
    pub source_document: Option<String>,
    pub tracks: Vec<TrackData>,
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ImportedTrack {
    pub track: u64,
    pub captions: Vec<u64>,
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ImportRecord {
    pub source_document: String,
    pub caption_sha256: String,
    pub tracks: Vec<ImportedTrack>,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ImportReceipt {
    pub sequence: u64,
    pub tracks: Vec<ImportedTrack>,
    pub captions: usize,
    pub source_sha256: Option<String>,
    pub after_caption_sha256: String,
    pub after_archive_sha256: Option<String>,
}
impl ImportData {
    pub(crate) fn validate(&self) -> Result<()> {
        if self.sequence == 0 || self.tracks.is_empty() || self.tracks.len() > 32 {
            return Err(error("explicit sequence and 1–32 caption tracks required"));
        }
        let mut budget = 4 * 1024 * 1024usize;
        let mut count = 0usize;
        if let Some(source) = &self.source_document
            && (source.is_empty() || !text(source, 512 * 1024))
        {
            return Err(error("bounded UTF-8 source document required"));
        }
        for t in &self.tracks {
            if !text(&t.name, 1024)
                || t.name.trim().is_empty()
                || !text(&t.language, 128)
                || !text(&t.format, 128)
                || CaptionFormat::from_name(&t.format).is_none()
                || t.style.as_ref().is_some_and(|s| !crate::captions::valid_style(s))
                || t.cues.len() > 100_000
                || t.blocks.len() > 1024
            {
                return Err(error("invalid caption track metadata"));
            }
            budget = budget.checked_sub(8192).ok_or_else(|| error("caption import exceeds 4 MiB budget"))?;
            for b in &t.blocks {
                if !text(b, 65536) {
                    return Err(error("invalid caption header block"));
                }
                budget = budget.checked_sub(b.len().saturating_mul(6).saturating_add(32)).ok_or_else(|| error("caption blocks exceed budget"))?;
            }
            let mut prior_end = None;
            for c in &t.cues {
                count = count.saturating_add(1);
                if count > 100_000
                    || c.end <= c.start
                    || c.end.checked_sub(c.start).is_none()
                    || !text(&c.text, 8192)
                    || !text(&c.settings, 2048)
                    || c.speaker.as_ref().is_some_and(|s| !text(s, 1024))
                    || c.cue_id.as_ref().is_some_and(|s| !text(s, 1024))
                    || prior_end.is_some_and(|end| end > c.start)
                {
                    return Err(error("cues need exact ordered nonoverlapping tick ranges, positive durations and bounded metadata"));
                }
                let bytes = c
                    .text
                    .len()
                    .saturating_add(c.settings.len())
                    .saturating_add(c.speaker.as_ref().map_or(0, String::len))
                    .saturating_add(c.cue_id.as_ref().map_or(0, String::len));
                budget = budget.checked_sub(bytes.saturating_mul(6).saturating_add(512)).ok_or_else(|| error("caption cues exceed 4 MiB budget"))?;
                prior_end = Some(c.end);
            }
        }
        Ok(())
    }
}
pub(crate) fn parse(value: &Value) -> Result<ImportData> {
    // CaptionStyle's project serde defaults are permissive; explicit import styles are complete.
    let style_keys = ["font", "size", "color", "background", "background_color", "align", "anchor", "margin", "line_spacing", "outline", "outline_color"];
    if let Some(tracks) = value.get("tracks").and_then(Value::as_array) {
        for t in tracks {
            if let Some(style) = t.get("style").filter(|s| !s.is_null())
                && !style.as_object().is_some_and(|o| o.len() == style_keys.len() && style_keys.iter().all(|k| o.contains_key(*k)))
            {
                return Err(error("explicit style must contain all native style fields"));
            }
        }
    }
    let data: ImportData = serde_json::from_value(value.clone()).map_err(|e| error(e.to_string()))?;
    data.validate()?;
    Ok(data)
}

/// Caller uses a Session edit or a private staged Project. Publication stays with that owner.
pub(crate) fn append(project: &mut Project, data: &ImportData) -> Result<ImportReceipt> {
    data.validate()?;
    let sid = ItemId(data.sequence);
    if project.item(sid).and_then(|i| i.metadata.get(ARCHIVE)).is_some_and(|raw| raw.len() > 4 * 1024 * 1024) {
        return Err(error("caption source archive exceeds 4 MiB"));
    }
    let current = project.sequence(sid).ok_or(crate::EngineError::NoSequence)?;
    caption_inventory(&current.caption_tracks)?;
    let mut sequence = current.clone();
    if sequence.caption_tracks.len().saturating_add(data.tracks.len()) > 32 {
        return Err(error("combined caption track count exceeds 32"));
    }
    let needed = data
        .tracks
        .iter()
        .try_fold(0u64, |n, t| n.checked_add(1).and_then(|n| n.checked_add(t.cues.len() as u64)))
        .ok_or_else(|| error("caption ID count overflow"))?;
    let mut next = allocation_start(project, needed)?;
    let mut tracks = Vec::new();
    let mut mappings = Vec::new();
    let mut count = 0usize;
    for input in &data.tracks {
        let tid = next;
        next = next.checked_add(1).ok_or_else(|| error("caption ID space exhausted"))?;
        let mut track = crate::captions::new_track(TrackId(tid), &json!({"name":input.name,"language":input.language,"format":input.format}));
        track.enabled = input.enabled;
        track.locked = input.locked;
        track.sync_lock = input.sync_lock;
        track.vtt_blocks = input.blocks.clone();
        if let Some(style) = &input.style {
            track.style = style.clone();
        }
        let mut ids = Vec::new();
        for c in &input.cues {
            let id = next;
            next = next.checked_add(1).ok_or_else(|| error("caption ID space exhausted"))?;
            track.captions.push(Caption {
                id: ClipId(id),
                start: Tick(c.start),
                duration: Tick(c.end.checked_sub(c.start).ok_or_else(|| error("caption duration overflow"))?),
                text: c.text.clone(),
                speaker: c.speaker.clone(),
                cue_id: c.cue_id.clone(),
                settings: c.settings.clone(),
            });
            ids.push(id);
            count = count.saturating_add(1);
        }
        track.check().map_err(error)?;
        mappings.push(ImportedTrack { track: tid, captions: ids });
        tracks.push(track);
    }
    let source_sha = data.source_document.as_ref().map(|s| format!("{:x}", Sha256::digest(s.as_bytes())));
    let archive = if let Some(source) = &data.source_document {
        let raw = project.item(sid).and_then(|i| i.metadata.get(ARCHIVE));
        if raw.is_some_and(|r| r.len() > 4 * 1024 * 1024) {
            return Err(error("caption source archive exceeds 4 MiB"));
        }
        let mut records: BTreeMap<String, ImportRecord> =
            raw.map(|r| serde_json::from_str(r).map_err(|e| error(format!("invalid caption archive: {e}")))).transpose()?.unwrap_or_default();
        if let Some(raw) = raw
            && serde_json::to_string(&records).map_err(|e| error(e.to_string()))? != *raw
        {
            return Err(error("caption archive is noncanonical or has duplicate keys; original metadata preserved"));
        }
        let valid_hash = |s: &str| s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase());
        let mut ids = 0usize;
        for (key, r) in &records {
            if !valid_hash(key)
                || !valid_hash(&r.caption_sha256)
                || !text(&r.source_document, 512 * 1024)
                || format!("{:x}", Sha256::digest(r.source_document.as_bytes())) != *key
                || r.tracks.is_empty()
                || r.tracks.len() > 32
            {
                return Err(error("caption archive identity or mapping is invalid"));
            }
            let mut seen = std::collections::BTreeSet::new();
            for t in &r.tracks {
                ids = ids.saturating_add(t.captions.len()).saturating_add(1);
                if ids > 100_000 || t.track == 0 || !seen.insert(t.track) || t.captions.iter().any(|id| *id == 0 || !seen.insert(*id)) {
                    return Err(error("caption archive IDs exceed bounds or repeat"));
                }
            }
        }
        let key = source_sha.as_ref().ok_or_else(|| error("missing source identity"))?;
        if records.contains_key(key) {
            return Err(error("this source document was already imported; no duplicate tracks created"));
        }
        if records.len() >= 16 {
            return Err(error("caption source archive exceeds 16 imports"));
        }
        records.insert(key.clone(), ImportRecord { source_document: source.clone(), caption_sha256: sha(&tracks)?, tracks: mappings.clone() });
        let encoded = serde_json::to_string(&records).map_err(|e| error(e.to_string()))?;
        if encoded.len() > 4 * 1024 * 1024 {
            return Err(error("caption source archive exceeds 4 MiB"));
        }
        Some(encoded)
    } else {
        None
    };
    sequence.caption_tracks.splice(0..0, tracks);
    caption_inventory(&sequence.caption_tracks)?;
    sequence.check().map_err(error)?;
    let after = sha(&sequence.caption_tracks)?;
    let item = project.item_mut(sid).ok_or(crate::EngineError::NoSequence)?;
    *item.as_sequence_mut().ok_or(crate::EngineError::NoSequence)? = sequence;
    if let Some(encoded) = archive {
        item.metadata.insert(ARCHIVE.into(), encoded);
    }
    let after_archive_sha256 = item.metadata.get(ARCHIVE).map(|raw| format!("{:x}", Sha256::digest(raw.as_bytes())));
    project.next_id = next;
    Ok(ImportReceipt {
        sequence: data.sequence,
        tracks: mappings,
        captions: count,
        source_sha256: source_sha,
        after_caption_sha256: after,
        after_archive_sha256,
    })
}
fn run(s: &mut Session, value: &Value) -> Result<Value> {
    let data = parse(value)?;
    let result = s.edit("Import Exact Caption Data", |p, _state| append(p, &data))?;
    serde_json::to_value(result).map_err(|e| error(e.to_string()))
}
pub(crate) fn command() -> CommandSpec {
    CommandSpec {
        id: COMMAND,
        label: "Import Exact Caption Data",
        menu: &[],
        shortcut: None,
        params: r#"{"sequence":id,"sourceDocument":UTF8? (retained original, duplicate SHA refuses),"tracks":[{"name":str,"language":str,"format":str,"cues":[{"start":ticks,"end":ticks,"text":str,"speaker":str|null?,"cueId":str|null?,"settings":str?}]}]}"#,
        enabled: always,
        run,
        journal: true,
    }
}

#[cfg(test)]
#[path = "caption_import_tests.rs"]
mod tests;
