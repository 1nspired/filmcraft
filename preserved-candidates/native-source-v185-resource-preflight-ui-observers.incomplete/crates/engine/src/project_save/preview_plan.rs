//! Complete native preview preflight under the genuine pending Save owner.
//! Read-only observations and conservative capacity, never file/Root/overwrite,
//! creation, reservation, completion or restart authority.
use super::operation::{self, Key, PreviewOwner};
use crate::{EngineError, Result, Session};
use filmcraft_format::retained::preview_context::{DirectoryFrame, NodeFrame, NodeState, Space, allocation};
use serde::Serialize;
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    io,
    path::{Path, PathBuf},
    sync::Arc,
};
const METADATA_BOUND: u64 = 65536;
#[derive(Clone)]
pub(crate) struct VolumeSpec {
    pub root: PathBuf,
    pub device: String,
    pub minimum_free: u64,
}
struct Volume {
    spec: VolumeSpec,
    frame: Arc<DirectoryFrame>,
    before: Space,
}
struct Entry {
    name: String,
    source: NodeFrame,
    target: NodeFrame,
}
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Demand {
    pub device: String,
    pub minimum_free: u64,
    pub observed_free: u64,
    pub required_bytes: u64,
    pub source_archive_bytes: u64,
    pub destination_payload_bytes: u64,
    pub metadata_bytes: u64,
    pub read_only: bool,
}
pub(crate) struct Plan {
    owner: PreviewOwner,
    cwd: PathBuf,
    source: Option<Arc<DirectoryFrame>>,
    target: Option<Arc<DirectoryFrame>>,
    entries: Vec<Entry>,
    volumes: Vec<Volume>,
    demands: Vec<Demand>,
    names: Vec<String>,
    may_move: bool,
    source_literal: Option<PathBuf>,
    target_literal: Option<PathBuf>,
}
fn error(m: impl Into<String>) -> EngineError {
    EngineError::Other(m.into())
}
fn io_error(e: io::Error) -> EngineError {
    error(e.to_string())
}
fn physical(cwd: &Path, p: &Path) -> PathBuf {
    if p.is_absolute() { p.to_owned() } else { cwd.join(p) }
}
fn add(a: u64, b: u64) -> Result<u64> {
    a.checked_add(b).ok_or_else(|| error("complete preview capacity budget overflow"))
}
fn multiply(a: u64, b: u64) -> Result<u64> {
    a.checked_mul(b).ok_or_else(|| error("complete preview capacity budget overflow"))
}
impl Volume {
    fn capture(spec: &VolumeSpec, check: &impl Fn() -> io::Result<()>) -> Result<Self> {
        let frame = DirectoryFrame::capture(&spec.root, check).map_err(io_error)?;
        if !frame.exists() {
            return Err(error("preview working volume root must already exist; no implicit creation/adoption"));
        }
        let before = frame.space(check).map_err(io_error)?;
        if before.device != spec.device {
            return Err(error("preview working root device differs from explicit live storage context"));
        }
        Ok(Self { spec: spec.clone(), frame, before })
    }
    fn current(&self, check: &impl Fn() -> io::Result<()>) -> Result<Space> {
        let now = self.frame.space(check).map_err(io_error)?;
        if now.device != self.before.device || now.allocation_unit != self.before.allocation_unit || now.read_only != self.before.read_only {
            return Err(error("preview working volume identity/allocation/writable state changed; no fallback"));
        }
        Ok(now)
    }
}
impl Plan {
    pub(crate) fn capture(s: &Session, key: &Key, specs: &[VolumeSpec], check: &impl Fn() -> io::Result<()>) -> Result<Self> {
        check().map_err(io_error)?;
        let (owner, p, pause) = operation::preview_inputs(s, key)?;
        let cwd = std::env::current_dir().map_err(io_error)?;
        if std::fs::canonicalize(&cwd).map_err(io_error)? != cwd {
            return Err(error("actual native working directory must be canonical"));
        }
        let names = p.basis.previews.save_catalog(pause)?;
        let source_literal = p.input.preview_dir.clone();
        let target_literal = p.adopt.then(|| crate::project_tools::previews_dir_for(&p.project, &p.path, &p.input.prefs.auto_save.scratch_folders));
        let may_move = target_literal.as_ref().is_some_and(|to| {
            source_literal
                .as_ref()
                .is_some_and(|old| old != to && (old.starts_with(crate::temp_dir()) || p.input.preview_temp.as_ref().is_some_and(|r| old.starts_with(r))))
        });
        let source = if may_move {
            source_literal.as_ref().map(|path| DirectoryFrame::capture(&physical(&cwd, path), check).map_err(io_error)).transpose()?
        } else {
            None
        };
        let target = target_literal.as_ref().map(|path| DirectoryFrame::capture(&physical(&cwd, path), check).map_err(io_error)).transpose()?;
        let mut volumes = Vec::new();
        for spec in specs {
            check().map_err(io_error)?;
            if volumes.iter().any(|v: &Volume| v.spec.root == spec.root) {
                return Err(error("duplicate preview working root context"));
            }
            volumes.push(Volume::capture(spec, check)?);
        }
        let mut entries = Vec::with_capacity(if may_move { names.len() } else { 0 });
        if may_move {
            let from = source.as_ref().ok_or_else(|| error("original moving preview source missing"))?;
            let to = target.as_ref().ok_or_else(|| error("original moving preview target missing"))?;
            for name in &names {
                check().map_err(io_error)?;
                let source = from.node(name, check).map_err(io_error)?;
                if matches!(source.state(), NodeState::Directory(_)) {
                    return Err(error("indexed preview source is a directory; no unscoped tree move admitted"));
                }
                let target = to.node(name, check).map_err(io_error)?;
                entries.push(Entry { name: name.clone(), source, target });
            }
        }
        let mut plan = Self { owner, cwd, source, target, entries, volumes, demands: Vec::new(), names, may_move, source_literal, target_literal };
        plan.demands = plan.budget(check)?;
        plan.current(s, key, check)?;
        Ok(plan)
    }
    fn volume_for(&self, frame: &DirectoryFrame) -> Result<usize> {
        let record = frame.record();
        self.volumes
            .iter()
            .enumerate()
            .filter(|(_, v)| record.path.starts_with(&v.spec.root) && record.anchor.identity.device == v.spec.device)
            .max_by_key(|(_, v)| v.spec.root.components().count())
            .map(|(i, _)| i)
            .ok_or_else(|| error("preview source/destination has no explicit matching working root; no CWD or volume fallback"))
    }
    fn budget(&self, check: &impl Fn() -> io::Result<()>) -> Result<Vec<Demand>> {
        let mut by_device: BTreeMap<String, Demand> = BTreeMap::new();
        for v in &self.volumes {
            let now = v.current(check)?;
            let row = by_device.entry(now.device.clone()).or_insert(Demand {
                device: now.device.clone(),
                minimum_free: v.spec.minimum_free,
                observed_free: now.free,
                required_bytes: 0,
                source_archive_bytes: 0,
                destination_payload_bytes: 0,
                metadata_bytes: 0,
                read_only: now.read_only,
            });
            row.minimum_free = row.minimum_free.max(v.spec.minimum_free);
            row.observed_free = row.observed_free.min(now.free);
            if row.read_only != now.read_only {
                return Err(error("one working device reports inconsistent writable status"));
            }
        }
        if let Some(to) = self.target.as_ref().filter(|_| self.may_move) {
            let i = self.volume_for(to)?;
            let v = &self.volumes[i];
            let record = to.record();
            let directory_meta =
                multiply(u64::try_from(record.missing.len()).map_err(|_| error("directory depth conversion failed"))?, multiply(8, v.before.allocation_unit)?)?;
            let row = by_device.get_mut(&v.spec.device).ok_or_else(|| error("preview target volume budget missing"))?;
            row.metadata_bytes = add(row.metadata_bytes, directory_meta)?;
        }
        if self.may_move {
            let from = self.source.as_ref().ok_or_else(|| error("source budget context missing"))?;
            let to = self.target.as_ref().ok_or_else(|| error("target budget context missing"))?;
            let si = self.volume_for(from)?;
            let di = self.volume_for(to)?;
            let sv = &self.volumes[si];
            let dv = &self.volumes[di];
            for row in &self.entries {
                if !matches!(row.target.state(), NodeState::Absent) {
                    continue;
                }
                let NodeState::File(source) = row.source.state() else { continue };
                let source_data = allocation(source.bytes, sv.before.allocation_unit).map_err(io_error)?;
                let target_data = multiply(2, allocation(source.bytes, dv.before.allocation_unit).map_err(io_error)?)?;
                // Rename and retirement have distinct source-side records; copy has one
                // destination-side record. Eight maximum metadata blocks per operation
                // conservatively cover JSON, directory/inode and namespace overhead.
                let source_meta = multiply(16, allocation(METADATA_BOUND, sv.before.allocation_unit).map_err(io_error)?)?;
                let target_meta = multiply(8, allocation(METADATA_BOUND, dv.before.allocation_unit).map_err(io_error)?)?;
                {
                    let row = by_device.get_mut(&sv.spec.device).ok_or_else(|| error("source volume budget missing"))?;
                    row.source_archive_bytes = add(row.source_archive_bytes, source_data)?;
                    row.metadata_bytes = add(row.metadata_bytes, source_meta)?;
                }
                {
                    let row = by_device.get_mut(&dv.spec.device).ok_or_else(|| error("target volume budget missing"))?;
                    row.destination_payload_bytes = add(row.destination_payload_bytes, target_data)?;
                    row.metadata_bytes = add(row.metadata_bytes, target_meta)?;
                }
            }
        }
        for row in by_device.values_mut() {
            row.required_bytes = add(add(row.source_archive_bytes, row.destination_payload_bytes)?, row.metadata_bytes)?;
            if row.read_only && row.required_bytes > 0 {
                return Err(error(
                    "preview write capacity is unqualified on a read-only working device; original auxiliary failure policy still requires full provider integration",
                ));
            }
            let minimum = add(row.minimum_free, row.required_bytes)?;
            if row.observed_free < minimum {
                return Err(error("complete preview worst-case space would consume working reserve; nothing published"));
            }
        }
        Ok(by_device.into_values().collect())
    }
    pub(crate) fn current(&self, s: &Session, key: &Key, check: &impl Fn() -> io::Result<()>) -> Result<()> {
        self.current_with(s, key, check, &mut || Ok(()))
    }
    fn current_with(&self, s: &Session, key: &Key, check: &impl Fn() -> io::Result<()>, before_fence: &mut impl FnMut() -> io::Result<()>) -> Result<()> {
        check().map_err(io_error)?;
        self.owner.current(s, key)?;
        if std::env::current_dir().map_err(io_error)? != self.cwd || std::fs::canonicalize(&self.cwd).map_err(io_error)? != self.cwd {
            return Err(error("actual native working directory changed during preview preflight"));
        }
        if let Some(from) = &self.source {
            from.current(check).map_err(io_error)?;
        }
        if let Some(to) = &self.target {
            to.current(check).map_err(io_error)?;
        }
        for row in &self.entries {
            row.source.current(check).map_err(io_error)?;
            row.target.current(check).map_err(io_error)?;
        }
        let _ = self.budget(check)?;
        self.owner.current(s, key)?;
        // Earlier complete files are fenced AFTER later files, capacity and native
        // postflight. Metadata fences never substitute for the full reads above.
        before_fence().map_err(io_error)?;
        for row in &self.entries {
            row.source.fence(check).map_err(io_error)?;
            row.target.fence(check).map_err(io_error)?;
        }
        check().map_err(io_error)?;
        Ok(())
    }
    pub(crate) fn description(&self) -> Value {
        json!({"kind":"native_indexed_preview_migration_preflight","version":1,"nativeCwd":self.cwd,"source":self.source_literal,"destination":self.target_literal,"mayMoveTemporaryFiles":self.may_move,"catalogEntries":self.names.len(),"observedEntries":self.entries.len(),"targetMissingDirectories":self.target.as_ref().map(|v|v.record().missing),"demands":self.demands,"previewOnly":true,"mediaEvidence":false,"unindexedTreeCapture":false,"allDocumentBudgetsIncluded":false,"spaceReserved":false,"writeAuthority":false,"directoryCreationAuthority":false,"nativeComplete":false,"startupAuthority":false})
    }
}
#[cfg(test)]
#[path = "preview_plan_tests.rs"]
mod tests;

/// Preserving physical provider for the original callbacks, not another Save policy.
pub(crate) mod provider;
