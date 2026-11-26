//! Original committed setting outcome readback. No writer or fresh ticket.
use super::{ClockPolicy, Read, ReadOwner, document, error};
use crate::{
    Result, Session,
    agent_bridge::{
        editor_compound,
        editor_setting::{self, Setting},
        history_checkpoint, parse, project_audit, project_clear, project_plane, sequences,
    },
    project_workspace::{self, ProjectScope},
};
use filmcraft_project::{EffectKind, ItemId, TrackId};
use serde::Deserialize;
use serde_json::{Value, json};
use std::sync::Arc;
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Request {
    version: u8,
    token: String,
    scope: ProjectScope,
    receipt_sha256: String,
}
fn target(s: &Session, setting: &Setting, sequence: Option<ItemId>, scope: &ProjectScope) -> Result<Value> {
    match setting {
        Setting::DefaultTransition { definition } => {
            let Some(def) = filmcraft_project::find_effect(definition) else {
                return Ok(json!({"kind":"default_transition","present":false,"matchesRequested":false}));
            };
            let (domain, actual) = match def.kind {
                EffectKind::VideoTransition => ("video", &s.state.default_video_transition),
                EffectKind::AudioTransition => ("audio", &s.state.default_audio_transition),
                _ => return Err(error("unknown original default transition domain")),
            };
            if actual.len() > 256 {
                return Err(error("current default identity exceeds its metadata bound"));
            }
            Ok(json!({"kind":"default_transition","present":true,"domain":domain,"definition":actual,"matchesRequested":actual==definition}))
        }
        Setting::ClipMixerMode { track, mode } => {
            let absent = || json!({"kind":"clip_mixer_mode","sequence":sequence.map(|id|id.0),"track":track,"present":false,"matchesRequested":false});
            let Some(id) = sequence else { return Ok(absent()) };
            let Some(owner) = crate::editorial_workspace::association(&s.project)? else { return Ok(absent()) };
            if owner.scope.project_id != scope.project_id || !sequences::belongs(&s.project, id, &owner.scope)? {
                return Ok(absent());
            }
            let Some(q) = s.project.sequence(id) else { return Ok(absent()) };
            if q.audio_tracks.len() > 4096 {
                return Err(error("current outcome track metadata exceeds4096"));
            }
            if q.audio_tracks.iter().filter(|t| t.id.0 == *track).count() != 1 {
                return Ok(absent());
            }
            let actual = crate::mixer::clip_mode(s, TrackId(*track));
            let stored = s.state.clip_mixer_modes.get(track).copied();
            Ok(
                json!({"kind":"clip_mixer_mode","sequence":id.0,"track":track,"present":true,"mode":actual.label(),"storedMode":stored.map(|m|m.label()),"matchesRequested":actual==*mode}),
            )
        }
    }
}
pub(super) fn read(s: &mut Session, value: &Value) -> Result<Value> {
    let r: Request = parse(super::OUTCOME, value)?;
    if r.version != 1 || !crate::agent_bridge::token_ok(&r.token) || !project_workspace::valid_scope(&r.scope) || !editor_setting::valid_hash(&r.receipt_sha256)
    {
        return Err(error("exact original outcome token, Root and complete receipt digest required"));
    }
    project_plane::require_scope(s, &r.scope)?;
    let t = s.agent_bridge.tickets.iter().find(|t| t.token == r.token).ok_or_else(|| error("original setting outcome expired"))?;
    let owner = t.editor_control.as_ref().filter(|o| o.change).ok_or_else(|| error("original editor outcome purpose required"))?;
    let (original, receipt) =
        t.committed.as_ref().filter(|(_, v)| editor_compound::following::is_result(v)).ok_or_else(|| error("committed following setting outcome required"))?;
    if t.project_scope.as_ref() != Some(&r.scope)
        || t.source_library.is_some()
        || t.path != s.path
        || history_checkpoint::digest(&project_audit::value_bytes(receipt)?) != r.receipt_sha256
    {
        return Err(error("foreign Root/path or changed complete setting receipt"));
    }
    let setting: Setting = parse(super::OUTCOME, original.get("setting").ok_or_else(|| error("original setting missing"))?)?;
    let target = target(s, &setting, t.after_sequence, &r.scope)?;
    let matches = target["matchesRequested"] == true;
    let warm = owner.post_binding.is_some();
    let same_epoch = receipt.get("nativeSession").and_then(Value::as_str) == s.agent_bridge.epoch.as_deref();
    let verified = same_epoch && owner.post_binding.as_ref().is_some_and(|p| p.prepare_current(s).is_ok()) && matches;
    let clock = receipt["publication"]["trimOwnerPreserved"] == true && receipt["publicationRead"]["trim"]["active"] == true;
    let exact_idle = if !warm
        && matches
        && !s.trim_play.active()
        && !s.mixrec.active()
        && !s.mcrec.active()
        && !s.previews.live.is_active()
        && t.after_sequence == s.state.active_sequence
        && t.after.as_ref().is_some_and(|p| p.ptr_eq(&Arc::downgrade(&s.project)))
        && project_clear::editor_hash(&s.state)?
            == receipt["publication"]["afterEditorSha256"].as_str().ok_or_else(|| error("publication editor digest missing"))?
    {
        owner.outcome.as_ref().ok_or_else(|| error("historical setting roots missing"))?.describes_after(&s.project, &s.history, &setting)?
    } else {
        false
    };
    let verification = if verified {
        if clock { "same_clock_lineage" } else { "same_current_basis" }
    } else if exact_idle {
        "exact_idle_current"
    } else if warm {
        "changed_since_setting"
    } else {
        "historical_only"
    };
    let address = Read {
        token: r.token.clone(),
        scope: r.scope.clone(),
        project_path: s.path.clone().ok_or_else(|| error("current saved Root path required"))?,
        clock_policy: Some(ClockPolicy::OriginalHumanTrim),
    };
    let current_owner = ReadOwner::capture(s)?;
    let sha = project_plane::basis(s)?;
    let epoch = s.agent_bridge.epoch.as_deref().ok_or_else(|| error("current native epoch missing"))?;
    // Read1 is documentary. In particular this is not a fresh following plan.
    let current = document(s, &address, &current_owner, false, false, &sha, epoch)?;
    let result = json!({"status":"read","kind":"native_editor_control_outcome_read","version":1,"token":r.token,"scope":r.scope,"receiptSha256":r.receipt_sha256,
   "verification":verification,"warmBinding":warm,"current":current,"currentHistory":editor_compound::history_state(&s.project,&s.history)?,"target":target,
   "newReadTicket":false,"agentTimelineCommits":0,"agentUndoOwned":false,"sourceValidation":"not-performed","mediaEvidence":false,"writeAuthority":false});
    if project_audit::value_bytes(&result)?.len() > 64 * 1024 {
        return Err(error("editor setting outcome metadata exceeds64KiB"));
    }
    Ok(result)
}
