//! Actual fresh current-image journal storage. Only the original persistence
//! constructor creates this object; no description grants journal ownership.
use super::*;
use filmcraft_format::retained::{
    directory_effect::{self, OwnedDirectory},
    preview_context::{DirectoryFrame, DirectoryRecord, NodeFrame},
};
use std::sync::atomic::{AtomicBool, Ordering};
pub(crate) mod release;
mod retained_root;
pub(super) use retained_root::PreparedRoot;
pub(crate) struct Journal {
    data: Arc<DirectoryFrame>,
    root: Root,
    session: OwnedDirectory,
    lock: NodeFrame,
    pending: Arc<AtomicBool>,
    live: AtomicBool,
    canonical_preference: std::sync::Mutex<Option<Arc<crate::agent_bridge::ColdPreferenceContinuation>>>,
}
pub(super) struct Created {
    pub session_dir: PathBuf,
    pub lock: File,
    pub journal: Arc<Journal>,
}
fn names(path: &Path, check: &impl Fn() -> io::Result<()>) -> io::Result<Vec<String>> {
    check()?;
    let mut result = Vec::new();
    for item in fs::read_dir(path)? {
        check()?;
        let name = item?.file_name().into_string().map_err(|_| io::Error::other("literal native journal member required"))?;
        result.push(name);
        if result.len() > 1 {
            return Err(io::Error::other("foreign native startup journal member appeared"));
        }
    }
    result.sort();
    Ok(result)
}
pub(super) fn create(cfg: &AutosaveConfig, session_name: &str, pending: Arc<AtomicBool>, check: &impl Fn() -> io::Result<()>) -> io::Result<Created> {
    create_inner(cfg, session_name, pending, None, check)
}
pub(super) fn create_retained(
    cfg: &AutosaveConfig,
    session_name: &str,
    pending: Arc<AtomicBool>,
    root: PreparedRoot,
    check: &impl Fn() -> io::Result<()>,
) -> io::Result<Created> {
    create_inner(cfg, session_name, pending, Some(root), check)
}
enum Root {
    Created(OwnedDirectory),
    Retained(Arc<retained_root::RetainedRoot>),
}
impl Root {
    fn path(&self) -> &Path {
        match self {
            Self::Created(root) => &root.pin().path,
            Self::Retained(root) => root.path(),
        }
    }
    fn current(&self, preference: Option<&crate::agent_bridge::ColdPreferenceContinuation>, check: &impl Fn() -> io::Result<()>) -> io::Result<()> {
        match self {
            Self::Created(root) => root.current(check),
            Self::Retained(root) => root.current_with_preference(preference, check),
        }
    }
    fn namespace(
        &self,
        session_name: &str,
        preference: Option<&crate::agent_bridge::ColdPreferenceContinuation>,
        check: &impl Fn() -> io::Result<()>,
    ) -> io::Result<()> {
        match self {
            Self::Created(root) if names(&root.pin().path, check)? == [session_name] => Ok(()),
            Self::Retained(root) => root.current_with_preference(preference, check),
            _ => Err(io::Error::other("foreign native startup journal member appeared")),
        }
    }
}
fn create_inner(
    cfg: &AutosaveConfig,
    session_name: &str,
    pending: Arc<AtomicBool>,
    prepared: Option<PreparedRoot>,
    check: &impl Fn() -> io::Result<()>,
) -> io::Result<Created> {
    check()?;
    let data = DirectoryFrame::capture(&cfg.data_dir, check)?;
    if !data.exists() {
        return Err(io::Error::other("actual configured current native directory required"));
    }
    let root = match prepared {
        Some(PreparedRoot::Existing(root)) => {
            root.current(check)?;
            Root::Retained(root)
        }
        Some(PreparedRoot::Missing(frame)) => Root::Created(directory_effect::stage(frame, check)?.publish_owned(check)?),
        None => Root::Created(directory_effect::stage(DirectoryFrame::capture(&recovery_root(&cfg.data_dir), check)?, check)?.publish_owned(check)?),
    };
    let session_dir = root.path().join(session_name);
    let session = directory_effect::stage(DirectoryFrame::capture(&session_dir, check)?, check)?.publish_owned(check)?;
    let (lock, observed) = session.create_empty_private_file(LOCK, check)?;
    let journal =
        Arc::new(Journal { data, root, session, lock: observed, pending, live: AtomicBool::new(true), canonical_preference: std::sync::Mutex::new(None) });
    journal.current(check)?;
    Ok(Created { session_dir, lock, journal })
}
impl Journal {
    /// Retain the actual original canonical writer's one-use continuation.
    /// This changes no file or baseline and grants no worker release rights.
    pub(crate) fn retain_canonical_preferences(
        &self,
        data: &DirectoryRecord,
        preference: Arc<crate::agent_bridge::ColdPreferenceContinuation>,
        check: &impl Fn() -> io::Result<()>,
    ) -> io::Result<()> {
        self.current(check)?;
        if !self.belongs_to(data) || !preference.belongs_to_data(data) {
            return Err(io::Error::other("original preference continuation crossed its actual worker data root"));
        }
        self.root.current(Some(&preference), check)?;
        {
            // Never call a reentrant filesystem guard while holding this lock.
            let mut retained = self.canonical_preference.lock().unwrap_or_else(|e| e.into_inner());
            if retained.is_some() {
                return Err(io::Error::other("original worker preference continuation was already retained; no replacement"));
            }
            *retained = Some(preference);
        }
        self.current(check)
    }
    pub(crate) fn description(&self, check: &impl Fn() -> io::Result<()>) -> io::Result<Value> {
        use filmcraft_format::retained::preview_context::NodeState;
        self.current(check)?;
        fn directory(path: &Path, pin: &filmcraft_format::retained::Identity) -> Value {
            json!({"path":path,"physical":{"dev":pin.device,"ino":pin.inode,"mode":pin.mode.to_string(),"uid":pin.uid.to_string()}})
        }
        fn ns(value: &str) -> io::Result<String> {
            let (s, n) = value.split_once(':').ok_or_else(|| io::Error::other("native timestamp required"))?;
            let s = s.parse::<i128>().map_err(io::Error::other)?;
            let n = n.parse::<i128>().map_err(io::Error::other)?;
            if !(0..1_000_000_000).contains(&n) {
                return Err(io::Error::other("native timestamp component out of bounds"));
            }
            s.checked_mul(1_000_000_000).and_then(|s| s.checked_add(n)).map(|v| v.to_string()).ok_or_else(|| io::Error::other("native timestamp overflow"))
        }
        let data = self.data.record();
        let session = self.session.pin();
        let root = DirectoryFrame::capture(self.root.path(), check)?.record();
        let NodeState::File(lock) = self.lock.state() else {
            return Err(io::Error::other("actual native lock file required"));
        };
        let value = json!({"kind":"native-gated-journal","version":1,
            "data":directory(&data.path,&data.anchor.identity),"root":directory(&root.path,&root.anchor.identity),
            "session":directory(&session.path,&session.identity),
            "lock":{"path":self.lock.path(),"sha256":lock.sha256,"bytes":lock.bytes,
                "physical":{"dev":lock.identity.device,"ino":lock.identity.inode,"mode":lock.identity.mode.to_string(),"uid":lock.identity.uid.to_string(),
                "size":lock.bytes.to_string(),"mtimeNs":ns(&lock.modified)?,"ctimeNs":ns(&lock.changed)?}},
            "startupPending":true,"workerMayResume":false,"writeAuthority":false});
        self.current(check)?;
        Ok(value)
    }
    /// Retain the SAME constructor's read-only data frame. This neither
    /// creates a directory nor converts an existing one into write custody.
    pub(in crate::autosave) fn retained_data_frame(&self, check: &impl Fn() -> io::Result<()>) -> io::Result<Arc<DirectoryFrame>> {
        self.current(check)?;
        Ok(self.data.clone())
    }
    pub(crate) fn belongs_to(&self, root: &DirectoryRecord) -> bool {
        self.data.record() == *root
    }
    pub(crate) fn current(&self, check: &impl Fn() -> io::Result<()>) -> io::Result<()> {
        let preference = self.canonical_preference.lock().unwrap_or_else(|e| e.into_inner()).clone();
        check()?;
        if !self.live.load(Ordering::Acquire) || !self.pending.load(Ordering::Acquire) {
            return Err(io::Error::other("actual native journal is no longer a live gated startup"));
        }
        self.data.current(check)?;
        self.root.current(preference.as_deref(), check)?;
        self.session.current(check)?;
        self.lock.current(check)?;
        self.lock.fence(check)?;
        let session_name =
            self.session.pin().path.file_name().and_then(|v| v.to_str()).ok_or_else(|| io::Error::other("literal native journal session required"))?;
        self.root.namespace(session_name, preference.as_deref(), check)?;
        if names(&self.session.pin().path, check)? != vec![LOCK.to_owned()] {
            return Err(io::Error::other("actual gated journal namespace changed"));
        }
        self.data.current(check)?;
        self.root.current(preference.as_deref(), check)?;
        self.session.current(check)?;
        self.lock.fence(check)?;
        if !self.live.load(Ordering::Acquire) || !self.pending.load(Ordering::Acquire) {
            return Err(io::Error::other("native gated journal ended during its observation"));
        }
        check()
    }
    pub(super) fn stopped(&self) {
        self.live.store(false, Ordering::Release);
    }
}
pub(super) struct Liveness(pub(super) Option<Arc<Journal>>);
impl Drop for Liveness {
    fn drop(&mut self) {
        if let Some(journal) = &self.0 {
            journal.stopped();
        }
    }
}
