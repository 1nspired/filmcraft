//! Complete physical project-tree pages and exact shared native organization.
//! Names are documentary; IDs are usable only with the owning current ticket.
use super::{Operation, organization_scope, parse, project_audit};
use crate::{EngineError, Result, Session, commands::bad, project_organization};
use filmcraft_project::{Bin, BinEntry, BinId, ItemId, Project};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;

pub(super) const COMMAND: &str = "agent.organizeProject";
const SAFE: u64 = 9_007_199_254_740_991;
const MAX_ENTRIES: usize = 100_000;
fn error(message: impl Into<String>) -> EngineError {
    bad(COMMAND, message)
}
fn text(value: &str) -> bool {
    value.len() <= 1024 && !value.contains('\0')
}
fn name(value: &str) -> bool {
    text(value) && !value.trim().is_empty()
}
#[derive(Serialize)]
struct Row {
    id: u64,
    kind: &'static str,
    parent: Option<u64>,
    position: usize,
    depth: usize,
    name: String,
    #[serde(rename = "itemType")]
    item_type: Option<&'static str>,
    children: usize,
    visible: bool,
}
struct Catalog {
    rows: Vec<Row>,
    bins: usize,
    items: usize,
    sha256: String,
}
enum Pending<'a> {
    Bin(&'a Bin, Option<u64>, usize, usize),
    Item(ItemId, u64, usize, usize),
}
fn catalog(project: &Project) -> Result<Catalog> {
    if project.items.len() > MAX_ENTRIES || project.search_bins.len() > MAX_ENTRIES {
        return Err(error("project organization inventory exceeds its entry bound"));
    }
    let mut rows = Vec::new();
    let mut ids = BTreeSet::new();
    let mut pending = vec![Pending::Bin(&project.root, None, 0, 0)];
    let mut bins = 0usize;
    let mut items = 0usize;
    while let Some(entry) = pending.pop() {
        if rows.len() >= MAX_ENTRIES {
            return Err(error("project organization inventory exceeds 100000 entries"));
        }
        let row = match entry {
            Pending::Bin(bin, parent, position, depth) => {
                if depth > 64 || bin.id.0 > SAFE || (bin.id.0 == 0 && parent.is_some()) || !text(&bin.name) {
                    return Err(error("invalid bin depth, identity or name"));
                }
                bins += 1;
                for (index, child) in bin.children.iter().enumerate().rev() {
                    if rows.len().saturating_add(pending.len()).saturating_add(1) >= MAX_ENTRIES {
                        return Err(error("project organization queue exceeds its entry bound"));
                    }
                    pending.push(match child {
                        BinEntry::Bin(child) => Pending::Bin(child, Some(bin.id.0), index + 1, depth + 1),
                        BinEntry::Item(id) => Pending::Item(*id, bin.id.0, index + 1, depth + 1),
                    });
                }
                Row { id: bin.id.0, kind: "bin", parent, position, depth, name: bin.name.clone(), item_type: None, children: bin.children.len(), visible: true }
            }
            Pending::Item(id, parent, position, depth) => {
                let item = project.item(id).ok_or_else(|| error("tree contains a missing project item"))?;
                if id.0 == 0 || id.0 > SAFE || item.id != id || !text(&item.name) || depth > 65 {
                    return Err(error("invalid physical project item metadata"));
                }
                items += 1;
                Row {
                    id: id.0,
                    kind: "item",
                    parent: Some(parent),
                    position,
                    depth,
                    name: item.name.clone(),
                    item_type: Some(item.type_label()),
                    children: 0,
                    visible: crate::project_panel::listed(project, item),
                }
            }
        };
        if !ids.insert(row.id) {
            return Err(error("project tree identity is duplicated or ambiguous"));
        }
        rows.push(row);
    }
    if items != project.items.len() {
        return Err(error("unfiled project items prevent a complete physical inventory"));
    }
    // Saved search definitions are preserved proof, not physical move targets.
    let bytes = project_audit::value_bytes(&(&rows, &project.search_bins))?;
    Ok(Catalog { rows, bins, items, sha256: format!("{:x}", Sha256::digest(bytes)) })
}
pub(super) fn require_visible_targets(project: &Project, entries: &[u64]) -> Result<()> {
    let c = catalog(project)?;
    for id in entries {
        if !c.rows.iter().any(|row| row.id == *id && row.parent.is_some() && row.visible) {
            return Err(error("each Clear target must be a current visible nonroot physical entry"));
        }
    }
    Ok(())
}
pub(super) fn summary(s: &Session) -> Value {
    project_summary(&s.project)
}
pub(super) fn project_summary(project: &filmcraft_project::Project) -> Value {
    match catalog(project) {
        Ok(c) => {
            json!({"status":"current","scope":"physical_project_tree","sha256":c.sha256,"entries":c.rows.len(),"bins":c.bins,"items":c.items,"savedSearchBins":project.search_bins.len()})
        }
        Err(e) => json!({"status":"unavailable","reason":e.to_string()}),
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Read {
    token: String,
    scope: organization_scope::Scope,
    offset: usize,
    limit: usize,
    #[serde(default)]
    projection: bool,
}
fn require_scope(s: &Session, scope: &organization_scope::Scope) -> Result<Option<filmcraft_project::ItemId>> {
    scope.require(s)
}

pub(super) fn read(s: &mut Session, p: &Value) -> Result<Value> {
    let r: Read = parse("agent.readBins", p)?;
    if r.offset > MAX_ENTRIES || r.limit == 0 || r.limit > 32 {
        return Err(error("explicit bounded physical-tree page required"));
    }
    r.scope.read(s, &r.token, r.projection)?;
    let sequence = require_scope(s, &r.scope)?;
    let c = catalog(&s.project)?;
    if r.offset > c.rows.len() {
        return Err(error("bin page offset exceeds the complete inventory"));
    }
    let end = r.offset.saturating_add(r.limit).min(c.rows.len());
    let page = c.rows.get(r.offset..end).ok_or_else(|| error("invalid bin page"))?;
    let result = json!({"status":"read","token":r.token,"scope":r.scope,"revision":s.revision,"sequence":sequence.map(|id|id.0),"sha256":c.sha256,
        "offset":r.offset,"total":c.rows.len(),"nextOffset":if end<c.rows.len(){Some(end)}else{None},"bins":c.bins,"items":c.items,"savedSearchBins":s.project.search_bins.len(),
        "entries":page,"referenceScope":"physical_project_tree","requiresSourceValidation":r.scope.source_validation_owed(r.projection)});
    if project_audit::value_bytes(&result)?.len() > 512 * 1024 {
        return Err(error("bin page exceeds 512 KiB; reduce page size"));
    }
    Ok(result)
}
#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
enum Edit {
    Create { name: String, parent: Option<u64> },
    Rename { bin: u64, name: String },
    Move { entries: Vec<u64>, parent: Option<u64> },
    Group { entries: Vec<u64>, name: Option<String> },
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Request {
    scope: organization_scope::Scope,
    before_sha256: String,
    edit: Edit,
}
pub(super) fn validate(op: &Operation) -> Result<()> {
    let r: Request = parse(COMMAND, &op.params)?;
    let parent_present = op.params.get("edit").is_some_and(|e| e.get("parent").is_some());
    let valid = match &r.edit {
        Edit::Create { name: n, parent } => name(n) && parent.is_none_or(|n| n <= SAFE) && parent_present,
        Edit::Rename { bin, name: n } => *bin <= SAFE && name(n),
        Edit::Group { entries, name: n } => {
            op.params.get("edit").is_some_and(|edit| edit.get("name").is_some())
                && n.as_deref().is_none_or(name)
                && !entries.is_empty()
                && entries.len() <= 4096
                && entries.iter().all(|n| *n > 0 && *n <= SAFE)
                && entries.iter().collect::<BTreeSet<_>>().len() == entries.len()
        }
        Edit::Move { entries, parent } => {
            !entries.is_empty()
                && entries.len() <= 4096
                && entries.iter().all(|n| *n > 0 && *n <= SAFE)
                && entries.iter().collect::<BTreeSet<_>>().len() == entries.len()
                && parent.is_none_or(|n| n <= SAFE)
                && parent_present
        }
    };
    if !r.scope.valid() || !valid || r.before_sha256.len() != 64 || !r.before_sha256.bytes().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()) {
        return Err(error("one explicit bounded organization gesture and complete current tree digest required"));
    }
    Ok(())
}
fn bin(c: &Catalog, id: u64) -> Result<BinId> {
    c.rows.iter().find(|r| r.id == id && r.kind == "bin").map(|r| BinId(r.id)).ok_or_else(|| error("target is not a current physical bin"))
}
fn group(project: &mut Project, before: &Catalog, entries: &[u64], name: Option<&str>) -> Result<(BinId, usize)> {
    for id in entries {
        if !before.rows.iter().any(|row| row.id == *id && row.visible && row.parent.is_some()) {
            return Err(error("each group entry must be current and human-visible; root cannot group"));
        }
    }
    let ids = entries.iter().map(|id| ItemId(*id)).collect::<Vec<_>>();
    let result = project_organization::group_selection(project, &ids, name, true)?;
    if project.next_id > SAFE {
        return Err(error("new group bin identity exceeds exact transport"));
    }
    Ok(result)
}

// Validate the closed new gesture against its shared pure native predictor.
// No Session command, history publication or saved recipe is replayed here.
pub(super) fn validate_group_history(before: &Project, after: &Project, op: &Operation, result: &Value) -> Result<()> {
    let request: Request = parse(COMMAND, &op.params)?;
    if let Edit::Group { entries, name } = request.edit {
        validate(op)?;
        let catalog = catalog(before)?;
        let mut expected = before.clone();
        let (bin, moved) = group(&mut expected, &catalog, &entries, name.as_deref())?;
        let fields = ["beforeSha256", "afterSha256", "entries", "moved", "humanCommand", "scope"];
        if result.as_object().is_none_or(|object| object.len() != fields.len() || fields.iter().any(|key| !object.contains_key(*key)))
            || request.before_sha256 != catalog.sha256
            || result["beforeSha256"] != catalog.sha256
            || result["afterSha256"] != project_summary(after)["sha256"]
            || result["humanCommand"] != "file.newBinFromSelection"
            || result["scope"] != "physical_project_tree"
            || project_audit::bytes(&expected)? != project_audit::bytes(after)?
            || result["entries"] != json!([bin.0])
            || result["moved"] != json!(moved)
        {
            return Err(error("historical group differs from its exact shared human gesture"));
        }
    }
    Ok(())
}

pub(super) fn run(s: &mut Session, op: &Operation) -> Result<Value> {
    validate(op)?;
    let r: Request = parse(COMMAND, &op.params)?;
    require_scope(s, &r.scope)?;
    let before = catalog(&s.project)?;
    if before.sha256 != r.before_sha256 {
        return Err(error("complete project tree changed since its read"));
    }
    let mut expected = (*s.project).clone();
    let editor = project_audit::value_bytes(&s.state)?;
    let (command, args, entries, moved) = match r.edit {
        Edit::Create { name, parent } => {
            let parent = parent.map(|id| bin(&before, id)).transpose()?;
            let id = project_organization::create(&mut expected, &name, parent)?;
            if expected.next_id > SAFE {
                return Err(error("new bin identity exceeds exact transport"));
            }
            ("file.newBin", json!({"name":name,"parent":parent.map(|id|id.0)}), vec![id.0], 0)
        }
        Edit::Group { entries, name } => {
            let (bin, moved) = group(&mut expected, &before, &entries, name.as_deref())?;
            ("file.newBinFromSelection", json!({"items":entries,"name":name}), vec![bin.0], moved)
        }
        Edit::Rename { bin: target, name } => {
            let id = bin(&before, target)?;
            project_organization::rename(&mut expected, id, &name)?;
            ("project.renameBin", json!({"bin":target,"name":name}), vec![target], 0)
        }
        Edit::Move { entries, parent } => {
            let parent = parent.map(|id| bin(&before, id)).transpose()?;
            for id in &entries {
                if !before.rows.iter().any(|r| r.id == *id && r.visible && r.parent.is_some()) {
                    return Err(error("each explicit move entry must be current and human-visible; root cannot move"));
                }
            }
            let ids = entries.iter().map(|n| ItemId(*n)).collect::<Vec<_>>();
            let moved = project_organization::move_entries(&mut expected, &ids, parent, true)?;
            ("project.moveToBin", json!({"items":entries,"bin":parent.map(|id|id.0)}), entries, moved)
        }
    };
    let after = catalog(&expected)?;
    if before.sha256 == after.sha256 {
        return Err(error("organization gesture makes no change"));
    }
    let bytes = project_audit::bytes(&expected)?;
    s.execute(command, args)?;
    if project_audit::bytes(&s.project)? != bytes || project_audit::value_bytes(&s.state)? != editor {
        return Err(error("human organization changed unexpected project or editor collateral; staging discarded"));
    }
    Ok(
        json!({"beforeSha256":r.before_sha256,"afterSha256":after.sha256,"entries":entries,"moved":moved,"humanCommand":command,"scope":"physical_project_tree"}),
    )
}
