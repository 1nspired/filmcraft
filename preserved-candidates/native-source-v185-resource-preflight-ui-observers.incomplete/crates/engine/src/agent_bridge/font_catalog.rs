//! The same original human font catalog in the existing owning read planes.
use super::{parse, project_plane, sequences};
use crate::{Result, Session, commands::bad, project_workspace::ProjectScope};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
pub(super) const READ: &str = "agent.readFontCatalog";
#[derive(Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum Realm {
    Project { scope: ProjectScope },
    Sequence { scope: sequences::Scope },
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Read {
    token: String,
    realm: Realm,
    system: bool,
    offset: usize,
    limit: usize,
}
pub(super) fn read(s: &mut Session, p: &Value) -> Result<Value> {
    let r: Read = parse(READ, p)?;
    if r.offset > 100_000 || !(1..=16).contains(&r.limit) {
        return Err(bad(READ, "explicit bounded font page required"));
    }
    match &r.realm {
        Realm::Project { scope } => project_plane::validate_ticket(s, &r.token, scope)?,
        Realm::Sequence { scope } => {
            super::validate_metadata_read(s, &r.token, false, READ)?;
            super::graphics::scope(s, scope)?;
        }
    }
    let catalog = crate::graphics::font_catalog(r.system);
    let total = catalog
        .iter()
        .try_fold(0usize, |n, (_, styles)| n.checked_add(styles.len()).filter(|n| *n <= 100_000))
        .ok_or_else(|| bad(READ, "complete native font catalog exceeds its bound"))?;
    if r.offset > total {
        return Err(bad(READ, "font offset is outside the complete native catalog"));
    }
    let hash = format!("{:x}", Sha256::digest(super::project_audit::value_bytes(&catalog)?));
    let rows = catalog
        .iter()
        .flat_map(|(family, styles)| styles.iter().map(move |style| (family, style)))
        .skip(r.offset)
        .take(r.limit)
        .enumerate()
        .map(|(i, (family, style))| json!({"position":r.offset+i,"family":family,"style":style}))
        .collect::<Vec<_>>();
    let end = r.offset.saturating_add(rows.len());
    let out = json!({"status":"read","kind":"native_font_catalog","version":1,"token":r.token,"realm":r.realm,"revision":s.revision,
        "sequence":s.state.active_sequence.map(|id|id.0),"systemScanRequested":r.system,"systemScanned":filmcraft_text::fonts::system_scanned(),
        "offset":r.offset,"total":total,"nextOffset":if end<total{Some(end)}else{None},"rows":rows,"catalogSha256":hash,
        "syntheticStyleOptions":["Bold","Italic","Bold Italic"],"styleResolution":"original_native_resolver","resourceEvidence":false,"writeAuthority":false});
    if super::project_audit::value_bytes(&out)?.len() > 64 * 1024 {
        return Err(bad(READ, "font catalog page exceeds64KiB; request a smaller page"));
    }
    Ok(out)
}
