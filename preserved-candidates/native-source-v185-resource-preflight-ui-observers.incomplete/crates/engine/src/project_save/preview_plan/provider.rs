//! Physical preview effects for the one original Save/PreviewStore body.
//! File receipts and preview-only credits are not complete Save/custody/Root
//! authority; every native result remains owned by the pending operation.
use super::Plan;
pub(crate) mod documents;
use crate::{
    Services,
    project_save::effects::{DirectoryRole, DocumentRole, Event, FileEffects},
};
use filmcraft_format::retained::{
    directory_effect,
    preview_context::{DirectoryFrame, NodeState, allocation},
    relocation,
};
use relocation::copying;
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    io,
    path::{Path, PathBuf},
    sync::Arc,
};
fn error(m: impl Into<String>) -> io::Error {
    io::Error::other(m.into())
}
fn from_engine(e: crate::EngineError) -> io::Error {
    error(e.to_string())
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Kind {
    Directories,
    Rename,
    Copy,
    Retire,
    Document(crate::project_save::effects::FileRole),
    PreferencesDirectories,
}
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct Role {
    name: String,
    kind: Kind,
}
struct Ledger {
    remaining: BTreeMap<String, u64>,
    charges: BTreeMap<Role, BTreeMap<String, u64>>,
    used: BTreeSet<Role>,
}
fn add(a: u64, b: u64) -> io::Result<u64> {
    a.checked_add(b).ok_or_else(|| error("preview remaining-credit budget overflow"))
}
fn mul(a: u64, b: u64) -> io::Result<u64> {
    a.checked_mul(b).ok_or_else(|| error("preview remaining-credit budget overflow"))
}
impl Ledger {
    fn new(plan: &Plan) -> io::Result<Self> {
        let mut ledger = Self { remaining: plan.demands.iter().map(|v| (v.device.clone(), 0)).collect(), charges: BTreeMap::new(), used: BTreeSet::new() };
        if plan.may_move {
            let from = plan.source.as_ref().ok_or_else(|| error("preview source plan absent"))?;
            let to = plan.target.as_ref().ok_or_else(|| error("preview target plan absent"))?;
            let sv = &plan.volumes[plan.volume_for(from).map_err(from_engine)?];
            let dv = &plan.volumes[plan.volume_for(to).map_err(from_engine)?];
            ledger.charge(
                Role { name: String::new(), kind: Kind::Directories },
                &dv.spec.device,
                mul(u64::try_from(to.record().missing.len()).map_err(|_| error("preview directory depth overflow"))?, mul(8, dv.before.allocation_unit)?)?,
            )?;
            for entry in &plan.entries {
                let mut source = 0;
                let mut dest = 0;
                let mut source_meta = 0;
                if matches!(entry.target.state(), NodeState::Absent) {
                    if let NodeState::File(file) = entry.source.state() {
                        source = allocation(file.bytes, sv.before.allocation_unit)?;
                        dest = mul(2, allocation(file.bytes, dv.before.allocation_unit)?)?;
                        source_meta = mul(8, allocation(super::METADATA_BOUND, sv.before.allocation_unit)?)?;
                    }
                }
                ledger.charge(Role { name: entry.name.clone(), kind: Kind::Rename }, &sv.spec.device, add(source, source_meta)?)?;
                ledger.charge(
                    Role { name: entry.name.clone(), kind: Kind::Copy },
                    &dv.spec.device,
                    if dest == 0 && source_meta == 0 { 0 } else { add(dest, mul(8, allocation(super::METADATA_BOUND, dv.before.allocation_unit)?)?)? },
                )?;
                ledger.charge(Role { name: entry.name.clone(), kind: Kind::Retire }, &sv.spec.device, source_meta)?;
            }
        }
        for demand in &plan.demands {
            if ledger.remaining.get(&demand.device).copied().unwrap_or(0) != demand.required_bytes {
                return Err(error("preview role credits do not equal complete declared demand"));
            }
        }
        Ok(ledger)
    }
    fn charge(&mut self, role: Role, device: &str, bytes: u64) -> io::Result<()> {
        let by_device = self.charges.entry(role).or_default();
        let value = by_device.entry(device.into()).or_default();
        *value = add(*value, bytes)?;
        let total = self.remaining.entry(device.into()).or_default();
        *total = add(*total, bytes)?;
        Ok(())
    }
    fn enter(&mut self, role: Role) -> io::Result<()> {
        if !self.charges.contains_key(&role) {
            return Err(error("unplanned Save credit role; no assignment"));
        }
        if !self.used.insert(role.clone()) {
            return Err(error("preview physical role already assigned; no callback replay"));
        }
        if let Some(charge) = self.charges.get(&role) {
            for (device, bytes) in charge {
                let remaining = self.remaining.get_mut(device).ok_or_else(|| error("preview device credit missing"))?;
                *remaining = remaining.checked_sub(*bytes).ok_or_else(|| error("preview credit underflow"))?;
            }
        }
        Ok(())
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Phase {
    Fresh,
    Skipped,
    RenameFailed,
    Copied,
    Renamed,
    Retired,
    Refused,
}
pub(crate) struct FileReport {
    pub name: String,
    pub phase: Phase,
    pub rename: Option<relocation::Publication>,
    pub copy: Option<copying::Publication>,
    pub retirement: Option<copying::retirement::Publication>,
    pub errors: Vec<String>,
}
pub(crate) struct Report {
    pub files: Vec<FileReport>,
    pub directories: Option<directory_effect::Publication>,
    pub boundary_errors: Vec<String>,
    pub remaining: BTreeMap<String, u64>,
    pub initial_demand: BTreeMap<String, u64>,
    pub document_admission: Option<documents::Documents>,
    _plan: Plan,
}
pub(in crate::project_save) struct Provider<'a> {
    plan: Plan,
    operation: String,
    source: Option<Arc<DirectoryFrame>>,
    target: Option<Arc<DirectoryFrame>>,
    rows: Vec<FileReport>,
    lookup: BTreeMap<String, usize>,
    ledger: Ledger,
    initial_demand: BTreeMap<String, u64>,
    documents: Option<documents::Documents>,
    directories: Option<directory_effect::Publication>,
    directory_called: bool,
    boundary_errors: Vec<String>,
    delegate: &'a mut dyn FileEffects,
    check: &'a dyn Fn() -> io::Result<()>,
}
fn guard(plan: &Plan, ledger: &Ledger, check: &dyn Fn() -> io::Result<()>) -> io::Result<()> {
    guard_remaining(plan, &ledger.remaining, check)
}
fn guard_remaining(plan: &Plan, remaining: &BTreeMap<String, u64>, check: &dyn Fn() -> io::Result<()>) -> io::Result<()> {
    check()?;
    plan.owner.workers_current().map_err(from_engine)?;
    if std::env::current_dir()? != plan.cwd || std::fs::canonicalize(&plan.cwd)? != plan.cwd {
        return Err(error("actual native CWD changed during preview effects"));
    }
    for volume in &plan.volumes {
        let now = volume.current(&|| check()).map_err(from_engine)?;
        let required = remaining.get(&now.device).copied().unwrap_or(0);
        let minimum = plan.demands.iter().find(|v| v.device == now.device).map(|v| v.minimum_free).ok_or_else(|| error("preview demand device disappeared"))?;
        if now.free < add(minimum, required)? {
            return Err(error("working reserve plus unassigned preview writes no longer fits; no fallback"));
        }
    }
    Ok(())
}
impl<'a> Provider<'a> {
    pub(in crate::project_save) fn new(
        plan: Plan,
        operation: &str,
        delegate: &'a mut dyn FileEffects,
        check: &'a dyn Fn() -> io::Result<()>,
    ) -> io::Result<Self> {
        if !(32..=128).contains(&operation.len()) || !operation.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-') {
            return Err(error("exact native preview operation id required"));
        }
        // Existing native recording is bounded. Refuse before publication rather
        // than overflow it after a partial catalog; scalable journaling remains open.
        if plan.entries.len().checked_mul(3).and_then(|v| v.checked_add(16)).is_none_or(|v| v > 65536) {
            return Err(error("complete preview calls exceed current native record bound; no partial dispatch"));
        }
        let mut identities = BTreeSet::new();
        for entry in &plan.entries {
            if let NodeState::File(file) = entry.source.state() {
                if !identities.insert((file.identity.device.clone(), file.identity.inode.clone())) {
                    return Err(error("aliased preview source entries require full causal namespace qualification; no file published"));
                }
            }
        }
        let ledger = Ledger::new(&plan)?;
        guard(&plan, &ledger, check)?;
        let rows = plan
            .entries
            .iter()
            .map(|e| FileReport { name: e.name.clone(), phase: Phase::Fresh, rename: None, copy: None, retirement: None, errors: Vec::new() })
            .collect();
        let lookup = plan.entries.iter().enumerate().map(|(i, e)| (e.name.clone(), i)).collect();
        let source = plan.source.clone();
        let target = plan.target.as_ref().filter(|v| v.exists()).cloned();
        Ok(Self {
            plan,
            operation: operation.into(),
            source,
            target,
            rows,
            lookup,
            initial_demand: ledger.remaining.clone(),
            ledger,
            documents: None,
            directories: None,
            directory_called: false,
            boundary_errors: Vec::new(),
            delegate,
            check,
        })
    }
    pub(in crate::project_save) fn into_report(self) -> Report {
        Report {
            files: self.rows,
            directories: self.directories,
            boundary_errors: self.boundary_errors,
            remaining: self.ledger.remaining,
            initial_demand: self.initial_demand,
            document_admission: self.documents,
            _plan: self.plan,
        }
    }
    fn guard(&self) -> io::Result<()> {
        guard(&self.plan, &self.ledger, self.check)
    }
    fn tag(&self, index: usize, kind: Kind) -> String {
        format!("{:x}", Sha256::digest(format!("{}:preview:{:?}:{}", self.operation, kind, self.rows[index].name).as_bytes()))
    }
    fn locate(&self, from: Option<&Path>, to: Option<&Path>) -> io::Result<usize> {
        let path = to.or(from).ok_or_else(|| error("preview effect path missing"))?;
        let name = path.file_name().and_then(|v| v.to_str()).ok_or_else(|| error("preview name cannot be represented"))?;
        let index = *self.lookup.get(name).ok_or_else(|| error("preview effect is outside exact original catalog"))?;
        let row = &self.plan.entries[index];
        if from.is_some_and(|p| self.plan.source_literal.as_ref().is_none_or(|root| root.join(&row.name) != p))
            || to.is_some_and(|p| self.plan.target_literal.as_ref().is_none_or(|root| root.join(&row.name) != p))
        {
            return Err(error("preview callback changed exact original source/destination path"));
        }
        Ok(index)
    }
    fn source_now(&self, index: usize) -> io::Result<NodeState> {
        self.source.as_ref().ok_or_else(|| error("preview source context missing"))?.node(&self.rows[index].name, &|| self.guard()).map(|v| v.state().clone())
    }
    fn target_now(&self, index: usize) -> io::Result<NodeState> {
        self.target
            .as_ref()
            .ok_or_else(|| error("preview destination directories are unconfirmed"))?
            .node(&self.rows[index].name, &|| self.guard())
            .map(|v| v.state().clone())
    }
    fn before(&self, index: usize) -> io::Result<()> {
        self.guard()?;
        if self.source_now(index)? != *self.plan.entries[index].source.state() || self.target_now(index)? != *self.plan.entries[index].target.state() {
            return Err(error("complete preview source/target changed from original preflight"));
        }
        Ok(())
    }
    fn refuse(&mut self, index: usize, e: &io::Error) {
        self.rows[index].phase = Phase::Refused;
        self.rows[index].errors.push(e.to_string());
    }
    fn rename_effect(&mut self, index: usize) -> io::Result<()> {
        if self.rows[index].phase != Phase::Fresh {
            return Err(error("preview rename callback already consumed"));
        }
        self.before(index)?;
        self.ledger.enter(Role { name: self.rows[index].name.clone(), kind: Kind::Rename })?;
        let NodeState::File(expected) = self.plan.entries[index].source.state() else {
            self.rows[index].phase = Phase::RenameFailed;
            return Err(io::Error::new(io::ErrorKind::NotFound, "original indexed preview source is absent"));
        };
        let from = self.plan.entries[index].source.path();
        let to = self.target.as_ref().ok_or_else(|| error("preview target missing"))?.node(&self.rows[index].name, &|| self.guard())?.path();
        let tag = self.tag(index, Kind::Rename);
        let staged = match relocation::stage(&from, &to, &tag, &|| self.guard()) {
            Ok(v) => v,
            Err(e) => {
                if self.before(index).is_ok() {
                    self.rows[index].phase = Phase::RenameFailed;
                } else {
                    self.refuse(index, &e);
                }
                self.rows[index].errors.push(e.to_string());
                return Err(e);
            }
        };
        if &staged.intent().source_before != expected {
            return Err(error("preview rename source changed during staging"));
        }
        let publication = staged.publish(&|| self.guard());
        let qualified = publication.receipt_durable
            && publication.receipt.outcome == relocation::Outcome::Moved
            && relocation::inspect(&publication.directory, &|| self.guard())
                .is_ok_and(|v| v.current == relocation::CurrentState::Moved && v.recorded_receipt.as_ref() == Some(&publication.receipt));
        let known_failed = publication.receipt_durable && publication.receipt.outcome == relocation::Outcome::NotMoved && self.before(index).is_ok();
        self.rows[index].rename = Some(publication);
        self.rows[index].phase = if qualified {
            Phase::Renamed
        } else if known_failed {
            Phase::RenameFailed
        } else {
            Phase::Refused
        };
        if qualified {
            Ok(())
        } else {
            Err(error("preview rename refused or unconfirmed; original fallback is admitted only for the same live known-unmoved file"))
        }
    }
    fn copy_effect(&mut self, index: usize) -> io::Result<u64> {
        if self.rows[index].phase != Phase::RenameFailed {
            return Err(error("preview copy requires its actual known-unmoved rename outcome"));
        }
        self.before(index)?;
        self.ledger.enter(Role { name: self.rows[index].name.clone(), kind: Kind::Copy })?;
        let NodeState::File(expected) = self.plan.entries[index].source.state() else {
            return Err(io::Error::new(io::ErrorKind::NotFound, "original indexed preview source is absent"));
        };
        let from = self.plan.entries[index].source.path();
        let to = self.target.as_ref().ok_or_else(|| error("preview target missing"))?.node(&self.rows[index].name, &|| self.guard())?.path();
        let tag = self.tag(index, Kind::Copy);
        let staged = copying::stage(&from, &to, &tag, &|| self.guard())?;
        if &staged.intent().source_before != expected {
            return Err(error("preview copy source changed during staging"));
        }
        let publication = staged.publish(&|| self.guard());
        let qualified = publication.verify_current(&|| self.guard()).is_ok();
        let bytes = expected.bytes;
        self.rows[index].copy = Some(publication);
        self.rows[index].phase = if qualified { Phase::Copied } else { Phase::Refused };
        if qualified { Ok(bytes) } else { Err(error("preview copy outcome is unconfirmed; no source retirement allowed")) }
    }
    fn retire_effect(&mut self, index: usize) -> io::Result<()> {
        if self.rows[index].phase != Phase::Copied {
            return Err(error("source removal requires its actual live qualified preview copy"));
        }
        self.guard()?;
        self.ledger.enter(Role { name: self.rows[index].name.clone(), kind: Kind::Retire })?;
        let plan = &self.plan;
        let ledger = &self.ledger;
        let check = self.check;
        let callback = || guard(plan, ledger, check);
        let copy = self.rows[index].copy.as_mut().ok_or_else(|| error("live preview copy receipt missing"))?;
        let staged = copy.stage_retirement(&callback)?;
        let publication = staged.publish(&callback);
        let qualified = publication.receipt_durable
            && publication.receipt.outcome == copying::retirement::Outcome::Retired
            && copying::retirement::inspect(&publication.directory, &callback)
                .is_ok_and(|v| v.current == copying::retirement::CurrentState::Retired && v.recorded_receipt.as_ref() == Some(&publication.receipt));
        self.rows[index].retirement = Some(publication);
        self.rows[index].phase = if qualified { Phase::Retired } else { Phase::Refused };
        if qualified { Ok(()) } else { Err(error("preview source retirement is unconfirmed; retained data and native workers remain owned")) }
    }
}
impl FileEffects for Provider<'_> {
    fn current_save_files(&self) -> io::Result<()> {
        self.guard()
    }
    fn stage_document(
        &mut self,
        role: crate::project_save::effects::FileRole,
        path: &Path,
        operation: &str,
        bytes: &[u8],
        check: &dyn Fn() -> io::Result<()>,
    ) -> io::Result<filmcraft_format::retained::StagedWrite> {
        if self.documents.is_some() {
            self.stage_complete_document(role, path, operation, bytes, check)
        } else {
            self.guard()?;
            self.delegate.stage_document(role, path, operation, bytes, check)
        }
    }
    fn document(&mut self, s: &dyn Services, r: DocumentRole, p: &str, b: &[u8]) -> io::Result<()> {
        if self.documents.is_some() {
            return Err(error("complete documents require the original retained receiver; no direct file fallback"));
        }
        self.guard()?;
        self.delegate.document(s, r, p, b)
    }
    fn preferences(&mut self, p: &Path, b: &[u8]) -> io::Result<()> {
        if self.documents.is_some() {
            return Err(error("complete preferences require the original retained receiver; no direct file fallback"));
        }
        self.guard()?;
        self.delegate.preferences(p, b)
    }
    fn directories(&mut self, role: DirectoryRole, path: &Path) -> io::Result<()> {
        self.guard()?;
        if role != DirectoryRole::Previews {
            if role == DirectoryRole::Preferences && self.documents.is_some() {
                return self.preferences_directories(path);
            }
            return self.delegate.directories(role, path);
        }
        if !self.plan.may_move || self.plan.target_literal.as_deref() != Some(path) || self.directory_called {
            return Err(error("unexpected or repeated original preview directory callback"));
        }
        self.directory_called = true;
        self.ledger.enter(Role { name: String::new(), kind: Kind::Directories })?;
        let frame = self.plan.target.as_ref().ok_or_else(|| error("preview target frame absent"))?.clone();
        let staged = directory_effect::stage(frame.clone(), &|| self.guard())?;
        let publication = staged.publish(&|| self.guard());
        let complete = publication.complete;
        let failure = publication.error.clone();
        self.directories = Some(publication);
        if !complete {
            return Err(error(failure.unwrap_or_else(|| "preview directory preparation unconfirmed".into())));
        }
        let target = DirectoryFrame::capture(&frame.record().path, &|| self.guard())?;
        let expected = self.directories.as_ref().and_then(|v| v.observed.last()).cloned().unwrap_or(frame.record().anchor);
        if target.record().anchor != expected {
            return Err(error("preview directory changed after its observed creation; no adoption"));
        }
        self.target = Some(target);
        Ok(())
    }
    fn preview_exists(&mut self, path: &Path) -> bool {
        let result = (|| {
            self.guard()?;
            let index = self.locate(None, Some(path))?;
            let actual = self.target_now(index)?;
            if actual != *self.plan.entries[index].target.state() {
                return Err(error("preview target changed before original existence decision"));
            }
            if actual != NodeState::Absent {
                self.rows[index].phase = Phase::Skipped;
                Ok(true)
            } else {
                Ok(false)
            }
        })();
        match result {
            Ok(v) => v,
            Err(e) => {
                self.boundary_errors.push(e.to_string());
                self.delegate.event(Event::BoundaryRefused { phase: "preview_exists", path: path.into(), error: e.to_string() });
                true
            }
        }
    }
    fn preview_rename(&mut self, from: &Path, to: &Path) -> io::Result<()> {
        let index = self.locate(Some(from), Some(to))?;
        let result = self.rename_effect(index);
        if let Err(e) = &result {
            if self.rows[index].phase != Phase::RenameFailed {
                self.rows[index].phase = Phase::Refused;
            }
            self.rows[index].errors.push(e.to_string());
        }
        result
    }
    fn preview_copy(&mut self, from: &Path, to: &Path) -> io::Result<u64> {
        let index = self.locate(Some(from), Some(to))?;
        let result = self.copy_effect(index);
        if let Err(e) = &result {
            self.rows[index].phase = Phase::Refused;
            self.rows[index].errors.push(e.to_string());
        }
        result
    }
    fn preview_remove(&mut self, from: &Path) -> io::Result<()> {
        let index = self.locate(Some(from), None)?;
        let result = self.retire_effect(index);
        if let Err(e) = &result {
            self.rows[index].phase = Phase::Refused;
            self.rows[index].errors.push(e.to_string());
        }
        result
    }
    fn event(&mut self, event: Event) {
        self.delegate.event(event)
    }
}
#[cfg(test)]
#[path = "provider_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "provider/complete_tests.rs"]
mod complete_tests;

mod observation;

pub(in crate::project_save) mod admission;

mod receipt_verification;
