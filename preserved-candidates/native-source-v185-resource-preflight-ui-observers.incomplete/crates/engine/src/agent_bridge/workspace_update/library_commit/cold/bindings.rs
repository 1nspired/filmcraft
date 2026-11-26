//! The original cold Commit's in-memory service binding continuation. It never
//! imports a writer, reloads a library, replaces a baseline or releases a worker.
use super::*;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase {
    Before,
    Changing,
    Bound,
    Stopped,
}
pub(crate) struct ServiceBindings {
    root: Arc<DirectoryFrame>,
    before_sources: Vec<Option<PathBuf>>,
    before_preferences: Option<PathBuf>,
    after_sources: Vec<Option<PathBuf>>,
    after_preferences: Option<PathBuf>,
    project: Arc<filmcraft_project::Project>,
    services: Arc<dyn crate::Services>,
    previews: Arc<crate::previews::PreviewStore>,
    media: Arc<crate::MediaPool>,
    phase: Mutex<Phase>,
}
impl ServiceBindings {
    pub(super) fn new(original: &Commit, s: &Session) -> Result<Self> {
        let (before_sources, before_preferences) = original.cold_sources.clone().ok_or_else(|| fail("original cold source binding tuple required"))?;
        if original.entries.len() != 4 || before_sources.len() != 4 || !original.preferences {
            return Err(fail("four original canonical service bindings required"));
        }
        let after_sources = original
            .entries
            .iter()
            .map(|entry| if entry.relative == "preferences.json" { None } else { Some(original.root.record().path.join(entry.relative)) })
            .collect();
        Ok(Self {
            root: original.root.clone(),
            before_sources,
            before_preferences,
            after_sources,
            after_preferences: Some(original.root.record().path.join("preferences.json")),
            project: s.project.clone(),
            services: s.services.clone(),
            previews: s.previews.clone(),
            media: s.media.clone(),
            phase: Mutex::new(Phase::Before),
        })
    }
    fn same_session(&self, s: &Session) -> Result<()> {
        if !Arc::ptr_eq(&self.project, &s.project)
            || !Arc::ptr_eq(&self.services, &s.services)
            || !Arc::ptr_eq(&self.previews, &s.previews)
            || !Arc::ptr_eq(&self.media, &s.media)
        {
            return Err(fail("original cold service binding belongs to another native owner"));
        }
        Ok(())
    }
    fn phase(&self) -> Phase {
        *self.phase.lock().unwrap_or_else(|e| e.into_inner())
    }
    pub(in crate::agent_bridge::workspace_update::library_commit) fn current(
        &self,
        s: &Session,
        original_sources: &[Option<PathBuf>],
        original_preferences: &Option<PathBuf>,
        actual_sources: &[Option<PathBuf>],
    ) -> Result<()> {
        self.same_session(s)?;
        if original_sources != self.before_sources || original_preferences != &self.before_preferences {
            return Err(fail("original cold service source tuple was replaced"));
        }
        let (sources, preferences) = match self.phase() {
            Phase::Before => (&self.before_sources, &self.before_preferences),
            Phase::Bound => (&self.after_sources, &self.after_preferences),
            Phase::Changing | Phase::Stopped => return Err(fail("cold service binding is unresolved; retain its actual owner without replay")),
        };
        if actual_sources != sources || &s.prefs_path != preferences {
            return Err(fail("actual cold native library service bindings changed"));
        }
        Ok(())
    }
    pub(crate) fn preference_current(&self, s: &Session, original_preferences: &Option<PathBuf>) -> Result<()> {
        self.same_session(s)?;
        if original_preferences != &self.before_preferences {
            return Err(fail("cold binding cannot replace the original immutable Save basis"));
        }
        let expected = match self.phase() {
            Phase::Before => &self.before_preferences,
            Phase::Bound => &self.after_preferences,
            Phase::Changing | Phase::Stopped => return Err(fail("original cold preference binding is unresolved")),
        };
        if &s.prefs_path != expected {
            return Err(fail("original cold preference binding changed outside its continuation"));
        }
        Ok(())
    }
}
struct BindingAttempt {
    original: Arc<ServiceBindings>,
    complete: bool,
}
impl Drop for BindingAttempt {
    fn drop(&mut self) {
        if !self.complete {
            *self.original.phase.lock().unwrap_or_else(|e| e.into_inner()) = Phase::Stopped;
        }
    }
}
impl ColdCommit {
    pub(in crate::agent_bridge) fn service_binding_description(&self) -> Option<Value> {
        if !self.binding_attempted {
            return None;
        }
        let phase = self.bindings.phase();
        Some(json!({"kind":"native-cold-canonical-service-binding-status","version":1,
            "phase":if phase == Phase::Bound { "bound" } else { "unresolved" },
            "bound":phase == Phase::Bound,"directory":self.bindings.root.record(),
            "preferences":self.bindings.after_preferences,"editingPaused":true,
            "workersMayResume":false,"startupAuthority":false,"writeAuthority":false,
            "fullApplicationClosure":false}))
    }
    pub(in crate::agent_bridge) fn service_bindings(&self) -> &ServiceBindings {
        &self.bindings
    }
    pub(in crate::agent_bridge) fn bind_services(&mut self, s: &mut Session, check: &impl Fn() -> io::Result<()>) -> Result<Value> {
        if self.binding_attempted {
            return Err(fail("original cold service binding was already attempted; no replay"));
        }
        // A rejected invocation consumes no publication and changes no service.
        // Once this binding attempt starts it cannot be retried after failure.
        self.verify_published(s, check)?;
        if self.bindings.phase() != Phase::Before || self.original.cold_bindings.as_ref().is_none_or(|v| !Arc::ptr_eq(v, &self.bindings)) {
            return Err(fail("same original unused cold service binding owner required"));
        }
        self.binding_attempted = true;
        let mut attempt = BindingAttempt { original: self.bindings.clone(), complete: false };
        let root = self.bindings.root.record().path;
        *self.bindings.phase.lock().unwrap_or_else(|e| e.into_inner()) = Phase::Changing;
        // Original bind-only services preserve their existing in-memory
        // libraries. These assignments perform no filesystem IO or callbacks.
        s.shortcuts.bind_directory(&root);
        s.presets.bind_directory(&root);
        s.export_presets.bind_directory(&root);
        s.prefs_path = Some(root.join("preferences.json"));
        *self.bindings.phase.lock().unwrap_or_else(|e| e.into_inner()) = Phase::Bound;
        self.verify_published(s, check)?;
        self.bindings.preference_current(s, &self.bindings.before_preferences)?;
        let result = json!({"kind":"native-cold-canonical-service-bindings","version":1,
            "directory":self.bindings.root.record(),"preferences":s.prefs_path,
            "bound":true,"editingPaused":true,"workersMayResume":false,
            "startupAuthority":false,"writeAuthority":false,"fullApplicationClosure":false});
        attempt.complete = true;
        Ok(result)
    }
}

#[cfg(test)]
mod tests;
