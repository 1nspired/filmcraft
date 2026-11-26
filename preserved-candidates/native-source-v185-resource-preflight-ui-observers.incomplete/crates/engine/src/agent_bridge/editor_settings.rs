//! Documentary original editor settings on existing exact read tickets.
//! No setting is changed, no source witness is renewed and no editor/history
//! state is reconstructed. Native live preview/recording owners remain live.
use super::{parse, project_audit, project_clear, project_plane, sequences};
use crate::{EngineError, Result, Session, commands::bad, project_workspace::ProjectScope};
use filmcraft_project::EffectKind;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeSet;

pub(super) const READ: &str = "agent.readEditorSettings";
fn error(message: impl Into<String>) -> EngineError {
    bad(READ, message)
}
#[derive(Deserialize, Serialize)]
#[serde(untagged)]
enum Scope {
    Project(ProjectScope),
    Sequence(sequences::Scope),
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Read {
    #[serde(default)]
    version: Option<u8>,
    token: String,
    scope: Scope,
    before_editor_sha256: String,
    offset: usize,
    limit: usize,
}
fn basis(s: &Session, r: &Read) -> Result<bool> {
    // Metadata reads may observe stored settings while the original live
    // owner runs. All original mutation callers keep their idle requirement.
    let ticket = s.agent_bridge.tickets.iter().find(|t| t.token == r.token).ok_or_else(|| error("expired editor-settings read"))?;
    if let Some(version) = r.version {
        let Scope::Project(scope) = &r.scope else {
            return Err(error("following metadata requires its original Root scope"));
        };
        if version != 2 {
            return Err(error("explicit version-two following metadata required"));
        }
        super::editor_control::following::basis(s, &r.token, scope, &r.before_editor_sha256)?;
    } else {
        if ticket.editor_control.as_ref().is_some_and(|o| o.following.is_some()) {
            return Err(error("following tickets require explicit version-two metadata; no strict-purpose fallback"));
        }
        super::check_ticket_editor_basis(s, &r.token, false)?;
        if r.before_editor_sha256.len() != 64
            || !r.before_editor_sha256.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            || project_clear::editor_hash(&s.state)? != r.before_editor_sha256
        {
            return Err(error("the original editor or private clipboard changed; read a fresh exact editor digest"));
        }
        if let Some(owner) = ticket.editor_control.as_ref().filter(|o| o.change) {
            owner.validate(s)?;
        }
    }
    if ticket.source_library.is_some() {
        return Err(error("a source-library ticket does not own editor settings"));
    }
    match &r.scope {
        Scope::Project(scope) => {
            project_plane::require_scope(s, scope)?;
            if ticket.project_scope.as_ref() != Some(scope) {
                return Err(error("the exact owning Root metadata read is required"));
            }
            let owner = crate::editorial_workspace::association(&s.project)?;
            match (s.state.active_sequence, owner) {
                (Some(id), Some(owner)) if owner.scope.project_id == scope.project_id => sequences::belongs(&s.project, id, &owner.scope),
                _ => Ok(false),
            }
        }
        Scope::Sequence(scope) => {
            let id = s.state.active_sequence.ok_or(EngineError::NoSequence)?;
            if ticket.project_scope.is_some() || !sequences::belongs(&s.project, id, scope)? {
                return Err(error("a current sequence read with its exact owning scope is required"));
            }
            Ok(true)
        }
    }
}
fn default(value: &str, kind: EffectKind) -> Result<Value> {
    if value.len() > 256 {
        return Err(error("stored transition default exceeds the bounded native identity"));
    }
    let known = filmcraft_project::find_effect(value).is_some_and(|d| d.kind == kind);
    Ok(json!({"definition":value,"nativeDefinitionKnown":known}))
}
pub(super) fn read(s: &mut Session, p: &Value) -> Result<Value> {
    let r: Read = parse(READ, p)?;
    let owned = basis(s, &r)?;
    let following = if r.version == Some(2) {
        let Scope::Project(scope) = &r.scope else {
            return Err(error("following Root scope required"));
        };
        Some(super::editor_control::following::basis(s, &r.token, scope, &r.before_editor_sha256)?)
    } else {
        None
    };
    let tracks = if owned { &s.active_sequence().ok_or(EngineError::NoSequence)?.audio_tracks[..] } else { &[] };
    if tracks.len() > 4096 || r.offset > tracks.len() || !(1..=32).contains(&r.limit) {
        return Err(error("bounded explicit editor-settings page required"));
    }
    let mut ids = BTreeSet::new();
    for track in tracks {
        if track.id.0 == 0 || track.id.0 > 9_007_199_254_740_991 || !ids.insert(track.id.0) || track.name.len() > 1024 {
            return Err(error("ambiguous, invalid or oversized original audio track inventory"));
        }
    }
    let end = r.offset.saturating_add(r.limit).min(tracks.len());
    let rows = tracks
        .get(r.offset..end)
        .ok_or_else(|| error("invalid original audio-track page"))?
        .iter()
        .map(|track| {
            let stored = s.state.clip_mixer_modes.get(&track.id.0).copied();
            json!({"track":track.id.0,"name":track.name,"mode":crate::mixer::clip_mode(s,track.id).label(),
                "storedMode":stored.map(filmcraft_project::AutomationMode::label)})
        })
        .collect::<Vec<_>>();
    let mode_state = if s.state.active_sequence.is_none() {
        "no_active_sequence"
    } else if owned {
        "owned_active_sequence"
    } else {
        "outside_owning_sequence"
    };
    let result = json!({"status":"read","kind":"native_editor_settings","version":1,"token":r.token,"revision":s.revision,
        "scope":r.scope,"sequence":s.state.active_sequence.map(|id|id.0),"editorStateSha256":project_clear::editor_hash(&s.state)?,
        "defaults":{"video":default(&s.state.default_video_transition,EffectKind::VideoTransition)?,
            "audio":default(&s.state.default_audio_transition,EffectKind::AudioTransition)?},
        "modeState":mode_state,"offset":r.offset,"total":tracks.len(),"nextOffset":if end<tracks.len(){Some(end)}else{None},"tracks":rows,
        "referenceScope":"exact_read_ticket_only","sourceValidation":"not-performed","mediaEvidence":false,"writeAuthority":false});
    let result = if let Some(following) = &following {
        let mut result = result;
        result["version"] = json!(2);
        result["followingBasis"] = following.clone();
        result
    } else {
        result
    };
    if project_audit::value_bytes(&result)?.len() > 64 * 1024 {
        return Err(error("editor-settings page exceeds its wire budget; use a smaller page"));
    }
    basis(s, &r)?;
    if let Some(following) = following {
        let Scope::Project(scope) = &r.scope else {
            return Err(error("following Root scope required"));
        };
        if following != super::editor_control::following::basis(s, &r.token, scope, &r.before_editor_sha256)? {
            return Err(error("following metadata changed during its bounded native read"));
        }
    }
    Ok(result)
}

#[cfg(test)]
mod tests;
