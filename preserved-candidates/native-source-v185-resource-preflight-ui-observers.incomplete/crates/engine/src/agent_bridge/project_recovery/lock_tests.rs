//! The original guarded inventory probe shares readers and still refuses writers.
use super::*;
use std::{
    fs::File,
    io::Write,
    os::unix::fs::{OpenOptionsExt, symlink},
    sync::{
        Arc, Barrier,
        atomic::{AtomicU64, Ordering},
    },
};

struct Scratch(PathBuf);
impl Scratch {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = crate::temp_dir().join(format!("poema-inventory-lock-probe-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
    fn create_lock(&self) -> File {
        let mut file = OpenOptions::new().read(true).write(true).create_new(true).mode(0o600).open(self.0.join("lock")).unwrap();
        file.write_all(b"original guarded recovery lock\n").unwrap();
        file.sync_all().unwrap();
        file
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn recovery_inventory_read_probe_coexists_with_original_shared_readers_without_rewriting_evidence() {
    let scratch = Scratch::new();
    let reader = scratch.create_lock();
    let path = scratch.0.join("lock");
    let before = stamp(&fs::symlink_metadata(&path).unwrap()).unwrap();
    let bytes = fs::read(&path).unwrap();
    // The old exclusive inventory probe fails deterministically while this
    // other actual read-only probe holds its shared kernel lock.
    reader.try_lock_shared().unwrap();
    unlocked(&path).unwrap();
    let barrier = Arc::new(Barrier::new(5));
    let threads: Vec<_> = (0..4)
        .map(|_| {
            let path = path.clone();
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                barrier.wait();
                for _ in 0..500 {
                    unlocked(&path).unwrap();
                }
            })
        })
        .collect();
    barrier.wait();
    for thread in threads {
        thread.join().unwrap();
    }
    assert_eq!(stamp(&fs::symlink_metadata(&path).unwrap()).unwrap(), before);
    assert_eq!(fs::read(&path).unwrap(), bytes);
    drop(reader);
    unlocked(&path).unwrap();
}

#[test]
fn recovery_inventory_read_probe_refuses_exclusive_writer_missing_and_redirected_locks_without_repair() {
    let scratch = Scratch::new();
    let path = scratch.0.join("lock");
    assert!(unlocked(&path).is_err());
    assert!(!path.exists());
    let writer = scratch.create_lock();
    let before = stamp(&fs::symlink_metadata(&path).unwrap()).unwrap();
    let bytes = fs::read(&path).unwrap();
    writer.try_lock().unwrap();
    assert!(unlocked(&path).is_err());
    let other = path.clone();
    assert!(!std::thread::spawn(move || unlocked(&other).is_ok()).join().unwrap());
    let redirected = scratch.0.join("redirected");
    symlink(&path, &redirected).unwrap();
    assert!(unlocked(&redirected).is_err());
    assert_eq!(stamp(&fs::symlink_metadata(&path).unwrap()).unwrap(), before);
    assert_eq!(fs::read(&path).unwrap(), bytes);
    drop(writer);
    unlocked(&path).unwrap();
    assert!(unlocked(&redirected).is_err());
    assert_eq!(stamp(&fs::symlink_metadata(&path).unwrap()).unwrap(), before);
    assert_eq!(fs::read(&path).unwrap(), bytes);
}
