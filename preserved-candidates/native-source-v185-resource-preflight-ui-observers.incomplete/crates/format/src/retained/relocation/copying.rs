//! Explicit streamed copy plus a live source-retirement continuation. No engine
//! policy, automatic fallback, arbitrary removal, native or startup grant.
use super::{
    ARCHIVE, DISPATCH, Directory, DirectoryPin, Dispatch, INTENT, MAX_BYTES, Observation, RECEIPT, absolute, copy_archive, error, hash, leaf, moved_same,
    stream_copy, valid_id,
};
use rustix::fs::{Mode, RenameFlags, mkdirat, renameat_with};
use serde::{Deserialize, Serialize};
use std::{
    ffi::OsStr,
    io,
    path::{Path, PathBuf},
};
const SLOT: &str = "copy-slot.bin";
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
    pub slot: Observation,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    Copied,
    NotCopied,
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
    pub source_after: Option<Observation>,
    pub destination_after: Option<Observation>,
    pub error: Option<String>,
}
/// Display fields are historical; only the private live owner can stage its
/// exact original source retirement. This type is not Clone/Deserialize.
pub struct Publication {
    pub receipt: Receipt,
    pub receipt_durable: bool,
    pub directory: PathBuf,
    retirement: Option<CopyOwner>,
}
struct CopyOwner {
    source: Directory,
    destination: Directory,
    root: Directory,
    intent: Intent,
    digest: String,
    receipt: Receipt,
}
pub struct StagedCopy {
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
pub fn stage(source: &Path, destination: &Path, operation: &str, check: &impl Fn() -> io::Result<()>) -> io::Result<StagedCopy> {
    check()?;
    if !absolute(source) || !absolute(destination) || source == destination || !valid_id(operation) {
        return Err(error("explicit distinct normalized preview copy paths and operation required"));
    }
    let source_parent = Directory::open(source.parent().ok_or_else(|| error("copy source parent missing"))?, false)?;
    let destination_parent = Directory::open(destination.parent().ok_or_else(|| error("copy destination parent missing"))?, false)?;
    let source_before = source_parent.observe(leaf(source)?, MAX_BYTES, check)?.ok_or_else(|| error("complete copy source required"))?;
    if destination_parent.observe(leaf(destination)?, MAX_BYTES, check)?.is_some() {
        return Err(error("existing preview copy destination is preserved"));
    }
    let name = format!(".poema-copy-{operation}.incomplete");
    mkdirat(&destination_parent.file, name.as_str(), Mode::RWXU)?;
    destination_parent.file.sync_all()?;
    let root = Directory::open(&destination_parent.pin.path.join(name), true)?;
    if root.pin.identity.device != destination_parent.pin.identity.device {
        return Err(error("preview copy staging left its admitted destination volume"));
    }
    let archive = copy_archive(&source_parent, leaf(source)?, &source_before, &root, check)?;
    let permissions =
        Mode::from_bits_truncate((source_before.identity.mode & 0o7777).try_into().map_err(|_| error("source permission bits exceed native mode width"))?);
    let slot = stream_copy(&root, OsStr::new(ARCHIVE), &archive, &root, SLOT, permissions, check)?;
    if slot.identity == archive.identity || slot.sha256 != source_before.sha256 || slot.bytes != source_before.bytes {
        return Err(error("preview copy slot is not an independent complete payload"));
    }
    if source_parent.observe(leaf(source)?, MAX_BYTES, check)?.as_ref() != Some(&source_before)
        || destination_parent.observe(leaf(destination)?, MAX_BYTES, check)?.is_some()
    {
        return Err(error("copy source/destination changed during complete staging"));
    }
    let intent = Intent {
        kind: "poema-retained-copy-intent".into(),
        version: 1,
        operation: operation.into(),
        source: source.into(),
        destination: destination.into(),
        source_parent: source_parent.pin.clone(),
        destination_parent: destination_parent.pin.clone(),
        directory: root.pin.clone(),
        source_before,
        archive,
        slot,
    };
    let raw = root.json(INTENT, &intent, check)?;
    let digest = hash(&raw, check)?;
    let value =
        StagedCopy { source: source_parent, destination: destination_parent, root, intent, digest, attempted: false, kernel_succeeded: false, synced: false };
    value.current(check)?;
    Ok(value)
}
impl StagedCopy {
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
        let (intent, digest) = self.root.read_json::<Intent>(INTENT, check)?.ok_or_else(|| error("copy intent missing"))?;
        if intent != self.intent || digest != self.digest {
            return Err(error("copy immutable intent changed"));
        }
        if self.root.observe(OsStr::new(ARCHIVE), MAX_BYTES, check)?.as_ref() != Some(&self.intent.archive) {
            return Err(error("copy independent archive changed"));
        }
        Ok(())
    }
    pub fn publish(self, check: &impl Fn() -> io::Result<()>) -> Publication {
        self.publish_with(check, &mut |_| Ok(()))
    }
    fn publish_with(mut self, check: &impl Fn() -> io::Result<()>, hook: &mut impl FnMut(Point) -> io::Result<()>) -> Publication {
        let mut outcome = Outcome::NotCopied;
        let mut source_after = None;
        let mut destination_after = None;
        let attempt = (|| {
            self.current(check)?;
            if self.source.observe(leaf(&self.intent.source)?, MAX_BYTES, check)?.as_ref() != Some(&self.intent.source_before)
                || self.destination.observe(leaf(&self.intent.destination)?, MAX_BYTES, check)?.is_some()
                || self.root.observe(OsStr::new(SLOT), MAX_BYTES, check)?.as_ref() != Some(&self.intent.slot)
            {
                return Err(error("copy source/destination/slot changed before dispatch"));
            }
            self.root.json(
                DISPATCH,
                &Dispatch {
                    kind: "poema-retained-copy-dispatch".into(),
                    version: 1,
                    operation: self.intent.operation.clone(),
                    intent_sha256: self.digest.clone(),
                },
                check,
            )?;
            self.current(check)?;
            if self.source.observe(leaf(&self.intent.source)?, MAX_BYTES, check)?.as_ref() != Some(&self.intent.source_before)
                || self.destination.observe(leaf(&self.intent.destination)?, MAX_BYTES, check)?.is_some()
                || self.root.observe(OsStr::new(SLOT), MAX_BYTES, check)?.as_ref() != Some(&self.intent.slot)
            {
                return Err(error("copy changed at final publication fence"));
            }
            hook(Point::BeforeKernel)?;
            check()?;
            self.attempted = true;
            outcome = Outcome::Uncertain;
            if let Err(e) = renameat_with(&self.root.file, SLOT, &self.destination.file, leaf(&self.intent.destination)?, RenameFlags::NOREPLACE) {
                self.current(check)?;
                source_after = self.source.observe(leaf(&self.intent.source)?, MAX_BYTES, check)?;
                destination_after = self.destination.observe(leaf(&self.intent.destination)?, MAX_BYTES, check)?;
                if source_after.as_ref() == Some(&self.intent.source_before)
                    && destination_after.is_none()
                    && self.root.observe(OsStr::new(SLOT), MAX_BYTES, check)?.as_ref() == Some(&self.intent.slot)
                {
                    outcome = Outcome::NotCopied;
                }
                return Err(e.into());
            }
            self.kernel_succeeded = true;
            hook(Point::AfterKernel)?;
            self.destination.file.sync_all()?;
            self.root.file.sync_all()?;
            self.synced = true;
            self.current(check)?;
            source_after = self.source.observe(leaf(&self.intent.source)?, MAX_BYTES, check)?;
            destination_after = self.destination.observe(leaf(&self.intent.destination)?, MAX_BYTES, check)?;
            outcome =
                if source_after.as_ref() == Some(&self.intent.source_before) && destination_after.as_ref().is_some_and(|v| moved_same(v, &self.intent.slot)) {
                    Outcome::Copied
                } else {
                    Outcome::ConflictRetained
                };
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
            kind: "poema-retained-copy-receipt".into(),
            version: 1,
            operation: self.intent.operation.clone(),
            intent_sha256: self.digest.clone(),
            outcome,
            attempted: self.attempted,
            directories_synced: self.synced,
            source_after,
            destination_after,
            error: failure,
        };
        let durable = self.root.json(RECEIPT, &receipt, &|| Ok(())).is_ok();
        if !durable {
            receipt.outcome = Outcome::Uncertain;
            receipt.error = Some("preview copy receipt durability unconfirmed; inspect without republishing".into());
        }
        let qualified = durable
            && receipt.outcome == Outcome::Copied
            && inspect(&self.root.pin.path, check).is_ok_and(|v| v.current == CurrentState::Copied && v.recorded_receipt.as_ref() == Some(&receipt));
        let directory = self.root.pin.path.clone();
        let retirement = qualified.then_some(CopyOwner {
            source: self.source,
            destination: self.destination,
            root: self.root,
            intent: self.intent,
            digest: self.digest,
            receipt: receipt.clone(),
        });
        Publication { receipt, receipt_durable: durable, directory, retirement }
    }
}
impl CopyOwner {
    fn verify(&self, require_source: bool, check: &impl Fn() -> io::Result<()>) -> io::Result<Inspection> {
        self.source.current()?;
        self.destination.current()?;
        self.root.current()?;
        let seen = inspect(&self.root.pin.path, check)?;
        if seen.intent != self.intent
            || seen.intent_sha256 != self.digest
            || seen.recorded_receipt.as_ref() != Some(&self.receipt)
            || !seen.dispatch
            || seen.destination.as_ref().is_none_or(|v| !moved_same(v, &self.intent.slot))
            || seen.slot.is_some()
            || require_source && seen.source.as_ref() != Some(&self.intent.source_before)
        {
            return Err(error("live copied source/destination/archive/receipt changed; no retirement admitted"));
        }
        Ok(seen)
    }
}
impl Publication {
    pub fn verify_current_after_directory_seals(
        &self,
        seals: &[crate::retained::directory_effect::SealedDirectory],
        check: &impl Fn() -> io::Result<()>,
    ) -> io::Result<()> {
        let owner = self.retirement.as_ref().ok_or_else(|| error("actual qualified copy owner required"))?;
        if self.receipt != owner.receipt || !self.receipt_durable || self.directory != owner.root.pin.path {
            return Err(error("copy display fields changed"));
        }
        owner.source.current()?;
        owner.root.current()?;
        let actual = Directory::open(&owner.destination.pin.path, false)?;
        if !crate::retained::directory_effect::matches_sealed(&owner.destination.pin, &actual.pin, seals)? {
            return Err(error("copy destination parent changed without its actual seal"));
        }
        let seen = inspect_with_seals(&owner.root.pin.path, seals, check, &mut || Ok(()))?;
        if seen.intent != owner.intent
            || seen.intent_sha256 != owner.digest
            || seen.recorded_receipt.as_ref() != Some(&owner.receipt)
            || seen.current != CurrentState::Copied
            || !seen.dispatch
            || seen.slot.is_some()
            || seen.source.as_ref() != Some(&owner.intent.source_before)
            || seen.destination.as_ref().is_none_or(|v| !moved_same(v, &owner.intent.slot))
        {
            return Err(error("sealed copy source/destination/archive/receipt changed"));
        }
        check()
    }
    pub fn verify_current(&self, check: &impl Fn() -> io::Result<()>) -> io::Result<()> {
        let owner = self.retirement.as_ref().ok_or_else(|| error("live qualified copy continuation is absent or already consumed"))?;
        if self.receipt != owner.receipt || !self.receipt_durable || self.directory != owner.root.pin.path {
            return Err(error("copy historical display fields changed"));
        }
        owner.verify(true, check).map(|_| ())
    }
    pub fn stage_retirement(&mut self, check: &impl Fn() -> io::Result<()>) -> io::Result<retirement::StagedRetirement> {
        check()?;
        let owner =
            self.retirement.as_ref().ok_or_else(|| error("no unused live qualified copy owner; historical receipt cannot authorize source retirement"))?;
        if self.receipt != owner.receipt || !self.receipt_durable || self.directory != owner.root.pin.path {
            return Err(error("copy display metadata changed; no new retirement admitted"));
        }
        owner.verify(true, check)?;
        let owner = self.retirement.take().ok_or_else(|| error("copy retirement continuation already consumed"))?;
        retirement::stage(owner, check)
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CurrentState {
    Prepared,
    Copied,
    CopiedSourceChanged,
    NotCopied,
    ConflictRetained,
    Unresolved,
    Changed,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Inspection {
    pub intent: Intent,
    pub intent_sha256: String,
    pub dispatch: bool,
    pub recorded_receipt: Option<Receipt>,
    pub current: CurrentState,
    pub source: Option<Observation>,
    pub destination: Option<Observation>,
    pub slot: Option<Observation>,
    pub write_authority: bool,
    pub retirement_authority: bool,
    pub startup_authority: bool,
    pub native_undo: bool,
}
pub fn inspect(directory: &Path, check: &impl Fn() -> io::Result<()>) -> io::Result<Inspection> {
    inspect_with(directory, check, &mut || Ok(()))
}
fn inspect_with(directory: &Path, check: &impl Fn() -> io::Result<()>, before_fence: &mut impl FnMut() -> io::Result<()>) -> io::Result<Inspection> {
    inspect_with_seals(directory, &[], check, before_fence)
}
fn inspect_with_seals(
    directory: &Path,
    seals: &[crate::retained::directory_effect::SealedDirectory],
    check: &impl Fn() -> io::Result<()>,
    before_fence: &mut impl FnMut() -> io::Result<()>,
) -> io::Result<Inspection> {
    let root = Directory::open(directory, true)?;
    let (intent, digest) = root.read_json::<Intent>(INTENT, check)?.ok_or_else(|| error("copy intent missing"))?;
    if intent.kind != "poema-retained-copy-intent"
        || intent.version != 1
        || !valid_id(&intent.operation)
        || !absolute(&intent.source)
        || !absolute(&intent.destination)
        || intent.source == intent.destination
        || intent.directory != root.pin
        || directory.file_name() != Some(OsStr::new(&format!(".poema-copy-{}.incomplete", intent.operation)))
        || intent.source_parent.path != intent.source.parent().ok_or_else(|| error("copy source parent missing"))?
        || intent.destination_parent.path != intent.destination.parent().ok_or_else(|| error("copy destination parent missing"))?
    {
        return Err(error("foreign copy intent or directory identity"));
    }
    let source_parent = Directory::open(&intent.source_parent.path, false)?;
    let destination_parent = Directory::open(&intent.destination_parent.path, false)?;
    if source_parent.pin != intent.source_parent
        || !crate::retained::directory_effect::matches_sealed(&intent.destination_parent, &destination_parent.pin, seals)?
    {
        return Err(error("copy parent identity changed"));
    }
    let archive = root.observe(OsStr::new(ARCHIVE), MAX_BYTES, check)?.ok_or_else(|| error("copy archive missing"))?;
    if archive != intent.archive
        || archive.sha256 != intent.source_before.sha256
        || archive.bytes != intent.source_before.bytes
        || archive.identity == intent.slot.identity
        || archive.identity == intent.source_before.identity
        || intent.slot.sha256 != archive.sha256
        || intent.slot.bytes != archive.bytes
    {
        return Err(error("copy independent archive/slot identity or complete bytes changed"));
    }
    let dispatch = root.read_json::<Dispatch>(DISPATCH, check)?.map(|v| v.0);
    if dispatch
        .as_ref()
        .is_some_and(|v| v.kind != "poema-retained-copy-dispatch" || v.version != 1 || v.operation != intent.operation || v.intent_sha256 != digest)
    {
        return Err(error("foreign copy dispatch"));
    }
    let recorded = root.read_json::<Receipt>(RECEIPT, check)?.map(|v| v.0);
    if recorded
        .as_ref()
        .is_some_and(|v| v.kind != "poema-retained-copy-receipt" || v.version != 1 || v.operation != intent.operation || v.intent_sha256 != digest)
    {
        return Err(error("foreign copy receipt"));
    }
    let source = source_parent.observe(leaf(&intent.source)?, MAX_BYTES, check)?;
    let destination = destination_parent.observe(leaf(&intent.destination)?, MAX_BYTES, check)?;
    let slot = root.observe(OsStr::new(SLOT), MAX_BYTES, check)?;
    let current = if source.as_ref() == Some(&intent.source_before) && destination.is_none() && slot.as_ref() == Some(&intent.slot) {
        if dispatch.is_none() {
            CurrentState::Prepared
        } else if recorded.as_ref().is_some_and(|r| r.outcome == Outcome::NotCopied) {
            CurrentState::NotCopied
        } else {
            CurrentState::Unresolved
        }
    } else if dispatch.is_some() && slot.is_none() && destination.as_ref().is_some_and(|v| moved_same(v, &intent.slot)) {
        if source.as_ref() == Some(&intent.source_before) { CurrentState::Copied } else { CurrentState::CopiedSourceChanged }
    } else if dispatch.is_some() {
        CurrentState::ConflictRetained
    } else {
        CurrentState::Changed
    };
    before_fence()?;
    let (again, again_digest) = root.read_json::<Intent>(INTENT, check)?.ok_or_else(|| error("copy intent disappeared"))?;
    if again != intent
        || again_digest != digest
        || root.read_json::<Dispatch>(DISPATCH, check)?.map(|v| v.0) != dispatch
        || root.read_json::<Receipt>(RECEIPT, check)?.map(|v| v.0) != recorded
    {
        return Err(error("copy metadata changed during full inspection"));
    }
    root.fence(OsStr::new(ARCHIVE), Some(&archive))?;
    root.fence(OsStr::new(SLOT), slot.as_ref())?;
    source_parent.fence(leaf(&intent.source)?, source.as_ref())?;
    destination_parent.fence(leaf(&intent.destination)?, destination.as_ref())?;
    root.current()?;
    source_parent.current()?;
    destination_parent.current()?;
    check()?;
    Ok(Inspection {
        intent,
        intent_sha256: digest,
        dispatch: dispatch.is_some(),
        recorded_receipt: recorded,
        current,
        source,
        destination,
        slot,
        write_authority: false,
        retirement_authority: false,
        startup_authority: false,
        native_undo: false,
    })
}
pub mod retirement;
#[cfg(test)]
#[path = "copying_tests.rs"]
mod tests;
