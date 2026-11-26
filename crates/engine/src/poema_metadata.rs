//! Editorial aliases/annotations live in the native project and its undo.
//! These cannot replace native clip geometry, playback state or track topology.
use crate::{Result, Session, commands::bad};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeMap;

pub(crate) const KEY: &str = "poema.editorial.v1";
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct TrackMetadata {
    id: String,
    fields: Vec<String>,
    props: BTreeMap<String, Value>,
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Metadata {
    schema: u32,
    project_id: String,
    sequence_id: String,
    root: BTreeMap<String, Value>,
    settings: Value,
    fields: Vec<String>,
    tracks: BTreeMap<String, TrackMetadata>,
    assets: BTreeMap<String, String>,
    #[serde(default)]
    links: BTreeMap<String, Value>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    metadata: Metadata,
}
fn identifier(value: &str) -> bool {
    !value.is_empty() && value.len() <= 160 && value.bytes().all(|b| b.is_ascii_alphanumeric() || b"_.:-".contains(&b))
}
pub(crate) fn set(s: &mut Session, params: &Value) -> Result<Value> {
    let request: Request = serde_json::from_value(params.clone()).map_err(|e| bad("poema.setSequenceMetadata", e.to_string()))?;
    let meta = request.metadata;
    if meta.schema != 1
        || !identifier(&meta.project_id)
        || !identifier(&meta.sequence_id)
        || meta.fields.len() > 128
        || meta.tracks.len() > 256
        || meta.assets.len() > 4096
        || meta.links.len() > 4096
    {
        return Err(bad("poema.setSequenceMetadata", "invalid or excessive editorial metadata"));
    }
    for key in ["tracks", "settings", "duration", "link_groups", "chapter_markers"] {
        if meta.root.contains_key(key) {
            return Err(bad("poema.setSequenceMetadata", "native geometry/topology cannot be stored in root annotations"));
        }
    }
    let sequence = s.state.active_sequence.ok_or_else(|| bad("poema.setSequenceMetadata", "no active sequence"))?;
    let q = s.active_sequence().ok_or_else(|| bad("poema.setSequenceMetadata", "no active sequence"))?;
    let frame = meta.settings.get("frame").filter(|v| !v.is_null()).unwrap_or(&meta.settings);
    let width = frame.get("width").and_then(Value::as_u64);
    let height = frame.get("height").and_then(Value::as_u64);
    let fps = meta.settings.get("fps").and_then(Value::as_f64);
    if width != Some(u64::from(q.settings.width))
        || height != Some(u64::from(q.settings.height))
        || !fps.is_some_and(|f| f.is_finite() && (f - (q.settings.frame_rate.num as f64) / (q.settings.frame_rate.den as f64)).abs() < 0.001)
    {
        return Err(bad("poema.setSequenceMetadata", "legacy frame representation must match native sequence settings"));
    }
    let mut aliases = std::collections::BTreeSet::new();
    for (key, track) in &meta.tracks {
        let id = key.parse::<u64>().map_err(|_| bad("poema.setSequenceMetadata", "invalid native track id"))?;
        if !identifier(&track.id)
            || !aliases.insert(&track.id)
            || !(q.video_tracks.iter().chain(&q.audio_tracks).any(|t| t.id.0 == id) || q.caption_tracks.iter().any(|t| t.id.0 == id))
            || track.fields.len() > 128
        {
            return Err(bad("poema.setSequenceMetadata", "invalid/duplicate track alias"));
        }
        for key in ["id", "kind", "clips", "items", "gain_db", "muted", "solo", "locked", "pan", "gain_curve", "pan_curve"] {
            if track.props.contains_key(key) {
                return Err(bad("poema.setSequenceMetadata", "native track state cannot be stored in annotations"));
            }
        }
    }
    for (key, alias) in &meta.assets {
        let id = key.parse::<u64>().map_err(|_| bad("poema.setSequenceMetadata", "invalid native item id"))?;
        if !identifier(alias) || s.project.item(filmcraft_project::ItemId(id)).is_none() {
            return Err(bad("poema.setSequenceMetadata", "unknown native asset or invalid alias"));
        }
    }
    let value = serde_json::to_string(&meta).map_err(|e| bad("poema.setSequenceMetadata", e.to_string()))?;
    if value.len() > 64 * 1024 {
        return Err(bad("poema.setSequenceMetadata", "editorial metadata exceeds 64 KiB"));
    }
    let current = s.project.item(sequence).ok_or_else(|| bad("poema.setSequenceMetadata", "sequence missing"))?;
    if current.metadata.get(KEY) == Some(&value) {
        return Ok(json!({"changed":false}));
    }
    s.edit("Poema editorial metadata", |project, _| {
        let item = project.item_mut(sequence).ok_or_else(|| bad("poema.setSequenceMetadata", "sequence missing"))?;
        item.metadata.insert(KEY.to_owned(), value);
        Ok(())
    })?;
    Ok(json!({"changed":true}))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn demo() -> Session {
        let mut s = Session::default();
        let (p, q) = crate::demo::demo_project(&s.media);
        s.project = std::sync::Arc::new(p);
        s.state.active_sequence = Some(q);
        s
    }
    fn request(s: &Session) -> Value {
        let q = s.active_sequence().unwrap();
        json!({"metadata":{"schema":1,"project_id":"p_fixture","sequence_id":"q_original","root":{"note":"editorial note"},"settings":{"width":q.settings.width,"height":q.settings.height,"fps":(q.settings.frame_rate.num as f64)/(q.settings.frame_rate.den as f64)},"fields":["note"],"tracks":{},"assets":{},"links":{}}})
    }
    #[test]
    fn metadata_is_one_native_undo_and_repeating_it_is_a_noop() {
        let mut s = demo();
        let p = request(&s);
        let before = s.project.clone();
        let depth = s.history.undo.len();
        s.execute("poema.setSequenceMetadata", p.clone()).unwrap();
        assert_eq!(s.history.undo.len(), depth + 1);
        let revision = s.revision;
        assert_eq!(s.execute("poema.setSequenceMetadata", p).unwrap()["changed"], false);
        assert_eq!(s.revision, revision);
        assert_eq!(s.history.undo.len(), depth + 1);
        s.execute("edit.undo", json!({})).unwrap();
        assert!(std::sync::Arc::ptr_eq(&before, &s.project));
    }
    #[test]
    fn aliases_cannot_smuggle_geometry_or_mismatch_the_native_frame() {
        let mut s = demo();
        let before = s.project.clone();
        for key in ["tracks", "duration", "link_groups"] {
            let mut p = request(&s);
            p["metadata"]["root"][key] = json!([]);
            assert!(s.execute("poema.setSequenceMetadata", p).is_err());
            assert!(std::sync::Arc::ptr_eq(&before, &s.project));
        }
        let mut p = request(&s);
        p["metadata"]["settings"]["width"] = json!(1);
        assert!(s.execute("poema.setSequenceMetadata", p).is_err());
        assert!(std::sync::Arc::ptr_eq(&before, &s.project));
    }
}
