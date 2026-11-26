//! Exclusive private companion files. Actual native/Root admission stays with
//! the caller. Deserializing intent/receipt metadata never creates this owner.
use super::preview_context::{DirectoryFrame, NodeFrame, NodeState, allocation};
use super::{Directory, DirectoryPin, Observation, error, hash, identity, moved_same, valid_id};
use rustix::fs::{AtFlags, Mode, OFlags, linkat, openat};
use serde::Serialize;
use std::{
    ffi::OsStr,
    fs::{self, File},
    io,
    path::PathBuf,
    sync::Arc,
};

pub const MAX_RECORD_BYTES: u64 = 80 * 1024 * 1024;
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Intent {
    pub kind: String,
    pub version: u32,
    pub operation: String,
    pub parent: DirectoryPin,
    pub destination: PathBuf,
    pub candidate: PathBuf,
    pub sha256: String,
    pub bytes: u64,
    pub reserve_bytes: u64,
    pub private: bool,
    pub overwrite: bool,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    Prepared,
    WritingCandidate,
    CandidateDurable,
    LinkUnconfirmed,
    Published,
}
#[derive(Clone, Debug, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Receipt {
    pub intent: Intent,
    pub phase: Phase,
    pub call_attempted: bool,
    pub candidate_created: bool,
    pub link_attempted: bool,
    pub directory_synced: bool,
    pub candidate: Option<Observation>,
    pub output: Option<Observation>,
    pub error: Option<String>,
    pub native_complete: bool,
    pub startup_authority: bool,
    pub workers_may_resume: bool,
}
#[derive(Clone, Debug, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Inspection {
    pub receipt: Receipt,
    pub candidate: Option<Observation>,
    pub output: Option<Observation>,
    pub matches_published: bool,
    pub current: bool,
    pub write_authority: bool,
}
/// Live directory descriptors and an owned candidate descriptor, never JSON authority.
pub struct Record {
    parent: Directory,
    context: Arc<DirectoryFrame>,
    name: String,
    candidate_name: String,
    candidate_file: Option<File>,
    candidate_identity: Option<super::Identity>,
    receipt: Receipt,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Point {
    BeforeCreate,
    AfterCreate,
    AfterBytes,
    BeforeLink,
    BeforeKernel,
    AfterLink,
    BeforeDirectorySync,
    AfterDirectorySync,
}
impl Record {
    /// Read-only preparation against an actual original absent-node frame.
    /// No directory, payload, destination, grant or dispatch is created here.
    pub fn prepare(frame: &NodeFrame, operation: &str, data: &[u8], reserve_bytes: u64, check: &impl Fn() -> io::Result<()>) -> io::Result<Self> {
        check()?;
        if !valid_id(operation) || data.is_empty() || data.len() as u64 > MAX_RECORD_BYTES || !matches!(frame.state(), NodeState::Absent) {
            return Err(error("exclusive private record requires original absence, bounded complete bytes and exact operation"));
        }
        frame.current(check)?;
        let (pin, target) = frame.publication_context()?;
        if target.is_some() {
            return Err(error("private companion cannot overwrite an observed file"));
        }
        let path = frame.path();
        let name = path.file_name().and_then(|n| n.to_str()).ok_or_else(|| error("private record filename must be UTF-8"))?.to_owned();
        let candidate_name = format!("{name}.{operation}.candidate");
        if candidate_name.len() > 255 {
            return Err(error("private candidate filename exceeds its literal component bound"));
        }
        let parent = Directory::open(&pin.path, false)?;
        if parent.pin != pin {
            return Err(error("original private record parent changed"));
        }
        if parent.observe(OsStr::new(&candidate_name), MAX_RECORD_BYTES, check)?.is_some() {
            return Err(error("private candidate already exists; inspect without adopting or retrying it"));
        }
        let intent = Intent {
            kind: "poema-exclusive-private-record".into(),
            version: 1,
            operation: operation.into(),
            parent: pin,
            destination: path,
            candidate: parent.pin.path.join(&candidate_name),
            sha256: hash(data, check)?,
            bytes: data.len() as u64,
            reserve_bytes,
            private: true,
            overwrite: false,
        };
        let owner = Self {
            parent,
            context: frame.directory_frame(),
            name,
            candidate_name,
            candidate_file: None,
            candidate_identity: None,
            receipt: Receipt {
                intent,
                phase: Phase::Prepared,
                call_attempted: false,
                candidate_created: false,
                link_attempted: false,
                directory_synced: false,
                candidate: None,
                output: None,
                error: None,
                native_complete: false,
                startup_authority: false,
                workers_may_resume: false,
            },
        };
        owner.current(check)?;
        frame.current(check)?;
        Ok(owner)
    }
    pub fn intent(&self) -> &Intent {
        &self.receipt.intent
    }
    pub fn receipt(&self) -> Receipt {
        self.receipt.clone()
    }
    fn current(&self, check: &impl Fn() -> io::Result<()>) -> io::Result<()> {
        check()?;
        self.parent.current()?;
        self.context.current(check)?;
        let written = if let Some(file) = &self.candidate_file {
            let opened = file.metadata()?;
            let named = fs::symlink_metadata(&self.receipt.intent.candidate)?;
            if !opened.is_file()
                || !named.is_file()
                || Some(identity(&opened)) != self.candidate_identity
                || identity(&opened) != identity(&named)
                || opened.mode() & 0o777 != 0o600
                || opened.len() > self.receipt.intent.bytes
                || opened.len() != named.len()
            {
                return Err(error("owned private candidate descriptor/name/owner/mode changed"));
            }
            if let Some(observed) = &self.receipt.candidate {
                match self.receipt.phase {
                    Phase::CandidateDurable => {
                        // A complete read already established these bytes. Recheck
                        // its exact generation at the last pre-link I/O boundary.
                        self.parent.fence(OsStr::new(&self.candidate_name), Some(observed))?;
                    }
                    Phase::LinkUnconfirmed => {
                        // Our hard link changes ctime, never bytes, mtime or the
                        // original file identity. Complete hashes follow below.
                        let now = super::stamp(&opened, observed.sha256.clone());
                        if !moved_same(observed, &now) {
                            return Err(error("private candidate changed across the exclusive link"));
                        }
                    }
                    _ => {}
                }
            }
            opened.len()
        } else {
            0
        };
        let space = self.context.space(check)?;
        let wanted = allocation(self.receipt.intent.bytes, space.allocation_unit)?;
        let written = allocation(written, space.allocation_unit)?;
        let metadata = allocation(65536, space.allocation_unit)?.checked_mul(8).ok_or_else(|| error("private metadata budget overflow"))?;
        let remaining = wanted
            .checked_sub(written)
            .and_then(|v| v.checked_add(metadata))
            .and_then(|v| v.checked_add(self.receipt.intent.reserve_bytes))
            .ok_or_else(|| error("private remaining-capacity calculation overflow"))?;
        if space.read_only || space.device != self.parent.pin.identity.device || space.free < remaining {
            return Err(error("private record storage changed, is read-only or would consume its reserve"));
        }
        self.parent.current()?;
        check()
    }
    /// Full current observations only, no retry, overwrite, completion or resume.
    pub fn inspect(&self, check: &impl Fn() -> io::Result<()>) -> io::Result<Inspection> {
        check()?;
        self.parent.current()?;
        let candidate = self.parent.observe(OsStr::new(&self.candidate_name), MAX_RECORD_BYTES, check)?;
        let output = self.parent.observe(OsStr::new(&self.name), MAX_RECORD_BYTES, check)?;
        self.parent.current()?;
        check()?;
        let matches_published = self.receipt.phase == Phase::Published && candidate == self.receipt.candidate && output == self.receipt.output;
        Ok(Inspection { receipt: self.receipt(), candidate, output, matches_published, current: false, write_authority: false })
    }
    /// At most one write attempt. Failure retains this exact owner and all files.
    /// `before_link` is the caller's original complete native/file revalidation;
    /// `check` remains the actual original I/O lifetime, not descriptive metadata.
    pub fn publish(&mut self, data: &[u8], check: &impl Fn() -> io::Result<()>, before_link: &impl Fn() -> io::Result<()>) -> io::Result<Receipt> {
        self.publish_with(data, check, before_link, &mut |_| Ok(()))
    }
    fn publish_with(
        &mut self,
        data: &[u8],
        check: &impl Fn() -> io::Result<()>,
        before_link: &impl Fn() -> io::Result<()>,
        hook: &mut impl FnMut(Point) -> io::Result<()>,
    ) -> io::Result<Receipt> {
        if self.receipt.call_attempted {
            return Err(error("original private record publication was already attempted; inspect without replay"));
        }
        self.receipt.call_attempted = true;
        let attempt = (|| {
            self.current(check)?;
            if data.len() as u64 != self.receipt.intent.bytes || hash(data, check)? != self.receipt.intent.sha256 {
                return Err(error("private record differs from its exact prepared bytes"));
            }
            if self.parent.observe(OsStr::new(&self.name), MAX_RECORD_BYTES, check)?.is_some()
                || self.parent.observe(OsStr::new(&self.candidate_name), MAX_RECORD_BYTES, check)?.is_some()
            {
                return Err(error("private record destination or candidate appeared; no overwrite or adoption"));
            }
            hook(Point::BeforeCreate)?;
            self.current(check)?;
            self.receipt.phase = Phase::WritingCandidate;
            let mut file = File::from(openat(
                &self.parent.file,
                self.candidate_name.as_str(),
                OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                Mode::RUSR | Mode::WUSR,
            )?);
            self.receipt.candidate_created = true;
            self.candidate_identity = Some(identity(&file.metadata()?));
            self.candidate_file = Some(file.try_clone()?);
            hook(Point::AfterCreate)?;
            crate::atomic::write_staged_bytes(&mut file, data, &|| self.current(check))?;
            self.receipt.phase = Phase::CandidateDurable;
            hook(Point::AfterBytes)?;
            self.current(check)?;
            let candidate = self
                .parent
                .observe(OsStr::new(&self.candidate_name), MAX_RECORD_BYTES, &|| self.current(check))?
                .ok_or_else(|| error("private candidate disappeared"))?;
            if candidate.sha256 != self.receipt.intent.sha256
                || candidate.bytes != self.receipt.intent.bytes
                || Some(candidate.identity.clone()) != self.candidate_identity
            {
                return Err(error("complete private candidate differs from its actual owned bytes"));
            }
            self.receipt.candidate = Some(candidate.clone());
            before_link()?;
            hook(Point::BeforeLink)?;
            self.current(check)?;
            self.parent.fence(OsStr::new(&self.candidate_name), Some(&candidate))?;
            if self.parent.observe(OsStr::new(&self.name), MAX_RECORD_BYTES, check)?.is_some() {
                return Err(error("private destination appeared before exclusive link"));
            }
            self.current(check)?;
            hook(Point::BeforeKernel)?;
            self.current(check)?;
            self.receipt.link_attempted = true;
            self.receipt.phase = Phase::LinkUnconfirmed;
            linkat(&self.parent.file, self.candidate_name.as_str(), &self.parent.file, self.name.as_str(), AtFlags::empty())?;
            hook(Point::AfterLink)?;
            self.current(check)?;
            hook(Point::BeforeDirectorySync)?;
            self.current(check)?;
            self.parent.file.sync_all()?;
            self.receipt.directory_synced = true;
            hook(Point::AfterDirectorySync)?;
            self.current(check)?;
            let output = self
                .parent
                .observe(OsStr::new(&self.name), MAX_RECORD_BYTES, &|| self.current(check))?
                .ok_or_else(|| error("private output disappeared after publication"))?;
            let now = self
                .parent
                .observe(OsStr::new(&self.candidate_name), MAX_RECORD_BYTES, &|| self.current(check))?
                .ok_or_else(|| error("private candidate disappeared after publication"))?;
            self.receipt.output = Some(output.clone());
            self.receipt.candidate = Some(now.clone());
            if !moved_same(&candidate, &output) || !moved_same(&candidate, &now) {
                return Err(error("private bytes changed after link; inspect retained files"));
            }
            self.parent.fence(OsStr::new(&self.name), Some(&output))?;
            self.parent.fence(OsStr::new(&self.candidate_name), Some(&now))?;
            self.current(check)?;
            self.receipt.phase = Phase::Published;
            Ok(self.receipt())
        })();
        if let Err(e) = &attempt {
            self.receipt.error = Some(e.to_string());
        }
        attempt
    }
}
use std::os::unix::fs::MetadataExt;
#[cfg(test)]
mod tests;
