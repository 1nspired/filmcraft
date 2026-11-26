//! Exact historical following outcomes; no live gesture or writer restored.
use super::super::{COMMAND, EDITOR_KIND, LIMIT, Opaque, Retained, error};
use super::{DEFAULT, ORIGIN_EDITOR_KIND, PreparedBasis, request};
use crate::{
    EditorState, Result,
    agent_bridge::{
        editor_control::{self, Document},
        editor_proof, editor_setting, history_checkpoint, media_guard, parse, project_clear, project_plane,
    },
    project_workspace::ProjectScope,
};
use filmcraft_project::Project;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::sync::Arc;
const ORIGIN_KIND: &str = "native_following_origin_basis";
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Basis {
    version: u8,
    revision: u64,
    saved_revision: u64,
    #[serde(default, skip_serializing_if = "unadvertised")]
    following_write: bool,
}
fn unadvertised(value: &bool) -> bool {
    !*value
}
pub(super) fn encode_basis(revision: u64, saved_revision: u64, following_write: bool) -> Result<Value> {
    let raw = history_checkpoint::bytes(&Basis { version: if following_write { 2 } else { 1 }, revision, saved_revision, following_write }, LIMIT)?;
    serde_json::to_value(Opaque {
        kind: ORIGIN_KIND.into(),
        version: 1,
        sha256: history_checkpoint::digest(&raw),
        bytes: raw.len(),
        data: String::from_utf8(raw).map_err(|e| error(e.to_string()))?,
    })
    .map_err(|e| error(e.to_string()))
}
fn decode_basis(value: &Value) -> Result<Basis> {
    let o: Opaque = parse(COMMAND, value)?;
    if o.kind != ORIGIN_KIND
        || o.version != 1
        || o.bytes == 0
        || o.bytes > LIMIT
        || o.bytes != o.data.len()
        || history_checkpoint::digest(o.data.as_bytes()) != o.sha256
    {
        return Err(error("changed native following origin basis"));
    }
    let b: Basis = serde_json::from_str(&o.data).map_err(|e| error(e.to_string()))?;
    if !matches!((b.version, b.following_write), (1, false) | (2, true))
        || b.revision == 0
        || encode_basis(b.revision, b.saved_revision, b.following_write)? != *value
    {
        return Err(error("foreign or noncanonical following origin basis"));
    }
    Ok(b)
}
pub(in crate::agent_bridge) fn comparable(state: &EditorState, clock_selection: bool) -> Result<Value> {
    let mut v = serde_json::to_value(state).map_err(|e| error(e.to_string()))?;
    let m = v.as_object_mut().ok_or_else(|| error("native editor object required"))?;
    m.remove("trim_shift");
    if let Some(id) = state.active_sequence {
        if let Some(heads) = m.get_mut("playheads").and_then(Value::as_object_mut) {
            heads.remove(&id.0.to_string());
        }
    }
    if clock_selection {
        m.remove("selection");
    }
    Ok(v)
}
fn document(
    command: &str,
    retained: &Retained,
    state: &EditorState,
    clipboard: &str,
    scope: &ProjectScope,
    token: &str,
    path: &str,
    epoch: &str,
    revision: u64,
    saved_revision: u64,
    advertised: bool,
) -> Result<Value> {
    let h = retained.evidence.validate(&retained.projects, command)?;
    h.validate_editor_basis(state.active_sequence, &state.edit_points, state.trim_shift)?;
    let summary = retained.evidence.default_forecast(&retained.projects)?;
    let (dynamic, around) = retained.evidence.playback_kind();
    let sha = project_plane::project_basis(h.before_project())?;
    editor_control::encode_document(Document {
        token,
        scope,
        path,
        project: h.before_project(),
        state,
        history: h.before_history(),
        project_sha256: &sha,
        clipboard_sha256: clipboard,
        revision,
        saved_revision,
        epoch,
        dynamic,
        around,
        settlement: editor_control::settlement_json(&summary),
        change: true,
        following: true,
        following_write: advertised,
        cancellation: true,
    })
}
pub(in crate::agent_bridge) fn validate(
    pool: &[Arc<Project>],
    publication_map: &[usize],
    original_map: &[usize],
    before: &Arc<Project>,
    after: &Arc<Project>,
    path: &str,
    scope: &ProjectScope,
    token: &str,
    revision: u64,
    before_sequence: Option<u64>,
    after_sequence: Option<u64>,
    value: &Value,
    result: &Value,
    check: Option<&media_guard::Check>,
) -> Result<Retained> {
    let r = request(value)?;
    if r.token != token || &r.scope != scope || before_sequence != after_sequence || !super::is_result(result) {
        return Err(error("foreign following historical identity"));
    }
    let publication = result.get("publication").ok_or_else(|| error("following publication receipt missing"))?;
    let publication_read = result.get("publicationRead").ok_or_else(|| error("following publication read missing"))?;
    let publication_request = json!({"version":2,"token":r.token,"scope":r.scope,"beforeSha256":publication_read["projectState"]["sha256"],"beforeEditorSha256":publication_read["editorStateSha256"],"setting":r.setting});
    let mut current = super::super::validate_history(
        pool,
        publication_map,
        before,
        after,
        path,
        scope,
        token,
        revision,
        before_sequence,
        after_sequence,
        &publication_request,
        publication,
        check,
    )?;
    let original = Retained::from_command_proof(
        result.get("originalHistoryProof").ok_or_else(|| error("following original history missing"))?,
        pool,
        original_map,
        DEFAULT,
        check,
    )?;
    original.evidence.validate_following_to(&original.projects, &current.evidence, &current.projects)?;
    let origin = editor_proof::decode(
        result.get("originalEditorProof").ok_or_else(|| error("following original editor proof missing"))?,
        ORIGIN_EDITOR_KIND,
        LIMIT,
        COMMAND,
    )?;
    let published =
        editor_proof::decode(publication.get("editorProof").ok_or_else(|| error("publication editor proof missing"))?, EDITOR_KIND, LIMIT, COMMAND)?;
    if origin.clipboard_sha256 != published.clipboard_sha256 {
        return Err(error("original/publication private clipboard differs"));
    }
    editor_proof::require_after(&origin.after_editor, &published.before, COMMAND)?;
    let (dynamic, around) = original.evidence.playback_kind();
    if comparable(&origin.before, origin.before.selection_follows_playhead && (dynamic || around))?
        != comparable(&published.before, origin.before.selection_follows_playhead && (dynamic || around))?
    {
        return Err(error("non-clock original/publication editor dependencies differ"));
    }
    let header = decode_basis(result.get("originalBasis").ok_or_else(|| error("following original native basis missing"))?)?;
    if header.revision > revision || !dynamic && header.revision != revision {
        return Err(error("original/publication native revision relation differs"));
    }
    let epoch = publication.get("nativeSession").and_then(Value::as_str).ok_or_else(|| error("publication epoch missing"))?;
    let original_read = document(
        DEFAULT,
        &original,
        &origin.before,
        &origin.clipboard_sha256,
        scope,
        token,
        path,
        epoch,
        header.revision,
        header.saved_revision,
        header.following_write,
    )?;
    let current_read = document(
        editor_setting::human(&r.setting).0,
        &current,
        &published.before,
        &published.clipboard_sha256,
        scope,
        token,
        path,
        epoch,
        revision,
        header.saved_revision,
        header.following_write,
    )?;
    let basis = PreparedBasis {
        basis: json!({"original":original_read,"current":current_read}),
        editor: serde_json::to_value(&origin.before).map_err(|e| error(e.to_string()))?,
        history: original,
        revision: header.revision,
        saved_revision: header.saved_revision,
        advertised: header.following_write,
    };
    if super::receipt(value, &basis, publication.clone(), check)? != *result {
        return Err(error("following historical outcome differs from exact original/publication evidence"));
    }
    current.original = Some(Box::new(basis.history));
    Ok(current)
}

// Same native historical validator before publication and after cold loading.
// It validates an immutable forecast; no live owner is synthesized or installed.
pub(in crate::agent_bridge) fn validate_publication(
    value: &Value,
    basis: &PreparedBasis,
    publication: &Retained,
    result: &Value,
    path: &str,
    scope: &ProjectScope,
    token: &str,
    revision: u64,
    sequence: Option<u64>,
    check: Option<&media_guard::Check>,
) -> Result<()> {
    let r = request(value)?;
    let h = publication.history(&r.setting)?;
    let mut pool = Vec::new();
    for p in publication.projects.iter().chain(&basis.history.projects) {
        if !pool.iter().any(|q| Arc::ptr_eq(p, q)) {
            if pool.len() >= 257 {
                return Err(error("complete following original/publication graph exceeds its snapshot bound"));
            }
            pool.push(p.clone());
        }
    }
    let publication_map = publication.mapping(&pool)?;
    let original_map = basis.history.mapping(&pool)?;
    validate(
        &pool,
        &publication_map,
        &original_map,
        h.before_project(),
        h.after_project(),
        path,
        scope,
        token,
        revision,
        sequence,
        sequence,
        value,
        result,
        check,
    )?;
    Ok(())
}
