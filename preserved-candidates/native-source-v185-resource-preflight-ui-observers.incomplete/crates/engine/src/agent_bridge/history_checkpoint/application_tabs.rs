//! Historical application-tab graph on the ORIGINAL native checkpoint pool.
//! No Session importer, read ticket, live clock, publication fence or Undo grant.
use super::{MAX_BYTES, MAX_STEPS, Step, add, bytes, digest, project_bytes};
use crate::agent_bridge::{
    application_tab_scope::{ApplicationTabReceipt, Snapshot, historical_result},
    compound_history, editor_compound, editor_proof, project_plane, sequence_views,
};
use crate::{EditorState, History, Result, commands::bad};
use filmcraft_project::Project;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{collections::BTreeSet, sync::Arc};
const KIND: &str = "native_application_tab_history_proof";
const COMMAND: &str = "application tab history";
fn error(message: impl Into<String>) -> crate::EngineError {
    bad(COMMAND, message)
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Forecast {
    evidence: String,
    snapshots: Vec<usize>,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SavedSnapshot {
    project: usize,
    undo: Vec<Step>,
    redo: Vec<Step>,
    limit: usize,
    merge_key: Option<String>,
    editor: Value,
    clipboard: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    native: Option<crate::agent_bridge::native_editor::Snapshot>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    native_error: Option<String>,
    path: Option<String>,
    epoch: String,
    revision: u64,
    saved_revision: u64,
    forecast: Option<Forecast>,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Saved {
    version: u8,
    original: Value,
    result: Value,
    confirmed: bool,
    origin: SavedSnapshot,
    before: SavedSnapshot,
    after: SavedSnapshot,
    actual: Option<SavedSnapshot>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Payload {
    version: u8,
    snapshots: Vec<String>,
    record: Saved,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Proof {
    kind: String,
    version: u8,
    bytes: usize,
    sha256: String,
    data: String,
}
/// Strong, immutable native historical snapshots. Decoding this object can
/// never create ApplicationTabScope or a native project/editor/history writer.
pub struct ApplicationTabHistory {
    saved: Saved,
    projects: Vec<Arc<Project>>,
}
// Inspect the closed outer carrier by reference BEFORE cloning or decoding any
// input-owned strings. A hostile caller may already hold an arbitrarily large
// Value; the native codec must not duplicate it merely to discover its bound.
fn proof_data(value: &Value) -> Result<&str> {
    if value.as_object().is_none_or(|object| object.len() != 5) {
        return Err(error("closed native application history proof required"));
    }
    let data = value
        .get("data")
        .and_then(Value::as_str)
        .filter(|data| !data.is_empty() && data.len() <= MAX_BYTES)
        .ok_or_else(|| error("bounded native application history data required"))?;
    if value.get("kind").and_then(Value::as_str) != Some(KIND)
        || !matches!(value.get("version").and_then(Value::as_u64), Some(1 | 2))
        || value.get("bytes").and_then(Value::as_u64) != u64::try_from(data.len()).ok()
        || value.get("sha256").and_then(Value::as_str).is_none_or(|hash| hash.len() != 64 || hash != digest(data.as_bytes()))
    {
        return Err(error("changed or oversized native application history proof"));
    }
    Ok(data)
}
fn save_snapshot(s: &Snapshot, pool: &mut Vec<Arc<Project>>) -> Result<SavedSnapshot> {
    let project = add(pool, &s.project)?;
    let mut steps =
        |v: &[(String, Arc<Project>)]| -> Result<Vec<Step>> { v.iter().map(|(label, p)| Ok(Step { label: label.clone(), project: add(pool, p)? })).collect() };
    let undo = steps(&s.history.undo)?;
    let redo = steps(&s.history.redo)?;
    let forecast = s
        .forecast
        .as_ref()
        .map(|capture| -> Result<Forecast> {
            let evidence = String::from_utf8(capture.evidence().encode()?).map_err(|e| error(e.to_string()))?;
            let snapshots = capture.projects().iter().map(|p| add(pool, p)).collect::<Result<Vec<_>>>()?;
            Ok(Forecast { evidence, snapshots })
        })
        .transpose()?;
    Ok(SavedSnapshot {
        project,
        undo,
        redo,
        limit: s.history.limit,
        merge_key: s.history.merge_key.clone(),
        editor: s.editor.clone(),
        clipboard: s.clipboard.clone(),
        native: s.native.clone(),
        native_error: s.native_error.clone(),
        path: s.path.clone(),
        epoch: s.epoch.clone(),
        revision: s.revision,
        saved_revision: s.saved_revision,
        forecast,
    })
}
pub(in crate::agent_bridge) fn capture(receipt: &ApplicationTabReceipt) -> Result<ApplicationTabHistory> {
    let p = receipt.history_parts();
    let mut pool = Vec::new();
    let saved = Saved {
        version: 2,
        original: p.original.clone(),
        result: receipt.document().clone(),
        confirmed: receipt.confirmed(),
        origin: save_snapshot(p.origin, &mut pool)?,
        before: save_snapshot(p.before, &mut pool)?,
        after: save_snapshot(p.after, &mut pool)?,
        actual: p.actual.map(|s| save_snapshot(s, &mut pool)).transpose()?,
    };
    let result = ApplicationTabHistory { saved, projects: pool };
    result.validate()?;
    result.proof()?;
    Ok(result)
}
fn lookup(pool: &[Arc<Project>], index: usize) -> Result<&Arc<Project>> {
    pool.get(index).ok_or_else(|| error("incomplete native application snapshot mapping"))
}
fn snapshot(v: &SavedSnapshot, pool: &[Arc<Project>]) -> Result<Snapshot> {
    if v.undo.len().saturating_add(v.redo.len()) > MAX_STEPS
        || v.limit > 1000
        || v.merge_key.as_ref().is_some_and(|s| s.len() > 2048)
        || !filmcraft_project::ClipIdentity::valid_id(&v.epoch)
        || v.revision == 0
        || v.revision > 9_007_199_254_740_991
        || v.saved_revision > v.revision
        || !crate::agent_bridge::editor_setting::valid_hash(&v.clipboard)
        || v.path
            .as_ref()
            .is_none_or(|p| p.len() > 4096 || !std::path::Path::new(p).is_absolute() || !p.ends_with(".fcproj") || p.chars().any(char::is_control))
    {
        return Err(error("invalid bounded native application snapshot header"));
    }
    let editor = editor_proof::exact_editor(&v.editor, COMMAND)?;
    let load = |steps: &[Step]| -> Result<Vec<(String, Arc<Project>)>> {
        steps
            .iter()
            .map(|s| {
                if s.label.len() > 2048 {
                    return Err(error("application history label exceeds original bound"));
                }
                Ok((s.label.clone(), lookup(pool, s.project)?.clone()))
            })
            .collect()
    };
    if editor.active_sequence.is_some_and(|id| lookup(pool, v.project).is_ok_and(|p| p.sequence(id).is_none())) {
        return Err(error("application editor sequence is absent from its snapshot"));
    }
    if let Some(native) = &v.native {
        native.validate(v.revision, editor.active_sequence.map(|id| id.0))?;
        let mut original = native.editor.clone();
        original.clipboard = native.clipboard.clone();
        original.clipboard_markers = native.clipboard_markers.clone();
        if serde_json::to_value(&native.editor).map_err(|e| error(e.to_string()))? != v.editor
            || super::super::project_clear::clipboard_hash(&original)? != v.clipboard
            || native.merge_key != v.merge_key
            || native.saved_revision != v.saved_revision
        {
            return Err(error("raw historical native editor/clipboard/history differs from its original snapshot"));
        }
    }
    Ok(Snapshot {
        project: lookup(pool, v.project)?.clone(),
        history: History { undo: load(&v.undo)?, redo: load(&v.redo)?, limit: v.limit, merge_key: v.merge_key.clone() },
        editor: v.editor.clone(),
        clipboard: v.clipboard.clone(),
        native: v.native.clone(),
        native_error: v.native_error.clone(),
        path: v.path.clone(),
        epoch: v.epoch.clone(),
        revision: v.revision,
        saved_revision: v.saved_revision,
        forecast: None,
    })
}
fn forecast(v: &Forecast, pool: &[Arc<Project>]) -> Result<compound_history::Retained> {
    if v.snapshots.is_empty() || v.snapshots.len() > MAX_STEPS + 1 || v.snapshots.iter().copied().collect::<BTreeSet<_>>().len() != v.snapshots.len() {
        return Err(error("collapsed application trim evidence mapping"));
    }
    let evidence = crate::trim::command_settlement::historical::Evidence::decode(v.evidence.as_bytes())?;
    if evidence.encode()? != v.evidence.as_bytes() {
        return Err(error("noncanonical original native trim history"));
    }
    evidence.validate_mapped(pool, &v.snapshots, "sequence.open")?;
    let projects = v.snapshots.iter().map(|i| lookup(pool, *i).cloned()).collect::<Result<Vec<_>>>()?;
    Ok(compound_history::Retained { evidence, projects, original: None })
}
fn validate_basis(s: &Snapshot, f: &compound_history::Retained) -> Result<EditorState> {
    let h = f.validate("sequence.open")?;
    let editor = editor_proof::exact_editor(&s.editor, COMMAND)?;
    h.validate_editor_basis(editor.active_sequence, &editor.edit_points, editor.trim_shift)?;
    if !Arc::ptr_eq(&s.project, h.before_project()) || !crate::trim::command_settlement::historical::same_history(&s.history, h.before_history()) {
        return Err(error("application snapshot differs from its exact original native trim/history basis"));
    }
    Ok(editor)
}
impl ApplicationTabHistory {
    pub fn projects(&self) -> &[Arc<Project>] {
        &self.projects
    }
    pub fn result(&self) -> &Value {
        &self.saved.result
    }
    pub fn actual_snapshot_complete(&self) -> bool {
        self.saved.confirmed || self.saved.actual.is_some()
    }
    /// Complete immutable raw snapshot data only; never a current action grant.
    /// Legacy hash-only records and busy/missing auxiliary captures stay partial.
    pub fn native_snapshot_complete(&self) -> bool {
        self.saved.version == 2
            && self.actual_snapshot_complete()
            && [&self.saved.origin, &self.saved.before, &self.saved.after]
                .into_iter()
                .chain(self.saved.actual.iter())
                .all(|s| s.native.as_ref().is_some_and(|n| n.auxiliary.is_some()) && s.native_error.is_none())
    }
    pub fn original_read(&self) -> &Value {
        &self.saved.original
    }
    pub fn mapping(&self, pool: &[Arc<Project>]) -> Result<Vec<usize>> {
        if pool.len() > MAX_STEPS + 1 {
            return Err(error("application archive pool exceeds original snapshot bound"));
        }
        self.projects
            .iter()
            .map(|p| pool.iter().position(|q| Arc::ptr_eq(p, q)).ok_or_else(|| error("original complete native pool is missing an application snapshot")))
            .collect()
    }
    fn hashes(&self) -> Result<Vec<String>> {
        let mut budget = MAX_BYTES;
        self.projects
            .iter()
            .map(|p| {
                let raw = project_bytes(p, budget)?;
                budget = budget.checked_sub(raw.len()).ok_or_else(|| error("application snapshot byte budget exhausted"))?;
                project_plane::project_basis(p)
            })
            .collect()
    }
    pub fn proof(&self) -> Result<Value> {
        self.validate()?;
        let payload = Payload { version: self.saved.version, snapshots: self.hashes()?, record: self.saved.clone() };
        let raw = bytes(&payload, MAX_BYTES)?;
        let proof = Proof {
            kind: KIND.into(),
            version: self.saved.version,
            bytes: raw.len(),
            sha256: digest(&raw),
            data: String::from_utf8(raw).map_err(|e| error(e.to_string()))?,
        };
        serde_json::to_value(proof).map_err(|e| error(e.to_string()))
    }
    /// Readonly decode on the existing complete native project pool. A mapping is
    /// injective even when separate original COW roots have equal serialized bytes.
    pub fn from_proof(value: &Value, pool: &[Arc<Project>], mapping: &[usize]) -> Result<Self> {
        if pool.len() > MAX_STEPS + 1
            || mapping.is_empty()
            || mapping.len() > MAX_STEPS + 1
            || mapping.iter().copied().collect::<BTreeSet<_>>().len() != mapping.len()
        {
            return Err(error("bounded injective complete native snapshot map required"));
        }
        let data = proof_data(value)?;
        let payload: Payload = serde_json::from_str(data).map_err(|e| error(e.to_string()))?;
        if !matches!(payload.version, 1 | 2)
            || value["version"].as_u64() != Some(u64::from(payload.version))
            || payload.version != payload.record.version
            || payload.snapshots.len() != mapping.len()
            || bytes(&payload, MAX_BYTES)? != data.as_bytes()
        {
            return Err(error("noncanonical or incomplete native application history"));
        }
        let projects = mapping.iter().map(|i| lookup(pool, *i).cloned()).collect::<Result<Vec<_>>>()?;
        for (i, p) in projects.iter().enumerate() {
            if projects.get(..i).is_none_or(|prior| prior.iter().any(|q| Arc::ptr_eq(p, q))) {
                return Err(error("distinct original snapshot identities were collapsed"));
            }
        }
        let result = Self { saved: payload.record, projects };
        if result.hashes()? != payload.snapshots {
            return Err(error("native application snapshot hashes differ"));
        }
        result.validate()?;
        if result.proof()? != *value {
            return Err(error("native application history did not roundtrip exactly"));
        }
        Ok(result)
    }
    fn validate(&self) -> Result<()> {
        let r = &self.saved;
        if !matches!(r.version, 1 | 2) || self.projects.is_empty() || self.projects.len() > MAX_STEPS + 1 {
            return Err(error("invalid native application history header"));
        }
        for (i, p) in self.projects.iter().enumerate() {
            if self.projects.get(..i).is_none_or(|prior| prior.iter().any(|q| Arc::ptr_eq(p, q))) {
                return Err(error("native application snapshot identity collapsed"));
            }
        }
        for s in [&r.origin, &r.before, &r.after].into_iter().chain(r.actual.iter()) {
            if (r.version == 1 && (s.native.is_some() || s.native_error.is_some()))
                || (r.version == 2 && (s.native.is_some() == s.native_error.is_some()))
                || s.native_error.as_ref().is_some_and(|e| e.is_empty() || e.chars().count() > 512)
            {
                return Err(error("explicit versioned historical native payload or capture failure required"));
            }
        }
        let origin = snapshot(&r.origin, &self.projects)?;
        let before = snapshot(&r.before, &self.projects)?;
        let after = snapshot(&r.after, &self.projects)?;
        let of = forecast(r.origin.forecast.as_ref().ok_or_else(|| error("original application trim evidence missing"))?, &self.projects)?;
        let bf = forecast(r.before.forecast.as_ref().ok_or_else(|| error("current application trim evidence missing"))?, &self.projects)?;
        let oe = validate_basis(&origin, &of)?;
        let be = validate_basis(&before, &bf)?;
        let ae = editor_proof::exact_editor(&after.editor, COMMAND)?;
        of.evidence.validate_sequence_view_following_to(&of.projects, &bf.evidence, &bf.projects)?;
        let (dynamic, around) = of.evidence.playback_kind();
        if origin.path != before.path
            || origin.epoch != before.epoch
            || origin.saved_revision != before.saved_revision
            || origin.revision > before.revision
            || !dynamic && origin.revision != before.revision
            || origin.clipboard != before.clipboard
            || editor_compound::following::history::comparable(&oe, oe.selection_follows_playhead && (dynamic || around))?
                != editor_compound::following::history::comparable(&be, oe.selection_follows_playhead && (dynamic || around))?
        {
            return Err(error("application original/current history or non-clock editor differs"));
        }
        let scope = serde_json::from_value(r.original.get("scope").cloned().ok_or_else(|| error("original application scope missing"))?)
            .map_err(|e| error(e.to_string()))?;
        let token = r.original["token"].as_str().ok_or_else(|| error("original application token missing"))?;
        let cancellation =
            r.original["capabilities"]["operationCancellation"].as_bool().ok_or_else(|| error("original application Stop descriptor missing"))?;
        let oh = of.validate("sequence.open")?;
        let original = sequence_views::encode_document(sequence_views::Document {
            project: &origin.project,
            state: &oe,
            clipboard_sha256: &origin.clipboard,
            history: &origin.history,
            path: origin.path.as_deref().ok_or_else(|| error("original saved path missing"))?,
            scope: &scope,
            token,
            epoch: &origin.epoch,
            revision: origin.revision,
            saved_revision: origin.saved_revision,
            active: dynamic || around,
            summary: oh.summary(),
            change: false,
            cancellation,
        })?;
        if original != r.original {
            return Err(error("historical native original read differs from original service"));
        }
        let expected = if r.confirmed { &r.result } else { r.result.get("expected").ok_or_else(|| error("uncertain application expected outcome missing"))? };
        let item =
            expected["item"].as_u64().filter(|id| (1..=9_007_199_254_740_991).contains(id)).ok_or_else(|| error("exact observed application item missing"))?;
        if !original["openTabs"].as_array().is_some_and(|rows| rows.iter().any(|row| row["item"].as_u64() == Some(item)))
            || !be.open_sequences.contains(&filmcraft_project::ItemId(item))
        {
            return Err(error("historical application target was not an observed open tab"));
        }
        let switched = be.active_sequence != Some(filmcraft_project::ItemId(item));
        let h = bf.validate("sequence.open")?;
        let mut wanted = be.clone();
        let summary = if switched {
            wanted.trim_shift = h.trim_shift_after();
            crate::commands::open_sequence_state(h.after_project(), &mut wanted, filmcraft_project::ItemId(item))?;
            if !Arc::ptr_eq(&after.project, h.after_project()) || !crate::trim::command_settlement::historical::same_history(&after.history, h.after_history())
            {
                return Err(error("application tab settlement snapshot/history differs"));
            }
            let af = forecast(r.after.forecast.as_ref().ok_or_else(|| error("applied original open history missing"))?, &self.projects)?;
            if af.evidence.encode()? != bf.evidence.encode()?
                || af.projects.len() != bf.projects.len()
                || af.projects.iter().zip(&bf.projects).any(|(a, b)| !Arc::ptr_eq(a, b))
            {
                return Err(error("applied tab history differs from original human settlement"));
            }
            h.summary().clone()
        } else {
            if r.after.forecast.is_some()
                || !Arc::ptr_eq(&before.project, &after.project)
                || !crate::trim::command_settlement::historical::same_history(&before.history, &after.history)
            {
                return Err(error("same-active tab must preserve original history and live trim description"));
            }
            crate::trim::command_settlement::Summary::default()
        };
        if serde_json::to_value(&wanted).map_err(|e| error(e.to_string()))? != after.editor
            || ae.active_sequence != Some(filmcraft_project::ItemId(item))
            || before.path != after.path
            || before.epoch != after.epoch
            || before.revision != after.revision
            || before.saved_revision != after.saved_revision
            || before.clipboard != after.clipboard
            || historical_result(&r.original, &before, &after, &summary, item, switched)? != *expected
        {
            return Err(error("native application outcome differs from exact original services"));
        }
        if r.confirmed {
            if r.actual.is_some() {
                return Err(error("confirmed native application history cannot borrow uncertain actual state"));
            }
        } else {
            if !r.result.as_object().is_some_and(|o| {
                o.keys().map(String::as_str).collect::<BTreeSet<_>>()
                    == [
                        "kind",
                        "version",
                        "status",
                        "expected",
                        "error",
                        "actualSnapshotRetained",
                        "actualSnapshotError",
                        "canReplay",
                        "mediaEvidence",
                        "writeAuthority",
                    ]
                    .into_iter()
                    .collect()
            }) || r.result["kind"] != "native_application_tab_uncertain"
                || r.result["version"] != 1
                || r.result["status"] != "unknown"
                || r.result["canReplay"] != false
                || r.result["mediaEvidence"] != false
                || r.result["writeAuthority"] != false
                || r.result["actualSnapshotRetained"] != r.actual.is_some()
                || r.result["error"].as_str().is_none_or(|s| s.chars().count() > 512)
            {
                return Err(error("invalid explicit uncertain native application outcome"));
            }
            if let Some(actual) = &r.actual {
                let s = snapshot(actual, &self.projects)?;
                let f = forecast(actual.forecast.as_ref().ok_or_else(|| error("uncertain actual native trim evidence missing"))?, &self.projects)?;
                validate_basis(&s, &f)?;
                if !r.result["actualSnapshotError"].is_null() {
                    return Err(error("retained actual snapshot cannot claim capture failure"));
                }
            } else if r.result["actualSnapshotError"].as_str().is_none_or(|s| s.chars().count() > 512) {
                return Err(error("missing actual snapshot must retain its original native failure"));
            }
        }
        // No pool member may be silently dropped, duplicated or left unattached.
        let mut used = BTreeSet::new();
        for s in [&r.origin, &r.before, &r.after].into_iter().chain(r.actual.iter()) {
            used.insert(s.project);
            used.extend(s.undo.iter().chain(&s.redo).map(|step| step.project));
            if let Some(f) = &s.forecast {
                used.extend(f.snapshots.iter().copied());
            }
        }
        if used.len() != self.projects.len() || used.iter().copied().ne(0..self.projects.len()) {
            return Err(error("native application archive contains unreferenced or incomplete roots"));
        }
        Ok(())
    }
}
#[cfg(test)]
mod tests;
