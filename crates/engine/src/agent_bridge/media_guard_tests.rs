use super::*;
use crate::agent_bridge::{
    assembly_tests, condense_tests,
    tests::{exact_batch, read, token},
};
use filmcraft_project::{ItemId, MediaRef};
use serde_json::{Value, json};
use std::{
    fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicUsize, Ordering},
};

static NEXT: AtomicUsize = AtomicUsize::new(0);

#[test]
fn requested_owner_generation_is_hashed_once_and_never_inherited_by_a_changed_read() {
    let (s, d, bytes) = two_originals();
    let plan = Plan::prepare(&s.project, &s.media).unwrap();
    let hashed = hashed_capture(&plan);
    let row = hashed.rows.first().unwrap();
    let State::Regular(stamp, Some(hash)) = &row.state else { panic!("regular hashed original required") };
    // Model the normal precise-identity native read, which does not hash until
    // a caller needs correspondence with an external owner's generation.
    let expected = Witnesses {
        scope: Some(vec![row.item]),
        rows: vec![Row { item: row.item, path: row.path.clone(), state: State::Regular(stamp.clone(), None) }],
        ..Witnesses::default()
    };
    let check = Check::new(web_time::Instant::now());
    expected.require_generation(row.item, &row.path, stamp.bytes, hash, &check).unwrap();
    assert_eq!(check.bytes.load(Ordering::Relaxed), stamp.bytes);
    let cached = Check::new(web_time::Instant::now());
    expected.require_generation(row.item, &row.path, stamp.bytes, hash, &cached).unwrap();
    assert_eq!(cached.bytes.load(Ordering::Relaxed), 0);
    assert!(expected.require_generation(row.item, &row.path, stamp.bytes, &"0".repeat(64), &cached).is_err());
    assert!(expected.require_generation(row.item + 10000, &row.path, stamp.bytes, hash, &cached).is_err());
    let mut changed = bytes[0].clone();
    *changed.last_mut().unwrap() ^= 17;
    fs::write(d.file(), changed).unwrap();
    assert!(validate(&s.project, &s.media, &expected).is_err());
    let fresh = Plan::prepare(&s.project, &s.media).unwrap().capture(&Check::new(web_time::Instant::now())).unwrap();
    assert!(fresh.require_generation(row.item, &row.path, stamp.bytes, hash, &Check::new(web_time::Instant::now())).is_err());
}

fn hashed_capture(plan: &Plan) -> Arc<Witnesses> {
    let check = Check::new(web_time::Instant::now());
    let inputs = plan.inputs(&check).unwrap();
    plan.capture_inputs(&inputs, &check, false, &|p, c| observe(p, c, true)).unwrap()
}

#[test]
fn retained_validation_reads_every_byte_once_without_issuing_another_witness() {
    let _serial = parallel::TEST_SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let (s, _d, bytes) = two_originals();
    let initial = before(&s);
    let plan = Plan::prepare(&s.project, &s.media).unwrap();
    let expected = hashed_capture(&plan);
    let check = Check::new(web_time::Instant::now());
    let inputs = plan.inputs(&check).unwrap();
    plan.recheck_inputs(&inputs, &[&expected], &check, true, &|p, c| observe(p, c, true)).unwrap();
    assert_eq!(check.files.load(Ordering::Relaxed), 2);
    assert_eq!(check.bytes.load(Ordering::Relaxed), bytes.iter().map(|b| b.len() as u64).sum::<u64>());
    assert_eq!(plan.total(), 4, "issuing an initial witness still needs two complete observations");
    assert_eq!(plan.validation_total(), 2);
    assert_eq!(Arc::strong_count(&expected), 1);
    assert_eq!(before(&s), initial);
    let empty = Check::new(web_time::Instant::now());
    assert!(plan.recheck_inputs(&inputs, &[], &empty, false, &|p, c| observe(p, c, true)).is_err());
    assert_eq!(empty.bytes.load(Ordering::Relaxed), 0);
    assert!(plan.recheck_inputs(&inputs[..1], &[&expected], &empty, false, &|p, c| observe(p, c, true)).is_err());
    assert_eq!(empty.bytes.load(Ordering::Relaxed), 0);
}

#[test]
fn retained_validation_detects_a_tail_change_even_when_the_full_metadata_identity_collides() {
    let (s, d, mut bytes) = two_originals();
    let plan = Plan::prepare(&s.project, &s.media).unwrap();
    let expected = hashed_capture(&plan);
    bytes[0][1024 * 1024] ^= 127;
    fs::write(d.file(), &bytes[0]).unwrap();
    let check = Check::new(web_time::Instant::now());
    let inputs = plan.inputs(&check).unwrap();
    let result = plan.recheck_inputs(&inputs, &[&expected], &check, false, &|path, c| {
        let mut state = observe(path, c, true)?;
        // Model a coarse filesystem identity collision. The complete current
        // hash must still differ, including bytes beyond the first MiB.
        if let State::Regular(stamp, _) = &mut state {
            let State::Regular(prior, _) = &expected.rows.iter().find(|r| Path::new(&r.path) == path).unwrap().state else {
                panic!("regular original required")
            };
            stamp.modified = prior.modified;
            stamp.created = prior.created;
            #[cfg(unix)]
            {
                stamp.unix = prior.unix;
            }
        }
        Ok(state)
    });
    assert!(result.unwrap_err().to_string().contains("original file inputs changed"));
    assert_eq!(check.bytes.load(Ordering::Relaxed), bytes.iter().map(|b| b.len() as u64).sum::<u64>());
}

#[test]
fn retained_validation_compares_every_old_generation_in_the_complete_union() {
    let (s, d, _) = two_originals();
    let all = Plan::prepare(&s.project, &s.media).unwrap();
    let left_plan = Plan::scoped(&s.project, &s.media, vec![all.files[0].0]).unwrap();
    let right_plan = Plan::scoped(&s.project, &s.media, vec![all.files[1].0]).unwrap();
    let left = hashed_capture(&left_plan);
    let right = hashed_capture(&right_plan);
    let plan = Plan::for_witnesses(&s.project, &s.media, &[left.clone(), right.clone()]).unwrap();
    let check = Check::new(web_time::Instant::now());
    let inputs = plan.inputs(&check).unwrap();
    plan.recheck_inputs(&inputs, &[&left, &right], &check, false, &|p, c| observe(p, c, true)).unwrap();
    assert_eq!(check.files.load(Ordering::Relaxed), 2);
    let incomplete = Check::new(web_time::Instant::now());
    assert!(left_plan.recheck_inputs(&left_plan.inputs(&incomplete).unwrap(), &[&left, &right], &incomplete, false, &|p, c| observe(p, c, true)).is_err());
    fs::write(d.0.join("second.bin"), b"new candidate generation cannot renew the previous snapshot").unwrap();
    let fresh = hashed_capture(&right_plan);
    let check = Check::new(web_time::Instant::now());
    assert!(plan.recheck_inputs(&plan.inputs(&check).unwrap(), &[&left, &right, &fresh], &check, false, &|p, c| observe(p, c, true)).is_err());
}

#[test]
fn retained_validation_stop_and_deadline_cannot_return_current_or_create_tickets() {
    for mode in ["before", "during", "deadline"] {
        let (s, _d, _) = two_originals();
        let initial = before(&s);
        let plan = Plan::prepare(&s.project, &s.media).unwrap();
        let expected = hashed_capture(&plan);
        let inputs = plan.inputs(&Check::new(web_time::Instant::now())).unwrap();
        let check = Check::new(web_time::Instant::now() - if mode == "deadline" { std::time::Duration::from_secs(46) } else { std::time::Duration::ZERO });
        if mode == "before" {
            check.stop.store(true, Ordering::Release);
        }
        assert!(
            plan.recheck_inputs(&inputs, &[&expected], &check, false, &|p, c| {
                let state = observe(p, c, true)?;
                if mode == "during" {
                    c.stop.store(true, Ordering::Release);
                }
                Ok(state)
            })
            .is_err()
        );
        assert_eq!(before(&s), initial);
        assert!(s.agent_bridge.tickets.is_empty());
    }
}

fn two_originals() -> (crate::Session, Scratch, Vec<Vec<u8>>) {
    let (mut s, d) = setup();
    let a = s.project.items.values().find(|i| i.metadata.contains_key(crate::agent_bridge::BINDING_KEY)).unwrap().clone();
    let mut b = a.clone();
    b.id = ItemId(1_500_000);
    b.name = "Second original".into();
    b.metadata
        .insert(crate::agent_bridge::BINDING_KEY.into(), json!({"projectId":"p_test","assetId":"a_second","mediaId":"second","role":"original"}).to_string());
    b.as_media_mut().unwrap().media = MediaRef::File { path: d.0.join("second.bin").to_string_lossy().into_owned() };
    Arc::make_mut(&mut s.project).items.insert(b.id, b);
    let bytes: Vec<Vec<u8>> = [17, 31]
        .into_iter()
        .map(|seed| {
            let mut bytes = vec![seed; 1024 * 1024 + 1];
            bytes[1024 * 1024] = seed + 1;
            bytes
        })
        .collect();
    fs::write(d.file(), &bytes[0]).unwrap();
    fs::write(d.0.join("second.bin"), &bytes[1]).unwrap();
    (s, d, bytes)
}

#[test]
fn parallel_capture_keeps_two_complete_byte_observations_and_exact_input_order() {
    use sha2::{Digest, Sha256};
    let _serial = parallel::TEST_SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let (s, _d, bytes) = two_originals();
    let plan = Plan::prepare(&s.project, &s.media).unwrap();
    let check = Check::new(web_time::Instant::now());
    let inputs = plan.inputs(&check).unwrap();
    let witness = plan.capture_inputs(&inputs, &check, true, &|path, check| observe(path, check, true)).unwrap();
    assert_eq!(check.files.load(Ordering::Relaxed), 4);
    assert_eq!(check.bytes.load(Ordering::Relaxed), bytes.iter().map(|b| b.len() as u64).sum::<u64>() * 2);
    assert_eq!(witness.rows.iter().map(|r| r.item).collect::<Vec<_>>(), inputs.iter().map(|r| r.item).collect::<Vec<_>>());
    for (row, data) in witness.rows.iter().zip(bytes) {
        let State::Regular(_, Some(hash)) = &row.state else { panic!("complete hash missing") };
        assert_eq!(*hash, format!("{:x}", Sha256::digest(data)));
    }
    assert!(plan.capture_inputs(&inputs[..1], &Check::new(web_time::Instant::now()), true, &|p, c| observe(p, c, true)).is_err());
}

#[test]
fn parallel_capture_refuses_changed_second_observations_and_stop_without_partial_witnesses() {
    let _serial = parallel::TEST_SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    for stop in [false, true] {
        let (s, d, _bytes) = two_originals();
        let plan = Plan::prepare(&s.project, &s.media).unwrap();
        let check = Check::new(web_time::Instant::now());
        let inputs = plan.inputs(&check).unwrap();
        let visits = AtomicUsize::new(0);
        let result = plan.capture_inputs(&inputs, &check, true, &|path, c| {
            if path == d.file() && visits.fetch_add(1, Ordering::SeqCst) == 1 {
                if stop {
                    c.stop.store(true, Ordering::Release);
                } else {
                    fs::write(path, b"late replaced original").unwrap();
                }
            }
            observe(path, c, true)
        });
        assert!(result.is_err());
        assert_eq!(visits.load(Ordering::SeqCst), 2);
    }
}

#[test]
fn cancellation_after_a_byte_chunk_refuses_a_partial_hash_before_the_next_read() {
    struct StopAfterChunk<'a> {
        check: &'a Check,
        calls: usize,
    }
    impl std::io::Read for StopAfterChunk<'_> {
        fn read(&mut self, b: &mut [u8]) -> std::io::Result<usize> {
            self.calls += 1;
            b[0] = 1;
            self.check.stop.store(true, Ordering::Release);
            Ok(1)
        }
    }
    let check = Check::new(web_time::Instant::now());
    let mut reader = StopAfterChunk { check: &check, calls: 0 };
    assert!(hash_bytes(&mut reader, 10, &check).is_err());
    assert_eq!(reader.calls, 1);
    assert_eq!(check.bytes.load(Ordering::Relaxed), 1);
}
struct Scratch(PathBuf);

#[test]
fn independent_operation_cancel_interrupts_the_same_complete_byte_guard() {
    struct CancelAfterChunk {
        control: crate::AgentOperationControl,
        token: String,
        calls: usize,
    }
    impl std::io::Read for CancelAfterChunk {
        fn read(&mut self, b: &mut [u8]) -> std::io::Result<usize> {
            self.calls += 1;
            b[0] = 1;
            assert_eq!(self.control.cancel(&json!({"token":self.token})).unwrap()["cancellationAccepted"], true);
            Ok(1)
        }
    }
    let (mut s, _) = assembly_tests::fixture();
    read(&mut s, 71009);
    let token = token(71009);
    let control = s.agent_operation_control();
    let result = crate::agent_bridge::operation_control::run(&mut s, "agent.apply", &json!({"token":token}), |s, _| {
        let check = s.agent_bridge.operations.check().unwrap();
        let mut reader = CancelAfterChunk { control: control.clone(), token: token.clone(), calls: 0 };
        assert!(hash_bytes(&mut reader, 10, &check).is_err());
        assert_eq!(reader.calls, 1);
        assert!(s.agent_bridge.operations.enter_publication(&token).is_err());
        Err(crate::commands::bad("agent.apply", "generated cancellation verified"))
    });
    assert!(result.is_err());
    assert!(s.agent_bridge.operations.check().is_none());
}
impl Scratch {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "filmcraft-media-guard-{}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
    fn file(&self) -> PathBuf {
        self.0.join("original.bin")
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn bind(s: &mut crate::Session, file: &Path) {
    let id = s.project.items.values().find(|i| i.metadata.contains_key(crate::agent_bridge::BINDING_KEY)).unwrap().id;
    Arc::make_mut(&mut s.project).item_mut(id).unwrap().as_media_mut().unwrap().media = MediaRef::File { path: file.to_string_lossy().into_owned() };
}
fn setup() -> (crate::Session, Scratch) {
    let (mut s, _) = assembly_tests::fixture();
    let dir = Scratch::new();
    fs::write(dir.file(), b"original bytes").unwrap();
    bind(&mut s, &dir.file());
    (s, dir)
}
fn split(s: &crate::Session, n: usize) -> Value {
    let c = &s.active_sequence().unwrap().video_tracks[0].items[0];
    exact_batch(n, vec![json!({"command":"timeline.razor","params":{"clip":c.id.0,"time":c.start.0+s.sequence_rate().tick_of(1).0}})])
}
fn before(s: &crate::Session) -> Value {
    json!({"project":s.project,"state":s.state,"revision":s.revision,"saved":s.saved_revision,"undo":s.history.undo.len(),"redo":s.history.redo.len(),"merge":s.history.merge_key,"journal":s.journal,"events":s.events.len()})
}

#[test]
fn same_size_replacement_with_restored_modification_time_refuses_without_any_native_edit() {
    let (mut s, d) = setup();
    read(&mut s, 1);
    let req = split(&s, 1);
    let previous = before(&s);
    let modified = fs::metadata(d.file()).unwrap().modified().unwrap();
    fs::write(d.file(), b"replaced bytes").unwrap();
    fs::File::options().write(true).open(d.file()).unwrap().set_times(fs::FileTimes::new().set_modified(modified)).unwrap();
    let error = s.execute("agent.apply", req).unwrap_err().to_string();
    assert!(error.contains("original file inputs changed"), "{error}");
    assert_eq!(before(&s), previous);
    assert!(s.execute("agent.validateSnapshot", json!({"token":token(1)})).is_err());
    read(&mut s, 2);
    s.execute("agent.apply", split(&s, 2)).unwrap();
}

#[test]
fn source_change_after_staging_is_rechecked_at_the_actual_publication_boundary() {
    let (mut s, d) = setup();
    read(&mut s, 3);
    let req = split(&s, 3);
    let batch = crate::agent_bridge::parse("agent.apply", &req).unwrap();
    let stage = crate::agent_bridge::stage_batch(&s, &batch).unwrap();
    let previous = before(&s);
    fs::write(d.file(), b"replaced bytes").unwrap();
    assert!(crate::agent_bridge::publish_batch(&mut s, &batch, stage, &req, None).is_err());
    assert_eq!(before(&s), previous);
}

#[test]
fn publication_validated_apply_keeps_one_final_original_check_and_exact_native_undo() {
    for publication in [false, true] {
        let (mut s, _d) = setup();
        let worker = crate::session_worker::SessionWorker::new().unwrap();
        worker.install(&mut s).unwrap();
        s.agent_operation_control().enable_transport();
        let original = s.project.clone();
        let doc = read(&mut s, 95000);
        assert_eq!(doc["capabilities"]["publicationValidatedApply"], true);
        let mut req = split(&s, 95000);
        if publication {
            req["validateAtPublication"] = json!(true);
        }
        let result = s.execute("agent.apply", req.clone()).unwrap();
        assert_eq!(result["status"], "committed");
        let checks = s.agent_operation_control().progress(&token(95000))["completed_checks"].as_u64().unwrap();
        assert_eq!(checks, if publication { 1 } else { 2 });
        let committed = before(&s);
        assert_eq!(s.execute("agent.apply", req).unwrap(), result);
        assert_eq!(before(&s), committed, "lost replies reconcile without another source scan or edit");
        s.execute("edit.undo", json!({})).unwrap();
        assert!(Arc::ptr_eq(&s.project, &original));
    }
}

#[test]
fn publication_validated_apply_refuses_missing_writer_or_cancellation_without_staging() {
    for missing_writer in [false, true] {
        let (mut s, _d) = setup();
        let worker = crate::session_worker::SessionWorker::new().unwrap();
        if missing_writer {
            s.agent_operation_control().enable_transport();
        } else {
            worker.install(&mut s).unwrap();
        }
        let doc = read(&mut s, 95010);
        assert_eq!(doc["capabilities"]["publicationValidatedApply"], false);
        let mut req = split(&s, 95010);
        req["validateAtPublication"] = json!(true);
        let initial = before(&s);
        assert!(s.execute("agent.apply", req).unwrap_err().to_string().contains("installed native writer"));
        assert_eq!(before(&s), initial);
        assert_eq!(s.agent_operation_control().progress(&token(95010))["completed_checks"], 0);
    }
}

#[test]
fn publication_validated_apply_rejects_changed_inputs_after_private_staging_before_any_publication() {
    let (mut s, d) = setup();
    let worker = crate::session_worker::SessionWorker::new().unwrap();
    worker.install(&mut s).unwrap();
    s.agent_operation_control().enable_transport();
    read(&mut s, 95020);
    let mut req = split(&s, 95020);
    req["validateAtPublication"] = json!(true);
    let batch = crate::agent_bridge::parse("agent.apply", &req).unwrap();
    let stage = crate::agent_bridge::stage_batch(&s, &batch).unwrap();
    let initial = before(&s);
    fs::write(d.file(), b"replaced bytes").unwrap();
    let error = crate::agent_bridge::publish_batch(&mut s, &batch, stage, &req, None).unwrap_err().to_string();
    assert!(error.contains("original file inputs changed"), "{error}");
    assert_eq!(before(&s), initial);
}

#[test]
fn publication_validated_apply_cannot_bypass_stop_or_mix_with_background_preparation() {
    let (mut s, _d) = setup();
    let worker = crate::session_worker::SessionWorker::new().unwrap();
    worker.install(&mut s).unwrap();
    s.agent_operation_control().enable_transport();
    read(&mut s, 95030);
    let mut req = split(&s, 95030);
    req["validateAtPublication"] = json!(true);
    let initial = before(&s);
    assert!(s.execute("agent.prepareApply", json!({"job":token(95031),"batch":req})).is_err());
    assert_eq!(before(&s), initial);
    assert!(s.agent_bridge.read_jobs.is_empty());
    assert_eq!(s.agent_operation_control().cancel(&json!({"token":token(95030)})).unwrap()["cancellationAccepted"], true);
    assert!(s.execute("agent.apply", req).is_err());
    assert_eq!(before(&s), initial);
}

#[test]
fn complete_hash_fallback_reads_all_bytes_and_not_just_a_header_or_sample() {
    use sha2::{Digest, Sha256};
    let d = Scratch::new();
    let mut bytes = vec![17u8; 1024 * 1024 + 1];
    bytes[1024 * 1024] = 99;
    fs::write(d.file(), &bytes).unwrap();
    let State::Regular(_, Some(hash)) = observe(&d.file(), &Check::new(web_time::Instant::now()), true).unwrap() else { panic!("full hash missing") };
    assert_eq!(hash, format!("{:x}", Sha256::digest(&bytes)));
    bytes[1024 * 1024] = 98;
    fs::write(d.file(), &bytes).unwrap();
    let State::Regular(_, Some(next)) = observe(&d.file(), &Check::new(web_time::Instant::now()), true).unwrap() else { panic!("full hash missing") };
    assert_ne!(hash, next);
}

#[test]
fn streaming_original_hash_matches_independent_known_vectors_across_short_reads() {
    use std::io::{Cursor, Read};
    struct Fragments {
        bytes: Cursor<Vec<u8>>,
        limit: usize,
    }
    impl Read for Fragments {
        fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
            let size = buffer.len().min(self.limit);
            self.bytes.read(&mut buffer[..size])
        }
    }
    for (bytes, expected) in [
        (Vec::new(), "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"),
        (b"abc".to_vec(), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"),
        (vec![b'a'; 1_000_000], "cdc76e5c9914fb9281a1c7e284d73e67f1809a48a497200e046d39ccc7112cd0"),
    ] {
        for limit in [1, 63, 64, 65, 4093] {
            let check = Check::new(web_time::Instant::now());
            let length = bytes.len() as u64;
            let mut reader = Fragments { bytes: Cursor::new(bytes.clone()), limit };
            assert_eq!(hash_bytes(&mut reader, length, &check).unwrap(), expected);
            assert_eq!(check.bytes.load(Ordering::Relaxed), length);
            assert_eq!(reader.bytes.position(), length);
        }
    }
}

#[test]
fn streaming_original_hash_refuses_short_long_or_failed_reads_without_returning_a_digest() {
    for expected in [2, 4] {
        let check = Check::new(web_time::Instant::now());
        let mut reader = std::io::Cursor::new(b"abc");
        assert!(hash_bytes(&mut reader, expected, &check).is_err());
    }
    struct FailedAfterChunk {
        calls: usize,
    }
    impl std::io::Read for FailedAfterChunk {
        fn read(&mut self, bytes: &mut [u8]) -> std::io::Result<usize> {
            self.calls += 1;
            if self.calls == 1 {
                bytes[0] = b'a';
                Ok(1)
            } else {
                Err(std::io::Error::other("generated original reader failure"))
            }
        }
    }
    let check = Check::new(web_time::Instant::now());
    let mut reader = FailedAfterChunk { calls: 0 };
    assert!(hash_bytes(&mut reader, 3, &check).is_err());
    assert_eq!(check.bytes.load(Ordering::Relaxed), 1);
}

#[test]
fn missing_relocated_or_redirected_originals_do_not_reuse_the_old_ticket() {
    for mode in 0..3 {
        let (mut s, d) = setup();
        read(&mut s, 10 + mode);
        let previous = before(&s);
        let file = d.file();
        if mode == 0 {
            fs::remove_file(&file).unwrap();
        } else if mode == 1 {
            fs::rename(&file, d.0.join("moved.bin")).unwrap();
            fs::write(&file, b"original bytes").unwrap();
        } else {
            fs::remove_file(&file).unwrap();
            fs::create_dir(&file).unwrap();
        }
        assert!(s.execute("agent.apply", split(&s, 10 + mode)).is_err());
        assert_eq!(before(&s), previous);
    }
}

#[test]
fn fresh_candidate_commit_ticket_cannot_renew_an_old_candidates_media_inputs() {
    let mut s = condense_tests::setup();
    let d = Scratch::new();
    fs::write(d.file(), b"original bytes").unwrap();
    bind(&mut s, &d.file());
    read(&mut s, 20);
    let prepare = json!({"token":token(20),"candidate":token(10020),"scope":{"projectId":"p_test","anchorSequence":s.state.active_sequence.unwrap().0},"label":"Media guarded candidate","operation":condense_tests::operation(&s)});
    s.execute("agent.prepareCondense", prepare).unwrap();
    let previous = before(&s);
    fs::write(d.file(), b"replaced bytes").unwrap();
    read(&mut s, 21);
    assert!(s.execute("agent.commitCondense", json!({"token":token(21),"candidate":token(10020)})).is_err());
    assert_eq!(before(&s), previous);
}

#[test]
fn committed_receipts_remain_historical_after_file_changes_without_repeating_a_write() {
    let (mut s, d) = setup();
    read(&mut s, 30);
    let req = split(&s, 30);
    let result = s.execute("agent.apply", req.clone()).unwrap();
    let previous = before(&s);
    fs::write(d.file(), b"replaced bytes").unwrap();
    assert_eq!(s.execute("agent.apply", req).unwrap(), result);
    assert_eq!(before(&s), previous);
    assert_eq!(s.execute("agent.receipt", json!({"token":token(30)})).unwrap()["status"], "committed");
}

#[test]
fn unchanged_native_reads_share_witness_storage_and_file_checks_never_enter_the_project() {
    let (mut s, _d) = setup();
    let previous = before(&s);
    let first = read(&mut s, 40);
    read(&mut s, 41);
    let tickets = &s.agent_bridge.tickets;
    assert!(Arc::ptr_eq(&tickets[tickets.len() - 1].media, &tickets[tickets.len() - 2].media));
    assert_eq!(before(&s), previous);
    assert_eq!(first["capabilities"]["sourceFileGuard"], true);
    let row =
        s.project.item(ItemId(first["sources"].as_array().unwrap().iter().find(|x| x["binding"].is_object()).unwrap()["item"].as_u64().unwrap())).unwrap();
    assert!(!row.metadata.keys().any(|k| k.contains("guard")));
}

#[test]
fn stale_owned_undo_refuses_but_a_fresh_read_preserves_native_history_recovery() {
    let (mut s, d) = setup();
    read(&mut s, 50);
    s.execute("agent.apply", split(&s, 50)).unwrap();
    read(&mut s, 51);
    let previous = before(&s);
    fs::write(d.file(), b"replaced bytes").unwrap();
    assert!(s.execute("agent.undo", json!({"token":token(51),"commit":token(50)})).is_err());
    assert_eq!(before(&s), previous);
    read(&mut s, 52);
    s.execute("agent.undo", json!({"token":token(52),"commit":token(50)})).unwrap();
    read(&mut s, 53);
    s.execute("agent.redo", json!({"token":token(53),"commit":token(50),"undo":token(52)})).unwrap();
}

#[test]
fn expired_verification_and_symlink_redirection_never_publish_a_current_witness() {
    let (_s, d) = setup();
    assert!(observe(&d.file(), &Check::new(web_time::Instant::now() - std::time::Duration::from_secs(46)), false).is_err());
    #[cfg(unix)]
    {
        let (mut s, d) = setup();
        read(&mut s, 60);
        let previous = before(&s);
        let target = d.0.join("target.bin");
        fs::rename(d.file(), &target).unwrap();
        std::os::unix::fs::symlink(&target, d.file()).unwrap();
        assert!(open_original(&d.file()).is_err());
        assert!(s.execute("agent.apply", split(&s, 60)).is_err());
        assert_eq!(before(&s), previous);
    }
}

#[test]
fn program_job_cannot_publish_evidence_after_its_original_inputs_change() {
    let _lock = crate::agent_bridge::inspection::TEST_SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let (mut s, d) = setup();
    read(&mut s, 70);
    let previous = before(&s);
    let scope = json!({"projectId":"p_test","anchorSequence":s.state.active_sequence.unwrap().0});
    s.execute("agent.inspect", json!({"token":token(70),"job":token(1070),"scope":scope,"request":{"kind":"frame","frame":1,"maxSide":64}})).unwrap();
    fs::write(d.file(), b"replaced bytes").unwrap();
    assert!(s.execute("agent.inspection", json!({"job":token(1070)})).is_err());
    assert_eq!(before(&s), previous);
}

#[test]
fn stored_relative_originals_watch_the_resolvers_actual_file_and_refused_paths_grant_no_edit() {
    let (mut s, d) = setup();
    let id = s.project.items.values().find(|i| i.metadata.contains_key(crate::agent_bridge::BINDING_KEY)).unwrap().id;
    Arc::make_mut(&mut s.project).item_mut(id).unwrap().as_media_mut().unwrap().media = MediaRef::File { path: "original.bin".into() };
    let document = d.0.join("working.fcproj").to_string_lossy().into_owned();
    s.media.add_stored_paths(["original.bin".to_string()], Some(&document));
    read(&mut s, 80);
    let previous = before(&s);
    fs::write(d.file(), b"replaced bytes").unwrap();
    assert!(s.execute("agent.apply", split(&s, 80)).is_err());
    assert_eq!(before(&s), previous);
    let foreign = "//192.0.2.1/refused/original.bin".to_string();
    Arc::make_mut(&mut s.project).item_mut(id).unwrap().as_media_mut().unwrap().media = MediaRef::File { path: foreign.clone() };
    s.media.add_stored_paths([foreign], None);
    read(&mut s, 81);
    let previous = before(&s);
    assert!(s.execute("agent.apply", split(&s, 81)).is_err());
    assert_eq!(before(&s), previous);
}

#[test]
fn retained_source_volume_selects_a_bounded_clock_without_qualifying_any_bytes() {
    use std::sync::atomic::AtomicBool;
    let (s, _directory, _) = two_originals();
    let original = hashed_capture(&Plan::prepare(&s.project, &s.media).unwrap());
    let mut rows = original.rows.clone();
    let State::Regular(stamp, _) = &mut rows.first_mut().unwrap().state else { panic!("regular fixture") };
    stamp.bytes = LARGE_SOURCE_BYTES;
    let large = Witnesses { rows, ..Witnesses::default() };
    assert!(large.source_bytes() >= LARGE_SOURCE_BYTES);
    let old = web_time::Instant::now() - std::time::Duration::from_secs(46);
    let stop = Arc::new(AtomicBool::new(false));
    assert!(Check::for_sources(old, stop.clone(), LARGE_SOURCE_BYTES - 1).check().is_err());
    let check = Check::for_sources(old, stop.clone(), large.source_bytes());
    assert!(check.check().is_ok());
    assert_eq!(check.policy().unwrap()["writeAuthority"], false);
    assert!(
        Plan::prepare(&s.project, &s.media).unwrap().recheck(&[&large], &check).is_err(),
        "a longer clock cannot turn the changed synthetic size into a witness"
    );
    stop.store(true, Ordering::Release);
    assert!(check.check().is_err());
    let expired = web_time::Instant::now() - std::time::Duration::from_secs(601);
    assert!(Check::for_sources(expired, Arc::new(AtomicBool::new(false)), u64::MAX).check().is_err());
    assert_eq!(source_deadline(0), OPERATION_DEADLINE_MS);
    assert_eq!(source_deadline(u64::MAX), LARGE_SOURCE_DEADLINE_MS);
}

#[test]
fn large_publication_clock_requires_both_worker_and_independent_stop() {
    for (worker, stop, expected_ms) in [(false, false, 45000), (true, false, 45000), (false, true, 45000), (true, true, 600000)] {
        let (mut s, _directory, _) = two_originals();
        s.worker_available = worker;
        if stop {
            s.agent_bridge.operations.enable_transport();
        }
        read(&mut s, 79010);
        // Supply a synthetic retained workload without allocating a GiB file.
        // The closure observes scheduling only; no project edit is published.
        let ticket = s.agent_bridge.tickets.back_mut().unwrap();
        let mut rows = ticket.media.rows.clone();
        let State::Regular(stamp, _) = &mut rows.first_mut().unwrap().state else { panic!("regular fixture") };
        stamp.bytes = LARGE_SOURCE_BYTES;
        ticket.media = Arc::new(Witnesses { rows, ..Witnesses::default() });
        let unchanged = before(&s);
        let out = crate::agent_bridge::operation_control::run(&mut s, "agent.apply", &json!({"token":token(79010)}), |s, _| {
            Ok(json!({"nativeDeadlineMs":s.agent_bridge.operations.check().unwrap().deadline_ms()}))
        })
        .unwrap();
        assert_eq!(out["nativeDeadlineMs"], expected_ms, "worker={worker}, stop={stop}");
        assert_eq!(before(&s), unchanged);
    }
}
