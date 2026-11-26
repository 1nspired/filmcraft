//! Explicit current-publisher identity admission. Archived frames stay unchanged.
use super::*;
use filmcraft_format::retained::DirectoryPin;
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Reference {
    path: PathBuf,
    observation: Observation,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Envelope {
    kind: String,
    version: u8,
    capture: String,
    frame_sha256: String,
    snapshot_sha256: String,
    activation_token: String,
    image: Observation,
    files: Vec<Entry>,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct DirectoryPhysical {
    dev: String,
    ino: String,
    mode: String,
    uid: String,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct FilePhysical {
    dev: String,
    ino: String,
    size: String,
    mtime_ns: String,
    ctime_ns: String,
    mode: String,
    uid: String,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
enum Entry {
    File { path: PathBuf, sha256: String, bytes: u64, mode: u32, physical: FilePhysical },
    Directory { path: PathBuf, mode: u32, physical: DirectoryPhysical },
}
impl Entry {
    fn path(&self) -> &Path {
        match self {
            Self::File { path, .. } | Self::Directory { path, .. } => path,
        }
    }
}
fn time(value: &str) -> Result<String> {
    let ns = value.parse::<u128>().map_err(fail)?;
    if ns.to_string() != value {
        return Err(fail("canonical bounded publication timestamp required"));
    }
    Ok(format!("{}:{}", ns / 1_000_000_000, ns % 1_000_000_000))
}
fn directory_matches(p: &DirectoryPhysical, actual: &DirectoryPin) -> bool {
    p.dev == actual.identity.device && p.ino == actual.identity.inode && p.mode == actual.identity.mode.to_string() && p.uid == actual.identity.uid.to_string()
}
pub(super) struct Admission {
    frame: NodeFrame,
    entries: BTreeMap<PathBuf, Entry>,
    originals: BTreeMap<PathBuf, NodeState>,
}
impl Admission {
    pub(super) fn load(reference: &Reference, descriptor: &Descriptor, expected: &Expected<'_>, check: &media_guard::Check) -> Result<Self> {
        if !within(&reference.path, &descriptor.storage_roots) || reference.observation.bytes == 0 || reference.observation.bytes > MAX_META as u64 {
            return Err(fail("bounded private current restoration input required"));
        }
        let io = || check.check().map_err(|e| io::Error::other(e.to_string()));
        let parent = reference.path.parent().ok_or_else(|| fail("restoration input parent required"))?;
        let name = reference.path.file_name().and_then(|n| n.to_str()).ok_or_else(|| fail("literal restoration input name required"))?;
        let frame = DirectoryFrame::capture(parent, &io).map_err(fail)?.node(name, &io).map_err(fail)?;
        if frame.state() != &NodeState::File(reference.observation.clone()) {
            return Err(fail("current restoration input generation changed"));
        }
        let raw = frame.read_private_bytes(MAX_META as u64, &io).map_err(fail)?;
        let value: Envelope = serde_json::from_slice(&raw).map_err(fail)?;
        if value.kind != "native_current_image_restoration"
            || value.version != 1
            || value.capture != descriptor.capture
            || value.frame_sha256 != descriptor.sha256
            || value.snapshot_sha256 != expected.source_snapshot
            || value.activation_token != expected.activation_token
            || value.image != descriptor.observation
            || value.files.len() > MAX_NODES
        {
            return Err(fail("current restoration crossed its exact image, original stage or fresh activation"));
        }
        let mut entries = BTreeMap::new();
        for entry in value.files {
            check.check()?;
            if !within(entry.path(), &descriptor.storage_roots) || entries.contains_key(entry.path()) {
                return Err(fail("distinct original current restoration paths required"));
            }
            entries.insert(entry.path().to_owned(), entry);
        }
        Ok(Self { frame, entries, originals: BTreeMap::new() })
    }
    pub(super) fn path(&self) -> PathBuf {
        self.frame.path()
    }
    fn retain(&mut self, path: &Path, old: &NodeState) -> Result<()> {
        if self.originals.get(path).is_some_and(|prior| prior != old) {
            return Err(fail("conflicting original current restoration generations"));
        }
        self.originals.insert(path.into(), old.clone());
        Ok(())
    }
    pub(super) fn node(&mut self, path: &Path, old: &NodeState, actual: &NodeState) -> Result<bool> {
        let Some(entry) = self.entries.get(path) else { return Ok(false) };
        match (entry, old, actual) {
            (Entry::File { sha256, bytes, mode, physical, .. }, NodeState::File(before), NodeState::File(now)) => {
                if before.sha256 != *sha256
                    || before.bytes != *bytes
                    || *mode > 0o777
                    || now.sha256 != before.sha256
                    || now.bytes != before.bytes
                    || now.identity.mode != before.identity.mode
                    || now.identity.uid != before.identity.uid
                    || now.identity.mode & 0o777 != *mode
                    || physical.dev != now.identity.device
                    || physical.ino != now.identity.inode
                    || physical.size != now.bytes.to_string()
                    || physical.mode != now.identity.mode.to_string()
                    || physical.uid != now.identity.uid.to_string()
                    || time(&physical.mtime_ns)? != now.modified
                    || time(&physical.ctime_ns)? != now.changed
                {
                    return Err(fail("confirmed current file differs from original bytes, mode, owner or actual publication"));
                }
            }
            (Entry::Directory { mode, physical, .. }, NodeState::Directory(before), NodeState::Directory(now)) => {
                if before.path != path
                    || now.path != path
                    || *mode > 0o777
                    || now.identity.mode != before.identity.mode
                    || now.identity.uid != before.identity.uid
                    || now.identity.mode & 0o777 != *mode
                    || !directory_matches(physical, now)
                {
                    return Err(fail("confirmed current directory differs from its original path, mode, owner or publication"));
                }
            }
            _ => return Err(fail("current restoration cannot adopt an absent, redirected or different-kind original node")),
        }
        self.retain(path, old)?;
        Ok(true)
    }
    pub(super) fn directory(&mut self, old: &DirectoryRecord, actual: &DirectoryRecord) -> Result<bool> {
        if old.path != actual.path || old.missing != actual.missing || old.anchor.path != actual.anchor.path {
            return Err(fail("original current restoration directory path or absence changed"));
        }
        self.node(&old.anchor.path, &NodeState::Directory(old.anchor.clone()), &NodeState::Directory(actual.anchor.clone()))
    }
    pub(super) fn basis(&self, original: &Value, snapshot: &str, activation: &str) -> Result<Value> {
        let roles = [
            ("saved-project", original["saved_file"]["path"].as_str()),
            ("preferences", original["preferences_file"]["path"].as_str()),
            ("journal-companion", original["journal_companion"]["path"].as_str()),
        ];
        let files = roles
            .into_iter()
            .filter_map(|(role, path)| path.map(|p| (role, Path::new(p))))
            .filter_map(|(role, path)| match self.entries.get(path) {
                Some(Entry::File { path, sha256, bytes, mode, physical }) => {
                    Some(json!({"role":role,"path":path,"sha256":sha256,"bytes":bytes,"mode":mode,"physical":physical}))
                }
                _ => None,
            })
            .collect::<Vec<_>>();
        if files.is_empty() {
            return Ok(original.clone());
        }
        crate::agent_bridge::project_recovery::restore_current_basis(
            original,
            &json!({"kind":"native_current_project_recovery_restoration","version":1,"snapshotSha256":snapshot,"activationToken":activation,"files":files}),
            snapshot,
            activation,
        )
    }
    pub(super) fn finish(self) -> Result<NodeFrame> {
        if self.originals.len() != self.entries.len() {
            return Err(fail("unindexed or unused current restoration publications"));
        }
        Ok(self.frame)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
    fn id() -> String {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        format!("{:08x}-0000-4000-8000-{:012x}", std::process::id(), NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed))
    }
    fn root() -> PathBuf {
        let p = std::env::temp_dir().join(format!("current-native-admission-{}", id()));
        std::fs::create_dir(&p).unwrap();
        p
    }
    fn node(p: &Path) -> NodeFrame {
        DirectoryFrame::capture(p.parent().unwrap(), &|| Ok(())).unwrap().node(p.file_name().unwrap().to_str().unwrap(), &|| Ok(())).unwrap()
    }
    fn physical(p: &Path) -> FilePhysical {
        let s = std::fs::symlink_metadata(p).unwrap();
        FilePhysical {
            dev: s.dev().to_string(),
            ino: s.ino().to_string(),
            size: s.size().to_string(),
            mtime_ns: (u128::try_from(s.mtime()).unwrap() * 1_000_000_000 + u128::try_from(s.mtime_nsec()).unwrap()).to_string(),
            ctime_ns: (u128::try_from(s.ctime()).unwrap() * 1_000_000_000 + u128::try_from(s.ctime_nsec()).unwrap()).to_string(),
            mode: s.mode().to_string(),
            uid: s.uid().to_string(),
        }
    }
    fn private(p: &Path, raw: &[u8]) {
        use std::io::Write;
        let mut f = std::fs::OpenOptions::new().create_new(true).write(true).mode(0o600).open(p).unwrap();
        f.write_all(raw).unwrap();
        f.sync_all().unwrap();
    }
    fn admission(root: &Path, entry: Entry) -> Admission {
        let p = root.join(format!("admission-{}.json", id()));
        private(&p, b"{}");
        Admission { frame: node(&p), entries: BTreeMap::from([(entry.path().into(), entry)]), originals: BTreeMap::new() }
    }
    fn file_fixture() -> (PathBuf, PathBuf, NodeState, NodeState, Entry) {
        let r = root();
        let p = r.join("file.bin");
        private(&p, b"unchanged original bytes");
        let old = node(&p).state().clone();
        std::fs::rename(&p, r.join("held-original.bin")).unwrap();
        private(&p, b"unchanged original bytes");
        let now = node(&p).state().clone();
        let NodeState::File(observed) = &now else { panic!("file fixture") };
        let e = Entry::File {
            path: p.clone(),
            sha256: observed.sha256.clone(),
            bytes: observed.bytes,
            mode: observed.identity.mode & 0o777,
            physical: physical(&p),
        };
        (r, p, old, now, e)
    }
    #[test]
    fn current_admission_accepts_exact_confirmed_file_and_retains_original_generation_consistency() {
        let (r, p, old, now, e) = file_fixture();
        assert_ne!(old, now);
        let mut a = admission(&r, e);
        assert!(a.node(&p, &old, &now).unwrap());
        assert!(a.node(&p, &old, &now).unwrap());
        let mut conflict = old.clone();
        if let NodeState::File(v) = &mut conflict {
            v.identity.inode = "999999".into();
        }
        assert!(a.node(&p, &conflict, &now).is_err());
        assert!(a.finish().is_ok());
    }
    #[test]
    fn current_admission_refuses_changed_bytes_modes_times_identity_absence_and_unused_entries() {
        let (r, p, old, now, e) = file_fixture();
        for fault in ["hash", "bytes", "mode", "inode", "uid", "mtime", "ctime", "size"] {
            let mut bad = e.clone();
            if let Entry::File { sha256, bytes, mode, physical, .. } = &mut bad {
                match fault {
                    "hash" => *sha256 = "0".repeat(64),
                    "bytes" => *bytes += 1,
                    "mode" => *mode = 0o700,
                    "inode" => physical.ino = "0".into(),
                    "uid" => physical.uid.push('0'),
                    "mtime" => physical.mtime_ns = "0".into(),
                    "ctime" => physical.ctime_ns = "00".into(),
                    "size" => physical.size = "0".into(),
                    _ => panic!("known test fault"),
                }
            }
            assert!(admission(&r, bad).node(&p, &old, &now).is_err(), "{fault}");
        }
        assert!(admission(&r, e.clone()).node(&p, &NodeState::Absent, &now).is_err());
        assert!(admission(&r, e.clone()).finish().is_err());
        std::fs::rename(&p, r.join("held-confirmed.bin")).unwrap();
        private(&p, b"unchanged original bytes");
        assert!(admission(&r, e).node(&p, &old, node(&p).state()).is_err());
    }
    #[test]
    fn current_admission_confirms_directory_anchor_but_preserves_missing_chain_and_mode() {
        let r = root();
        let p = r.join("child");
        std::fs::create_dir(&p).unwrap();
        let old = DirectoryFrame::capture(&p, &|| Ok(())).unwrap().record();
        let missing = DirectoryFrame::capture(&p.join("absent"), &|| Ok(())).unwrap().record();
        std::fs::rename(&p, r.join("held-child")).unwrap();
        std::fs::create_dir(&p).unwrap();
        let now = DirectoryFrame::capture(&p, &|| Ok(())).unwrap().record();
        let m = std::fs::symlink_metadata(&p).unwrap();
        let e = Entry::Directory {
            path: p.clone(),
            mode: m.mode() & 0o777,
            physical: DirectoryPhysical { dev: m.dev().to_string(), ino: m.ino().to_string(), mode: m.mode().to_string(), uid: m.uid().to_string() },
        };
        let mut a = admission(&r, e.clone());
        assert!(a.directory(&old, &now).unwrap());
        let now_missing = DirectoryFrame::capture(&p.join("absent"), &|| Ok(())).unwrap().record();
        assert!(a.directory(&missing, &now_missing).unwrap());
        assert!(a.finish().is_ok());
        std::fs::create_dir(p.join("absent")).unwrap();
        let appeared = DirectoryFrame::capture(&p.join("absent"), &|| Ok(())).unwrap().record();
        assert!(admission(&r, e.clone()).directory(&missing, &appeared).is_err());
        let mut wrong = old;
        wrong.anchor.identity.mode ^= 0o100;
        assert!(admission(&r, e).directory(&wrong, &now).is_err());
    }
    #[test]
    fn current_admission_metadata_is_private_bounded_and_bound_to_exact_image_and_activation() {
        let r = root();
        let image = r.join("image.json");
        private(&image, b"{}");
        let NodeState::File(observation) = node(&image).state().clone() else { panic!("image fixture") };
        let scope = crate::project_workspace::ProjectScope { project_id: id(), workspace_id: id() };
        let capture = id();
        let update = id();
        let source = "a".repeat(64);
        let token = "b".repeat(32);
        let project = r.join("project.fcproj").to_str().unwrap().to_owned();
        let d = Descriptor {
            capture: capture.clone(),
            sha256: observation.sha256.clone(),
            bytes: observation.bytes,
            native_pid: 1,
            native_session: "c".repeat(32),
            activation_token: "d".repeat(32),
            observation: observation.clone(),
            storage_roots: vec![DirectoryFrame::capture(&r, &|| Ok(())).unwrap().record()],
            reserve_bytes: 1,
            restoration: None,
        };
        let saved_file = Value::Null;
        let expected = Expected {
            scope: &scope,
            workspace: &scope.workspace_id,
            update: &update,
            project_path: &project,
            activation_token: &token,
            minimum_revision: 1,
            source_snapshot: &source,
            saved_file: &saved_file,
        };
        let good = json!({"kind":"native_current_image_restoration","version":1,"capture":capture,"frameSha256":d.sha256,"snapshotSha256":source,"activationToken":token,"image":observation,"files":[]});
        for field in ["valid", "capture", "frameSha256", "snapshotSha256", "activationToken", "unknown"] {
            let mut value = good.clone();
            if field == "unknown" {
                value["unknown"] = json!(true)
            } else if field != "valid" {
                value[field] = json!("wrong");
            }
            let p = r.join(format!("input-{field}.json"));
            private(&p, &serde_json::to_vec(&value).unwrap());
            let NodeState::File(observation) = node(&p).state().clone() else { panic!("input fixture") };
            let reference = Reference { path: p, observation };
            let result = Admission::load(&reference, &d, &expected, &media_guard::Check::new(web_time::Instant::now()));
            if field == "valid" {
                assert!(result.unwrap().finish().is_ok())
            } else {
                assert!(result.is_err(), "{field}");
            }
        }
    }
}
