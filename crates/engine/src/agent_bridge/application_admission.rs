//! Trusted application setup fence, separate from native activation/history.
//! A small private completion record is not a read ticket or source witness.
use super::*;
use crate::EngineError;
use serde::Deserialize;
use std::{
    fs,
    path::{Path, PathBuf},
};
pub(super) const STATUS: &str = "agent.readApplicationProjectAdmission";
#[derive(Clone, Debug, Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct Descriptor {
    kind: String,
    version: u8,
    operation_id: String,
    ready_file: String,
}
#[derive(Clone)]
pub(super) struct Binding {
    descriptor: Descriptor,
    scope: crate::project_workspace::ProjectScope,
    birth_operation_id: String,
    project_path: String,
    epoch: String,
    parent: PathBuf,
    ready_digest: std::sync::Arc<std::sync::OnceLock<String>>,
    #[cfg(unix)]
    parent_identity: (u64, u64),
    #[cfg(unix)]
    parent_handle: std::sync::Arc<fs::File>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Ready {
    kind: String,
    version: u8,
    operation_id: String,
    birth_operation_id: String,
    scope: crate::project_workspace::ProjectScope,
    project_path: String,
    native_pid: u32,
    native_session: String,
    checkpoint_sha256: String,
    fingerprint: String,
    source_validation: String,
    media_evidence: bool,
    write_authority: bool,
}
fn fail() -> EngineError {
    bad(STATUS, "exact private application readiness is unavailable; native setup remains paused")
}
fn sha(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
pub(super) fn prepare(d: Descriptor, project: &str, scope: crate::project_workspace::ProjectScope, birth: String) -> Result<Binding> {
    let path = Path::new(project);
    let parent = path.parent().and_then(Path::parent).ok_or_else(fail)?.to_path_buf();
    let ready = Path::new(&d.ready_file);
    if d.kind != "poema-native-application-admission"
        || d.version != 1
        || !crate::editorial_workspace::uuid(&d.operation_id)
        || d.ready_file.len() > 4096
        || ready != parent.join("native-admission-ready.json")
        || fs::canonicalize(&parent).map_err(|_| fail())? != parent
    {
        return Err(fail());
    }
    match fs::symlink_metadata(ready) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        _ => return Err(fail()),
    }
    #[cfg(unix)]
    let parent_handle = std::sync::Arc::new(fs::File::open(&parent).map_err(|_| fail())?);
    #[cfg(unix)]
    let parent_identity = {
        use std::os::unix::fs::MetadataExt;
        let m = parent_handle.metadata().map_err(|_| fail())?;
        if !m.is_dir() {
            return Err(fail());
        }
        (m.dev(), m.ino())
    };
    Ok(Binding {
        descriptor: d,
        scope,
        birth_operation_id: birth,
        project_path: project.into(),
        epoch: String::new(),
        parent,
        ready_digest: std::sync::Arc::new(std::sync::OnceLock::new()),
        #[cfg(unix)]
        parent_identity,
        #[cfg(unix)]
        parent_handle,
    })
}
pub(super) fn install(s: &mut Session, mut b: Binding, epoch: String) {
    b.epoch = epoch;
    s.agent_bridge.application_admission = Some(b);
}
fn parent_current(b: &Binding) -> Result<()> {
    if fs::canonicalize(&b.parent).map_err(|_| fail())? != b.parent
        || Path::new(&b.project_path).parent().and_then(|p| fs::canonicalize(p).ok()).as_deref() != Path::new(&b.project_path).parent()
    {
        return Err(fail());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let m = fs::metadata(&b.parent).map_err(|_| fail())?;
        let pinned = b.parent_handle.metadata().map_err(|_| fail())?;
        if !pinned.is_dir() || (pinned.dev(), pinned.ino()) != b.parent_identity || (m.dev(), m.ino()) != b.parent_identity {
            return Err(fail());
        }
    }
    Ok(())
}
#[inline(never)]
fn ready(s: &Session, b: &Binding) -> Result<(Ready, String)> {
    if s.agent_bridge.epoch.as_deref() != Some(b.epoch.as_str())
        || b.ready_digest.get().is_none()
            && (s.path.as_deref() != Some(b.project_path.as_str())
                || !crate::project_workspace::matches_identity(&s.project, &b.scope.project_id, &b.scope.workspace_id)?.unwrap_or(false))
    {
        return Err(fail());
    }
    parent_current(b)?;
    let raw = history_checkpoint::private_native_bytes(Path::new(&b.descriptor.ready_file), 4096, None).map_err(|_| fail())?;
    let r: Ready = serde_json::from_slice(&raw).map_err(|_| fail())?;
    if r.kind != "poema-native-application-ready"
        || r.version != 1
        || r.operation_id != b.descriptor.operation_id
        || r.birth_operation_id != b.birth_operation_id
        || r.scope != b.scope
        || r.project_path != b.project_path
        || r.native_pid != std::process::id()
        || r.native_session != b.epoch
        || !sha(&r.checkpoint_sha256)
        || !sha(&r.fingerprint)
        || r.source_validation != "not-performed"
        || r.media_evidence
        || r.write_authority
    {
        return Err(fail());
    }
    parent_current(b)?;
    let digest = history_checkpoint::digest(&raw);
    // First admission pins the exact private bytes, including its checkpoint.
    // A structurally valid replacement cannot renew readiness after admission.
    if b.ready_digest.get_or_init(|| digest.clone()) != &digest {
        return Err(fail());
    }
    Ok((r, digest))
}
#[inline(never)]
pub(super) fn held(s: &Session) -> bool {
    s.agent_bridge.application_admission.as_ref().is_some_and(|b| ready(s, b).is_err())
}
pub(super) fn status(s: &Session) -> Option<Value> {
    let b = s.agent_bridge.application_admission.as_ref()?;
    let completed = ready(s, b).ok();
    Some(
        json!({"kind":"poema-native-application-admission-status","version":1,"operationId":b.descriptor.operation_id,"birthOperationId":b.birth_operation_id,"scope":b.scope,"projectPath":b.project_path,"readyFile":b.descriptor.ready_file,"nativePid":std::process::id(),"nativeSession":b.epoch,"status":if completed.is_some(){"ready"}else{"held"},"admitted":b.ready_digest.get().is_some(),"readySha256":completed.as_ref().map(|v|&v.1),"checkpointSha256":completed.as_ref().map(|v|&v.0.checkpoint_sha256),"sourceValidation":"not-performed","mediaEvidence":false,"writeAuthority":false}),
    )
}
pub(super) fn command_allowed(s: &Session, command: &str) -> Result<()> {
    if held(s)
        && !matches!(
            command,
            STATUS
                | "agent.readProject"
                | "agent.validateProjectRead"
                | "agent.workspaceUpdateStatus"
                | "agent.beginWorkspaceUpdate"
                | "agent.cancelWorkspaceUpdate"
                | "agent.activateWorkspaceUpdate"
                | "agent.refreshWorkspaceActivation"
                | "agent.readHistoryCheckpointBasis"
                | "agent.saveHistoryCheckpoint"
                | "agent.stageProjectRecovery"
                | "agent.prepareProjectRecovery"
                | "agent.validateRetainedHistory"
                | super::workspace_update::INSPECT_PAUSED_HISTORY
        )
    {
        return Err(fail());
    }
    Ok(())
}
fn read(s: &mut Session, p: &Value) -> Result<Value> {
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase", deny_unknown_fields)]
    struct Address {
        scope: crate::project_workspace::ProjectScope,
        operation_id: String,
        project_path: String,
    }
    let r: Address = serde_json::from_value(p.clone()).map_err(|_| fail())?;
    let b = s.agent_bridge.application_admission.as_ref().ok_or_else(fail)?;
    if r.scope != b.scope || r.operation_id != b.descriptor.operation_id || r.project_path != b.project_path {
        return Err(fail());
    }
    status(s).ok_or_else(fail)
}
pub(super) fn commands() -> Vec<CommandSpec> {
    vec![CommandSpec {
        id: STATUS,
        label: "Read Native Application Setup Admission",
        menu: &[],
        shortcut: None,
        params: r#"{scope:{projectId,workspaceId},operationId:uuid,projectPath:absolute}"#,
        enabled: always,
        run: read,
        journal: false,
    }]
}
