//! Canonical native library files published by the original candidate owner.
//! Payloads come from the original native services, not host reconstruction.
pub(in crate::agent_bridge) mod cold;
mod observation;
use super::*;
use filmcraft_format::retained::preview_context::{DirectoryFrame, NodeFrame, NodeState};
use filmcraft_format::retained::{self, CurrentState, Outcome, Publication, StagedWrite};
pub(super) use observation::Observation;
use sha2::{Digest, Sha256};
use std::{
    io,
    path::{Path, PathBuf},
    sync::Arc,
};
const FILE_BYTES: usize = 64 * 1024 * 1024;
const TOTAL_BYTES: usize = 256 * 1024 * 1024;
enum Effect {
    Before(Arc<NodeFrame>),
    Staging,
    Staged(StagedWrite),
    Published { publication: Publication, intent: retained::Intent },
    Unchanged(Arc<NodeFrame>),
}
struct Entry {
    relative: &'static str,
    bytes: Vec<u8>,
    source: Option<NodeFrame>,
    effect: Effect,
}
pub(super) struct Commit {
    root: Arc<DirectoryFrame>,
    shortcut: Arc<DirectoryFrame>,
    directory_attempted: bool,
    operation: String,
    entries: Vec<Entry>,
    stopped: Option<String>,
    preferences: bool,
    cold_sources: Option<(Vec<Option<PathBuf>>, Option<PathBuf>)>,
    cold_bindings: Option<Arc<cold::ServiceBindings>>,
    owned_shortcut: Option<retained::directory_effect::OwnedDirectory>,
    directory_seals: Arc<Vec<retained::directory_effect::SealedDirectory>>,
}
fn fail(message: impl std::fmt::Display) -> crate::EngineError {
    bad(ACTIVATE, message.to_string())
}
fn io_error(e: crate::EngineError) -> io::Error {
    io::Error::other(e.to_string())
}
fn payloads(s: &Session, preferences: bool) -> Result<Vec<(&'static str, Vec<u8>, Option<PathBuf>)>> {
    let mut values = vec![
        (
            "Keyboard Shortcuts/active.json",
            s.shortcuts.recovery_file_bytes().map_err(fail)?,
            s.shortcuts.recovery_directory().map(|p| p.join("Keyboard Shortcuts/active.json")),
        ),
        ("effect-presets.json", s.presets.recovery_file_bytes().map_err(fail)?, s.presets.recovery_directory().map(|p| p.join(crate::presets::LIBRARY_FILE))),
        (
            "export-presets.json",
            s.export_presets.recovery_file_bytes().map_err(fail)?,
            s.export_presets.dir().map(|p| p.join(crate::export_tools::LIBRARY_FILE)),
        ),
    ];
    if preferences {
        let bytes = crate::project_save::effects::preference_bytes(&s.prefs);
        if bytes.is_empty() || bytes.len() > 2 * 1024 * 1024 {
            return Err(fail("native preferences exceed their original complete bound"));
        }
        values.push(("preferences.json", bytes, None));
    }
    let mut total = 0usize;
    for (_, bytes, _) in &values {
        total =
            total.checked_add(bytes.len()).filter(|n| *n <= TOTAL_BYTES).ok_or_else(|| fail("native library payload exceeds its original aggregate bound"))?;
        if bytes.is_empty() || bytes.len() > FILE_BYTES {
            return Err(fail("native library payload exceeds its original file bound"));
        }
    }
    Ok(values)
}
fn frame(path: &Path, check: &impl Fn() -> io::Result<()>) -> Result<NodeFrame> {
    if let Ok(meta) = std::fs::symlink_metadata(path) {
        if !meta.is_file() || meta.len() > FILE_BYTES as u64 {
            return Err(fail("original native library is redirected or excessive"));
        }
    }
    let parent = DirectoryFrame::capture(path.parent().ok_or_else(|| fail("library parent missing"))?, check).map_err(fail)?;
    parent.node(path.file_name().and_then(|v| v.to_str()).ok_or_else(|| fail("literal library filename required"))?, check).map_err(fail)
}
fn copied(source: Option<&NodeFrame>, target: &NodeFrame) -> bool {
    match (source.map(|n| n.state()), target.state()) {
        (None | Some(NodeState::Absent), NodeState::Absent) => true,
        (Some(NodeState::File(a)), NodeState::File(b)) => a.bytes == b.bytes && a.sha256 == b.sha256,
        _ => false,
    }
}
enum Preparation<'a> {
    Copied,
    Fresh(&'a super::current_image::activation::FreshDirectory),
    Cold,
}
impl Commit {
    pub(super) fn prepare(s: &Session, root: Arc<DirectoryFrame>, operation: &str, check: &impl Fn() -> io::Result<()>) -> Result<Self> {
        Self::prepare_inner(s, root, operation, check, Preparation::Copied)
    }
    pub(super) fn prepare_fresh(s: &Session, key: &super::current_image::activation::FreshDirectory, check: &media_guard::Check) -> Result<Self> {
        key.initial(s, check)?;
        let operation = s.agent_bridge.workspace_update.as_ref().ok_or_else(|| fail("actual activation required"))?.token.clone();
        let io = || key.io_current(check).map_err(io_error);
        Self::prepare_inner(s, key.root(), &operation, &io, Preparation::Fresh(key))
    }
    fn prepare_inner(
        s: &Session,
        root: Arc<DirectoryFrame>,
        operation: &str,
        check: &impl Fn() -> io::Result<()>,
        preparation: Preparation<'_>,
    ) -> Result<Self> {
        require_idle_writer_jobs(s)?;
        check().map_err(fail)?;
        root.current(check).map_err(fail)?;
        if !root.exists() || !token_ok(operation) {
            return Err(fail("actual configured candidate directory and activation owner required"));
        }
        let shortcut = DirectoryFrame::capture(&root.record().path.join(crate::shortcuts::DIR_NAME), check).map_err(fail)?;
        let mut entries = Vec::new();
        let preferences = !matches!(preparation, Preparation::Copied);
        let cold = matches!(preparation, Preparation::Cold);
        let values = payloads(s, preferences)?;
        if cold && s.prefs_path.as_ref().is_some_and(|p| p != &root.record().path.join("preferences.json")) {
            return Err(fail("cold native preferences are bound outside the actual worker data directory"));
        }
        let cold_sources = cold.then(|| (values.iter().map(|(_, _, source)| source.clone()).collect(), s.prefs_path.clone()));
        // Validate every original and destination before staging or publication.
        for (relative, bytes, source) in values {
            let target_path = root.record().path.join(relative);
            // Cold restoration keeps the actual in-memory library. Its original
            // service may be unbound or bound to this exact native data root;
            // never follow a restored source binding into the internal original.
            if cold && source.as_ref().is_some_and(|p| p != &target_path) {
                return Err(fail("cold native library source is outside its actual worker data directory"));
            }
            let source = if cold { None } else { source.as_ref().map(|p| frame(p, check)).transpose()? };
            let target = Arc::new(frame(&target_path, check)?);
            if let Preparation::Fresh(key) = &preparation {
                if let Some(source) = &source {
                    key.source(source)?;
                }
                if !matches!(target.state(), NodeState::Absent) {
                    return Err(fail("fresh native library target appeared; no foreign file overwritten"));
                }
            } else if !cold && !copied(source.as_ref(), &target) {
                return Err(fail("candidate library differs from its original resource copy; no library was overwritten"));
            }
            let effect = if matches!(target.state(),NodeState::File(v)if v.bytes==bytes.len()as u64&&v.sha256==format!("{:x}",Sha256::digest(&bytes))) {
                Effect::Unchanged(target)
            } else {
                Effect::Before(target)
            };
            entries.push(Entry { relative, bytes, source, effect });
        }
        let result = Self {
            root,
            shortcut,
            directory_attempted: false,
            operation: operation.into(),
            entries,
            stopped: None,
            preferences,
            cold_sources,
            cold_bindings: None,
            owned_shortcut: None,
            directory_seals: Arc::new(Vec::new()),
        };
        result.current_memory(s, check)?;
        Ok(result)
    }
    pub(super) fn take_owned_shortcut(&mut self) -> Option<retained::directory_effect::OwnedDirectory> {
        self.owned_shortcut.take()
    }
    pub(super) fn accept_directory_seals(
        &mut self,
        seals: Arc<Vec<retained::directory_effect::SealedDirectory>>,
        check: &impl Fn() -> io::Result<()>,
    ) -> Result<()> {
        if !self.preferences || !self.directory_seals.is_empty() {
            return Err(fail("fresh original canonical directory continuation required once"));
        }
        for seal in seals.iter() {
            seal.current(check).map_err(fail)?;
            if !seal.pin().path.starts_with(self.root.record().path) || seal.pin().path == self.root.record().path {
                return Err(fail("authored directory seal escapes the actual configured root"));
            }
            if seal.pin().path == self.shortcut.record().path && !seal.continues(&self.shortcut.record().anchor).map_err(fail)? {
                return Err(fail("canonical shortcut directory seal is foreign"));
            }
        }
        self.shortcut = DirectoryFrame::capture(&self.shortcut.record().path, check).map_err(fail)?;
        self.directory_seals = seals;
        for e in &self.entries {
            Self::verify_effect(e, &self.directory_seals, check)?;
        }
        Ok(())
    }
    pub(super) fn verify_published(&self, s: &Session, check: &impl Fn() -> io::Result<()>) -> Result<()> {
        if self.stopped.is_some() {
            return Err(fail("canonical native publication is stopped; no replay"));
        }
        self.current_memory(s, check)?;
        self.verify_published_files(check)?;
        self.current_memory(s, check)
    }
    /// Read-only half of the SAME original Commit. No Session, service rebind,
    /// new frame or matching publication can stand in for its actual receipts.
    fn verify_published_files(&self, check: &impl Fn() -> io::Result<()>) -> Result<()> {
        self.read_published_files(check, true)
    }
    /// Final original generations in the SAME complete source read. Never
    /// replace verify_published_files before a write with this final fence.
    fn fence_published_files_after_read(&self, check: &impl Fn() -> io::Result<()>) -> Result<()> {
        self.read_published_files(check, false)
    }
    fn read_published_files(&self, check: &impl Fn() -> io::Result<()>, read_bytes: bool) -> Result<()> {
        if self.stopped.is_some() {
            return Err(fail("canonical native publication is stopped; no replay"));
        }
        self.root.current(check).map_err(fail)?;
        if let Some(owned) = &self.owned_shortcut {
            owned.current(check).map_err(fail)?;
            if self.shortcut.record().anchor != *owned.pin() {
                return Err(fail("original owned shortcut directory differs from the retained canonical frame"));
            }
        }
        self.shortcut.current(check).map_err(fail)?;
        for entry in &self.entries {
            if let Some(source) = &entry.source {
                if read_bytes {
                    source.current(check).map_err(fail)?;
                }
                source.fence(check).map_err(fail)?;
            }
            if !matches!(entry.effect, Effect::Published { .. } | Effect::Unchanged(_)) {
                return Err(fail("complete original canonical publication required"));
            }
            if read_bytes {
                Self::verify_effect(entry, &self.directory_seals, check)?;
            } else {
                match &entry.effect {
                    Effect::Unchanged(frame) => frame.fence(check).map_err(fail)?,
                    Effect::Published { publication, intent } => {
                        retained::fence_publication_after_read(publication, intent, &self.directory_seals, check).map_err(fail)?;
                    }
                    _ => return Err(fail("complete original canonical publication required for its final fence")),
                }
            }
        }
        self.root.current(check).map_err(fail)
    }
    pub(super) fn published_preferences(&self, check: &impl Fn() -> io::Result<()>) -> Result<NodeFrame> {
        if !self.preferences {
            return Err(fail("original stopped-current preference writer required"));
        }
        let entry = self.entries.iter().find(|e| e.relative == "preferences.json").ok_or_else(|| fail("canonical preference role missing"))?;
        Self::verify_effect(entry, &self.directory_seals, check)?;
        let Effect::Published { publication, .. } = &entry.effect else {
            return Err(fail("actual preference publication required"));
        };
        let output = publication.receipt.output.as_ref().ok_or_else(|| fail("actual preference output missing"))?;
        let target = frame(&self.root.record().path.join("preferences.json"), check)?;
        if target.state() != &NodeState::File(output.clone()) {
            return Err(fail("published native preference generation changed"));
        }
        target.current(check).map_err(fail)?;
        target.fence(check).map_err(fail)?;
        Ok(target)
    }
    fn current_memory(&self, s: &Session, check: &impl Fn() -> io::Result<()>) -> Result<()> {
        check().map_err(fail)?;
        self.root.current(check).map_err(fail)?;
        if let Some(owned) = &self.owned_shortcut {
            // Already inside the original guarded memory/directory fence.
            // Observe the real owned descriptor without repeating the outer
            // complete source guard around this bounded metadata operation.
            owned.current(&|| Ok(())).map_err(fail)?;
            if self.shortcut.record().anchor != *owned.pin() {
                return Err(fail("original owned shortcut directory differs from the retained canonical frame"));
            }
        }
        let values = payloads(s, self.preferences)?;
        if let Some((sources, path)) = &self.cold_sources {
            if let Some(bindings) = &self.cold_bindings {
                bindings.current(s, sources, path, &values.iter().map(|(_, _, source)| source.clone()).collect::<Vec<_>>())?;
            } else if values.iter().map(|(_, _, source)| source).ne(sources.iter()) || path != &s.prefs_path {
                return Err(fail("actual cold native library service bindings changed"));
            }
        }
        if values.len() != self.entries.len() || values.iter().zip(&self.entries).any(|((path, bytes, _), e)| *path != e.relative || bytes != &e.bytes) {
            return Err(fail("actual native library memory changed during publication"));
        }
        for e in &self.entries {
            if let Some(source) = &e.source {
                source.current(check).map_err(fail)?;
                source.fence(check).map_err(fail)?;
            }
        }
        Ok(())
    }
    fn verify_effect(e: &Entry, seals: &[retained::directory_effect::SealedDirectory], check: &impl Fn() -> io::Result<()>) -> Result<()> {
        match &e.effect {
            Effect::Before(frame) | Effect::Unchanged(frame) => frame.current(check).map_err(fail),
            Effect::Staged(_) => Ok(()),
            Effect::Published { publication: p, intent } => {
                let seen = retained::inspect_publication_after_directory_seals(p, intent, seals, check).map_err(fail)?;
                if !p.receipt_durable
                    || p.receipt.outcome != Outcome::Published
                    || seen.current != CurrentState::Published
                    || seen.recorded_receipt.as_ref() != Some(&p.receipt)
                {
                    return Err(fail("native library publication remains unconfirmed or changed; no replay"));
                }
                Ok(())
            }
            Effect::Staging => Err(fail("native library staging was already attempted; retain its evidence without replay")),
        }
    }
    pub(super) fn publish(&mut self, s: &Session, check: &impl Fn() -> io::Result<()>) -> Result<()> {
        if let Some(e) = &self.stopped {
            return Err(fail(e));
        }
        let result = self.publish_inner(s, check);
        if let Err(e) = &result {
            self.stopped = Some(e.to_string());
        }
        result
    }
    fn publish_inner(&mut self, s: &Session, check: &impl Fn() -> io::Result<()>) -> Result<()> {
        self.current_memory(s, check)?;
        for e in &self.entries {
            Self::verify_effect(e, &self.directory_seals, check)?;
        }
        if !self.shortcut.exists() {
            if self.directory_attempted {
                return Err(fail("candidate shortcut directory creation is unresolved; no adoption or retry"));
            }
            self.shortcut.current(check).map_err(fail)?;
            self.directory_attempted = true;
            self.owned_shortcut = Some(retained::directory_effect::stage(self.shortcut.clone(), check).map_err(fail)?.publish_owned(check).map_err(fail)?);
            self.root.current(check).map_err(fail)?;
            self.shortcut = DirectoryFrame::capture(&self.shortcut.record().path, check).map_err(fail)?;
            if let Some(owned) = &self.owned_shortcut {
                // Already inside the original guarded memory/directory fence.
                // Observe the real owned descriptor without repeating the outer
                // complete source guard around this bounded metadata operation.
                owned.current(&|| Ok(())).map_err(fail)?;
                if self.shortcut.record().anchor != *owned.pin() {
                    return Err(fail("new shortcut directory was replaced; retain both generations without adoption"));
                }
            }
            if !self.shortcut.exists() || std::fs::read_dir(&self.shortcut.record().path).map_err(fail)?.next().is_some() {
                return Err(fail("new shortcut directory changed before publication"));
            }
            std::fs::File::open(&self.root.record().path).and_then(|f| f.sync_all()).map_err(fail)?;
            let target = self.shortcut.node("active.json", check).map_err(fail)?;
            let first = self.entries.first_mut().ok_or_else(|| fail("canonical shortcut role missing"))?;
            if !matches!(target.state(), NodeState::Absent) {
                return Err(fail("candidate shortcut target appeared"));
            }
            first.effect = Effect::Before(Arc::new(target));
        }
        self.shortcut.current(check).map_err(fail)?;
        // No target changes until all complete payloads have been staged.
        for index in 0..self.entries.len() {
            // The variant test is pure. Preserve the complete original guard
            // around every actual staging operation; unchanged rows do no IO.
            if !matches!(self.entries.get(index).map(|e| &e.effect), Some(Effect::Before(_))) {
                continue;
            }
            self.current_memory(s, check)?;
            let entry = self.entries.get_mut(index).ok_or_else(|| fail("canonical library role missing"))?;
            let before = std::mem::replace(&mut entry.effect, Effect::Staging);
            let Effect::Before(frame) = before else {
                return Err(fail("canonical library staging ownership changed"));
            };
            let operation = format!("{:x}", Sha256::digest(format!("{}:native-library:{}", self.operation, entry.relative).as_bytes()));
            let staged = retained::stage_from_frame(&frame, &operation, &entry.bytes, check).map_err(fail)?;
            entry.effect = Effect::Staged(staged);
        }
        for index in 0..self.entries.len() {
            // Only the original staged owner can dispatch. Keep its full
            // before-publication memory and complete-effect fences unchanged.
            if !matches!(self.entries.get(index).map(|e| &e.effect), Some(Effect::Staged(_))) {
                continue;
            }
            self.current_memory(s, check)?;
            for e in &self.entries {
                Self::verify_effect(e, &self.directory_seals, check)?;
            }
            let entry = self.entries.get_mut(index).ok_or_else(|| fail("canonical library role missing"))?;
            if !matches!(&entry.effect, Effect::Staged(_)) {
                continue;
            }
            let effect = std::mem::replace(&mut entry.effect, Effect::Staging);
            let Effect::Staged(staged) = effect else {
                return Err(fail("original staged library owner missing"));
            };
            // The original retained writer atomically preserves displaced bytes
            // and its immutable intent/receipt; no body is ever replayed.
            let intent = staged.intent().clone();
            entry.effect = Effect::Published { publication: staged.publish(check), intent };
            Self::verify_effect(entry, &self.directory_seals, check)?;
        }
        self.current_memory(s, check)?;
        for e in &self.entries {
            Self::verify_effect(e, &self.directory_seals, check)?;
        }
        Ok(())
    }
}
pub(super) fn activate(s: &mut Session, check: &media_guard::Check) -> Result<()> {
    if !s.native_auxiliary_restored {
        return Ok(());
    }
    let (had, root, token) = {
        let h = s.agent_bridge.workspace_update.as_ref().ok_or_else(|| fail("actual paused candidate required"))?;
        let root = h.persistence_directory.clone().ok_or_else(|| fail("original configured directory descriptor required"))?;
        if h.persistence_start.as_ref().is_none_or(|cfg| cfg.data_dir != root.record().path) {
            return Err(fail("original candidate directory binding changed"));
        }
        (h.library_commit.clone(), root, h.token.clone())
    };
    let io = || check.check().map_err(io_error);
    let owner = if let Some(owner) = had {
        owner
    } else {
        let prepared = Commit::prepare(s, root, &token, &io)?;
        let owner = Arc::new(std::sync::Mutex::new(prepared));
        s.agent_bridge.workspace_update.as_mut().ok_or_else(|| fail("candidate owner disappeared"))?.library_commit = Some(owner.clone());
        owner
    };
    let result = owner.lock().unwrap_or_else(|e| e.into_inner()).publish(s, &io);
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    struct Scratch(PathBuf);
    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    fn scratch() -> Scratch {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let path = crate::temp_dir().join(format!("native-library-commit-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
        std::fs::create_dir(&path).unwrap();
        Scratch(std::fs::canonicalize(path).unwrap())
    }
    fn root(path: &Path) -> Arc<DirectoryFrame> {
        DirectoryFrame::capture(path, &|| Ok(())).unwrap()
    }
    #[test]
    fn library_durability_missing_shortcut_directory_is_owned_once_and_all_payloads_survive_original_loaders() {
        let dir = scratch();
        let s = Session::default();
        let mut commit = Commit::prepare(&s, root(&dir.0), &"a".repeat(64), &|| Ok(())).unwrap();
        commit.publish(&s, &|| Ok(())).unwrap();
        let before = std::fs::metadata(dir.0.join("Keyboard Shortcuts/active.json")).unwrap();
        commit.publish(&s, &|| Ok(())).unwrap();
        assert_eq!(before.len(), std::fs::metadata(dir.0.join("Keyboard Shortcuts/active.json")).unwrap().len());
        let mut loaded = Session::default();
        loaded.shortcuts.set_dir(&dir.0);
        loaded.presets.set_dir(&dir.0);
        loaded.export_presets.set_dir(&dir.0);
        assert_eq!(loaded.shortcuts.recovery_file_bytes().unwrap(), s.shortcuts.recovery_file_bytes().unwrap());
        assert_eq!(loaded.presets.recovery_file_bytes().unwrap(), s.presets.recovery_file_bytes().unwrap());
        assert_eq!(loaded.export_presets.recovery_file_bytes().unwrap(), s.export_presets.recovery_file_bytes().unwrap());
        assert!(loaded.persistence.is_none());
    }
    #[test]
    fn library_durability_partial_publication_retains_human_bytes_original_receipts_and_never_replays() {
        let dir = scratch();
        let source = dir.0.join("source");
        let target = dir.0.join("candidate");
        std::fs::create_dir(&source).unwrap();
        std::fs::create_dir(&target).unwrap();
        let mut s = Session::default();
        let old = payloads(&s, false).unwrap();
        s.shortcuts.bind_directory(&source);
        s.presets.bind_directory(&source);
        s.export_presets.bind_directory(&source);
        crate::agent_bridge::native_auxiliary_tests::populate(&mut s);
        for (relative, bytes, _) in old {
            for base in [&source, &target] {
                let path = base.join(relative);
                std::fs::create_dir_all(path.parent().unwrap()).unwrap();
                std::fs::write(path, &bytes).unwrap();
            }
        }
        let mut commit = Commit::prepare(&s, root(&target), &"b".repeat(64), &|| Ok(())).unwrap();
        let changed = AtomicBool::new(false);
        let first = target.join("Keyboard Shortcuts/active.json");
        let second = target.join("effect-presets.json");
        let first_bytes = s.shortcuts.recovery_file_bytes().unwrap();
        let check = || {
            if std::fs::read(&first).is_ok_and(|b| b == first_bytes) && !changed.swap(true, Ordering::SeqCst) {
                std::fs::write(&second, b"human edit after first publication")?;
            }
            Ok(())
        };
        assert!(commit.publish(&s, &check).is_err());
        assert!(changed.load(Ordering::SeqCst));
        assert_eq!(std::fs::read(&first).unwrap(), first_bytes);
        assert_eq!(std::fs::read(&second).unwrap(), b"human edit after first publication");
        let stable = std::fs::read(&first).unwrap();
        assert!(commit.publish(&s, &|| Ok(())).is_err());
        assert_eq!(std::fs::read(&first).unwrap(), stable);
        assert_eq!(std::fs::read(&second).unwrap(), b"human edit after first publication");
        assert!(s.persistence.is_none());
    }
    fn library_effects_source_and_target(dir: &Scratch) -> (Session, PathBuf) {
        let source = dir.0.join("source");
        let target = dir.0.join("candidate");
        std::fs::create_dir(&source).unwrap();
        std::fs::create_dir(&target).unwrap();
        let mut s = Session::default();
        let old = payloads(&s, false).unwrap();
        s.shortcuts.bind_directory(&source);
        s.presets.bind_directory(&source);
        s.export_presets.bind_directory(&source);
        crate::agent_bridge::native_auxiliary_tests::populate(&mut s);
        for (relative, bytes, _) in old {
            for base in [&source, &target] {
                let path = base.join(relative);
                std::fs::create_dir_all(path.parent().unwrap()).unwrap();
                std::fs::write(path, &bytes).unwrap();
            }
        }
        (s, target)
    }
    fn library_effects_operation_count(target: &Path) -> usize {
        [target.to_path_buf(), target.join("Keyboard Shortcuts")]
            .iter()
            .map(|p| std::fs::read_dir(p).unwrap().filter_map(|v| v.ok()).filter(|e| e.file_name().to_string_lossy().starts_with(".poema-save-")).count())
            .sum()
    }
    #[test]
    fn library_effects_staged_only_observation_preserves_later_human_bytes_and_never_dispatches() {
        let dir = scratch();
        let (s, target) = library_effects_source_and_target(&dir);
        let mut commit = Commit::prepare(&s, root(&target), &"c".repeat(64), &|| Ok(())).unwrap();
        // The original writer returns each real one-shot owner. No dispatch
        // is submitted: this fixture specifically observes completed staging.
        for entry in &mut commit.entries {
            let before = std::mem::replace(&mut entry.effect, Effect::Staging);
            let Effect::Before(frame) = before else { panic!("fresh observed target required") };
            let operation = format!("{:x}", Sha256::digest(format!("{}:native-library:{}", commit.operation, entry.relative).as_bytes()));
            entry.effect = Effect::Staged(retained::stage_from_frame(&frame, &operation, &entry.bytes, &|| Ok(())).unwrap());
        }
        commit.stopped = Some("controlled stopped candidate after completed staging".into());
        let before = commit.observe(&s, &|| Ok(())).unwrap();
        for row in before["entries"].as_array().unwrap() {
            assert_eq!(row["phase"], "staged");
            assert_eq!(row["observation"]["current"], "prepared");
            assert_eq!(row["observation"]["dispatch"], false);
        }
        let human = target.join("effect-presets.json");
        std::fs::write(&human, b"later human bytes").unwrap();
        let after = commit.observe(&s, &|| Ok(())).unwrap();
        assert!(after["entries"].as_array().unwrap().iter().any(|r| r["observation"]["current"] == "changed"));
        assert_eq!(after["writeAuthority"], false);
        assert_eq!(after["startupAuthority"], false);
        assert_eq!(after["replayAllowed"], false);
        assert!(commit.publish(&s, &|| Ok(())).is_err());
        assert_eq!(std::fs::read(human).unwrap(), b"later human bytes");
        for row in after["entries"].as_array().unwrap() {
            assert_eq!(row["observation"]["dispatch"], false);
        }
        assert!(s.persistence.is_none());
    }
    #[test]
    fn library_effects_incomplete_staging_remains_unknown_without_importing_an_archived_dispatch_owner() {
        let dir = scratch();
        let (s, target) = library_effects_source_and_target(&dir);
        let mut commit = Commit::prepare(&s, root(&target), &"d".repeat(64), &|| Ok(())).unwrap();
        let check = || {
            if library_effects_operation_count(&target) > 0 {
                return Err(io::Error::other("controlled stop during first staging"));
            }
            Ok(())
        };
        assert!(commit.publish(&s, &check).is_err());
        let seen = commit.observe(&s, &|| Ok(())).unwrap();
        assert_eq!(seen["entries"][0]["phase"], "staging_unknown");
        assert_eq!(seen["entries"][0]["retainedDispatchOwner"], false);
        assert_eq!(seen["entries"][1]["phase"], "before");
        assert_eq!(seen["entries"][2]["phase"], "before");
        assert_eq!(seen["writeAuthority"], false);
        assert_eq!(seen["startupAuthority"], false);
        assert_eq!(seen["replayAllowed"], false);
        assert!(commit.publish(&s, &|| Ok(())).is_err());
        assert_eq!(library_effects_operation_count(&target), 1);
        assert!(s.persistence.is_none());
    }
}
