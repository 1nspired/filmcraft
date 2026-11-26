//! Original authored resource and recovery inputs retained as real file frames.
//! Descriptions never grant write, native worker or cold-start authority.
use super::*;
use std::collections::BTreeSet;
pub(super) const NAMES: [&str; 6] = [
    crate::shortcuts::DIR_NAME,
    crate::presets::LIBRARY_FILE,
    crate::export_tools::LIBRARY_FILE,
    crate::graphic_templates::USER_TEMPLATES_DIRECTORY,
    crate::project_tools::TEMPLATES_DIRECTORY,
    crate::settings::LABEL_PRESETS_DIR,
];
const LIBRARY_FILE_BYTES: u64 = 64 * 1024 * 1024;
const LIBRARY_BYTES: u64 = 256 * 1024 * 1024;
struct Scope {
    frame: Arc<DirectoryFrame>,
    names: Vec<String>,
    persistence: bool,
}
pub(super) struct Resources {
    roots: Vec<Arc<DirectoryFrame>>,
    directories: Vec<Scope>,
    nodes: Vec<NodeFrame>,
    seen: BTreeSet<PathBuf>,
    covered: BTreeSet<PathBuf>,
    pub(super) basis: Option<Value>,
    library_bytes: u64,
    limit: usize,
    pub(super) records: Value,
}
fn error(message: &str) -> crate::EngineError {
    bad(COMMAND, message)
}
fn entries(path: &Path, persistence: bool, check: &media_guard::Check) -> Result<Vec<String>> {
    let mut values = super::names(path, check)?;
    if persistence {
        // The original persistence inventory excludes only this regular,
        // derived Finder file; unknown and redirected members still refuse.
        if std::fs::symlink_metadata(path.join(".DS_Store")).is_ok_and(|m| m.is_file()) {
            values.retain(|v| v != ".DS_Store");
        }
    }
    Ok(values)
}
impl Resources {
    fn bounded(&self) -> Result<()> {
        if self.roots.len().saturating_add(self.directories.len()).saturating_add(self.nodes.len()) > self.limit {
            return Err(error("original complete resource node bound exceeded"));
        }
        Ok(())
    }
    fn node(&mut self, parent: &Arc<DirectoryFrame>, name: &str, library: bool, check: &media_guard::Check) -> Result<()> {
        let path = parent.record().path.join(name);
        if self.seen.contains(&path) {
            return Ok(());
        }
        if library {
            match std::fs::symlink_metadata(&path) {
                Ok(m) if m.is_file() && m.len() > 0 && m.len() <= LIBRARY_FILE_BYTES => {}
                Ok(m) if m.is_dir() => {}
                Ok(_) => return Err(error("authored resource must be a bounded regular file or directory")),
                Err(e) if e.kind() == io::ErrorKind::NotFound => {}
                Err(e) => return Err(io_error(e)),
            }
        }
        let io = || check.check().map_err(|e| io::Error::other(e.to_string()));
        let node = parent.node(name, &io).map_err(io_error)?;
        if let NodeState::File(v) = node.state() {
            if library {
                if v.bytes == 0 || v.bytes > LIBRARY_FILE_BYTES {
                    return Err(error("authored resource grew beyond its original bound"));
                }
                self.library_bytes = self
                    .library_bytes
                    .checked_add(v.bytes)
                    .filter(|n| *n <= LIBRARY_BYTES)
                    .ok_or_else(|| error("original authored resource byte bound exceeded"))?;
            }
        }
        self.seen.insert(path);
        self.nodes.push(node);
        self.bounded()
    }
    fn library(&mut self, root: &Arc<DirectoryFrame>, name: &str, check: &media_guard::Check) -> Result<()> {
        self.node(root, name, true, check)?;
        let path = root.record().path.join(name);
        self.covered.insert(path.clone());
        let Some(node) = self.nodes.iter().find(|v| v.path() == path) else {
            return Err(error("retained resource frame missing"));
        };
        if let NodeState::File(v) = node.state() {
            if v.identity.device != root.record().anchor.identity.device {
                return Err(error("authored resource crosses its original volume"));
            }
        }
        if !matches!(node.state(), NodeState::Directory(_)) {
            return Ok(());
        }
        let io = || check.check().map_err(|e| io::Error::other(e.to_string()));
        let device = root.record().anchor.identity.device;
        let mut pending = vec![(DirectoryFrame::capture(&path, &io).map_err(io_error)?, 0usize)];
        while let Some((frame, depth)) = pending.pop() {
            check.check()?;
            if depth > 16 || frame.record().anchor.identity.device != device {
                return Err(error("authored resource depth or original volume changed"));
            }
            let names = entries(&frame.record().path, false, check)?;
            for name in &names {
                self.node(&frame, name, true, check)?;
                let path = frame.record().path.join(name);
                let node = self.nodes.iter().find(|v| v.path() == path).ok_or_else(|| error("retained child frame missing"))?;
                if matches!(node.state(), NodeState::Directory(_)) {
                    pending.push((DirectoryFrame::capture(&path, &io).map_err(io_error)?, depth.saturating_add(1)));
                } else if let NodeState::File(v) = node.state() {
                    if v.identity.device != device {
                        return Err(error("authored resource crosses its original volume"));
                    }
                }
            }
            self.directories.push(Scope { frame, names, persistence: false });
            self.bounded()?;
        }
        Ok(())
    }
    pub(super) fn capture(s: &Session, check: &media_guard::Check, limit: usize) -> Result<Self> {
        let basis = s.agent_bridge.workspace_update.as_ref().and_then(|h| h.recovery_basis.clone());
        let persistence = basis.as_ref().map(super::super::super::project_recovery::persistence_dependencies).transpose()?;
        let mut paths = BTreeSet::new();
        for p in [s.shortcuts.recovery_directory(), s.presets.recovery_directory(), s.export_presets.dir(), s.prefs_path.as_deref().and_then(Path::parent)] {
            if let Some(p) = p {
                paths.insert(p.to_path_buf());
            }
        }
        if let Some(p) = persistence.as_ref().and_then(|v| v["dataDirectory"].as_str()) {
            paths.insert(PathBuf::from(p));
        }
        if paths.len() > 16 {
            return Err(error("original explicit working-root bound exceeded"));
        }
        let mut result = Self {
            roots: Vec::new(),
            directories: Vec::new(),
            nodes: Vec::new(),
            seen: BTreeSet::new(),
            covered: BTreeSet::new(),
            basis,
            library_bytes: 0,
            limit,
            records: Value::Null,
        };
        let io = || check.check().map_err(|e| io::Error::other(e.to_string()));
        for path in paths {
            let root = DirectoryFrame::capture(&path, &io).map_err(io_error)?;
            for name in NAMES {
                result.library(&root, name, check)?;
            }
            result.roots.push(root);
            result.bounded()?;
        }
        if let Some(p) = &persistence {
            for row in p["directories"].as_array().ok_or_else(|| error("original persistence directories missing"))? {
                let path = Path::new(row.as_str().ok_or_else(|| error("literal original persistence directory required"))?);
                let frame = DirectoryFrame::capture(path, &io).map_err(io_error)?;
                if !frame.exists() {
                    return Err(error("original persistence directory disappeared"));
                }
                result.directories.push(Scope { frame, names: entries(path, true, check)?, persistence: true });
                result.covered.insert(path.to_owned());
                result.bounded()?;
            }
            for row in p["files"].as_array().ok_or_else(|| error("original persistence files missing"))? {
                let path = Path::new(row["path"].as_str().ok_or_else(|| error("literal original persistence file required"))?);
                let parent = DirectoryFrame::capture(path.parent().ok_or_else(|| error("original persistence parent missing"))?, &io).map_err(io_error)?;
                result.node(
                    &parent,
                    path.file_name().and_then(|v| v.to_str()).ok_or_else(|| error("literal original persistence name required"))?,
                    false,
                    check,
                )?;
                let node = result.nodes.iter().find(|n| n.path() == path).ok_or_else(|| error("original persistence frame missing"))?;
                if !matches!(node.state(),NodeState::File(v)if Some(v.sha256.as_str())==row["sha256"].as_str()&&Some(v.bytes)==row["bytes"].as_u64()) {
                    return Err(error("original complete persistence file differs"));
                }
                result.covered.insert(path.to_owned());
            }
            for row in p["absent"].as_array().ok_or_else(|| error("original persistence absences missing"))? {
                let path = Path::new(row.as_str().ok_or_else(|| error("literal original absence required"))?);
                let parent = DirectoryFrame::capture(path.parent().ok_or_else(|| error("original absence parent missing"))?, &io).map_err(io_error)?;
                result.node(&parent, path.file_name().and_then(|v| v.to_str()).ok_or_else(|| error("literal absence name required"))?, false, check)?;
                if !result.nodes.iter().any(|n| n.path() == path && matches!(n.state(), NodeState::Absent)) {
                    return Err(error("original missing persistence member appeared"));
                }
                result.covered.insert(path.to_owned());
            }
        }
        result.records = json!({"kind":"native-original-resource-frames","version":1,
            "roots":result.roots.iter().map(|r|r.record()).collect::<Vec<_>>(),
            "directories":result.directories.iter().map(|d|json!({"frame":d.frame.record(),"names":d.names,"persistence":d.persistence})).collect::<Vec<_>>(),
            "nodes":result.nodes.iter().map(|n|(n.path(),n.state())).collect::<BTreeMap<_,_>>(),
            "persistence":persistence});
        result.current(s, check)?;
        Ok(result)
    }
    pub(super) fn archive_nodes(&self) -> impl Iterator<Item = &NodeFrame> {
        self.nodes.iter()
    }
    pub(super) fn archive_directories(&self) -> impl Iterator<Item = &Arc<DirectoryFrame>> {
        self.roots.iter().chain(self.directories.iter().map(|d| &d.frame))
    }
    pub(super) fn current(&self, s: &Session, check: &media_guard::Check) -> Result<()> {
        if s.agent_bridge.workspace_update.as_ref().and_then(|h| h.recovery_basis.as_ref()) != self.basis.as_ref() {
            return Err(error("original native recovery basis changed"));
        }
        let io = || check.check().map_err(|e| io::Error::other(e.to_string()));
        for root in &self.roots {
            root.current(&io).map_err(io_error)?;
        }
        for d in &self.directories {
            d.frame.current(&io).map_err(io_error)?;
            if entries(&d.frame.record().path, d.persistence, check)? != d.names {
                return Err(error("original authored or persistence namespace changed"));
            }
        }
        for node in &self.nodes {
            node.current(&io).map_err(io_error)?;
            node.fence(&io).map_err(io_error)?;
        }
        check.check()
    }
    pub(super) fn covers(&self, path: &Path) -> bool {
        self.covered.iter().any(|p| path == p || path.starts_with(p))
    }
}
