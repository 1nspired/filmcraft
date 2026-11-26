//! Explicit following metadata on the original Root Ticket. Never a writer.
//! Historical outcomes have no Following; old purposes cannot borrow this hand.
use super::{ClockPolicy, Read, ReadOwner, document, error};
use crate::agent_bridge::{project_audit, project_plane};
use crate::{Result, Session, project_workspace::ProjectScope};
use serde_json::{Value, json};

pub(in crate::agent_bridge) fn basis(s: &Session, token: &str, scope: &ProjectScope, original_editor_sha256: &str) -> Result<Value> {
    project_plane::require_scope(s, scope)?;
    let t = s.agent_bridge.tickets.iter().find(|t| t.token == token).ok_or_else(|| error("expired following-control read"))?;
    let owner = t.editor_control.as_ref().filter(|o| o.change).ok_or_else(|| error("explicit following-control purpose required"))?;
    if t.committed.is_some() || t.source_library.is_some() || t.project_scope.as_ref() != Some(scope) || t.path != s.path {
        return Err(error("foreign, consumed or changed following Root identity"));
    }
    let original = owner.observation.as_ref().ok_or_else(|| error("the original following observation is required"))?;
    if original.get("editorStateSha256").and_then(Value::as_str) != Some(original_editor_sha256) {
        return Err(error("metadata must bind the ORIGINAL following editor digest"));
    }
    let prepared = owner.following.as_ref().ok_or_else(|| error("strict or historical reads cannot acquire a live following owner"))?.prepare_current(s)?;
    let address = Read {
        token: token.into(),
        scope: scope.clone(),
        project_path: s.path.clone().ok_or_else(|| error("saved following Root path required"))?,
        clock_policy: Some(ClockPolicy::OriginalHumanTrim),
    };
    let current_owner = ReadOwner {
        settlement: Some(prepared),
        change: true,
        following: None,
        observation: None,
        post_binding: None,
        #[cfg(not(target_arch = "wasm32"))]
        outcome: None,
    };
    let sha = project_plane::basis(s)?;
    let epoch = s.agent_bridge.epoch.as_deref().ok_or_else(|| error("following metadata requires the already initialized native epoch"))?;
    let current = document(s, &address, &current_owner, true, true, &sha, epoch)?;
    if original["nativeSession"] != current["nativeSession"]
        || original["scope"] != current["scope"]
        || original["project"]["path"] != current["project"]["path"]
        || original["sequence"] != current["sequence"]
    {
        return Err(error("following observation changed native epoch, Root or sequence"));
    }
    let result = json!({"original":original,"current":current});
    if project_audit::value_bytes(&result)?.len() > 64 * 1024 {
        return Err(error("following metadata basis exceeds64KiB"));
    }
    Ok(result)
}
