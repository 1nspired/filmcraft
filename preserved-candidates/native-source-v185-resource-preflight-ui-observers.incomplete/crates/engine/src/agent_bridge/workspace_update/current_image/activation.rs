//! A new native owner minted from actual stopped input and actual startup
//! controls. Old library Record/Commit descriptions never become live owners.
use super::*;
use crate::agent_bridge::save_operation::managed_dispatch::{CurrentImageControls, current_image_controls};
use std::sync::Mutex;

pub(in crate::agent_bridge::workspace_update) struct FreshDirectory {
    input: Arc<restore::Input>,
    root: Arc<DirectoryFrame>,
    controls: CurrentImageControls,
    token: String,
    epoch: String,
    authored_namespace: std::collections::BTreeMap<PathBuf, std::collections::BTreeSet<String>>,
    journal: std::sync::OnceLock<Arc<crate::autosave::current_startup::Journal>>,
    #[cfg(test)]
    stop_after_directory: std::sync::atomic::AtomicBool,
    #[cfg(test)]
    stop_before_release: std::sync::atomic::AtomicBool,
    #[cfg(test)]
    authored_stop: std::sync::atomic::AtomicU8,
    #[cfg(test)]
    journal_fault: std::sync::atomic::AtomicU8,
}
impl FreshDirectory {
    #[cfg(test)]
    pub(super) fn after_authored_effect(&self, phase: u8) -> Result<()> {
        if phase == 1 && self.authored_stop.load(std::sync::atomic::Ordering::Relaxed) == 3 {
            std::fs::write(self.root.record().path.join("foreign-after-copy.json"), b"later human data").map_err(|e| bad(ACTIVATE, e.to_string()))?;
        }
        if self.authored_stop.load(std::sync::atomic::Ordering::Relaxed) == phase {
            return Err(bad(ACTIVATE, "controlled Stop after actual authored publication effect"));
        }
        Ok(())
    }
    fn bind_journal(&self, journal: Arc<crate::autosave::current_startup::Journal>, check: &media_guard::Check) -> Result<()> {
        self.controls_current(check)?;
        let io = || check.check().map_err(|e| io::Error::other(e.to_string()));
        journal.current(&io).map_err(|e| bad(ACTIVATE, e.to_string()))?;
        if !journal.belongs_to(&self.root.record()) {
            return Err(bad(ACTIVATE, "native journal continuation crossed the actual configured directory"));
        }
        self.journal.set(journal).map_err(|_| bad(ACTIVATE, "native journal continuation already bound; no replacement"))?;
        self.io_current(check)
    }
    pub(in crate::agent_bridge::workspace_update) fn root(&self) -> Arc<DirectoryFrame> {
        self.root.clone()
    }
    pub(in crate::agent_bridge::workspace_update) fn source(&self, frame: &NodeFrame) -> Result<()> {
        if !self.input.matches_source(frame) {
            return Err(bad(ACTIVATE, "canonical library source is outside the actual original current-image frames"));
        }
        Ok(())
    }
    pub(in crate::agent_bridge::workspace_update) fn current(&self, s: &Session, check: &media_guard::Check) -> Result<()> {
        self.input.current(check)?;
        let h = s.agent_bridge.workspace_update.as_ref().ok_or_else(|| bad(ACTIVATE, "actual stopped-current owner required"))?;
        if h.token != self.token
            || h.native_session != self.epoch
            || h.restored_current_image.as_ref().is_none_or(|v| !Arc::ptr_eq(v, &self.input))
            || h.persistence_directory.as_ref().is_none_or(|v| !Arc::ptr_eq(v, &self.root))
            || h.persistence_start.as_ref().is_none_or(|c| c.data_dir != self.root.record().path)
            || s.persistence.is_some()
            || s.save_operation_held()
        {
            return Err(bad(ACTIVATE, "actual fresh stopped-current owner changed"));
        }
        self.io_current(check)
    }
    pub(in crate::agent_bridge::workspace_update) fn initial(&self, s: &Session, check: &media_guard::Check) -> Result<()> {
        self.current(s, check)?;
        if !super::names(&self.root.record().path, check)?.is_empty() {
            return Err(bad(ACTIVATE, "new native activation directory is not empty; no foreign file was replaced"));
        }
        Ok(())
    }
    fn controls_current(&self, check: &media_guard::Check) -> Result<()> {
        self.controls.current(check)?;
        self.input.owner_exited()?;
        let io = || check.check().map_err(|e| io::Error::other(e.to_string()));
        self.root.current(&io).map_err(|e| bad(ACTIVATE, e.to_string()))?;
        let space = self.root.space(&io).map_err(|e| bad(ACTIVATE, e.to_string()))?;
        // Canonical writers and authored copies retain independent archives and
        // slots. Include every bounded copy plus metadata before publication.
        let needed = self.input.reserve().saturating_add(2048 * 1024 * 1024).saturating_add(MAX_BODY as u64);
        if space.read_only || space.free < needed {
            return Err(bad(ACTIVATE, "current activation volume or reserve unavailable"));
        }
        self.controls.current(check)?;
        check.check()
    }
    /// Inner retained I/O still checks the actual source controls, producer exit,
    /// candidate root, live source process, volume, reserve and operation deadline.
    /// Full authored namespace fences surround each native effect and final release.
    pub(super) fn resource_io_current(&self, check: &media_guard::Check) -> Result<()> {
        self.controls_current(check)
    }
    pub(in crate::agent_bridge::workspace_update) fn io_current(&self, check: &media_guard::Check) -> Result<()> {
        self.controls_current(check)?;
        let io = || check.check().map_err(|e| io::Error::other(e.to_string()));
        #[cfg(test)]
        if self.stop_after_directory.load(std::sync::atomic::Ordering::Relaxed) && self.root.record().path.join("Keyboard Shortcuts").exists() {
            return Err(bad(ACTIVATE, "controlled original stop after native directory publication"));
        }
        if let Some(journal) = self.journal.get() {
            if !journal.belongs_to(&self.root.record()) {
                return Err(bad(ACTIVATE, "native gated journal root changed"));
            }
            journal.current(&io).map_err(|e| bad(ACTIVATE, e.to_string()))?;
        }
        let root = self.root.record().path;
        for (relative, allowed) in &self.authored_namespace {
            let path = root.join(relative);
            match std::fs::symlink_metadata(&path) {
                Ok(_) => {
                    let frame = DirectoryFrame::capture(&path, &io).map_err(|e| bad(ACTIVATE, e.to_string()))?;
                    frame.current(&io).map_err(|e| bad(ACTIVATE, e.to_string()))?;
                    if !frame.exists()
                        || super::names(&path, check)?
                            .iter()
                            .any(|name| !allowed.contains(name) && !(relative.as_os_str().is_empty() && name == "Recovery" && self.journal.get().is_some()))
                    {
                        return Err(bad(ACTIVATE, "foreign native authored or canonical namespace member appeared"));
                    }
                }
                Err(e) if e.kind() == io::ErrorKind::NotFound => {}
                Err(e) => return Err(bad(ACTIVATE, e.to_string())),
            }
        }
        self.controls.current(check)?;
        check.check()
    }
}
pub(in crate::agent_bridge::workspace_update) struct Owner {
    directory: FreshDirectory,
    authored: super::authored::Plan,
    canonical: Option<Arc<Mutex<super::super::library_commit::Commit>>>,
    attempted: bool,
    stopped: Option<String>,
}
impl Owner {
    fn verify_current_files(&self, s: &Session, check: &media_guard::Check) -> Result<()> {
        self.authored.verify(&self.directory, check)?;
        let io = || self.directory.resource_io_current(check).map_err(|e| io::Error::other(e.to_string()));
        self.canonical
            .as_ref()
            .ok_or_else(|| bad(ACTIVATE, "actual original canonical writer required"))?
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .verify_published(s, &io)?;
        self.directory.io_current(check)
    }
    #[cfg(test)]
    fn test_journal_fault(&self, s: &Session) -> io::Result<()> {
        use std::os::unix::fs::PermissionsExt;
        let fault = self.directory.journal_fault.load(std::sync::atomic::Ordering::Relaxed);
        if fault == 0 {
            return Ok(());
        }
        let p = s.persistence.as_ref().ok_or_else(|| io::Error::other("actual test worker required"))?;
        if !p.startup_pending() {
            return Err(io::Error::other("test mutation must precede actual worker release"));
        }
        match fault {
            1 | 5 => {
                let lock = p.session_dir.join("lock");
                let bytes = std::fs::read(&lock)?;
                std::fs::rename(&lock, p.session_dir.join("held-lock"))?;
                std::fs::write(&lock, bytes)?;
                std::fs::set_permissions(lock, std::fs::Permissions::from_mode(0o600))?;
            }
            2 => {
                let root = crate::autosave::recovery_root(&p.data_dir);
                std::fs::rename(&root, p.data_dir.join("held-native-Recovery"))?;
                std::fs::create_dir(&root)?;
                std::fs::set_permissions(root, std::fs::Permissions::from_mode(0o700))?;
            }
            3 => {
                std::fs::rename(&p.session_dir, p.session_dir.with_extension("held"))?;
                std::fs::create_dir(&p.session_dir)?;
                std::fs::set_permissions(&p.session_dir, std::fs::Permissions::from_mode(0o700))?;
            }
            4 => std::fs::write(p.session_dir.join("foreign-native-member"), b"later human data")?,
            6 => {
                let file = p.data_dir.join("effect-presets.json");
                let bytes = std::fs::read(&file)?;
                let permissions = std::fs::metadata(&file)?.permissions();
                let held = p.data_dir.parent().ok_or_else(|| io::Error::other("test data parent missing"))?.join("held-effect-presets.json");
                std::fs::rename(&file, held)?;
                std::fs::write(&file, bytes)?;
                std::fs::set_permissions(file, permissions)?;
            }
            _ => return Err(io::Error::other("unknown controlled journal fault")),
        }
        Ok(())
    }
    pub(in crate::agent_bridge::workspace_update) fn start_persistence(
        &self,
        s: &mut Session,
        cfg: crate::autosave::AutosaveConfig,
        preferences: &NodeFrame,
        check: &media_guard::Check,
    ) -> io::Result<()> {
        let io = || self.directory.io_current(check).map_err(|e| io::Error::other(e.to_string()));
        self.directory.input.current(check).map_err(|e| io::Error::other(e.to_string()))?;
        io()?;
        self.verify_current_files(s, check).map_err(|e| io::Error::other(e.to_string()))?;
        let startup = || self.directory.input.current(check).and_then(|_| self.directory.controls_current(check)).map_err(|e| io::Error::other(e.to_string()));
        s.start_current_image_persistence(cfg, preferences, &io, &startup)?;
        let journal = s.persistence.as_ref().ok_or_else(|| io::Error::other("actual original startup worker missing"))?.current_image_journal(&startup)?;
        self.directory.bind_journal(journal, check).map_err(|e| io::Error::other(e.to_string()))?;
        // The original worker is waiting on its native startup gate. Its
        // original FIFO is intact; no pending snapshot/Prefs/Flush was consumed.
        #[cfg(test)]
        if self.directory.stop_before_release.load(std::sync::atomic::Ordering::Relaxed) {
            return Err(io::Error::other("controlled original Stop before actual worker release"));
        }
        #[cfg(test)]
        if self.directory.journal_fault.load(std::sync::atomic::Ordering::Relaxed) < 5 {
            self.test_journal_fault(s)?;
        }
        self.directory.input.current(check).and_then(|_| self.directory.controls_current(check)).map_err(|e| io::Error::other(e.to_string()))?;
        self.verify_current_files(s, check).map_err(|e| io::Error::other(e.to_string()))?;
        let released = s.persistence.as_ref().ok_or_else(|| io::Error::other("original startup worker missing"))?.release_current_image(
            |prepared| {
                #[cfg(test)]
                if prepared && self.directory.journal_fault.load(std::sync::atomic::Ordering::Relaxed) >= 5 {
                    self.test_journal_fault(s).map_err(|e| e.to_string())?;
                }
                if prepared {
                    self.directory.input.current(check).map_err(|e| e.to_string())?;
                }
                self.verify_current_files(s, check).and_then(|_| self.directory.controls_current(check)).map_err(|e| e.to_string())
            },
            || self.directory.controls_current(check).map_err(|e| e.to_string()),
        );
        if let Err(error) = released {
            if let Some(persistence) = s.persistence.as_mut() {
                persistence.stop_worker();
            }
            return Err(io::Error::other(error));
        }
        Ok(())
    }
    pub(in crate::agent_bridge::workspace_update) fn publish(&mut self, s: &mut Session, check: &media_guard::Check) -> Result<NodeFrame> {
        if self.attempted || self.stopped.is_some() {
            return Err(bad(ACTIVATE, "stopped-current activation was already attempted; retain its effects without replay"));
        }
        self.directory.initial(s, check)?;
        let prepared = super::super::library_commit::Commit::prepare_fresh(s, &self.directory, check)?;
        let commit = Arc::new(Mutex::new(prepared));
        let h = s.agent_bridge.workspace_update.as_mut().ok_or_else(|| bad(ACTIVATE, "native current-image owner disappeared"))?;
        if h.library_commit.is_some() {
            return Err(bad(ACTIVATE, "an old library Commit cannot be adopted by a new activation owner"));
        }
        h.library_commit = Some(commit.clone());
        self.canonical = Some(commit.clone());
        self.attempted = true;
        let result: Result<NodeFrame> = (|| {
            self.directory.current(s, check)?;
            let io = || self.directory.io_current(check).map_err(|e| io::Error::other(e.to_string()));
            let mut writer = commit.lock().unwrap_or_else(|e| e.into_inner());
            writer.publish(s, &io)?;
            let shortcut = writer.take_owned_shortcut();
            let seals = self.authored.publish(&self.directory, shortcut, &self.directory.token, check)?;
            writer.accept_directory_seals(seals, &io)?;
            let preferences = writer.published_preferences(&io)?;
            drop(writer);
            self.directory.current(s, check)?;
            Ok(preferences)
        })();
        if let Err(e) = &result {
            self.stopped = Some(e.to_string());
        }
        result
    }
}
pub(in crate::agent_bridge::workspace_update) fn admit(s: &mut Session, check: &media_guard::Check) -> Result<Option<Arc<Mutex<Owner>>>> {
    let Some(h) = s.agent_bridge.workspace_update.as_ref() else { return Ok(None) };
    let Some(input) = h.restored_current_image.clone() else { return Ok(None) };
    if let Some(owner) = &h.current_activation_owner {
        return Ok(Some(owner.clone()));
    }
    let root = h.persistence_directory.clone().ok_or_else(|| bad(ACTIVATE, "actual configured fresh native data directory required"))?;
    if h.library_commit.is_some() {
        return Err(bad(ACTIVATE, "old library owner cannot supply stopped-current activation custody"));
    }
    input.fresh_directory(&root, check)?;
    let expected = input.activation_description(s, &root)?;
    let controls = current_image_controls(s, &expected, check)?;
    let h = s.agent_bridge.workspace_update.as_ref().ok_or_else(|| bad(ACTIVATE, "native candidate disappeared"))?;
    let authored = super::authored::Plan::prepare(&input, &root, check)?;
    let authored_namespace = authored.namespace(&h.token)?;
    let directory = FreshDirectory {
        input,
        root,
        controls,
        token: h.token.clone(),
        epoch: h.native_session.clone(),
        authored_namespace,
        journal: std::sync::OnceLock::new(),
        #[cfg(test)]
        stop_after_directory: std::sync::atomic::AtomicBool::new(false),
        #[cfg(test)]
        stop_before_release: std::sync::atomic::AtomicBool::new(false),
        #[cfg(test)]
        authored_stop: std::sync::atomic::AtomicU8::new(0),
        #[cfg(test)]
        journal_fault: std::sync::atomic::AtomicU8::new(0),
    };
    directory.initial(s, check)?;
    let owner = Arc::new(Mutex::new(Owner { directory, authored, canonical: None, attempted: false, stopped: None }));
    s.agent_bridge.workspace_update.as_mut().ok_or_else(|| bad(ACTIVATE, "native candidate disappeared"))?.current_activation_owner = Some(owner.clone());
    Ok(Some(owner))
}

#[cfg(test)]
pub(in crate::agent_bridge::workspace_update) fn prepare_test(s: &mut Session, stop: bool) -> Result<()> {
    let owner = admit(s, &media_guard::Check::new(web_time::Instant::now()))?.ok_or_else(|| bad(ACTIVATE, "actual current-image test owner required"))?;
    owner.lock().unwrap_or_else(|e| e.into_inner()).directory.stop_after_directory.store(stop, std::sync::atomic::Ordering::Relaxed);
    Ok(())
}

#[cfg(test)]
pub(in crate::agent_bridge::workspace_update) fn prepare_worker_stop_test(s: &mut Session) -> Result<()> {
    let owner = admit(s, &media_guard::Check::new(web_time::Instant::now()))?.ok_or_else(|| bad(ACTIVATE, "actual current-image test owner required"))?;
    owner.lock().unwrap_or_else(|e| e.into_inner()).directory.stop_before_release.store(true, std::sync::atomic::Ordering::Relaxed);
    Ok(())
}

#[cfg(test)]
pub(in crate::agent_bridge::workspace_update) fn prepare_authored_stop_test(s: &mut Session, phase: u8) -> Result<()> {
    let owner = admit(s, &media_guard::Check::new(web_time::Instant::now()))?.ok_or_else(|| bad(ACTIVATE, "actual current image owner required"))?;
    owner.lock().unwrap_or_else(|e| e.into_inner()).directory.authored_stop.store(phase, std::sync::atomic::Ordering::Relaxed);
    Ok(())
}

#[cfg(test)]
pub(in crate::agent_bridge::workspace_update) fn prepare_journal_fault_test(s: &mut Session, fault: u8) -> Result<()> {
    let owner = admit(s, &media_guard::Check::new(web_time::Instant::now()))?.ok_or_else(|| bad(ACTIVATE, "actual current image owner required"))?;
    owner.lock().unwrap_or_else(|e| e.into_inner()).directory.journal_fault.store(fault, std::sync::atomic::Ordering::Relaxed);
    Ok(())
}
