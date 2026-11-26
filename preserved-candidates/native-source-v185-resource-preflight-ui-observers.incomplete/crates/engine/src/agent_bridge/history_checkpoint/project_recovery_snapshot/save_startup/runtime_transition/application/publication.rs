//! Same original Cold controller -> same original gated checkpoint worker.
//! These Arcs retain actual witnesses, never serialized substitute authority.
use super::*;
mod release;
mod settled;

pub(super) struct SourceWitness {
    original: Arc<Input>,
    successor: std::sync::Mutex<JournalContinuation>,
    source: Arc<Source>,
    root: Arc<root::Root>,
    records: Arc<records::Records>,
    canonical: Arc<workspace_update::ColdCommit>,
}
impl SourceWitness {
    /// Same original held source, borrowing this operation's Stop/deadline.
    pub(super) fn inspect_held_source(&self, check: &media_guard::Check) -> Result<()> {
        check.check()?;
        let io = || check.check().map_err(|e| io::Error::other(e.to_string()));
        with_original_successor(&self.successor, |successor| {
            self.read_current_at_with_canonical(successor, check, &|| self.canonical.verify_published_source(&io))
        })
    }
    pub(super) fn fence_held_source(&self, check: &media_guard::Check) -> Result<()> {
        check.check()?;
        let io = || check.check().map_err(|e| io::Error::other(e.to_string()));
        with_original_successor(&self.successor, |successor| {
            self.fence_at_with_canonical(successor, check, &|| self.canonical.fence_published_source_after_read(&io))
        })
    }
    fn current(&self) -> Result<()> {
        let _timing = SourceReadTiming::new("complete-with-lock");
        with_original_successor(&self.successor, |successor| self.current_at(successor))
    }
    fn fence_after_read(&self) -> Result<()> {
        let _timing = SourceReadTiming::new("final-fence");
        with_original_successor(&self.successor, |successor| {
            let check = media_guard::Check::new(web_time::Instant::now());
            let io = || check.check().map_err(|e| io::Error::other(e.to_string()));
            self.fence_at_with_canonical(successor, &check, &|| self.canonical.fence_published_source_after_read(&io))
        })
    }
    fn fence_at_with_canonical(&self, successor: &JournalContinuation, check: &media_guard::Check, canonical: &impl Fn() -> Result<()>) -> Result<()> {
        self.source.fence_after_read(check)?;
        self.records.checkpoint_ready()?;
        self.records.fence_after_read(check)?;
        let preference = self.canonical.preference();
        let manifest = self.root.acknowledged_manifest_after_read(check)?;
        self.original.fence_application_after_read(check, Some(successor), Some(&preference), Some(manifest))?;
        canonical()?;
        self.records.fence_after_read(check)?;
        self.root.fence_after_read(check)?;
        self.source.fence_after_read(check)?;
        check.check()
    }
    fn advance(&self, previous: &JournalContinuation, next: JournalContinuation) -> Result<()> {
        {
            let successor = self.successor.lock().unwrap_or_else(|e| e.into_inner());
            if !successor.same_original(previous) {
                return Err(error("the checkpoint source continuation crossed its retained original journal Controls"));
            }
        }
        // next contains the actual Control Arcs returned by the original
        // successor validator, never a newly matching descriptive baseline.
        // Validate outside the shared mutex, then compare the SAME previous
        // Arcs again while installing the actual acknowledged successor.
        self.current_at(&next)?;
        replace_original_successor(&self.successor, previous, next)
    }
    pub(super) fn advance_with_check(&self, previous: &JournalContinuation, next: JournalContinuation, check: &media_guard::Check) -> Result<()> {
        if !self.successor.lock().unwrap_or_else(|e| e.into_inner()).same_original(previous) {
            return Err(error("the SAME original pre-Resume journal predecessor changed"));
        }
        let io = || check.check().map_err(|e| io::Error::other(e.to_string()));
        self.read_current_at_with_canonical(&next, check, &|| self.canonical.verify_published_source(&io))?;
        check.check()?;
        replace_original_successor(&self.successor, previous, next)
    }
    fn current_at(&self, successor: &JournalContinuation) -> Result<()> {
        let _timing = SourceReadTiming::new("complete");
        // Preserve the original held witness clock and strict canonical reader.
        let check = media_guard::Check::new(web_time::Instant::now());
        let io = || check.check().map_err(|e| io::Error::other(e.to_string()));
        self.read_current_at_with_canonical(successor, &check, &|| self.canonical.verify_published_source(&io))
    }
    fn read_current_at_with_canonical(&self, successor: &JournalContinuation, check: &media_guard::Check, canonical: &impl Fn() -> Result<()>) -> Result<()> {
        self.source.current(check)?;
        // No effect in this source query. Reuse only this Root's original
        // acknowledged Control within one fully read, finally fenced scope.
        // Every caller's actual writer still invokes the full source query.
        self.root.observe_current(check, |manifest| {
            let observed = (|| {
                self.records.checkpoint_ready()?;
                self.records.current(check)?;
                let preference = self.canonical.preference();
                self.original.current_with_application(check, Some(successor), Some(&preference), Some(manifest))?;
                canonical()?;
                self.records.fence_after_read(check)
            })();
            // Keep the real source/PID/control fence after a failed read too.
            self.source.fence_after_read(check)?;
            check.check()?;
            observed
        })
    }
}
// The main controller and original worker both read this same witness. Keep
// only the existing Control Arcs for one query; serializing their full IO under
// this mutex can starve the other thread. Every read and every writer's fresh
// complete source query still runs. Even failed reads finish with the original
// continuation identity check, and no observation is cached across effects.
fn with_original_successor<T>(successor: &std::sync::Mutex<JournalContinuation>, read: impl FnOnce(&JournalContinuation) -> Result<T>) -> Result<T> {
    let original = successor.lock().unwrap_or_else(|e| e.into_inner()).clone();
    let observed = read(&original);
    if !successor.lock().unwrap_or_else(|e| e.into_inner()).same_original(&original) {
        return Err(error("the original checkpoint source continuation changed during its query"));
    }
    observed
}
fn replace_original_successor(successor: &std::sync::Mutex<JournalContinuation>, previous: &JournalContinuation, next: JournalContinuation) -> Result<()> {
    let mut retained = successor.lock().unwrap_or_else(|e| e.into_inner());
    if !retained.same_original(previous) {
        return Err(error("the checkpoint source continuation crossed its retained original journal Controls"));
    }
    *retained = next;
    Ok(())
}
// Diagnostics only, using the existing exact opt-in. No source observation or
// result is retained here, and no diagnostic can supply a guard or permission.
struct SourceReadTiming {
    kind: &'static str,
    began: Option<web_time::Instant>,
}
impl SourceReadTiming {
    fn new(kind: &'static str) -> Self {
        Self { kind, began: (std::env::var("POEMA_NATIVE_CHECKPOINT_TIMING").ok().as_deref() == Some("1")).then(web_time::Instant::now) }
    }
    fn write_record(writer: &mut impl io::Write, kind: &'static str, elapsed_us: u128) {
        // A closed diagnostic pipe must not panic during a source error or
        // unwinding. This best-effort record cannot change the actual guard.
        let _ = io::Write::write_fmt(writer, format_args!("poema-source-read-timing kind={kind} elapsed_us={elapsed_us}\n"));
    }
}
impl Drop for SourceReadTiming {
    fn drop(&mut self) {
        if let Some(began) = self.began {
            Self::write_record(&mut io::stderr().lock(), self.kind, began.elapsed().as_micros());
        }
    }
}
impl crate::autosave::held_checkpoint::publication::CheckpointSource for SourceWitness {
    fn current(&self) -> io::Result<()> {
        SourceWitness::current(self).map_err(|e| io::Error::other(e.to_string()))
    }
    fn fence_after_read(&self) -> io::Result<()> {
        SourceWitness::fence_after_read(self).map_err(|e| io::Error::other(e.to_string()))
    }
}
impl Pending {
    pub(in crate::agent_bridge::history_checkpoint::project_recovery_snapshot::save_startup::runtime_transition) fn confirm_checkpoint_publication_successor(
        &mut self,
        s: &Session,
        value: &Value,
    ) -> Result<Value> {
        let event =
            self.application.checkpoint.as_mut().ok_or_else(|| error("the actual retained checkpoint is required"))?.begin_publication_observation(s)?;
        bytes(value, INPUT)?;
        let receipt: IntentReceipt = parse("native.coldSaveCheckpointPublication", value)?;
        if receipt.kind != "poema-original-Save-native-checkpoint-archive-observation-receipt" || receipt.version != 1 || receipt.event != event {
            return Err(error("the sole original checkpoint observation must name the exact actual publication"));
        }
        let previous = self.successor.clone().ok_or_else(|| error("the original checkpoint journal predecessor is missing"))?;
        let check = media_guard::Check::new(web_time::Instant::now());
        let next = self.read_published_successor(&receipt.anchor, &receipt.head, event.clone(), &check)?;
        self.application
            .publication_source
            .as_ref()
            .ok_or_else(|| error("the same original typed checkpoint source witness is required"))?
            .advance(&previous, next.clone())?;
        self.successor = Some(next);
        self.worker_current(s.persistence.as_ref())?;
        let canonical = self.canonical.as_ref().ok_or_else(|| error("the actual original canonical owner disappeared"))?;
        canonical.commit.verify_published(s, &|| self.source_current().map_err(|e| io::Error::other(e.to_string())))?;
        self.application.checkpoint.as_mut().ok_or_else(|| error("the actual original checkpoint disappeared"))?.confirm_publication_observation(s)?;
        self.current()?;
        self.check_canonical(s)?;
        Ok(json!({"kind":"native_cold_Save_checkpoint_archive_publication_confirmation","version":1,
            "nativePid":std::process::id(),"nativeSession":self.preparation["nativeSession"],"event":event,"anchor":receipt.anchor,"head":receipt.head,
            "nativeAcknowledgementConfirmed":true,"editingPaused":true,"archiveCreated":true,"filesWritten":true,"workerReleased":false,
            "writeAuthority":false,"startupAuthority":false,"workersMayResume":false,"fullApplicationClosure":false}))
    }
    pub(in crate::agent_bridge::history_checkpoint::project_recovery_snapshot::save_startup::runtime_transition) fn publish_application_checkpoint_files(
        &mut self,
        s: &Session,
        request: &Value,
    ) -> Result<Value> {
        bytes(request, 16384)?;
        if !fields(request, "kind,version,nativeSession,privateInput")
            || request["kind"] != "poema-original-Save-native-application-checkpoint-archive-publish"
            || request["version"] != 1
            || request["nativeSession"] != self.preparation["nativeSession"]
            || request["privateInput"] != self.preparation["privateInput"]
        {
            return Err(error("the same closed original checkpoint publication request and private native pipe are required"));
        }
        if self.application.publication_source.is_some() {
            return Err(error("the same original controller publication request was already attempted; no replay"));
        }
        self.current()?;
        self.worker_current(s.persistence.as_ref())?;
        self.check_canonical(s)?;
        if !self.application.confirmed {
            return Err(error("the actual original application intent must remain acknowledged"));
        }
        let canonical = self.canonical.as_ref().ok_or_else(|| error("the actual original canonical owner is required"))?.retained_published()?;
        let witness = Arc::new(SourceWitness {
            original: self.original.clone(),
            successor: std::sync::Mutex::new(self.successor.clone().ok_or_else(|| error("the SAME acknowledged original journal successor is required"))?),
            source: self.application.source.clone().ok_or_else(|| error("the original running source controls are required"))?,
            root: self.application.root.clone().ok_or_else(|| error("the original acknowledged Root is required"))?,
            records: self.application.records.clone().ok_or_else(|| error("the original acknowledged native records are required"))?,
            canonical: canonical.clone(),
        });
        witness.current()?;
        self.application.publication_source = Some(witness.clone());
        // No Prepared::resolved call while its consumed publication is pending.
        // The original Guard/COW graph and all live canonical service-memory
        // checks remain on this controller thread throughout the worker wait.
        // One actual operation guard serves the full controller source reads
        // AND its bounded wait observations. Never fork a fresh unchecked Stop
        // token for the reduced wait. The worker's full source stays unchanged.
        let read_check = media_guard::Check::new(web_time::Instant::now());
        let complete = || witness.inspect_held_source(&read_check).map_err(|e| io::Error::other(e.to_string()));
        let after_read = || witness.fence_held_source(&read_check).map_err(|e| io::Error::other(e.to_string()));
        let live = || canonical.inspect_published_source_read(s, &filmcraft_format::retained::ReadSource::new(&complete, &after_read));
        let wait = || canonical.inspect_checkpoint_wait(s, &read_check);
        let source: crate::autosave::held_checkpoint::publication::Source = witness.clone();
        let result = self
            .application
            .checkpoint
            .as_mut()
            .ok_or_else(|| error("the original complete checkpoint is required"))?
            .publish_with_wait(s, source, &live, &wait)?;
        self.current()?;
        self.worker_current(s.persistence.as_ref())?;
        self.check_canonical(s)?;
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        io::Write,
        os::unix::fs::{DirBuilderExt, OpenOptionsExt},
        sync::Mutex,
    };

    #[test]
    fn original_source_diagnostic_write_failure_does_not_panic_or_change_line_encoding() {
        let mut bytes = Vec::new();
        SourceReadTiming::write_record(&mut bytes, "complete", 123);
        assert_eq!(bytes, b"poema-source-read-timing kind=complete elapsed_us=123\n");
        struct BrokenPipe;
        impl io::Write for BrokenPipe {
            fn write(&mut self, _: &[u8]) -> io::Result<usize> {
                Err(io::Error::new(io::ErrorKind::BrokenPipe, "controlled diagnostic pipe closure"))
            }
            fn flush(&mut self) -> io::Result<()> {
                Err(io::Error::new(io::ErrorKind::BrokenPipe, "controlled diagnostic pipe closure"))
            }
        }
        assert!(std::panic::catch_unwind(|| SourceReadTiming::write_record(&mut BrokenPipe, "final-fence", 456)).is_ok());
    }
    fn control(root: &Path, name: &str) -> Arc<Control> {
        Arc::new(Control::capture(&root.join(name), 4096, &media_guard::Check::new(web_time::Instant::now())).unwrap())
    }
    fn fixture() -> (PathBuf, JournalContinuation) {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let root = crate::temp_dir().join(format!(
            "poema-original-continuation-read-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        std::fs::DirBuilder::new().mode(0o700).create(&root).unwrap();
        for name in ["anchor.json", "head.json", "page.json"] {
            let mut f = std::fs::OpenOptions::new().create_new(true).write(true).mode(0o600).open(root.join(name)).unwrap();
            f.write_all(b"{\"generatedReadOnlyControl\":true}").unwrap();
            f.sync_all().unwrap();
        }
        let continuation =
            JournalContinuation { anchor: control(&root, "anchor.json"), head: control(&root, "head.json"), pages: vec![control(&root, "page.json")] };
        (root, continuation)
    }
    #[test]
    fn original_continuation_queries_read_concurrently_without_holding_the_source_mutex() {
        let (_root, original) = fixture();
        let slot = Arc::new(Mutex::new(original.clone()));
        let worker_slot = slot.clone();
        let (entered, ready) = std::sync::mpsc::sync_channel(1);
        let (release, resume) = std::sync::mpsc::sync_channel(1);
        let worker = std::thread::spawn(move || {
            with_original_successor(&worker_slot, |current| {
                assert!(worker_slot.try_lock().is_ok(), "disk reads must not hold the continuation mutex");
                entered.send(()).map_err(error)?;
                resume.recv_timeout(std::time::Duration::from_secs(2)).map_err(error)?;
                current.current(&media_guard::Check::new(web_time::Instant::now()))
            })
            .is_ok()
        });
        ready.recv_timeout(std::time::Duration::from_secs(2)).unwrap();
        let controller = with_original_successor(&slot, |current| {
            assert!(slot.try_lock().is_ok());
            assert!(current.same_original(&original));
            current.current(&media_guard::Check::new(web_time::Instant::now()))?;
            release.send(()).map_err(error)?;
            Ok(())
        });
        let worker = worker.join().unwrap();
        controller.unwrap();
        assert!(worker);
        assert!(slot.lock().unwrap().same_original(&original));
    }
    #[test]
    fn original_continuation_final_identity_refuses_equal_description_replacements_even_after_read_failure() {
        for failed_read in [false, true] {
            let (root, original) = fixture();
            let slot = Mutex::new(original.clone());
            let mut foreign = original.clone();
            foreign.pages = vec![control(&root, "page.json")];
            assert_eq!(foreign.pages[0].value(), original.pages[0].value());
            let observed = with_original_successor(&slot, |current| {
                assert!(current.same_original(&original));
                *slot.lock().unwrap() = foreign.clone();
                if failed_read { Err(error("controlled failed read")) } else { Ok(()) }
            });
            assert!(observed.unwrap_err().to_string().contains("changed during its query"));
            assert!(slot.lock().unwrap().same_original(&foreign));
        }
    }
    #[test]
    fn original_continuation_install_compares_exact_previous_controls_without_replay_or_value_repinning() {
        let (root, original) = fixture();
        let slot = Mutex::new(original.clone());
        let mut crossed = original.clone();
        crossed.anchor = control(&root, "anchor.json");
        let mut next = original.clone();
        next.head = control(&root, "head.json");
        assert_eq!(crossed.anchor.value(), original.anchor.value());
        assert!(replace_original_successor(&slot, &crossed, next.clone()).is_err());
        assert!(slot.lock().unwrap().same_original(&original));
        replace_original_successor(&slot, &original, next.clone()).unwrap();
        assert!(slot.lock().unwrap().same_original(&next));
        assert!(replace_original_successor(&slot, &original, original.clone()).is_err());
        assert!(slot.lock().unwrap().same_original(&next));
    }
}
