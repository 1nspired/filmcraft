//! Owning-project font declarations through the original human usage service.
use super::{parse, project_audit, project_plane};
use crate::{Result, Session, commands::bad, project_workspace::ProjectScope};
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
pub(super) const READ: &str = "agent.readFontsUsed";
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Read {
    token: String,
    scope: ProjectScope,
    offset: usize,
    limit: usize,
}
pub(super) fn read(s: &mut Session, p: &Value) -> Result<Value> {
    let r: Read = parse(READ, p)?;
    project_plane::validate_ticket(s, &r.token, &r.scope)?;
    if r.offset > 100_000 || !(1..=16).contains(&r.limit) {
        return Err(bad(READ, "explicit bounded font usage page required"));
    }
    let usage = crate::graphic_templates::fonts_used_in_project(&s.project);
    if usage.len() > 100_000 || usage.iter().any(|(f, st, _)| f.len() > 16 * 1024 || st.len() > 16 * 1024 || f.contains('\0') || st.contains('\0')) {
        return Err(bad(READ, "complete authored font usage exceeds its bound"));
    }
    if r.offset > usage.len() {
        return Err(bad(READ, "font usage offset outside complete collection"));
    }
    let mut observed = None;
    for _ in 0..2 {
        let generation = filmcraft_text::fonts::registry_revision();
        let rows=usage.iter().enumerate().map(|(position,(family,style,count))|json!({"position":position,"family":family,"style":style,"uses":count,"missing":filmcraft_text::resolve(family,style).missing})).collect::<Vec<_>>();
        if filmcraft_text::fonts::registry_revision() == generation {
            observed = Some(rows);
            break;
        }
    }
    let rows = observed.ok_or_else(|| bad(READ, "font registry changed during usage observation; refresh"))?;
    let hash = format!("{:x}", Sha256::digest(project_audit::value_bytes(&rows)?));
    let end = r.offset.saturating_add(r.limit).min(rows.len());
    let out = json!({"status":"read","kind":"native_authored_font_usage","version":1,"token":r.token,"scope":r.scope,
        "revision":s.revision,"sequence":s.state.active_sequence.map(|id|id.0),"projectSha256":project_plane::basis(s)?,"usageSha256":hash,
        "offset":r.offset,"total":rows.len(),"nextOffset":if end<rows.len(){Some(end)}else{None},"rows":rows.get(r.offset..end).ok_or_else(||bad(READ,"font usage page missing"))?,
        "usageScope":"authored_defaults_saved_character_runs_and_caption_styles","keyframeInventory":false,
        "fontFileState":"not-inspected","resourceEvidence":false,"sourceValidation":"not-performed","writeAuthority":false});
    if project_audit::value_bytes(&out)?.len() > 64 * 1024 {
        return Err(bad(READ, "font usage page exceeds64KiB; request a smaller page"));
    }
    Ok(out)
}
