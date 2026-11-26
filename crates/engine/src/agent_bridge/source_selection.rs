//! Read existing native selections without media I/O, edit tickets or history.
//! Project, Timeline and Source monitor remain distinct; no guessed focus.
use super::{parse, sequences, source_catalog};
use crate::{Result, Session, commands::bad};
use filmcraft_project::ClipId;
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
const COMMAND: &str = "agent.readSourceSelection";
const MAX_SELECTED: usize = 4096;
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    scope: sequences::Scope,
    nonce: String,
}
fn exact_id(value: u64) -> bool {
    (1..=9_007_199_254_740_991).contains(&value)
}
pub(super) fn read(s: &mut Session, params: &Value) -> Result<Value> {
    let request: Request = parse(COMMAND, params)?;
    if !(32..=128).contains(&request.nonce.len()) || !request.nonce.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-') {
        return Err(bad(COMMAND, "an explicit bounded nonce is required"));
    }
    if s.state.selection.len() > MAX_SELECTED || s.state.project_selection.len() > MAX_SELECTED {
        return Err(bad(COMMAND, "complete selection exceeds its bound; no entries truncated"));
    }
    let selected: BTreeSet<ClipId> = s.state.selection.iter().copied().collect();
    let project_selected: BTreeSet<_> = s.state.project_selection.iter().copied().collect();
    if selected.len() != s.state.selection.len()
        || project_selected.len() != s.state.project_selection.len()
        || s.state.selection.iter().any(|i| !exact_id(i.0))
        || s.state.project_selection.iter().any(|i| !exact_id(i.0))
        || s.state.source_item.is_some_and(|i| !exact_id(i.0))
    {
        return Err(bad(COMMAND, "selection contains duplicate or unsafe identities"));
    }
    // Reuse the complete, strict native source/binding metadata projection.
    // It does not stat, resolve, decode or hash original files.
    let catalog = source_catalog::read(s, &json!({"scope":request.scope}))?;
    let sources: BTreeMap<u64, &Value> = catalog
        .get("sources")
        .and_then(Value::as_array)
        .ok_or_else(|| bad(COMMAND, "complete native source catalog unavailable"))?
        .iter()
        .map(|r| r.get("item").and_then(Value::as_u64).map(|id| (id, r)).ok_or_else(|| bad(COMMAND, "source identity unavailable")))
        .collect::<Result<_>>()?;
    let binding_key = |row: &Value| -> Option<(String, String)> {
        let b = row.get("binding")?;
        if b.get("role")?.as_str()? != "original" {
            return None;
        }
        Some((b.get("projectId")?.as_str()?.into(), b.get("assetId")?.as_str()?.into()))
    };
    let mut counts = BTreeMap::<(String, String), usize>::new();
    for row in sources.values() {
        if let Some(key) = binding_key(row) {
            let count = counts.entry(key).or_default();
            *count = count.checked_add(1).ok_or_else(|| bad(COMMAND, "binding count overflow"))?;
        }
    }
    let binding_matches = |item: u64| sources.get(&item).and_then(|row| binding_key(row)).and_then(|key| counts.get(&key)).copied().unwrap_or(0);
    let sequence = s.active_sequence().ok_or(crate::EngineError::NoSequence)?;
    let mut clips = BTreeMap::new();
    let mut scanned = 0usize;
    for track in sequence.all_tracks() {
        scanned = scanned.checked_add(track.items.len()).ok_or_else(|| bad(COMMAND, "sequence clip count overflow"))?;
        if scanned > 100_000 {
            return Err(bad(COMMAND, "sequence exceeds the complete selection scan bound"));
        }
        for clip in &track.items {
            if selected.contains(&clip.id) && (!exact_id(clip.item.0) || clips.insert(clip.id, clip.item).is_some()) {
                return Err(bad(COMMAND, "selected timeline identity is duplicated or unsafe"));
            }
        }
    }
    if clips.len() != selected.len() {
        return Err(bad(COMMAND, "selected timeline identity is absent from this sequence"));
    }
    let source = |item: u64| sources.get(&item).copied().cloned().unwrap_or(Value::Null);
    let timeline: Vec<Value> = s
        .state
        .selection
        .iter()
        .map(|clip| {
            let item = clips.get(clip).ok_or_else(|| bad(COMMAND, "selected timeline identity changed"))?;
            Ok(json!({"clip":clip.0,"item":item.0,"source":source(item.0),"bindingMatches":binding_matches(item.0)}))
        })
        .collect::<Result<_>>()?;
    let project: Vec<Value> =
        s.state.project_selection.iter().map(|item| json!({"item":item.0,"source":source(item.0),"bindingMatches":binding_matches(item.0)})).collect();
    let monitor = s.state.source_item.map(|item| json!({"item":item.0,"source":source(item.0),"bindingMatches":binding_matches(item.0)}));
    let value = json!({"kind":"native_source_selection","version":1,"status":"metadata","nonce":request.nonce,
        "projectId":request.scope.project_id,"anchorSequence":request.scope.anchor_sequence,
        "nativeSession":catalog.get("nativeSession"),"revision":catalog.get("revision"),"project":catalog.get("project"),"sequence":catalog.get("sequence"),
        "timeline":timeline,"projectSelection":project,"sourceMonitor":monitor,
        "requiresSourceValidation":true,"writeAuthority":false});
    if serde_json::to_vec(&value).map_err(|e| bad(COMMAND, e.to_string()))?.len() > 7 * 1024 * 1024 {
        return Err(bad(COMMAND, "complete selection metadata exceeds its byte bound"));
    }
    Ok(value)
}
