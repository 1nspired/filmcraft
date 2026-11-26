//! Fresh native geometry/context using retained EXPECTED file generations.
//! No filesystem observation, current footage claim or generic edit executor.
use super::*;
const PROJECT: &str = "agent.projectDocument";
const QUALIFIED: &str = "agent.qualifiedDocument";
#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct Projection {
    token: String,
    basis: String,
    media_scope: media_scope::Request,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Qualified {
    token: String,
    validation: String,
}
fn project(s: &mut Session, p: &Value) -> Result<Value> {
    let r: Projection = parse(PROJECT, p)?;
    if !token_ok(&r.basis) || r.basis == r.token {
        return Err(bad(PROJECT, "distinct valid native basis and fresh document token required"));
    }
    fresh_snapshot(s, &r.token)?;
    let sequence = s.state.active_sequence.ok_or(crate::EngineError::NoSequence)?;
    let items = media_scope::resolve(&s.project, sequence, s.state.source_item, &r.media_scope)?;
    let current = media_guard::Plan::scoped(&s.project, &s.media, items.clone())?;
    let needs = |reason: &str| Ok(json!({"status":"needs_qualification","token":r.token,"basis":r.basis,"reason":reason}));
    let Some(basis) = s.agent_bridge.tickets.iter().find(|t| t.token == r.basis && t.path == s.path) else {
        return needs("basis_expired");
    };
    let Some(old) = basis.project.upgrade() else {
        return needs("basis_expired");
    };
    let Ok(previous) = media_guard::Plan::scoped(&old, &s.media, items.clone()) else {
        return needs("source_declarations_changed");
    };
    if previous != current
        || items.iter().any(|id| {
            old.item(filmcraft_project::ItemId(*id)).and_then(|i| i.metadata.get(BINDING_KEY))
                != s.project.item(filmcraft_project::ItemId(*id)).and_then(|i| i.metadata.get(BINDING_KEY))
        })
    {
        return needs("source_declarations_changed");
    }
    let Some(expected) = media_guard::Witnesses::retained_subset(&basis.media, items) else {
        return needs("expected_generations_unavailable");
    };
    // Only the current document/context becomes fresh. Preserve native expected
    // bytes for the unchanged final guards; never replace the observed cache.
    super::snapshot_document(s, r.token, expected, false)
}
fn qualified(s: &mut Session, p: &Value) -> Result<Value> {
    let r: Qualified = parse(QUALIFIED, p)?;
    if !token_ok(&r.token) || !token_ok(&r.validation) {
        return Err(bad(QUALIFIED, "exact document token and fresh native validation delivery required"));
    }
    super::read_jobs::qualified_document(s, &r.token, &r.validation)
}
pub(super) fn commands() -> Vec<CommandSpec> {
    vec![
        CommandSpec {
            id: PROJECT,
            label: "Project Current Native Document",
            menu: &[],
            shortcut: None,
            params: r#"{token:freshRandomString,basis:readToken,mediaScope}"#,
            enabled: always,
            run: project,
            journal: false,
        },
        CommandSpec {
            id: QUALIFIED,
            label: "Read Qualified Native Document",
            menu: &[],
            shortcut: None,
            params: r#"{token:readToken,validation:deliveredJob}"#,
            enabled: always,
            run: qualified,
            journal: false,
        },
    ]
}
