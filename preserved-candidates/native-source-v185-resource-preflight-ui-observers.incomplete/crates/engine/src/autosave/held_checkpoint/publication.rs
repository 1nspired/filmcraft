//! Exact original encoded checkpoint bytes, published on their SAME gated worker.
//! This owns actual created directories and retained file receipts. It grants no
//! ordinary restart selection, application settlement or worker release.
use super::*;
use filmcraft_format::retained::{
    self, Outcome,
    directory_effect::{self, OwnedDirectory},
    preview_context::{DirectoryFrame, NodeFrame, NodeState},
};
use std::{io, path::Path, sync::atomic::AtomicBool};
mod namespace;
pub(crate) mod release;

/// One fresh complete source query, followed only by a same-query read fence.
/// Neither method supplies publication or native ownership. The caller must
/// never substitute the after-read fence for the complete query before a write.
pub(crate) trait CheckpointSource: Send + Sync {
    fn current(&self) -> io::Result<()>;
    fn fence_after_read(&self) -> io::Result<()>;
}
impl<F: Fn() -> io::Result<()> + Send + Sync> CheckpointSource for F {
    fn current(&self) -> io::Result<()> {
        self()
    }
    fn fence_after_read(&self) -> io::Result<()> {
        self()
    }
}
pub(crate) type Source = Arc<dyn CheckpointSource>;
fn source_read<T>(source: &dyn CheckpointSource, read: impl FnOnce() -> io::Result<T>) -> io::Result<T> {
    source.current()?;
    let observed = read();
    source.fence_after_read()?;
    observed
}
pub(in crate::autosave) struct Input {
    encoded: Encoded,
    data: Arc<DirectoryFrame>,
    root: Arc<DirectoryFrame>,
    epoch: String,
    reserve: u64,
    source: Source,
    aborted: Arc<AtomicBool>,
    began: super::super::Instant,
}
struct AbortOnDrop(Arc<AtomicBool>);
impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Release);
    }
}
pub(crate) struct Published {
    encoded: Encoded,
    source: Source,
    data: Arc<DirectoryFrame>,
    epoch: String,
    root: OwnedDirectory,
    files: OwnedDirectory,
    writes: OwnedDirectory,
    outputs: Vec<NodeFrame>,
    publications: Vec<retained::Publication>,
    intents: Vec<retained::Intent>,
}
fn failure(e: impl std::fmt::Display) -> String {
    e.to_string()
}
// Optional task-owned diagnostic transport only: static phase labels and time,
// never paths, file contents, permission, publication or release authority.
struct Timing {
    began: super::super::Instant,
    scope: &'static str,
    enabled: bool,
}
impl Timing {
    fn new(scope: &'static str) -> Self {
        Self { began: super::super::Instant::now(), scope, enabled: std::env::var_os("POEMA_NATIVE_CHECKPOINT_TIMING").is_some_and(|v| v == "1") }
    }
    fn phase(&self, phase: &'static str) -> Result<(), String> {
        if self.enabled {
            use std::io::Write;
            writeln!(std::io::stderr().lock(), "poema-checkpoint-timing scope={} phase={} elapsed_ms={}", self.scope, phase, self.began.elapsed().as_millis())
                .map_err(failure)?;
        }
        Ok(())
    }
}
// Effect-free composition only. No directory/file publisher receives `inspect`.
// The final full source fence also runs when any original read refuses.
fn observe<T>(check: &impl Fn() -> io::Result<()>, read: impl FnOnce() -> io::Result<T>) -> io::Result<T> {
    observe_with_fence(check, check, read)
}
fn observe_with_fence<T>(check: &impl Fn() -> io::Result<()>, after_read: &impl Fn() -> io::Result<()>, read: impl FnOnce() -> io::Result<T>) -> io::Result<T> {
    check()?;
    let observed = read();
    after_read()?;
    observed
}
fn owned_frame(
    owner: &OwnedDirectory,
    check: &impl Fn() -> io::Result<()>,
    after_read: &impl Fn() -> io::Result<()>,
    inspect: &impl Fn() -> io::Result<()>,
) -> io::Result<Arc<DirectoryFrame>> {
    observe_with_fence(check, after_read, || {
        owner.current(inspect)?;
        let frame = DirectoryFrame::capture(&owner.pin().path, inspect)?;
        if !frame.exists() || frame.record().anchor != *owner.pin() {
            return Err(io::Error::other("original owned checkpoint directory changed; no new baseline"));
        }
        owner.current(inspect)?;
        Ok(frame)
    })
}
fn child(
    owner: &OwnedDirectory,
    name: &str,
    check: &impl Fn() -> io::Result<()>,
    after_read: &impl Fn() -> io::Result<()>,
    inspect: &impl Fn() -> io::Result<()>,
) -> io::Result<OwnedDirectory> {
    let staged = observe_with_fence(check, after_read, || {
        let parent = owned_frame(owner, inspect, inspect, inspect)?;
        let frame = DirectoryFrame::capture(&parent.record().path.join(name), inspect)?;
        if frame.exists() || frame.record().anchor != *owner.pin() || frame.record().missing.len() != 1 {
            return Err(io::Error::other("checkpoint child must remain absent in the actual owned parent"));
        }
        directory_effect::stage(frame, inspect)
    })?;
    staged.publish_owned(check)
}
// The worker independently checks the complete source before every effect.
// Repeated controller-only disk observations every 10ms can contend with that
// worker. Keep all original controller checks, but allow a bounded reply wait.
// The remaining original deadline always wins, including sub-millisecond tails.
fn controller_reply_wait(elapsed: Duration) -> Result<Duration, String> {
    Duration::from_secs(45)
        .checked_sub(elapsed)
        .filter(|remaining| !remaining.is_zero())
        .map(|remaining| remaining.min(Duration::from_millis(100)))
        .ok_or_else(|| "original checkpoint publication exceeded its bounded wait".to_owned())
}
// A controller-only wait cannot acknowledge a result until the original full
// source observer has finished, including after a failed wait. Neither error
// may hide the other, and dropping a returned owner never makes it replayable.
fn finish_controller_wait<T>(observed: Result<T, String>, current: &impl Fn() -> Result<(), String>) -> Result<T, String> {
    match (observed, current()) {
        (Ok(value), Ok(())) => Ok(value),
        (Err(primary), Ok(())) => Err(primary),
        (Ok(_), Err(fence)) => Err(fence),
        (Err(primary), Err(fence)) => Err(format!("{primary}; original checkpoint final source observation also failed: {fence}")),
    }
}
struct ControllerObservation<'a> {
    complete: &'a dyn Fn() -> Result<(), String>,
    wait: Option<&'a dyn Fn() -> Result<(), String>>,
}
impl Persistence {
    /// Private composition only. The actual Cold owner supplies its original
    /// retained source witness and COW guard; neither callback grants a worker.
    pub(crate) fn publish_held_current_checkpoint(
        &self,
        encoded: Encoded,
        epoch: &str,
        root: Arc<DirectoryFrame>,
        reserve: u64,
        source: Source,
        current: impl Fn() -> Result<(), String>,
    ) -> Result<Published, String> {
        self.publish_held_checkpoint_observed(encoded, epoch, root, reserve, source, ControllerObservation { complete: &current, wait: None })
    }
    /// Private original Cold composition. Only the effect-free reply wait uses
    /// the actual caller's COW/canonical/Stop observation. All worker writes and
    /// all complete publication/readback boundaries retain their full guards.
    #[allow(clippy::too_many_arguments)] // The two observers have deliberately distinct semantics.
    pub(crate) fn publish_held_current_checkpoint_with_wait(
        &self,
        encoded: Encoded,
        epoch: &str,
        root: Arc<DirectoryFrame>,
        reserve: u64,
        source: Source,
        current: &impl Fn() -> Result<(), String>,
        wait: &impl Fn() -> Result<(), String>,
    ) -> Result<Published, String> {
        self.publish_held_checkpoint_observed(encoded, epoch, root, reserve, source, ControllerObservation { complete: current, wait: Some(wait) })
    }
    fn publish_held_checkpoint_observed(
        &self,
        encoded: Encoded,
        epoch: &str,
        root: Arc<DirectoryFrame>,
        reserve: u64,
        source: Source,
        observation: ControllerObservation<'_>,
    ) -> Result<Published, String> {
        let current = || (observation.complete)();
        let timing = Timing::new("controller");
        timing.phase("entered")?;
        encoded.current(self)?;
        current()?;
        source.current().map_err(failure)?;
        timing.phase("source-preflight-begin")?;
        // Pure preflight only: retain full original COW/canonical/source
        // checks on both sides. Recursive journal/frame/space observations
        // keep the original held-worker liveness and bounded read guard here;
        // no directory/file effect or startup gate send occurs in this pass.
        let preflight_began = super::super::Instant::now();
        let inspect = || {
            self.require_held_checkpoint_worker().map_err(io::Error::other)?;
            if preflight_began.elapsed() >= Duration::from_secs(45) {
                return Err(io::Error::other("original checkpoint preflight exceeded its bounded read"));
            }
            Ok(())
        };
        let observed = (|| -> Result<Arc<DirectoryFrame>, String> {
            let data = encoded.retained_data_frame(self, &inspect)?;
            let expected = data.record().path.join(format!(".poema-checkpoint-{epoch}"));
            let record = root.record();
            if epoch.len() != 32
                || !epoch.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
                || reserve == 0
                || record.path != expected
                || record.anchor != data.record().anchor
                || record.missing != [expected]
                || root.exists()
            {
                return Err("the exact original absent archive and actual native epoch are required".into());
            }
            root.current(&inspect).map_err(failure)?;
            let space = data.space(&inspect).map_err(failure)?;
            let unit = space.allocation_unit;
            let round = |n| filmcraft_format::retained::preview_context::allocation(n, unit).map_err(failure);
            let mut allowance = round(4096)?.checked_mul(3).ok_or("checkpoint directory allowance overflow")?;
            for (_, bytes) in encoded.files(self)? {
                let n = u64::try_from(bytes.len()).map_err(failure)?;
                if n == 0 {
                    return Err("complete original checkpoint files must be nonempty".into());
                }
                let controls = round(65536)?.checked_mul(8).ok_or("checkpoint control allowance overflow")?;
                let cost = round(n)?.checked_mul(2).and_then(|v| v.checked_add(controls)).ok_or("checkpoint allowance overflow")?;
                allowance = allowance.checked_add(cost).ok_or("complete checkpoint allowance overflow")?;
            }
            if space.read_only
                || space.device != data.record().anchor.identity.device
                || space.free < reserve.checked_add(allowance).ok_or("checkpoint reserve overflow")?
            {
                return Err("the complete original checkpoint would consume its working reserve".into());
            }
            inspect().map_err(failure)?;
            Ok(data)
        })();
        current()?;
        source.fence_after_read().map_err(failure)?;
        let data = observed?;
        timing.phase("preflight-complete")?;
        self.startup_checkpoint_publication_attempted
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .map_err(|_| "original checkpoint publication already attempted; no replay".to_owned())?;
        let aborted = Arc::new(AtomicBool::new(false));
        let _abort = AbortOnDrop(aborted.clone());
        let input = Input { encoded, data, root, epoch: epoch.to_owned(), reserve, source, aborted, began: super::super::Instant::now() };
        let (tx, rx) = channel();
        self.startup_gate
            .as_ref()
            .ok_or("original startup gate disappeared")?
            .send(StartupControl::PublishCheckpoint(Box::new(input), tx))
            .map_err(|_| "original worker ended before checkpoint publication".to_owned())?;
        timing.phase("worker-request-sent")?;
        let began = super::super::Instant::now();
        let observed = (|| {
            loop {
                self.require_held_checkpoint_worker()?;
                match observation.wait {
                    Some(wait) => wait()?,
                    None => current()?,
                }
                let wait = controller_reply_wait(began.elapsed())?;
                match rx.recv_timeout(wait) {
                    Ok(value) => {
                        timing.phase("worker-reply-received")?;
                        return value;
                    }
                    Err(RecvTimeoutError::Timeout) => {}
                    Err(RecvTimeoutError::Disconnected) => return Err("original checkpoint publication acknowledgement lost; no replay".into()),
                }
            }
        })();
        let value = if observation.wait.is_some() { finish_controller_wait(observed, &current)? } else { observed? };
        value.encoded.received(self)?;
        current()?;
        let inspection = super::super::Instant::now();
        value.current(self, &|| {
            if inspection.elapsed() >= Duration::from_secs(45) {
                Err(io::Error::other("original checkpoint readback exceeded its bounded inspection"))
            } else {
                Ok(())
            }
        })?;
        current()?;
        timing.phase("complete")?;
        self.startup_checkpoint_publication_ready.store(true, Ordering::Release);
        Ok(value)
    }
}
impl Worker {
    pub(in crate::autosave) fn publish_held_checkpoint(
        &self,
        input: Input,
        owner: &Arc<()>,
        journal: &Arc<super::super::current_startup::Journal>,
    ) -> Result<Published, String> {
        let encoded = &input.encoded;
        if !Arc::ptr_eq(owner, &encoded.owner)
            || !Arc::ptr_eq(journal, &encoded.journal)
            || encoded.thread != std::thread::current().id()
            || encoded.data != self.data_dir
            || encoded.session != self.session_dir
        {
            return Err("only the SAME original encoding worker can publish these bytes".into());
        }
        let timing = Timing::new("worker");
        timing.phase("entered")?;
        let status = || {
            if input.aborted.load(Ordering::Acquire) || input.began.elapsed() >= Duration::from_secs(45) {
                Err(io::Error::other("original checkpoint publication stopped or expired"))
            } else {
                Ok(())
            }
        };
        let lengths = encoded_lengths(encoded);
        // Identical effect-free data/journal/storage/namespace checks on both
        // paths. Only `check` can serve as a writer callback: it refreshes the
        // complete actual source before these observations every time.
        let metadata = || {
            status()?;
            input.data.current(&status)?;
            journal.current(&status)?;
            let space = input.data.space(&status)?;
            if space.read_only || space.device != input.data.record().anchor.identity.device || space.free < input.reserve {
                return Err(io::Error::other("original checkpoint volume or reserve changed; no fallback"));
            }
            namespace::current(&input.data, &input.root.record().path, &input.epoch, &lengths, &status)?;
            status()
        };
        let check = || {
            status()?;
            let observed = source_read(input.source.as_ref(), &metadata);
            status()?;
            observed
        };
        let after_read = || {
            status()?;
            let observed = metadata();
            input.source.fence_after_read()?;
            status()?;
            observed
        };
        let sdk_source = retained::ReadSource::new(&check, &after_read);
        check().map_err(failure)?;
        timing.phase("source-namespace-checked")?;
        // No effect in this original worker preflight. Fresh full source and
        // namespace fences surround every original journal/frame/decoder read;
        // abort/deadline guards remain inside. Never reuse these observations
        // for a kernel effect: all publishers below still call full `check`.
        let observed = (|| -> Result<(), String> {
            if !Arc::ptr_eq(&journal.retained_data_frame(&status).map_err(failure)?, &input.data) {
                return Err("checkpoint parent is not the original constructor frame".into());
            }
            input.root.current(&status).map_err(failure)?;
            crate::agent_bridge::validate_held_checkpoint_bytes(&encoded.native, &encoded.snapshot).map_err(failure)?;
            let metadata: super::super::JournalMeta = serde_json::from_slice(&encoded.metadata).map_err(failure)?;
            if metadata.pid != std::process::id()
                || metadata.revision != encoded.revision
                || metadata.project_path != encoded.path
                || metadata.bytes != encoded.snapshot.len()
                || metadata.clean_exit
                || metadata.generator != filmcraft_format::generator()
            {
                return Err("original checkpoint metadata crossed its actual worker and encoded current image".into());
            }
            status().map_err(failure)
        })();
        check().map_err(failure)?;
        observed?;
        timing.phase("preflight-complete")?;
        let root = directory_effect::stage(input.root.clone(), &check).and_then(|s| s.publish_owned(&check)).map_err(failure)?;
        timing.phase("archive-created")?;
        let files = child(&root, "files", &check, &after_read, &status).map_err(failure)?;
        let writes = child(&root, "writes", &check, &after_read, &status).map_err(failure)?;
        let files_frame = owned_frame(&files, &check, &after_read, &status).map_err(failure)?;
        let writes_frame = owned_frame(&writes, &check, &after_read, &status).map_err(failure)?;
        timing.phase("child-directories-created")?;
        let mut outputs = Vec::new();
        let mut publications = Vec::new();
        let mut intents = Vec::new();
        for (index, (name, bytes)) in encoded_slices(encoded).into_iter().enumerate() {
            check().map_err(failure)?;
            let destination = observe_with_fence(&check, &after_read, || files_frame.node_bounded(name, bytes.len() as u64, &status)).map_err(failure)?;
            if !matches!(destination.state(), NodeState::Absent) {
                return Err("foreign checkpoint target cannot be adopted".into());
            }
            let suffix = ["snapshot", "metadata", "native"].get(index).ok_or("original checkpoint role missing")?;
            let operation = format!("{}_{}", input.epoch, suffix);
            let staged = retained::stage_private_from_frames(&destination, &writes_frame, &operation, bytes, input.reserve, &sdk_source).map_err(failure)?;
            let intent = staged.intent().clone();
            let publication = staged.publish(&sdk_source);
            if !publication.receipt_durable
                || publication.receipt.outcome != Outcome::Published
                || !publication.receipt.attempted
                || !publication.receipt.directories_synced
            {
                return Err("original checkpoint file durability is unresolved; retain all effects without replay".into());
            }
            let output = observe_with_fence(&check, &after_read, || {
                publication.controls_current(&status)?;
                let output = files_frame.node_bounded(name, bytes.len() as u64, &status)?;
                if publication.receipt.output.as_ref().is_none_or(|expected| output.state() != &NodeState::File(expected.clone()))
                    || output.read_private_bytes(bytes.len() as u64, &status)? != bytes
                {
                    return Err(io::Error::other("checkpoint readback differs from its actual original writer receipt or complete bytes"));
                }
                Ok(output)
            })
            .map_err(failure)?;
            outputs.push(output);
            publications.push(publication);
            intents.push(intent);
            timing.phase("file-published-and-read")?;
        }
        check().map_err(failure)?;
        let result = Published {
            encoded: input.encoded,
            source: input.source.clone(),
            data: input.data.clone(),
            epoch: input.epoch.clone(),
            root,
            files,
            writes,
            outputs,
            publications,
            intents,
        };
        // Complete receipt/payload readback is effect-free too. Keep the same
        // full source fences on both sides, even when the inspection fails.
        let observed = result.physical_current(&status);
        check().map_err(failure)?;
        observed.map_err(failure)?;
        timing.phase("complete")?;
        Ok(result)
    }
}
fn encoded_slices(encoded: &Encoded) -> [(&'static str, &[u8]); 3] {
    [(super::super::SNAPSHOT, &encoded.snapshot), (super::super::META, &encoded.metadata), (crate::agent_bridge::NATIVE_JOURNAL_FILE, &encoded.native)]
}
fn encoded_lengths(encoded: &Encoded) -> [u64; 3] {
    [encoded.snapshot.len() as u64, encoded.metadata.len() as u64, encoded.native.len() as u64]
}
impl Published {
    fn physical_current(&self, check: &impl Fn() -> io::Result<()>) -> io::Result<()> {
        self.root.current(check)?;
        self.files.current(check)?;
        self.writes.current(check)?;
        let original = self.encoded.journal.retained_data_frame(check)?;
        self.physical_current_with_original_data(&original, check)
    }
    // Complete original archive/receipt/payload reader shared by two distinct
    // callers. The held entry above still obtains its data through the strict
    // gated Journal. ReleasePreparation retains that SAME frame before the
    // once-only release attempt; neither path captures a replacement baseline.
    fn physical_current_with_original_data(&self, original: &Arc<DirectoryFrame>, check: &impl Fn() -> io::Result<()>) -> io::Result<()> {
        if !Arc::ptr_eq(original, &self.data) {
            return Err(io::Error::other("original checkpoint data frame was replaced"));
        }
        namespace::current(&self.data, &self.root.pin().path, &self.epoch, &encoded_lengths(&self.encoded), check)?;
        if self.outputs.len() != 3 || self.publications.len() != 3 || self.intents.len() != 3 {
            return Err(io::Error::other("three actual original checkpoint receipts required"));
        }
        let mut read = Vec::new();
        for (((output, publication), (_, expected)), intent) in
            self.outputs.iter().zip(&self.publications).zip(encoded_slices(&self.encoded)).zip(&self.intents)
        {
            let inspection = retained::inspect_publication_after_directory_seals(publication, intent, &[], check)?;
            if inspection.current != retained::CurrentState::Published
                || !inspection.dispatch
                || inspection.slot.is_some()
                || inspection.recorded_receipt.as_ref() != Some(&publication.receipt)
                || inspection.output != publication.receipt.output
            {
                return Err(io::Error::other("original checkpoint publication receipt or independent archive payload changed"));
            }
            output.current(check)?;
            output.fence(check)?;
            let bytes = output.read_private_bytes(expected.len() as u64, check)?;
            if bytes != expected || publication.receipt.output.as_ref().is_none_or(|p| output.state() != &NodeState::File(p.clone())) {
                return Err(io::Error::other("original checkpoint output or receipt changed"));
            }
            read.push(bytes);
        }
        let snapshot = read.first().ok_or_else(|| io::Error::other("original snapshot readback missing"))?;
        let native = read.get(2).ok_or_else(|| io::Error::other("original native readback missing"))?;
        crate::agent_bridge::validate_held_checkpoint_bytes(native, snapshot).map_err(io::Error::other)?;
        namespace::current(&self.data, &self.root.pin().path, &self.epoch, &encoded_lengths(&self.encoded), check)?;
        self.root.current(check)?;
        self.files.current(check)?;
        self.writes.current(check)?;
        check()
    }
    pub(crate) fn current(&self, worker: &Persistence, check: &impl Fn() -> io::Result<()>) -> Result<(), String> {
        self.encoded.received(worker)?;
        check().map_err(failure)?;
        // Complete source first, same retained source fence last, including on
        // failed reads. No kernel write occurs inside this observation.
        let observed = source_read(self.source.as_ref(), || self.physical_current(check));
        check().map_err(failure)?;
        observed.map_err(failure)?;
        self.encoded.received(worker)
    }
    pub(crate) fn encoded(&self) -> &Encoded {
        &self.encoded
    }
    pub(crate) fn description(&self, worker: &Persistence) -> Result<Value, String> {
        self.current(worker, &|| Ok(()))?;
        Ok(json!({"kind":"original-gated-worker-checkpoint-publication","version":1,"nativePid":std::process::id(),
            "archive":self.root.pin(),"filesDirectory":self.files.pin(),"writesDirectory":self.writes.pin(),
            "files":self.outputs.iter().zip(&self.publications).map(|(o,p)|json!({"path":o.path(),"receipt":p.receipt,"receiptDurable":p.receipt_durable,"controls":p.control_observations()})).collect::<Vec<_>>(),
            "sameOriginalWorker":true,"filesWritten":true,"durable":true,"completeAuxiliaryReadback":true,
            "independentArchivePayloadsReverified":true,"completeArchiveNamespaceChecked":true,
            "startupPending":true,"workerReleased":false,"writeAuthority":false,"startupAuthority":false,"fullApplicationClosure":false}))
    }
}
#[cfg(test)]
mod tests;
