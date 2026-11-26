//! Read complete current images through the original native constructor.
//! Observations remain observations: no Record, Commit, writer or grant is
//! deserialized. A restored candidate opens paused; actual fresh-manager controls
//! must independently admit the original native activation and persistence owners.
use super::*;
use filmcraft_format::retained::{Observation, preview_context::check_owner_exited};
use std::collections::BTreeSet;
mod admission;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Descriptor {
    capture: String,
    sha256: String,
    bytes: u64,
    native_pid: u32,
    native_session: String,
    activation_token: String,
    observation: Observation,
    storage_roots: Vec<DirectoryRecord>,
    reserve_bytes: u64,
    #[serde(default)]
    restoration: Option<admission::Reference>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Body {
    kind: String,
    version: u8,
    context: Context,
    history: history_checkpoint::Capsule,
    editor: super::super::super::native_editor::Snapshot,
    files: Value,
    basis: Value,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct FilesRecord {
    saved_path: PathBuf,
    saved_state: NodeState,
    directories: Vec<DirectoryRecord>,
    nodes: BTreeMap<PathBuf, NodeState>,
    source: SourceRecord,
    #[serde(default)]
    library_publication: Option<Value>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SourceRecord {
    kind: String,
    version: u8,
    roots: Vec<DirectoryRecord>,
    directories: Vec<SourceDirectory>,
    nodes: BTreeMap<PathBuf, NodeState>,
    persistence: Option<Value>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SourceDirectory {
    frame: DirectoryRecord,
    names: Vec<String>,
    persistence: bool,
}
pub(in crate::agent_bridge) struct Expected<'a> {
    pub scope: &'a crate::project_workspace::ProjectScope,
    pub workspace: &'a str,
    pub update: &'a str,
    pub project_path: &'a str,
    pub activation_token: &'a str,
    pub minimum_revision: u64,
    pub source_snapshot: &'a str,
    pub saved_file: &'a Value,
}
struct Membership {
    frame: Arc<DirectoryFrame>,
    names: Vec<String>,
    persistence: bool,
}
pub(in crate::agent_bridge) struct Input {
    pid: u32,
    context: Context,
    capture: String,
    sha256: String,
    bytes: u64,
    source_snapshot: String,
    restored_files: bool,
    roots: Vec<Arc<DirectoryFrame>>,
    directories: Vec<Arc<DirectoryFrame>>,
    nodes: Vec<NodeFrame>,
    membership: Vec<Membership>,
    reserve: u64,
    basis: Value,
    // Includes all original library intents, receipts, displaced observations
    // and unknown staging. Keeping this data never recreates their old owner.
    _files: Value,
}
pub(in crate::agent_bridge) struct Loaded {
    pub history: history_checkpoint::Capsule,
    pub editor: super::super::super::native_editor::Snapshot,
    pub basis: Value,
    pub input: Arc<Input>,
}
fn fail(e: impl std::fmt::Display) -> crate::EngineError {
    bad("native.currentImageStartup", e.to_string())
}
fn hash_ok(v: &str) -> bool {
    v.len() == 64 && v.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
fn within(path: &Path, roots: &[DirectoryRecord]) -> bool {
    path.is_absolute()
        && path.to_str().is_some_and(|v| v.len() <= 16384 && !v.chars().any(char::is_control))
        && path.components().all(|v| matches!(v, std::path::Component::RootDir | std::path::Component::Normal(_)))
        && roots.iter().any(|r| path == r.path || path.starts_with(&r.path))
}
fn directory_record(record: &DirectoryRecord, roots: &[DirectoryRecord]) -> Result<()> {
    if !within(&record.path, roots)
        || !within(&record.anchor.path, roots)
        || record.missing.len() > MAX_DEPTH
        || record.missing.iter().any(|p| !within(p, roots))
    {
        return Err(fail("retained directory escapes explicit original working roots"));
    }
    Ok(())
}
impl Input {
    pub(super) fn activation_description(&self, s: &Session, root: &DirectoryFrame) -> Result<Value> {
        let h = s.agent_bridge.workspace_update.as_ref().ok_or_else(|| fail("actual current candidate required"))?;
        if h.workspace != self.context.workspace
            || h.update != self.context.update
            || h.scope != LifecycleScope::Project(self.context.scope.clone())
            || s.path.as_deref() != Some(self.context.project_path.as_str())
            || h.native_session != s.agent_bridge.epoch.as_deref().unwrap_or("")
            || h.revision != s.revision
            || h.token == self.context.activation_token
            || !h.candidate
            || h.candidate_cancelled
            || h.retiring
            || h.persistence_start.as_ref().is_none_or(|cfg| cfg.data_dir != root.record().path)
        {
            return Err(fail("current image cannot cross its actual fresh candidate"));
        }
        Ok(json!({"capture":self.capture,"sha256":self.sha256,"bytes":self.bytes,"sourceSnapshot":self.source_snapshot,"restoredFiles":self.restored_files,
            "scope":self.context.scope,"workspace":self.context.workspace,"update":self.context.update,"projectPath":self.context.project_path,
            "producer":{"pid":self.context.native_pid,"nativeSession":self.context.native_session,"activationToken":self.context.activation_token,"dataDir":self.context.data_directory},
            "candidate":{"pid":std::process::id(),"nativeSession":h.native_session,"activationToken":h.token,"dataDir":root.record().path,"revision":s.revision}}))
    }
    pub(super) fn reserve(&self) -> u64 {
        self.reserve
    }
    pub(super) fn owner_exited(&self) -> Result<()> {
        check_owner_exited(self.pid).map_err(fail)
    }
    pub(in crate::agent_bridge::workspace_update) fn preflight_directory(&self, path: &Path, check: &media_guard::Check) -> Result<()> {
        self.current(check)?;
        let parent = path.parent().ok_or_else(|| fail("actual new directory parent required"))?;
        let io = || check.check().map_err(|e| io::Error::other(e.to_string()));
        let frame = DirectoryFrame::capture(parent, &io).map_err(fail)?;
        if !path.is_absolute()
            || path.components().any(|c| !matches!(c, std::path::Component::RootDir | std::path::Component::Normal(_)))
            || !self.roots.iter().any(|r| path.starts_with(r.record().path) && r.record().anchor.identity.device == frame.record().anchor.identity.device)
            || self.membership.iter().any(|r| path.starts_with(r.frame.record().path))
            || self.nodes.iter().any(|r| r.path() == path)
        {
            return Err(fail("new activation directory escapes its explicit original volume or overlaps retained input"));
        }
        Ok(())
    }
    pub(super) fn fresh_directory(&self, fresh: &DirectoryFrame, check: &media_guard::Check) -> Result<()> {
        self.current(check)?;
        let record = fresh.record();
        let path = record.path.as_path();
        if !self.roots.iter().any(|r| path.starts_with(r.record().path) && r.record().anchor.identity.device == record.anchor.identity.device)
            || self.membership.iter().any(|r| path.starts_with(r.frame.record().path))
            || self.nodes.iter().any(|r| r.path() == path)
        {
            return Err(fail("new activation directory overlaps an original retained namespace"));
        }
        Ok(())
    }
    /// Select only the original resource enumerator's explicit authored families.
    /// Every returned description must still match a live admitted NodeFrame.
    pub(super) fn authored_members(&self) -> Result<Vec<(PathBuf, PathBuf, NodeState)>> {
        let files: FilesRecord = serde_json::from_value(self._files.clone()).map_err(fail)?;
        let mut roots = files.source.roots.into_iter().map(|r| r.path).collect::<BTreeSet<_>>();
        roots.insert(self.context.data_directory.clone());
        if roots.len() > 17 {
            return Err(fail("authored resource root bound exceeded"));
        }
        let mut result = Vec::new();
        for root in roots {
            for node in &self.nodes {
                let Ok(relative) = node.path().strip_prefix(&root).map(Path::to_path_buf) else { continue };
                let family = relative.components().next().and_then(|c| c.as_os_str().to_str()).unwrap_or("");
                if !super::resources::NAMES.contains(&family)
                    || relative == Path::new("Keyboard Shortcuts/active.json")
                    || relative == Path::new(crate::presets::LIBRARY_FILE)
                    || relative == Path::new(crate::export_tools::LIBRARY_FILE)
                    || matches!(node.state(), NodeState::Absent)
                {
                    continue;
                }
                if result.len() >= MAX_NODES {
                    return Err(fail("authored resource selection exceeds the original node bound"));
                }
                result.push((relative, node.path(), node.state().clone()));
            }
        }
        Ok(result)
    }
    pub(super) fn matches_source(&self, frame: &NodeFrame) -> bool {
        self.nodes.iter().any(|n| n.path() == frame.path() && n.state() == frame.state())
            || matches!(frame.state(), NodeState::Absent)
                && self.nodes.iter().any(|n| matches!(n.state(), NodeState::Absent) && frame.path().starts_with(n.path()))
            || matches!(frame.state(), NodeState::Absent)
                && self.membership.iter().any(|r| {
                    frame.path().parent() == Some(r.frame.record().path.as_path())
                        && frame.path().file_name().and_then(|v| v.to_str()).is_some_and(|name| !r.names.iter().any(|v| v == name))
                })
    }
    pub(in crate::agent_bridge) fn current(&self, check: &media_guard::Check) -> Result<()> {
        check_owner_exited(self.pid).map_err(fail)?;
        let io = || check.check().map_err(|e| io::Error::other(e.to_string()));
        for root in &self.roots {
            root.current(&io).map_err(fail)?;
            let space = root.space(&io).map_err(fail)?;
            if space.read_only || space.free < self.reserve.saturating_add(MAX_BODY as u64) {
                return Err(fail("original working volume is read-only or below its reserve"));
            }
        }
        for directory in &self.directories {
            directory.current(&io).map_err(fail)?;
        }
        for node in &self.nodes {
            node.current(&io).map_err(fail)?;
            node.fence(&io).map_err(fail)?;
        }
        for scope in &self.membership {
            scope.frame.current(&io).map_err(fail)?;
            let path = scope.frame.record().path;
            let mut current = super::names(&path, check)?;
            if scope.persistence && std::fs::symlink_metadata(path.join(".DS_Store")).is_ok_and(|m| m.is_file()) {
                current.retain(|n| n != ".DS_Store");
            }
            if current != scope.names {
                return Err(fail("original current-image namespace changed"));
            }
        }
        crate::agent_bridge::project_recovery::validate(&self.basis)?;
        check_owner_exited(self.pid).map_err(fail)?;
        check.check()
    }
}
pub(in crate::agent_bridge) fn load(value: &Value, expected: Expected<'_>, check: &media_guard::Check) -> Result<Loaded> {
    history_checkpoint::bytes(value, 16 * 1024)?;
    let d: Descriptor = parse("native.currentImageStartup", value)?;
    if !uuid(&d.capture)
        || !hash_ok(&d.sha256)
        || d.bytes == 0
        || d.bytes > MAX_BODY as u64
        || !token_ok(&d.activation_token)
        || d.activation_token == expected.activation_token
        || !filmcraft_project::ClipIdentity::valid_id(&d.native_session)
        || d.native_pid == 0
        || d.native_pid > i32::MAX as u32
        || d.native_pid == std::process::id()
        || d.storage_roots.is_empty()
        || d.storage_roots.len() > 16
        || d.reserve_bytes == 0
        || d.reserve_bytes > 9_007_199_254_740_991
        || d.observation.sha256 != d.sha256
        || d.observation.bytes != d.bytes
    {
        return Err(fail("exact complete current image, exited prior PID and explicit working storage required"));
    }
    check_owner_exited(d.native_pid).map_err(fail)?;
    let io = || check.check().map_err(|e| io::Error::other(e.to_string()));
    let mut root_frames = Vec::new();
    let mut root_paths = BTreeSet::new();
    for root in &d.storage_roots {
        if !root.missing.is_empty() || root.anchor.path != root.path || !root_paths.insert(root.path.clone()) {
            return Err(fail("distinct actual existing working roots required"));
        }
        directory_record(root, &d.storage_roots)?;
        let frame = DirectoryFrame::capture(&root.path, &io).map_err(fail)?;
        if frame.record() != *root {
            return Err(fail("original working root identity changed"));
        }
        let space = frame.space(&io).map_err(fail)?;
        if space.read_only || space.free < d.reserve_bytes.saturating_add(MAX_BODY as u64) {
            return Err(fail("working reserve unavailable"));
        }
        root_frames.push(frame);
    }
    let project = Path::new(expected.project_path);
    if !within(project, &d.storage_roots) {
        return Err(fail("native project is outside working roots"));
    }
    let parent = project.parent().ok_or_else(|| fail("actual native project parent required"))?;
    let directory = DirectoryFrame::capture(parent, &io).map_err(fail)?;
    let name = format!(".poema-project-recovery-{}-{}.json", expected.workspace, d.capture);
    let meta = std::fs::symlink_metadata(parent.join(&name)).map_err(fail)?;
    if !meta.is_file() || meta.len() != d.bytes {
        return Err(fail("bounded complete original image file required"));
    }
    let frame = directory.node(&name, &io).map_err(fail)?;
    if frame.state() != &NodeState::File(d.observation.clone()) {
        return Err(fail("original complete private image file changed"));
    }
    let raw = frame.read_private_bytes(MAX_BODY as u64, &io).map_err(fail)?;
    if raw.len() as u64 != d.bytes || digest(&raw) != d.sha256 {
        return Err(fail("complete private current-image bytes differ"));
    }
    let mut body: Body = serde_json::from_slice(&raw).map_err(fail)?;
    if history_checkpoint::bytes_checked(&body, MAX_BODY, Some(check))? != raw {
        return Err(fail("noncanonical current-image values"));
    }
    let c = &body.context;
    if body.kind != "native-paused-current-image"
        || body.version != 2
        || c.scope != *expected.scope
        || c.workspace != expected.workspace
        || c.update != expected.update
        || c.project_path != expected.project_path
        || c.native_pid != d.native_pid
        || c.native_session != d.native_session
        || c.activation_token != d.activation_token
        || c.revision < expected.minimum_revision
        || c.revision == 0
        || c.revision >= u64::MAX - 1
        || body.basis["saved_file"] != *expected.saved_file
        || body.basis["scope"] != serde_json::to_value(&c.scope).map_err(fail)?
        || !body.basis.is_object()
        || !within(&c.data_directory, &d.storage_roots)
    {
        return Err(fail("current image crossed its original native context or lacks a complete recovery basis"));
    }
    history_checkpoint::validate_current_image_context(&body.history, &c.scope, &c.workspace, &c.project_path, &c.native_session, c.revision, c.sequence)?;
    body.editor.validate(c.revision, c.sequence)?;
    history_checkpoint::bytes(&body.basis, MAX_META)?;
    for key in ["data_directory"] {
        let p = body.basis[key].as_str().ok_or_else(|| fail("complete recovery directory missing"))?;
        if !within(Path::new(p), &d.storage_roots) {
            return Err(fail("basis directory escapes original working roots"));
        }
    }
    for key in ["preferences_file", "journal_companion"] {
        if !body.basis[key].is_null() {
            let p = body.basis[key]["path"].as_str().ok_or_else(|| fail("complete recovery input path missing"))?;
            if !within(Path::new(p), &d.storage_roots) {
                return Err(fail("basis input escapes original working roots"));
            }
        }
    }
    history_checkpoint::bytes(&body.files, MAX_META)?;
    let files: FilesRecord = serde_json::from_value(body.files.clone()).map_err(fail)?;
    let mut restoration = d.restoration.as_ref().map(|r| admission::Admission::load(r, &d, &expected, check)).transpose()?;
    if files.saved_path != project
        || !matches!(&files.saved_state, NodeState::File(v) if Some(v.sha256.as_str()) == expected.saved_file["sha256"].as_str())
        || files.source.kind != "native-original-resource-frames"
        || files.source.version != 1
        || !files.directories.iter().any(|r| r.path == c.data_directory && r.missing.is_empty())
        || files
            .directories
            .len()
            .saturating_add(files.nodes.len())
            .saturating_add(files.source.roots.len())
            .saturating_add(files.source.directories.len())
            .saturating_add(files.source.nodes.len())
            > MAX_NODES
    {
        return Err(fail("complete original current-image file closure required"));
    }
    if let Some(effects) = &files.library_publication {
        if effects["kind"] != "native-original-library-publication-effects"
            || effects["version"] != 1
            || ["writeAuthority", "startupAuthority", "retirementAuthority", "workersMayResume", "replayAllowed"].iter().any(|k| effects[*k] != false)
        {
            return Err(fail("library observations cannot become old owner or replay authority"));
        }
    }
    let top_paths = files.directories.iter().map(|r| r.path.clone()).collect::<BTreeSet<_>>();
    if files.nodes.keys().any(|p| p.parent().is_none_or(|p| !top_paths.contains(p))) {
        return Err(fail("unindexed current-image file parent"));
    }
    let mut descriptors = files.source.roots.clone();
    descriptors.extend(files.directories.clone());
    descriptors.extend(files.source.directories.iter().map(|r| r.frame.clone()));
    let mut nodes = files.nodes.clone();
    for (path, state) in &files.source.nodes {
        if nodes.get(path).is_some_and(|v| v != state) {
            return Err(fail("conflicting original file observations"));
        }
        nodes.insert(path.clone(), state.clone());
    }
    if nodes.get(&files.saved_path).is_some_and(|v| v != &files.saved_state) {
        return Err(fail("conflicting saved-file observation"));
    }
    nodes.insert(files.saved_path.clone(), files.saved_state);
    for record in &descriptors {
        directory_record(record, &d.storage_roots)?;
    }
    if nodes.keys().any(|p| !within(p, &d.storage_roots)) {
        return Err(fail("file observation escapes explicit working roots"));
    }
    if let Some(admitted) = &restoration {
        let path = admitted.path();
        if path == project
            || path == frame.path()
            || nodes.contains_key(&path)
            || files.directories.iter().any(|r| path.starts_with(&r.path))
            || files.source.directories.iter().any(|r| path.starts_with(&r.frame.path))
        {
            return Err(fail("current restoration input overlaps original observed files or namespace"));
        }
    }
    let mut directories = Vec::new();
    for record in &descriptors {
        let actual = DirectoryFrame::capture(&record.path, &io).map_err(fail)?;
        let admitted = restoration.as_mut().map(|r| r.directory(record, &actual.record())).transpose()?.unwrap_or(false);
        if !admitted && actual.record() != *record {
            return Err(fail("original current-image directory generation changed"));
        }
        directories.push(actual);
    }
    let mut membership = Vec::new();
    for record in &files.directories {
        let names = files
            .nodes
            .keys()
            .filter(|p| p.parent() == Some(record.path.as_path()))
            .map(|p| p.file_name().and_then(|n| n.to_str()).map(str::to_owned).ok_or_else(|| fail("literal current-image member required")))
            .collect::<Result<Vec<_>>>()?;
        membership.push(Membership { frame: DirectoryFrame::capture(&record.path, &io).map_err(fail)?, names, persistence: false });
    }
    for scope in &files.source.directories {
        if scope.names.len() > MAX_NODES || scope.names.windows(2).any(|w| w.first() >= w.get(1)) {
            return Err(fail("complete ordered original membership required"));
        }
        membership.push(Membership {
            frame: DirectoryFrame::capture(&scope.frame.path, &io).map_err(fail)?,
            names: scope.names.clone(),
            persistence: scope.persistence,
        });
    }
    let mut node_frames = vec![frame];
    for (path, state) in nodes {
        let parent = path.parent().ok_or_else(|| fail("literal retained parent required"))?;
        let actual = DirectoryFrame::capture(parent, &io)
            .map_err(fail)?
            .node(path.file_name().and_then(|v| v.to_str()).ok_or_else(|| fail("literal retained name required"))?, &io)
            .map_err(fail)?;
        let admitted = restoration.as_mut().map(|r| r.node(&path, &state, actual.state())).transpose()?.unwrap_or(false);
        if !admitted && actual.state() != &state {
            return Err(fail("original current-image node generation changed"));
        }
        node_frames.push(actual);
    }
    if let Some(admitted) = restoration {
        body.basis = admitted.basis(&body.basis, expected.source_snapshot, expected.activation_token)?;
        node_frames.push(admitted.finish()?);
    }
    crate::agent_bridge::project_recovery::validate(&body.basis)?;
    if crate::agent_bridge::project_recovery::persistence_dependencies(&body.basis)? != files.source.persistence.unwrap_or(Value::Null) {
        return Err(fail("complete original persistence inventory changed"));
    }
    let input = Arc::new(Input {
        pid: d.native_pid,
        context: body.context.clone(),
        capture: d.capture,
        sha256: d.sha256,
        bytes: d.bytes,
        source_snapshot: expected.source_snapshot.into(),
        restored_files: d.restoration.is_some(),
        roots: root_frames,
        directories,
        nodes: node_frames,
        membership,
        reserve: d.reserve_bytes,
        basis: body.basis.clone(),
        _files: body.files,
    });
    input.current(check)?;
    Ok(Loaded { history: body.history, editor: body.editor, basis: body.basis, input })
}
