//! Original native Save snapshot construction with fresh private-input custody.
//! No historical Key, Record, Commit, application-update owner or worker is
//! deserialized. The complete host journal remains a separate live prerequisite.
use super::*;
mod control_reads;
mod native_directory;
mod owner_manifest;
mod runtime_transition;
use crate::agent_bridge::save_operation::managed_dispatch::Control;
use filmcraft_format::retained::{
    Observation,
    preview_context::{DirectoryFrame, NodeFrame, NodeState, check_owner_exited, check_owner_process},
};
use native_directory::NativeDirectory;
use std::{collections::BTreeSet, io};
const INPUT: usize = 512 * 1024;
const CATALOG: u64 = 128 * 1024;
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Reference {
    kind: String,
    version: u8,
    path: PathBuf,
    sha256: String,
    bytes: u64,
    physical: Physical,
    max_bytes: usize,
    native_startup_authority: bool,
    workers_may_resume: bool,
    write_authority: bool,
    full_application_closure: bool,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Physical {
    dev: String,
    ino: String,
    mode: String,
    uid: String,
    size: String,
    mtime_ns: String,
    ctime_ns: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DirectoryPhysical {
    dev: String,
    ino: String,
    mode: String,
    uid: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Root {
    path: PathBuf,
    physical: DirectoryPhysical,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Observed {
    path: PathBuf,
    sha256: String,
    bytes: u64,
    physical: Physical,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Controls {
    manifest: Observed,
    selection: Observed,
    catalog: Observed,
    lease: Observed,
    journal_anchor: Observed,
    journal_head: Observed,
}
#[derive(Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct RequestIds {
    job: String,
    operation: String,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SourceInput {
    manifest_path: PathBuf,
    catalog_file: PathBuf,
    catalog_sha256: String,
    evidence_id: String,
    request: RequestIds,
    native_data: PathBuf,
    storage_roots: Vec<PathBuf>,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Frame {
    kind: String,
    version: u8,
    job: String,
    operation: String,
    scope: crate::project_workspace::ProjectScope,
    native_session: String,
    native_pid: u32,
    revision: u64,
    sequence: Option<u64>,
    project_path: String,
    phase: String,
    capture: String,
    checkpoint: String,
    sha256: String,
    bytes: u64,
    history_sha256: String,
    project_sha256: String,
    project_read_sha256: String,
    staged: bool,
    current: bool,
    write_authority: bool,
    native_complete: bool,
    startup_authority: bool,
    workers_may_resume: bool,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Role {
    role: String,
    path: PathBuf,
    #[serde(rename = "type")]
    node_type: String,
    provenance: String,
    physical: Option<Physical>,
    sha256: Option<String>,
    bytes: Option<u64>,
    mode: Option<u32>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Owner {
    owner_id: String,
    origin: String,
    data_root: PathBuf,
    project_id: String,
    workspace_id: String,
    ledger_file: Option<PathBuf>,
    bootstrap_file: Option<PathBuf>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Journal {
    directory: PathBuf,
    anchor: PathBuf,
    head: PathBuf,
    binding: Value,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Basis {
    kind: String,
    version: u8,
    frame: Frame,
    input: SourceInput,
    critical: Vec<Role>,
    manifest_path: PathBuf,
    selection_file: PathBuf,
    owner: Owner,
    whole_journal: Journal,
    working_roots: Vec<Root>,
    reserve_bytes: u64,
    runtime_readiness: String,
    native_startup_authority: bool,
    workers_may_resume: bool,
    write_authority: bool,
    full_application_closure: bool,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Manager {
    pid: u32,
    identity: String,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Body {
    kind: String,
    version: u8,
    activation_token: String,
    basis: Basis,
    controls: Controls,
    after_frame: Observed,
    manager: Manager,
    native_startup_authority: bool,
    workers_may_resume: bool,
    write_authority: bool,
    full_application_closure: bool,
}
struct Input {
    body: Body,
    input: Control,
    controls: Vec<Control>,
    roots: Vec<Arc<DirectoryFrame>>,
    nodes: Vec<NodeFrame>,
    input_directory: Arc<DirectoryFrame>,
    reference_hash: String,
    native_directory: Option<NativeDirectory>,
}
fn error(e: impl std::fmt::Display) -> crate::EngineError {
    bad("native.coldSaveStartup", e.to_string())
}
fn literal(p: &Path) -> bool {
    p.is_absolute()
        && p.to_str().is_some_and(|s| s.len() <= 4096 && !s.chars().any(char::is_control))
        && p.components().all(|c| matches!(c, std::path::Component::RootDir | std::path::Component::Normal(_)))
}
fn ids(v: &str) -> bool {
    (32..=128).contains(&v.len()) && v.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}
fn nanos(s: &str) -> Result<String> {
    let (seconds, nanos) = s.split_once(':').ok_or_else(|| error("native physical timestamp required"))?;
    let seconds = seconds.parse::<i128>().map_err(error)?;
    let nanos = nanos.parse::<i128>().map_err(error)?;
    if !(0..1_000_000_000).contains(&nanos) {
        return Err(error("native nanosecond component is out of bounds"));
    }
    seconds.checked_mul(1_000_000_000).and_then(|n| n.checked_add(nanos)).map(|n| n.to_string()).ok_or_else(|| error("native timestamp overflow"))
}
impl Physical {
    fn matches(&self, o: &Observation) -> Result<()> {
        if self.dev != o.identity.device
            || self.ino != o.identity.inode
            || self.mode != o.identity.mode.to_string()
            || self.uid != o.identity.uid.to_string()
            || self.size != o.bytes.to_string()
            || self.mtime_ns != nanos(&o.modified)?
            || self.ctime_ns != nanos(&o.changed)?
        {
            return Err(error("exact current native publication generation changed"));
        }
        Ok(())
    }
}
fn observed(r: &Observed, max: u64, check: &media_guard::Check) -> Result<Control> {
    if !literal(&r.path) || !hash_ok(&r.sha256) || r.bytes == 0 || r.bytes > max {
        return Err(error("bounded exact original private control required"));
    }
    let c = Control::capture(&r.path, max, check)?;
    let o = c.observation()?;
    r.physical.matches(o)?;
    if o.sha256 != r.sha256 || o.bytes != r.bytes {
        return Err(error("original private control bytes changed"));
    }
    Ok(c)
}
// These are query modes, never ownership or permission. Fence is used only
// after the same native callback's complete source read and before any effect.
#[derive(Clone, Copy)]
enum ApplicationRead {
    Complete,
    FenceAfterRead,
}
impl Input {
    fn within(&self, p: &Path) -> bool {
        literal(p) && self.body.basis.working_roots.iter().any(|r| p.starts_with(&r.path))
    }
    fn role(&self, name: &str) -> Result<&Role> {
        self.body.basis.critical.iter().find(|r| r.role == name).ok_or_else(|| error("complete native baseline role required"))
    }
    fn current(&self, check: &media_guard::Check) -> Result<()> {
        self.current_with_journal(check, None)
    }
    fn current_with_journal(&self, check: &media_guard::Check, successor: Option<&runtime_transition::JournalContinuation>) -> Result<()> {
        self.current_with_runtime(check, successor, None)
    }
    fn current_with_runtime(
        &self,
        check: &media_guard::Check,
        successor: Option<&runtime_transition::JournalContinuation>,
        preference: Option<&workspace_update::PreferenceContinuation>,
    ) -> Result<()> {
        self.current_with_application(check, successor, preference, None)
    }
    fn current_with_application(
        &self,
        check: &media_guard::Check,
        successor: Option<&runtime_transition::JournalContinuation>,
        preference: Option<&workspace_update::PreferenceContinuation>,
        manifest: Option<&Control>,
    ) -> Result<()> {
        self.read_application(check, successor, preference, manifest, None, ApplicationRead::Complete)
    }
    /// The complete original verifier's namespace, owner, reserve, Control and
    /// file-generation checks, immediately after the same caller's full read.
    fn fence_application_after_read(
        &self,
        check: &media_guard::Check,
        successor: Option<&runtime_transition::JournalContinuation>,
        preference: Option<&workspace_update::PreferenceContinuation>,
        manifest: Option<&Control>,
    ) -> Result<()> {
        self.read_application(check, successor, preference, manifest, None, ApplicationRead::FenceAfterRead)
    }
    fn read_settled_application(
        &self,
        check: &media_guard::Check,
        successor: &runtime_transition::JournalContinuation,
        preference: &workspace_update::PreferenceContinuation,
        manifest: &Control,
        catalog: &Control,
        mode: ApplicationRead,
    ) -> Result<()> {
        self.read_application(check, Some(successor), Some(preference), Some(manifest), Some(catalog), mode)
    }
    fn read_application(
        &self,
        check: &media_guard::Check,
        successor: Option<&runtime_transition::JournalContinuation>,
        preference: Option<&workspace_update::PreferenceContinuation>,
        manifest: Option<&Control>,
        catalog: Option<&Control>,
        mode: ApplicationRead,
    ) -> Result<()> {
        check.check()?;
        if let Some(journal) = successor {
            journal.begin_application_read(check, mode)?;
        }
        self.native_directory.as_ref().ok_or_else(|| error("actual native directory has not been captured"))?.current(check)?;
        check_owner_exited(self.body.basis.frame.native_pid).map_err(error)?;
        check_owner_process(self.body.manager.pid).map_err(error)?;
        match mode {
            ApplicationRead::Complete => self.input.current(check)?,
            ApplicationRead::FenceAfterRead => self.input.fence(check)?,
        }
        self.read_selected_controls(check, successor, manifest, catalog, mode)?;
        let io = || check.check().map_err(|e| io::Error::other(e.to_string()));
        for r in &self.roots {
            r.current(&io).map_err(error)?;
            let space = r.space(&io).map_err(error)?;
            if space.read_only || space.free < self.body.basis.reserve_bytes.saturating_add(INPUT as u64).saturating_add(8 * 1024 * 1024) {
                return Err(error("native working reserve unavailable; no fallback"));
            }
        }
        self.input_directory.current(&io).map_err(error)?;
        let mut names = Vec::new();
        for entry in fs::read_dir(&self.input_directory.record().path).map_err(error)? {
            if names.len() >= 2 {
                return Err(error("unknown private native input files retained"));
            }
            names.push(entry.map_err(error)?.file_name());
        }
        if names != [std::ffi::OsString::from("input.json")] {
            return Err(error("private native input namespace changed"));
        }
        for node in &self.nodes {
            let continued = preference
                .map(|p| match mode {
                    ApplicationRead::Complete => p.current_node(node, &io),
                    ApplicationRead::FenceAfterRead => p.fence_node_after_read(node, &io),
                })
                .transpose()?
                .unwrap_or(false);
            if continued {
                continue;
            }
            if matches!(mode, ApplicationRead::Complete) {
                node.current(&io).map_err(error)?;
            }
            node.fence(&io).map_err(error)?;
        }
        self.input.fence(check)?;
        self.read_selected_controls(check, successor, manifest, catalog, ApplicationRead::FenceAfterRead)?;
        if let Some(journal) = successor {
            journal.fence_after_read(check)?;
        }
        check_owner_exited(self.body.basis.frame.native_pid).map_err(error)?;
        check_owner_process(self.body.manager.pid).map_err(error)?;
        self.native_directory.as_ref().ok_or_else(|| error("actual native directory has not been captured"))?.current(check)?;
        check.check()
    }
    fn read_selected_controls(
        &self,
        check: &media_guard::Check,
        successor: Option<&runtime_transition::JournalContinuation>,
        manifest: Option<&Control>,
        catalog: Option<&Control>,
        mode: ApplicationRead,
    ) -> Result<()> {
        let selected = self
            .controls
            .iter()
            .enumerate()
            .map(|(i, c)| {
                if i == 0 {
                    manifest.unwrap_or(c)
                } else if i == 2 {
                    catalog.unwrap_or(c)
                } else {
                    successor.map_or(c, |journal| journal.control(i, c))
                }
            })
            .collect::<Vec<_>>();
        control_reads::selected(&selected, check, mode)
    }
    fn read(p: &Value, check: &media_guard::Check) -> Result<Self> {
        bytes(p, 16384)?;
        let r: Reference = parse("native.coldSaveStartup", p)?;
        if r.kind != "poema-original-Save-native-input-description"
            || r.version != 1
            || r.max_bytes != INPUT
            || r.native_startup_authority
            || r.workers_may_resume
            || r.write_authority
            || r.full_application_closure
        {
            return Err(error("a current descriptive private Save startup reference is required"));
        }
        let input = observed(&Observed { path: r.path.clone(), sha256: r.sha256.clone(), bytes: r.bytes, physical: r.physical }, INPUT as u64, check)?;
        let body: Body = serde_json::from_value(input.value().clone()).map_err(error)?;
        let b = &body.basis;
        let f = &b.frame;
        if body.kind != "poema-native-Save-cold-startup-input"
            || body.version != 1
            || b.kind != "poema-original-Save-native-input-basis"
            || b.version != 1
            || !crate::editorial_workspace::uuid(&body.activation_token)
            || body.activation_token == f.capture
            || body.activation_token == b.input.request.operation
            || [
                body.native_startup_authority,
                body.workers_may_resume,
                body.write_authority,
                body.full_application_closure,
                b.native_startup_authority,
                b.workers_may_resume,
                b.write_authority,
                b.full_application_closure,
            ]
            .iter()
            .any(|v| *v)
            || b.runtime_readiness != "not-inferred-from-hashes"
            || b.reserve_bytes == 0
            || b.working_roots.is_empty()
            || b.working_roots.len() > 16
            || b.critical.len() != 3
            || f.kind != "native_Save_recovery_read"
            || f.version != 1
            || f.phase != "AwaitingReconciliation"
            || !crate::project_workspace::valid_scope(&f.scope)
            || f.scope.workspace_id != b.owner.workspace_id
            || f.scope.project_id != b.owner.project_id
            || f.job != b.input.request.job
            || f.operation != b.input.request.operation
            || !ids(&f.job)
            || !ids(&f.operation)
            || f.job == f.operation
            || !token_ok(&f.native_session)
            || !crate::editorial_workspace::uuid(&f.capture)
            || f.checkpoint != f.capture
            || f.revision == 0
            || f.revision >= u64::MAX - 1
            || f.sequence.is_some_and(|v| v == 0 || v > 9_007_199_254_740_991)
            || !literal(Path::new(&f.project_path))
            || !f.project_path.ends_with(".fcproj")
            || !hash_ok(&f.sha256)
            || !hash_ok(&f.history_sha256)
            || !hash_ok(&f.project_sha256)
            || !hash_ok(&f.project_read_sha256)
            || f.bytes == 0
            || f.bytes > LIMIT as u64
            || [f.staged, f.current, f.write_authority, f.native_complete, f.startup_authority, f.workers_may_resume].iter().any(|v| *v)
            || !crate::editorial_workspace::uuid(&body.manager.identity)
            || body.manager.pid == 0
            || body.manager.pid == std::process::id()
            || body.manager.pid == f.native_pid
            || b.manifest_path != b.input.manifest_path
            || !hash_ok(&b.input.catalog_sha256)
            || !crate::editorial_workspace::uuid(&b.input.evidence_id)
            || b.input.storage_roots.is_empty()
            || b.input.storage_roots.len() > 16
            || b.owner.owner_id.is_empty()
            || b.owner.owner_id.len() > 128
            || b.owner.origin.is_empty()
            || b.owner.origin.len() > 4096
        {
            return Err(error("crossed incomplete or authority-bearing native Save input"));
        }
        check_owner_exited(f.native_pid).map_err(error)?;
        check_owner_process(body.manager.pid).map_err(error)?;
        let io = || check.check().map_err(|e| io::Error::other(e.to_string()));
        let mut roots = Vec::new();
        for (i, root) in b.working_roots.iter().enumerate() {
            if !literal(&root.path)
                || b.working_roots.iter().enumerate().any(|(j, q)| i != j && (root.path.starts_with(&q.path) || q.path.starts_with(&root.path)))
            {
                return Err(error("explicit disjoint original working roots required"));
            }
            let frame = DirectoryFrame::capture(&root.path, &io).map_err(error)?;
            let pin = frame.record();
            let expected = &root.physical;
            if !pin.missing.is_empty()
                || pin.anchor.identity.device != expected.dev
                || pin.anchor.identity.inode != expected.ino
                || pin.anchor.identity.mode.to_string() != expected.mode
                || pin.anchor.identity.uid.to_string() != expected.uid
            {
                return Err(error("native working root generation changed"));
            }
            roots.push(frame);
        }
        let input_directory = DirectoryFrame::capture(r.path.parent().ok_or_else(|| error("actual private input parent required"))?, &io).map_err(error)?;
        let mut result =
            Self { body, input, controls: Vec::new(), roots, nodes: Vec::new(), input_directory, reference_hash: r.sha256, native_directory: None };
        if !result.within(&r.path) {
            return Err(error("private native input escapes explicit working roots; no fallback"));
        }
        result.capture_controls(check)?;
        result.capture_roles(check)?;
        result.current(check)?;
        Ok(result)
    }
    fn capture_controls(&mut self, check: &media_guard::Check) -> Result<()> {
        let b = &self.body.basis;
        let c = &self.body.controls;
        let f = &b.frame;
        let expected_selection = b
            .input
            .catalog_file
            .parent()
            .ok_or_else(|| error("actual catalog parent required"))?
            .join(format!("native-save-recovery-selection-{}.json", b.input.evidence_id));
        let manifest_text = b.manifest_path.to_str().ok_or_else(|| error("literal manager path required"))?;
        let context =
            b.manifest_path.parent().ok_or_else(|| error("actual manager parent required"))?.join(format!("workspace-state-{}", f.scope.workspace_id));
        if b.input.catalog_file != context.join("native-save-evidence.json")
            || b.whole_journal.anchor != context.join(format!("native-save-owner-journal-{}.json", b.input.evidence_id))
        {
            return Err(error("original Save private controls crossed their actual workspace"));
        }
        let private_input = self.input_directory.record().path;
        for excluded in [&b.owner.data_root, &b.input.native_data, &b.whole_journal.directory, &context] {
            if private_input.starts_with(excluded) || excluded.starts_with(&private_input) {
                return Err(error("private startup input overlaps an original owned namespace"));
            }
        }

        if c.manifest.path != b.manifest_path
            || c.selection.path != b.selection_file
            || b.selection_file != expected_selection
            || c.catalog.path != b.input.catalog_file
            || c.catalog.sha256 != b.input.catalog_sha256
            || c.lease.path != PathBuf::from(format!("{manifest_text}.lock"))
            || c.journal_anchor.path != b.whole_journal.anchor
            || c.journal_head.path != b.whole_journal.head
            || b.whole_journal.head != b.whole_journal.directory.join("head.json")
        {
            return Err(error("native startup crossed its exact original control roles"));
        }
        let descriptors = [
            (&c.manifest, CATALOG),
            (&c.selection, CATALOG),
            (&c.catalog, CATALOG),
            (&c.lease, 4096),
            (&c.journal_anchor, CATALOG),
            (&c.journal_head, 1024 * 1024),
        ];
        let mut controls = Vec::new();
        for (r, max) in descriptors {
            if !self.within(&r.path) {
                return Err(error("native private control escapes working roots"));
            }
            controls.push(observed(r, max, check)?);
        }
        let get = |i: usize| controls.get(i).map(Control::value).ok_or_else(|| error("complete original controls required"));
        let manifest = get(0)?;
        let selection = get(1)?;
        let catalog = get(2)?;
        let lease = get(3)?;
        let anchor = get(4)?;
        let head = get(5)?;
        if manifest["kind"] != "poema-saved-native-workspace"
            || manifest["id"] != f.scope.workspace_id
            || manifest["projectId"] != f.scope.project_id
            || manifest["nativePid"] != f.native_pid
            || manifest["nativeData"] != serde_json::to_value(&b.input.native_data).map_err(error)?
            || manifest["editorialServer"]["dataDir"] != serde_json::to_value(&b.owner.data_root).map_err(error)?
            || lease.as_object().is_none_or(|v| v.len() != 2)
            || lease["pid"] != self.body.manager.pid
            || lease["identity"] != self.body.manager.identity
            || selection["kind"] != "poema-original-Save-recovery-selection"
            || selection["version"] != 1
            || selection["input"] != serde_json::to_value(&b.input).map_err(error)?
            || selection["afterFrameSha256"] != f.sha256
            || ["writeAuthority", "nativeStartupAuthority", "workersMayResume"].iter().any(|k| selection[*k] != false)
            || catalog["kind"] != "poema-native-Save-evidence"
            || catalog["version"] != 8
            || ["current", "writeAuthority", "nativeComplete", "startupAuthority", "requestReplay"].iter().any(|k| catalog[*k] != false)
            || anchor["kind"] != "poema-original-Save-owner-journal-anchor"
            || anchor["version"] != 1
            || head["kind"] != "poema-original-Save-owner-journal-head"
            || head["version"] != 1
            || anchor["id"] != head["id"]
            || anchor["head"]["sha256"] != c.journal_head.sha256
            || anchor["head"]["physical"] != serde_json::to_value(&c.journal_head.physical).map_err(error)?
            || anchor["root"] != head["storage"]["root"]
            || b.whole_journal.binding["kind"] != "poema-original-Save-owner-journal-binding"
            || b.whole_journal.binding["version"] != 1
            || anchor["binding"] != b.whole_journal.binding
            || head["binding"] != b.whole_journal.binding
            || head["phase"] != "bytes_restored"
            || !head["pending"].is_null()
            || !head["packagePending"].is_null()
            || b.whole_journal.binding["afterFrame"] != f.sha256
        {
            return Err(error("native startup lacks its actual complete selected after-Save controls"));
        }
        // The original server configuration contains paths, not an origin.
        // Recover descriptive owner identity from the existing hash-bound
        // original recovery manifest; never open a credential-bearing owner file.
        let receipt_directory = b.whole_journal.binding["receiptDirectory"]
            .as_str()
            .map(Path::new)
            .filter(|p| literal(p) && self.within(p))
            .ok_or_else(|| error("explicit original owner recovery directory required"))?;
        let receipt_hash =
            b.whole_journal.binding["receipt"].as_str().filter(|v| hash_ok(v)).ok_or_else(|| error("original owner recovery manifest hash required"))?;
        let owner_manifest = Control::capture(&receipt_directory.join("manifest.json"), 32 * 1024 * 1024, check)?;
        if owner_manifest.observation()?.sha256 != receipt_hash {
            return Err(error("original owner recovery manifest bytes changed"));
        }
        owner_manifest::validate(owner_manifest.value(), manifest, &b.owner, receipt_directory, &f.sha256)?;
        let rows = catalog["operations"].as_array().filter(|v| v.len() <= 4).ok_or_else(|| error("bounded original catalog rows required"))?;
        let selected = rows.iter().filter(|r| r["id"] == b.input.evidence_id).collect::<Vec<_>>();
        let row = selected.first().filter(|_| selected.len() == 1).ok_or_else(|| error("one exact selected original Save operation required"))?;
        if row["request"] != serde_json::to_value(&b.input.request).map_err(error)?
            || row["phase"] != "needs_observation"
            || row["nativeObservation"]["status"] != "held"
            || row["nativeObservation"]["pending"]["phase"] != "AwaitingReconciliation"
        {
            return Err(error("the catalog does not retain the selected actual after-Save operation"));
        }
        let declared_directory =
            row.get("basis").and_then(|basis| basis.get("nativeDirectory")).ok_or_else(|| error("explicit original native directory observation required"))?;
        self.native_directory = Some(NativeDirectory::capture(declared_directory, &b.working_roots, check)?);
        let refs = row["observations"].as_array().filter(|v| v.len() <= 9).ok_or_else(|| error("bounded original observation references required"))?;
        let refs = refs.iter().filter(|r| r["role"] == "recovery_preparation").collect::<Vec<_>>();
        let r = refs.first().filter(|_| refs.len() == 1).ok_or_else(|| error("actual original recovery preparation required"))?;
        let hash = r["sha256"].as_str().filter(|h| hash_ok(h)).ok_or_else(|| error("original preparation hash required"))?;
        let size = r["bytes"].as_u64().filter(|v| *v > 0 && *v <= 64 * 1024).ok_or_else(|| error("bounded original preparation required"))?;
        let name = format!("native-save-observation-{}-recovery_preparation-{hash}.json", b.input.evidence_id);
        if r["name"] != name {
            return Err(error("original preparation reference name differs"));
        }
        let path = b.input.catalog_file.parent().ok_or_else(|| error("actual catalog parent required"))?.join(name);
        let preparation = Control::capture(&path, 64 * 1024, check)?;
        let o = preparation.observation()?;
        if o.sha256 != hash || o.bytes != size {
            return Err(error("actual original preparation bytes differ"));
        }
        let frames = preparation.value()["retainedFrames"]
            .as_array()
            .filter(|v| !v.is_empty() && v.len() <= 2)
            .ok_or_else(|| error("complete original preparation frames required"))?;
        let want = serde_json::to_value(f).map_err(error)?;
        if frames.iter().filter(|v| **v == want).count() != 1 {
            return Err(error("a before-frame or different after-frame cannot replace the selected native state"));
        }
        if !self.within(&b.owner.data_root)
            || !self.within(&b.input.native_data)
            || b.owner.ledger_file.as_ref().is_some_and(|p| !self.within(p))
            || b.owner.bootstrap_file.as_ref().is_some_and(|p| !self.within(p))
            || b.input.storage_roots.iter().any(|p| !self.within(p))
        {
            return Err(error("original Save native/data scopes escape explicit roots"));
        }
        let io = || check.check().map_err(|e| io::Error::other(e.to_string()));
        let directory = DirectoryFrame::capture(&b.whole_journal.directory, &io).map_err(error)?;
        let record = directory.record();
        let identity = &record.anchor.identity;
        let expected = json!({"dev":identity.device,"ino":identity.inode,"mode":identity.mode.to_string(),"uid":identity.uid.to_string()});
        if !record.missing.is_empty() || head["storage"]["root"] != expected {
            return Err(error("the actual original journal directory generation changed"));
        }
        controls.push(owner_manifest);
        controls.push(preparation);
        self.roots.push(directory);
        self.controls = controls;
        Ok(())
    }
    fn capture_roles(&mut self, check: &media_guard::Check) -> Result<()> {
        let f = &self.body.basis.frame;
        let b = &self.body.basis;
        let saved = self.role("saved-project")?;
        let preferences = self.role("preferences")?;
        let after = self.role("after-frame")?;
        let expected = Path::new(&f.project_path)
            .parent()
            .ok_or_else(|| error("actual saved Root parent required"))?
            .join(format!(".poema-project-recovery-{}-{}.json", f.scope.workspace_id, f.capture));
        if saved.path != Path::new(&f.project_path)
            || preferences.path != b.input.native_data.join("preferences.json")
            || after.path != expected
            || self.body.after_frame.path != after.path
            || self.body.after_frame.sha256 != f.sha256
            || self.body.after_frame.bytes != f.bytes
        {
            return Err(error("native critical roles crossed the original snapshot scope"));
        }
        let mut paths = BTreeSet::new();
        let io = || check.check().map_err(|e| io::Error::other(e.to_string()));
        let mut nodes = Vec::new();
        for role in &b.critical {
            if !paths.insert(role.path.clone()) || !self.within(&role.path) || role.mode.is_some_and(|m| m > 0o777) {
                return Err(error("distinct bounded native baseline roles required"));
            }
            let parent = role.path.parent().ok_or_else(|| error("actual native baseline parent required"))?;
            let name = role.path.file_name().and_then(|v| v.to_str()).ok_or_else(|| error("literal baseline filename required"))?;
            let node = DirectoryFrame::capture(parent, &io).map_err(error)?.node_bounded(name, LIMIT as u64, &io).map_err(error)?;
            if role.node_type == "absent" {
                if role.role != "preferences"
                    || role.provenance != "original-absence"
                    || role.physical.is_some()
                    || role.sha256.is_some()
                    || role.bytes.is_some()
                    || role.mode.is_some()
                    || node.state() != &NodeState::Absent
                {
                    return Err(error("an original preference absence changed"));
                }
            } else {
                if role.node_type != "file" || !matches!(role.provenance.as_str(), "original-confirmed-publication" | "original-preserved-generation") {
                    return Err(error("an actual preserved or confirmed native file role is required"));
                }
                let NodeState::File(o) = node.state() else {
                    return Err(error("native baseline file changed type"));
                };
                role.physical.as_ref().ok_or_else(|| error("native publication physical generation required"))?.matches(o)?;
                if role.sha256.as_deref() != Some(o.sha256.as_str()) || role.bytes != Some(o.bytes) || role.mode.is_some_and(|m| o.identity.mode & 0o777 != m) {
                    return Err(error("native baseline bytes differ from their complete original role"));
                }
                if role.role == "after-frame" {
                    self.body.after_frame.physical.matches(o)?;
                    if o.sha256 != f.sha256 || o.bytes != f.bytes {
                        return Err(error("the actual selected opaque after-frame changed"));
                    }
                }
            }
            nodes.push(node);
        }
        self.nodes = nodes;
        Ok(())
    }
    fn native_basis(&self, snapshot: &Snapshot) -> Result<Value> {
        let f = &self.body.basis.frame;
        if snapshot.scope != f.scope
            || snapshot.workspace != f.scope.workspace_id
            || snapshot.update != f.capture
            || snapshot.source_pid != f.native_pid
            || snapshot.history.epoch != f.native_session
            || snapshot.history.revision != f.revision
            || snapshot.history.sequence != f.sequence
            || snapshot.history.checkpoint != f.checkpoint
            || snapshot.history.path != f.project_path
            || snapshot.history_sha256 != f.history_sha256
            || snapshot.basis["project_sha256"] != f.project_sha256
            || snapshot.basis["data_directory"] != serde_json::to_value(&self.body.basis.input.native_data).map_err(error)?
        {
            return Err(error("opaque native history differs from the exact selected original Save after-frame"));
        }
        let mut files = Vec::new();
        for name in ["saved-project", "preferences"] {
            let role = self.role(name)?;
            if role.provenance == "original-confirmed-publication" {
                let physical = role.physical.as_ref().ok_or_else(|| error("actual confirmed physical role required"))?;
                let mode = physical.mode.parse::<u32>().map_err(error)? & 0o777;
                files.push(json!({"role":name,"path":role.path,"sha256":role.sha256,"bytes":role.bytes,"mode":mode,"physical":physical}));
            }
        }
        let basis = if files.is_empty() {
            snapshot.basis.clone()
        } else {
            project_recovery::restore_basis(
                &snapshot.basis,
                &json!({"kind":"native_project_recovery_restoration","version":1,"snapshotSha256":f.sha256,"activationToken":self.body.activation_token,"files":files}),
                &f.sha256,
                &self.body.activation_token,
            )?
        };
        project_recovery::validate(&basis)?;
        Ok(basis)
    }
}
impl crate::project_save::operation::ColdSaveOrigin for Input {
    fn prepare_runtime(self: Arc<Self>, epoch: &str) -> Result<Box<dyn crate::project_save::operation::ColdRuntimePreparation>> {
        Ok(Box::new(runtime_transition::Pending::prepare(self, epoch)?))
    }
    fn current(&self) -> Result<()> {
        self.current(&media_guard::Check::new(web_time::Instant::now()))
    }
    fn description(&self) -> Result<Value> {
        let f = &self.body.basis.frame;
        let directory = self.native_directory.as_ref().ok_or_else(|| error("actual native directory is unavailable"))?;
        Ok(
            json!({"kind":"native_cold_Save_candidate","version":1,"scope":f.scope,"workspace":f.scope.workspace_id,"capture":f.capture,"sourceNativePid":f.native_pid,"sourceNativeSession":f.native_session,"afterFrameSha256":f.sha256,"privateInputSha256":self.reference_hash,"nativePid":std::process::id(),"projectPath":f.project_path,"revision":f.revision,"sequence":f.sequence,"editingPaused":true,"nativeHistory":"retained-original-constructor","nativeDirectory":directory.path(),"sourceNativeDirectory":directory.recorded(),"writeAuthority":false,"startupAuthority":false,"workersMayResume":false,"oldKeyImported":false,"persistenceStarted":false,"execution":"cold-Save-candidate-prepared"}),
        )
    }
}
// NativeRestore installs the original graph/history and source checkpoint.
// Mint this actor's epoch through the existing native identity service before
// installing the distinct Cold hold. The source epoch never becomes this one.
fn hold_fresh_candidate(candidate: &mut Session, origin: Arc<dyn crate::project_save::operation::ColdSaveOrigin>) -> Result<()> {
    if crate::agent_bridge::retained_native_epoch(candidate).is_some() {
        return Err(error("cold Save candidate already has a native epoch; no identity may be borrowed"));
    }
    origin.current()?;
    epoch(candidate)?;
    crate::project_save::operation::hold_cold_recovery(candidate, origin)
}
pub(super) fn open(s: &mut Session, p: &Value) -> Result<Value> {
    native_editor::require_fresh(s)?;
    let check = media_guard::Check::new(web_time::Instant::now());
    let input = Input::read(p, &check)?;
    let f = &input.body.basis.frame;
    let mut candidate = Session::new(s.services.clone());
    candidate.path = Some(f.project_path.clone());
    let (snapshot, size) = load(&candidate, &f.scope, &f.scope.workspace_id, &f.capture, &f.sha256, &check)?;
    if size as u64 != f.bytes || input.body.activation_token == snapshot.stage_token {
        return Err(error("exact after-frame and distinct new activation required"));
    }
    let basis = input.native_basis(&snapshot)?;
    NativeRestore {
        project_path: &f.project_path,
        scope: &f.scope,
        workspace: &f.scope.workspace_id,
        activation_token: &input.body.activation_token,
        history: &snapshot.history,
        editor: &snapshot.editor,
        basis: &basis,
    }
    .install(&mut candidate)?;
    input.current(&check)?;
    hold_fresh_candidate(&mut candidate, Arc::new(input))?;
    let view = crate::project_save::operation::cold_recovery_status(&candidate)?.ok_or_else(|| error("actual distinct cold Save hold missing"))?;
    check.check()?;
    *s = candidate;
    Ok(view)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cold_save_startup_refuses_malformed_copied_and_authority_bearing_references_without_installing_a_candidate() {
        for request in [
            Value::Null,
            json!({}),
            json!({"kind":"poema-original-Save-native-input-description","version":1,"writeAuthority":true}),
            json!({"kind":"native_cold_Save_candidate","version":1,"editingPaused":true}),
            json!({"path":"/unowned/input.json","maxBytes":usize::MAX}),
        ] {
            let mut s = Session::default();
            let graph = s.project.clone();
            let state = serde_json::to_value(&s.state).unwrap();
            assert!(open(&mut s, &request).is_err());
            assert!(Arc::ptr_eq(&graph, &s.project));
            assert_eq!(serde_json::to_value(&s.state).unwrap(), state);
            assert!(!s.save_operation_held());
            assert!(s.path.is_none());
            assert!(s.persistence.is_none());
        }
    }
    #[test]
    fn cold_save_startup_requires_virgin_native_state_before_reading_a_caller_path() {
        for fault in ["path", "revision", "epoch", "clipboard-owner"] {
            let mut s = Session::default();
            match fault {
                "path" => s.path = Some("/preserve/human.fcproj".into()),
                "revision" => s.revision = 2,
                "epoch" => s.agent_bridge.epoch = Some("a".repeat(32)),
                "clipboard-owner" => s.view_only = true,
                _ => panic!("known generated fault"),
            }
            let graph = s.project.clone();
            let before = (s.path.clone(), s.revision, s.agent_bridge.epoch.clone());
            assert!(open(&mut s, &json!({"path":"/never-read/input.json"})).is_err());
            assert!(Arc::ptr_eq(&graph, &s.project));
            assert_eq!((s.path.clone(), s.revision, s.agent_bridge.epoch.clone()), before);
        }
    }

    struct IdentityOrigin(bool);
    impl crate::project_save::operation::ColdSaveOrigin for IdentityOrigin {
        fn current(&self) -> Result<()> {
            if self.0 { Ok(()) } else { Err(error("generated identity origin unavailable")) }
        }
        fn description(&self) -> Result<Value> {
            self.current()?;
            Ok(json!({"kind":"native_cold_Save_candidate","writeAuthority":false,"workersMayResume":false}))
        }
    }
    #[test]
    fn fresh_cold_save_candidate_uses_original_native_epoch_service_before_installing_the_distinct_hold() {
        let mut identities = Vec::new();
        for _ in 0..2 {
            let mut candidate = Session::default();
            let graph = candidate.project.clone();
            let state = serde_json::to_value(&candidate.state).unwrap();
            let revision = candidate.revision;
            assert!(crate::agent_bridge::retained_native_epoch(&candidate).is_none());
            hold_fresh_candidate(&mut candidate, Arc::new(IdentityOrigin(true))).unwrap();
            let identity = crate::agent_bridge::retained_native_epoch(&candidate).unwrap().to_owned();
            assert_eq!(identity.len(), 32);
            assert!(identity.bytes().all(|c| c.is_ascii_hexdigit()));
            assert!(!identities.contains(&identity));
            let status = crate::project_save::operation::cold_recovery_status(&candidate).unwrap().unwrap();
            assert_eq!(status["nativeSession"], identity);
            assert_eq!(status["writeAuthority"], false);
            assert_eq!(status["workersMayResume"], false);
            assert!(candidate.save_operation_held());
            assert!(candidate.is_display_only());
            assert!(candidate.persistence.is_none());
            assert!(Arc::ptr_eq(&candidate.project, &graph));
            assert_eq!(candidate.revision, revision);
            assert_eq!(serde_json::to_value(&candidate.state).unwrap(), state);
            identities.push(identity);
        }
    }
    #[test]
    fn fresh_cold_save_candidate_refuses_borrowed_epoch_or_unavailable_origin_before_taking_a_hold() {
        for borrowed in [true, false] {
            let mut candidate = Session::default();
            if borrowed {
                epoch(&mut candidate).unwrap();
            }
            let before = candidate.agent_bridge.epoch.clone();
            let graph = candidate.project.clone();
            assert!(hold_fresh_candidate(&mut candidate, Arc::new(IdentityOrigin(borrowed))).is_err());
            assert_eq!(candidate.agent_bridge.epoch, before);
            assert!(Arc::ptr_eq(&candidate.project, &graph));
            assert!(!candidate.save_operation_held());
            assert!(candidate.persistence.is_none());
            drop(candidate.previews.pause_writers(&|| Ok(())).unwrap());
        }
    }
    #[test]
    fn cold_runtime_refuses_descriptive_origins_and_copied_receipts_without_changing_the_live_cold_owner() {
        let mut candidate = Session::default();
        hold_fresh_candidate(&mut candidate, Arc::new(IdentityOrigin(true))).unwrap();
        let before = candidate.cold_save_recovery_status().unwrap().unwrap();
        let graph = candidate.project.clone();
        assert!(candidate.prepare_cold_save_runtime().is_err());
        assert!(candidate.confirm_cold_save_runtime(&json!({"kind":"poema-original-Save-cold-runtime-intent-receipt","version":1})).is_err());
        assert_eq!(candidate.cold_save_recovery_status().unwrap().unwrap(), before);
        assert!(candidate.is_display_only());
        assert!(candidate.persistence.is_none());
        assert!(Arc::ptr_eq(&candidate.project, &graph));
    }
    #[test]
    fn cold_runtime_keeps_the_original_hold_when_the_native_epoch_is_replaced() {
        let mut candidate = Session::default();
        hold_fresh_candidate(&mut candidate, Arc::new(IdentityOrigin(true))).unwrap();
        let original = candidate.agent_bridge.epoch.clone();
        crate::agent_bridge::test_native_epoch(&mut candidate, true).unwrap();
        assert_ne!(candidate.agent_bridge.epoch, original);
        assert!(candidate.prepare_cold_save_runtime().is_err());
        assert!(candidate.confirm_cold_save_runtime(&json!({})).is_err());
        assert!(candidate.is_display_only());
        assert!(candidate.persistence.is_none());
        assert!(candidate.save_operation_held());
    }
    #[test]
    fn a_descriptive_cold_origin_cannot_create_or_confirm_a_persistence_worker() {
        let mut candidate = Session::default();
        hold_fresh_candidate(&mut candidate, Arc::new(IdentityOrigin(true))).unwrap();
        let before = candidate.cold_save_recovery_status().unwrap().unwrap();
        assert!(candidate.create_cold_save_persistence().is_err());
        assert!(candidate.confirm_cold_save_persistence(&json!({})).is_err());
        assert!(candidate.persistence.is_none());
        assert!(candidate.is_display_only());
        assert_eq!(candidate.cold_save_recovery_status().unwrap().unwrap(), before);
    }
}
