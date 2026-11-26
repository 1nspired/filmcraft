//! One exact global font gesture through the original human service and ledger.
//! Historical hashes describe a past sampled result; they never qualify current fonts.
use super::{Batch, Operation, document_authority, parse, project_audit, project_font_resources, project_plane};
use crate::{Result, Session, commands::bad, project_workspace::ProjectScope};
use filmcraft_project::Project;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
pub(super) const COMMAND: &str = "agent.replaceProjectFonts";
fn error(m: impl Into<String>) -> crate::EngineError {
    bad(COMMAND, m)
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Proof {
    version: u8,
    read_token: String,
    proposal_sha256: String,
    before_project_sha256: String,
    after_project_sha256: String,
    collateral_sha256: String,
    before_resources_sha256: String,
    after_resources_sha256: String,
    sampling_sha256: String,
    eligibility_sha256: String,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Request {
    scope: ProjectScope,
    before_project_sha256: String,
    replacement: Value,
    font_plan: Proof,
}
fn hash(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
fn request(op: &Operation) -> Result<Request> {
    let r: Request = parse(COMMAND, &op.params)?;
    let p = &r.font_plan;
    if op.command != COMMAND
        || !crate::project_workspace::valid_scope(&r.scope)
        || !hash(&r.before_project_sha256)
        || p.version != 1
        || !super::token_ok(&p.read_token)
        || p.before_project_sha256 != r.before_project_sha256
        || p.before_project_sha256 == p.after_project_sha256
        || [
            &p.proposal_sha256,
            &p.before_project_sha256,
            &p.after_project_sha256,
            &p.collateral_sha256,
            &p.before_resources_sha256,
            &p.after_resources_sha256,
            &p.sampling_sha256,
            &p.eligibility_sha256,
        ]
        .iter()
        .any(|v| !hash(v))
    {
        return Err(error("exact current Root font proposal and complete native resource proof required"));
    }
    Ok(r)
}
pub(super) fn validate(op: &Operation) -> Result<()> {
    request(op).map(|_| ())
}
pub(super) fn is_batch(b: &Batch) -> bool {
    b.operations.iter().any(|op| op.command == COMMAND)
}
pub(super) fn is_commit(t: &super::Ticket) -> bool {
    t.committed.as_ref().is_some_and(|(request, result)| {
        result["action"] == "project_font_replace" && request["operations"].as_array().is_some_and(|ops| ops.len() == 1 && ops[0]["command"] == COMMAND)
    })
}
fn proof_value(p: &Proof) -> Result<Value> {
    serde_json::to_value(p).map_err(|e| error(e.to_string()))
}
fn live(project: &Project, r: &Request, token: &str, check: &dyn Fn() -> Result<()>) -> Result<project_font_resources::Binding> {
    if r.font_plan.read_token != token {
        return Err(error("font plan belongs to a different exact Root read"));
    }
    check()?;
    let b = project_font_resources::binding(project, token, &r.scope, &r.replacement, true, check)?;
    if !b.eligible || b.proof != proof_value(&r.font_plan)? {
        return Err(error("current global authored scope, locks/protection/tracking or native font resources changed; refresh"));
    }
    check()?;
    Ok(b)
}
fn receipt(r: &Request, b: &project_font_resources::Binding) -> Result<Value> {
    Ok(
        json!({"scope":r.scope,"beforeSha256":r.font_plan.before_project_sha256,"afterSha256":r.font_plan.after_project_sha256,"replacement":r.replacement,"collateralSha256":r.font_plan.collateral_sha256,"totals":b.totals,"fontResources":proof_value(&r.font_plan)?,"humanCommand":"file.replaceFonts","humanResult":b.human_result,"resourceEvidence":"historical_native_samples","allFramesVerified":false,"fontFileState":"not-inspected","embeddingRights":"not-established"}),
    )
}
pub(super) fn run(s: &mut Session, op: &Operation, token: &str, check: &dyn Fn() -> Result<()>) -> Result<Value> {
    let r = request(op)?;
    let b = live(&s.project, &r, token, check)?;
    let expected = receipt(&r, &b)?;
    let editor = super::project_clear::editor_hash(&s.state)?;
    check()?;
    let human = s.execute("file.replaceFonts", r.replacement.clone())?;
    if human != b.human_result
        || project_audit::bytes(&s.project)? != project_audit::bytes(&b.predicted)?
        || super::project_clear::editor_hash(&s.state)? != editor
    {
        return Err(error("isolated human font replacement differs from exact project/result/view prediction; no live publication"));
    }
    check()?;
    Ok(expected)
}
pub(super) fn validate_live_basis(s: &Session, op: &Operation, token: &str) -> Result<()> {
    let r = request(op)?;
    #[cfg(not(target_arch = "wasm32"))]
    let control = s.agent_bridge.operations.check();
    live(&s.project, &r, token, &|| {
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(check) = &control {
            check.check()?;
        }
        Ok(())
    })?;
    Ok(())
}
/// Pure historical forward validation: no renderer, font loader, source I/O,
/// native commands, stored recipe execution or renewal of past resource grants.
pub(super) fn validate_transition(before: &Project, after: &Project, op: &Operation, result: &Value) -> Result<()> {
    let r = request(op)?;
    document_authority::transition(before, after)?;
    let b = project_font_resources::binding(before, &r.font_plan.read_token, &r.scope, &r.replacement, false, &|| Ok(()))?;
    if !b.eligible {
        return Err(error("original global font scope was not completely editable"));
    }
    let proof = proof_value(&r.font_plan)?;
    for key in
        ["version", "readToken", "proposalSha256", "beforeProjectSha256", "afterProjectSha256", "collateralSha256", "samplingSha256", "eligibilitySha256"]
    {
        if b.proof[key] != proof[key] {
            return Err(error("historical native font authored/sampling/scope binding differs"));
        }
    }
    if project_audit::bytes(&b.predicted)? != project_audit::bytes(after)? || receipt(&r, &b)? != *result {
        return Err(error("font history differs from complete original human mutation and documentary receipt"));
    }
    Ok(())
}
pub(super) fn document_transition(before: &Project, after: &Project, b: &Batch, results: &[Value]) -> Result<()> {
    let op = b.operations.first().filter(|_| b.operations.len() == 1 && results.len() == 1).ok_or_else(|| error("one exact global font operation required"))?;
    let r = request(op)?;
    project_plane::validate_project_batch(&r.scope, b)?;
    if r.font_plan.read_token != b.token {
        return Err(error("global font batch and plan tokens differ"));
    }
    validate_transition(before, after, op, results.first().ok_or_else(|| error("font receipt missing"))?)
}
