//! Retained atomic file publication. This is NOT native scope, overwrite,
//! startup or Undo authority. A live owner stages once; inspection never replays.
//! Ordinary human atomic_write remains unconditional and shares byte staging.
use rustix::fs::{Mode, OFlags, RenameFlags, mkdirat, open, openat, renameat_with};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use sha2::{Digest, Sha256};
use std::os::unix::fs::MetadataExt;
use std::{
    ffi::OsStr,
    fs::{self, File},
    io::{self, Read},
    path::{Path, PathBuf},
};

mod readonly;

/// Caller-owned validation only, never publication, native or startup authority.
/// Every effect invokes current(). The final fence belongs only to the SAME
/// effect-free read; it cannot replace current() before a separate write.
pub trait SourceCheck {
    fn current(&self) -> io::Result<()>;
    fn fence_after_read(&self) -> io::Result<()> {
        self.current()
    }
}
impl<F: Fn() -> io::Result<()>> SourceCheck for F {
    fn current(&self) -> io::Result<()> {
        self()
    }
}
/// Ephemeral callback pair. It owns no directory, file, worker, or permission.
/// Ordinary closure callers retain complete validation on both sides.
pub struct ReadSource<'a, F, A> {
    current: &'a F,
    after_read: &'a A,
}
impl<'a, F, A> ReadSource<'a, F, A> {
    pub fn new(current: &'a F, after_read: &'a A) -> Self {
        Self { current, after_read }
    }
}
impl<F: Fn() -> io::Result<()>, A: Fn() -> io::Result<()>> SourceCheck for ReadSource<'_, F, A> {
    fn current(&self) -> io::Result<()> {
        (self.current)()
    }
    fn fence_after_read(&self) -> io::Result<()> {
        (self.after_read)()
    }
}

const MAX_BYTES: u64 = 4 * 1024_u64.pow(4);
const MAX_METADATA: u64 = 65536;
const PAYLOAD: &str = "payload.fcproj";
const SLOT: &str = "exchange-slot.fcproj";
const INTENT: &str = "intent.json";
const DISPATCH: &str = "dispatch.json";
const RECEIPT: &str = "receipt.json";
fn error(message: impl Into<String>) -> io::Error {
    io::Error::other(message.into())
}
fn valid_id(id: &str) -> bool {
    (32..=128).contains(&id.len()) && id.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'-')
}
fn absolute(path: &Path) -> bool {
    path.is_absolute()
        && path.to_str().is_some_and(|s| s.len() <= 4096 && !s.chars().any(char::is_control))
        && path.components().all(|c| !matches!(c, std::path::Component::ParentDir | std::path::Component::CurDir))
}
fn hash(data: &[u8], check: &impl Fn() -> io::Result<()>) -> io::Result<String> {
    let mut h = Sha256::new();
    for chunk in data.chunks(65536) {
        check()?;
        h.update(chunk);
    }
    check()?;
    Ok(format!("{:x}", h.finalize()))
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Identity {
    pub device: String,
    pub inode: String,
    pub mode: u32,
    pub uid: u32,
}
fn identity(meta: &fs::Metadata) -> Identity {
    Identity { device: meta.dev().to_string(), inode: meta.ino().to_string(), mode: meta.mode(), uid: meta.uid() }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Observation {
    pub identity: Identity,
    pub bytes: u64,
    pub modified: String,
    pub changed: String,
    pub sha256: String,
}
fn stamp(meta: &fs::Metadata, sha256: String) -> Observation {
    Observation {
        identity: identity(meta),
        bytes: meta.len(),
        modified: format!("{}:{}", meta.mtime(), meta.mtime_nsec()),
        changed: format!("{}:{}", meta.ctime(), meta.ctime_nsec()),
        sha256,
    }
}
// Rename itself changes ctime. Complete bytes, original inode, size, mtime,
// owner and mode still have to agree after the kernel moves a named file.
fn moved_same(a: &Observation, b: &Observation) -> bool {
    a.identity == b.identity && a.bytes == b.bytes && a.modified == b.modified && a.sha256 == b.sha256
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DirectoryPin {
    pub path: PathBuf,
    pub identity: Identity,
}
struct Directory {
    pin: DirectoryPin,
    file: File,
}
impl Directory {
    fn open(path: &Path, private: bool) -> io::Result<Self> {
        if !absolute(path) || fs::canonicalize(path)? != path {
            return Err(error("retained publication directory must be explicit and canonical"));
        }
        let file = File::from(open(path, OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC, Mode::empty())?);
        let meta = file.metadata()?;
        if !meta.is_dir() || private && (meta.mode() & 0o077) != 0 {
            return Err(error("retained operation directory must remain private"));
        }
        let value = Self { pin: DirectoryPin { path: path.to_owned(), identity: identity(&meta) }, file };
        value.current()?;
        Ok(value)
    }
    fn current(&self) -> io::Result<()> {
        let named = fs::symlink_metadata(&self.pin.path)?;
        let opened = self.file.metadata()?;
        if !named.is_dir()
            || !opened.is_dir()
            || identity(&named) != self.pin.identity
            || identity(&opened) != self.pin.identity
            || fs::canonicalize(&self.pin.path)? != self.pin.path
        {
            return Err(error("retained publication directory changed; no fallback"));
        }
        Ok(())
    }
    fn observe(&self, name: &OsStr, max: u64, check: &impl Fn() -> io::Result<()>) -> io::Result<Option<Observation>> {
        self.read(name, max, false, check).map(|v| v.map(|v| v.0))
    }
    fn read(&self, name: &OsStr, max: u64, collect: bool, check: &impl Fn() -> io::Result<()>) -> io::Result<Option<(Observation, Vec<u8>)>> {
        check()?;
        self.current()?;
        let path = self.pin.path.join(name);
        let before = match fs::symlink_metadata(&path) {
            Ok(v) => v,
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                self.current()?;
                return Ok(None);
            }
            Err(e) => return Err(e),
        };
        if !before.is_file() || before.len() > max {
            return Err(error("retained input is redirected, nonregular or over its complete byte bound"));
        }
        let mut file = File::from(openat(&self.file, name, OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC, Mode::empty())?);
        let first = stamp(&before, String::new());
        if !file.metadata()?.is_file() || stamp(&file.metadata()?, String::new()) != first {
            return Err(error("retained input changed before reading"));
        }
        let mut digest = Sha256::new();
        let mut buffer = [0_u8; 65536];
        let mut bytes = 0_u64;
        let mut data = Vec::new();
        loop {
            check()?;
            let size = file.read(&mut buffer)?;
            if size == 0 {
                break;
            }
            bytes = bytes.checked_add(size as u64).ok_or_else(|| error("retained byte count overflow"))?;
            if bytes > max {
                return Err(error("retained input grew beyond its complete byte bound"));
            }
            let chunk = buffer.get(..size).ok_or_else(|| error("invalid retained read length"))?;
            digest.update(chunk);
            if collect {
                data.extend_from_slice(chunk);
            }
        }
        check()?;
        let after = fs::symlink_metadata(&path)?;
        if !after.is_file() || stamp(&after, String::new()) != first || stamp(&file.metadata()?, String::new()) != first || bytes != before.len() {
            return Err(error("retained input changed during its complete read"));
        }
        self.current()?;
        Ok(Some((stamp(&before, format!("{:x}", digest.finalize())), data)))
    }
    fn fence(&self, name: &OsStr, expected: Option<&Observation>) -> io::Result<()> {
        self.current()?;
        let now = match fs::symlink_metadata(self.pin.path.join(name)) {
            Ok(v) => Some(v),
            Err(e) if e.kind() == io::ErrorKind::NotFound => None,
            Err(e) => return Err(e),
        };
        match (expected, now) {
            (None, None) => Ok(()),
            (Some(expected), Some(meta)) if meta.is_file() && stamp(&meta, expected.sha256.clone()) == *expected => Ok(()),
            _ => Err(error("retained file changed after its complete observation")),
        }
    }
    fn write(&self, name: &str, bytes: &[u8], private: bool, check: &impl Fn() -> io::Result<()>) -> io::Result<()> {
        self.write_file(name, bytes, private, check).map(drop)
    }
    fn write_file(&self, name: &str, bytes: &[u8], private: bool, check: &impl Fn() -> io::Result<()>) -> io::Result<File> {
        check()?;
        self.current()?;
        let mode = Mode::from_bits_truncate(if private { 0o600 } else { 0o666 });
        let mut file = File::from(openat(&self.file, name, OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC, mode)?);
        crate::atomic::write_staged_bytes(&mut file, bytes, check)?;
        self.file.sync_all()?;
        self.current()?;
        Ok(file)
    }
    fn json_owned<T: Serialize>(&self, name: &str, value: &T, check: &impl Fn() -> io::Result<()>) -> io::Result<(Vec<u8>, Observation)> {
        let bytes = serde_json::to_vec(value).map_err(error_json)?;
        if bytes.is_empty() || bytes.len() as u64 > MAX_METADATA {
            return Err(error("retained metadata exceeds its bound"));
        }
        let file = self.write_file(name, &bytes, true, check)?;
        // Bounded owned metadata bytes; this pure digest adds no source/IO
        // callback. The original writer and inspector keep all their guards.
        let observed = stamp(&file.metadata()?, format!("{:x}", Sha256::digest(&bytes)));
        self.fence(OsStr::new(name), Some(&observed))?;
        if stamp(&file.metadata()?, observed.sha256.clone()) != observed {
            return Err(error("original written control generation changed before confirmation"));
        }
        Ok((bytes, observed))
    }
    fn json<T: Serialize>(&self, name: &str, value: &T, check: &impl Fn() -> io::Result<()>) -> io::Result<Vec<u8>> {
        let data = serde_json::to_vec(value).map_err(error_json)?;
        if data.is_empty() || data.len() as u64 > MAX_METADATA {
            return Err(error("retained metadata exceeds its bound"));
        }
        self.write(name, &data, true, check)?;
        Ok(data)
    }
    fn read_json<T: DeserializeOwned>(&self, name: &str, check: &impl Fn() -> io::Result<()>) -> io::Result<Option<(T, String)>> {
        let Some((observation, bytes)) = self.read(OsStr::new(name), MAX_METADATA, true, check)? else { return Ok(None) };
        if observation.identity.mode & 0o077 != 0 {
            return Err(error("retained metadata lost its private mode"));
        }
        let value = serde_json::from_slice(&bytes).map_err(error_json)?;
        Ok(Some((value, observation.sha256)))
    }
}
fn error_json(e: serde_json::Error) -> io::Error {
    error(format!("invalid retained metadata: {e}"))
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Intent {
    pub kind: String,
    pub version: u32,
    pub operation: String,
    pub destination: PathBuf,
    pub parent: DirectoryPin,
    pub directory: DirectoryPin,
    pub target_before: Option<Observation>,
    pub payload_archive: Observation,
    pub slot_before: Observation,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    Published,
    ConflictRetained,
    NotPublished,
    Uncertain,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Receipt {
    pub kind: String,
    pub version: u32,
    pub operation: String,
    pub intent_sha256: String,
    pub outcome: Outcome,
    pub attempted: bool,
    pub directories_synced: bool,
    pub output: Option<Observation>,
    pub displaced: Option<Observation>,
    pub error: Option<String>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Dispatch {
    kind: String,
    version: u32,
    operation: String,
    intent_sha256: String,
}
#[derive(Clone, Debug, Serialize)]
pub struct ControlObservations {
    pub intent: Observation,
    pub dispatch: Option<Observation>,
    pub receipt: Option<Observation>,
}
// Read-only storage custody, never a publication or startup capability.
struct ArchiveContext {
    frame: std::sync::Arc<preview_context::DirectoryFrame>,
    device: String,
    minimum_free: u64,
}
impl ArchiveContext {
    fn capture(
        frame: &std::sync::Arc<preview_context::DirectoryFrame>,
        destination_parent: &DirectoryPin,
        bytes: u64,
        reserve: u64,
        check: &impl SourceCheck,
    ) -> io::Result<std::sync::Arc<Self>> {
        let current = || check.current();
        let after_read = || check.fence_after_read();
        readonly::observe_with_fence(&current, &after_read, |pass| {
            let inspect = || pass.check();
            let check = &inspect;
            frame.current(check)?;
            let record = frame.record();
            if !frame.exists()
                || record.path != record.anchor.path
                || record.path.starts_with(&destination_parent.path)
                || destination_parent.path.starts_with(&record.path)
                || record.anchor.identity.uid != rustix::process::geteuid().as_raw()
                || record.anchor.identity.mode & 0o077 != 0
                || record.anchor.identity.device != destination_parent.identity.device
            {
                return Err(error("separate existing private same-device archive frame required; no fallback"));
            }
            let space = frame.space(check)?;
            let payload = preview_context::allocation(bytes, space.allocation_unit)?;
            let metadata =
                preview_context::allocation(65536, space.allocation_unit)?.checked_mul(8).ok_or_else(|| error("private archive metadata budget overflow"))?;
            // A conservative two-payload charge stays live through publication.
            // Partial files/receipts are retained if available space later drops.
            let minimum_free = payload
                .checked_mul(2)
                .and_then(|v| v.checked_add(metadata))
                .and_then(|v| v.checked_add(reserve))
                .ok_or_else(|| error("private archive reserve calculation overflow"))?;
            let value = std::sync::Arc::new(Self { frame: frame.clone(), device: destination_parent.identity.device.clone(), minimum_free });
            value.current(check)?;
            Ok(value)
        })
    }

    fn current(&self, source: &impl SourceCheck) -> io::Result<()> {
        let current = || source.current();
        let after_read = || source.fence_after_read();
        readonly::observe_with_fence(&current, &after_read, |pass| self.storage(&|| pass.check()))
    }
    fn fence_after_read(&self, source: &impl SourceCheck) -> io::Result<()> {
        let after_read = || source.fence_after_read();
        readonly::observe(&after_read, |pass| self.storage(&|| pass.check()))
    }
    fn storage(&self, check: &impl Fn() -> io::Result<()>) -> io::Result<()> {
        self.frame.current(check)?;
        let space = self.frame.space(check)?;
        if space.read_only || space.device != self.device || space.free < self.minimum_free {
            return Err(error("private archive storage changed or would consume its conservative working reserve"));
        }
        self.frame.current(check)
    }
}
fn archive_guard<'a>(archive: Option<&'a ArchiveContext>, source: &'a impl SourceCheck) -> impl Fn() -> io::Result<()> + 'a {
    move || match archive {
        Some(archive) => archive.current(source),
        None => source.current(),
    }
}
fn archive_fence<'a>(archive: Option<&'a ArchiveContext>, source: &'a impl SourceCheck) -> impl Fn() -> io::Result<()> + 'a {
    move || match archive {
        Some(archive) => archive.fence_after_read(source),
        None => source.fence_after_read(),
    }
}

pub struct Publication {
    pub receipt: Receipt,
    pub receipt_durable: bool,
    pub directory: PathBuf,
    original_directory: DirectoryPin,
    controls: ControlObservations,
    archive_context: Option<std::sync::Arc<ArchiveContext>>,
}
impl Publication {
    /// Exact generations from the original open file writers, never re-pinned
    /// from a matching value after the write returned.
    pub fn control_observations(&self) -> &ControlObservations {
        &self.controls
    }
    pub fn controls_current(&self, check: &impl SourceCheck) -> io::Result<()> {
        let guarded = archive_guard(self.archive_context.as_deref(), check);
        let after_read = archive_fence(self.archive_context.as_deref(), check);
        readonly::observe_with_fence(&guarded, &after_read, |pass| {
            let inspect = || pass.check();
            let check = &inspect;
            check()?;
            let root = Directory::open(&self.directory, true)?;
            if root.pin != self.original_directory {
                return Err(error("original publication control directory changed"));
            }
            for (name, expected) in
                [(INTENT, Some(&self.controls.intent)), (DISPATCH, self.controls.dispatch.as_ref()), (RECEIPT, self.controls.receipt.as_ref())]
            {
                if root.observe(OsStr::new(name), MAX_METADATA, check)?.as_ref() != expected {
                    return Err(error("original publication control generation changed; no re-pinning"));
                }
                root.fence(OsStr::new(name), expected)?;
            }
            root.current()?;
            check()
        })
    }
}
/// Live, single-use staging owner. Not Clone or Deserialize. Reading archived
/// intent/receipt bytes can NEVER reconstruct this dispatch capability.
pub struct StagedWrite {
    archive_context: Option<std::sync::Arc<ArchiveContext>>,
    parent: Directory,
    root: Directory,
    intent: Intent,
    intent_sha256: String,
    intent_control: Observation,
    dispatch_control: Option<Observation>,
    attempted: bool,
    kernel_succeeded: bool,
    directories_synced: bool,
}
#[derive(Clone, Copy)]
enum Point {
    BeforeKernel,
    AfterKernel,
    BeforeReceipt,
}
impl StagedWrite {
    pub fn intent(&self) -> &Intent {
        &self.intent
    }
    pub fn directory(&self) -> &Path {
        &self.root.pin.path
    }
    fn current(&self, check: &impl Fn() -> io::Result<()>) -> io::Result<()> {
        readonly::observe(check, |pass| {
            let inspect = || pass.check();
            let check = &inspect;
            check()?;
            self.parent.current()?;
            self.root.current()?;
            let (intent, digest) = self.root.read_json::<Intent>(INTENT, check)?.ok_or_else(|| error("retained intent disappeared"))?;
            if intent != self.intent || digest != self.intent_sha256 {
                return Err(error("retained immutable intent changed"));
            }
            self.root.fence(OsStr::new(INTENT), Some(&self.intent_control))?;
            let archive = self.root.observe(OsStr::new(PAYLOAD), MAX_BYTES, check)?.ok_or_else(|| error("retained payload disappeared"))?;
            if archive != self.intent.payload_archive {
                return Err(error("retained immutable payload changed"));
            }
            let (_, after) = self.root.read_json::<Intent>(INTENT, check)?.ok_or_else(|| error("retained intent disappeared"))?;
            if after != self.intent_sha256 {
                return Err(error("retained intent changed during payload verification"));
            }
            self.root.fence(OsStr::new(PAYLOAD), Some(&archive))?;
            Ok(())
        })
    }
    pub fn publish(self, check: &impl SourceCheck) -> Publication {
        self.publish_with(check, &mut |_| Ok(()))
    }
    fn publish_with(mut self, check: &impl SourceCheck, hook: &mut impl FnMut(Point) -> io::Result<()>) -> Publication {
        let archive = self.archive_context.clone();
        let guarded = archive_guard(archive.as_deref(), check);
        let after_read = archive_fence(archive.as_deref(), check);
        let check = &guarded;
        let mut output = None;
        let mut displaced = None;
        let mut outcome = Outcome::NotPublished;
        let attempt: io::Result<()> = (|| {
            let name = self.intent.destination.file_name().ok_or_else(|| error("missing retained destination leaf"))?;
            readonly::observe_with_fence(check, &after_read, |pass| {
                let inspect = || pass.check();
                self.current(&inspect)?;
                let slot = self.root.observe(OsStr::new(SLOT), MAX_BYTES, &inspect)?;
                if slot.as_ref() != Some(&self.intent.slot_before) || self.parent.observe(name, MAX_BYTES, &inspect)? != self.intent.target_before {
                    return Err(error("retained destination or staging changed before publication"));
                }
                Ok(())
            })?;
            let dispatch = Dispatch {
                kind: "poema-retained-file-dispatch".into(),
                version: 1,
                operation: self.intent.operation.clone(),
                intent_sha256: self.intent_sha256.clone(),
            };
            let (_, observed_dispatch) = self.root.json_owned(DISPATCH, &dispatch, check)?;
            self.dispatch_control = Some(observed_dispatch);
            // Fresh fence after durable dispatch metadata; no authority comes from it.
            readonly::observe_with_fence(check, &after_read, |pass| {
                let inspect = || pass.check();
                self.current(&inspect)?;
                if self.root.observe(OsStr::new(SLOT), MAX_BYTES, &inspect)?.as_ref() != Some(&self.intent.slot_before)
                    || self.parent.observe(name, MAX_BYTES, &inspect)? != self.intent.target_before
                {
                    return Err(error("retained destination changed at final publication fence"));
                }
                Ok(())
            })?;
            hook(Point::BeforeKernel)?;
            check()?;
            self.attempted = true;
            outcome = Outcome::Uncertain;
            let flags = if self.intent.target_before.is_some() { RenameFlags::EXCHANGE } else { RenameFlags::NOREPLACE };
            if let Err(e) = renameat_with(&self.root.file, SLOT, &self.parent.file, name, flags) {
                if e == rustix::io::Errno::EXIST || e == rustix::io::Errno::NOENT {
                    outcome = Outcome::NotPublished;
                }
                return Err(e.into());
            }
            self.kernel_succeeded = true;
            hook(Point::AfterKernel)?;
            self.parent.file.sync_all()?;
            self.root.file.sync_all()?;
            self.directories_synced = true;
            readonly::observe_with_fence(check, &after_read, |pass| {
                let inspect = || pass.check();
                self.current(&inspect)?;
                output = self.parent.observe(name, MAX_BYTES, &inspect)?;
                displaced = self.root.observe(OsStr::new(SLOT), MAX_BYTES, &inspect)?;
                if output.as_ref().is_none_or(|v| !moved_same(v, &self.intent.slot_before)) {
                    return Err(error("a later output replaced or changed the published file; it was retained"));
                }
                outcome = match (&self.intent.target_before, &displaced) {
                    (None, None) => Outcome::Published,
                    (Some(old), Some(now)) if moved_same(old, now) => Outcome::Published,
                    _ => Outcome::ConflictRetained,
                };
                Ok(())
            })?;
            hook(Point::BeforeReceipt)?;
            Ok(())
        })();
        let failure = attempt.err().map(|e| e.to_string());
        if failure.is_some() && self.kernel_succeeded {
            outcome = Outcome::Uncertain;
        }
        let mut receipt = Receipt {
            kind: "poema-retained-file-receipt".into(),
            version: 1,
            operation: self.intent.operation.clone(),
            intent_sha256: self.intent_sha256.clone(),
            outcome,
            attempted: self.attempted,
            directories_synced: self.directories_synced,
            output,
            displaced,
            error: failure,
        };
        // Receipt cleanup does not reuse an aborted model/native signal. It only
        // writes a bounded new record in the still-owned directory, never the target.
        let receipt_check = || match archive.as_deref() {
            Some(context) => context.current(&|| Ok(())),
            None => Ok(()),
        };
        let receipt_control = self.root.json_owned(RECEIPT, &receipt, &receipt_check).ok().map(|(_, observed)| observed);
        let durable = receipt_control.is_some();
        if !durable {
            receipt.outcome = Outcome::Uncertain;
            receipt.error = Some("receipt durability is unconfirmed; inspect the retained operation without repeating publication".into());
        }
        Publication {
            receipt,
            receipt_durable: durable,
            directory: self.root.pin.path.clone(),
            original_directory: self.root.pin.clone(),
            controls: ControlObservations { intent: self.intent_control, dispatch: self.dispatch_control, receipt: receipt_control },
            archive_context: self.archive_context,
        }
    }
}
/// Prepare one exact native file payload. Nothing is published to destination.
/// A separate archive inode survives later in-place writes to the output.
pub fn stage(destination: &Path, operation: &str, data: &[u8], check: &impl Fn() -> io::Result<()>) -> io::Result<StagedWrite> {
    stage_expected(destination, operation, data, None, check)
}
/// Stage against the original live frame, not a newly adopted target. Frames
/// are observations, never overwrite/Root grants; the caller still owns those.
pub fn stage_from_frame(frame: &preview_context::NodeFrame, operation: &str, data: &[u8], check: &impl Fn() -> io::Result<()>) -> io::Result<StagedWrite> {
    frame.current(check)?;
    let (parent, target) = frame.publication_context()?;
    stage_expected(&frame.path(), operation, data, Some((&parent, target.as_ref())), check)
}
/// Stage an absent private checkpoint file through the ORIGINAL retained writer.
/// Archive bytes/control records live in a separate existing private context.
/// Both actual parent frames and same-device storage remain fenced through I/O;
/// a description cannot reconstruct this single-use owner. All native/session,
/// complete-union, durable-intent and application admission remain caller-owned.
pub fn stage_private_from_frames(
    destination: &preview_context::NodeFrame,
    archive: &std::sync::Arc<preview_context::DirectoryFrame>,
    operation: &str,
    data: &[u8],
    reserve_bytes: u64,
    check: &impl SourceCheck,
) -> io::Result<StagedWrite> {
    if data.is_empty() || data.len() as u64 > MAX_BYTES || !matches!(destination.state(), preview_context::NodeState::Absent) {
        return Err(error("original absent target and bounded complete private bytes required"));
    }
    let current = || check.current();
    let after_read = || check.fence_after_read();
    let (parent, target, context) = readonly::observe_with_fence(&current, &after_read, |pass| {
        let inspect = || pass.check();
        destination.current(&inspect)?;
        let (parent, target) = destination.publication_context()?;
        let context = ArchiveContext::capture(archive, &parent, data.len() as u64, reserve_bytes, &inspect)?;
        Ok((parent, target, context))
    })?;
    stage_expected_in(&destination.path(), operation, data, Some((&parent, target.as_ref())), Some(context), check)
}

fn stage_expected(
    destination: &Path,
    operation: &str,
    data: &[u8],
    expected_context: Option<(&DirectoryPin, Option<&Observation>)>,
    check: &impl Fn() -> io::Result<()>,
) -> io::Result<StagedWrite> {
    stage_expected_in(destination, operation, data, expected_context, None, check)
}
fn stage_expected_in(
    destination: &Path,
    operation: &str,
    data: &[u8],
    expected_context: Option<(&DirectoryPin, Option<&Observation>)>,
    archive_context: Option<std::sync::Arc<ArchiveContext>>,
    check: &impl SourceCheck,
) -> io::Result<StagedWrite> {
    let guard_archive = archive_context.clone();
    let guarded = archive_guard(guard_archive.as_deref(), check);
    let after_read = archive_fence(guard_archive.as_deref(), check);
    let check = &guarded;
    check()?;
    if !absolute(destination) || !valid_id(operation) || data.len() as u64 > MAX_BYTES {
        return Err(error("explicit normalized destination, operation and bounded complete payload required"));
    }
    let parent_path = destination.parent().ok_or_else(|| error("destination parent missing"))?;
    let name = destination.file_name().ok_or_else(|| error("destination leaf missing"))?;
    let parent = Directory::open(parent_path, false)?;
    let (target_before, expected) = readonly::observe_with_fence(check, &after_read, |pass| {
        let inspect = || pass.check();
        let target_before = parent.observe(name, MAX_BYTES, &inspect)?;
        if expected_context.is_some_and(|(expected_parent, expected_target)| &parent.pin != expected_parent || target_before.as_ref() != expected_target) {
            return Err(error("original observed document parent/target changed before staging; no operation directory created"));
        }
        let expected = hash(data, &inspect)?;
        Ok((target_before, expected))
    })?;
    let directory_name = format!(".poema-save-{operation}.incomplete");
    let archive_parent = archive_context.as_ref().map(|context| Directory::open(&context.frame.record().path, true)).transpose()?;
    if let (Some(directory), Some(context)) = (&archive_parent, &archive_context) {
        if directory.pin != context.frame.record().anchor {
            return Err(error("original private archive parent changed before staging"));
        }
    }
    let staging_parent = archive_parent.as_ref().unwrap_or(&parent);
    check()?;
    staging_parent.current()?;
    mkdirat(&staging_parent.file, directory_name.as_str(), Mode::RWXU)?;
    staging_parent.file.sync_all()?;
    let path = staging_parent.pin.path.join(directory_name);
    let root = Directory::open(&path, true)?;
    let private = archive_context.is_some();
    root.write(PAYLOAD, data, private, check)?;
    root.write(SLOT, data, private, check)?;
    let (payload_archive, slot_before) = readonly::observe_with_fence(check, &after_read, |pass| {
        let inspect = || pass.check();
        let payload_archive = root.observe(OsStr::new(PAYLOAD), MAX_BYTES, &inspect)?.ok_or_else(|| error("retained payload missing after stage"))?;
        let slot_before = root.observe(OsStr::new(SLOT), MAX_BYTES, &inspect)?.ok_or_else(|| error("retained exchange slot missing after stage"))?;
        if payload_archive.sha256 != expected || slot_before.sha256 != expected || payload_archive.identity == slot_before.identity {
            return Err(error("complete retained payload/independent slot verification failed"));
        }
        Ok((payload_archive, slot_before))
    })?;
    let intent = Intent {
        kind: "poema-retained-file-intent".into(),
        version: 1,
        operation: operation.into(),
        destination: destination.into(),
        parent: parent.pin.clone(),
        directory: root.pin.clone(),
        target_before,
        payload_archive,
        slot_before,
    };
    let (raw, intent_control) = root.json_owned(INTENT, &intent, check)?;
    let value = readonly::observe_with_fence(check, &after_read, |pass| {
        let inspect = || pass.check();
        let intent_sha256 = hash(&raw, &inspect)?;
        let value = StagedWrite {
            archive_context,
            parent,
            root,
            intent,
            intent_sha256,
            intent_control,
            dispatch_control: None,
            attempted: false,
            kernel_succeeded: false,
            directories_synced: false,
        };
        value.current(&inspect)?;
        Ok(value)
    })?;
    Ok(value)
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CurrentState {
    Prepared,
    Published,
    ConflictRetained,
    NotPublished,
    Unresolved,
    Changed,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Inspection {
    pub intent: Intent,
    pub dispatch: bool,
    pub recorded_receipt: Option<Receipt>,
    pub current: CurrentState,
    pub output: Option<Observation>,
    pub slot: Option<Observation>,
    pub write_authority: bool,
    pub startup_authority: bool,
    pub native_undo: bool,
}
/// Fresh physical observations and historical receipt only. No restart,
/// dispatch, reverse exchange, cleanup, native adoption or copied grant.
pub fn inspect(directory: &Path, check: &impl Fn() -> io::Result<()>) -> io::Result<Inspection> {
    inspect_with(directory, check, &mut || Ok(()))
}
/// Inspect the exact immutable intent held by an original live caller.
/// Expected metadata narrows reads; it never grants dispatch or startup rights.
pub fn inspect_expected(directory: &Path, expected: &Intent, check: &impl Fn() -> io::Result<()>) -> io::Result<Inspection> {
    inspect_expected_with(directory, Some(expected), check, &mut || Ok(()))
}
fn inspect_with(directory: &Path, check: &impl Fn() -> io::Result<()>, before_fence: &mut impl FnMut() -> io::Result<()>) -> io::Result<Inspection> {
    inspect_expected_with(directory, None, check, before_fence)
}
fn inspect_expected_with(
    directory: &Path,
    expected: Option<&Intent>,
    check: &impl Fn() -> io::Result<()>,
    before_fence: &mut impl FnMut() -> io::Result<()>,
) -> io::Result<Inspection> {
    inspect_expected_with_seals(directory, expected, &[], check, before_fence)
}
pub fn inspect_expected_after_directory_seals(
    directory: &Path,
    expected: &Intent,
    seals: &[directory_effect::SealedDirectory],
    check: &impl Fn() -> io::Result<()>,
) -> io::Result<Inspection> {
    inspect_expected_with_seals(directory, Some(expected), seals, check, &mut || Ok(()))
}
fn inspect_expected_with_seals(
    directory: &Path,
    expected: Option<&Intent>,
    seals: &[directory_effect::SealedDirectory],
    check: &impl Fn() -> io::Result<()>,
    before_fence: &mut impl FnMut() -> io::Result<()>,
) -> io::Result<Inspection> {
    inspect_expected_with_controls(directory, expected, seals, None, check, before_fence)
}
/// The original complete inspector, additionally fenced to the control
/// generations retained by its actual consuming writer. No duplicate payload
/// pass, renewed budget, value-only re-pinning or deserialized owner.
pub fn inspect_publication_after_directory_seals(
    publication: &Publication,
    expected: &Intent,
    seals: &[directory_effect::SealedDirectory],
    check: &impl SourceCheck,
) -> io::Result<Inspection> {
    let guarded = archive_guard(publication.archive_context.as_deref(), check);
    let after_read = archive_fence(publication.archive_context.as_deref(), check);
    readonly::observe_with_fence(&guarded, &after_read, |pass| {
        let inspect = || pass.check();
        inspect_expected_with_controls(&publication.directory, Some(expected), seals, Some(publication), &inspect, &mut || Ok(()))
    })
}
/// Final generations of a SAME complete, successfully published observation.
/// The caller must first run the complete original inspector in this read scope.
/// This reads no new payload baseline and grants no write or startup authority.
/// Original writers and standalone inspections retain their complete byte reads.
pub fn fence_publication_after_read(
    publication: &Publication,
    expected: &Intent,
    seals: &[directory_effect::SealedDirectory],
    check: &impl Fn() -> io::Result<()>,
) -> io::Result<()> {
    readonly::observe(check, |pass| {
        let inspect = || pass.check();
        let root = Directory::open(&publication.directory, true)?;
        control_fences(&root, publication)?;
        if expected.kind != "poema-retained-file-intent"
            || expected.version != 1
            || !valid_id(&expected.operation)
            || !absolute(&expected.destination)
            || expected.directory != root.pin
            || expected.parent.path != expected.destination.parent().ok_or_else(|| error("invalid intent parent"))?
            || publication.directory.file_name() != Some(OsStr::new(&format!(".poema-save-{}.incomplete", expected.operation)))
        {
            return Err(error("original completed publication intent changed before its final fence"));
        }
        fence_metadata_value(expected, &publication.controls.intent)?;
        let receipt = &publication.receipt;
        if !publication.receipt_durable
            || receipt.kind != "poema-retained-file-receipt"
            || receipt.version != 1
            || receipt.operation != expected.operation
            || receipt.intent_sha256 != publication.controls.intent.sha256
            || receipt.outcome != Outcome::Published
            || receipt.output.as_ref().is_none_or(|output| !moved_same(output, &expected.slot_before))
            || match (&expected.target_before, &receipt.displaced) {
                (None, None) => false,
                (Some(before), Some(displaced)) => !moved_same(before, displaced),
                _ => true,
            }
        {
            return Err(error("same original completed publication required for a final read fence"));
        }
        let dispatch = Dispatch {
            kind: "poema-retained-file-dispatch".into(),
            version: 1,
            operation: expected.operation.clone(),
            intent_sha256: publication.controls.intent.sha256.clone(),
        };
        fence_metadata_value(&dispatch, publication.controls.dispatch.as_ref().ok_or_else(|| error("original dispatch generation missing"))?)?;
        fence_metadata_value(receipt, publication.controls.receipt.as_ref().ok_or_else(|| error("original receipt generation missing"))?)?;
        let parent = Directory::open(&expected.parent.path, false)?;
        if !directory_effect::matches_sealed(&expected.parent, &parent.pin, seals)? {
            return Err(error("original completed publication parent changed"));
        }
        if let Some(archive) = &publication.archive_context {
            archive.storage(&inspect)?;
        }
        root.fence(OsStr::new(PAYLOAD), Some(&expected.payload_archive))?;
        root.fence(OsStr::new(SLOT), receipt.displaced.as_ref())?;
        parent.fence(expected.destination.file_name().ok_or_else(|| error("destination leaf missing"))?, receipt.output.as_ref())?;
        control_fences(&root, publication)?;
        parent.current()?;
        root.current()?;
        if let Some(archive) = &publication.archive_context {
            archive.storage(&inspect)?;
        }
        inspect()
    })
}
// Bound serialization before allocating or hashing hostile public descriptions.
// The digest binds the retained writer's private control observation, not a new
// observation or a caller-provided matching JSON object.
fn fence_metadata_value(value: &impl Serialize, expected: &Observation) -> io::Result<()> {
    struct BoundedDigest {
        digest: Sha256,
        bytes: u64,
    }
    impl io::Write for BoundedDigest {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            let length = u64::try_from(bytes.len()).map_err(io::Error::other)?;
            self.bytes =
                self.bytes.checked_add(length).filter(|length| *length <= MAX_METADATA).ok_or_else(|| error("original metadata fence exceeds its bound"))?;
            self.digest.update(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    let mut observed = BoundedDigest { digest: Sha256::new(), bytes: 0 };
    serde_json::to_writer(&mut observed, value).map_err(error_json)?;
    if observed.bytes != expected.bytes || format!("{:x}", observed.digest.finalize()) != expected.sha256 {
        return Err(error("metadata differs from the original retained writer's control"));
    }
    Ok(())
}
fn control_fences(root: &Directory, publication: &Publication) -> io::Result<()> {
    if root.pin != publication.original_directory {
        return Err(error("original publication control directory changed"));
    }
    root.fence(OsStr::new(INTENT), Some(&publication.controls.intent))
        .map_err(|e| error(&format!("retained immutable intent differs from its actual original owner: {e}")))?;
    root.fence(OsStr::new(DISPATCH), publication.controls.dispatch.as_ref())?;
    root.fence(OsStr::new(RECEIPT), publication.controls.receipt.as_ref())
}
fn inspect_expected_with_controls(
    directory: &Path,
    expected: Option<&Intent>,
    seals: &[directory_effect::SealedDirectory],
    publication: Option<&Publication>,
    check: &impl Fn() -> io::Result<()>,
    before_fence: &mut impl FnMut() -> io::Result<()>,
) -> io::Result<Inspection> {
    readonly::observe(check, |pass| {
        let inspect = || pass.check();
        let check = &inspect;
        let root = Directory::open(directory, true)?;
        if let Some(publication) = publication {
            control_fences(&root, publication)?;
        }
        let (intent, intent_sha256) = root.read_json::<Intent>(INTENT, check)?.ok_or_else(|| error("retained intent unavailable"))?;
        if intent.kind != "poema-retained-file-intent"
            || intent.version != 1
            || !valid_id(&intent.operation)
            || !absolute(&intent.destination)
            || intent.directory != root.pin
            || intent.parent.path != intent.destination.parent().ok_or_else(|| error("invalid intent parent"))?
            || directory.file_name() != Some(OsStr::new(&format!(".poema-save-{}.incomplete", intent.operation)))
        {
            return Err(error("retained intent directory or identity is foreign"));
        }
        if expected.is_some_and(|original| original != &intent) {
            return Err(error("retained immutable intent differs from its actual original owner"));
        }
        let payload_bound = expected.map_or(MAX_BYTES, |v| v.payload_archive.bytes.min(MAX_BYTES));
        let target_bound = expected.map_or(MAX_BYTES, |v| v.slot_before.bytes.max(v.target_before.as_ref().map_or(0, |b| b.bytes)).min(MAX_BYTES));
        let parent = Directory::open(&intent.parent.path, false)?;
        if !directory_effect::matches_sealed(&intent.parent, &parent.pin, seals)? {
            return Err(error("retained destination parent identity changed"));
        }
        let archive = root.observe(OsStr::new(PAYLOAD), payload_bound, check)?.ok_or_else(|| error("retained payload unavailable"))?;
        if archive != intent.payload_archive {
            return Err(error("retained payload changed; no recovery authority"));
        }
        let dispatch = root.read_json::<Dispatch>(DISPATCH, check)?.map(|v| v.0);
        if dispatch
            .as_ref()
            .is_some_and(|v| v.kind != "poema-retained-file-dispatch" || v.version != 1 || v.operation != intent.operation || v.intent_sha256 != intent_sha256)
        {
            return Err(error("foreign retained dispatch"));
        }
        let recorded = root.read_json::<Receipt>(RECEIPT, check)?.map(|v| v.0);
        if recorded
            .as_ref()
            .is_some_and(|v| v.kind != "poema-retained-file-receipt" || v.version != 1 || v.operation != intent.operation || v.intent_sha256 != intent_sha256)
        {
            return Err(error("foreign retained receipt"));
        }
        let output = parent.observe(intent.destination.file_name().ok_or_else(|| error("missing destination leaf"))?, target_bound, check)?;
        let slot = root.observe(OsStr::new(SLOT), target_bound, check)?;
        let current = if dispatch.is_none() {
            if output == intent.target_before && slot.as_ref() == Some(&intent.slot_before) {
                if recorded.as_ref().is_some_and(|r| r.outcome == Outcome::NotPublished) { CurrentState::NotPublished } else { CurrentState::Prepared }
            } else {
                CurrentState::Changed
            }
        } else if output.as_ref().is_some_and(|v| moved_same(v, &intent.slot_before)) {
            match (&intent.target_before, &slot) {
                (None, None) => CurrentState::Published,
                (Some(old), Some(now)) if moved_same(old, now) => CurrentState::Published,
                _ => CurrentState::ConflictRetained,
            }
        } else if output == intent.target_before && slot.as_ref() == Some(&intent.slot_before) {
            if recorded.as_ref().is_some_and(|r| r.outcome == Outcome::NotPublished) { CurrentState::NotPublished } else { CurrentState::Unresolved }
        } else {
            CurrentState::Changed
        };
        before_fence()?;
        let (again, digest) = root.read_json::<Intent>(INTENT, check)?.ok_or_else(|| error("retained intent disappeared"))?;
        if again != intent || digest != intent_sha256 {
            return Err(error("retained intent changed during inspection"));
        }
        if root.read_json::<Dispatch>(DISPATCH, check)?.map(|v| v.0) != dispatch || root.read_json::<Receipt>(RECEIPT, check)?.map(|v| v.0) != recorded {
            return Err(error("retained dispatch/receipt changed during inspection"));
        }
        if let Some(publication) = publication {
            control_fences(&root, publication)?;
        }
        root.fence(OsStr::new(PAYLOAD), Some(&archive))?;
        root.fence(OsStr::new(SLOT), slot.as_ref())?;
        parent.fence(intent.destination.file_name().ok_or_else(|| error("destination leaf missing"))?, output.as_ref())?;
        parent.current()?;
        root.current()?;
        check()?;
        Ok(Inspection {
            intent,
            dispatch: dispatch.is_some(),
            recorded_receipt: recorded,
            current,
            output,
            slot,
            write_authority: false,
            startup_authority: false,
            native_undo: false,
        })
    })
}
#[cfg(test)]
#[path = "retained_tests.rs"]
mod tests;

/// Retained source-file relocation for the original preview rename branch.
pub mod relocation;

/// Read-only complete file/directory/storage observations for native preview planning.
pub mod preview_context;

/// Descriptor-relative missing-directory effects; returned observations are not custody.
pub mod directory_effect;

#[cfg(test)]
#[path = "retained/observed_stage_tests.rs"]
mod observed_stage_tests;

/// Exclusive private companions using the original directory and byte writer.
pub mod private_record;

#[cfg(test)]
#[path = "retained/external_archive_tests.rs"]
mod external_archive_tests;
