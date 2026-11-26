//! One-use cold publication through the original native Commit and services.
//! Preparing observations does not authorize a write. The cold coordinator must
//! first confirm the original host journal's exact canonical intent successor.
use super::*;
use std::sync::Mutex;
use std::thread::{self, ThreadId};
mod bindings;
pub(in crate::agent_bridge) mod release;
pub(crate) use bindings::ServiceBindings;

#[derive(Clone)]
enum PreferencePhase {
    Before,
    Publishing(ThreadId),
    Published(Arc<NodeFrame>),
    Stopped,
}

/// Only the actual original writer can continue this exact preference frame.
/// No serialized observation, status flag or same-byte replacement can mint it.
pub(crate) struct PreferenceContinuation {
    before: Arc<NodeFrame>,
    phase: Mutex<PreferencePhase>,
}
impl PreferenceContinuation {
    pub(crate) fn belongs_to_data(&self, data: &filmcraft_format::retained::preview_context::DirectoryRecord) -> bool {
        self.before.path() == data.path.join("preferences.json")
    }
    pub(crate) fn current_node(&self, original: &NodeFrame, check: &impl Fn() -> io::Result<()>) -> Result<bool> {
        self.verify_node(original, check, true)
    }
    /// Same retained generation after a complete source read; never a fresh
    /// preference owner, arbitrary observation, or standalone publication check.
    pub(crate) fn fence_node_after_read(&self, original: &NodeFrame, check: &impl Fn() -> io::Result<()>) -> Result<bool> {
        self.verify_node(original, check, false)
    }
    fn verify_node(&self, original: &NodeFrame, check: &impl Fn() -> io::Result<()>, read_bytes: bool) -> Result<bool> {
        if original.path() != self.before.path() {
            return Ok(false);
        }
        if original.state() != self.before.state() {
            return Err(fail("cold preference continuation does not belong to the retained original frame"));
        }
        // Never call a caller-supplied guard while holding the phase mutex:
        // the original input fence can re-enter this same continuation.
        let phase = self.phase.lock().unwrap_or_else(|e| e.into_inner()).clone();
        match &phase {
            PreferencePhase::Before => {
                if read_bytes {
                    self.before.current(check).map_err(fail)?;
                }
                self.before.fence(check).map_err(fail)?;
            }
            PreferencePhase::Publishing(owner) if *owner == thread::current().id() => {
                // This exact original Commit is on this thread's stack. It
                // fences the displaced/after preference using its retained
                // staged owner and real publication receipt at every boundary.
                check().map_err(fail)?;
            }
            PreferencePhase::Published(after) => {
                if read_bytes {
                    after.current(check).map_err(fail)?;
                }
                after.fence(check).map_err(fail)?;
            }
            _ => return Err(fail("cold preference publication is unresolved or belongs to another thread; no continuation")),
        }
        Ok(true)
    }
}

// Unwinding a trusted callback must never leave an in-flight preference
// delegation behind. The actual consuming writer still cannot be replayed.
struct PublishingPreference(Arc<PreferenceContinuation>);
impl Drop for PublishingPreference {
    fn drop(&mut self) {
        let mut phase = self.0.phase.lock().unwrap_or_else(|e| e.into_inner());
        if matches!(*phase, PreferencePhase::Publishing(_)) {
            *phase = PreferencePhase::Stopped;
        }
    }
}
/// A bounded READ ONLY observation of the same actual canonical owner.
/// The complete caller/source fence surrounds every pass; all original file,
/// directory, receipt, memory and worker observations still run inside it.
/// Never use this bracket for staging, publication, binding or worker release.
fn inspect_read<T>(source: &impl Fn() -> io::Result<()>, inspect: &impl Fn() -> io::Result<()>, read: impl FnOnce() -> Result<T>) -> Result<T> {
    inspect_read_with_fence(source, source, inspect, read)
}
fn inspect_read_with_fence<T>(
    source: &impl Fn() -> io::Result<()>,
    after_read: &impl Fn() -> io::Result<()>,
    inspect: &impl Fn() -> io::Result<()>,
    read: impl FnOnce() -> Result<T>,
) -> Result<T> {
    inspect().map_err(fail)?;
    source().map_err(fail)?;
    inspect().map_err(fail)?;
    let result = read();
    inspect().map_err(fail)?;
    // Even a failed complete file observation must recheck the actual source.
    // Cancellation/deadline refusal above already fails without any effect.
    after_read().map_err(fail)?;
    inspect().map_err(fail)?;
    result
}
pub(in crate::agent_bridge) struct ColdCommit {
    original: Commit,
    journal: Arc<crate::autosave::current_startup::Journal>,
    preference: Arc<PreferenceContinuation>,
    bindings: Arc<ServiceBindings>,
    binding_attempted: bool,
    attempted: bool,
    observation: Option<Value>,
}
impl ColdCommit {
    pub(in crate::agent_bridge) fn same_original_journal(&self, journal: &Arc<crate::autosave::current_startup::Journal>) -> bool {
        Arc::ptr_eq(&self.journal, journal)
    }
    pub(in crate::agent_bridge) fn prepare(s: &Session, root: Arc<DirectoryFrame>, operation: &str, check: &impl Fn() -> io::Result<()>) -> Result<Self> {
        let worker = s.persistence.as_ref().ok_or_else(|| fail("actual gated cold worker required before canonical preparation"))?;
        if worker.last_sent.is_some() || !worker.startup_pending() {
            return Err(fail("canonical preparation requires the original unsent cold worker"));
        }
        let journal = worker.current_image_journal(check).map_err(fail)?;
        if !journal.belongs_to(&root.record()) {
            return Err(fail("canonical preparation crossed its actual worker data directory"));
        }
        let mut original = Commit::prepare_inner(s, root, operation, check, Preparation::Cold)?;
        let before = match original.entries.iter().find(|e| e.relative == "preferences.json").map(|e| &e.effect) {
            Some(Effect::Before(before) | Effect::Unchanged(before)) => before.clone(),
            _ => return Err(fail("complete original cold preference frame required")),
        };
        let bindings = Arc::new(ServiceBindings::new(&original, s)?);
        original.cold_bindings = Some(bindings.clone());
        let value = Self {
            original,
            bindings,
            binding_attempted: false,
            journal,
            preference: Arc::new(PreferenceContinuation { before, phase: Mutex::new(PreferencePhase::Before) }),
            attempted: false,
            observation: None,
        };
        value.current_before(s, check)?;
        // The existing worker retains this same opaque writer continuation.
        // Its original inventory is never recaptured or replaced by a receipt.
        value.journal.retain_canonical_preferences(&value.original.root.record(), value.preference.clone(), check).map_err(fail)?;
        value.current_before(s, check)?;
        Ok(value)
    }
    fn worker_current(&self, s: &Session, check: &impl Fn() -> io::Result<()>) -> Result<()> {
        let worker = s.persistence.as_ref().ok_or_else(|| fail("original cold worker disappeared"))?;
        if worker.last_sent.is_some() || !worker.startup_pending() {
            return Err(fail("cold worker was released or sent input before canonical closure"));
        }
        // No publication occurs in these worker observations. Keep complete
        // caller/source fences on both sides and retain every original journal
        // check with its own bounded cancellation guard inside them.
        check().map_err(fail)?;
        let deadline = media_guard::Check::new(web_time::Instant::now());
        let observe = || deadline.check().map_err(io_error);
        let journal = worker.current_image_journal(&observe).map_err(fail)?;
        if !Arc::ptr_eq(&journal, &self.journal) || !journal.belongs_to(&self.original.root.record()) {
            return Err(fail("canonical writer belongs to another actual native worker"));
        }
        self.journal.current(&observe).map_err(fail)?;
        check().map_err(fail)
    }
    fn current_before(&self, s: &Session, check: &impl Fn() -> io::Result<()>) -> Result<()> {
        self.worker_current(s, check)?;
        self.original.current_memory(s, check)?;
        self.original.shortcut.current(check).map_err(fail)?;
        for entry in &self.original.entries {
            if !matches!(entry.effect, Effect::Before(_) | Effect::Unchanged(_)) {
                return Err(fail("unused original canonical preparation required"));
            }
            Commit::verify_effect(entry, &self.original.directory_seals, check)?;
        }
        Ok(())
    }
    pub(in crate::agent_bridge) fn prepare_description(&self, s: &Session, check: &impl Fn() -> io::Result<()>) -> Result<Value> {
        let deadline = media_guard::Check::new(web_time::Instant::now());
        self.prepare_description_with_inspection(s, check, &|| deadline.check().map_err(io_error))
    }
    fn prepare_description_with_inspection(&self, s: &Session, source: &impl Fn() -> io::Result<()>, inspect: &impl Fn() -> io::Result<()>) -> Result<Value> {
        if self.attempted {
            return Err(fail("cold canonical publication was already attempted; retain its observation without replay"));
        }
        inspect_read(source, inspect, || {
            self.current_before(s, inspect)?;
            self.original.observe(s, inspect)
        })
    }
    pub(in crate::agent_bridge) fn preference(&self) -> Arc<PreferenceContinuation> {
        self.preference.clone()
    }
    /// One actual dispatch; even a partial result consumes this in-memory owner.
    /// The caller must hold the exact original durable host intent beforehand.
    /// A returned record describes effects, including failure; it never grants
    /// startup, binding adoption, worker release, retry or overwrite authority.
    pub(in crate::agent_bridge) fn publish(&mut self, s: &Session, check: &impl Fn() -> io::Result<()>) -> Result<Value> {
        if self.attempted {
            return Err(fail("cold canonical publication was already attempted; no replay"));
        }
        // Consume before the first boundary check. A lost or rejected dispatch
        // cannot be repaired by re-entering the original consuming writer.
        self.attempted = true;
        {
            let mut phase = self.preference.phase.lock().unwrap_or_else(|e| e.into_inner());
            if !matches!(*phase, PreferencePhase::Before) {
                return Err(fail("cold canonical preference owner changed before dispatch"));
            }
            *phase = PreferencePhase::Publishing(thread::current().id());
        }
        let _publishing = PublishingPreference(self.preference.clone());
        let root = self.original.root.record();
        // Borrow disjoint fields explicitly; no new writer or restored grant.
        let worker = s.persistence.as_ref();
        let journal = self.journal.clone();
        let io = || {
            check()?;
            let worker = worker.ok_or_else(|| io::Error::other("cold canonical worker missing"))?;
            if worker.last_sent.is_some() || !worker.startup_pending() {
                return Err(io::Error::other("cold canonical worker no longer held"));
            }
            // This callback remains at every original writer boundary. Only
            // its nested read-only worker observation is bracketed; no source
            // observation is cached and no writer receives a cheaper guard.
            let deadline = media_guard::Check::new(web_time::Instant::now());
            let observe = || deadline.check().map_err(io_error);
            let current = worker.current_image_journal(&observe)?;
            if !Arc::ptr_eq(&current, &journal) || !journal.belongs_to(&root) {
                return Err(io::Error::other("cold canonical worker identity changed"));
            }
            journal.current(&observe)?;
            check()
        };
        let result = self.original.publish(s, &io).and_then(|()| self.original.verify_published(s, &io));
        let after = if result.is_ok() {
            match self.original.entries.iter().find(|e| e.relative == "preferences.json").map(|e| &e.effect) {
                Some(Effect::Unchanged(before)) => Ok(before.clone()),
                _ => self.original.published_preferences(&io).map(Arc::new),
            }
        } else {
            Err(fail("original canonical publication did not complete"))
        };
        let seen = self.original.observe_with_controls(s, &io);
        let published = result.is_ok() && after.is_ok() && seen.is_ok();
        let publication_error = result.err().or_else(|| after.as_ref().err().map(|e| fail(e.to_string()))).map(|e| e.to_string());
        let observation_error = seen.as_ref().err().map(ToString::to_string);
        let record = json!({"kind":"native-cold-canonical-publication-result","version":1,
            "published":published,"effects":seen.ok(),"publicationError":publication_error,"observationError":observation_error,
            "editingPaused":true,"workersMayResume":false,"startupAuthority":false,"writeAuthority":false,"replayAllowed":false,"fullApplicationClosure":false});
        self.observation = Some(record.clone());
        let mut phase = self.preference.phase.lock().unwrap_or_else(|e| e.into_inner());
        *phase = match after {
            Ok(after) if published => PreferencePhase::Published(after),
            _ => PreferencePhase::Stopped,
        };
        Ok(record)
    }
    /// Actual retained canonical receipts for the original worker's read-only
    /// source witness. The controller separately checks live Session memory.
    pub(in crate::agent_bridge) fn verify_published_source(&self, check: &impl Fn() -> io::Result<()>) -> Result<()> {
        self.read_published_source(check, true)
    }
    /// Original source/canonical generations after the SAME complete read.
    /// The actual writer always starts with verify_published_source again.
    pub(in crate::agent_bridge) fn fence_published_source_after_read(&self, check: &impl Fn() -> io::Result<()>) -> Result<()> {
        self.read_published_source(check, false)
    }
    fn read_published_source(&self, check: &impl Fn() -> io::Result<()>, read_bytes: bool) -> Result<()> {
        if !self.attempted
            || self.observation.as_ref().is_none_or(|v| v["published"] != true)
            || self.service_binding_description().is_none_or(|v| v["bound"] != true)
        {
            return Err(fail("the actual completed and bound canonical owner is required"));
        }
        self.journal.current(check).map_err(fail)?;
        if read_bytes {
            self.original.verify_published_files(check)?;
        } else {
            self.original.fence_published_files_after_read(check)?;
        }
        let preference = if read_bytes {
            self.preference.current_node(&self.preference.before, check)?
        } else {
            self.preference.fence_node_after_read(&self.preference.before, check)?
        };
        if !preference {
            return Err(fail("original cold preference continuation missing"));
        }
        self.journal.current(check).map_err(fail)
    }
    pub(in crate::agent_bridge) fn verify_published(&self, s: &Session, check: &impl Fn() -> io::Result<()>) -> Result<()> {
        let deadline = media_guard::Check::new(web_time::Instant::now());
        self.verify_published_with_inspection(s, check, &|| deadline.check().map_err(io_error))
    }
    /// Pure controller wait observation only. The same source starts with a
    /// complete query and ends with its original generation fence, including
    /// after failed file reads. All actual writers retain their full guards.
    pub(in crate::agent_bridge) fn inspect_published_source_read(&self, s: &Session, source: &impl retained::SourceCheck) -> Result<()> {
        let deadline = media_guard::Check::new(web_time::Instant::now());
        self.verify_published_with_source_fence(s, &|| source.current(), &|| source.fence_after_read(), &|| deadline.check().map_err(io_error))
    }
    /// Observation while the SAME original checkpoint worker is doing guarded
    /// publication. This cannot publish, release a worker or acknowledge a
    /// source result. The actual complete source brackets the entire wait.
    /// Keep the original canonical memory, service, physical source frames and
    /// preference checks; the supplied guard is the full read's SAME clock/Stop.
    pub(in crate::agent_bridge) fn inspect_checkpoint_wait(&self, s: &Session, check: &media_guard::Check) -> Result<()> {
        check.check()?;
        if !self.attempted || self.observation.as_ref().is_none_or(|v| v["published"] != true) {
            return Err(fail("complete actual cold canonical publication required"));
        }
        let inspect = || check.check().map_err(io_error);
        self.worker_current(s, &inspect)?;
        self.original.current_memory(s, &inspect)?;
        if !self.preference.current_node(&self.preference.before, &inspect)? {
            return Err(fail("original cold preference continuation missing"));
        }
        self.worker_current(s, &inspect)?;
        check.check()
    }
    fn verify_published_with_inspection(&self, s: &Session, source: &impl Fn() -> io::Result<()>, inspect: &impl Fn() -> io::Result<()>) -> Result<()> {
        self.verify_published_with_source_fence(s, source, source, inspect)
    }
    fn verify_published_with_source_fence(
        &self,
        s: &Session,
        source: &impl Fn() -> io::Result<()>,
        after_read: &impl Fn() -> io::Result<()>,
        inspect: &impl Fn() -> io::Result<()>,
    ) -> Result<()> {
        if !self.attempted || self.observation.as_ref().is_none_or(|v| v["published"] != true) {
            return Err(fail("complete actual cold canonical publication required"));
        }
        inspect_read_with_fence(source, after_read, inspect, || {
            self.worker_current(s, inspect)?;
            self.original.verify_published(s, inspect)?;
            if !self.preference.current_node(&self.preference.before, inspect)? {
                return Err(fail("original cold preference continuation missing"));
            }
            Ok(())
        })
    }
}
#[cfg(test)]
mod tests;
