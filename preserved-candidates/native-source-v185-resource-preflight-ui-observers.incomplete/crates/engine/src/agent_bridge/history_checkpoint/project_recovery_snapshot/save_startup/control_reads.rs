//! Schedule only the original retained Control observations. This reuses the
//! engine's one extra read-only slot; owners and source/effect fences stay on
//! their original caller. No new Check, remembered observation or file writer.
use super::{ApplicationRead, Control, error, media_guard};
use crate::Result;

pub(super) fn selected(controls: &[&Control], check: &media_guard::Check, mode: ApplicationRead) -> Result<()> {
    check.check()?;
    if controls.len() > 8 {
        return Err(error("the original bounded control roles are required"));
    }
    media_guard::parallel::sweep(controls.len(), true, |index| {
        let control = controls.get(index).ok_or_else(|| error("an actual retained original control is required"))?;
        match mode {
            ApplicationRead::Complete => control.current(check),
            ApplicationRead::FenceAfterRead => control.fence(check),
        }
    })?;
    check.check()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        fs,
        io::Write,
        os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt, symlink},
        path::PathBuf,
        sync::{
            Arc,
            atomic::{AtomicBool, AtomicUsize, Ordering},
        },
        time::Duration,
    };

    pub(super) struct Fixture {
        pub(super) root: PathBuf,
        pub(super) files: Vec<PathBuf>,
        pub(super) controls: Vec<Control>,
        bytes: Vec<Vec<u8>>,
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }
    pub(super) fn fixture() -> Fixture {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let root = crate::temp_dir().join(format!("original-control-reads-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
        fs::DirBuilder::new().mode(0o700).create(&root).unwrap();
        let root = fs::canonicalize(root).unwrap();
        let mut files = Vec::new();
        let mut controls = Vec::new();
        let mut bytes = Vec::new();
        let check = media_guard::Check::new(web_time::Instant::now());
        for i in 0..8 {
            let mut directory = root.clone();
            for depth in 0..(i % 3 + 1) {
                directory = directory.join(format!("retained-control-{i}-level-{depth}"));
                fs::DirBuilder::new().mode(0o700).create(&directory).unwrap();
            }
            let file = directory.join("control.json");
            let raw = serde_json::to_vec(&serde_json::json!({"role":i,"payload":"x".repeat(1024)})).unwrap();
            let mut output = fs::OpenOptions::new().create_new(true).write(true).mode(0o600).open(&file).unwrap();
            output.write_all(&raw).unwrap();
            output.sync_all().unwrap();
            controls.push(Control::capture(&file, 64 * 1024, &check).unwrap());
            files.push(file);
            bytes.push(raw);
        }
        Fixture { root, files, controls, bytes }
    }
    pub(super) fn same_bytes(f: &Fixture) {
        for (file, raw) in f.files.iter().zip(&f.bytes) {
            assert_eq!(fs::read(file).unwrap(), *raw);
        }
    }
    fn run(f: &Fixture, mode: ApplicationRead) -> Result<()> {
        let check = media_guard::Check::new(web_time::Instant::now());
        selected(&f.controls.iter().collect::<Vec<_>>(), &check, mode)
    }

    #[test]
    fn original_control_reads_freshly_refuse_every_changed_leaf_in_complete_and_final_modes_without_writing() {
        let _serial = media_guard::parallel::TEST_SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        for mode in [ApplicationRead::Complete, ApplicationRead::FenceAfterRead] {
            for index in 0..8 {
                let f = fixture();
                run(&f, mode).unwrap();
                same_bytes(&f);
                let mut changed = f.bytes[index].clone();
                changed[0] = b' ';
                fs::write(&f.files[index], &changed).unwrap();
                assert!(run(&f, mode).is_err(), "changed original control {index}");
                assert_eq!(fs::read(&f.files[index]).unwrap(), changed);
            }
        }
    }

    #[test]
    fn original_control_reads_refuse_same_bytes_new_inode_restored_mtime_mode_and_redirected_parent() {
        let _serial = media_guard::parallel::TEST_SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        for mode in [ApplicationRead::Complete, ApplicationRead::FenceAfterRead] {
            for fault in ["replacement", "restored-mtime", "mode", "parent-link"] {
                let f = fixture();
                run(&f, mode).unwrap();
                let file = &f.files[7];
                match fault {
                    "replacement" => {
                        fs::rename(file, file.with_file_name("held-original.json")).unwrap();
                        let mut output = fs::OpenOptions::new().create_new(true).write(true).mode(0o600).open(file).unwrap();
                        output.write_all(&f.bytes[7]).unwrap();
                    }
                    "restored-mtime" => {
                        let modified = fs::metadata(file).unwrap().modified().unwrap();
                        let mut changed = f.bytes[7].clone();
                        changed[0] = b' ';
                        fs::write(file, changed).unwrap();
                        fs::File::open(file).unwrap().set_times(fs::FileTimes::new().set_modified(modified)).unwrap();
                    }
                    "mode" => fs::set_permissions(file, fs::Permissions::from_mode(0o400)).unwrap(),
                    "parent-link" => {
                        let parent = file.parent().unwrap();
                        let held = parent.with_file_name("held-original-parent");
                        fs::rename(parent, &held).unwrap();
                        symlink(&held, parent).unwrap();
                    }
                    _ => unreachable!(),
                }
                assert!(run(&f, mode).is_err(), "original control mutation {fault}");
            }
        }
    }

    #[test]
    fn original_control_reads_borrow_the_same_stop_and_expired_clock_without_resetting_them() {
        let _serial = media_guard::parallel::TEST_SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        let f = fixture();
        let controls = f.controls.iter().collect::<Vec<_>>();
        let stop = Arc::new(AtomicBool::new(true));
        let cancelled = media_guard::Check::with_stop(web_time::Instant::now(), stop.clone());
        let expired = media_guard::Check::new(web_time::Instant::now() - Duration::from_secs(46));
        for mode in [ApplicationRead::Complete, ApplicationRead::FenceAfterRead] {
            assert!(selected(&controls, &cancelled, mode).is_err());
            assert!(stop.load(Ordering::Acquire));
            assert!(selected(&controls, &expired, mode).is_err());
        }
        same_bytes(&f);
        let check = media_guard::Check::new(web_time::Instant::now());
        assert!(selected(&[&f.controls[0]; 9], &check, ApplicationRead::Complete).is_err());
    }

    #[test]
    fn concurrent_original_control_readers_join_with_the_same_frames_and_refuse_a_later_changed_generation() {
        let _serial = media_guard::parallel::TEST_SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        let f = fixture();
        let controls = f.controls.iter().collect::<Vec<_>>();
        let check = media_guard::Check::new(web_time::Instant::now());
        for changed in [false, true] {
            if changed {
                fs::write(&f.files[4], &f.bytes[4]).unwrap();
            }
            let results = std::thread::scope(|scope| {
                let first = scope.spawn(|| selected(&controls, &check, ApplicationRead::Complete));
                let second = scope.spawn(|| selected(&controls, &check, ApplicationRead::FenceAfterRead));
                (first.join().unwrap(), second.join().unwrap())
            });
            assert_eq!(results.0.is_ok(), !changed);
            assert_eq!(results.1.is_ok(), !changed);
            same_bytes(&f);
        }
    }
}

#[cfg(test)]
mod perf {
    use super::*;
    use std::time::Instant;

    #[test]
    #[ignore = "bounded generated-file scheduling pilot, not an acceptance timing assertion"]
    fn original_actual_control_read_scheduling_pilot() {
        let f = tests::fixture();
        let controls = f.controls.iter().collect::<Vec<_>>();
        let mut results = Vec::new();
        for (label, mode) in [("complete", ApplicationRead::Complete), ("final-fence", ApplicationRead::FenceAfterRead)] {
            let check = media_guard::Check::new(web_time::Instant::now());
            let mut rounds = Vec::new();
            for parallel in [false, true, true, false] {
                let mut samples = Vec::new();
                for index in 0..65 {
                    let start = Instant::now();
                    if parallel {
                        selected(&controls, &check, mode).unwrap();
                    } else {
                        for control in &controls {
                            match mode {
                                ApplicationRead::Complete => control.current(&check).unwrap(),
                                ApplicationRead::FenceAfterRead => control.fence(&check).unwrap(),
                            }
                        }
                    }
                    if index >= 5 {
                        samples.push(start.elapsed().as_micros() as u64);
                    }
                }
                rounds.push(serde_json::json!({"parallel":parallel,"samplesUs":samples}));
            }
            tests::same_bytes(&f);
            results.push(serde_json::json!({"mode":label,"rounds":rounds}));
        }
        println!(
            "{}",
            serde_json::json!({"kind":"original-retained-control-scheduling-pilot","controls":8,"sameActualControlFrames":true,"sameCallerCheckPerMode":true,"oneExtraOriginalObserverPermit":true,"noPayloadWritesDuringMeasurement":true,"wholeProducerNotMeasured":true,"fullGoalComplete":false,"results":results})
        );
    }
}
