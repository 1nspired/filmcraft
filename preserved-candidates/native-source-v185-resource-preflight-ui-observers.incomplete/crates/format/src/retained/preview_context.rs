//! Complete read-only observations. These frames own real directory handles,
//! not directory creation, file overwrite, native or storage reservation grants.
use super::{Directory, DirectoryPin, MAX_BYTES, Observation, absolute, error};
use rustix::fs::{StatVfsMountFlags, fstatvfs};
use serde::{Deserialize, Serialize};
use std::{
    ffi::OsStr,
    fs, io,
    path::{Path, PathBuf},
    sync::Arc,
};
const MAX_DEPTH: usize = 64;
pub struct DirectoryFrame {
    path: PathBuf,
    anchor: Directory,
    missing: Vec<PathBuf>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DirectoryRecord {
    pub path: PathBuf,
    pub anchor: DirectoryPin,
    pub missing: Vec<PathBuf>,
}
impl DirectoryFrame {
    pub(super) fn anchor(&self) -> &Directory {
        &self.anchor
    }
    pub fn capture(path: &Path, check: &impl Fn() -> io::Result<()>) -> io::Result<Arc<Self>> {
        check()?;
        if !absolute(path) {
            return Err(error("explicit canonical preview directory context required"));
        }
        let mut cursor = path;
        let mut missing = Vec::new();
        loop {
            check()?;
            match fs::symlink_metadata(cursor) {
                Ok(meta) => {
                    if !meta.is_dir() {
                        return Err(error("preview directory is redirected or non-directory"));
                    }
                    break;
                }
                Err(e) if e.kind() == io::ErrorKind::NotFound => {
                    if missing.len() >= MAX_DEPTH {
                        return Err(error("complete missing directory chain exceeds original depth bound"));
                    }
                    missing.push(cursor.to_owned());
                    cursor = cursor.parent().ok_or_else(|| error("no known preview directory ancestor"))?;
                }
                Err(e) => return Err(e),
            }
        }
        let anchor = Directory::open(cursor, false)?;
        missing.reverse();
        let frame = Arc::new(Self { path: path.into(), anchor, missing });
        frame.current(check)?;
        Ok(frame)
    }
    pub fn record(&self) -> DirectoryRecord {
        DirectoryRecord { path: self.path.clone(), anchor: self.anchor.pin.clone(), missing: self.missing.clone() }
    }
    pub fn exists(&self) -> bool {
        self.missing.is_empty()
    }
    pub fn current(&self, check: &impl Fn() -> io::Result<()>) -> io::Result<()> {
        check()?;
        self.anchor.current()?;
        // No path traversal or caller callback separates the two anchor checks
        // for an already-existing parent. Keep both around every missing chain.
        if self.missing.is_empty() {
            return Ok(());
        }
        for p in &self.missing {
            match fs::symlink_metadata(p) {
                Err(e) if e.kind() == io::ErrorKind::NotFound => {}
                Err(e) => return Err(e),
                Ok(_) => return Err(error("a missing preview parent appeared; no implicit adoption")),
            }
        }
        self.anchor.current()
    }
    pub fn node(self: &Arc<Self>, name: &str, check: &impl Fn() -> io::Result<()>) -> io::Result<NodeFrame> {
        self.node_bounded(name, MAX_BYTES, check)
    }
    /// Complete observation bounded before reading, including expected empty locks.
    /// This remains read-only and cannot manufacture a native or publication owner.
    pub fn node_bounded(self: &Arc<Self>, name: &str, max: u64, check: &impl Fn() -> io::Result<()>) -> io::Result<NodeFrame> {
        if max > MAX_BYTES {
            return Err(error("preview observation exceeds the original byte bound"));
        }
        if name.contains('/')
            || name.is_empty()
            || name.len() > 512
            || name.chars().any(char::is_control)
            || Path::new(name).components().count() != 1
            || !matches!(Path::new(name).components().next(), Some(std::path::Component::Normal(_)))
        {
            return Err(error("one bounded literal preview file name required"));
        }
        let state = self.observe_bounded(name, max, check)?;
        Ok(NodeFrame { parent: self.clone(), name: name.into(), state })
    }
    fn observe_bounded(&self, name: &str, max: u64, check: &impl Fn() -> io::Result<()>) -> io::Result<NodeState> {
        self.current(check)?;
        if !self.exists() {
            return Ok(NodeState::Absent);
        }
        let path = self.path.join(name);
        let meta = match fs::symlink_metadata(&path) {
            Ok(v) => v,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(NodeState::Absent),
            Err(e) => return Err(e),
        };
        if meta.is_dir() {
            return Ok(NodeState::Directory(Directory::open(&path, false)?.pin));
        }
        if !meta.is_file() {
            return Err(error("preview file is redirected or nonregular; no implicit target traversal"));
        }
        self.anchor.observe(OsStr::new(name), max, check)?.map(NodeState::File).ok_or_else(|| error("preview file disappeared during complete observation"))
    }
    pub fn space(&self, check: &impl Fn() -> io::Result<()>) -> io::Result<Space> {
        self.current(check)?;
        let value = fstatvfs(&self.anchor.file)?;
        if value.f_frsize == 0 {
            return Err(error("preview storage returned no allocation unit"));
        }
        let free = u128::from(value.f_bavail).checked_mul(u128::from(value.f_frsize)).ok_or_else(|| error("preview free-space calculation overflow"))?;
        let free = u64::try_from(free).map_err(|_| error("preview free-space result exceeds its exact bound"))?;
        let result = Space {
            device: self.anchor.pin.identity.device.clone(),
            free,
            allocation_unit: value.f_frsize,
            read_only: value.f_flag.contains(StatVfsMountFlags::RDONLY),
        };
        self.current(check)?;
        Ok(result)
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "observation", rename_all = "snake_case", deny_unknown_fields)]
pub enum NodeState {
    Absent,
    File(Observation),
    Directory(DirectoryPin),
}
pub struct NodeFrame {
    parent: Arc<DirectoryFrame>,
    name: String,
    state: NodeState,
}
impl NodeFrame {
    pub(super) fn directory_frame(&self) -> Arc<DirectoryFrame> {
        self.parent.clone()
    }
    pub(super) fn publication_context(&self) -> io::Result<(DirectoryPin, Option<Observation>)> {
        if !self.parent.exists() {
            return Err(error("original document parent is still missing; no implicit directory creation"));
        }
        let target = match &self.state {
            NodeState::Absent => None,
            NodeState::File(v) => Some(v.clone()),
            NodeState::Directory(_) => return Err(error("original document target is a directory; no file publication")),
        };
        Ok((self.parent.anchor.pin.clone(), target))
    }
    /// Complete bytes through the original pinned directory and file generation.
    /// This does not construct overwrite, Root, startup or publication authority.
    pub fn read_private_bytes(&self, max: u64, check: &impl Fn() -> io::Result<()>) -> io::Result<Vec<u8>> {
        check()?;
        let NodeState::File(expected) = &self.state else {
            return Err(error("private control must be a complete existing regular file"));
        };
        if expected.identity.uid != rustix::process::geteuid().as_raw()
            || expected.identity.mode & 0o077 != 0
            || expected.bytes == 0
            || expected.bytes > max
            || max > MAX_BYTES
        {
            return Err(error("private control ownership, mode or complete size is invalid"));
        }
        self.parent.current(check)?;
        let (observed, bytes) =
            self.parent.anchor.read(OsStr::new(&self.name), max, true, check)?.ok_or_else(|| error("private control disappeared during complete read"))?;
        if &observed != expected {
            return Err(error("private control changed from its original file generation"));
        }
        self.fence(check)?;
        Ok(bytes)
    }
    pub fn state(&self) -> &NodeState {
        &self.state
    }
    /// Complete original-generation read of an owner-written data file.
    /// Ordinary journal files can be 0644 inside their private session. Keep
    /// the private-control reader above strict; this distinct data reader
    /// requires the current uid, one link, no other-writer bits and the same
    /// bounded directory-handle read, complete hash and final original fence.
    pub fn read_owned_bytes(&self, max: u64, check: &impl Fn() -> io::Result<()>) -> io::Result<Vec<u8>> {
        use std::os::unix::fs::MetadataExt;
        check()?;
        let NodeState::File(expected) = &self.state else {
            return Err(error("owned data must be a complete existing regular file"));
        };
        if expected.identity.uid != rustix::process::geteuid().as_raw()
            || expected.identity.mode & 0o022 != 0
            || expected.bytes == 0
            || expected.bytes > max
            || max > MAX_BYTES
            || fs::symlink_metadata(self.path())?.nlink() != 1
        {
            return Err(error("owned data uid, write mode, link count or complete byte bound is invalid"));
        }
        self.parent.current(check)?;
        let (observed, bytes) =
            self.parent.anchor.read(OsStr::new(&self.name), max, true, check)?.ok_or_else(|| error("owned data disappeared during complete read"))?;
        if &observed != expected || fs::symlink_metadata(self.path())?.nlink() != 1 {
            return Err(error("owned data changed from its original file generation"));
        }
        self.fence(check)?;
        Ok(bytes)
    }
    pub fn path(&self) -> PathBuf {
        self.parent.path.join(&self.name)
    }
    pub fn current(&self, check: &impl Fn() -> io::Result<()>) -> io::Result<()> {
        let max = match &self.state {
            NodeState::File(observed) => observed.bytes,
            _ => MAX_BYTES,
        };
        if self.parent.observe_bounded(&self.name, max, check)? != self.state {
            return Err(error("complete preview file context changed"));
        }
        Ok(())
    }
    /// Final metadata/name fence after a complete current byte observation. This
    /// is not a replacement for that full read or a stat-only authority cache.
    pub fn fence(&self, check: &impl Fn() -> io::Result<()>) -> io::Result<()> {
        // The pinned fence itself freshly checks the named/opened/canonical
        // parent. With an existing file, no effect or callback separates that
        // check from parent.current; retain the caller guard exactly once.
        if self.parent.exists() {
            if let NodeState::File(expected) = &self.state {
                check()?;
                return self.parent.anchor.fence(OsStr::new(&self.name), Some(expected));
            }
        }
        self.parent.current(check)?;
        if !self.parent.exists() {
            return if self.state == NodeState::Absent { Ok(()) } else { Err(error("missing preview parent changed")) };
        }
        match &self.state {
            NodeState::Absent => self.parent.anchor.fence(OsStr::new(&self.name), None),
            NodeState::File(expected) => self.parent.anchor.fence(OsStr::new(&self.name), Some(expected)),
            NodeState::Directory(pin) => {
                let now = Directory::open(&self.path(), false)?;
                if &now.pin != pin { Err(error("existing preview destination directory changed")) } else { Ok(()) }
            }
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Space {
    pub device: String,
    pub free: u64,
    pub allocation_unit: u64,
    pub read_only: bool,
}
pub fn allocation(bytes: u64, unit: u64) -> io::Result<u64> {
    if unit == 0 {
        return Err(error("zero allocation unit"));
    }
    if bytes == 0 {
        return Ok(0);
    }
    bytes.checked_add(unit - 1).and_then(|v| v.checked_div(unit)).and_then(|v| v.checked_mul(unit)).ok_or_else(|| error("preview allocation budget overflow"))
}
#[cfg(test)]
#[path = "preview_context_tests.rs"]
mod tests;

/// Read-only absence check. ESRCH is required; permission errors and a reused
/// live PID refuse. This does not reconstruct a process or a startup lease.
pub fn check_owner_exited(pid: u32) -> io::Result<()> {
    let raw = i32::try_from(pid).map_err(|_| error("bounded positive prior PID required"))?;
    let pid = rustix::process::Pid::from_raw(raw).ok_or_else(|| error("bounded positive prior PID required"))?;
    match rustix::process::test_kill_process(pid) {
        Err(rustix::io::Errno::SRCH) => Ok(()),
        Ok(()) => Err(error("prior native process remains live; no duplicate candidate may open")),
        Err(e) => Err(io::Error::from(e)),
    }
}

/// Probe only: never send a signal or assume a PID reconstructs a live lease.
pub fn check_owner_process(pid: u32) -> io::Result<()> {
    let raw = i32::try_from(pid).map_err(|_| error("bounded positive manager PID required"))?;
    let pid = rustix::process::Pid::from_raw(raw).ok_or_else(|| error("bounded positive manager PID required"))?;
    rustix::process::test_kill_process(pid).map_err(io::Error::from)
}
