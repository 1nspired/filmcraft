//! External original-file witnesses retained by a native read, not project data.
//! Precise Unix change identity is the same fast-path policy as original Poema;
//! coarse/non-Unix files require complete streaming SHA-256 reads. A matching
//! filename, size or restored modification time alone is never continuity.
use std::sync::Arc;

use filmcraft_project::Project;

use crate::{Result, commands::bad};

// Advertised to the trusted host so its publication socket can cover the
// existing native verification budget. This does not extend native work.
pub(super) const OPERATION_DEADLINE_MS: u64 = 45_000;
// A native retained source set can require minutes of complete-byte work.
// This changes time only, never witnesses, permissions or publication checks.
pub(super) const LARGE_SOURCE_BYTES: u64 = 1024 * 1024 * 1024;
pub(super) const LARGE_SOURCE_DEADLINE_MS: u64 = 600_000;
pub(super) const LARGE_SOURCE_REPLY_MS: u64 = LARGE_SOURCE_DEADLINE_MS + 2000;
pub(super) const LARGE_SOURCE_POLICY: &str = "large_originals_v1";

pub(super) fn source_deadline(bytes: u64) -> u64 {
    if bytes >= LARGE_SOURCE_BYTES { LARGE_SOURCE_DEADLINE_MS } else { OPERATION_DEADLINE_MS }
}

// Read workers can observe a complete library without changing a publication's
// bound. This policy is fixed and advertised, never supplied by a model/caller.
#[cfg(not(target_arch = "wasm32"))]
pub(super) const SNAPSHOT_READ_DEADLINE_MS: u64 = 300_000;

#[cfg(not(target_arch = "wasm32"))]
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};

#[cfg(not(target_arch = "wasm32"))]
mod parallel;

#[cfg(not(target_arch = "wasm32"))]
struct Input {
    item: u64,
    path: String,
    refused: bool,
}

#[cfg(not(target_arch = "wasm32"))]
impl Input {
    fn heavy(&self) -> bool {
        if self.refused {
            return false;
        }
        // Scheduling hint only. Each actual observation still obtains and
        // checks its own full metadata/bytes; this stat grants no authority.
        let Ok(meta) = std::fs::symlink_metadata(&self.path) else {
            return false;
        };
        if !meta.is_file() || meta.len() < 64 * 1024 * 1024 {
            return false;
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            meta.ctime_nsec() % 1_000_000 == 0
        }
        #[cfg(not(unix))]
        {
            true
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
pub(super) struct Check {
    started: web_time::Instant,
    deadline_ms: u64,
    pub stop: Arc<AtomicBool>,
    pub files: AtomicUsize,
    pub bytes: AtomicU64,
}
#[cfg(not(target_arch = "wasm32"))]
impl Check {
    pub fn new(started: web_time::Instant) -> Self {
        Self::with_stop(started, Arc::new(AtomicBool::new(false)))
    }
    pub fn with_stop(started: web_time::Instant, stop: Arc<AtomicBool>) -> Self {
        Self { started, deadline_ms: OPERATION_DEADLINE_MS, stop, files: AtomicUsize::new(0), bytes: AtomicU64::new(0) }
    }
    pub fn for_snapshot(started: web_time::Instant, bytes_hint: u64) -> Self {
        Self { deadline_ms: SNAPSHOT_READ_DEADLINE_MS.max(source_deadline(bytes_hint)), ..Self::new(started) }
    }
    pub(super) fn for_sources(started: web_time::Instant, stop: Arc<AtomicBool>, bytes: u64) -> Self {
        Self { deadline_ms: source_deadline(bytes), ..Self::with_stop(started, stop) }
    }
    pub(super) fn deadline_ms(&self) -> u64 {
        self.deadline_ms
    }
    pub(super) fn policy(&self) -> Option<serde_json::Value> {
        (self.deadline_ms() == LARGE_SOURCE_DEADLINE_MS).then(|| {
            serde_json::json!({
                "version":1,"class":LARGE_SOURCE_POLICY,"deadline_ms":LARGE_SOURCE_DEADLINE_MS,
                "threshold_bytes":LARGE_SOURCE_BYTES,"writeAuthority":false
            })
        })
    }
    pub(super) fn for_admission_preparation(started: web_time::Instant, stop: Arc<AtomicBool>) -> Self {
        Self { deadline_ms: super::admission_jobs::PREPARE_MS, ..Self::with_stop(started, stop) }
    }
    pub(super) fn for_admission_publication(started: web_time::Instant) -> Self {
        Self { deadline_ms: super::admission_jobs::PUBLICATION_MS, ..Self::new(started) }
    }
    pub fn check(&self) -> Result<()> {
        if self.stop.load(Ordering::Acquire) {
            return Err(error("media-input verification cancelled; no edit was published"));
        }
        if self.started.elapsed() >= std::time::Duration::from_millis(self.deadline_ms) {
            return Err(error("media-input verification exceeded its deadline; no edit was published"));
        }
        Ok(())
    }
}

#[cfg(not(target_arch = "wasm32"))]
#[derive(PartialEq, Eq)]
pub(super) struct Plan {
    scope: Option<Vec<u64>>,
    files: Vec<(u64, String, Option<Option<std::path::PathBuf>>)>,
}
#[cfg(not(target_arch = "wasm32"))]
impl Plan {
    /// Project-only work; no path resolution, stat, open or file read on UI.
    pub fn prepare(project: &Project, pool: &crate::MediaPool) -> Result<Self> {
        Self::selected(project, pool, None)
    }
    pub fn scoped(project: &Project, pool: &crate::MediaPool, items: Vec<u64>) -> Result<Self> {
        if items.len() > 2048 || items.contains(&0) || items.windows(2).any(|pair| pair[0] >= pair[1]) {
            return Err(error("resolved original source scope is not unique bounded sorted IDs"));
        }
        Self::selected(project, pool, Some(items))
    }
    pub fn rebuild(&self, project: &Project, pool: &crate::MediaPool) -> Result<Self> {
        Self::selected(project, pool, self.scope.clone())
    }
    pub fn for_witnesses(project: &Project, pool: &crate::MediaPool, expected: &[Arc<Witnesses>]) -> Result<Self> {
        if expected.is_empty() || expected.iter().any(|w| w.scope.is_none()) {
            return Self::prepare(project, pool);
        }
        let items = expected.iter().flat_map(|w| w.scope.iter().flatten().copied()).collect::<std::collections::BTreeSet<_>>();
        Self::scoped(project, pool, items.into_iter().collect())
    }
    fn selected(project: &Project, pool: &crate::MediaPool, scope: Option<Vec<u64>>) -> Result<Self> {
        use filmcraft_project::MediaRef;
        if project.items.len() > 100_000 {
            return Err(error("project inventory exceeds the complete media verification scan"));
        }
        let mut files = Vec::new();
        let mut path_bytes = 0usize;
        for item in project.items.values() {
            if scope.as_ref().is_some_and(|items| items.binary_search(&item.id.0).is_err()) {
                continue;
            }
            let Some(raw) = item.metadata.get(super::BINDING_KEY) else { continue };
            let binding: super::Binding = serde_json::from_str(raw).map_err(|_| error("malformed original binding"))?;
            if binding.role != "original" {
                continue;
            }
            let Some(media) = item.as_media() else { continue };
            let MediaRef::File { path } = &media.media else { continue };
            let origin = pool.path_origin(path);
            let origin_bytes = origin.as_ref().and_then(Option::as_ref).map_or(0, |p| p.as_os_str().len());
            path_bytes = path_bytes.checked_add(path.len()).and_then(|n| n.checked_add(origin_bytes)).ok_or_else(|| error("original path budget overflow"))?;
            if files.len() >= 2048 || path_bytes > 2 * 1024 * 1024 || path.len() > 4096 || origin_bytes > 4096 || path.chars().any(char::is_control) {
                return Err(error("bound original inputs exceed the complete verification budget"));
            }
            files.push((item.id.0, path.clone(), origin));
        }
        if scope.as_ref().is_some_and(|items| items.len() != files.len()) {
            return Err(error("scoped original file inventory changed or became unavailable"));
        }
        Ok(Self { scope, files })
    }
    pub fn total(&self) -> usize {
        self.files.len().saturating_mul(2)
    }
    pub fn validation_total(&self) -> usize {
        self.files.len()
    }
    /// Bounded, project-only scheduling hint. Stale or overstated media info
    /// changes the worker lifetime only; capture still observes actual bytes.
    pub fn source_bytes_hint(&self, project: &Project) -> u64 {
        self.files.iter().fold(0u64, |bytes, (item, _, _)| {
            bytes.saturating_add(project.item(filmcraft_project::ItemId(*item)).and_then(|it| it.as_media()).and_then(|m| m.info.file_size).unwrap_or(0))
        })
    }
    pub fn capture(&self, check: &Check) -> Result<Arc<Witnesses>> {
        let inputs = self.inputs(check)?;
        let parallel = inputs.iter().filter(|input| input.heavy()).take(2).count() == 2;
        self.capture_inputs(&inputs, check, parallel, &|path, check| observe(path, check, false))
    }
    /// Compare a complete current observation with every retained generation.
    /// This cannot issue a new witness/ticket. New snapshots still use capture's
    /// two observations; validation already has the expected initial evidence.
    pub fn recheck(&self, expected: &[&Witnesses], check: &Check) -> Result<()> {
        let inputs = self.inputs(check)?;
        let parallel = inputs.iter().filter(|input| input.heavy()).take(2).count() == 2;
        self.recheck_inputs(&inputs, expected, check, parallel, &|path, check| observe(path, check, false))
    }
    fn inputs(&self, check: &Check) -> Result<Vec<Input>> {
        let mut inputs = Vec::with_capacity(self.files.len());
        let mut path_bytes = 0usize;
        for (item, raw, origin) in &self.files {
            check.check()?;
            let (path, refused) = match crate::media_pool::follow_origin(raw, origin.as_ref()) {
                Ok(file) => {
                    path_bytes =
                        path_bytes.checked_add(raw.len()).and_then(|n| n.checked_add(file.len())).ok_or_else(|| error("resolved path budget overflow"))?;
                    if file.len() > 4096 || path_bytes > 2 * 1024 * 1024 || !std::path::Path::new(&file).is_absolute() || file.chars().any(char::is_control) {
                        return Err(error("resolved original path exceeds its verification contract"));
                    }
                    (file, false)
                }
                Err(_) => (raw.clone(), true),
            };
            inputs.push(Input { item: *item, path, refused });
        }
        Ok(inputs)
    }
    fn observe_inputs(
        &self,
        inputs: &[Input],
        check: &Check,
        parallel: bool,
        observer: &(impl Fn(&std::path::Path, &Check) -> Result<State> + Sync),
    ) -> Result<Vec<Row>> {
        if inputs.len() != self.files.len() || inputs.iter().zip(&self.files).any(|(input, (item, _, _))| input.item != *item) {
            return Err(error("original observer inputs differ from the complete declared plan"));
        }
        parallel::sweep(inputs.len(), parallel, |index| {
            check.check()?;
            let input = inputs.get(index).ok_or_else(|| error("missing original observer input"))?;
            let state = if input.refused { State::PathRefused } else { observer(std::path::Path::new(&input.path), check)? };
            check.files.fetch_add(1, Ordering::Relaxed);
            Ok(Row { item: input.item, path: input.path.clone(), state })
        })
    }
    fn capture_inputs(
        &self,
        inputs: &[Input],
        check: &Check,
        parallel: bool,
        observer: &(impl Fn(&std::path::Path, &Check) -> Result<State> + Sync),
    ) -> Result<Arc<Witnesses>> {
        let rows = self.observe_inputs(inputs, check, parallel, observer)?;
        parallel::sweep(rows.len(), parallel, |index| {
            check.check()?;
            let row = rows.get(index).ok_or_else(|| error("missing original observation"))?;
            if row.state != State::PathRefused && observer(std::path::Path::new(&row.path), check)? != row.state {
                return Err(error("original inputs changed during verification"));
            }
            check.files.fetch_add(1, Ordering::Relaxed);
            Ok(())
        })?;
        check.check()?;
        Ok(Arc::new(Witnesses { scope: self.scope.clone(), rows, ..Witnesses::default() }))
    }
    fn recheck_inputs(
        &self,
        inputs: &[Input],
        expected: &[&Witnesses],
        check: &Check,
        parallel: bool,
        observer: &(impl Fn(&std::path::Path, &Check) -> Result<State> + Sync),
    ) -> Result<()> {
        if expected.is_empty() {
            return Err(error("file validation requires retained original evidence; no new witness was issued"));
        }
        for previous in expected {
            admissible(previous)?;
        }
        let rows = self.observe_inputs(inputs, check, parallel, observer)?;
        let current = Witnesses { scope: self.scope.clone(), rows, ..Witnesses::default() };
        for previous in expected {
            compare(previous, &current)?;
        }
        check.check()
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
#[path = "media_guard_tests.rs"]
mod tests;

#[derive(Default)]
pub(super) struct Witnesses {
    scope: Option<Vec<u64>>,
    #[cfg(not(target_arch = "wasm32"))]
    rows: Vec<Row>,
    // Full hashes learned on demand belong to these immutable observed rows.
    // Cache mutation is not a new file witness and is excluded from equality.
    #[cfg(not(target_arch = "wasm32"))]
    generations: Arc<std::sync::Mutex<std::collections::BTreeMap<u64, String>>>,
}
impl PartialEq for Witnesses {
    fn eq(&self, other: &Self) -> bool {
        #[cfg(not(target_arch = "wasm32"))]
        {
            self.scope == other.scope && self.rows == other.rows
        }
        #[cfg(target_arch = "wasm32")]
        {
            self.scope == other.scope
        }
    }
}
impl Eq for Witnesses {}

impl Witnesses {
    /// Historical Root source tickets carry no file observations. Keep that
    /// explicit even for an empty library; a consumed receipt is never a read.
    #[cfg(not(target_arch = "wasm32"))]
    pub(super) fn unqualified() -> Self {
        Self { scope: Some(Vec::new()), ..Self::default() }
    }
    // Native captured rows only. This is a scheduling estimate, not fresh
    // evidence; all original-byte comparisons still run before publication.
    pub(super) fn source_bytes(&self) -> u64 {
        #[cfg(not(target_arch = "wasm32"))]
        return self.rows.iter().fold(0u64, |bytes, row| match &row.state {
            State::Regular(stamp, _) => bytes.saturating_add(stamp.bytes),
            _ => bytes,
        });
        #[cfg(target_arch = "wasm32")]
        0
    }
    // Select expected generations already observed by this native session.
    // This observes no files and cannot assert current source qualification.
    #[cfg(not(target_arch = "wasm32"))]
    pub(super) fn retained_subset(expected: &Arc<Self>, items: Vec<u64>) -> Option<Arc<Self>> {
        if items.len() > 2048 || items.contains(&0) || items.windows(2).any(|p| p[0] >= p[1]) {
            return None;
        }
        let rows = items
            .iter()
            .map(|id| expected.rows.iter().find(|r| r.item == *id && matches!(r.state, State::Regular(..))).cloned())
            .collect::<Option<Vec<_>>>()?;
        if expected.scope.as_ref() == Some(&items) {
            return Some(expected.clone());
        }
        Some(Arc::new(Self { scope: Some(items), rows, generations: expected.generations.clone() }))
    }
    pub(super) fn is_scoped(&self) -> bool {
        self.scope.is_some()
    }
    pub(super) fn qualification(&self, item: u64) -> &'static str {
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(row) = self.rows.iter().find(|r| r.item == item) {
            return if matches!(row.state, State::Regular(..)) { "verified" } else { "unavailable" };
        }
        let _ = item;
        "unverified"
    }
    pub(super) fn scope_json(&self) -> serde_json::Value {
        #[cfg(not(target_arch = "wasm32"))]
        let complete = self.scope.is_none() && self.rows.iter().all(|row| matches!(row.state, State::Regular(..)));
        #[cfg(target_arch = "wasm32")]
        let complete = false;
        serde_json::json!({"kind":if self.scope.is_some(){"dependencies"}else{"library"},"items":self.scope,
            "libraryVerified":complete})
    }
    pub(super) fn require(&self, items: &[u64]) -> Result<()> {
        if items.iter().any(|id| self.qualification(*id) != "verified") {
            return Err(error("operation needs original files outside this verified source scope; take an explicitly expanded fresh read"));
        }
        Ok(())
    }
    // Compare the original owner's full generation, never merely its size or
    // metadata. A precise native row may initially lack a hash; learn it from
    // complete bytes on the exclusive writer and bind it to the exact row stamp.
    // This cache cannot issue another ticket or bypass final file validation.
    #[cfg(not(target_arch = "wasm32"))]
    pub(super) fn require_generation(&self, item: u64, path: &str, bytes: u64, sha256: &str, check: &Check) -> Result<()> {
        let row = self.rows.iter().find(|r| r.item == item).ok_or_else(|| error("original is outside this native read's byte scope"))?;
        let State::Regular(stamp, known) = &row.state else {
            return Err(error("original has no regular retained file witness"));
        };
        if row.path != path || stamp.bytes != bytes {
            return Err(error("original owner's byte generation differs from this native source witness"));
        }
        check.check()?;
        let cached = known.clone().or_else(|| self.generations.lock().unwrap_or_else(|e| e.into_inner()).get(&item).cloned());
        let actual = if let Some(hash) = cached {
            hash
        } else {
            let current = observe(std::path::Path::new(path), check, true)?;
            let State::Regular(now, Some(hash)) = current else {
                return Err(error("complete original byte identification failed"));
            };
            if now != *stamp {
                return Err(error("original changed before its owner generation was qualified"));
            }
            self.generations.lock().unwrap_or_else(|e| e.into_inner()).insert(item, hash.clone());
            hash
        };
        if actual != sha256 {
            return Err(error("original owner's byte generation differs from actual complete native bytes"));
        }
        Ok(())
    }
}

#[cfg(not(target_arch = "wasm32"))]
#[derive(Clone, PartialEq, Eq)]
struct Row {
    item: u64,
    path: String,
    state: State,
}

#[cfg(not(target_arch = "wasm32"))]
#[derive(Clone, PartialEq, Eq)]
enum State {
    PathRefused,
    Missing,
    Unavailable(std::io::ErrorKind),
    NonRegular,
    Regular(Stamp, Option<String>),
}

#[cfg(not(target_arch = "wasm32"))]
#[derive(Clone, PartialEq, Eq)]
pub(super) struct Stamp {
    real: std::path::PathBuf,
    bytes: u64,
    modified: std::time::SystemTime,
    created: Option<std::time::SystemTime>,
    #[cfg(unix)]
    unix: (u64, u64, i64, i64, i64, i64),
}

#[cfg(not(target_arch = "wasm32"))]
pub(super) fn stamp(path: &std::path::Path, m: &std::fs::Metadata) -> std::io::Result<Stamp> {
    #[cfg(unix)]
    use std::os::unix::fs::MetadataExt;
    Ok(Stamp {
        real: std::fs::canonicalize(path)?,
        bytes: m.len(),
        modified: m.modified()?,
        created: m.created().ok(),
        #[cfg(unix)]
        unix: (m.dev(), m.ino(), m.mtime(), m.mtime_nsec(), m.ctime(), m.ctime_nsec()),
    })
}

#[cfg(not(target_arch = "wasm32"))]
fn precise(s: &Stamp) -> bool {
    #[cfg(unix)]
    {
        s.unix.5 % 1_000_000 != 0
    }
    #[cfg(not(unix))]
    {
        let _ = s;
        false
    }
}

fn error(message: impl Into<String>) -> crate::EngineError {
    bad("agent.mediaInputs", message)
}

#[cfg(not(target_arch = "wasm32"))]
fn open_original(path: &std::path::Path) -> Result<std::fs::File> {
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    options.open(path).map_err(|_| error("original file became unavailable or redirected before byte verification"))
}

#[cfg(not(target_arch = "wasm32"))]
fn hash_bytes(file: &mut impl std::io::Read, expected: u64, check: &Check) -> Result<String> {
    use sha2::{Digest, Sha256};
    let mut digest = Sha256::new();
    let mut buffer = vec![0u8; 256 * 1024];
    let mut bytes = 0u64;
    loop {
        check.check()?;
        let n = file.read(&mut buffer).map_err(|_| error("original file could not be completely verified"))?;
        if n == 0 {
            break;
        }
        bytes = bytes.checked_add(n as u64).ok_or_else(|| error("original byte count overflow"))?;
        if bytes > expected {
            return Err(error("original grew during byte verification"));
        }
        digest.update(buffer.get(..n).ok_or_else(|| error("invalid original read length"))?);
        let _ = check.bytes.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |total| Some(total.saturating_add(n as u64)));
    }
    if bytes != expected {
        return Err(error("original changed during byte verification"));
    }
    check.check()?;
    Ok(format!("{:x}", digest.finalize()))
}

#[cfg(not(target_arch = "wasm32"))]
fn observe(path: &std::path::Path, check: &Check, force_hash: bool) -> Result<State> {
    use std::fs;
    check.check()?;
    let meta = match fs::symlink_metadata(path) {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(State::Missing),
        Err(e) => return Ok(State::Unavailable(e.kind())),
    };
    if !meta.is_file() {
        return Ok(State::NonRegular);
    }
    let before = stamp(path, &meta).map_err(|_| error("original file changed or became unavailable during observation"))?;
    check.check()?;
    let mut hash = None;
    if force_hash || !precise(&before) {
        // Fixed memory, complete bytes, checked accounting and a total budget.
        // Never substitute a prefix/sample hash for an original generation.
        let mut file = open_original(path)?;
        let opened = file.metadata().map_err(|_| error("original file metadata unavailable"))?;
        if !opened.is_file() || stamp(path, &opened).map_err(|_| error("original file moved while opening"))? != before {
            return Err(error("original file changed before byte verification"));
        }
        hash = Some(hash_bytes(&mut file, before.bytes, check)?);
        if stamp(path, &file.metadata().map_err(|_| error("original file disappeared"))?).map_err(|_| error("original moved"))? != before {
            return Err(error("original changed during byte verification"));
        }
    }
    let after = fs::symlink_metadata(path).map_err(|_| error("original disappeared during verification"))?;
    if !after.is_file() || stamp(path, &after).map_err(|_| error("original moved during verification"))? != before {
        return Err(error("original changed during verification"));
    }
    check.check()?;
    Ok(State::Regular(before, hash))
}

// An unbound import is absent from an ordinary read's original-file plan.
// Admission therefore retains its own COMPLETE byte observation, then compares
// it again immediately before publication. It never adopts a prefix identity.
#[cfg(not(target_arch = "wasm32"))]
pub(super) struct AdmissionWitness {
    path: std::path::PathBuf,
    state: State,
}
#[cfg(not(target_arch = "wasm32"))]
impl AdmissionWitness {
    pub(super) fn source_bytes(&self) -> u64 {
        match &self.state {
            State::Regular(stamp, _) => stamp.bytes,
            _ => 0,
        }
    }
}
#[cfg(not(target_arch = "wasm32"))]
pub(super) fn admit(path: &std::path::Path, bytes: u64, sha256: &str, check: &Check) -> Result<Arc<AdmissionWitness>> {
    let state = observe(path, check, true)?;
    if !matches!(&state, State::Regular(stamp, Some(hash)) if stamp.bytes == bytes && hash == sha256) {
        return Err(error("new original does not match the complete owner-ingest byte generation"));
    }
    Ok(Arc::new(AdmissionWitness { path: path.into(), state }))
}
#[cfg(not(target_arch = "wasm32"))]
pub(super) fn validate_admission(expected: &AdmissionWitness, check: &Check) -> Result<()> {
    if observe(&expected.path, check, true)? != expected.state {
        return Err(error("new original changed after admission staging; no binding was published"));
    }
    Ok(())
}

pub(super) fn capture(project: &Project, pool: &crate::MediaPool) -> Result<Arc<Witnesses>> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        Plan::prepare(project, pool)?.capture(&Check::new(web_time::Instant::now()))
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = (project, pool);
        Ok(Arc::default())
    }
}

pub(super) fn capture_scoped(project: &Project, pool: &crate::MediaPool, items: Vec<u64>) -> Result<Arc<Witnesses>> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        Plan::scoped(project, pool, items)?.capture(&Check::new(web_time::Instant::now()))
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = (project, pool, items);
        Err(error("explicit original-file dependency verification requires a native engine"))
    }
}

pub(super) fn validate(project: &Project, pool: &crate::MediaPool, expected: &Witnesses) -> Result<()> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        validate_checked(project, pool, expected, &Check::new(web_time::Instant::now()))
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = (project, pool, expected);
        Ok(())
    }
}

#[cfg(not(target_arch = "wasm32"))]
pub(super) fn validate_checked(project: &Project, pool: &crate::MediaPool, expected: &Witnesses, check: &Check) -> Result<()> {
    admissible(expected)?;
    Plan::selected(project, pool, expected.scope.clone())?.recheck(&[expected], check)
}

#[cfg(not(target_arch = "wasm32"))]
pub(super) fn admissible(expected: &Witnesses) -> Result<()> {
    if expected.rows.iter().any(|r| r.state == State::PathRefused) {
        return Err(error("the native media resolver refuses an original path; no current file authority or edit was granted"));
    }
    Ok(())
}
#[cfg(not(target_arch = "wasm32"))]
pub(super) fn compare(expected: &Witnesses, current: &Witnesses) -> Result<()> {
    admissible(expected)?;
    if expected.rows.iter().any(|row| current.rows.iter().find(|r| r.item == row.item) != Some(row)) {
        return Err(error("original file inputs changed since the native read; take a fresh read and requalify evidence; no edit was published"));
    }
    Ok(())
}

// A new file witness must not inherit a decoder cached only by item/path.
// This is cache invalidation, never a project/view/history change. Existing
// jobs keep their old source Arcs but their retained witness refuses delivery.
pub(super) fn forget_changed(pool: &crate::MediaPool, current: &Witnesses, previous: Option<&Witnesses>) {
    if previous == Some(current) {
        return;
    }
    #[cfg(not(target_arch = "wasm32"))]
    for row in &current.rows {
        if previous.and_then(|p| p.rows.iter().find(|r| r.item == row.item)) != Some(row) {
            pool.remove(filmcraft_project::ItemId(row.item));
        }
    }
    #[cfg(target_arch = "wasm32")]
    let _ = (pool, current, previous);
}
