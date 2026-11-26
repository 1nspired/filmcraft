//! Reader probes share kernel locks; actual persistence writers remain exclusive.
use super::*;
use std::sync::{
    Barrier,
    atomic::{AtomicU64, Ordering},
};

struct Scratch(PathBuf);
impl Scratch {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = crate::temp_dir().join(format!("poema-recovery-lock-probe-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
    fn create_lock(&self) -> File {
        use std::os::unix::fs::OpenOptionsExt;
        let mut file = File::options().read(true).write(true).create_new(true).mode(0o600).open(self.0.join(LOCK)).unwrap();
        use std::io::Write;
        file.write_all(b"original recovery lock evidence\n").unwrap();
        file.sync_all().unwrap();
        file
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
#[cfg(unix)]
fn generation(path: &Path) -> (u64, u64, u32, u64, i64, i64, i64, i64) {
    use std::os::unix::fs::MetadataExt;
    let s = fs::symlink_metadata(path).unwrap();
    (s.dev(), s.ino(), s.mode(), s.len(), s.mtime(), s.mtime_nsec(), s.ctime(), s.ctime_nsec())
}

#[test]
#[cfg(unix)]
fn recovery_probe_shared_readers_do_not_report_each_other_as_live_writers() {
    let scratch = Scratch::new();
    let reader = scratch.create_lock();
    let lock = scratch.0.join(LOCK);
    let before = generation(&lock);
    let bytes = fs::read(&lock).unwrap();
    // Deterministic overlap: this exact shared probe stays locked while the
    // actual original reader runs. The old exclusive probe refuses here.
    reader.try_lock_shared().unwrap();
    assert!(lock_is_stale(&scratch.0));
    let barrier = Arc::new(Barrier::new(5));
    let threads: Vec<_> = (0..4)
        .map(|_| {
            let path = scratch.0.clone();
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                barrier.wait();
                for _ in 0..1000 {
                    assert!(lock_is_stale(&path), "another original reader cannot become a journal writer");
                }
            })
        })
        .collect();
    barrier.wait();
    for thread in threads {
        thread.join().unwrap();
    }
    assert_eq!(generation(&lock), before);
    assert_eq!(fs::read(&lock).unwrap(), bytes);
    drop(reader);
    assert!(lock_is_stale(&scratch.0));
}

#[test]
#[cfg(unix)]
fn recovery_probe_still_refuses_an_exclusive_writer_and_never_creates_or_rewrites_its_lock() {
    let scratch = Scratch::new();
    let lock = scratch.0.join(LOCK);
    assert!(lock_is_stale(&scratch.0));
    assert!(!lock.exists(), "read-only probes must not create missing lock files");
    let writer = scratch.create_lock();
    let before = generation(&lock);
    let bytes = fs::read(&lock).unwrap();
    writer.try_lock().unwrap();
    assert!(!lock_is_stale(&scratch.0));
    let path = scratch.0.clone();
    assert!(!std::thread::spawn(move || lock_is_stale(&path)).join().unwrap());
    assert_eq!(generation(&lock), before);
    assert_eq!(fs::read(&lock).unwrap(), bytes);
    drop(writer);
    assert!(lock_is_stale(&scratch.0));
    assert_eq!(generation(&lock), before);
    assert_eq!(fs::read(&lock).unwrap(), bytes);
}
