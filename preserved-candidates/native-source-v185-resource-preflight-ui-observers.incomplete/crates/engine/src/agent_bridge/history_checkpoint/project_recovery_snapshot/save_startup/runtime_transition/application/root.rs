//! A plan from the SAME held native Session and its actual running executable.
//! No Root writer, new owner, application admission or worker release lives here.
use super::*;
mod publication;

pub(super) struct Root {
    plan: Value,
    executable: NodeFrame,
    intent_attempted: bool,
    intent_confirmed: bool,
    publication_attempted: bool,
    publication_confirmed: bool,
    manifest: Option<Arc<Control>>,
}
impl Root {
    fn resolved(&self) -> Result<()> {
        if self.intent_attempted && !self.intent_confirmed || self.publication_attempted && !self.publication_confirmed {
            return Err(error("the original Root continuation is unresolved; no replay or release"));
        }
        Ok(())
    }
    fn current_with_check(&self, check: &media_guard::Check) -> Result<()> {
        self.resolved()?;
        let io = || check.check().map_err(|e| io::Error::other(e.to_string()));
        self.executable.current(&io).map_err(error)?;
        self.executable.fence(&io).map_err(error)?;
        if let Some(manifest) = &self.manifest {
            manifest.current(check)?;
            manifest.fence(check)?;
        }
        check.check()
    }
    pub(super) fn current(&self) -> Result<()> {
        self.current_with_check(&media_guard::Check::new(web_time::Instant::now()))
    }
    /// One effect-free read composition from this ACTUAL retained Root. Every
    /// query fully reads the executable and original acknowledged Control, then
    /// checks their final generations even on failure. Large executable reads
    /// may overlap the read-only foreground on its original caller thread;
    /// both must join before any result. No cached byte authority or release.
    pub(super) fn observe_current<T>(&self, check: &media_guard::Check, observe: impl FnOnce(&Control) -> Result<T>) -> Result<T> {
        // Scheduling hint from the retained expected byte count only. Both
        // paths run the same complete read and original final generations.
        if matches!(self.executable.state(), NodeState::File(value) if value.bytes >= 8 * 1024 * 1024) {
            return self.observe_overlapped(check, observe);
        }
        self.current_with_check(check)?;
        let observed = (|| {
            if !self.publication_confirmed {
                return Err(error("the SAME actual Root publication acknowledgement is required"));
            }
            let manifest = self.manifest.as_deref().ok_or_else(|| error("the actual published Root Control is missing"))?;
            observe(manifest)
        })();
        self.fence_after_read(check)?;
        observed
    }
    /// Effect-free foreground only. The complete executable read uses the SAME
    /// actual retained frame and caller Check. No read result may escape until
    /// that full hash has joined and the original final Root fence has passed.
    fn observe_overlapped<T>(&self, check: &media_guard::Check, observe: impl FnOnce(&Control) -> Result<T>) -> Result<T> {
        self.resolved()?;
        let io = || check.check().map_err(|e| io::Error::other(e.to_string()));
        // Refuse an already changed name/generation before foreground entry.
        self.executable.fence(&io).map_err(error)?;
        if let Some(manifest) = &self.manifest {
            manifest.current(check)?;
            manifest.fence(check)?;
        }
        let observed = media_guard::parallel::paired(
            || {
                self.executable.current(&io).map_err(error)?;
                self.executable.fence(&io).map_err(error)
            },
            || {
                if !self.publication_confirmed {
                    return Err(error("the SAME actual Root publication acknowledgement is required"));
                }
                let manifest = self.manifest.as_deref().ok_or_else(|| error("the actual published Root Control is missing"))?;
                observe(manifest)
            },
        );
        // Preserve failure precedence and the original final fence, even after
        // a failed foreground or auxiliary read. paired has already joined.
        self.fence_after_read(check)?;
        observed.map(|(_, value)| value)
    }
    /// Final original-generation checks only, after a complete current() in
    /// the SAME source read. This is not a standalone write or startup check.
    pub(super) fn fence_after_read(&self, check: &media_guard::Check) -> Result<()> {
        self.resolved()?;
        let io = || check.check().map_err(|e| io::Error::other(e.to_string()));
        self.executable.fence(&io).map_err(error)?;
        if let Some(manifest) = &self.manifest {
            manifest.fence(check)?;
        }
        check.check()
    }
    pub(super) fn acknowledged_manifest_after_read(&self, check: &media_guard::Check) -> Result<&Control> {
        self.fence_after_read(check)?;
        if !self.publication_confirmed {
            return Err(error("the SAME actual Root publication acknowledgement is required"));
        }
        self.manifest.as_deref().ok_or_else(|| error("the actual published Root Control is missing"))
    }
    pub(super) fn acknowledged_manifest(&self) -> Result<&Control> {
        self.current()?;
        if !self.publication_confirmed {
            return Err(error("the SAME actual Root publication acknowledgement is required"));
        }
        self.manifest.as_deref().ok_or_else(|| error("the actual published Root Control is missing"))
    }
    pub(super) fn manifest(&self) -> Option<&Control> {
        self.manifest.as_deref()
    }
}

fn replacement(original: &Value, pid: u32, binary: &Path, digest: &str) -> Result<Value> {
    let mut value = original.clone();
    if original.as_object().is_none_or(|o| o.len() > 128)
        || original["kind"] != "poema-saved-native-workspace"
        || !original["version"].as_u64().is_some_and(|v| (2..=5).contains(&v))
        || original["nativePid"] == pid
        || pid < 2
        || pid > i32::MAX as u32
        || !literal(binary)
        || binary.to_str().is_none_or(|s| s.split('/').skip(1).any(|part| part.is_empty() || part == "." || part == ".."))
        || !hash_ok(digest)
    {
        return Err(error("exact original Root and distinct actual native process/build required"));
    }
    let object = value.as_object_mut().ok_or_else(|| error("actual original manifest object required"))?;
    object.insert("binary".into(), json!(binary));
    object.insert("binarySha256".into(), json!(digest));
    object.insert("nativePid".into(), json!(pid));
    bytes(&value, CATALOG as usize)?;
    Ok(value)
}

impl Pending {
    pub(in crate::agent_bridge::history_checkpoint::project_recovery_snapshot::save_startup::runtime_transition) fn prepare_application_root_plan(
        &mut self,
        s: &Session,
        request: &Value,
    ) -> Result<Value> {
        if self.application.root_attempted {
            return Err(error("the original application Root preparation was already attempted"));
        }
        self.current()?;
        self.worker_current(s.persistence.as_ref())?;
        self.check_canonical(s)?;
        if !self.application.confirmed {
            return Err(error("the same actual acknowledged original application intent is required"));
        }
        self.application.root_attempted = true;
        bytes(request, 16384)?;
        if !fields(request, "kind,version,nativeSession,privateInput,binary,sha256")
            || request["kind"] != "poema-original-Save-native-application-root-prepare"
            || request["version"] != 1
            || request["nativeSession"] != self.preparation["nativeSession"]
            || request["privateInput"] != self.preparation["privateInput"]
        {
            return Err(error("the same retained private pipe and actual application Root request are required"));
        }
        let binary = request["binary"].as_str().map(Path::new).filter(|p| literal(p)).ok_or_else(|| error("bounded actual executable path required"))?;
        let digest = request["sha256"].as_str().filter(|s| hash_ok(s)).ok_or_else(|| error("actual executable digest required"))?;
        let actual = std::env::current_exe().map_err(error)?;
        if actual != binary || std::fs::canonicalize(binary).map_err(error)? != binary {
            return Err(error("application Root must retain this actual running native executable"));
        }
        let check = media_guard::Check::new(web_time::Instant::now());
        let io = || check.check().map_err(|e| io::Error::other(e.to_string()));
        let directory = DirectoryFrame::capture(binary.parent().ok_or_else(|| error("actual executable parent missing"))?, &io).map_err(error)?;
        let name = binary.file_name().and_then(|s| s.to_str()).ok_or_else(|| error("actual native executable name required"))?;
        let executable = directory.node_bounded(name, 512 * 1024 * 1024, &io).map_err(error)?;
        let NodeState::File(observation) = executable.state() else {
            return Err(error("actual native executable must remain a complete regular file"));
        };
        if observation.bytes == 0 || observation.sha256 != digest {
            return Err(error("the actual running native executable differs from the retained original build"));
        }
        let original = control(&self.original, 0)?.value();
        let source = self.application.source.as_ref().ok_or_else(|| error("actual retained original replacement source required"))?;
        source.current(&check)?;
        let release = source.handoff.value()["nextBuild"].as_str().filter(|s| hash_ok(s)).ok_or_else(|| error("actual original source release required"))?;
        let prepared = self.application.prepared.as_ref().ok_or_else(|| error("the actual original application preparation disappeared"))?;
        let command_directory = prepared["owner"]["nativeDirectory"]
            .as_str()
            .map(Path::new)
            .filter(|p| literal(p))
            .ok_or_else(|| error("actual native command directory required"))?;
        let http_port = original["httpPort"].as_u64().filter(|v| (1..=65535).contains(v)).ok_or_else(|| error("original bounded composer port required"))?;
        let native_port = original["nativePort"].as_u64().filter(|v| (1..=65535).contains(v)).ok_or_else(|| error("original bounded native port required"))?;
        if http_port == native_port || original["projectPath"] != json!(s.path) {
            return Err(error("original independent project and distinct composer/native ports required"));
        }
        let next = replacement(original, std::process::id(), binary, digest)?;
        let plan = json!({"kind":"poema-native-Save-application-root-plan","version":1,
            "nativePid":std::process::id(),"nativeSession":self.preparation["nativeSession"],"privateInput":self.preparation["privateInput"],
            "manifest":original_description(&self.original,0,&self.original.body.basis.manifest_path)?,
            "original":original,"replacement":next,"nativeDirectory":command_directory,
            "composerBinding":{"version":2,"httpPort":http_port,"workspaceId":original["id"],"projectId":original["projectId"],
                "projectPath":original["projectPath"],"applicationRelease":release},
            "editingPaused":true,"workerReleased":false,"writeAuthority":false,"fullApplicationClosure":false});
        bytes(&plan, CATALOG as usize)?;
        self.application.root = Some(Arc::new(Root {
            plan: plan.clone(),
            executable,
            intent_attempted: false,
            intent_confirmed: false,
            publication_attempted: false,
            publication_confirmed: false,
            manifest: None,
        }));
        self.current()?;
        self.worker_current(s.persistence.as_ref())?;
        self.check_canonical(s)?;
        let root = self.application.root.as_ref().ok_or_else(|| error("actual application Root preparation disappeared"))?;
        Ok(root.plan.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn root_replacement_preserves_all_original_project_source_conversation_and_port_fields() {
        let original = json!({"kind":"poema-saved-native-workspace","version":4,"nativePid":101,"binary":"/generated/old","binarySha256":"a".repeat(64),
            "projectPath":"/generated/project.fcproj","nativeData":"/generated/data","nativePort":40001,"httpPort":40002,
            "originalEditorial":{"ownerId":"kept"},"conversationScope":{"identityPlane":"project"},"unknownFutureField":{"kept":true}});
        let next = replacement(&original, 202, Path::new("/generated/new"), &"b".repeat(64)).unwrap();
        let mut expected = original.clone();
        expected["nativePid"] = json!(202);
        expected["binary"] = json!("/generated/new");
        expected["binarySha256"] = json!("b".repeat(64));
        assert_eq!(next, expected);
        assert_eq!(original["nativePid"], 101);
    }
    #[test]
    fn root_replacement_refuses_retired_pid_hostile_paths_and_invalid_bounded_builds() {
        let original = json!({"kind":"poema-saved-native-workspace","version":4,"nativePid":101});
        for pid in [0, 1, 101, u32::MAX] {
            assert!(replacement(&original, pid, Path::new("/generated/new"), &"b".repeat(64)).is_err());
        }
        for path in ["relative", "/generated/../new", "/generated/./new", "/generated/with\ncontrol"] {
            assert!(replacement(&original, 202, Path::new(path), &"b".repeat(64)).is_err());
        }
        for digest in ["", "no-digest"] {
            assert!(replacement(&original, 202, Path::new("/generated/new"), digest).is_err());
        }
        let mut large = original.clone();
        large["oversized"] = json!("x".repeat(CATALOG as usize));
        assert!(replacement(&large, 202, Path::new("/generated/new"), &"b".repeat(64)).is_err());
    }
    // Generated filesystem witnesses only; these construct no Cold native owner.
    struct ReadFixture {
        directory: std::path::PathBuf,
        root: Root,
    }
    impl ReadFixture {
        fn new() -> Self {
            Self::sized(2 * 65536)
        }
        fn sized(executable_bytes: usize) -> Self {
            use std::io::Write;
            use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
            use std::sync::atomic::{AtomicU64, Ordering};
            static NEXT: AtomicU64 = AtomicU64::new(1);
            let directory = crate::temp_dir().join(format!("poema-root-read-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
            std::fs::DirBuilder::new().mode(0o700).create(&directory).unwrap();
            let directory = std::fs::canonicalize(directory).unwrap();
            for (name, bytes) in [("native.bin", vec![37; executable_bytes]), ("manifest.json", b"{\"original\":true}".to_vec())] {
                let mut file = std::fs::OpenOptions::new().create_new(true).write(true).mode(0o600).open(directory.join(name)).unwrap();
                file.write_all(&bytes).unwrap();
                file.sync_all().unwrap();
            }
            let check = media_guard::Check::new(web_time::Instant::now());
            let executable = DirectoryFrame::capture(&directory, &|| Ok(())).unwrap().node_bounded("native.bin", executable_bytes as u64, &|| Ok(())).unwrap();
            let manifest = Arc::new(Control::capture(&directory.join("manifest.json"), 4096, &check).unwrap());
            let root = Root {
                plan: Value::Null,
                executable,
                intent_attempted: true,
                intent_confirmed: true,
                publication_attempted: true,
                publication_confirmed: true,
                manifest: Some(manifest),
            };
            Self { directory, root }
        }
    }
    impl Drop for ReadFixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.directory);
        }
    }
    #[test]
    fn pure_root_read_borrows_only_same_original_acknowledged_control_and_keeps_standalone_checks() {
        let f = ReadFixture::new();
        let check = media_guard::Check::new(web_time::Instant::now());
        f.root
            .observe_current(&check, |manifest| {
                assert!(std::ptr::eq(manifest, f.root.manifest.as_deref().unwrap()));
                assert_eq!(manifest.value(), &json!({"original":true}));
                Ok(())
            })
            .unwrap();
        f.root.current().unwrap();
        f.root.acknowledged_manifest().unwrap();
        let mut f = ReadFixture::new();
        f.root.publication_attempted = false;
        f.root.publication_confirmed = false;
        let entered = std::cell::Cell::new(false);
        assert!(
            f.root
                .observe_current(&check, |_| {
                    entered.set(true);
                    Ok(())
                })
                .is_err()
        );
        assert!(!entered.get());
    }
    #[test]
    fn pure_root_read_refuses_in_place_bytes_equal_byte_inode_replacement_and_control_replacement() {
        for kind in 0..3 {
            let f = ReadFixture::new();
            let check = media_guard::Check::new(web_time::Instant::now());
            let result = f.root.observe_current(&check, |_| {
                let path = f.directory.join(if kind == 2 { "manifest.json" } else { "native.bin" });
                if kind == 0 {
                    std::fs::write(&path, vec![38; 2 * 65536]).unwrap();
                } else {
                    let bytes = std::fs::read(&path).unwrap();
                    std::fs::rename(&path, f.directory.join("held-original")).unwrap();
                    std::fs::write(&path, bytes).unwrap();
                }
                Ok(())
            });
            assert!(result.is_err(), "changed original generation cannot pass the final read fence");
            assert!(f.root.current().is_err());
        }
    }
    #[test]
    fn failed_pure_root_read_still_checks_its_complete_original_generation() {
        let f = ReadFixture::new();
        let check = media_guard::Check::new(web_time::Instant::now());
        let result: Result<()> = f.root.observe_current(&check, |_| {
            std::fs::write(f.directory.join("native.bin"), vec![38; 2 * 65536]).unwrap();
            Err(error("controlled read failure"))
        });
        let message = result.unwrap_err().to_string();
        assert!(message.contains("changed"));
        assert!(!message.contains("controlled read failure"));
    }
    #[test]
    fn pure_root_read_original_stop_prevents_entry_or_return_without_any_new_owner() {
        use std::sync::atomic::{AtomicBool, Ordering};
        let f = ReadFixture::new();
        let stop = Arc::new(AtomicBool::new(true));
        let check = media_guard::Check::with_stop(web_time::Instant::now(), stop.clone());
        let entered = std::cell::Cell::new(false);
        assert!(
            f.root
                .observe_current(&check, |_| {
                    entered.set(true);
                    Ok(())
                })
                .is_err()
        );
        assert!(!entered.get());
        stop.store(false, Ordering::Release);
        assert!(
            f.root
                .observe_current(&check, |_| {
                    stop.store(true, Ordering::Release);
                    Ok(())
                })
                .is_err()
        );
        stop.store(false, Ordering::Release);
        f.root.current().unwrap();
    }
    #[test]
    fn large_root_read_keeps_non_send_foreground_and_result_on_the_original_caller_with_same_manifest() {
        use std::{cell::Cell, rc::Rc};
        let _serial = media_guard::parallel::TEST_SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        let f = ReadFixture::sized(8 * 1024 * 1024);
        let check = media_guard::Check::new(web_time::Instant::now());
        let caller = std::thread::current().id();
        let state = Rc::new(Cell::new(0));
        let result = f
            .root
            .observe_current(&check, |manifest| {
                assert_eq!(std::thread::current().id(), caller);
                assert!(std::ptr::eq(manifest, f.root.manifest.as_deref().unwrap()));
                assert_eq!(manifest.value(), &json!({"original":true}));
                state.set(1);
                Ok(state.clone())
            })
            .unwrap();
        assert!(Rc::ptr_eq(&state, &result));
        assert_eq!(state.get(), 1);
        f.root.current().unwrap();
        f.root.acknowledged_manifest().unwrap();
    }

    #[test]
    fn large_root_read_refuses_in_place_same_byte_inode_and_manifest_changes_without_publishing_or_overwriting() {
        use std::{io::Write, os::unix::fs::OpenOptionsExt};
        let _serial = media_guard::parallel::TEST_SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        for fault in ["in-place", "replacement", "manifest"] {
            let f = ReadFixture::sized(8 * 1024 * 1024);
            let check = media_guard::Check::new(web_time::Instant::now());
            let path = f.directory.join(if fault == "manifest" { "manifest.json" } else { "native.bin" });
            let before = std::fs::read(&path).unwrap();
            let result = f.root.observe_current(&check, |_| {
                if fault == "in-place" {
                    std::fs::write(&path, vec![38; before.len()]).unwrap();
                } else {
                    std::fs::rename(&path, f.directory.join("held-original")).unwrap();
                    let mut output = std::fs::OpenOptions::new().create_new(true).write(true).mode(0o600).open(&path).unwrap();
                    output.write_all(&before).unwrap();
                }
                Ok(())
            });
            assert!(result.is_err(), "changed actual Root generation {fault}");
            assert!(f.root.current().is_err());
            assert!(path.exists());
            if fault != "in-place" {
                assert_eq!(std::fs::read(f.directory.join("held-original")).unwrap(), before);
            }
        }
    }

    #[test]
    fn large_root_read_joins_failed_or_panicking_foreground_and_preserves_the_final_generation_refusal() {
        let _serial = media_guard::parallel::TEST_SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        let f = ReadFixture::sized(8 * 1024 * 1024);
        let check = media_guard::Check::new(web_time::Instant::now());
        let result: Result<()> = f.root.observe_current(&check, |_| {
            std::fs::write(f.directory.join("native.bin"), vec![38; 8 * 1024 * 1024]).unwrap();
            Err(error("controlled foreground failure"))
        });
        let message = result.unwrap_err().to_string();
        assert!(message.contains("changed"));
        assert!(!message.contains("controlled foreground failure"));
        let f = ReadFixture::sized(8 * 1024 * 1024);
        let result =
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| f.root.observe_current::<()>(&check, |_| panic!("generated read-only foreground panic"))));
        assert!(result.is_ok());
        assert!(result.unwrap().is_err());
        f.root.current().unwrap();
    }

    #[test]
    fn large_root_read_preserves_initial_and_late_stop_and_original_expired_deadline_without_reset_or_entry() {
        use std::{
            cell::Cell,
            sync::atomic::{AtomicBool, Ordering},
            time::Duration,
        };
        let _serial = media_guard::parallel::TEST_SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        let f = ReadFixture::sized(8 * 1024 * 1024);
        let stop = Arc::new(AtomicBool::new(true));
        let check = media_guard::Check::with_stop(web_time::Instant::now(), stop.clone());
        let entered = Cell::new(false);
        assert!(
            f.root
                .observe_current(&check, |_| {
                    entered.set(true);
                    Ok(())
                })
                .is_err()
        );
        assert!(!entered.get());
        let expired = media_guard::Check::new(web_time::Instant::now() - Duration::from_secs(46));
        assert!(
            f.root
                .observe_current(&expired, |_| {
                    entered.set(true);
                    Ok(())
                })
                .is_err()
        );
        assert!(!entered.get());
        stop.store(false, Ordering::Release);
        assert!(
            f.root
                .observe_current(&check, |_| {
                    stop.store(true, Ordering::Release);
                    Ok(())
                })
                .is_err()
        );
        assert!(stop.load(Ordering::Acquire));
        stop.store(false, Ordering::Release);
        f.root.current().unwrap();
    }

    #[test]
    fn large_root_read_busy_original_slot_keeps_full_serial_bytes_same_foreground_thread_and_later_refusals() {
        let _serial = media_guard::parallel::TEST_SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        let f = ReadFixture::sized(8 * 1024 * 1024);
        let check = media_guard::Check::new(web_time::Instant::now());
        let caller = std::thread::current().id();
        media_guard::parallel::paired(
            || Ok(()),
            || {
                f.root.observe_current(&check, |manifest| {
                    assert_eq!(std::thread::current().id(), caller);
                    assert!(std::ptr::eq(manifest, f.root.manifest.as_deref().unwrap()));
                    Ok(())
                })
            },
        )
        .unwrap();
        std::fs::write(f.directory.join("native.bin"), vec![38; 8 * 1024 * 1024]).unwrap();
        let entered = std::cell::Cell::new(false);
        assert!(
            media_guard::parallel::paired(
                || Ok(()),
                || f.root.observe_current(&check, |_| {
                    entered.set(true);
                    Ok(())
                })
            )
            .is_err()
        );
        assert!(!entered.get());
    }

    #[test]
    fn large_root_same_original_constructor_read_composition_preserves_complete_bytes_and_records_abba_timing() {
        use crate::autosave::{AutoSavePrefs, AutosaveConfig, Persistence, session_reservation::ColdJournalReservation};
        use std::{
            fs,
            io::Write,
            os::unix::fs::{DirBuilderExt, OpenOptionsExt},
            time::Instant,
        };
        let _serial = media_guard::parallel::TEST_SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        let f = ReadFixture::sized(59_785_008);
        let data = f.directory.join("original-constructor-data");
        let recovery = data.join("Recovery");
        fs::DirBuilder::new().recursive(true).mode(0o700).create(&recovery).unwrap();
        // Genuine original constructor inventory, including stale locks and
        // preserved partial files. These are generated negative witnesses,
        // not a Cold application, native owner description or admission.
        for i in 0..3 {
            let old = recovery.join(format!("{}-1", 100 + i));
            fs::DirBuilder::new().mode(0o700).create(&old).unwrap();
            for name in [crate::autosave::SNAPSHOT, crate::autosave::META, crate::agent_bridge::NATIVE_JOURNAL_FILE, "lock"] {
                let mut file = fs::OpenOptions::new().create_new(true).write(true).mode(0o600).open(old.join(name)).unwrap();
                if name != "lock" {
                    file.write_all(&vec![41; 1024]).unwrap();
                }
                file.sync_all().unwrap();
            }
        }
        let cfg = AutosaveConfig::new(&data);
        let check = media_guard::Check::new(web_time::Instant::now());
        let io = || check.check().map_err(|e| std::io::Error::other(e.to_string()));
        let reservation = ColdJournalReservation::prepare(&cfg, &io).unwrap();
        struct StopWorker(Persistence);
        impl Drop for StopWorker {
            fn drop(&mut self) {
                self.0.stop_worker();
            }
        }
        let worker = StopWorker(Persistence::start_cold_reserved(&cfg, AutoSavePrefs::default(), reservation, &io).unwrap());
        let journal = worker.0.current_image_journal(&io).unwrap();
        let manifest = f.root.manifest.as_deref().unwrap();
        let caller = std::thread::current().id();
        let mut rounds = Vec::new();
        for overlap in [false, true, true, false] {
            let mut samples = Vec::new();
            for sample in 0..15 {
                let foreground = |same_manifest: &Control| {
                    assert_eq!(std::thread::current().id(), caller);
                    assert!(std::ptr::eq(same_manifest, manifest));
                    // The actual canonical source reader calls this SAME
                    // original journal.current before and after its files.
                    journal.current(&io).map_err(error)?;
                    journal.current(&io).map_err(error)
                };
                let started = Instant::now();
                if overlap {
                    f.root.observe_current(&check, foreground).unwrap();
                } else {
                    f.root.current_with_check(&check).unwrap();
                    foreground(manifest).unwrap();
                    f.root.fence_after_read(&check).unwrap();
                }
                if sample >= 3 {
                    samples.push(started.elapsed().as_micros() as u64);
                }
            }
            rounds.push(json!({"overlap":overlap,"samplesUs":samples}));
        }
        assert!(worker.0.startup_pending());
        assert!(worker.0.last_sent.is_none());
        f.root.current().unwrap();
        journal.current(&io).unwrap();
        println!(
            "{}",
            json!({"kind":"original-root-constructor-read-overlap-pilot","executableBytes":59_785_008,"sameActualRootAndManifestFrames":true,"sameActualOriginalConstructorJournal":true,"foregroundStayedOnCaller":true,"sameCallerCheck":true,"fullHashesRetained":true,"workerStillHeld":true,"wholeProducerNotMeasured":true,"rounds":rounds,"fullGoalComplete":false})
        );
        drop(journal);
        drop(worker);
    }
}
