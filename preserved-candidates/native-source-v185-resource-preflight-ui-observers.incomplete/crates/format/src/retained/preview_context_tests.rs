use super::*;
use std::{
    os::unix::fs::{MetadataExt, symlink},
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};
static NEXT: AtomicU64 = AtomicU64::new(1);
fn root() -> PathBuf {
    let p = std::env::temp_dir().join(format!(
        "preview-context-{}-{}-{}",
        std::process::id(),
        SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos(),
        NEXT.fetch_add(1, Ordering::SeqCst)
    ));
    fs::create_dir(&p).unwrap();
    fs::canonicalize(p).unwrap()
}
#[test]
fn preview_directory_context_records_exact_known_ancestor_and_every_missing_parent_without_creating_any_folder() {
    let root = root();
    let target = root.join("missing/deep");
    let frame = DirectoryFrame::capture(&target, &|| Ok(())).unwrap();
    let record = frame.record();
    assert_eq!(record.anchor.path, root);
    assert_eq!(record.missing, vec![root.join("missing"), target.clone()]);
    assert!(!frame.exists());
    assert!(!target.exists());
    assert_eq!(frame.node("one.mov", &|| Ok(())).unwrap().state(), &NodeState::Absent);
    frame.current(&|| Ok(())).unwrap();
    assert!(!root.join("missing").exists());
}
#[test]
fn preview_node_context_reads_complete_empty_and_multichunk_files_and_existing_directory_collisions_without_collecting_unbounded_payloads() {
    use sha2::Digest;
    let root = root();
    let frame = DirectoryFrame::capture(&root, &|| Ok(())).unwrap();
    for (name, data) in [("empty.mov", Vec::new()), ("chunks.mov", vec![0x41; 196615])] {
        fs::write(root.join(name), &data).unwrap();
        let node = frame.node(name, &|| Ok(())).unwrap();
        let NodeState::File(observed) = node.state() else { panic!("regular generated preview required") };
        assert_eq!(observed.bytes, data.len() as u64);
        assert_eq!(observed.sha256, format!("{:x}", sha2::Sha256::digest(&data)));
        node.current(&|| Ok(())).unwrap();
        node.fence(&|| Ok(())).unwrap();
    }
    fs::create_dir(root.join("directory.mov")).unwrap();
    assert!(matches!(frame.node("directory.mov", &|| Ok(())).unwrap().state(), NodeState::Directory(_)));
}
#[test]
fn preview_context_refuses_symlink_parents_sources_hostile_names_and_noncanonical_paths_without_following_redirected_payloads() {
    let root = root();
    let frame = DirectoryFrame::capture(&root, &|| Ok(())).unwrap();
    fs::write(root.join("held"), b"kept source").unwrap();
    symlink(root.join("held"), root.join("redirect.mov")).unwrap();
    assert!(frame.node("redirect.mov", &|| Ok(())).is_err());
    symlink(&root, root.join("alias")).unwrap();
    assert!(DirectoryFrame::capture(&root.join("alias"), &|| Ok(())).is_err());
    assert!(DirectoryFrame::capture(&root.join("../other"), &|| Ok(())).is_err());
    for name in ["", ".", "..", "../held", "/absolute", "a/b", "a/", "a\nb", &"x".repeat(513)] {
        assert!(frame.node(name, &|| Ok(())).is_err());
    }
    assert_eq!(fs::read(root.join("held")).unwrap(), b"kept source");
}
#[test]
fn preview_context_final_fences_detect_earlier_file_changes_after_later_complete_reads_even_if_mtime_is_restored() {
    let root = root();
    fs::write(root.join("a.mov"), b"old data").unwrap();
    fs::write(root.join("b.mov"), b"later read").unwrap();
    let frame = DirectoryFrame::capture(&root, &|| Ok(())).unwrap();
    let a = frame.node("a.mov", &|| Ok(())).unwrap();
    let b = frame.node("b.mov", &|| Ok(())).unwrap();
    let modified = fs::metadata(a.path()).unwrap().modified().unwrap();
    a.current(&|| Ok(())).unwrap();
    b.current(&|| Ok(())).unwrap();
    fs::write(a.path(), b"new data").unwrap();
    fs::File::open(a.path()).unwrap().set_times(fs::FileTimes::new().set_modified(modified)).unwrap();
    assert!(a.current(&|| Ok(())).is_err());
    assert!(a.fence(&|| Ok(())).is_err());
    assert_eq!(fs::read(a.path()).unwrap(), b"new data");
}
#[test]
fn preview_context_missing_parent_appearance_and_identical_replacement_directory_cannot_be_adopted_from_old_observation() {
    let root = root();
    let missing = DirectoryFrame::capture(&root.join("new"), &|| Ok(())).unwrap();
    fs::create_dir(root.join("new")).unwrap();
    assert!(missing.current(&|| Ok(())).is_err());
    fs::create_dir(root.join("source")).unwrap();
    fs::write(root.join("source/a.mov"), b"same bytes").unwrap();
    let old = DirectoryFrame::capture(&root.join("source"), &|| Ok(())).unwrap();
    let node = old.node("a.mov", &|| Ok(())).unwrap();
    let ino = fs::metadata(root.join("source")).unwrap().ino();
    fs::rename(root.join("source"), root.join("held-source")).unwrap();
    fs::create_dir(root.join("source")).unwrap();
    fs::write(root.join("source/a.mov"), b"same bytes").unwrap();
    assert_ne!(fs::metadata(root.join("source")).unwrap().ino(), ino);
    assert!(old.current(&|| Ok(())).is_err());
    assert!(node.current(&|| Ok(())).is_err());
    assert_eq!(fs::read(root.join("held-source/a.mov")).unwrap(), b"same bytes");
}
#[test]
fn preview_storage_observation_matches_actual_fd_device_and_bounds_rounding_zero_units_and_overflow_without_reserving_or_writing() {
    let root = root();
    let frame = DirectoryFrame::capture(&root, &|| Ok(())).unwrap();
    let before = fs::read_dir(&root).unwrap().count();
    let space = frame.space(&|| Ok(())).unwrap();
    assert_eq!(space.device, fs::metadata(&root).unwrap().dev().to_string());
    assert!(space.allocation_unit > 0 && space.free > 0);
    assert_eq!(allocation(0, 4096).unwrap(), 0);
    assert_eq!(allocation(1, 4096).unwrap(), 4096);
    assert_eq!(allocation(4097, 4096).unwrap(), 8192);
    assert!(allocation(1, 0).is_err());
    assert!(allocation(u64::MAX, 4096).is_err());
    assert_eq!(fs::read_dir(&root).unwrap().count(), before);
}

#[test]
fn complete_private_control_reads_keep_original_generation_uid_mode_and_bound() {
    use std::os::unix::fs::PermissionsExt;
    let p = root();
    let bytes = vec![0x41; 196615];
    fs::write(p.join("control.json"), &bytes).unwrap();
    fs::set_permissions(p.join("control.json"), fs::Permissions::from_mode(0o600)).unwrap();
    let d = DirectoryFrame::capture(&p, &|| Ok(())).unwrap();
    let node = d.node("control.json", &|| Ok(())).unwrap();
    assert_eq!(node.read_private_bytes(200000, &|| Ok(())).unwrap(), bytes);
    assert!(node.read_private_bytes(100, &|| Ok(())).is_err());
    fs::set_permissions(p.join("control.json"), fs::Permissions::from_mode(0o644)).unwrap();
    assert!(node.read_private_bytes(200000, &|| Ok(())).is_err());
    let changed = d.node("control.json", &|| Ok(())).unwrap();
    assert!(changed.read_private_bytes(200000, &|| Ok(())).is_err());
}
#[test]
fn private_control_same_byte_replacement_and_stopped_read_cannot_borrow_original_frame() {
    use std::os::unix::fs::PermissionsExt;
    let p = root();
    for name in ["original", "replacement"] {
        fs::write(p.join(name), b"same complete bytes").unwrap();
        fs::set_permissions(p.join(name), fs::Permissions::from_mode(0o600)).unwrap();
    }
    let d = DirectoryFrame::capture(&p, &|| Ok(())).unwrap();
    let node = d.node("original", &|| Ok(())).unwrap();
    assert!(node.read_private_bytes(100, &|| Err(io::Error::new(io::ErrorKind::Interrupted, "generated Stop"))).is_err());
    fs::rename(p.join("replacement"), p.join("original")).unwrap();
    assert!(node.read_private_bytes(100, &|| Ok(())).is_err());
    assert_eq!(fs::read(p.join("original")).unwrap(), b"same complete bytes");
}

#[test]
fn complete_owned_data_reads_preserve_ordinary_modes_and_do_not_relax_private_control_reads() {
    use std::os::unix::fs::PermissionsExt;
    let p = root();
    let bytes = vec![0x41; 196615];
    for mode in [0o600, 0o640, 0o644] {
        let file = p.join(format!("data-{mode}"));
        fs::write(&file, &bytes).unwrap();
        fs::set_permissions(&file, fs::Permissions::from_mode(mode)).unwrap();
        let frame = DirectoryFrame::capture(&p, &|| Ok(())).unwrap();
        let node = frame.node_bounded(file.file_name().unwrap().to_str().unwrap(), 200000, &|| Ok(())).unwrap();
        assert_eq!(node.read_owned_bytes(200000, &|| Ok(())).unwrap(), bytes);
        assert_eq!(fs::metadata(&file).unwrap().permissions().mode() & 0o7777, mode);
        assert_eq!(node.read_private_bytes(200000, &|| Ok(())).is_ok(), mode == 0o600);
    }
}

#[test]
fn complete_owned_data_refuses_other_writers_empty_missing_redirected_and_over_bound_files() {
    use std::os::unix::fs::PermissionsExt;
    let p = root();
    let frame = DirectoryFrame::capture(&p, &|| Ok(())).unwrap();
    for mode in [0o620, 0o660, 0o666] {
        let name = format!("writable-{mode}");
        let file = p.join(&name);
        fs::write(&file, b"generated data").unwrap();
        fs::set_permissions(&file, fs::Permissions::from_mode(mode)).unwrap();
        let node = frame.node(&name, &|| Ok(())).unwrap();
        assert!(node.read_owned_bytes(100, &|| Ok(())).is_err());
    }
    fs::write(p.join("empty"), b"").unwrap();
    assert!(frame.node("empty", &|| Ok(())).unwrap().read_owned_bytes(100, &|| Ok(())).is_err());
    assert!(frame.node("missing", &|| Ok(())).unwrap().read_owned_bytes(100, &|| Ok(())).is_err());
    fs::create_dir(p.join("directory")).unwrap();
    assert!(frame.node("directory", &|| Ok(())).unwrap().read_owned_bytes(100, &|| Ok(())).is_err());
    fs::write(p.join("data"), b"generated data").unwrap();
    let node = frame.node("data", &|| Ok(())).unwrap();
    assert!(node.read_owned_bytes(1, &|| Ok(())).is_err());
    assert!(node.read_owned_bytes(MAX_BYTES + 1, &|| Ok(())).is_err());
    symlink(p.join("data"), p.join("alias")).unwrap();
    assert!(frame.node("alias", &|| Ok(())).is_err());
}

#[test]
fn complete_owned_data_keeps_original_generation_and_refuses_hardlinked_equal_byte_replacements() {
    let p = root();
    fs::write(p.join("data"), b"same complete data").unwrap();
    let frame = DirectoryFrame::capture(&p, &|| Ok(())).unwrap();
    let original = frame.node("data", &|| Ok(())).unwrap();
    fs::rename(p.join("data"), p.join("retained-original")).unwrap();
    fs::write(p.join("data"), b"same complete data").unwrap();
    assert!(original.read_owned_bytes(100, &|| Ok(())).is_err());
    fs::hard_link(p.join("data"), p.join("linked")).unwrap();
    let linked = frame.node("data", &|| Ok(())).unwrap();
    assert!(linked.read_owned_bytes(100, &|| Ok(())).is_err());
    assert_eq!(fs::read(p.join("retained-original")).unwrap(), b"same complete data");
    assert_eq!(fs::read(p.join("linked")).unwrap(), b"same complete data");
}

#[test]
fn complete_owned_data_preserves_stop_and_deadline_guards_without_writing_or_reobserving() {
    let p = root();
    let bytes = vec![0x41; 196615];
    fs::write(p.join("data"), &bytes).unwrap();
    let frame = DirectoryFrame::capture(&p, &|| Ok(())).unwrap();
    let node = frame.node("data", &|| Ok(())).unwrap();
    let measured_calls = AtomicU64::new(0);
    let count = || {
        measured_calls.fetch_add(1, Ordering::SeqCst);
        Ok(())
    };
    assert_eq!(node.read_owned_bytes(200000, &count).unwrap(), bytes);
    let boundary_count = measured_calls.load(Ordering::SeqCst);
    assert!(boundary_count > 1);
    // Exercise every actual boundary, including chunk reads and final fences.
    // The reader must refuse, rather than silently succeeding after Stop.
    for stop_at in 1..=boundary_count {
        let calls = AtomicU64::new(0);
        let guard = || {
            let call = calls.fetch_add(1, Ordering::SeqCst) + 1;
            if call == stop_at { Err(io::Error::other("original bounded read stopped")) } else { Ok(()) }
        };
        assert!(node.read_owned_bytes(200000, &guard).is_err());
        assert!(calls.load(Ordering::SeqCst) >= stop_at);
        assert_eq!(fs::read(p.join("data")).unwrap(), bytes);
    }
    assert_eq!(node.read_owned_bytes(200000, &|| Ok(())).unwrap(), bytes);
}
#[test]
fn manager_liveness_probe_never_accepts_zero_or_overflow_pid_and_does_not_signal_current_process() {
    assert!(check_owner_process(0).is_err());
    assert!(check_owner_process(u32::MAX).is_err());
    check_owner_process(std::process::id()).unwrap();
}

#[test]
fn preview_context_exited_process_probe_refuses_live_and_unbounded_pids_and_requires_actual_exit() {
    assert!(check_owner_exited(0).is_err());
    assert!(check_owner_exited(u32::MAX).is_err());
    assert!(check_owner_exited(std::process::id()).is_err());
    let mut child = std::process::Command::new("/bin/cat").stdin(std::process::Stdio::piped()).stdout(std::process::Stdio::null()).spawn().unwrap();
    assert!(check_owner_exited(child.id()).is_err());
    drop(child.stdin.take());
    child.wait().unwrap();
    check_owner_exited(child.id()).unwrap();
}

// Test-only original compositions. These deliberately retain the old adjacent
// observations and use the same actual Directory primitives and pinned frames.
fn original_directory_current(frame: &DirectoryFrame, check: &impl Fn() -> io::Result<()>) -> io::Result<()> {
    check()?;
    frame.anchor.current()?;
    for p in &frame.missing {
        match fs::symlink_metadata(p) {
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(e),
            Ok(_) => return Err(error("a missing preview parent appeared; no implicit adoption")),
        }
    }
    frame.anchor.current()
}

fn original_node_fence(node: &NodeFrame, check: &impl Fn() -> io::Result<()>) -> io::Result<()> {
    original_directory_current(&node.parent, check)?;
    if !node.parent.exists() {
        return if node.state == NodeState::Absent { Ok(()) } else { Err(error("missing preview parent changed")) };
    }
    match &node.state {
        NodeState::Absent => node.parent.anchor.fence(OsStr::new(&node.name), None),
        NodeState::File(expected) => node.parent.anchor.fence(OsStr::new(&node.name), Some(expected)),
        NodeState::Directory(pin) => {
            let now = Directory::open(&node.path(), false)?;
            if &now.pin != pin { Err(error("existing preview destination directory changed")) } else { Ok(()) }
        }
    }
}

#[derive(Debug)]
struct AdjacentStop(std::sync::Arc<()>);
impl std::fmt::Display for AdjacentStop {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("original caller Stop")
    }
}
impl std::error::Error for AdjacentStop {}

#[test]
fn adjacent_frame_checks_preserve_single_callback_and_original_stop_error() {
    let p = root();
    fs::write(p.join("data"), b"complete original data").unwrap();
    let frame = DirectoryFrame::capture(&p, &|| Ok(())).unwrap();
    let node = frame.node("data", &|| Ok(())).unwrap();
    for route in 0..4 {
        let calls = AtomicU64::new(0);
        let token = Arc::new(());
        let stopped = || {
            calls.fetch_add(1, Ordering::SeqCst);
            Err(io::Error::new(io::ErrorKind::Interrupted, AdjacentStop(token.clone())))
        };
        let result = match route {
            0 => original_directory_current(&frame, &stopped),
            1 => frame.current(&stopped),
            2 => original_node_fence(&node, &stopped),
            _ => node.fence(&stopped),
        };
        let error = result.unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::Interrupted);
        assert!(Arc::ptr_eq(&error.get_ref().unwrap().downcast_ref::<AdjacentStop>().unwrap().0, &token));
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        let calls = AtomicU64::new(0);
        let checked = || {
            calls.fetch_add(1, Ordering::SeqCst);
            Ok(())
        };
        match route {
            0 => original_directory_current(&frame, &checked),
            1 => frame.current(&checked),
            2 => original_node_fence(&node, &checked),
            _ => node.fence(&checked),
        }
        .unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }
}

#[test]
fn adjacent_file_fences_refuse_each_real_metadata_or_type_change() {
    use std::os::unix::fs::PermissionsExt;
    for mutation in 0..7 {
        let p = root();
        let path = p.join("data");
        fs::write(&path, b"same original bytes").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        fs::File::open(&path).unwrap().set_times(fs::FileTimes::new().set_modified(UNIX_EPOCH + std::time::Duration::from_secs(100))).unwrap();
        let frame = DirectoryFrame::capture(&p, &|| Ok(())).unwrap();
        let node = frame.node("data", &|| Ok(())).unwrap();
        let before = fs::metadata(&path).unwrap();
        original_node_fence(&node, &|| Ok(())).unwrap();
        node.fence(&|| Ok(())).unwrap();
        match mutation {
            0 => {
                fs::write(p.join("replacement"), b"same original bytes").unwrap();
                fs::set_permissions(p.join("replacement"), fs::Permissions::from_mode(0o600)).unwrap();
                fs::File::open(p.join("replacement")).unwrap().set_times(fs::FileTimes::new().set_modified(before.modified().unwrap())).unwrap();
                fs::rename(p.join("replacement"), &path).unwrap();
                assert_ne!(fs::metadata(&path).unwrap().ino(), before.ino());
            }
            1 => fs::set_permissions(&path, fs::Permissions::from_mode(0o640)).unwrap(),
            2 => fs::OpenOptions::new().write(true).open(&path).unwrap().set_len(2).unwrap(),
            3 => fs::File::open(&path).unwrap().set_times(fs::FileTimes::new().set_modified(UNIX_EPOCH + std::time::Duration::from_secs(200))).unwrap(),
            4 => {
                // Restore size and mtime after writing equal-length bytes: the
                // original complete observation's ctime still must be checked.
                fs::write(&path, b"DIFF original bytes").unwrap();
                fs::File::open(&path).unwrap().set_times(fs::FileTimes::new().set_modified(before.modified().unwrap())).unwrap();
                let after = fs::metadata(&path).unwrap();
                assert_eq!(after.len(), before.len());
                assert_eq!(after.modified().unwrap(), before.modified().unwrap());
                assert_ne!((after.ctime(), after.ctime_nsec()), (before.ctime(), before.ctime_nsec()));
            }
            5 => {
                fs::rename(&path, p.join("held")).unwrap();
                symlink(p.join("held"), &path).unwrap();
            }
            _ => {
                fs::rename(&path, p.join("held")).unwrap();
                fs::create_dir(&path).unwrap();
            }
        }
        assert!(original_node_fence(&node, &|| Ok(())).is_err(), "baseline mutation {mutation}");
        assert!(node.fence(&|| Ok(())).is_err(), "candidate mutation {mutation}");
    }
}

#[test]
fn adjacent_frames_refuse_same_byte_parent_replacement_and_same_inode_ancestor_alias() {
    for alias in [false, true] {
        let p = root();
        let ancestor = p.join("ancestor");
        let parent = ancestor.join("parent");
        fs::create_dir_all(&parent).unwrap();
        fs::write(parent.join("data"), b"same original bytes").unwrap();
        let frame = DirectoryFrame::capture(&parent, &|| Ok(())).unwrap();
        let node = frame.node("data", &|| Ok(())).unwrap();
        let inode = fs::metadata(&parent).unwrap().ino();
        fs::rename(&ancestor, p.join("held-ancestor")).unwrap();
        if alias {
            symlink(p.join("held-ancestor"), &ancestor).unwrap();
            assert_eq!(fs::metadata(&parent).unwrap().ino(), inode);
        } else {
            fs::create_dir_all(&parent).unwrap();
            fs::write(parent.join("data"), b"same original bytes").unwrap();
            assert_ne!(fs::metadata(&parent).unwrap().ino(), inode);
        }
        assert!(original_directory_current(&frame, &|| Ok(())).is_err());
        assert!(frame.current(&|| Ok(())).is_err());
        assert!(original_node_fence(&node, &|| Ok(())).is_err());
        assert!(node.fence(&|| Ok(())).is_err());
        assert_eq!(fs::read(p.join("held-ancestor/parent/data")).unwrap(), b"same original bytes");
    }
}

#[test]
fn adjacent_file_fences_observe_mutations_performed_by_the_caller_guard() {
    for original in [true, false] {
        let p = root();
        fs::write(p.join("data"), b"complete original bytes").unwrap();
        let frame = DirectoryFrame::capture(&p, &|| Ok(())).unwrap();
        let node = frame.node("data", &|| Ok(())).unwrap();
        original_node_fence(&node, &|| Ok(())).unwrap();
        node.fence(&|| Ok(())).unwrap();
        let calls = AtomicU64::new(0);
        let changed = || {
            assert_eq!(calls.fetch_add(1, Ordering::SeqCst), 0);
            fs::rename(p.join("data"), p.join("held"))?;
            fs::write(p.join("data"), b"complete original bytes")?;
            Ok(())
        };
        let result = if original { original_node_fence(&node, &changed) } else { node.fence(&changed) };
        assert!(result.is_err());
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(fs::read(p.join("held")).unwrap(), b"complete original bytes");
    }
}

#[test]
fn adjacent_frame_missing_and_nonfile_branches_keep_original_absence_and_identity_checks() {
    let p = root();
    let missing = DirectoryFrame::capture(&p.join("missing/deep"), &|| Ok(())).unwrap();
    let absent = missing.node("data", &|| Ok(())).unwrap();
    original_directory_current(&missing, &|| Ok(())).unwrap();
    missing.current(&|| Ok(())).unwrap();
    original_node_fence(&absent, &|| Ok(())).unwrap();
    absent.fence(&|| Ok(())).unwrap();
    fs::create_dir(p.join("missing")).unwrap();
    assert!(original_directory_current(&missing, &|| Ok(())).is_err());
    assert!(missing.current(&|| Ok(())).is_err());
    assert!(original_node_fence(&absent, &|| Ok(())).is_err());
    assert!(absent.fence(&|| Ok(())).is_err());
    let frame = DirectoryFrame::capture(&p, &|| Ok(())).unwrap();
    fs::create_dir(p.join("directory")).unwrap();
    let directory = frame.node("directory", &|| Ok(())).unwrap();
    let absent = frame.node("not-created", &|| Ok(())).unwrap();
    for node in [&directory, &absent] {
        original_node_fence(node, &|| Ok(())).unwrap();
        node.fence(&|| Ok(())).unwrap();
    }
    fs::rename(p.join("directory"), p.join("held-directory")).unwrap();
    fs::create_dir(p.join("directory")).unwrap();
    fs::write(p.join("not-created"), b"foreign new file").unwrap();
    for node in [&directory, &absent] {
        assert!(original_node_fence(node, &|| Ok(())).is_err());
        assert!(node.fence(&|| Ok(())).is_err());
    }
}

#[test]
fn adjacent_full_reads_keep_callback_boundaries_and_refuse_reentrant_parent_redirect() {
    use std::os::unix::fs::PermissionsExt;
    for private in [true, false] {
        let p = root();
        let parent = p.join("parent");
        fs::create_dir(&parent).unwrap();
        fs::write(parent.join("data"), b"original complete bytes").unwrap();
        fs::set_permissions(parent.join("data"), fs::Permissions::from_mode(0o600)).unwrap();
        let frame = DirectoryFrame::capture(&parent, &|| Ok(())).unwrap();
        let node = frame.node("data", &|| Ok(())).unwrap();
        let calls = AtomicU64::new(0);
        let redirected = || {
            if calls.fetch_add(1, Ordering::SeqCst) == 2 {
                fs::rename(&parent, p.join("held-parent"))?;
                symlink(p.join("held-parent"), &parent)?;
            }
            Ok(())
        };
        let result = if private { node.read_private_bytes(100, &redirected) } else { node.read_owned_bytes(100, &redirected) };
        assert!(result.is_err());
        assert_eq!(calls.load(Ordering::SeqCst), 3);
        assert_eq!(fs::read(p.join("held-parent/data")).unwrap(), b"original complete bytes");
    }
}

#[test]
fn adjacent_frame_balanced_original_composition_measurement() {
    use std::io::Write;
    use std::time::Instant;
    let Some(report) = std::env::var_os("POEMA_NATIVE_FRAME_BENCH_REPORT") else {
        return;
    };
    let report = PathBuf::from(report);
    assert!(report.is_absolute());
    let p = root();
    let mut parent = p.clone();
    for level in 0..12 {
        parent.push(format!("existing-directory-context-level-{level:02}"));
    }
    fs::create_dir_all(&parent).unwrap();
    fs::write(parent.join("data"), b"complete original benchmark data").unwrap();
    let frame = DirectoryFrame::capture(&parent, &|| Ok(())).unwrap();
    let node = frame.node("data", &|| Ok(())).unwrap();
    let original_file = fs::metadata(node.path()).unwrap();
    let calls = AtomicU64::new(0);
    let checked = || {
        calls.fetch_add(1, Ordering::Relaxed);
        Ok(())
    };
    let deadline = Instant::now();
    let mut rows = Vec::new();
    for directory in [true, false] {
        let operation = |original: bool| {
            if directory {
                if original { original_directory_current(&frame, &checked) } else { frame.current(&checked) }
            } else if original {
                original_node_fence(&node, &checked)
            } else {
                node.fence(&checked)
            }
        };
        for _ in 0..64 {
            operation(true).unwrap();
            operation(false).unwrap();
        }
        for pair in 0..32 {
            let mut elapsed = [0_u128; 2];
            for original in if pair % 2 == 0 { [true, false] } else { [false, true] } {
                assert!(deadline.elapsed().as_secs() < 45, "bounded frame measurement exceeded 45 seconds");
                let begin = Instant::now();
                for _ in 0..512 {
                    operation(original).unwrap();
                }
                elapsed[usize::from(!original)] = begin.elapsed().as_nanos();
            }
            rows.push(serde_json::json!({"route": if directory { "directory-current" } else { "file-fence" }, "pair":pair,"iterationsPerSide":512,"originalNs":elapsed[0],"candidateNs":elapsed[1]}));
        }
    }
    assert_eq!(calls.load(Ordering::Relaxed), 2 * (128 + 32 * 2 * 512));
    let after = fs::metadata(node.path()).unwrap();
    assert_eq!(
        (after.dev(), after.ino(), after.mode(), after.len(), after.mtime(), after.mtime_nsec(), after.ctime(), after.ctime_nsec()),
        (
            original_file.dev(),
            original_file.ino(),
            original_file.mode(),
            original_file.len(),
            original_file.mtime(),
            original_file.mtime_nsec(),
            original_file.ctime(),
            original_file.ctime_nsec()
        )
    );
    assert_eq!(fs::read(node.path()).unwrap(), b"complete original benchmark data");
    frame.current(&|| Ok(())).unwrap();
    node.current(&|| Ok(())).unwrap();
    let value = serde_json::json!({"fixture":parent,"rows":rows,"sameActualPinnedFrames":true,"callerCheckCalls":calls.load(Ordering::Relaxed),"fileGenerationPreserved":true,"elapsedMs":deadline.elapsed().as_millis(),"performanceAssertionMade":false});
    let mut output = fs::OpenOptions::new().write(true).create_new(true).open(report).unwrap();
    output.write_all(serde_json::to_vec_pretty(&value).unwrap().as_slice()).unwrap();
    output.sync_all().unwrap();
}
