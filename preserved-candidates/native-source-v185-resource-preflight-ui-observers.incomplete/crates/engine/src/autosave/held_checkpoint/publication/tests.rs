//! Actual original-worker/frame composition. These callbacks are synthetic
//! source fences, not a fabricated Cold Input, controller or application grant.
use super::*;
use crate::autosave::held_checkpoint::tests::{encode, fixture, no_checkpoint_files, stop};
use std::{
    fs,
    io::Write,
    os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt},
    sync::atomic::AtomicUsize,
};
const EPOCH: &str = "abcdef0123456789abcdef0123456789";
const RESERVE: u64 = 1024 * 1024;
#[test]
fn controller_reply_wait_never_extends_the_original_deadline_even_at_zero_and_hostile_tails() {
    assert_eq!(controller_reply_wait(Duration::ZERO).unwrap(), Duration::from_millis(100));
    assert_eq!(controller_reply_wait(Duration::from_secs(44)).unwrap(), Duration::from_millis(100));
    assert_eq!(controller_reply_wait(Duration::from_secs(45) - Duration::from_nanos(1)).unwrap(), Duration::from_nanos(1));
    assert!(controller_reply_wait(Duration::from_secs(45)).is_err());
    assert!(controller_reply_wait(Duration::MAX).is_err());
}

#[test]
fn original_controller_wait_does_not_spin_full_source_reads_while_the_same_worker_is_blocked() {
    use std::{
        cell::RefCell,
        sync::{Mutex, mpsc::channel},
    };
    let (_d, mut s) = fixture(true);
    let encoded = encode(&mut s);
    let original = encoded_slices(&encoded).map(|(name, bytes)| (name, bytes.to_vec()));
    let memory = crate::project_save::guard(&s).unwrap();
    let worker = s.persistence.as_ref().unwrap();
    let selected = frame(worker);
    let path = selected.record().path;
    let caller = std::thread::current().id();
    let first = Arc::new(AtomicBool::new(true));
    let blocked = Arc::new(AtomicBool::new(false));
    let (entered_tx, entered_rx) = channel();
    let (release_tx, release_rx) = channel();
    let release_rx = Mutex::new(release_rx);
    let entered = blocked.clone();
    let live: Source = Arc::new(move || {
        if std::thread::current().id() != caller && first.swap(false, Ordering::AcqRel) {
            entered.store(true, Ordering::Release);
            entered_tx.send(()).map_err(io::Error::other)?;
            let result = release_rx.lock().unwrap().recv_timeout(Duration::from_secs(2));
            entered.store(false, Ordering::Release);
            result.map_err(io::Error::other)?;
        }
        Ok(())
    });
    let release = std::thread::spawn(move || {
        entered_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        std::thread::sleep(Duration::from_millis(350));
        release_tx.send(()).unwrap();
    });
    let observations = RefCell::new(Vec::new());
    let published = worker
        .publish_held_current_checkpoint(encoded, EPOCH, selected, RESERVE, live, || {
            memory.current(&s).map_err(|e| e.to_string())?;
            if blocked.load(Ordering::Acquire) {
                assert!(!path.exists(), "the real worker has not passed its first source check");
                observations.borrow_mut().push(std::time::Instant::now());
            }
            Ok(())
        })
        .unwrap();
    release.join().unwrap();
    let observed = observations.borrow();
    assert!(!observed.is_empty() && observed.len() <= 8, "controller repeatedly rescanned during a bounded 350ms worker wait: {}", observed.len());
    for pair in observed.windows(2) {
        assert!(pair[1].duration_since(pair[0]) >= Duration::from_millis(50));
    }
    for (name, bytes) in original {
        assert_eq!(fs::read(path.join("files").join(name)).unwrap(), bytes);
    }
    published.current(worker, &|| Ok(())).unwrap();
    no_checkpoint_files(worker);
    assert!(worker.startup_pending());
    assert!(worker.last_sent.is_none());
    stop(&mut s);
}

fn frame(worker: &Persistence) -> Arc<DirectoryFrame> {
    DirectoryFrame::capture(&worker.data_dir.join(format!(".poema-checkpoint-{EPOCH}")), &|| Ok(())).unwrap()
}
fn source() -> Source {
    Arc::new(|| Ok(()))
}
// Block the SAME real encoding worker before its first publication effect.
// This is a synthetic source fence, never a fabricated Cold owner or receipt.
fn blocked_checkpoint_source(on_enter: impl FnOnce() + Send + 'static) -> (Source, Arc<AtomicBool>, std::thread::JoinHandle<()>) {
    use std::sync::{Mutex, mpsc::channel};
    let caller = std::thread::current().id();
    let first = AtomicBool::new(true);
    let blocked = Arc::new(AtomicBool::new(false));
    let entered = blocked.clone();
    let (entered_tx, entered_rx) = channel();
    let (release_tx, release_rx) = channel();
    let release_rx = Mutex::new(release_rx);
    let source: Source = Arc::new(move || {
        if std::thread::current().id() != caller && first.swap(false, Ordering::AcqRel) {
            entered.store(true, Ordering::Release);
            entered_tx.send(()).map_err(io::Error::other)?;
            let result = release_rx.lock().unwrap().recv_timeout(Duration::from_secs(2));
            entered.store(false, Ordering::Release);
            result.map_err(io::Error::other)?;
        }
        Ok(())
    });
    let release = std::thread::spawn(move || {
        entered_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        on_enter();
        std::thread::sleep(Duration::from_millis(350));
        release_tx.send(()).unwrap();
    });
    (source, blocked, release)
}

#[test]
fn scoped_checkpoint_controller_wait_keeps_actual_worker_and_memory_without_repeating_complete_disk_observations() {
    use std::cell::{Cell, RefCell};
    let (_d, mut s) = fixture(true);
    let encoded = encode(&mut s);
    let expected = encoded_slices(&encoded).map(|(name, bytes)| (name, bytes.to_vec()));
    let worker = s.persistence.as_ref().unwrap();
    let selected = frame(worker);
    let path = selected.record().path;
    let memory = crate::project_save::guard(&s).unwrap();
    let (source, blocked, release) = blocked_checkpoint_source(|| {});
    let full_during_wait = Cell::new(0);
    let observations = RefCell::new(Vec::new());
    let published = worker
        .publish_held_current_checkpoint_with_wait(
            encoded,
            EPOCH,
            selected,
            RESERVE,
            source,
            &|| {
                memory.current(&s).map_err(|e| e.to_string())?;
                if blocked.load(Ordering::Acquire) {
                    full_during_wait.set(full_during_wait.get() + 1);
                }
                Ok(())
            },
            &|| {
                memory.current(&s).map_err(|e| e.to_string())?;
                if blocked.load(Ordering::Acquire) {
                    assert!(!path.exists());
                    observations.borrow_mut().push(std::time::Instant::now());
                }
                Ok(())
            },
        )
        .unwrap();
    release.join().unwrap();
    assert_eq!(full_during_wait.get(), 0);
    let observations = observations.borrow();
    assert!(!observations.is_empty() && observations.len() <= 8);
    for pair in observations.windows(2) {
        assert!(pair[1].duration_since(pair[0]) >= Duration::from_millis(50));
    }
    for (name, bytes) in expected {
        assert_eq!(fs::read(path.join("files").join(name)).unwrap(), bytes);
    }
    published.current(worker, &|| Ok(())).unwrap();
    assert!(worker.startup_pending());
    assert!(worker.last_sent.is_none());
    no_checkpoint_files(worker);
    stop(&mut s);
}

#[test]
fn scoped_checkpoint_controller_wait_refuses_shared_memory_revocation_and_the_same_source_stop() {
    for revoke_memory in [false, true] {
        let (_d, mut s) = fixture(true);
        let encoded = encode(&mut s);
        let memory = crate::project_save::guard(&s).unwrap();
        let worker = s.persistence.as_ref().unwrap();
        let selected = frame(worker);
        let path = selected.record().path;
        let stop_flag = Arc::new(AtomicBool::new(false));
        let stopped = stop_flag.clone();
        let previews = s.previews.clone();
        let (source, _blocked, release) = blocked_checkpoint_source(move || {
            if revoke_memory {
                previews.generation.fetch_add(1, Ordering::AcqRel);
            } else {
                stopped.store(true, Ordering::Release);
            }
        });
        let source_stop = stop_flag.clone();
        let original: Source = Arc::new(move || {
            source.current()?;
            if source_stop.load(Ordering::Acquire) {
                return Err(io::Error::other("same actual source Stop"));
            }
            Ok(())
        });
        let current = || {
            memory.current(&s).map_err(|e| e.to_string())?;
            if stop_flag.load(Ordering::Acquire) {
                return Err("same actual source Stop".to_owned());
            }
            Ok(())
        };
        let result = worker.publish_held_current_checkpoint_with_wait(encoded, EPOCH, selected, RESERVE, original, &current, &current);
        assert!(result.is_err());
        release.join().unwrap();
        assert!(!path.exists());
        unknown(worker);
        no_checkpoint_files(worker);
        stop(&mut s);
    }
}

#[test]
fn scoped_checkpoint_controller_final_source_fence_retains_actual_published_bytes_without_acknowledging_or_replaying() {
    let (d, mut s) = fixture(true);
    let original = d.0.join("original-selected-source");
    let retained = d.0.join("retained-selected-source");
    put(&original, b"same generated source bytes");
    let parent = DirectoryFrame::capture(&d.0, &|| Ok(())).unwrap();
    let pinned = Arc::new(parent.node("original-selected-source", &|| Ok(())).unwrap());
    let source_pin = pinned.clone();
    let source: Source = Arc::new(move || {
        source_pin.current(&|| Ok(()))?;
        source_pin.fence(&|| Ok(()))
    });
    let encoded = encode(&mut s);
    let expected = encoded_slices(&encoded).map(|(name, bytes)| (name, bytes.to_vec()));
    let memory = crate::project_save::guard(&s).unwrap();
    let worker = s.persistence.as_ref().unwrap();
    let selected = frame(worker);
    let path = selected.record().path;
    let replace_once = AtomicBool::new(true);
    let result = worker.publish_held_current_checkpoint_with_wait(
        encoded,
        EPOCH,
        selected,
        RESERVE,
        source,
        &|| {
            memory.current(&s).map_err(|e| e.to_string())?;
            if path.join("files").is_dir() && replace_once.swap(false, Ordering::AcqRel) {
                fs::rename(&original, &retained).unwrap();
                put(&original, b"same generated source bytes");
            }
            pinned.current(&|| Ok(())).map_err(|e| e.to_string())
        },
        &|| memory.current(&s).map_err(|e| e.to_string()),
    );
    assert!(result.is_err());
    assert!(!replace_once.load(Ordering::Acquire));
    assert_eq!(fs::read(&original).unwrap(), fs::read(&retained).unwrap());
    for (name, bytes) in expected {
        assert_eq!(fs::read(path.join("files").join(name)).unwrap(), bytes);
    }
    unknown(worker);
    no_checkpoint_files(worker);
    stop(&mut s);
}

#[test]
fn scoped_checkpoint_controller_retains_primary_wait_failure_and_final_source_failure() {
    let (_d, mut s) = fixture(true);
    let encoded = encode(&mut s);
    let worker = s.persistence.as_ref().unwrap();
    let selected = frame(worker);
    let path = selected.record().path;
    let (source, blocked, release) = blocked_checkpoint_source(|| {});
    let result = worker.publish_held_current_checkpoint_with_wait(
        encoded,
        EPOCH,
        selected,
        RESERVE,
        source,
        &|| if blocked.load(Ordering::Acquire) { Err("original final source refusal".to_owned()) } else { Ok(()) },
        &|| if blocked.load(Ordering::Acquire) { Err("original wait cancellation".to_owned()) } else { Ok(()) },
    );
    let message = result.err().unwrap();
    assert!(message.contains("original wait cancellation") && message.contains("original final source refusal"));
    release.join().unwrap();
    assert!(!path.exists());
    unknown(worker);
    no_checkpoint_files(worker);
    stop(&mut s);
}
fn put(path: &Path, bytes: &[u8]) {
    let mut f = fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(path).unwrap();
    f.write_all(bytes).unwrap();
    f.sync_all().unwrap();
}
fn unknown(worker: &Persistence) {
    assert!(worker.startup_checkpoint_publication_attempted.load(Ordering::Acquire));
    assert!(!worker.startup_checkpoint_publication_ready.load(Ordering::Acquire));
    assert!(worker.release_current_image(|_| Ok(()), || Ok(())).is_err());
    assert!(!worker.startup_attempted.load(Ordering::Acquire));
}
#[test]
fn same_original_worker_publishes_exact_three_opaque_checkpoint_bytes_and_actual_private_receipts_without_reserializing_or_consuming_fifo() {
    let (_d, mut s) = fixture(true);
    let encoded = encode(&mut s);
    let original = encoded_slices(&encoded).map(|(name, bytes)| (name, bytes.to_vec()));
    let worker = s.persistence.as_ref().unwrap();
    let root = frame(worker);
    let path = root.record().path.clone();
    let saved = fs::read(s.path.as_ref().unwrap()).unwrap();
    let expected_thread = encoded.thread;
    let observed = Arc::new(std::sync::Mutex::new(Vec::new()));
    let seen = observed.clone();
    let live: Source = Arc::new(move || {
        seen.lock().unwrap().push(std::thread::current().id());
        Ok(())
    });
    let published = worker.publish_held_current_checkpoint(encoded, EPOCH, root, RESERVE, live, || Ok(())).unwrap();
    assert!(observed.lock().unwrap().contains(&expected_thread));
    assert_eq!(published.encoded.thread, expected_thread);
    assert_eq!(published.encoded.description(worker).unwrap()["filesWritten"], false);
    let description = published.description(worker).unwrap();
    assert_eq!(description["filesWritten"], true);
    assert_eq!(description["durable"], true);
    assert_eq!(description["workerReleased"], false);
    assert_eq!(description["completeAuxiliaryReadback"], true);
    for (name, bytes) in original {
        let file = path.join("files").join(name);
        assert_eq!(fs::read(&file).unwrap(), bytes);
        assert_eq!(fs::metadata(file).unwrap().permissions().mode() & 0o7777, 0o600);
    }
    assert_eq!(published.publications.len(), 3);
    for p in &published.publications {
        assert!(p.receipt_durable);
        assert_eq!(p.receipt.outcome, Outcome::Published);
        assert!(!p.directory.join("exchange-slot.fcproj").exists());
        assert!(p.directory.join("payload.fcproj").exists());
        p.controls_current(&|| Ok(())).unwrap();
    }
    assert_eq!(fs::metadata(&path).unwrap().permissions().mode() & 0o7777, 0o700);
    assert_eq!(fs::read(s.path.as_ref().unwrap()).unwrap(), saved);
    no_checkpoint_files(worker);
    published.current(worker, &|| Ok(())).unwrap();
    stop(&mut s);
}
#[test]
fn complete_private_archive_survives_original_worker_resume_and_ordinary_clean_without_claiming_restart_selection() {
    let (_d, mut s) = fixture(false);
    let encoded = encode(&mut s);
    let worker = s.persistence.as_ref().unwrap();
    let published = worker.publish_held_current_checkpoint(encoded, EPOCH, frame(worker), RESERVE, source(), || Ok(())).unwrap();
    let path = published.root.pin().path.clone();
    let before = fs::read(path.join("files").join(crate::agent_bridge::NATIVE_JOURNAL_FILE)).unwrap();
    let release = worker.release_current_image(|_| Ok(()), || Ok(())).unwrap();
    worker.send_clean();
    worker.flush_current_image_pending_journal(&release, || Ok(())).unwrap();
    assert_eq!(fs::read(path.join("files").join(crate::agent_bridge::NATIVE_JOURNAL_FILE)).unwrap(), before);
    assert!(published.current(worker, &|| Ok(())).is_err());
    stop(&mut s);
}
#[test]
fn encoded_checkpoint_cannot_cross_actual_worker_or_publication_parent_before_effects() {
    let (_d, mut first) = fixture(true);
    let encoded = encode(&mut first);
    let (_e, mut second) = fixture(false);
    let worker = second.persistence.as_ref().unwrap();
    let path = frame(worker).record().path;
    assert!(worker.publish_held_current_checkpoint(encoded, EPOCH, frame(worker), RESERVE, source(), || Ok(())).is_err());
    assert!(!path.exists());
    assert!(!worker.startup_checkpoint_publication_attempted.load(Ordering::Acquire));
    stop(&mut second);
    stop(&mut first);
}
#[test]
fn existing_matching_archives_and_crossed_frames_epochs_or_reserves_are_preserved_without_publication() {
    for kind in 0..5 {
        let (_d, mut s) = fixture(true);
        let encoded = encode(&mut s);
        let worker = s.persistence.as_ref().unwrap();
        let original = frame(worker);
        let path = original.record().path;
        let selected = if kind == 0 {
            fs::DirBuilder::new().mode(0o700).create(&path).unwrap();
            put(&path.join("foreign.keep"), b"preserve");
            frame(worker)
        } else if kind == 1 {
            DirectoryFrame::capture(&worker.data_dir.join("foreign-archive"), &|| Ok(())).unwrap()
        } else {
            original
        };
        let epoch = if kind == 2 { "short" } else { EPOCH };
        let reserve = if kind == 3 {
            0
        } else if kind == 4 {
            u64::MAX
        } else {
            RESERVE
        };
        assert!(worker.publish_held_current_checkpoint(encoded, epoch, selected, reserve, source(), || Ok(())).is_err());
        assert!(!worker.startup_checkpoint_publication_attempted.load(Ordering::Acquire));
        if kind == 0 {
            assert_eq!(fs::read(path.join("foreign.keep")).unwrap(), b"preserve");
        } else {
            assert!(!path.exists());
        }
        stop(&mut s);
    }
}
#[test]
fn original_source_or_cow_loss_before_send_creates_no_archive_or_publication_attempt() {
    for lost_source in [false, true] {
        let (_d, mut s) = fixture(true);
        let encoded = encode(&mut s);
        let worker = s.persistence.as_ref().unwrap();
        let path = frame(worker).record().path;
        let live: Source = if lost_source { Arc::new(|| Err(io::Error::other("initial source lost"))) } else { source() };
        assert!(
            worker
                .publish_held_current_checkpoint(encoded, EPOCH, frame(worker), RESERVE, live, || if lost_source {
                    Ok(())
                } else {
                    Err("initial COW lost".into())
                })
                .is_err()
        );
        assert!(!path.exists());
        assert!(!worker.startup_checkpoint_publication_attempted.load(Ordering::Acquire));
        stop(&mut s);
    }
}
#[test]
fn source_loss_after_original_directory_effect_keeps_partial_archive_and_blocks_replay_and_release() {
    let (_d, mut s) = fixture(true);
    let encoded = encode(&mut s);
    let worker = s.persistence.as_ref().unwrap();
    let selected = frame(worker);
    let path = selected.record().path;
    let watched = path.clone();
    let live: Source = Arc::new(move || if watched.exists() { Err(io::Error::other("source lost after mkdir")) } else { Ok(()) });
    assert!(worker.publish_held_current_checkpoint(encoded, EPOCH, selected, RESERVE, live, || Ok(())).is_err());
    assert!(path.is_dir());
    assert!(!path.join("files").exists());
    unknown(worker);
    no_checkpoint_files(worker);
    stop(&mut s);
}
#[test]
fn caller_loss_during_a_worker_guard_aborts_before_another_kernel_effect_and_retains_unknown_state() {
    let (_d, mut s) = fixture(true);
    let encoded = encode(&mut s);
    let worker = s.persistence.as_ref().unwrap();
    let selected = frame(worker);
    let path = selected.record().path;
    let caller = std::thread::current().id();
    let seen = Arc::new(AtomicBool::new(false));
    let release = Arc::new(AtomicBool::new(false));
    let a = seen.clone();
    let b = release.clone();
    let live: Source = Arc::new(move || {
        if std::thread::current().id() != caller {
            a.store(true, Ordering::Release);
            let start = std::time::Instant::now();
            while !b.load(Ordering::Acquire) {
                if start.elapsed() > Duration::from_secs(2) {
                    return Err(io::Error::other("generated guarded wait expired"));
                }
                std::thread::sleep(Duration::from_millis(1));
            }
        }
        Ok(())
    });
    assert!(
        worker
            .publish_held_current_checkpoint(encoded, EPOCH, selected, RESERVE, live, || if seen.load(Ordering::Acquire) {
                Err("caller lost after enqueue".into())
            } else {
                Ok(())
            })
            .is_err()
    );
    release.store(true, Ordering::Release);
    unknown(worker);
    stop(&mut s);
    assert!(!path.exists());
}
#[test]
fn source_loss_after_actual_target_kernel_keeps_original_output_and_uncertain_receipt_without_cleanup() {
    let (_d, mut s) = fixture(true);
    let encoded = encode(&mut s);
    let expected = encoded.snapshot.clone();
    let worker = s.persistence.as_ref().unwrap();
    let selected = frame(worker);
    let path = selected.record().path;
    let target = path.join("files").join(crate::autosave::SNAPSHOT);
    let watch = target.clone();
    let live: Source = Arc::new(move || if watch.exists() { Err(io::Error::other("source lost after target kernel")) } else { Ok(()) });
    assert!(worker.publish_held_current_checkpoint(encoded, EPOCH, selected, RESERVE, live, || Ok(())).is_err());
    assert_eq!(fs::read(&target).unwrap(), expected);
    let receipt = path.join("writes").join(format!(".poema-save-{EPOCH}_snapshot.incomplete/receipt.json"));
    let value: Value = serde_json::from_slice(&fs::read(receipt).unwrap()).unwrap();
    assert_eq!(value["outcome"], "uncertain");
    assert_eq!(value["attempted"], true);
    assert!(!path.join("files").join(crate::autosave::META).exists());
    unknown(worker);
    stop(&mut s);
}
#[test]
fn bounded_original_namespace_rejects_and_preserves_foreign_work_after_root_creation() {
    let (_d, mut s) = fixture(true);
    let encoded = encode(&mut s);
    let worker = s.persistence.as_ref().unwrap();
    let selected = frame(worker);
    let path = selected.record().path;
    let watch = path.clone();
    let injected = Arc::new(AtomicBool::new(false));
    let done = injected.clone();
    let live: Source = Arc::new(move || {
        if watch.is_dir() && !done.swap(true, Ordering::AcqRel) {
            put(&watch.join("foreign.keep"), b"preserve later work");
        }
        Ok(())
    });
    assert!(worker.publish_held_current_checkpoint(encoded, EPOCH, selected, RESERVE, live, || Ok(())).is_err());
    assert!(injected.load(Ordering::Acquire));
    assert_eq!(fs::read(path.join("foreign.keep")).unwrap(), b"preserve later work");
    assert!(!path.join("files").exists());
    unknown(worker);
    stop(&mut s);
}
#[test]
fn matching_foreign_checkpoint_output_is_never_adopted_or_replaced_by_the_original_worker() {
    let (_d, mut s) = fixture(true);
    let encoded = encode(&mut s);
    let expected = encoded.snapshot.clone();
    let foreign = expected.clone();
    let worker = s.persistence.as_ref().unwrap();
    let selected = frame(worker);
    let path = selected.record().path;
    let target = path.join("files").join(crate::autosave::SNAPSHOT);
    let watch = target.clone();
    let injected = Arc::new(AtomicBool::new(false));
    let done = injected.clone();
    let live: Source = Arc::new(move || {
        if watch.parent().is_some_and(|p| p.is_dir()) && !done.swap(true, Ordering::AcqRel) {
            put(&watch, &foreign);
        }
        Ok(())
    });
    assert!(worker.publish_held_current_checkpoint(encoded, EPOCH, selected, RESERVE, live, || Ok(())).is_err());
    assert!(injected.load(Ordering::Acquire));
    assert_eq!(fs::read(target).unwrap(), expected);
    assert!(!path.join("writes").join(format!(".poema-save-{EPOCH}_snapshot.incomplete")).exists());
    unknown(worker);
    stop(&mut s);
}
#[test]
fn original_publication_controls_and_same_byte_output_generations_remain_fenced_after_success() {
    for controls in [false, true] {
        let (_d, mut s) = fixture(true);
        let encoded = encode(&mut s);
        let worker = s.persistence.as_ref().unwrap();
        let published = worker.publish_held_current_checkpoint(encoded, EPOCH, frame(worker), RESERVE, source(), || Ok(())).unwrap();
        let file = if controls { published.publications[0].directory.join("receipt.json") } else { published.outputs[0].path() };
        let original = fs::read(&file).unwrap();
        fs::rename(&file, _d.0.join("held-original-generation")).unwrap();
        put(&file, &original);
        assert!(published.current(worker, &|| Ok(())).is_err());
        assert_eq!(fs::read(file).unwrap(), original);
        stop(&mut s);
    }
}
#[test]
fn incomplete_auxiliary_or_crossed_checkpoint_metadata_refuses_before_directory_effects_and_cannot_release() {
    for native in [false, true] {
        let (_d, mut s) = fixture(true);
        let mut encoded = encode(&mut s);
        if native {
            encoded.native = b"{}".to_vec();
        } else {
            let mut metadata: Value = serde_json::from_slice(&encoded.metadata).unwrap();
            metadata["revision"] = json!(s.revision + 1);
            encoded.metadata = serde_json::to_vec(&metadata).unwrap();
        }
        let worker = s.persistence.as_ref().unwrap();
        let path = frame(worker).record().path;
        assert!(worker.publish_held_current_checkpoint(encoded, EPOCH, frame(worker), RESERVE, source(), || Ok(())).is_err());
        assert!(!path.exists());
        unknown(worker);
        stop(&mut s);
    }
}
#[test]
fn source_witness_is_retained_for_later_actual_publication_readback_without_granting_a_fresh_owner() {
    let (_d, mut s) = fixture(false);
    let encoded = encode(&mut s);
    let worker = s.persistence.as_ref().unwrap();
    let live = Arc::new(AtomicBool::new(true));
    let observed = live.clone();
    let calls = Arc::new(AtomicUsize::new(0));
    let counted = calls.clone();
    let source: Source = Arc::new(move || {
        counted.fetch_add(1, Ordering::Relaxed);
        if observed.load(Ordering::Acquire) { Ok(()) } else { Err(io::Error::other("retained source lost")) }
    });
    let published = worker.publish_held_current_checkpoint(encoded, EPOCH, frame(worker), RESERVE, source, || Ok(())).unwrap();
    assert!(calls.load(Ordering::Relaxed) > 10);
    live.store(false, Ordering::Release);
    assert!(published.current(worker, &|| Ok(())).is_err());
    assert!(published.description(worker).is_err());
    assert!(published.root.pin().path.is_dir());
    stop(&mut s);
}

#[test]
fn completed_checkpoint_keeps_its_entire_bounded_namespace_current_without_accepting_unknown_later_children() {
    for which in 0..4 {
        let (_d, mut s) = fixture(true);
        let encoded = encode(&mut s);
        let worker = s.persistence.as_ref().unwrap();
        let published = worker.publish_held_current_checkpoint(encoded, EPOCH, frame(worker), RESERVE, source(), || Ok(())).unwrap();
        let parent = match which {
            0 => published.root.pin().path.clone(),
            1 => published.files.pin().path.clone(),
            2 => published.writes.pin().path.clone(),
            _ => published.publications[0].directory.clone(),
        };
        let foreign = parent.join("foreign.keep");
        put(&foreign, b"preserve later foreign work");
        assert!(published.current(worker, &|| Ok(())).is_err());
        assert!(published.description(worker).is_err());
        assert_eq!(fs::read(foreign).unwrap(), b"preserve later foreign work");
        stop(&mut s);
    }
}
#[test]
fn original_publication_inspector_rejects_missing_changed_and_same_byte_replaced_independent_archive_payload() {
    for which in 0..3 {
        let (d, mut s) = fixture(true);
        let encoded = encode(&mut s);
        let expected = encoded.snapshot.clone();
        let worker = s.persistence.as_ref().unwrap();
        let published = worker.publish_held_current_checkpoint(encoded, EPOCH, frame(worker), RESERVE, source(), || Ok(())).unwrap();
        let payload = published.publications[0].directory.join("payload.fcproj");
        let held = d.0.join("held-original-payload");
        if which == 0 {
            fs::write(&payload, vec![b'x'; expected.len()]).unwrap();
        } else {
            fs::rename(&payload, &held).unwrap();
            if which == 1 {
                put(&payload, &expected);
            }
        }
        assert!(published.current(worker, &|| Ok(())).is_err());
        assert!(published.description(worker).is_err());
        assert_eq!(fs::read(published.outputs[0].path()).unwrap(), expected);
        if which != 0 {
            assert_eq!(fs::read(held).unwrap(), expected);
        }
        stop(&mut s);
    }
}
#[test]
fn original_payload_tamper_after_actual_receipt_never_becomes_a_ready_acknowledgement_even_when_target_bytes_match() {
    let (_d, mut s) = fixture(true);
    let encoded = encode(&mut s);
    let expected = encoded.snapshot.clone();
    let worker = s.persistence.as_ref().unwrap();
    let selected = frame(worker);
    let path = selected.record().path;
    let stage = path.join("writes").join(format!(".poema-save-{EPOCH}_snapshot.incomplete"));
    let corrupted = Arc::new(AtomicBool::new(false));
    let done = corrupted.clone();
    let live: Source = Arc::new(move || {
        if stage.join("receipt.json").exists() && !done.swap(true, Ordering::AcqRel) {
            let payload = stage.join("payload.fcproj");
            let length = fs::metadata(&payload).unwrap().len() as usize;
            fs::write(payload, vec![b'x'; length]).unwrap();
        }
        Ok(())
    });
    assert!(worker.publish_held_current_checkpoint(encoded, EPOCH, selected, RESERVE, live, || Ok(())).is_err());
    assert!(corrupted.load(Ordering::Acquire));
    assert_eq!(fs::read(path.join("files").join(crate::autosave::SNAPSHOT)).unwrap(), expected);
    unknown(worker);
    stop(&mut s);
}

#[test]
fn pure_preflight_source_loss_after_complete_read_refuses_before_attempt_send_or_directory_creation() {
    let (_d, mut s) = fixture(true);
    let encoded = encode(&mut s);
    let worker = s.persistence.as_ref().unwrap();
    let root = frame(worker);
    let path = root.record().path;
    let calls = Arc::new(AtomicUsize::new(0));
    let observed = calls.clone();
    let source =
        Arc::new(
            move || {
                if observed.fetch_add(1, Ordering::AcqRel) > 0 { Err(io::Error::other("original source revoked after pure preflight")) } else { Ok(()) }
            },
        );
    let result = worker.publish_held_current_checkpoint(encoded, EPOCH, root, RESERVE, source, || Ok(()));
    assert!(result.err().unwrap().contains("original source revoked after pure preflight"));
    assert!(calls.load(Ordering::Acquire) >= 2);
    assert!(!path.exists());
    assert!(!worker.startup_checkpoint_publication_attempted.load(Ordering::Acquire));
    assert!(worker.startup_pending());
    assert!(worker.last_sent.is_none());
    stop(&mut s);
}
#[test]
fn pure_preflight_caller_cow_loss_after_read_refuses_before_attempt_send_or_directory_creation() {
    let (_d, mut s) = fixture(true);
    let encoded = encode(&mut s);
    let worker = s.persistence.as_ref().unwrap();
    let root = frame(worker);
    let path = root.record().path;
    let calls = std::cell::Cell::new(0);
    let result = worker.publish_held_current_checkpoint(encoded, EPOCH, root, RESERVE, source(), || {
        let count = calls.get() + 1;
        calls.set(count);
        if count > 1 { Err("original caller COW changed during pure preflight".into()) } else { Ok(()) }
    });
    assert!(result.err().unwrap().contains("original caller COW changed during pure preflight"));
    assert!(calls.get() >= 2);
    assert!(!path.exists());
    assert!(!worker.startup_checkpoint_publication_attempted.load(Ordering::Acquire));
    assert!(worker.startup_pending());
    assert!(worker.last_sent.is_none());
    stop(&mut s);
}

#[test]
fn same_worker_source_revocation_at_pure_preflight_boundary_prevents_archive_and_keeps_attempt_unresolved() {
    let (_d, mut s) = fixture(true);
    let encoded = encode(&mut s);
    let original_thread = encoded.thread;
    let worker = s.persistence.as_ref().unwrap();
    let selected = frame(worker);
    let path = selected.record().path;
    let calls = Arc::new(AtomicUsize::new(0));
    let seen = calls.clone();
    let live: Source = Arc::new(move || {
        if std::thread::current().id() == original_thread && seen.fetch_add(1, Ordering::AcqRel) >= 2 {
            return Err(io::Error::other("same worker source revoked at pure read boundary"));
        }
        Ok(())
    });
    let error = worker.publish_held_current_checkpoint(encoded, EPOCH, selected, RESERVE, live, || Ok(())).err().unwrap();
    assert!(error.contains("same worker source revoked at pure read boundary"));
    assert!(calls.load(Ordering::Acquire) >= 3);
    assert!(!path.exists());
    unknown(worker);
    no_checkpoint_files(worker);
    stop(&mut s);
}

#[test]
fn same_worker_preflight_read_error_still_observes_the_final_full_source_fence_before_returning() {
    let (_d, mut s) = fixture(true);
    let mut encoded = encode(&mut s);
    let original_thread = encoded.thread;
    encoded.metadata = b"{".to_vec();
    let worker = s.persistence.as_ref().unwrap();
    let selected = frame(worker);
    let path = selected.record().path;
    let calls = Arc::new(AtomicUsize::new(0));
    let seen = calls.clone();
    let live: Source = Arc::new(move || {
        if std::thread::current().id() == original_thread && seen.fetch_add(1, Ordering::AcqRel) >= 3 {
            return Err(io::Error::other("same worker source revoked after malformed preflight"));
        }
        Ok(())
    });
    let error = worker.publish_held_current_checkpoint(encoded, EPOCH, selected, RESERVE, live, || Ok(())).err().unwrap();
    assert!(error.contains("same worker source revoked after malformed preflight"));
    assert!(calls.load(Ordering::Acquire) >= 4);
    assert!(!path.exists());
    unknown(worker);
    no_checkpoint_files(worker);
    stop(&mut s);
}

#[test]
fn read_only_worker_observation_checks_same_source_after_failure_and_does_not_authorize_an_effect() {
    let calls = std::cell::Cell::new(0);
    let source = || {
        calls.set(calls.get() + 1);
        if calls.get() == 2 { Err(io::Error::other("original source revoked")) } else { Ok(()) }
    };
    let result: io::Result<()> = observe(&source, || Err(io::Error::other("original read failed")));
    assert_eq!(calls.get(), 2);
    assert_eq!(result.unwrap_err().to_string(), "original source revoked");
}

#[test]
fn read_only_worker_observation_cannot_return_success_after_original_stop() {
    let live = std::cell::Cell::new(true);
    let source = || if live.get() { Ok(()) } else { Err(io::Error::other("original Stop")) };
    let result = observe(&source, || {
        live.set(false);
        Ok(())
    });
    assert_eq!(result.unwrap_err().to_string(), "original Stop");
}

// Synthetic query callbacks below carry no Cold source or startup authority.
#[derive(Default)]
struct ReadSourceProbe {
    complete: AtomicUsize,
    fenced: AtomicUsize,
    fail_complete: AtomicBool,
    fail_fence: AtomicBool,
}
impl CheckpointSource for ReadSourceProbe {
    fn current(&self) -> io::Result<()> {
        self.complete.fetch_add(1, Ordering::Relaxed);
        if self.fail_complete.load(Ordering::Acquire) { Err(io::Error::other("complete source refused")) } else { Ok(()) }
    }
    fn fence_after_read(&self) -> io::Result<()> {
        self.fenced.fetch_add(1, Ordering::Relaxed);
        if self.fail_fence.load(Ordering::Acquire) { Err(io::Error::other("same-source final fence refused")) } else { Ok(()) }
    }
}
#[test]
fn original_source_read_refusal_cannot_enter_metadata_or_substitute_a_final_fence_for_full_bytes() {
    let source = ReadSourceProbe::default();
    source.fail_complete.store(true, Ordering::Release);
    let entered = std::cell::Cell::new(false);
    assert!(
        source_read(&source, || {
            entered.set(true);
            Ok(())
        })
        .is_err()
    );
    assert!(!entered.get());
    assert_eq!(source.complete.load(Ordering::Acquire), 1);
    assert_eq!(source.fenced.load(Ordering::Acquire), 0);
}
#[test]
fn original_source_read_failure_still_runs_same_source_final_fence() {
    let source = ReadSourceProbe::default();
    source.fail_fence.store(true, Ordering::Release);
    let result: io::Result<()> = source_read(&source, || Err(io::Error::other("metadata read failed")));
    assert_eq!(result.unwrap_err().to_string(), "same-source final fence refused");
    assert_eq!(source.complete.load(Ordering::Acquire), 1);
    assert_eq!(source.fenced.load(Ordering::Acquire), 1);
}
#[test]
fn separate_original_read_boundaries_always_repeat_complete_source_validation() {
    let source = ReadSourceProbe::default();
    source_read(&source, || Ok(())).unwrap();
    source_read(&source, || Ok(())).unwrap();
    assert_eq!(source.complete.load(Ordering::Acquire), 2);
    assert_eq!(source.fenced.load(Ordering::Acquire), 2);
}
#[test]
fn original_function_source_adapter_retains_complete_callbacks_before_and_after_reads() {
    let calls = AtomicUsize::new(0);
    let source = || {
        calls.fetch_add(1, Ordering::Relaxed);
        Ok(())
    };
    source_read(&source, || Ok(())).unwrap();
    assert_eq!(calls.load(Ordering::Acquire), 2);
}
#[test]
fn original_worker_final_source_fence_failure_retains_created_archive_and_never_releases_or_replays() {
    struct AfterCreated {
        root: std::path::PathBuf,
        complete: Arc<AtomicUsize>,
        fenced: Arc<AtomicUsize>,
    }
    impl CheckpointSource for AfterCreated {
        fn current(&self) -> io::Result<()> {
            self.complete.fetch_add(1, Ordering::Relaxed);
            Ok(())
        }
        fn fence_after_read(&self) -> io::Result<()> {
            self.fenced.fetch_add(1, Ordering::Relaxed);
            if self.root.exists() { Err(io::Error::other("controlled final source generation refusal")) } else { Ok(()) }
        }
    }
    let (_d, mut session) = fixture(true);
    let encoded = encode(&mut session);
    let worker = session.persistence.as_ref().unwrap();
    let root = frame(worker);
    let path = root.record().path;
    let complete = Arc::new(AtomicUsize::new(0));
    let fenced = Arc::new(AtomicUsize::new(0));
    let source: Source = Arc::new(AfterCreated { root: path.clone(), complete: complete.clone(), fenced: fenced.clone() });
    assert!(worker.publish_held_current_checkpoint(encoded, EPOCH, root, RESERVE, source, || Ok(())).is_err());
    assert!(path.is_dir());
    assert_eq!(fs::read_dir(&path).unwrap().count(), 0);
    assert!(complete.load(Ordering::Acquire) > 0 && fenced.load(Ordering::Acquire) > 0);
    unknown(worker);
    no_checkpoint_files(worker);
    stop(&mut session);
}

#[test]
fn original_native_paired_read_checks_full_source_first_and_final_generation_after_failed_metadata() {
    let full = std::cell::Cell::new(0);
    let fenced = std::cell::Cell::new(0);
    let current = || {
        full.set(full.get() + 1);
        Ok(())
    };
    let after = || {
        fenced.set(fenced.get() + 1);
        Err(io::Error::other("same generation lost"))
    };
    let result: io::Result<()> = observe_with_fence(&current, &after, || Err(io::Error::other("metadata failed")));
    assert_eq!(result.unwrap_err().to_string(), "same generation lost");
    assert_eq!(full.get(), 1);
    assert_eq!(fenced.get(), 1);
}
