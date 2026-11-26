//! Explicit owning-film association for a native workspace. This is project
//! metadata, not a source observation or permission to edit another film.
use crate::{
    Result, Session,
    commands::{CommandSpec, bad, has_seq},
};
use filmcraft_project::{ItemId, Project};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

pub(crate) const COMMAND: &str = "project.associateEditorialOwner";
pub(crate) const KEY: &str = "poema.workspace.v1";
pub(crate) const SEQUENCE_KEY: &str = "poema.sequence.v1";
#[derive(Clone, Deserialize, Serialize, PartialEq, Debug)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct Scope {
    pub(crate) project_id: String,
    pub(crate) anchor_sequence: u64,
}
pub(crate) fn valid_scope(scope: &Scope) -> bool {
    !scope.project_id.is_empty()
        && scope.project_id.len() <= 256
        && !scope.project_id.chars().any(char::is_control)
        && (1..=9_007_199_254_740_991).contains(&scope.anchor_sequence)
}
#[derive(Clone, Deserialize, Serialize, PartialEq, Debug)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct Association {
    kind: String,
    version: u32,
    pub(crate) workspace_id: String,
    pub(crate) scope: Scope,
}
pub(crate) fn uuid(value: &str) -> bool {
    value.len() == 36
        && value.bytes().enumerate().all(|(i, c)| if [8, 13, 18, 23].contains(&i) { c == b'-' } else { c.is_ascii_digit() || (b'a'..=b'f').contains(&c) })
}
fn error(message: impl Into<String>) -> crate::EngineError {
    bad(COMMAND, message)
}
pub(crate) fn association(project: &Project) -> Result<Option<Association>> {
    if project.items.len() > 100_000 {
        return Err(error("workspace inventory exceeds its bound"));
    }
    let mut found = None;
    for item in project.items.values() {
        let Some(raw) = item.metadata.get(KEY) else { continue };
        if raw.len() > 2048 || found.is_some() {
            return Err(error("oversized or duplicate owning-film association"));
        }
        let record: Association = serde_json::from_str(raw).map_err(|e| error(e.to_string()))?;
        if record.kind != "poema-native-workspace"
            || record.version != 1
            || !uuid(&record.workspace_id)
            || !valid_scope(&record.scope)
            || record.scope.anchor_sequence != item.id.0
            || item.as_sequence().is_none()
        {
            return Err(error("invalid owning-film association on the anchor sequence"));
        }
        found = Some(record);
    }
    if let Some(identity) = crate::project_workspace::identity(project)? {
        if found.as_ref().is_some_and(|record| record.scope.project_id != identity.scope.project_id || record.workspace_id != identity.scope.workspace_id) {
            return Err(error("sequence anchor disagrees with the saved project workspace identity"));
        }
        let retired = crate::project_workspace::retired_anchor_ids(project)?;
        if found.is_none() && crate::project_workspace::ordinary_sequences(project).next().is_some() && retired.is_empty() {
            return Err(error("project workspace has sequences but no owning anchor"));
        }
    }
    Ok(found)
}
pub(crate) fn matches(project: &Project, scope: &Scope, workspace: Option<&str>) -> Result<bool> {
    Ok(association(project)?.is_some_and(|r| r.scope == *scope && workspace.is_none_or(|w| w == r.workspace_id)))
}
pub(crate) fn validate(p: &Value) -> Result<Association> {
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase", deny_unknown_fields)]
    struct Request {
        scope: Scope,
        workspace_id: String,
    }
    let r: Request = serde_json::from_value(p.clone()).map_err(|e| error(e.to_string()))?;
    if !valid_scope(&r.scope) || !uuid(&r.workspace_id) {
        return Err(error("explicit film, anchor sequence and workspace UUID required"));
    }
    Ok(Association { kind: "poema-native-workspace".into(), version: 1, workspace_id: r.workspace_id, scope: r.scope })
}
fn run(s: &mut Session, params: &Value) -> Result<Value> {
    let wanted = validate(params)?;
    if s.state.active_sequence != Some(ItemId(wanted.scope.anchor_sequence)) || s.project.sequence(ItemId(wanted.scope.anchor_sequence)).is_none() {
        return Err(error("initial association requires the active anchor sequence"));
    }
    if let Some(existing) = association(&s.project)? {
        if existing != wanted {
            return Err(error("workspace already belongs to another film, anchor or workspace; reassignment is not supported"));
        }
        return Ok(json!({"association":existing,"changed":false,"sourceValidation":"not-performed","timelineChanged":false}));
    }
    // Initial setup must precede original-owner admission. Existing legacy
    // workspaces keep their bound-source path; this command never adopts one.
    if s.project.items.values().any(|it| it.metadata.contains_key("poema.binding.v1") || it.metadata.contains_key("poema.sequence.v1")) {
        return Err(error("associate the workspace before original binding or scoped sequence creation"));
    }
    let raw = serde_json::to_string(&wanted).map_err(|e| error(e.to_string()))?;
    // The same undoable Metadata service is used by the human and the guarded
    // native coordinator. No source, cut, allocation counter or view is edited.
    s.execute("metadata.set", json!({"item":wanted.scope.anchor_sequence,"field":KEY,"value":raw}))?;
    if association(&s.project)?.as_ref() != Some(&wanted) {
        return Err(error("owning-film association readback differs"));
    }
    Ok(json!({"association":wanted,"changed":true,"sourceValidation":"not-performed","timelineChanged":false}))
}
pub(crate) fn commands() -> Vec<CommandSpec> {
    vec![CommandSpec {
        id: COMMAND,
        label: "Associate Editorial Owner",
        menu: &[],
        shortcut: None,
        params: r#"{"scope":{"projectId":str,"anchorSequence":id},"workspaceId":uuid}"#,
        enabled: has_seq,
        run,
        journal: true,
    }]
}
