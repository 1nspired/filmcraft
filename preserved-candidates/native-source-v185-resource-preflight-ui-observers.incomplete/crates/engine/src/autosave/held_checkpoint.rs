//! One-use data-only checkpoint encoding on the SAME original gated worker.
//! The ordinary persistence FIFO stays untouched. No file is written and no
//! serialization/receipt description supplies publication or startup authority.
use super::{Arc, Persistence, Project, StartupControl, Worker};
pub(crate) mod publication;
use serde_json::{Value, json};
use std::{
    path::PathBuf,
    sync::{
        atomic::Ordering,
        mpsc::{RecvTimeoutError, channel},
    },
    thread::ThreadId,
    time::Duration,
};

pub(super) struct Input {
    project: Arc<Project>,
    revision: u64,
    path: Option<String>,
    native: crate::agent_bridge::NativeJournalPending,
    journal: Arc<super::current_startup::Journal>,
}
pub(crate) struct Encoded {
    owner: Arc<()>,
    journal: Arc<super::current_startup::Journal>,
    thread: ThreadId,
    data: PathBuf,
    session: PathBuf,
    revision: u64,
    path: Option<String>,
    snapshot: Vec<u8>,
    metadata: Vec<u8>,
    native: Vec<u8>,
}
impl Encoded {
    fn received(&self, worker: &Persistence) -> Result<(), String> {
        worker.require_held_checkpoint_worker()?;
        self.same_worker_identity(worker)
    }
    // Negative process-local identity only. The original received/current
    // methods still require the SAME unsent, unattempted gated worker first.
    // A separately retained release preparation uses this during the actual
    // Prepare/Resume exchange; it never manufactures its acknowledgement.
    fn same_worker_identity(&self, worker: &Persistence) -> Result<(), String> {
        if !Arc::ptr_eq(&self.owner, &worker.startup_identity)
            || worker.current_journal.as_ref().is_none_or(|j| !Arc::ptr_eq(j, &self.journal))
            || worker.thread.as_ref().is_none_or(|h| h.thread().id() != self.thread)
            || self.data != worker.data_dir
            || self.session != worker.session_dir
            || !worker.startup_checkpoint_attempted.load(Ordering::Acquire)
        {
            return Err("encoded checkpoint does not belong to this same original worker and journal".into());
        }
        Ok(())
    }
    pub(crate) fn current(&self, worker: &Persistence) -> Result<(), String> {
        self.received(worker)?;
        if !worker.startup_checkpoint_ready.load(Ordering::Acquire) {
            return Err("original checkpoint serialization is unresolved".into());
        }
        Ok(())
    }
    /// Original read-only parent witness, accessible only while these actual
    /// encoded bytes still belong to this SAME live gated persistence worker.
    pub(crate) fn retained_data_frame(
        &self,
        worker: &Persistence,
        check: &impl Fn() -> std::io::Result<()>,
    ) -> Result<Arc<filmcraft_format::retained::preview_context::DirectoryFrame>, String> {
        self.current(worker)?;
        let data = self.journal.retained_data_frame(check).map_err(|e| e.to_string())?;
        self.current(worker)?;
        Ok(data)
    }
    /// Immutable data only. Caller retains original COW/source/file guards and
    /// actual retained publishers; these bytes never authorize a filesystem effect.
    pub(crate) fn files<'a>(&'a self, worker: &Persistence) -> Result<[(&'static str, &'a [u8]); 3], String> {
        self.current(worker)?;
        Ok([(super::SNAPSHOT, &self.snapshot), (super::META, &self.metadata), (crate::agent_bridge::NATIVE_JOURNAL_FILE, &self.native)])
    }
    pub(crate) fn description(&self, worker: &Persistence) -> Result<Value, String> {
        self.current(worker)?;
        let files = self
            .files(worker)?
            .into_iter()
            .map(|(name, bytes)| json!({"name":name,"bytes":bytes.len(),"sha256":crate::saved_file::sha256(bytes)}))
            .collect::<Vec<_>>();
        Ok(json!({"kind":"original-gated-worker-checkpoint-bytes","version":1,"nativePid":std::process::id(),
            "dataDirectory":self.data,"sessionDirectory":self.session,"revision":self.revision,"projectPath":self.path,"files":files,
            "sameOriginalWorker":true,"serialized":true,"filesWritten":false,"startupPending":true,
            "workerReleased":false,"writeAuthority":false,"nativeComplete":false,"startupAuthority":false,"fullApplicationClosure":false}))
    }
}
impl Persistence {
    fn require_held_checkpoint_worker(&self) -> Result<(), String> {
        if self.startup_gate.is_none()
            || !self.startup_pending()
            || self.startup_attempted.load(Ordering::Acquire)
            || self.last_sent.is_some()
            || self.lock.is_none()
            || self.current_journal.is_none()
            || self.thread.as_ref().is_none_or(|h| h.is_finished())
        {
            return Err("the same live original unsent gated worker is required for checkpoint encoding".into());
        }
        Ok(())
    }
    pub(super) fn require_checkpoint_preparation_resolved(&self) -> Result<(), String> {
        if self.startup_checkpoint_publication_attempted.load(Ordering::Acquire) && !self.startup_checkpoint_publication_ready.load(Ordering::Acquire) {
            return Err("original checkpoint publication is unresolved; keep the gate and evidence without replay".into());
        }
        if self.startup_checkpoint_attempted.load(Ordering::Acquire) && !self.startup_checkpoint_ready.load(Ordering::Acquire) {
            return Err("original checkpoint preparation is unresolved; keep the gate and evidence without replay".into());
        }
        Ok(())
    }
    pub(crate) fn encode_held_current_checkpoint(
        &self,
        project: Arc<Project>,
        revision: u64,
        path: Option<String>,
        native: crate::agent_bridge::NativeJournalPending,
        source: impl Fn() -> Result<(), String>,
    ) -> Result<Encoded, String> {
        self.require_held_checkpoint_worker()?;
        source()?;
        let began = super::Instant::now();
        let inspect = || {
            if began.elapsed() >= Duration::from_secs(45) {
                Err(std::io::Error::other("original checkpoint encoding exceeded its bounded wait"))
            } else {
                Ok(())
            }
        };
        let journal = self.current_image_journal(&inspect).map_err(|e| e.to_string())?;
        source()?;
        self.startup_checkpoint_attempted
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .map_err(|_| "original worker checkpoint preparation already attempted; no replay".to_owned())?;
        let (tx, rx) = channel();
        let gate = self.startup_gate.as_ref().ok_or_else(|| "original startup gate disappeared".to_owned())?;
        gate.send(StartupControl::PrepareCheckpoint(Box::new(Input { project, revision, path, native, journal }), tx))
            .map_err(|_| "original worker ended before checkpoint preparation".to_owned())?;
        loop {
            self.require_held_checkpoint_worker()?;
            inspect().map_err(|e| e.to_string())?;
            source()?;
            match rx.recv_timeout(Duration::from_millis(10)) {
                Ok(result) => {
                    let value = result?;
                    value.received(self)?;
                    source()?;
                    let current = self.current_image_journal(&inspect).map_err(|e| e.to_string())?;
                    if !Arc::ptr_eq(&current, &value.journal) {
                        return Err("original checkpoint journal changed after encoding".into());
                    }
                    source()?;
                    self.require_held_checkpoint_worker()?;
                    self.startup_checkpoint_ready.store(true, Ordering::Release);
                    return Ok(value);
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => return Err("original checkpoint acknowledgement lost; no replay".into()),
            }
        }
    }
}
impl Worker {
    pub(super) fn encode_held_checkpoint(&self, input: Input, owner: Arc<()>, journal: Arc<super::current_startup::Journal>) -> Result<Encoded, String> {
        if !Arc::ptr_eq(&input.journal, &journal) {
            return Err("checkpoint input crossed the original worker journal".into());
        }
        let snapshot = filmcraft_format::encode(&input.project, false);
        let native = input
            .native
            .encode_current_checkpoint(&input.project, &snapshot, input.revision, input.path.as_deref(), &self.data_dir, &self.session_dir)
            .map_err(|e| e.to_string())?;
        let meta = self.journal_metadata(&input.project, input.revision, &input.path, snapshot.len(), super::unix_now());
        let metadata = serde_json::to_vec_pretty(&meta).map_err(|e| format!("original checkpoint metadata: {e}"))?;
        Ok(Encoded {
            owner,
            journal,
            thread: std::thread::current().id(),
            data: self.data_dir.clone(),
            session: self.session_dir.clone(),
            revision: input.revision,
            path: input.path,
            snapshot,
            metadata,
            native,
        })
    }
}
#[cfg(test)]
#[path = "held_checkpoint_tests.rs"]
mod tests;
