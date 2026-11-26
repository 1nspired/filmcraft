//! Single-use preserved source-file relocation, not overwrite/native/startup
//! authority. Original preview policy remains in the engine. No copy/remove
//! fallback, reverse move, replay or cleanup is performed by this primitive.
use super::{DISPATCH, Directory, DirectoryPin, Dispatch, INTENT, MAX_BYTES, Observation, RECEIPT, absolute, error, hash, moved_same, stamp, valid_id};
use rustix::fs::{Mode, OFlags, RenameFlags, mkdirat, openat, renameat_with};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::os::unix::fs::PermissionsExt;
use std::{
    ffi::OsStr,
    fs::File,
    io::{self, Read, Write},
    path::{Path, PathBuf},
};
const ARCHIVE: &str = "source-archive.bin";
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Intent {
    pub kind: String,
    pub version: u32,
    pub operation: String,
    pub source: PathBuf,
    pub destination: PathBuf,
    pub source_parent: DirectoryPin,
    pub destination_parent: DirectoryPin,
    pub directory: DirectoryPin,
    pub source_before: Observation,
    pub archive: Observation,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    Moved,
    NotMoved,
    ConflictRetained,
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
    pub kernel_error: Option<i32>,
    pub source_after: Option<Observation>,
    pub destination_after: Option<Observation>,
    pub error: Option<String>,
}
pub struct Publication {
    pub receipt: Receipt,
    pub receipt_durable: bool,
    pub directory: PathBuf,
}
/// A real live owner with original directory handles, never Clone/Deserialize.
pub struct StagedRename {
    source: Directory,
    destination: Directory,
    root: Directory,
    intent: Intent,
    digest: String,
    attempted: bool,
    kernel_succeeded: bool,
    synced: bool,
}
#[derive(Clone, Copy)]
enum Point {
    BeforeKernel,
    AfterKernel,
    BeforeReceipt,
}
fn leaf(p: &Path) -> io::Result<&OsStr> {
    p.file_name().ok_or_else(|| error("relocation file leaf missing"))
}
fn copy_archive(source: &Directory, name: &OsStr, expected: &Observation, root: &Directory, check: &impl Fn() -> io::Result<()>) -> io::Result<Observation> {
    stream_copy(source, name, expected, root, ARCHIVE, Mode::RUSR | Mode::WUSR, check)
}
fn stream_copy(
    source: &Directory,
    name: &OsStr,
    expected: &Observation,
    root: &Directory,
    output_name: &str,
    mode: Mode,
    check: &impl Fn() -> io::Result<()>,
) -> io::Result<Observation> {
    check()?;
    source.current()?;
    root.current()?;
    let mut input = File::from(openat(&source.file, name, OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC, Mode::empty())?);
    let meta = input.metadata()?;
    if !meta.is_file() || stamp(&meta, expected.sha256.clone()) != *expected {
        return Err(error("preview source changed before archive streaming"));
    }
    let mut output = File::from(openat(&root.file, output_name, OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC, mode)?);
    let mut digest = Sha256::new();
    let mut bytes = 0u64;
    let mut buffer = [0u8; 65536];
    loop {
        check()?;
        let count = input.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        bytes = bytes.checked_add(count as u64).ok_or_else(|| error("preview archive size overflow"))?;
        if bytes > MAX_BYTES || bytes > expected.bytes {
            return Err(error("preview source grew beyond its exact complete byte bound"));
        }
        let chunk = buffer.get(..count).ok_or_else(|| error("invalid preview stream length"))?;
        output.write_all(chunk)?;
        digest.update(chunk);
    }
    check()?;
    output.set_permissions(std::fs::Permissions::from_mode(mode.bits().into()))?;
    output.sync_all()?;
    input.sync_all()?;
    root.file.sync_all()?;
    if bytes != expected.bytes
        || format!("{:x}", digest.finalize()) != expected.sha256
        || stamp(&input.metadata()?, expected.sha256.clone()) != *expected
        || source.observe(name, MAX_BYTES, check)?.as_ref() != Some(expected)
    {
        return Err(error("complete preview source changed while archiving; no move was dispatched"));
    }
    let archive = root.observe(OsStr::new(output_name), MAX_BYTES, check)?.ok_or_else(|| error("preview streamed payload disappeared"))?;
    if archive.sha256 != expected.sha256 || archive.bytes != expected.bytes || archive.identity == expected.identity {
        return Err(error("preview archive is not an independent complete payload"));
    }
    source.fence(name, Some(expected))?;
    root.fence(OsStr::new(output_name), Some(&archive))?;
    Ok(archive)
}
pub fn stage(source: &Path, destination: &Path, operation: &str, check: &impl Fn() -> io::Result<()>) -> io::Result<StagedRename> {
    check()?;
    if !absolute(source) || !absolute(destination) || source == destination || !valid_id(operation) {
        return Err(error("explicit distinct normalized preview paths and operation required"));
    }
    let source_parent = Directory::open(source.parent().ok_or_else(|| error("source parent missing"))?, false)?;
    let destination_parent = Directory::open(destination.parent().ok_or_else(|| error("destination parent missing"))?, false)?;
    let source_before = source_parent.observe(leaf(source)?, MAX_BYTES, check)?.ok_or_else(|| error("complete preview source required"))?;
    if destination_parent.observe(leaf(destination)?, MAX_BYTES, check)?.is_some() {
        return Err(error("existing preview destination is preserved; rename cannot overwrite it"));
    }
    let directory_name = format!(".poema-move-{operation}.incomplete");
    mkdirat(&source_parent.file, directory_name.as_str(), Mode::RWXU)?;
    source_parent.file.sync_all()?;
    let root = Directory::open(&source_parent.pin.path.join(directory_name), true)?;
    let archive = copy_archive(&source_parent, leaf(source)?, &source_before, &root, check)?;
    let intent = Intent {
        kind: "poema-retained-relocation-intent".into(),
        version: 1,
        operation: operation.into(),
        source: source.into(),
        destination: destination.into(),
        source_parent: source_parent.pin.clone(),
        destination_parent: destination_parent.pin.clone(),
        directory: root.pin.clone(),
        source_before,
        archive,
    };
    let raw = root.json(INTENT, &intent, check)?;
    let digest = hash(&raw, check)?;
    let value =
        StagedRename { source: source_parent, destination: destination_parent, root, intent, digest, attempted: false, kernel_succeeded: false, synced: false };
    value.current(check)?;
    if value.source.observe(leaf(source)?, MAX_BYTES, check)?.as_ref() != Some(&value.intent.source_before)
        || value.destination.observe(leaf(destination)?, MAX_BYTES, check)?.is_some()
    {
        return Err(error("preview source or destination changed after staging"));
    }
    Ok(value)
}
impl StagedRename {
    pub fn directory(&self) -> &Path {
        &self.root.pin.path
    }
    pub fn intent(&self) -> &Intent {
        &self.intent
    }
    fn current(&self, check: &impl Fn() -> io::Result<()>) -> io::Result<()> {
        check()?;
        self.source.current()?;
        self.destination.current()?;
        self.root.current()?;
        let (raw, digest) = self.root.read_json::<Intent>(INTENT, check)?.ok_or_else(|| error("relocation intent missing"))?;
        if raw != self.intent || digest != self.digest {
            return Err(error("relocation immutable intent changed"));
        }
        let archive = self.root.observe(OsStr::new(ARCHIVE), MAX_BYTES, check)?.ok_or_else(|| error("relocation archive missing"))?;
        if archive != self.intent.archive {
            return Err(error("relocation independent archive changed"));
        }
        self.root.fence(OsStr::new(ARCHIVE), Some(&archive))?;
        Ok(())
    }
    pub fn publish(self, check: &impl Fn() -> io::Result<()>) -> Publication {
        self.publish_with(check, &mut |_| Ok(()))
    }
    fn publish_with(mut self, check: &impl Fn() -> io::Result<()>, hook: &mut impl FnMut(Point) -> io::Result<()>) -> Publication {
        let mut outcome = Outcome::NotMoved;
        let mut source_after = None;
        let mut destination_after = None;
        let mut kernel_error = None;
        let attempt = (|| {
            self.current(check)?;
            if self.source.observe(leaf(&self.intent.source)?, MAX_BYTES, check)?.as_ref() != Some(&self.intent.source_before)
                || self.destination.observe(leaf(&self.intent.destination)?, MAX_BYTES, check)?.is_some()
            {
                return Err(error("preview source/destination changed before dispatch"));
            }
            self.root.json(
                DISPATCH,
                &Dispatch {
                    kind: "poema-retained-relocation-dispatch".into(),
                    version: 1,
                    operation: self.intent.operation.clone(),
                    intent_sha256: self.digest.clone(),
                },
                check,
            )?;
            self.current(check)?;
            if self.source.observe(leaf(&self.intent.source)?, MAX_BYTES, check)?.as_ref() != Some(&self.intent.source_before)
                || self.destination.observe(leaf(&self.intent.destination)?, MAX_BYTES, check)?.is_some()
            {
                return Err(error("preview changed at final move fence"));
            }
            hook(Point::BeforeKernel)?;
            check()?;
            self.attempted = true;
            outcome = Outcome::Uncertain;
            if let Err(e) =
                renameat_with(&self.source.file, leaf(&self.intent.source)?, &self.destination.file, leaf(&self.intent.destination)?, RenameFlags::NOREPLACE)
            {
                kernel_error = Some(e.raw_os_error());
                self.current(check)?;
                source_after = self.source.observe(leaf(&self.intent.source)?, MAX_BYTES, check)?;
                destination_after = self.destination.observe(leaf(&self.intent.destination)?, MAX_BYTES, check)?;
                if source_after.as_ref() == Some(&self.intent.source_before) && destination_after.is_none() {
                    outcome = Outcome::NotMoved;
                }
                return Err(e.into());
            }
            self.kernel_succeeded = true;
            hook(Point::AfterKernel)?;
            self.source.file.sync_all()?;
            self.destination.file.sync_all()?;
            self.root.file.sync_all()?;
            self.synced = true;
            self.current(check)?;
            source_after = self.source.observe(leaf(&self.intent.source)?, MAX_BYTES, check)?;
            destination_after = self.destination.observe(leaf(&self.intent.destination)?, MAX_BYTES, check)?;
            outcome = if source_after.is_none() && destination_after.as_ref().is_some_and(|v| moved_same(v, &self.intent.source_before)) {
                Outcome::Moved
            } else {
                Outcome::ConflictRetained
            };
            // A later complete inspection is still required; these are historical returns.
            self.source.fence(leaf(&self.intent.source)?, source_after.as_ref())?;
            self.destination.fence(leaf(&self.intent.destination)?, destination_after.as_ref())?;
            hook(Point::BeforeReceipt)?;
            Ok(())
        })();
        let failure = attempt.err().map(|e| e.to_string());
        if failure.is_some() && self.kernel_succeeded {
            outcome = Outcome::Uncertain;
        }
        let mut receipt = Receipt {
            kind: "poema-retained-relocation-receipt".into(),
            version: 1,
            operation: self.intent.operation.clone(),
            intent_sha256: self.digest.clone(),
            outcome,
            attempted: self.attempted,
            directories_synced: self.synced,
            kernel_error,
            source_after,
            destination_after,
            error: failure,
        };
        let durable = self.root.json(RECEIPT, &receipt, &|| Ok(())).is_ok();
        if !durable {
            receipt.outcome = Outcome::Uncertain;
            receipt.error = Some("relocation receipt durability unconfirmed; inspect without repeating move".into());
        }
        Publication { receipt, receipt_durable: durable, directory: self.root.pin.path.clone() }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CurrentState {
    Prepared,
    Moved,
    NotMoved,
    ConflictRetained,
    Unresolved,
    Changed,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Inspection {
    pub intent: Intent,
    pub current: CurrentState,
    pub dispatch: bool,
    pub recorded_receipt: Option<Receipt>,
    pub source: Option<Observation>,
    pub destination: Option<Observation>,
    pub write_authority: bool,
    pub startup_authority: bool,
    pub native_undo: bool,
}
pub fn inspect(directory: &Path, check: &impl Fn() -> io::Result<()>) -> io::Result<Inspection> {
    inspect_with(directory, check, &mut || Ok(()))
}
fn inspect_with(directory: &Path, check: &impl Fn() -> io::Result<()>, before_fence: &mut impl FnMut() -> io::Result<()>) -> io::Result<Inspection> {
    let root = Directory::open(directory, true)?;
    let (intent, digest) = root.read_json::<Intent>(INTENT, check)?.ok_or_else(|| error("relocation intent missing"))?;
    if intent.kind != "poema-retained-relocation-intent"
        || intent.version != 1
        || !valid_id(&intent.operation)
        || !absolute(&intent.source)
        || !absolute(&intent.destination)
        || intent.source == intent.destination
        || intent.directory != root.pin
        || directory.file_name() != Some(OsStr::new(&format!(".poema-move-{}.incomplete", intent.operation)))
        || intent.source_parent.path != intent.source.parent().ok_or_else(|| error("source parent missing"))?
        || intent.destination_parent.path != intent.destination.parent().ok_or_else(|| error("destination parent missing"))?
    {
        return Err(error("relocation directory or immutable intent is foreign"));
    }
    let source_parent = Directory::open(&intent.source_parent.path, false)?;
    let destination_parent = Directory::open(&intent.destination_parent.path, false)?;
    if source_parent.pin != intent.source_parent || destination_parent.pin != intent.destination_parent {
        return Err(error("relocation parent identity changed"));
    }
    let archive = root.observe(OsStr::new(ARCHIVE), MAX_BYTES, check)?.ok_or_else(|| error("relocation archive missing"))?;
    if archive != intent.archive
        || archive.sha256 != intent.source_before.sha256
        || archive.bytes != intent.source_before.bytes
        || archive.identity == intent.source_before.identity
    {
        return Err(error("relocation complete archive changed"));
    }
    let dispatch = root.read_json::<Dispatch>(DISPATCH, check)?.map(|v| v.0);
    if dispatch
        .as_ref()
        .is_some_and(|v| v.kind != "poema-retained-relocation-dispatch" || v.version != 1 || v.operation != intent.operation || v.intent_sha256 != digest)
    {
        return Err(error("foreign relocation dispatch"));
    }
    let recorded = root.read_json::<Receipt>(RECEIPT, check)?.map(|v| v.0);
    if recorded
        .as_ref()
        .is_some_and(|v| v.kind != "poema-retained-relocation-receipt" || v.version != 1 || v.operation != intent.operation || v.intent_sha256 != digest)
    {
        return Err(error("foreign relocation receipt"));
    }
    let source = source_parent.observe(leaf(&intent.source)?, MAX_BYTES, check)?;
    let destination = destination_parent.observe(leaf(&intent.destination)?, MAX_BYTES, check)?;
    let current = if source.as_ref() == Some(&intent.source_before) && destination.is_none() {
        if dispatch.is_none() {
            CurrentState::Prepared
        } else if recorded.as_ref().is_some_and(|v| v.outcome == Outcome::NotMoved) {
            CurrentState::NotMoved
        } else {
            CurrentState::Unresolved
        }
    } else if dispatch.is_some() && source.is_none() && destination.as_ref().is_some_and(|v| moved_same(v, &intent.source_before)) {
        CurrentState::Moved
    } else if dispatch.is_some() {
        CurrentState::ConflictRetained
    } else {
        CurrentState::Changed
    };
    before_fence()?;
    let (again, again_digest) = root.read_json::<Intent>(INTENT, check)?.ok_or_else(|| error("relocation intent disappeared"))?;
    if again != intent
        || again_digest != digest
        || root.read_json::<Dispatch>(DISPATCH, check)?.map(|v| v.0) != dispatch
        || root.read_json::<Receipt>(RECEIPT, check)?.map(|v| v.0) != recorded
    {
        return Err(error("relocation metadata changed during complete inspection"));
    }
    root.fence(OsStr::new(ARCHIVE), Some(&archive))?;
    source_parent.fence(leaf(&intent.source)?, source.as_ref())?;
    destination_parent.fence(leaf(&intent.destination)?, destination.as_ref())?;
    root.current()?;
    source_parent.current()?;
    destination_parent.current()?;
    check()?;
    Ok(Inspection {
        intent,
        current,
        dispatch: dispatch.is_some(),
        recorded_receipt: recorded,
        source,
        destination,
        write_authority: false,
        startup_authority: false,
        native_undo: false,
    })
}
#[cfg(test)]
#[path = "relocation_tests.rs"]
mod tests;

/// Explicit preserved copy and source retirement; original engine policy selects fallback.
pub mod copying;
