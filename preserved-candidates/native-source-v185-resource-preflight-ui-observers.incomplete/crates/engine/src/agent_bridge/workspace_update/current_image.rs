//! A current paused image owns real native COW values and real file frames.
//! Metadata can observe this object, never reconstruct it or retire a Session.
use super::*;
use filmcraft_format::retained::preview_context::{DirectoryFrame, DirectoryRecord, NodeFrame, NodeState};
use serde::Serialize;
use std::{
    collections::BTreeMap,
    io,
    path::{Path, PathBuf},
    sync::Arc,
};
pub(super) mod activation;
mod archive_inventory;
mod authored;
pub(super) mod publication;
mod resources;
pub(super) mod restore;
pub(super) const RECORD_COMMAND: &str = publication::COMMAND;
pub(super) fn prepare_record(s: &mut Session, p: &Value) -> Result<()> {
    publication::prepare(s, p)
}
pub(super) fn run_record(s: &mut Session, p: &Value) -> Result<Value> {
    publication::run(s, p)
}
pub(super) const COMMAND: &str = "agent.preparePausedProjectRecovery";
const MAX_NODES: usize = 4096;
const MAX_DEPTH: usize = 64;
const MAX_META: usize = 512 * 1024;
const MAX_BODY: usize = 80 * 1024 * 1024;
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Request {
    token: String,
    scope: crate::project_workspace::ProjectScope,
    workspace: String,
    update: String,
    project_path: String,
    activation_token: String,
    native_session: String,
    capture: String,
    action: String,
}
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Context {
    scope: crate::project_workspace::ProjectScope,
    workspace: String,
    update: String,
    project_path: String,
    activation_token: String,
    native_session: String,
    native_pid: u32,
    revision: u64,
    sequence: Option<u64>,
    data_directory: PathBuf,
}
fn context(s: &Session) -> Result<Context> {
    let h = s.agent_bridge.workspace_update.as_ref().ok_or_else(|| bad(COMMAND, "actual paused candidate required"))?;
    let LifecycleScope::Project(scope) = &h.scope else {
        return Err(bad(COMMAND, "independent paused Root required"));
    };
    if !h.candidate
        || h.candidate_cancelled
        || h.retiring
        || h.restored_current_image.is_some()
        || s.view_only
        || s.persistence.is_some()
        || s.save_operation_held()
        || !s.worker_available
        || !operation_cancellation_available(s)
        || s.agent_bridge.epoch.as_deref() != Some(h.native_session.as_str())
        || h.revision != s.revision
    {
        return Err(bad(COMMAND, "current paused candidate with persistence deferred and no pending Save required"));
    }
    history_checkpoint::validate_retained_basis_graph(s, &h.basis)?;
    require_idle_writer_jobs(s)?;
    bundle::check_saved_project(s)?;
    let cfg = h.persistence_start.as_ref().ok_or_else(|| bad(COMMAND, "actual deferred native data directory required"))?;
    Ok(Context {
        scope: scope.clone(),
        workspace: h.workspace.clone(),
        update: h.update.clone(),
        project_path: s.path.clone().ok_or_else(|| bad(COMMAND, "actual saved native path required"))?,
        activation_token: h.token.clone(),
        native_session: h.native_session.clone(),
        native_pid: std::process::id(),
        revision: s.revision,
        sequence: s.state.active_sequence.map(|v| v.0),
        data_directory: cfg.data_dir.clone(),
    })
}
fn request(s: &Session, p: &Value) -> Result<(Request, Context)> {
    let r: Request = parse(COMMAND, p)?;
    let c = context(s)?;
    if !token_ok(&r.token)
        || r.token == c.activation_token
        || !uuid(&r.capture)
        || !matches!(r.action.as_str(), "prepare" | "read" | "dependencies")
        || r.scope != c.scope
        || r.workspace != c.workspace
        || r.update != c.update
        || r.project_path != c.project_path
        || r.activation_token != c.activation_token
        || r.native_session != c.native_session
        || history_checkpoint::has_basis(s, &r.token)
        || s.agent_bridge.tickets.iter().any(|t| t.token == r.token)
    {
        return Err(bad(COMMAND, "exact paused scope, epoch, fresh operation and capture identity required"));
    }
    Ok((r, c))
}
pub(super) fn prepare(s: &mut Session, p: &Value) -> Result<()> {
    let (r, _) = request(s, p)?;
    s.agent_bridge.operations.register(&r.token)
}
struct DirectoryScope {
    frame: Arc<DirectoryFrame>,
    names: Vec<String>,
}
struct Files {
    saved: NodeFrame,
    saved_sha256: String,
    source: resources::Resources,
    library: super::library_commit::Observation,
    directories: Vec<DirectoryScope>,
    nodes: Vec<NodeFrame>,
    records: Value,
}
fn io_error(e: io::Error) -> crate::EngineError {
    bad(COMMAND, e.to_string())
}
fn names(path: &Path, check: &media_guard::Check) -> Result<Vec<String>> {
    let mut values = Vec::new();
    for entry in std::fs::read_dir(path).map_err(io_error)? {
        check.check()?;
        if values.len() >= MAX_NODES {
            return Err(bad(COMMAND, "original resource entry bound exceeded"));
        }
        let name = entry.map_err(io_error)?.file_name().into_string().map_err(|_| bad(COMMAND, "literal UTF-8 resource name required"))?;
        values.push(name);
    }
    values.sort();
    Ok(values)
}
impl Files {
    fn capture(s: &Session, c: &Context, check: &media_guard::Check) -> Result<Self> {
        let io = || check.check().map_err(|e| io::Error::other(e.to_string()));
        let saved = s.saved_file.as_ref().filter(|b| b.path == c.project_path).ok_or_else(|| bad(COMMAND, "actual native saved baseline required"))?;
        let path = Path::new(&saved.path);
        let parent = DirectoryFrame::capture(path.parent().ok_or_else(|| bad(COMMAND, "saved parent required"))?, &io).map_err(io_error)?;
        let file =
            parent.node(path.file_name().and_then(|v| v.to_str()).ok_or_else(|| bad(COMMAND, "literal saved filename required"))?, &io).map_err(io_error)?;
        if !matches!(file.state(),NodeState::File(v)if v.sha256==saved.sha256) {
            return Err(bad(COMMAND, "native saved baseline differs from actual complete file"));
        }
        let root = DirectoryFrame::capture(&c.data_directory, &io).map_err(io_error)?;
        if !root.exists() {
            return Err(bad(COMMAND, "actual deferred directory must exist"));
        }
        let device = root.record().anchor.identity.device;
        let mut pending = vec![(root, 0usize)];
        let mut directories = Vec::new();
        let mut nodes = Vec::new();
        let mut records = BTreeMap::new();
        while let Some((dir, depth)) = pending.pop() {
            check.check()?;
            if depth > MAX_DEPTH || directories.len().saturating_add(nodes.len()) >= MAX_NODES {
                return Err(bad(COMMAND, "original complete resource tree bound exceeded"));
            }
            let record = dir.record();
            if record.anchor.identity.device != device {
                return Err(bad(COMMAND, "deferred resource tree crosses its original volume"));
            }
            let entries = names(&record.path, check)?;
            for name in &entries {
                if directories.len().saturating_add(nodes.len()) >= MAX_NODES {
                    return Err(bad(COMMAND, "original complete resource tree bound exceeded"));
                }
                let node = dir.node(name, &io).map_err(io_error)?;
                let path = node.path();
                match node.state() {
                    NodeState::Absent => return Err(bad(COMMAND, "resource disappeared during capture")),
                    NodeState::Directory(_) => pending.push((DirectoryFrame::capture(&path, &io).map_err(io_error)?, depth.saturating_add(1))),
                    NodeState::File(_) => {}
                }
                records.insert(path, serde_json::to_value(node.state()).map_err(|e| bad(COMMAND, e.to_string()))?);
                nodes.push(node);
            }
            directories.push(DirectoryScope { frame: dir, names: entries });
        }
        let dir_records = directories.iter().map(|d| d.frame.record()).collect::<Vec<DirectoryRecord>>();
        let source = resources::Resources::capture(s, check, MAX_NODES.saturating_sub(directories.len().saturating_add(nodes.len())))?;
        let library = super::library_commit::Observation::capture(s, &io)?;
        let mut records = json!({"savedPath":file.path(),"savedState":file.state(),"directories":dir_records,"nodes":records,"source":source.records});
        if !library.record.is_null() {
            records["libraryPublication"] = library.record.clone();
        }
        history_checkpoint::bytes(&records, MAX_META)?;
        let result = Self { saved: file, saved_sha256: saved.sha256.clone(), source, library, directories, nodes, records };
        result.current(s, c, check)?;
        Ok(result)
    }
    fn current(&self, s: &Session, c: &Context, check: &media_guard::Check) -> Result<()> {
        let io = || check.check().map_err(|e| io::Error::other(e.to_string()));
        if s.saved_file.as_ref().is_none_or(|b| b.path != c.project_path || b.sha256 != self.saved_sha256) {
            return Err(bad(COMMAND, "original native saved baseline changed"));
        }
        self.saved.current(&io).map_err(io_error)?;
        self.source.current(s, check)?;
        self.library.current(s, &io)?;
        for dir in &self.directories {
            dir.frame.current(&io).map_err(io_error)?;
            if names(&dir.frame.record().path, check)? != dir.names {
                return Err(bad(COMMAND, "original resource namespace changed"));
            }
        }
        for node in &self.nodes {
            node.current(&io).map_err(io_error)?;
        }
        self.saved.fence(&io).map_err(io_error)?;
        for node in &self.nodes {
            node.fence(&io).map_err(io_error)?;
        }
        check.check()
    }
}
#[derive(Serialize)]
struct Payload<'a> {
    kind: &'static str,
    version: u8,
    context: &'a Context,
    history: &'a history_checkpoint::Capsule,
    editor: &'a super::super::native_editor::Snapshot,
    files: &'a Value,
    basis: Option<&'a Value>,
}
pub(super) struct Image {
    capture: String,
    context: Context,
    history: history_checkpoint::PendingCapsule,
    editor: super::super::native_editor::Snapshot,
    files: Files,
    sha256: String,
    bytes: usize,
    publication: std::sync::Mutex<Option<publication::Publication>>,
}
fn digest(raw: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    format!("{:x}", Sha256::digest(raw))
}
fn encode(
    c: &Context,
    history: &history_checkpoint::PendingCapsule,
    editor: &super::super::native_editor::Snapshot,
    files: &Files,
    check: &media_guard::Check,
) -> Result<Vec<u8>> {
    let history = history.encode_checked(Some(check))?;
    history_checkpoint::bytes_checked(&history, 32 * 1024 * 1024, Some(check))?;
    history_checkpoint::bytes_checked(
        &Payload {
            kind: "native-paused-current-image",
            version: 2,
            context: c,
            history: &history,
            editor,
            files: &files.records,
            basis: files.source.basis.as_ref(),
        },
        MAX_BODY,
        Some(check),
    )
}
impl Image {
    fn capture(s: &Session, r: &Request, c: Context, check: &media_guard::Check) -> Result<Self> {
        let files = Files::capture(s, &c, check)?;
        let history = history_checkpoint::capture_paused_current_history(s, &c.scope, &r.capture)?;
        let editor = super::super::native_editor::Snapshot::capture_complete(s)?;
        let raw = encode(&c, &history, &editor, &files, check)?;
        let image = Self {
            capture: r.capture.clone(),
            context: c,
            history,
            editor,
            files,
            sha256: digest(&raw),
            bytes: raw.len(),
            publication: std::sync::Mutex::new(None),
        };
        drop(raw);
        image.current(s, check)?;
        Ok(image)
    }
    fn current(&self, s: &Session, check: &media_guard::Check) -> Result<()> {
        let original = encode(&self.context, &self.history, &self.editor, &self.files, check)?;
        self.current_against(s, &original, check)
    }
    // Publication already owns these complete original bytes. Compare directly
    // against that same buffer, rather than allocating another original body.
    fn current_against(&self, s: &Session, original: &[u8], check: &media_guard::Check) -> Result<()> {
        if context(s)? != self.context {
            return Err(bad(COMMAND, "original paused native lifecycle changed"));
        }
        self.files.current(s, &self.context, check)?;
        if original.len() != self.bytes || digest(original) != self.sha256 {
            return Err(bad(COMMAND, "original current-image values changed"));
        }
        let history = history_checkpoint::capture_paused_current_history(s, &self.context.scope, &self.capture)?;
        let editor = super::super::native_editor::Snapshot::capture_complete(s)?;
        let current = encode(&self.context, &history, &editor, &self.files, check)?;
        if current.as_slice() != original {
            return Err(bad(COMMAND, "actual current editor or history changed after image preparation"));
        }
        self.files.current(s, &self.context, check)
    }
    fn receipt(&self, r: &Request) -> Value {
        json!({"status":"prepared","kind":"native_paused_current_recovery","version":1,"token":r.token,
            "scope":self.context.scope,"workspace":self.context.workspace,"update":self.context.update,"projectPath":self.context.project_path,
            "activationToken":self.context.activation_token,"nativeSession":self.context.native_session,"nativePid":std::process::id(),
            "capture":self.capture,"sha256":self.sha256,"bytes":self.bytes,"revision":self.context.revision,"sequence":self.context.sequence,
            "dataDirectory":self.context.data_directory,"resourceNodes":self.files.nodes.len(),"editingPaused":true,
            "projectUnchanged":true,"historyUnchanged":true,"editorUnchanged":true,"writeAuthority":false,"retirementAuthority":false,
            "startupAuthority":false,"sourceValidation":"not-performed","mediaEvidence":false})
    }
}
pub(super) fn run(s: &mut Session, p: &Value) -> Result<Value> {
    let (r, c) = request(s, p)?;
    let check = s.agent_bridge.operations.check().ok_or_else(|| bad(COMMAND, "original guarded read operation required"))?;
    if let Some(image) = s.agent_bridge.workspace_update.as_ref().and_then(|h| h.current_image.clone()) {
        if image.capture != r.capture {
            return Err(bad(COMMAND, "another exact current image is retained; no implicit replacement"));
        }
        image.current(s, &check)?;
        if r.action == "dependencies" {
            let inventory = history_checkpoint::current_recovery_dependencies(s, &image.history, &check)?;
            let mut receipt = image.receipt(&r);
            receipt["kind"] = json!("native_paused_current_recovery_dependencies");
            receipt["status"] = json!("metadata");
            receipt["version"] = json!(2);
            receipt["resourceInventory"] = archive_inventory::read(&image.files, &check)?;
            receipt["mediaInventory"] = inventory;
            // Preserve every native integer exactly; JavaScript must not decode
            // and re-encode filesystem generations as floating-point numbers.
            let frames = history_checkpoint::bytes_checked(&image.files.records, MAX_META, Some(&check))?;
            let frames_hash = digest(&frames);
            let frames_bytes = frames.len();
            let frames_json = String::from_utf8(frames).map_err(|e| bad(COMMAND, e.to_string()))?;
            receipt["fileFrames"] =
                json!({"kind":"native_paused_current_file_frames","version":1,"json":frames_json,"sha256":frames_hash,"bytes":frames_bytes});
            history_checkpoint::bytes_checked(&receipt, 8 * 1024 * 1024, Some(&check))?;
            image.current(s, &check)?;
            return Ok(receipt);
        }
        return Ok(image.receipt(&r));
    }
    if r.action != "prepare" {
        return Err(bad(COMMAND, "no original live image exists; metadata cannot reconstruct one"));
    }
    let image = Arc::new(Image::capture(s, &r, c, &check)?);
    check.check()?;
    s.agent_bridge.operations.enter_publication(&r.token)?;
    let receipt = image.receipt(&r);
    let h = s.agent_bridge.workspace_update.as_mut().ok_or_else(|| bad(COMMAND, "original pause disappeared"))?;
    h.current_image = Some(image);
    Ok(receipt)
}
