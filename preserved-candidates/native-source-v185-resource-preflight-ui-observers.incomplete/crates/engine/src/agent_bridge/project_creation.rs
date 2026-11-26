//! Trusted empty-project birth. Shared human constructor and native lifecycle;
//! never a live-project reset, source qualifier, history fork or model tool.
use super::*;
use crate::EngineError;
use serde::Deserialize;
use std::{
    fs,
    path::{Component, Path},
};
const CREATE: &str = "native.createProjectWorkspace";
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Startup {
    kind: String,
    version: u8,
    scope: crate::project_workspace::ProjectScope,
    workspace: String,
    update: String,
    name: String,
    project_path: String,
    activation_token: String,
    application_project: Option<application_admission::Descriptor>,
}
fn fail(message: impl Into<String>) -> EngineError {
    bad(CREATE, message)
}
pub(super) fn create(s: &mut Session, params: &Value) -> Result<Value> {
    native_editor::require_fresh(s)?;
    drop(history_checkpoint::bytes(params, 16384)?);
    let r: Startup = parse(CREATE, params)?;
    if r.kind != "poema-native-project-creation"
        || ![1, 2, 3].contains(&r.version)
        || !crate::project_workspace::valid_scope(&r.scope)
        || r.workspace != r.scope.workspace_id
        || !crate::editorial_workspace::uuid(&r.update)
        || !token_ok(&r.activation_token)
        || r.project_path.len() > 16384
        || r.project_path.chars().any(char::is_control)
    {
        return Err(fail("closed explicit new project/workspace/update identity and fresh activation token required"));
    }
    let target = Path::new(&r.project_path);
    let parent = target.parent().ok_or_else(|| fail("new project parent missing"))?;
    if !target.is_absolute()
        || !r.project_path.ends_with(".fcproj")
        || target.components().any(|c| matches!(c, Component::CurDir | Component::ParentDir))
        || fs::canonicalize(parent).map_err(|e| fail(e.to_string()))? != parent
    {
        return Err(fail("new absolute project path and canonical existing parent required"));
    }
    match fs::symlink_metadata(target) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(fail(e.to_string())),
        Ok(_) => return Err(fail("new project target must not exist; existing paths are never adopted or overwritten")),
    }
    if r.version == 1 && crate::commands::project_name_for(&r.project_path).as_deref() != Some(r.name.as_str()) {
        return Err(fail("new project name must match its native filename stem, as with human Save As"));
    }
    if r.version >= 2 && !safe_storage_name(target) {
        return Err(fail("version2 requires a bounded portable storage filename, independently of its display title"));
    }
    if (r.version == 3) != r.application_project.is_some() || r.version != 3 && params.get("applicationProject").is_some() {
        return Err(fail("application admission requires the exact version3 startup"));
    }
    let app = match r.application_project.clone() {
        Some(d) => Some(application_admission::prepare(d, &r.project_path, r.scope.clone(), r.update.clone())?),
        None => None,
    };
    let project = crate::project_workspace::new_project(&json!({"name":r.name,"workspace":r.scope,"preserveDisplayTitle":r.version >= 2}))?;
    let mut candidate = Session::new(s.services.clone());
    crate::commands::install_project(&mut candidate, project, Some(r.project_path.clone()), true);
    let raw = filmcraft_format::encode_with_view(&candidate.project, Some(&candidate.project_view()), false);
    if raw.is_empty() || raw.len() > 1024 * 1024 {
        return Err(fail("new empty project exceeds its native file bound"));
    }
    let hash = history_checkpoint::digest(&raw);
    candidate.saved_file = Some(crate::saved_file::Baseline::from_bytes(&r.project_path, &raw));
    // Nothing is installed in the caller until the complete exclusive file and
    // paused candidate exist. A failed publication retains inert evidence only.
    history_checkpoint::publish_new_native_file(target, &raw, &r.activation_token)?;
    workspace_update::hold_scoped_candidate(
        &mut candidate,
        organization_scope::Scope::Project(r.scope.clone()),
        r.workspace.clone(),
        r.update.clone(),
        r.activation_token,
        hash.clone(),
    )?;
    let epoch = history_checkpoint::epoch(&mut candidate)?;
    if let Some(app) = app {
        application_admission::install(&mut candidate, app, epoch.clone());
    }
    let mut receipt = json!({"status":"created","kind":"native_project_creation","version":r.version,"scope":r.scope,"workspace":r.workspace,"update":r.update,"projectPath":r.project_path,"name":r.name,"sha256":hash,"bytes":raw.len(),"schema":filmcraft_format::SCHEMA_VERSION,"nativeSession":epoch,"nativePid":std::process::id(),"revision":candidate.revision,"sequence":null,"editingPaused":true,"history":"fresh","sourceValidation":"not-performed","mediaEvidence":false,"writeAuthority":false,"execution":"project-created-paused"});
    if r.version == 3 {
        receipt["applicationProject"] = serde_json::to_value(&r.application_project).map_err(|_| fail("invalid application admission descriptor"))?;
    }
    workspace_update::install_project_creation_receipt(&mut candidate, receipt)?;
    let result = workspace_update::view(&candidate).ok_or_else(|| fail("new paused project fence missing"))?;
    *s = candidate;
    Ok(result)
}

fn safe_storage_name(path: &Path) -> bool {
    let Some(file) = path.file_name().and_then(|v| v.to_str()) else { return false };
    let Some(stem) = file.strip_suffix(".fcproj") else { return false };
    if stem.is_empty()
        || stem.len() > 96
        || !stem.as_bytes().first().is_some_and(u8::is_ascii_alphanumeric)
        || !stem.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'-')
    {
        return false;
    }
    let upper = stem.to_ascii_uppercase();
    !matches!(upper.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        && !["COM", "LPT"].iter().any(|p| upper.strip_prefix(p).is_some_and(|v| v.len() == 1 && v.bytes().all(|c| (b'1'..=b'9').contains(&c))))
}
