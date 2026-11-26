//! Real original worker, serializers, archive publisher and release exchange.
//! Source callbacks here are generated read-only component fences, not an
//! actual Cold Input, source handoff or application admission capability.
use super::*;
use crate::{
    Session,
    autosave::held_checkpoint::tests::{encode, fixture, stop},
};
use std::{cell::Cell, fs, io::Write, os::unix::fs::OpenOptionsExt, sync::atomic::AtomicUsize};

const EPOCH: &str = "abcdef0123456789abcdef0123456789";
fn source() -> Source {
    Arc::new(|| Ok(()))
}
fn publish(s: &mut Session) -> Published {
    let encoded = encode(s);
    let p = s.persistence.as_ref().unwrap();
    let frame = DirectoryFrame::capture(&p.data_dir.join(format!(".poema-checkpoint-{EPOCH}")), &|| Ok(())).unwrap();
    p.publish_held_current_checkpoint(encoded, EPOCH, frame, 1024 * 1024, source(), || Ok(())).unwrap()
}
fn saved(p: &ReleasePreparation) -> Vec<(std::path::PathBuf, Vec<u8>)> {
    p.published.outputs.iter().map(|n| (n.path().to_owned(), fs::read(n.path()).unwrap())).collect()
}
fn unchanged(files: &[(std::path::PathBuf, Vec<u8>)]) {
    for (p, b) in files {
        assert_eq!(fs::read(p).unwrap(), *b);
    }
}
fn put(p: &std::path::Path, b: &[u8]) {
    let mut f = fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(p).unwrap();
    f.write_all(b).unwrap();
    f.sync_all().unwrap();
}

#[test]
fn release_preparation_consumes_actual_publisher_before_attempt_and_retains_complete_original_bytes_without_fifo_or_release() {
    let (_d, mut s) = fixture(true);
    let published = publish(&mut s);
    let worker = s.persistence.as_ref().unwrap();
    let memory = crate::project_save::guard(&s).unwrap();
    let saved_project = fs::read(s.path.as_ref().unwrap()).unwrap();
    let prepared = published.prepare_release(worker, &|| Ok(())).unwrap();
    assert!(worker.startup_pending());
    assert!(!worker.startup_attempted.load(Ordering::Acquire));
    assert!(worker.last_sent.is_none());
    assert!(Arc::ptr_eq(&prepared.original_data, &prepared.published.data));
    let expected = prepared.expected_files(worker).unwrap();
    assert_eq!(expected[0].1, filmcraft_format::encode(&s.project, false));
    crate::agent_bridge::validate_held_checkpoint_bytes(expected[2].1, expected[0].1).unwrap();
    for (name, bytes) in expected {
        assert_eq!(fs::read(prepared.published.files.pin().path.join(name)).unwrap(), bytes);
        assert!(!worker.session_dir.join(name).exists());
    }
    prepared.inspect_transition(worker, source().as_ref(), &|| Ok(())).unwrap();
    memory.current(&s).unwrap();
    assert_eq!(fs::read(s.path.as_ref().unwrap()).unwrap(), saved_project);
    stop(&mut s);
}

#[test]
fn release_preparation_fences_actual_prepare_and_resume_then_requires_same_original_ack_without_reusing_held_reader() {
    let (_d, mut s) = fixture(true);
    let published = publish(&mut s);
    let worker = s.persistence.as_ref().unwrap();
    let prepared = published.prepare_release(worker, &|| Ok(())).unwrap();
    let before = Cell::new(0);
    let after = Cell::new(0);
    let attempted_seen = Cell::new(false);
    let original = saved(&prepared);
    let ack = worker
        .release_current_image(
            |_| {
                before.set(before.get() + 1);
                if worker.startup_attempted.load(Ordering::Acquire) {
                    attempted_seen.set(true);
                    assert!(prepared.published.encoded.current(worker).is_err());
                }
                prepared.inspect_transition(worker, source().as_ref(), &|| Ok(()))
            },
            || {
                after.set(after.get() + 1);
                prepared.inspect_transition(worker, source().as_ref(), &|| Ok(()))
            },
        )
        .unwrap();
    assert!(before.get() >= 3 && after.get() >= 2 && attempted_seen.get());
    assert!(!worker.startup_pending());
    assert!(prepared.published.encoded.current(worker).is_err());
    assert!(prepared.published.current(worker, &|| Ok(())).is_err());
    assert!(prepared.published.encoded.journal.current(&|| Ok(())).is_err());
    prepared.inspect_acknowledged_release(worker, &ack, source().as_ref(), &|| Ok(())).unwrap();
    unchanged(&original);
    assert!(worker.release_current_image(|_| Ok(()), || Ok(())).is_err());
    stop(&mut s);
}

#[test]
fn release_preparation_cannot_be_created_after_prepare_attempt_even_if_original_checkpoint_bytes_still_match() {
    let (_d, mut s) = fixture(true);
    let published = publish(&mut s);
    let worker = s.persistence.as_ref().unwrap();
    let files = published.outputs.iter().map(|n| (n.path().to_owned(), fs::read(n.path()).unwrap())).collect::<Vec<_>>();
    let ack = worker.release_current_image(|_| Ok(()), || Ok(())).unwrap();
    worker.assert_current_image_release(&ack).unwrap();
    assert!(published.prepare_release(worker, &|| Ok(())).is_err());
    unchanged(&files);
    stop(&mut s);
}

#[test]
fn release_preparation_refuses_crossed_worker_and_real_release_receipts_and_invalidates_after_original_stop() {
    let (_d, mut first) = fixture(true);
    let published = publish(&mut first);
    let worker = first.persistence.as_ref().unwrap();
    let prepared = published.prepare_release(worker, &|| Ok(())).unwrap();
    let ack = worker.release_current_image(|_| Ok(()), || Ok(())).unwrap();
    let (_other, mut second) = fixture(false);
    let other = second.persistence.as_ref().unwrap();
    let other_ack = other.release_current_image(|_| Ok(()), || Ok(())).unwrap();
    assert!(prepared.inspect_transition(other, source().as_ref(), &|| Ok(())).is_err());
    assert!(prepared.inspect_acknowledged_release(other, &ack, source().as_ref(), &|| Ok(())).is_err());
    assert!(prepared.inspect_acknowledged_release(worker, &other_ack, source().as_ref(), &|| Ok(())).is_err());
    assert!(prepared.expected_files(other).is_err());
    stop(&mut second);
    stop(&mut first);
    let worker = first.persistence.as_ref().unwrap();
    assert!(prepared.expected_files(worker).is_err());
    assert!(prepared.inspect_transition(worker, source().as_ref(), &|| Ok(())).is_err());
    assert!(prepared.inspect_acknowledged_release(worker, &ack, source().as_ref(), &|| Ok(())).is_err());
}

#[test]
fn release_preparation_keeps_full_receipt_payload_namespace_and_generation_checks_after_original_resume() {
    for kind in ["equal-output-generation", "equal-payload-generation", "unknown-member"] {
        let (d, mut s) = fixture(true);
        let published = publish(&mut s);
        let worker = s.persistence.as_ref().unwrap();
        let prepared = published.prepare_release(worker, &|| Ok(())).unwrap();
        let ack = worker.release_current_image(|_| Ok(()), || Ok(())).unwrap();
        let output = prepared.published.outputs[0].path().to_owned();
        let bytes = fs::read(&output).unwrap();
        if kind == "unknown-member" {
            put(&prepared.published.files.pin().path.join("foreign.json"), b"generated unknown work");
        } else {
            let target = if kind == "equal-output-generation" { output.clone() } else { prepared.published.publications[0].directory.join("payload.fcproj") };
            let held = d.0.join(format!("retained-{kind}.bin"));
            let original = fs::read(&target).unwrap();
            fs::rename(&target, &held).unwrap();
            put(&target, &original);
            assert_eq!(fs::read(&held).unwrap(), fs::read(&target).unwrap());
        }
        assert!(prepared.inspect_acknowledged_release(worker, &ack, source().as_ref(), &|| Ok(())).is_err());
        assert_eq!(fs::read(&output).unwrap(), bytes);
        stop(&mut s);
    }
}

struct ObservedSource {
    node: NodeFrame,
    reads: AtomicUsize,
    fences: AtomicUsize,
}
impl CheckpointSource for ObservedSource {
    fn current(&self) -> io::Result<()> {
        self.reads.fetch_add(1, Ordering::SeqCst);
        self.node.current(&|| Ok(()))?;
        self.node.fence(&|| Ok(()))
    }
    fn fence_after_read(&self) -> io::Result<()> {
        self.fences.fetch_add(1, Ordering::SeqCst);
        self.node.fence(&|| Ok(()))
    }
}
#[test]
fn release_preparation_requires_full_source_before_and_final_original_fence_after_success_and_failed_archive_reads() {
    let (d, mut s) = fixture(true);
    let published = publish(&mut s);
    let worker = s.persistence.as_ref().unwrap();
    let prepared = published.prepare_release(worker, &|| Ok(())).unwrap();
    let ack = worker.release_current_image(|_| Ok(()), || Ok(())).unwrap();
    put(&d.0.join("source.json"), b"{\"source\":\"original\"}\n");
    let parent = DirectoryFrame::capture(&d.0, &|| Ok(())).unwrap();
    let original =
        ObservedSource { node: parent.node_bounded("source.json", 1024, &|| Ok(())).unwrap(), reads: AtomicUsize::new(0), fences: AtomicUsize::new(0) };
    prepared.inspect_acknowledged_release(worker, &ack, &original, &|| Ok(())).unwrap();
    assert_eq!(original.reads.load(Ordering::SeqCst), 1);
    assert_eq!(original.fences.load(Ordering::SeqCst), 1);
    put(&prepared.published.files.pin().path.join("foreign.json"), b"generated foreign work");
    assert!(prepared.inspect_acknowledged_release(worker, &ack, &original, &|| Ok(())).is_err());
    assert_eq!(original.reads.load(Ordering::SeqCst), 2);
    assert_eq!(original.fences.load(Ordering::SeqCst), 2);
    let bytes = fs::read(d.0.join("source.json")).unwrap();
    fs::rename(d.0.join("source.json"), d.0.join("retained-source.json")).unwrap();
    put(&d.0.join("source.json"), &bytes);
    assert!(prepared.inspect_acknowledged_release(worker, &ack, &original, &|| Ok(())).is_err());
    assert_eq!(original.reads.load(Ordering::SeqCst), 3);
    assert_eq!(original.fences.load(Ordering::SeqCst), 2);
    stop(&mut s);
}

#[test]
fn release_preparation_negative_after_resume_fence_does_not_mint_or_replay_a_failed_original_exchange() {
    let (_d, mut s) = fixture(true);
    let published = publish(&mut s);
    let worker = s.persistence.as_ref().unwrap();
    let prepared = published.prepare_release(worker, &|| Ok(())).unwrap();
    let files = saved(&prepared);
    let result = worker.release_current_image(
        |_| prepared.inspect_transition(worker, source().as_ref(), &|| Ok(())),
        || {
            prepared.inspect_transition(worker, source().as_ref(), &|| Ok(()))?;
            Err("controlled original after-Resume source refusal".into())
        },
    );
    assert!(result.is_err());
    assert!(worker.startup_attempted.load(Ordering::Acquire));
    assert!(worker.release_current_image(|_| Ok(()), || Ok(())).is_err());
    unchanged(&files);
    stop(&mut s);
}

#[test]
fn release_preparation_keeps_original_stop_deadline_and_same_worker_paths_without_writing_or_rebaselining() {
    let (_d, mut s) = fixture(true);
    let published = publish(&mut s);
    let worker = s.persistence.as_ref().unwrap();
    let prepared = published.prepare_release(worker, &|| Ok(())).unwrap();
    let files = saved(&prepared);
    for message in ["original Stop", "original bounded deadline"] {
        assert!(prepared.inspect_transition(worker, source().as_ref(), &|| Err(io::Error::other(message))).unwrap_err().contains(message));
        unchanged(&files);
    }
    let original = s.persistence.as_ref().unwrap().data_dir.clone();
    s.persistence.as_mut().unwrap().data_dir = original.join("foreign");
    assert!(prepared.inspect_transition(s.persistence.as_ref().unwrap(), source().as_ref(), &|| Ok(())).is_err());
    s.persistence.as_mut().unwrap().data_dir = original;
    prepared.inspect_transition(s.persistence.as_ref().unwrap(), source().as_ref(), &|| Ok(())).unwrap();
    unchanged(&files);
    stop(&mut s);
}
