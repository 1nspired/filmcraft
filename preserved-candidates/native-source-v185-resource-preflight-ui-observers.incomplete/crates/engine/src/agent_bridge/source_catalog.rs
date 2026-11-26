//! Current project metadata for trusted dependency selection, never a read ticket.
//! No original-path resolution, stat, decode, file observation or history writer.
use super::{BINDING_KEY, organization_scope, parse};
use crate::{Result, Session, commands::bad};
use serde::Deserialize;
use serde_json::{Value, json};

const COMMAND: &str = "agent.readSourceCatalog";
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    scope: organization_scope::Scope,
}

pub(super) fn read(s: &mut Session, params: &Value) -> Result<Value> {
    let request: Request = parse(COMMAND, params)?;
    if cfg!(target_arch = "wasm32") {
        return Err(bad(COMMAND, "native source catalog projection is unavailable"));
    }
    super::idle(s)?;
    if !request.scope.valid() {
        return Err(bad(COMMAND, "an exact closed project or sequence scope is required"));
    }
    let sequence = request.scope.require(s)?;
    let independent = matches!(&request.scope, organization_scope::Scope::Project(_));
    if s.project.items.len() > 100_000 {
        return Err(bad(COMMAND, "project exceeds the complete source catalog bound"));
    }
    if s.path.as_ref().is_some_and(|p| p.len() > 4096 || p.chars().any(char::is_control)) {
        return Err(bad(COMMAND, "project path exceeds the metadata bound"));
    }
    let mut rows = Vec::new();
    let mut bytes = 0usize;
    for item in s.project.items.values() {
        let Some(media) = item.as_media() else { continue };
        if !(1..=9_007_199_254_740_991).contains(&item.id.0) {
            return Err(bad(COMMAND, "source ID exceeds the exact metadata transport"));
        }
        if item.name.len() > 1024 {
            return Err(bad(COMMAND, "source name exceeds the complete metadata bound"));
        }
        let raw = item.metadata.get(BINDING_KEY);
        if raw.is_some_and(|r| r.len() > 8192) {
            return Err(bad(COMMAND, "source binding exceeds the metadata bound"));
        }
        let binding = super::source_binding(raw, COMMAND)?;
        if binding.as_ref().is_some_and(|b| [b.project_id.len(), b.asset_id.len(), b.media_id.len(), b.role.len()].into_iter().any(|n| n > 256)) {
            return Err(bad(COMMAND, "source binding fields exceed the metadata bound"));
        }
        let row = json!({"item":item.id.0,"name":item.name,"binding":binding,"file":matches!(media.media,filmcraft_project::MediaRef::File{..})});
        let size = serde_json::to_vec(&row).map_err(|e| bad(COMMAND, e.to_string()))?.len();
        bytes = bytes.checked_add(size).and_then(|n| n.checked_add(1)).ok_or_else(|| bad(COMMAND, "source metadata size overflow"))?;
        if bytes > 7 * 1024 * 1024 {
            return Err(bad(COMMAND, "complete source metadata exceeds its transport bound; no rows were truncated"));
        }
        rows.push(row);
    }
    #[cfg(not(target_arch = "wasm32"))]
    let native_session = super::history_checkpoint::epoch(s)?;
    #[cfg(target_arch = "wasm32")]
    let native_session = Value::Null;
    let mut result = json!({"status":"metadata","kind":"native_source_catalog","version":if independent { 2 } else { 1 },"nativeSession":native_session,
        "revision":s.revision,"project":{"path":s.path,"dirty":s.revision != s.saved_revision},"sequence":sequence.map(|id|id.0),"sources":rows,
        "requiresSourceValidation":true,"writeAuthority":false});
    if independent {
        result["scope"] = serde_json::to_value(&request.scope).map_err(|e| bad(COMMAND, e.to_string()))?;
    }
    Ok(result)
}
