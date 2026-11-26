//! Immutable, scoped native picker receipts. One bounded file lookup per ID;
//! publication runs on the retirement worker, never on the GUI thread.
use super::{Record, Reference, reference, uuid};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Component, Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::SystemTime,
};

const MAX_BYTES: u64 = super::MAX_RECEIPT_BYTES as u64 + 64 * 1024;
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub(super) enum Entry {
    Record { record: Record },
    Fence { reference: Reference },
    Released { proof: Box<super::resolution::Proof>, original: Option<Record> },
}
impl Entry {
    pub(super) fn id(&self) -> &str {
        match self {
            Self::Record { record } => &record.request.operation_id,
            Self::Fence { reference } => &reference.operation_id,
            Self::Released { proof, .. } => &proof.request.operation_id,
        }
    }
    pub(super) fn reference(&self) -> Reference {
        match self {
            Self::Record { record } => Reference {
                operation_id: record.request.operation_id.clone(),
                native_session: record.request.native_session.clone(),
                project_path: record.request.project_path.clone(),
            },
            Self::Fence { reference } => reference.clone(),
            Self::Released { proof, .. } => Reference {
                operation_id: proof.request.operation_id.clone(),
                native_session: proof.request.native_session.clone(),
                project_path: proof.request.project_path.clone(),
            },
        }
    }
    fn valid(&self) -> bool {
        if !reference(&self.reference()) {
            return false;
        }
        match self {
            Self::Fence { .. } => true,
            Self::Released { proof, original } => {
                proof.valid()
                    && original.as_ref().is_none_or(|r| {
                        r.request == proof.request
                            && r.value["status"] == "unknown"
                            && r.value["receipt_complete"] == false
                            && r.value["operation_id"] == proof.request.operation_id
                            && r.value["can_restart"] == false
                            && r.value["original_owner_write"] == false
                            && r.value["perception_requested"] == false
                    })
            }
            Self::Record { record: r } => {
                let q = &r.request;
                super::retirable(r)
                    && q.revision <= 9_007_199_254_740_991
                    && q.scope.valid()
                    && r.value["kind"] == "native_media_intake"
                    && r.value["version"] == 1
                    && r.value["operation_id"] == q.operation_id
                    && r.value["native_session"] == q.native_session
                    && r.value["project_path"] == q.project_path
                    && r.value["start_revision"] == q.revision
                    && serde_json::to_value(&q.scope).is_ok_and(|scope| r.value["scope"] == scope)
                    && r.value["selection_kind"]
                        == match q.kind {
                            super::Kind::Files => "files",
                            super::Kind::Folder => "folder",
                        }
                    && r.value["can_restart"] == false
                    && r.value["original_owner_write"] == false
                    && r.value["perception_requested"] == false
            }
        }
    }
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Envelope {
    kind: String,
    version: u32,
    sha256: String,
    entry: Entry,
}
#[derive(Clone, PartialEq)]
struct Stamp {
    len: u64,
    modified: Option<SystemTime>,
    #[cfg(unix)]
    dev: u64,
    #[cfg(unix)]
    ino: u64,
    #[cfg(unix)]
    ctime: (i64, i64),
}
fn stamp(m: &fs::Metadata) -> Stamp {
    #[cfg(unix)]
    use std::os::unix::fs::MetadataExt;
    Stamp {
        len: m.len(),
        modified: m.modified().ok(),
        #[cfg(unix)]
        dev: m.dev(),
        #[cfg(unix)]
        ino: m.ino(),
        #[cfg(unix)]
        ctime: (m.ctime(), m.ctime_nsec()),
    }
}
fn private(m: &fs::Metadata) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        m.permissions().mode() & 0o077 == 0
    }
    #[cfg(not(unix))]
    {
        let _ = m;
        true
    }
}
fn sync_dir(p: &Path) -> Result<(), String> {
    #[cfg(unix)]
    File::open(p).and_then(|f| f.sync_all()).map_err(|e| format!("Native media archive directory sync failed: {e}"))?;
    #[cfg(not(unix))]
    let _ = p;
    Ok(())
}
fn error(e: impl std::fmt::Display) -> String {
    format!("Native media receipt archive is unavailable or changed: {e}. No picker was reopened")
}
#[derive(Clone)]
pub(super) struct Archive {
    root: PathBuf,
    pinned: Arc<Mutex<Option<Stamp>>>,
    #[cfg(test)]
    pub(super) publish_gate: Option<PublishGate>,
}
#[cfg(test)]
pub(super) type PublishGate = (std::sync::mpsc::Sender<()>, Arc<Mutex<std::sync::mpsc::Receiver<()>>>);
impl Archive {
    pub(super) fn new(root: PathBuf) -> Self {
        Self {
            root,
            pinned: Arc::default(),
            #[cfg(test)]
            publish_gate: None,
        }
    }
    #[cfg(test)]
    pub(super) fn root(&self) -> &Path {
        &self.root
    }
    fn directory(&self, create: bool, pinned: &mut Option<Stamp>) -> Result<Option<fs::Metadata>, String> {
        if !self.root.is_absolute() || self.root.as_os_str().len() > 4096 {
            return Err(error("invalid runtime path"));
        }
        let components: Vec<_> = self.root.components().collect();
        if components.len() > 128 || components.iter().any(|c| matches!(c, Component::ParentDir | Component::CurDir)) {
            return Err(error("invalid runtime path components"));
        }
        let mut prefix = PathBuf::new();
        for c in components.iter().take(components.len().saturating_sub(1)) {
            prefix.push(c.as_os_str());
            let m = fs::symlink_metadata(&prefix).map_err(error)?;
            if !m.is_dir() || m.file_type().is_symlink() {
                return Err(error("redirected runtime directory"));
            }
        }
        let m = match fs::symlink_metadata(&self.root) {
            Ok(m) => m,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound && pinned.is_none() => {
                if !create {
                    return Ok(None);
                }
                let mut builder = fs::DirBuilder::new();
                #[cfg(unix)]
                {
                    use std::os::unix::fs::DirBuilderExt;
                    builder.mode(0o700);
                }
                builder.create(&self.root).map_err(error)?;
                sync_dir(self.root.parent().ok_or_else(|| error("missing runtime parent"))?)?;
                fs::symlink_metadata(&self.root).map_err(error)?
            }
            Err(e) => return Err(error(e)),
        };
        if !m.is_dir() || m.file_type().is_symlink() || !private(&m) {
            return Err(error("non-private archive directory"));
        }
        let current = stamp(&m);
        if pinned.as_ref().is_some_and(|p| p != &current) {
            return Err(error("archive directory changed outside its owner"));
        }
        *pinned = Some(current);
        Ok(Some(m))
    }
    fn load(&self, id: &str, root: &fs::Metadata) -> Result<Option<Entry>, String> {
        let path = self.root.join(format!("{id}.json"));
        let mut options = OpenOptions::new();
        options.read(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
        }
        let file = match options.open(&path) {
            Ok(f) => f,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(error(e)),
        };
        let before = file.metadata().map_err(error)?;
        if !before.is_file() || before.len() == 0 || before.len() > MAX_BYTES || !private(&before) {
            return Err(error("invalid receipt file"));
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            if before.uid() != root.uid() {
                return Err(error("foreign receipt owner"));
            }
        }
        #[cfg(unix)]
        if !before.permissions().readonly() {
            return Err(error("writable receipt file"));
        }
        let mut bytes = Vec::new();
        (&file).take(MAX_BYTES + 1).read_to_end(&mut bytes).map_err(error)?;
        let named = fs::symlink_metadata(&path).map_err(error)?;
        if !named.is_file()
            || named.file_type().is_symlink()
            || stamp(&named) != stamp(&before)
            || stamp(&file.metadata().map_err(error)?) != stamp(&before)
            || bytes.len() as u64 != before.len()
        {
            return Err(error("receipt changed while reading"));
        }
        let envelope: Envelope = serde_json::from_slice(&bytes).map_err(error)?;
        let encoded = serde_json::to_vec(&envelope.entry).map_err(error)?;
        if envelope.kind != "filmcraft-native-media-retired"
            || envelope.version != if matches!(envelope.entry, Entry::Released { .. }) { 2 } else { 1 }
            || envelope.entry.id() != id
            || !envelope.entry.valid()
            || envelope.sha256 != format!("{:x}", Sha256::digest(&encoded))
        {
            return Err(error("corrupt or foreign receipt"));
        }
        Ok(Some(envelope.entry))
    }
    pub(super) fn read(&self, id: &str) -> Result<Option<Entry>, String> {
        if !uuid(id) {
            return Err(error("invalid receipt ID"));
        }
        let mut pinned = self.pinned.lock().unwrap_or_else(|e| e.into_inner());
        let Some(root) = self.directory(false, &mut pinned)? else { return Ok(None) };
        let result = self.load(id, &root)?;
        self.directory(false, &mut pinned)?;
        Ok(result)
    }
    pub(super) fn publish(&self, entries: &[Entry]) -> Result<(), String> {
        #[cfg(test)]
        if let Some((entered, gate)) = &self.publish_gate {
            let _ = entered.send(());
            gate.lock().unwrap_or_else(|e| e.into_inner()).recv().map_err(error)?;
        }
        if entries.is_empty() || entries.len() > 64 {
            return Err(error("invalid retirement batch"));
        }
        let mut pinned = self.pinned.lock().unwrap_or_else(|e| e.into_inner());
        let root = self.directory(true, &mut pinned)?.ok_or_else(|| error("missing archive"))?;
        for entry in entries {
            if !entry.valid() {
                return Err(error("unsettled or foreign receipt"));
            }
            if let Some(old) = self.load(entry.id(), &root)? {
                if old != *entry {
                    return Err(error("retirement identity conflict"));
                }
                continue;
            }
            let body = serde_json::to_vec(entry).map_err(error)?;
            let bytes = serde_json::to_vec(&Envelope {
                kind: "filmcraft-native-media-retired".into(),
                version: if matches!(entry, Entry::Released { .. }) { 2 } else { 1 },
                sha256: format!("{:x}", Sha256::digest(&body)),
                entry: entry.clone(),
            })
            .map_err(error)?;
            if bytes.len() as u64 > MAX_BYTES {
                return Err(error("oversized receipt"));
            }
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let mut temp = None;
            for _ in 0..4 {
                let path = self.root.join(format!(".{}.{}-{}.tmp", entry.id(), std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
                let mut options = OpenOptions::new();
                options.write(true).create_new(true);
                #[cfg(unix)]
                {
                    use std::os::unix::fs::OpenOptionsExt;
                    options.mode(0o600);
                }
                match options.open(&path) {
                    Ok(f) => {
                        temp = Some((path, f));
                        break;
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                    Err(e) => return Err(error(e)),
                }
            }
            let (temp_path, mut file) = temp.ok_or_else(|| error("temporary file collisions"))?;
            let result = (|| {
                file.write_all(&bytes).map_err(error)?;
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    file.set_permissions(fs::Permissions::from_mode(0o400)).map_err(error)?;
                }
                file.sync_all().map_err(error)?;
                let final_path = self.root.join(format!("{}.json", entry.id()));
                match fs::hard_link(&temp_path, &final_path) {
                    Ok(()) => {}
                    Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                        if self.load(entry.id(), &root)?.as_ref() != Some(entry) {
                            return Err(error("retirement publication conflict"));
                        }
                    }
                    Err(e) => return Err(error(e)),
                }
                sync_dir(&self.root)
            })();
            drop(file);
            let cleanup = fs::remove_file(&temp_path).map_err(error).and_then(|()| sync_dir(&self.root));
            result?;
            cleanup?;
        }
        // Publication is durable before the GUI may remove any active entry.
        let final_root = fs::symlink_metadata(&self.root).map_err(error)?;
        if !final_root.is_dir() || final_root.file_type().is_symlink() || !private(&final_root) {
            return Err(error("archive directory changed during publication"));
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            if final_root.dev() != root.dev() || final_root.ino() != root.ino() || final_root.uid() != root.uid() {
                return Err(error("archive directory identity changed during publication"));
            }
        }
        *pinned = Some(stamp(&final_root));
        Ok(())
    }
}
