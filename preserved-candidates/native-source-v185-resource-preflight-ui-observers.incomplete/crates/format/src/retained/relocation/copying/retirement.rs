//! The original copy/remove fallback's remove effect, preserving the actual
//! source inode in a private source-side directory. Only a live qualified copy
//! continuation can stage it. No deletion, reverse move, replay or cleanup.
use super::{
    CopyOwner, DISPATCH, Directory, DirectoryPin, Dispatch, INTENT, Intent as CopyIntent, MAX_BYTES, Observation, RECEIPT, Receipt as CopyReceipt, absolute,
    error, hash, leaf, moved_same, valid_id,
};
use rustix::fs::{Mode, RenameFlags, mkdirat, renameat_with};
use serde::{Deserialize, Serialize};
use std::{
    ffi::OsStr,
    io,
    path::{Path, PathBuf},
};
const RETAINED: &str = "retained-source.bin";
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Intent {
    pub kind: String,
    pub version: u32,
    pub operation: String,
    pub directory: DirectoryPin,
    pub copy: CopyIntent,
    pub copy_digest: String,
    pub copy_receipt: CopyReceipt,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    Retired,
    NotRetired,
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
    pub retained_after: Option<Observation>,
    pub error: Option<String>,
}
pub struct Publication {
    pub receipt: Receipt,
    pub receipt_durable: bool,
    pub directory: PathBuf,
}
pub struct StagedRetirement {
    owner: CopyOwner,
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
pub(super) fn stage(owner: CopyOwner, check: &impl Fn() -> io::Result<()>) -> io::Result<StagedRetirement> {
    check()?;
    owner.verify(true, check)?;
    let name = format!(".poema-retire-{}.incomplete", owner.intent.operation);
    mkdirat(&owner.source.file, name.as_str(), Mode::RWXU)?;
    owner.source.file.sync_all()?;
    let root = Directory::open(&owner.source.pin.path.join(name), true)?;
    if root.pin.identity.device != owner.source.pin.identity.device {
        return Err(error("source retention staging left its admitted source volume"));
    }
    let intent = Intent {
        kind: "poema-retained-source-retirement-intent".into(),
        version: 1,
        operation: owner.intent.operation.clone(),
        directory: root.pin.clone(),
        copy: owner.intent.clone(),
        copy_digest: owner.digest.clone(),
        copy_receipt: owner.receipt.clone(),
    };
    let raw = root.json(INTENT, &intent, check)?;
    let digest = hash(&raw, check)?;
    let value = StagedRetirement { owner, root, intent, digest, attempted: false, kernel_succeeded: false, synced: false };
    value.current(true, check)?;
    Ok(value)
}
impl StagedRetirement {
    pub fn directory(&self) -> &Path {
        &self.root.pin.path
    }
    pub fn intent(&self) -> &Intent {
        &self.intent
    }
    fn current(&self, require_source: bool, check: &impl Fn() -> io::Result<()>) -> io::Result<()> {
        check()?;
        self.root.current()?;
        self.owner.verify(require_source, check)?;
        let (intent, digest) = self.root.read_json::<Intent>(INTENT, check)?.ok_or_else(|| error("source-retirement intent missing"))?;
        if intent != self.intent || digest != self.digest {
            return Err(error("source-retirement immutable intent changed"));
        }
        Ok(())
    }
    pub fn publish(self, check: &impl Fn() -> io::Result<()>) -> Publication {
        self.publish_with(check, &mut |_| Ok(()))
    }
    fn publish_with(mut self, check: &impl Fn() -> io::Result<()>, hook: &mut impl FnMut(Point) -> io::Result<()>) -> Publication {
        let mut outcome = Outcome::NotRetired;
        let mut source_after = None;
        let mut retained_after = None;
        let attempt = (|| {
            self.current(true, check)?;
            if self.root.observe(OsStr::new(RETAINED), MAX_BYTES, check)?.is_some() {
                return Err(error("source-retirement slot is occupied; no overwrite"));
            }
            self.root.json(
                DISPATCH,
                &Dispatch {
                    kind: "poema-retained-source-retirement-dispatch".into(),
                    version: 1,
                    operation: self.intent.operation.clone(),
                    intent_sha256: self.digest.clone(),
                },
                check,
            )?;
            self.current(true, check)?;
            if self.root.observe(OsStr::new(RETAINED), MAX_BYTES, check)?.is_some() {
                return Err(error("source-retirement slot changed at final fence"));
            }
            hook(Point::BeforeKernel)?;
            check()?;
            self.attempted = true;
            outcome = Outcome::Uncertain;
            if let Err(e) = renameat_with(&self.owner.source.file, leaf(&self.intent.copy.source)?, &self.root.file, RETAINED, RenameFlags::NOREPLACE) {
                self.current(true, check)?;
                source_after = self.owner.source.observe(leaf(&self.intent.copy.source)?, MAX_BYTES, check)?;
                retained_after = self.root.observe(OsStr::new(RETAINED), MAX_BYTES, check)?;
                if source_after.as_ref() == Some(&self.intent.copy.source_before) && retained_after.is_none() {
                    outcome = Outcome::NotRetired;
                }
                return Err(e.into());
            }
            self.kernel_succeeded = true;
            hook(Point::AfterKernel)?;
            self.owner.source.file.sync_all()?;
            self.root.file.sync_all()?;
            self.synced = true;
            self.current(false, check)?;
            source_after = self.owner.source.observe(leaf(&self.intent.copy.source)?, MAX_BYTES, check)?;
            retained_after = self.root.observe(OsStr::new(RETAINED), MAX_BYTES, check)?;
            outcome = if source_after.is_none() && retained_after.as_ref().is_some_and(|v| moved_same(v, &self.intent.copy.source_before)) {
                Outcome::Retired
            } else {
                Outcome::ConflictRetained
            };
            self.owner.source.fence(leaf(&self.intent.copy.source)?, source_after.as_ref())?;
            self.root.fence(OsStr::new(RETAINED), retained_after.as_ref())?;
            hook(Point::BeforeReceipt)?;
            Ok(())
        })();
        let failure = attempt.err().map(|e| e.to_string());
        if failure.is_some() && self.kernel_succeeded {
            outcome = Outcome::Uncertain;
        }
        let mut receipt = Receipt {
            kind: "poema-retained-source-retirement-receipt".into(),
            version: 1,
            operation: self.intent.operation.clone(),
            intent_sha256: self.digest.clone(),
            outcome,
            attempted: self.attempted,
            directories_synced: self.synced,
            source_after,
            retained_after,
            error: failure,
        };
        let durable = self.root.json(RECEIPT, &receipt, &|| Ok(())).is_ok();
        if !durable {
            receipt.outcome = Outcome::Uncertain;
            receipt.error = Some("source-retirement receipt durability unconfirmed; inspect without removing or retrying".into());
        }
        Publication { receipt, receipt_durable: durable, directory: self.root.pin.path.clone() }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CurrentState {
    Prepared,
    Retired,
    NotRetired,
    ConflictRetained,
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
    pub source: Option<Observation>,
    pub retained: Option<Observation>,
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
    let (intent, digest) = root.read_json::<Intent>(INTENT, check)?.ok_or_else(|| error("retirement intent missing"))?;
    if intent.kind != "poema-retained-source-retirement-intent"
        || intent.version != 1
        || !valid_id(&intent.operation)
        || intent.operation != intent.copy.operation
        || intent.directory != root.pin
        || directory.file_name() != Some(OsStr::new(&format!(".poema-retire-{}.incomplete", intent.operation)))
        || !absolute(&intent.copy.source)
        || !absolute(&intent.copy.destination)
        || directory.parent() != Some(intent.copy.source_parent.path.as_path())
    {
        return Err(error("foreign source-retirement intent or directory"));
    }
    let copied = super::inspect(&intent.copy.directory.path, check)?;
    if copied.intent != intent.copy
        || copied.intent_sha256 != intent.copy_digest
        || copied.recorded_receipt.as_ref() != Some(&intent.copy_receipt)
        || !copied.dispatch
        || intent.copy_receipt.outcome != super::Outcome::Copied
    {
        return Err(error("source retirement does not match its original complete copy record"));
    }
    let source_parent = Directory::open(&intent.copy.source_parent.path, false)?;
    let destination_parent = Directory::open(&intent.copy.destination_parent.path, false)?;
    if source_parent.pin != intent.copy.source_parent || destination_parent.pin != intent.copy.destination_parent {
        return Err(error("source-retirement parent identity changed"));
    }
    let dispatch = root.read_json::<Dispatch>(DISPATCH, check)?.map(|v| v.0);
    if dispatch.as_ref().is_some_and(|v| {
        v.kind != "poema-retained-source-retirement-dispatch" || v.version != 1 || v.operation != intent.operation || v.intent_sha256 != digest
    }) {
        return Err(error("foreign source-retirement dispatch"));
    }
    let recorded = root.read_json::<Receipt>(RECEIPT, check)?.map(|v| v.0);
    if recorded
        .as_ref()
        .is_some_and(|v| v.kind != "poema-retained-source-retirement-receipt" || v.version != 1 || v.operation != intent.operation || v.intent_sha256 != digest)
    {
        return Err(error("foreign source-retirement receipt"));
    }
    let source = source_parent.observe(leaf(&intent.copy.source)?, MAX_BYTES, check)?;
    let retained = root.observe(OsStr::new(RETAINED), MAX_BYTES, check)?;
    let destination = destination_parent.observe(leaf(&intent.copy.destination)?, MAX_BYTES, check)?;
    let destination_matches = destination.as_ref().is_some_and(|v| moved_same(v, &intent.copy.slot)) && copied.slot.is_none();
    let current = if destination_matches && source.as_ref() == Some(&intent.copy.source_before) && retained.is_none() {
        if dispatch.is_none() {
            CurrentState::Prepared
        } else if recorded.as_ref().is_some_and(|r| r.outcome == Outcome::NotRetired) {
            CurrentState::NotRetired
        } else {
            CurrentState::Unresolved
        }
    } else if dispatch.is_some() && destination_matches && source.is_none() && retained.as_ref().is_some_and(|v| moved_same(v, &intent.copy.source_before)) {
        CurrentState::Retired
    } else if dispatch.is_some() {
        CurrentState::ConflictRetained
    } else {
        CurrentState::Changed
    };
    before_fence()?;
    let (again, again_digest) = root.read_json::<Intent>(INTENT, check)?.ok_or_else(|| error("source-retirement intent disappeared"))?;
    if again != intent
        || again_digest != digest
        || root.read_json::<Dispatch>(DISPATCH, check)?.map(|v| v.0) != dispatch
        || root.read_json::<Receipt>(RECEIPT, check)?.map(|v| v.0) != recorded
    {
        return Err(error("source-retirement metadata changed during full inspection"));
    }
    // Re-read the complete bound copy after later source/retained/metadata reads.
    let copy_again = super::inspect(&intent.copy.directory.path, check)?;
    if copy_again.intent != copied.intent
        || copy_again.intent_sha256 != copied.intent_sha256
        || copy_again.recorded_receipt != copied.recorded_receipt
        || copy_again.dispatch != copied.dispatch
        || copy_again.destination != copied.destination
        || copy_again.slot != copied.slot
        || copy_again.source != copied.source
    {
        return Err(error("bound copy changed during source-retirement inspection"));
    }
    source_parent.fence(leaf(&intent.copy.source)?, source.as_ref())?;
    destination_parent.fence(leaf(&intent.copy.destination)?, destination.as_ref())?;
    root.fence(OsStr::new(RETAINED), retained.as_ref())?;
    root.current()?;
    source_parent.current()?;
    destination_parent.current()?;
    check()?;
    Ok(Inspection {
        intent,
        dispatch: dispatch.is_some(),
        recorded_receipt: recorded,
        current,
        source,
        retained,
        destination,
        write_authority: false,
        startup_authority: false,
        native_undo: false,
    })
}
#[cfg(test)]
#[path = "retirement_tests.rs"]
mod tests;
