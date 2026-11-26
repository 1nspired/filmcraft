//! Complete immutable Root snapshot through shared native history/lifecycle services.
use super::super::{native_editor, project_recovery};
use super::*;
mod recovery_dependencies;
#[cfg(any(target_vendor = "apple", target_os = "linux"))]
mod save_startup;

#[cfg(any(target_vendor = "apple", target_os = "linux"))]
pub(super) fn current_dependencies(s: &Session, projects: &[String], check: &media_guard::Check) -> Result<Value> {
    recovery_dependencies::current_dependencies(s, projects, check)
}
const PREPARE: &str = "agent.prepareProjectRecovery";
pub(super) const STAGE: &str = "agent.stageProjectRecovery";
const KIND: &str = "poema-native-project-recovery";
const LIMIT: usize = 80 * 1024 * 1024;
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Stage {
    token: String,
    scope: crate::project_workspace::ProjectScope,
    workspace: String,
    update: String,
    pause_token: String,
    checkpoint: String,
    checkpoint_sha256: String,
    #[serde(default)]
    expected_sha256: Option<String>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Prepare {
    token: String,
    stage_token: String,
    scope: crate::project_workspace::ProjectScope,
    workspace: String,
    update: String,
    pause_token: String,
    checkpoint: String,
    checkpoint_sha256: String,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Snapshot {
    kind: String,
    version: u8,
    schema: u32,
    scope: crate::project_workspace::ProjectScope,
    workspace: String,
    update: String,
    stage_token: String,
    source_pid: u32,
    history_sha256: String,
    history: Capsule,
    editor: native_editor::Snapshot,
    basis: Value,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Envelope {
    payload: Snapshot,
    sha256: String,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Startup {
    project_path: String,
    scope: crate::project_workspace::ProjectScope,
    workspace: String,
    update: String,
    sha256: String,
    activation_token: String,
    #[serde(default)]
    latest_journal: bool,
    #[serde(default)]
    restoration: Option<Value>,
    #[serde(default)]
    current_image: Option<Value>,
}
fn fail(v: impl Into<String>) -> crate::EngineError {
    bad(STAGE, v)
}
fn hash_ok(v: &str) -> bool {
    v.len() == 64 && v.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
fn address(s: &Session, scope: &crate::project_workspace::ProjectScope, workspace: &str, update: &str) -> Result<PathBuf> {
    if !crate::project_workspace::valid_scope(scope) || scope.workspace_id != workspace || !crate::editorial_workspace::uuid(update) {
        return Err(fail("exact Root scope/workspace and snapshot UUID required"));
    }
    Ok(workspace_directory(s)?.join(format!(".poema-project-recovery-{workspace}-{update}.json")))
}
fn request(scope: &crate::project_workspace::ProjectScope, workspace: &str, checkpoint: &str, token: &str) -> Request {
    Request {
        token: token.into(),
        scope: CheckpointScope::Project(scope.clone()),
        workspace: workspace.into(),
        checkpoint: checkpoint.into(),
        sha256: None,
        metadata_basis: true,
    }
}
fn validate_header(b: &Snapshot) -> Result<()> {
    if b.kind != KIND
        || b.version != 1
        || b.schema != filmcraft_format::SCHEMA_VERSION
        || !crate::project_workspace::valid_scope(&b.scope)
        || b.scope.workspace_id != b.workspace
        || !crate::editorial_workspace::uuid(&b.update)
        || !token_ok(&b.stage_token)
        || b.source_pid == 0
        || b.history.scope != CheckpointScope::Project(b.scope.clone())
        || b.history.workspace != b.workspace
        || b.history.revision == 0
        || b.history.revision >= u64::MAX - 1
        || !hash_ok(&b.history_sha256)
        || digest(&bytes(&b.history, MAX_BYTES)?) != b.history_sha256
        || b.basis["kind"] != "poema-native-project-recovery-basis"
        || b.basis["version"] != 1
        || b.basis["scope"] != serde_json::to_value(&b.scope).map_err(|e| fail(e.to_string()))?
        || b.basis["saved_file"]["path"] != b.history.path
        || !b.basis["saved_file"]["sha256"].as_str().is_some_and(hash_ok)
        || b.basis["native_session"] != b.history.epoch
        || b.basis["revision"] != b.history.revision
        || b.basis["saved_revision"] != b.editor.saved_revision
        || b.basis["sequence"] != json!(b.history.sequence)
        || b.basis["project_sha256"] != digest(b.history.projects.first().map_or(&[], |v| v.as_bytes()))
        || !b.basis["preferences_file"].is_object()
    {
        return Err(fail("incompatible or crossed complete project recovery snapshot"));
    }
    b.editor.validate(b.history.revision, b.history.sequence)
}
fn load(
    s: &Session,
    scope: &crate::project_workspace::ProjectScope,
    workspace: &str,
    update: &str,
    hash: &str,
    check: &media_guard::Check,
) -> Result<(Snapshot, usize)> {
    if !hash_ok(hash) {
        return Err(fail("exact immutable snapshot checksum required"));
    }
    let raw = private_native_bytes(&address(s, scope, workspace, update)?, LIMIT, Some(check))?;
    if digest(&raw) != hash {
        return Err(fail("complete project snapshot changed; nothing recovered"));
    }
    let envelope: Envelope = serde_json::from_slice(&raw).map_err(|e| fail(e.to_string()))?;
    if bytes(&envelope, LIMIT)? != raw || digest(&bytes(&envelope.payload, LIMIT)?) != envelope.sha256 {
        return Err(fail("snapshot checksum or canonical native values changed"));
    }
    let b = envelope.payload;
    validate_header(&b)?;
    if b.scope != *scope || b.workspace != workspace || b.update != update || Some(&b.history.path) != s.path.as_ref() {
        return Err(fail("snapshot belongs to another native project or owner"));
    }
    Ok((b, raw.len()))
}
// Complete original snapshot comparison inside the same guarded read. This
// neither retires a process nor installs a Save owner or a recovery key.
pub(super) fn validate_paused(
    s: &Session,
    scope: &crate::project_workspace::ProjectScope,
    update: &str,
    checkpoint: &str,
    hashes: (&str, &str),
) -> Result<Value> {
    let check = s.agent_bridge.operations.check().ok_or_else(|| fail("original paused inspection operation required"))?;
    check.check()?;
    if s.save_operation_held() {
        return Err(fail("a pending native Save cannot supply a paused snapshot comparison"));
    }
    workspace_update::require_idle_writer_jobs(s)?;
    let (b, size) = load(s, scope, &scope.workspace_id, update, hashes.1, &check)?;
    if !workspace_update::matches_candidate_recovery_basis(s, &b.basis)
        || b.history.checkpoint != checkpoint
        || b.history_sha256 != hashes.0
        || b.history.revision != s.revision
        || !b.editor.matches(s)?
    {
        return Err(fail("the actual paused candidate differs from its complete original snapshot"));
    }
    project_recovery::validate(&b.basis)?;
    check.check()?;
    Ok(json!({"kind":"native_paused_project_snapshot","version":1,"sha256":hashes.1,"bytes":size,
        "historySha256":b.history_sha256,"sourceNativeSession":b.history.epoch,"sourceNativePid":b.source_pid,
        "revision":b.history.revision,"sequence":b.history.sequence,"editorUnchanged":true,
        "projectUnchanged":true,"historyUnchanged":true,"retirementAuthority":false,"writeAuthority":false}))
}
fn receipt(b: &Snapshot, path: &Path, hash: &str, size: usize) -> Value {
    json!({"status":"staged","kind":"native_project_recovery_snapshot","version":1,"scope":b.scope,"workspace":b.workspace,"update":b.update,"token":b.stage_token,"nativeSession":b.history.epoch,"nativePid":b.source_pid,"revision":b.history.revision,"sequence":b.history.sequence,"projectPath":b.history.path,"path":path,"sha256":hash,"bytes":size,"checkpoint":b.history.checkpoint,"checkpointSha256":b.history_sha256,"savedFileSha256":b.basis["saved_file"]["sha256"],"sourceValidation":"not-performed","mediaEvidence":false,"writeAuthority":false,"execution":"snapshot-staged"})
}
// Preparation and publication serialize the same complete native values. The
// manager can durably record the whole-file digest before any artifact exists;
// preparation grants no edit/media authority and never reserves a stage slot.
fn capture(s: &mut Session, r: &Stage, stage_token: &str) -> Result<(Envelope, Vec<u8>)> {
    if !s.worker_available
        || !operation_cancellation_available(s)
        || !workspace_update::paused_project_owner(s, &r.scope)
        || r.workspace != r.scope.workspace_id
        || !token_ok(&r.token)
        || !crate::editorial_workspace::uuid(&r.checkpoint)
        || !hash_ok(&r.checkpoint_sha256)
    {
        return Err(fail("paused independent native owner, exact saved history and independent Stop required"));
    }
    let pause = workspace_update::view(s).ok_or_else(|| fail("Root source fence required"))?;
    if pause["workspace"] != r.workspace || pause["update"] != r.update || pause["token"] != r.pause_token {
        return Err(fail("snapshot belongs to a different paused source"));
    }
    if !pause["stage"].is_null() {
        return Err(fail("snapshot publication already attempted; inspect instead of preparing or replaying"));
    }
    let scope = CheckpointScope::Project(r.scope.clone());
    retained_scoped_basis(s, &r.token, &scope)?;
    let check = s.agent_bridge.operations.check().ok_or_else(|| fail("current one-attempt snapshot operation required"))?;
    check.check()?;
    let req = request(&r.scope, &r.workspace, &r.checkpoint, &r.token);
    let (history, _) = super::load(&path(s, &req)?, &r.checkpoint_sha256)?;
    if s.agent_bridge.epoch.as_deref() != Some(history.epoch.as_str()) {
        return Err(fail("source checkpoint belongs to another native epoch"));
    }
    prepare_restore(s, &req, &history, &r.checkpoint_sha256, s.state.active_sequence)?;
    let basis = project_recovery::capture_held(s, &serde_json::to_value(&r.scope).map_err(|e| fail(e.to_string()))?)?;
    let snapshot = Snapshot {
        kind: KIND.into(),
        version: 1,
        schema: filmcraft_format::SCHEMA_VERSION,
        scope: r.scope.clone(),
        workspace: r.workspace.clone(),
        update: r.update.clone(),
        stage_token: stage_token.into(),
        source_pid: std::process::id(),
        history_sha256: r.checkpoint_sha256.clone(),
        history,
        editor: native_editor::Snapshot::capture_complete(s)?,
        basis,
    };
    let (envelope, raw) = encode_snapshot(s, snapshot, Some(&check), None)?;
    retained_scoped_basis(s, &r.token, &scope)?;
    check.check()?;
    Ok((envelope, raw))
}
fn encode_snapshot(
    s: &Session,
    snapshot: Snapshot,
    check: Option<&media_guard::Check>,
    save_key: Option<&crate::project_save::operation::Key>,
) -> Result<(Envelope, Vec<u8>)> {
    validate_header(&snapshot)?;
    let payload = bytes_checked(&snapshot, LIMIT, check)?;
    let envelope = Envelope { payload: snapshot, sha256: digest(&payload) };
    let raw = bytes_checked(&envelope, LIMIT, check)?;
    let roundtrip: Envelope = serde_json::from_slice(&raw).map_err(|e| fail(e.to_string()))?;
    if bytes_checked(&roundtrip, LIMIT, check)? != raw {
        return Err(fail("complete native snapshot would lose data during roundtrip"));
    }
    let snapshot = &envelope.payload;
    if let Some(key) = save_key {
        let check = check.ok_or_else(|| fail("original cancellable Save recovery read required"))?;
        project_recovery::validate_save_live(s, &snapshot.basis, key, check)?;
    } else {
        project_recovery::validate_live(s, &snapshot.basis)?;
    }
    if let Some(check) = check {
        check.check()?;
    }
    Ok((envelope, raw))
}

// Complete opaque native values remain behind the same live Save owner. They
// are not a staged file, imported Key, startup grant or publication admission.
pub(in crate::agent_bridge) struct SaveRecovery {
    envelope: Envelope,
    raw: Vec<u8>,
    owner: crate::project_save::operation::RecoveryOwner,
    project_read_sha256: String,
    #[cfg(any(target_vendor = "apple", target_os = "linux"))]
    private_record: Option<filmcraft_format::retained::private_record::Record>,
}
impl SaveRecovery {
    pub(in crate::agent_bridge) fn current(&self, s: &Session, key: &crate::project_save::operation::Key, check: &media_guard::Check) -> Result<()> {
        self.owner.current(s, key, &|| check.check())?;
        project_recovery::validate_save_live(s, &self.envelope.payload.basis, key, check)?;
        self.owner.current(s, key, &|| check.check())
    }
    pub(in crate::agent_bridge) fn describe(&self, job: &str, operation: &str) -> Value {
        let b = &self.envelope.payload;
        json!({"kind":"native_Save_recovery_read","version":1,"job":job,"operation":operation,"scope":b.scope,
          "nativeSession":b.history.epoch,"nativePid":b.source_pid,"revision":b.history.revision,"sequence":b.history.sequence,
          "projectPath":b.history.path,"phase":self.owner.phase(),"capture":b.update,"checkpoint":b.history.checkpoint,
          "sha256":digest(&self.raw),"bytes":self.raw.len(),"historySha256":b.history_sha256,"projectSha256":b.basis["project_sha256"],"projectReadSha256":self.project_read_sha256,
          "staged":false,"current":false,"writeAuthority":false,"nativeComplete":false,"startupAuthority":false,"workersMayResume":false})
    }
    // Shared original-key composition. The managed owner durably binds the
    // complete intent before the finite publication call reaches this writer.
    #[cfg(any(target_vendor = "apple", target_os = "linux"))]
    pub(in crate::agent_bridge) fn prepare_private_record(
        &mut self,
        s: &Session,
        key: &crate::project_save::operation::Key,
        reserve: u64,
        check: &media_guard::Check,
    ) -> Result<Value> {
        self.current(s, key, check)?;
        self.owner.write_current(s, key, &|| check.check())?;
        if self.private_record.is_none() {
            let b = &self.envelope.payload;
            let target = address(s, &b.scope, &b.workspace, &b.update)?;
            let parent = target.parent().ok_or_else(|| fail("actual private Root parent missing"))?;
            let io = || self.owner.write_current(s, key, &|| check.check()).map_err(|e| std::io::Error::other(e.to_string()));
            let directory = filmcraft_format::retained::preview_context::DirectoryFrame::capture(parent, &io).map_err(|e| fail(e.to_string()))?;
            let name = target.file_name().and_then(|n| n.to_str()).ok_or_else(|| fail("literal private native filename missing"))?;
            let frame = directory.node(name, &io).map_err(|e| fail(e.to_string()))?;
            self.private_record =
                Some(filmcraft_format::retained::private_record::Record::prepare(&frame, &b.update, &self.raw, reserve, &io).map_err(|e| fail(e.to_string()))?);
        }
        let record = self.private_record.as_ref().ok_or_else(|| fail("original private record unavailable"))?;
        if record.intent().reserve_bytes != reserve {
            return Err(fail("original private-record reserve cannot be replaced"));
        }
        self.current(s, key, check)?;
        serde_json::to_value(record.intent()).map_err(|e| fail(e.to_string()))
    }
    #[cfg(any(target_vendor = "apple", target_os = "linux"))]
    pub(in crate::agent_bridge) fn publish_private_record(
        &mut self,
        s: &Session,
        key: &crate::project_save::operation::Key,
        expected_intent: &Value,
        check: &media_guard::Check,
    ) -> Result<Value> {
        self.current(s, key, check)?;
        self.owner.write_current(s, key, &|| check.check())?;
        let record = self.private_record.as_mut().ok_or_else(|| fail("original live private-record preparation required; no reconstruction from JSON"))?;
        if serde_json::to_value(record.intent()).map_err(|e| fail(e.to_string()))? != *expected_intent {
            return Err(fail("complete original private intent differs; no file I/O"));
        }
        let io = || self.owner.write_current(s, key, &|| check.check()).map_err(|e| std::io::Error::other(e.to_string()));
        let before_link = || {
            io()?;
            project_recovery::validate_save_live(s, &self.envelope.payload.basis, key, check).map_err(|e| std::io::Error::other(e.to_string()))?;
            io()
        };
        let result = record.publish(&self.raw, &io, &before_link).map_err(|e| fail(e.to_string()))?;
        self.current(s, key, check)?;
        serde_json::to_value(result).map_err(|e| fail(e.to_string()))
    }
    #[cfg(any(target_vendor = "apple", target_os = "linux"))]
    pub(in crate::agent_bridge) fn inspect_private_record(&self, check: &media_guard::Check) -> Result<Value> {
        let record = self.private_record.as_ref().ok_or_else(|| fail("original private record preparation required"))?;
        let value = record.inspect(&|| check.check().map_err(|e| std::io::Error::other(e.to_string()))).map_err(|e| fail(e.to_string()))?;
        serde_json::to_value(value).map_err(|e| fail(e.to_string()))
    }
    // A prerequisite for retaining dispatch admission, never a custody grant.
    // Inspect the SAME actual private Record freshly; JSON cannot supply one.
    #[cfg(any(target_vendor = "apple", target_os = "linux"))]
    pub(in crate::agent_bridge) fn require_published_private_record(
        &self,
        s: &Session,
        key: &crate::project_save::operation::Key,
        check: &media_guard::Check,
    ) -> Result<()> {
        self.current(s, key, check)?;
        let record = self.private_record.as_ref().ok_or_else(|| fail("publish the original before-frame private record before file admission"))?;
        let observed = record.inspect(&|| check.check().map_err(|e| std::io::Error::other(e.to_string()))).map_err(|e| fail(e.to_string()))?;
        if !observed.matches_published {
            return Err(fail("original before-frame private record is no longer physically published"));
        }
        self.current(s, key, check)
    }
    #[cfg(test)]
    pub(in crate::agent_bridge) fn test_bytes(&self) -> &[u8] {
        &self.raw
    }
}
fn new_capture_id() -> Result<String> {
    let mut raw = [0u8; 16];
    getrandom::fill(&mut raw).map_err(|e| fail(e.to_string()))?;
    let mut text = String::new();
    for (index, byte) in raw.iter().enumerate() {
        if matches!(index, 4 | 6 | 8 | 10) {
            text.push('-');
        }
        text.push_str(&format!("{byte:02x}"));
    }
    Ok(text)
}
pub(in crate::agent_bridge) fn prepare_save_recovery(
    s: &mut Session,
    key: &crate::project_save::operation::Key,
    check: &media_guard::Check,
) -> Result<SaveRecovery> {
    let owner = crate::project_save::operation::recovery_owner(s, key, &|| check.check())?;
    let identity = crate::project_workspace::identity(&s.project)?.ok_or_else(|| fail("original saved Root identity required for complete Save recovery"))?;
    let scope = identity.scope;
    super::super::project_plane::require_scope(s, &scope)?;
    let capture = new_capture_id()?;
    let epoch = epoch(s)?;
    let request = Request {
        token: capture.clone(),
        scope: CheckpointScope::Project(scope.clone()),
        workspace: scope.workspace_id.clone(),
        checkpoint: capture.clone(),
        sha256: None,
        metadata_basis: true,
    };
    let history = pending_capsule(s, &request, s.state.active_sequence)?.encode_checked(Some(check))?;
    if history.epoch != epoch {
        return Err(fail("native Save recovery epoch changed"));
    }
    let history_sha256 = digest(&bytes_checked(&history, MAX_BYTES, Some(check))?);
    owner.current(s, key, &|| check.check())?;
    let basis = project_recovery::capture_save_held(s, &serde_json::to_value(&scope).map_err(|e| fail(e.to_string()))?, key, check)?;
    let snapshot = Snapshot {
        kind: KIND.into(),
        version: 1,
        schema: filmcraft_format::SCHEMA_VERSION,
        scope: scope.clone(),
        workspace: scope.workspace_id.clone(),
        update: capture.clone(),
        stage_token: capture,
        source_pid: std::process::id(),
        history_sha256,
        history,
        editor: native_editor::Snapshot::capture_complete(s)?,
        basis,
    };
    let (envelope, raw) = encode_snapshot(s, snapshot, Some(check), Some(key))?;
    owner.current(s, key, &|| check.check())?;
    let project_read_sha256 = super::super::project_plane::basis(s)?;
    owner.current(s, key, &|| check.check())?;
    Ok(SaveRecovery {
        envelope,
        raw,
        owner,
        project_read_sha256,
        #[cfg(any(target_vendor = "apple", target_os = "linux"))]
        private_record: None,
    })
}

fn prepare(s: &mut Session, p: &Value) -> Result<Value> {
    let r: Prepare = parse(PREPARE, p)?;
    if !token_ok(&r.stage_token) || r.stage_token == r.token || r.stage_token == r.pause_token {
        return Err(bad(PREPARE, "distinct current stage, preparation and pause tokens required"));
    }
    retained_scoped_basis(s, &r.stage_token, &CheckpointScope::Project(r.scope.clone()))?;
    let req = Stage {
        token: r.token,
        scope: r.scope,
        workspace: r.workspace,
        update: r.update,
        pause_token: r.pause_token,
        checkpoint: r.checkpoint,
        checkpoint_sha256: r.checkpoint_sha256,
        expected_sha256: None,
    };
    let (envelope, raw) = capture(s, &req, &r.stage_token)?;
    let mut result = receipt(&envelope.payload, &address(s, &req.scope, &req.workspace, &req.update)?, &digest(&raw), raw.len());
    result["status"] = json!("prepared");
    result["token"] = json!(req.token);
    result["stageToken"] = json!(r.stage_token);
    result["execution"] = json!("snapshot-prepared");
    Ok(result)
}
fn stage(s: &mut Session, p: &Value) -> Result<Value> {
    let r: Stage = parse(STAGE, p)?;
    if workspace_update::stage_reconciling(s, p) {
        return workspace_update::view(s).map(|v| v["stage"].clone()).ok_or_else(|| fail("staged source fence missing"));
    }
    if r.expected_sha256.as_deref().is_some_and(|v| !hash_ok(v)) {
        return Err(fail("exact prepared snapshot checksum required"));
    }
    let (envelope, raw) = capture(s, &r, &r.token)?;
    let hash = digest(&raw);
    if r.expected_sha256.as_ref().is_some_and(|v| v != &hash) {
        return Err(fail("native snapshot differs from the manager's prepared intent; nothing published"));
    }
    let snapshot = &envelope.payload;
    let target = address(s, &r.scope, &r.workspace, &r.update)?;
    let result = receipt(snapshot, &target, &hash, raw.len());
    s.agent_bridge.operations.enter_publication(&r.token)?;
    workspace_update::install_project_stage(
        s,
        p.clone(),
        snapshot.basis.clone(),
        snapshot.basis["saved_file"]["sha256"].as_str().ok_or_else(|| fail("saved hash missing"))?.into(),
        None,
    )?;
    identity_publication::publish_record(&target, &raw, &r.token)?;
    workspace_update::install_project_stage(
        s,
        p.clone(),
        snapshot.basis.clone(),
        snapshot.basis["saved_file"]["sha256"].as_str().ok_or_else(|| fail("saved hash missing"))?.into(),
        Some(result.clone()),
    )?;
    Ok(result)
}
fn read_validated_staged(s: &Session) -> Result<(Snapshot, usize)> {
    let (r, basis) = workspace_update::project_stage(s).ok_or_else(|| fail("complete staged Root receipt required before retirement"))?;
    let scope: crate::project_workspace::ProjectScope = parse(STAGE, &r["scope"])?;
    let check = media_guard::Check::new(web_time::Instant::now());
    let (b, size) = load(s, &scope, r["workspace"].as_str().unwrap_or(""), r["update"].as_str().unwrap_or(""), r["sha256"].as_str().unwrap_or(""), &check)?;
    if b.basis != *basis
        || b.source_pid != std::process::id()
        || s.agent_bridge.epoch.as_deref() != Some(b.history.epoch.as_str())
        || b.history.revision != s.revision
        || !b.editor.matches(s)?
    {
        return Err(fail("staged source editor or exact snapshot changed before retirement"));
    }
    project_recovery::validate_live(s, &b.basis)?;
    Ok((b, size))
}
pub(super) fn validate_staged(s: &Session) -> Result<()> {
    read_validated_staged(s).map(|_| ())
}
// Common native decoding/install sequence only. Callers must retain their own
// genuine startup admission, physical custody and lifecycle hold. This helper
// does not activate a candidate or construct an operation owner from a record.
struct NativeRestore<'a> {
    project_path: &'a str,
    scope: &'a crate::project_workspace::ProjectScope,
    workspace: &'a str,
    activation_token: &'a str,
    history: &'a Capsule,
    editor: &'a native_editor::Snapshot,
    basis: &'a Value,
}
impl NativeRestore<'_> {
    fn install(self, candidate: &mut Session) -> Result<()> {
        let Self { project_path, scope, workspace, activation_token, history, editor, basis } = self;
        project_recovery::validate(basis)?;
        let project = workspace_project(history)?;
        crate::commands::install_project(candidate, project, Some(project_path.into()), true);
        editor.install_cold(candidate)?;
        candidate.revision = history.revision;
        candidate.saved_revision = candidate.revision;
        let history_hash = digest(&bytes(history, MAX_BYTES)?);
        let req = request(scope, workspace, &history.checkpoint, activation_token);
        let prepared = prepare_restore(candidate, &req, history, &history_hash, candidate.state.active_sequence)?;
        candidate.history = prepared.history;
        candidate.agent_bridge.application_history = prepared.application;
        candidate.agent_bridge.application_ui = prepared.application_ui;
        candidate.history.merge_key = editor.merge_key.clone();
        candidate.agent_bridge.tickets = prepared.tickets;
        candidate.agent_bridge.head = prepared.owned_head.then(|| (candidate.revision, Arc::downgrade(&candidate.project)));
        candidate.saved_revision = editor.saved_revision;
        candidate.saved_file = Some(serde_json::from_value(basis["saved_file"].clone()).map_err(|e| fail(e.to_string()))?);
        retain_history_media(candidate);
        if let Some(aux) = &editor.auxiliary {
            aux.restore_origins(candidate)?;
        }
        remember_workspace(candidate, workspace, &history.checkpoint, &history_hash, &history.epoch)?;
        Ok(())
    }
}

pub(super) fn open(s: &mut Session, p: &Value) -> Result<Value> {
    bytes(p, 16384)?;
    let r: Startup = parse(STAGE, p)?;
    native_editor::require_fresh(s)?;
    if !token_ok(&r.activation_token) {
        return Err(fail("fresh private activation token required"));
    }
    let check = media_guard::Check::new(web_time::Instant::now());
    let mut candidate = Session::new(s.services.clone());
    candidate.path = Some(r.project_path.clone());
    let (b, _) = load(&candidate, &r.scope, &r.workspace, &r.update, &r.sha256, &check)?;
    if r.activation_token == b.stage_token {
        return Err(fail("staging token cannot activate a recovered candidate"));
    }
    if r.current_image.is_some() && (r.latest_journal || r.restoration.is_some()) {
        return Err(fail("current image is complete; journal overlay and restoration require their own current-image admission"));
    }
    let old_revision = b.history.revision;
    #[cfg(any(target_vendor = "apple", target_os = "linux"))]
    let current = r
        .current_image
        .as_ref()
        .map(|value| {
            workspace_update::read_current_image(
                value,
                workspace_update::CurrentImageExpected {
                    scope: &r.scope,
                    workspace: &r.workspace,
                    update: &r.update,
                    project_path: &r.project_path,
                    activation_token: &r.activation_token,
                    minimum_revision: old_revision,
                    source_snapshot: &r.sha256,
                    saved_file: &b.basis["saved_file"],
                },
                &check,
            )
        })
        .transpose()?;
    #[cfg(not(any(target_vendor = "apple", target_os = "linux")))]
    if r.current_image.is_some() {
        return Err(fail("current-image native file context is unsupported on this platform"));
    }
    #[cfg(any(target_vendor = "apple", target_os = "linux"))]
    let mut current_input = None;
    #[cfg(any(target_vendor = "apple", target_os = "linux"))]
    let original_current_checkpoint = current.as_ref().map(|_| (b.history.clone(), b.history_sha256.clone()));
    let starting_basis = if let Some(restoration) = &r.restoration {
        project_recovery::restore_basis(&b.basis, restoration, &r.sha256, &r.activation_token)?
    } else {
        b.basis.clone()
    };
    let journal = if r.latest_journal { project_recovery::read_newer_journal(&starting_basis, b.source_pid)? } else { None };
    let restored = {
        #[cfg(any(target_vendor = "apple", target_os = "linux"))]
        {
            current.map(|image| {
                current_input = Some(image.input);
                (image.history, image.editor, image.basis, None)
            })
        }
        #[cfg(not(any(target_vendor = "apple", target_os = "linux")))]
        {
            None
        }
    };
    let (history, editor, basis, journal_identity) = if let Some(image) = restored {
        image
    } else if let Some(plan) = journal {
        let raw_hash = digest(&plan.raw);
        let payload = super::journal::decode(&plan.raw, &plan.project)?;
        let source_epoch = payload.history.epoch.clone();
        let basis = plan.finish(project_recovery::JournalIdentity {
            scope: match &payload.history.scope {
                CheckpointScope::Project(scope) => scope.clone(),
                _ => return Err(fail("independent journal scope required")),
            },
            saved_file: payload.saved_file.clone(),
            source_pid: payload.source_pid,
            native_session: source_epoch.clone(),
            data_directory: payload.data_directory.clone(),
            session_directory: payload.session_directory.clone(),
            revision: payload.history.revision,
            saved_revision: payload.editor.saved_revision,
            sequence: payload.history.sequence,
            project_sha256: digest(payload.history.projects.first().map_or(&[], |p| p.as_bytes())),
        })?;
        (payload.history, payload.editor, basis, Some((raw_hash, source_epoch)))
    } else {
        (b.history, b.editor, starting_basis, None)
    };
    NativeRestore {
        project_path: &r.project_path,
        scope: &r.scope,
        workspace: &r.workspace,
        activation_token: &r.activation_token,
        history: &history,
        editor: &editor,
        basis: &basis,
    }
    .install(&mut candidate)?;
    workspace_update::hold_scoped_candidate(
        &mut candidate,
        CheckpointScope::Project(r.scope.clone()),
        r.workspace.clone(),
        r.update.clone(),
        r.activation_token,
        basis["saved_file"]["sha256"].as_str().ok_or_else(|| fail("saved baseline missing"))?.into(),
    )?;
    workspace_update::install_candidate_recovery_basis(&mut candidate, basis.clone())?;
    #[cfg(any(target_vendor = "apple", target_os = "linux"))]
    if let Some((original, hash)) = original_current_checkpoint {
        retain_current_image_original_checkpoint(&mut candidate, &original, &hash, &check)?;
    }
    if let Some((hash, source_epoch)) = journal_identity {
        let epoch = candidate.agent_bridge.epoch.as_deref().ok_or_else(|| fail("journal candidate epoch missing"))?;
        let receipt = json!({"status":"prepared","kind":"native_project_journal_recovery","version":1,"scope":r.scope,"workspace":r.workspace,"update":r.update,"projectPath":r.project_path,"sourceSnapshotSha256":r.sha256,"sourceSnapshotRevision":old_revision,"journalSha256":hash,"sourceNativeSession":source_epoch,"sourceNativePid":b.source_pid,"nativeSession":epoch,"nativePid":std::process::id(),"revision":candidate.revision,"sequence":candidate.state.active_sequence.map(|v|v.0),"editingPaused":true,"history":"native_history_retained","sourceValidation":"not-performed","mediaEvidence":false,"writeAuthority":false,"execution":"journal-candidate-prepared"});
        workspace_update::install_candidate_journal_receipt(&mut candidate, receipt)?;
    }
    project_recovery::validate(&basis)?;
    #[cfg(any(target_vendor = "apple", target_os = "linux"))]
    if let Some(input) = current_input {
        input.current(&check)?;
        workspace_update::retain_restored_current_image(&mut candidate, input)?;
    }
    let fence = workspace_update::view(&candidate).ok_or_else(|| fail("journal candidate fence missing"))?;
    check.check()?;
    *s = candidate;
    Ok(fence)
}
pub(super) fn commands() -> Vec<CommandSpec> {
    vec![
        recovery_dependencies::command(),
        recovery_dependencies::archive_command(),
        CommandSpec {
            id: PREPARE,
            label: "Prepare Independent Native Project Recovery",
            menu: &[],
            shortcut: None,
            params: "{token:privateBasis,stageToken:privateBasis,scope:{projectId,workspaceId},workspace,update,pauseToken,checkpoint,checkpointSha256}",
            enabled: always,
            run: prepare,
            journal: false,
        },
        CommandSpec {
            id: STAGE,
            label: "Stage Independent Native Project Recovery",
            menu: &[],
            shortcut: None,
            params: "{token:privateBasis,scope:{projectId,workspaceId},workspace,update,pauseToken,checkpoint,checkpointSha256,expectedSha256?:preparedHash}",
            enabled: always,
            run: stage,
            journal: false,
        },
    ]
}

// Trusted startup only, deliberately separate from the workspace-update owner.
pub(in crate::agent_bridge) fn open_save(s: &mut Session, p: &Value) -> Result<Value> {
    #[cfg(any(target_vendor = "apple", target_os = "linux"))]
    {
        save_startup::open(s, p)
    }
    #[cfg(not(any(target_vendor = "apple", target_os = "linux")))]
    {
        let _ = (s, p);
        Err(fail("cold native Save restoration needs supported physical file identities"))
    }
}
