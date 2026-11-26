//! Saved-query definitions and live metadata matches; one exact shared human gesture.
use super::{Operation, organization_scope, parse, project_audit};
use crate::{EngineError, Result, Session, commands::bad, search_organization as shared};
use filmcraft_project::{BinId, FindQuery, Project, SearchBin};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
pub(super) const COMMAND: &str = "agent.editSearchBins";
const SAFE: u64 = 9_007_199_254_740_991;
fn error(s: impl Into<String>) -> EngineError {
    bad(COMMAND, s)
}
fn text(s: &str, max: usize) -> bool {
    s.len() <= max && !s.contains('\0')
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Row {
    column: String,
    operator: String,
    text: String,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Criteria {
    rows: Vec<Row>,
    match_all: bool,
    case_sensitive: bool,
}
impl Criteria {
    fn query(&self) -> Result<FindQuery> {
        if self.rows.len() > 32 || self.rows.iter().any(|r| !filmcraft_project::FindOp::ALL.iter().any(|op| op.name() == r.operator)) {
            return Err(error("explicit bounded canonical query rows required"));
        }
        shared::query_params(&serde_json::to_value(self).map_err(|e| error(e.to_string()))?, COMMAND)
    }
    fn from_query(q: &FindQuery) -> Result<Self> {
        shared::validate_query(q)?;
        Ok(Self {
            rows: q.rows.iter().map(|r| Row { column: r.column.clone(), operator: r.op.name().into(), text: r.text.clone() }).collect(),
            match_all: q.match_all,
            case_sensitive: q.case_sensitive,
        })
    }
}
#[derive(Serialize)]
struct Definition {
    bin: u64,
    name: String,
    criteria: Criteria,
}
fn definition(b: &SearchBin) -> Result<Definition> {
    if b.id.0 == 0 || b.id.0 > SAFE || !text(&b.name, 1024) {
        return Err(error("unsafe saved search identity or name"));
    }
    Ok(Definition { bin: b.id.0, name: b.name.clone(), criteria: Criteria::from_query(&b.query)? })
}
fn digest(v: &impl Serialize) -> Result<String> {
    Ok(format!("{:x}", Sha256::digest(project_audit::value_bytes(v)?)))
}
fn catalog(p: &Project) -> Result<(Vec<Definition>, String)> {
    shared::validate_inventory(p)?;
    let mut ids = BTreeSet::new();
    let mut rows = Vec::new();
    for b in &p.search_bins {
        let d = definition(b)?;
        if !ids.insert(d.bin) {
            return Err(error("ambiguous saved search"));
        }
        rows.push(d);
    }
    let sha = digest(&rows)?;
    Ok((rows, sha))
}
pub(super) fn summary(s: &Session) -> Value {
    project_summary(&s.project)
}
pub(super) fn project_summary(project: &filmcraft_project::Project) -> Value {
    match catalog(project) {
        Ok((rows, sha)) => json!({"status":"current","scope":"saved_search_bins","sha256":sha,"bins":rows.len()}),
        Err(e) => json!({"status":"unavailable","reason":e.to_string()}),
    }
}
fn require_scope(s: &Session, scope: &organization_scope::Scope) -> Result<Option<filmcraft_project::ItemId>> {
    scope.require(s)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Read {
    token: String,
    scope: organization_scope::Scope,
    query: Option<u64>,
    offset: usize,
    limit: usize,
    #[serde(default)]
    projection: bool,
}
pub(super) fn read(s: &mut Session, p: &Value) -> Result<Value> {
    let r: Read = parse("agent.readSearchBins", p)?;
    if p.get("query").is_none() || r.query.is_some_and(|id| id == 0 || id > SAFE) || r.offset > 100_000 || r.limit == 0 || r.limit > 32 {
        return Err(error("explicit query or null and bounded page required"));
    }
    r.scope.read(s, &r.token, r.projection)?;
    let sequence = require_scope(s, &r.scope)?;
    let (definitions, basis) = catalog(&s.project)?;
    let (mode, query, entries, sha) = if let Some(id) = r.query {
        let b = shared::get(&s.project, BinId(id))?;
        let def = definition(&b)?;
        let mut matches = Vec::new();
        for item in shared::results(&s.project, &b.query)? {
            let it = s.project.item(item).ok_or_else(|| error("matched item is unavailable"))?;
            if item.0 == 0 || item.0 > SAFE || !text(&it.name, 1024) {
                return Err(error("matched metadata exceeds its bound"));
            }
            matches.push(json!({"item":item.0,"name":it.name,"itemType":it.type_label()}));
        }
        let sha = digest(&(&def, &matches))?;
        ("matches", serde_json::to_value(def).map_err(|e| error(e.to_string()))?, matches, sha)
    } else {
        let entries = definitions.iter().map(serde_json::to_value).collect::<std::result::Result<Vec<_>, _>>().map_err(|e| error(e.to_string()))?;
        ("catalog", Value::Null, entries, basis.clone())
    };
    if r.offset > entries.len() {
        return Err(error("search offset exceeds complete inventory"));
    }
    let end = r.offset.saturating_add(r.limit).min(entries.len());
    let page = entries.get(r.offset..end).ok_or_else(|| error("invalid search page"))?;
    let result = json!({"status":"read","token":r.token,"scope":r.scope,"revision":s.revision,"sequence":sequence.map(|id|id.0),"basis":basis,"mode":mode,"query":query,"sha256":sha,"offset":r.offset,"total":entries.len(),"nextOffset":if end<entries.len(){Some(end)}else{None},"entries":page,"referenceScope":"saved_search_metadata","requiresSourceValidation":r.scope.source_validation_owed(r.projection)});
    if project_audit::value_bytes(&result)?.len() > 512 * 1024 {
        return Err(error("saved-search page exceeds512KiB; reduce page size"));
    }
    Ok(result)
}
#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
enum Edit {
    Create { name: String, criteria: Criteria },
    Update { bin: u64, name: String, criteria: Criteria },
    Delete { bin: u64 },
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
    if !r.scope.valid() || r.before_sha256.len() != 64 || !r.before_sha256.bytes().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()) {
        return Err(error("current complete saved-query basis required"));
    }
    match r.edit {
        Edit::Create { name, criteria } | Edit::Update { name, criteria, .. } => {
            if !text(&name, 1024) || name.trim().is_empty() {
                return Err(error("explicit bounded nonempty name required"));
            }
            criteria.query()?;
        }
        Edit::Delete { .. } => {}
    }
    if op.params.get("edit").and_then(|e| e.get("bin")).is_some_and(|v| v.as_u64().is_none_or(|n| n == 0 || n > SAFE)) {
        return Err(error("exact safe saved-query target required"));
    }
    Ok(())
}
pub(super) fn run(s: &mut Session, op: &Operation) -> Result<Value> {
    validate(op)?;
    let r: Request = parse(COMMAND, &op.params)?;
    require_scope(s, &r.scope)?;
    let (_, before) = catalog(&s.project)?;
    if before != r.before_sha256 {
        return Err(error("unpaged saved-query definitions changed"));
    }
    let mut expected = (*s.project).clone();
    let editor = project_audit::value_bytes(&s.state)?;
    let (command, args, id) = match r.edit {
        Edit::Create { name, criteria } => {
            let b = shared::create(&mut expected, Some(&name), criteria.query()?)?;
            let mut args = serde_json::to_value(criteria).map_err(|e| error(e.to_string()))?;
            args["name"] = json!(name);
            ("file.newSearchBin", args, b.id.0)
        }
        Edit::Update { bin, name, criteria } => {
            shared::update(&mut expected, BinId(bin), Some(&name), Some(criteria.query()?))?;
            let mut args = serde_json::to_value(criteria).map_err(|e| error(e.to_string()))?;
            args["name"] = json!(name);
            args["bin"] = json!(bin);
            ("project.editSearchBin", args, bin)
        }
        Edit::Delete { bin } => {
            shared::delete(&mut expected, BinId(bin))?;
            ("project.deleteSearchBin", json!({"bin":bin}), bin)
        }
    };
    if id == 0 || id > SAFE || expected.next_id > SAFE {
        return Err(error("saved-query identity exceeds exact transport"));
    }
    let (_, after) = catalog(&expected)?;
    if before == after {
        return Err(error("saved-query gesture makes no change"));
    }
    let bytes = project_audit::bytes(&expected)?;
    s.execute(command, args)?;
    if project_audit::bytes(&s.project)? != bytes || project_audit::value_bytes(&s.state)? != editor {
        return Err(error("human saved search changed unexpected project/editor collateral; staging discarded"));
    }
    Ok(json!({"beforeSha256":before,"afterSha256":after,"bin":id,"humanCommand":command,"scope":"saved_search_bins"}))
}
