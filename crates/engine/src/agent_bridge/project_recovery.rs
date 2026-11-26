//! Native-only recovery comparisons. No model/RPC command, restore or activation.
//! A cold snapshot must authenticate this basis separately; equal metadata alone
//! never confers document, retained-history or source authority.
use super::*;
use crate::saved_file::{Baseline, sha256};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    fs::{self, OpenOptions},
    io::Read,
    path::{Path, PathBuf},
};
const MAX_FILE: usize = 32 * 1024 * 1024;
const MAX_META: usize = 64 * 1024;
const MAX_JOURNALS: usize = 256;
mod journal_recovery;
mod restoration;
pub(super) use journal_recovery::{JournalIdentity, read_newer_journal};
pub(super) use restoration::restore_basis;
fn fail(message: impl Into<String>) -> crate::EngineError {
    bad("native.projectRecoveryBasis", message)
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Stamp {
    length: u64,
    modified: String,
    #[cfg(unix)]
    device: String,
    #[cfg(unix)]
    inode: String,
    #[cfg(unix)]
    changed: String,
}
fn stamp(meta: &fs::Metadata) -> Result<Stamp> {
    let modified = meta
        .modified()
        .and_then(|v| v.duration_since(std::time::UNIX_EPOCH).map_err(std::io::Error::other))
        .map_err(|e| fail(e.to_string()))?
        .as_nanos()
        .to_string();
    #[cfg(unix)]
    use std::os::unix::fs::MetadataExt;
    Ok(Stamp {
        length: meta.len(),
        modified,
        #[cfg(unix)]
        device: meta.dev().to_string(),
        #[cfg(unix)]
        inode: meta.ino().to_string(),
        #[cfg(unix)]
        changed: format!("{}:{}", meta.ctime(), meta.ctime_nsec()),
    })
}
fn directory(path: &Path) -> Result<()> {
    if !path.is_absolute()
        || !fs::symlink_metadata(path).map_err(|e| fail(e.to_string()))?.is_dir()
        || fs::canonicalize(path).map_err(|e| fail(e.to_string()))? != path
    {
        return Err(fail("unredirected absolute native persistence directory required"));
    }
    Ok(())
}
fn exists(path: &Path) -> Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(fail(e.to_string())),
    }
}
fn regular(path: &Path, max: usize, check: &media_guard::Check) -> Result<(Vec<u8>, Stamp)> {
    check.check()?;
    let before = fs::symlink_metadata(path).map_err(|e| fail(e.to_string()))?;
    if !before.is_file() || before.len() > max as u64 {
        return Err(fail("native recovery input is redirected, nonregular or oversized"));
    }
    let first = stamp(&before)?;
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let mut file = options.open(path).map_err(|e| fail(e.to_string()))?;
    if !file.metadata().map_err(|e| fail(e.to_string()))?.is_file() || stamp(&file.metadata().map_err(|e| fail(e.to_string()))?)? != first {
        return Err(fail("native recovery file changed before reading"));
    }
    let mut bytes = Vec::new();
    let mut chunk = [0u8; 65536];
    loop {
        check.check()?;
        let size = file.read(&mut chunk).map_err(|e| fail(e.to_string()))?;
        if size == 0 {
            break;
        }
        if bytes.len().checked_add(size).is_none_or(|v| v > max) {
            return Err(fail("native recovery input exceeds byte bound"));
        }
        bytes.extend_from_slice(chunk.get(..size).ok_or_else(|| fail("invalid native recovery read"))?);
    }
    let after = fs::symlink_metadata(path).map_err(|e| fail(e.to_string()))?;
    if !after.is_file() || stamp(&after)? != first || stamp(&file.metadata().map_err(|e| fail(e.to_string()))?)? != first || bytes.len() as u64 != first.length
    {
        return Err(fail("native recovery input changed while reading"));
    }
    Ok((bytes, first))
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Journal {
    directory: String,
    revision: u64,
    project_path: Option<String>,
    snapshot_sha256: String,
    metadata_sha256: String,
    project_sha256: String,
    #[serde(default)]
    clean_exit: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    metadata_without_clean_exit_sha256: Option<String>,
}
fn graph_hash(project: &filmcraft_project::Project) -> Result<String> {
    crate::search_organization::validate_inventory(project)?;
    Ok(sha256(&history_checkpoint::project_bytes(project, MAX_FILE)?))
}
fn unlocked(path: &Path) -> Result<()> {
    let meta = fs::symlink_metadata(path).map_err(|e| fail(e.to_string()))?;
    if !meta.is_file() {
        return Err(fail("native recovery lock is not a regular file"));
    }
    let mut options = OpenOptions::new();
    options.read(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let file = options.open(path).map_err(|e| fail(e.to_string()))?;
    if !file.metadata().map_err(|e| fail(e.to_string()))?.is_file() || stamp(&file.metadata().map_err(|e| fail(e.to_string()))?)? != stamp(&meta)? {
        return Err(fail("native recovery lock changed"));
    }
    file.try_lock().map_err(|_| fail("native persistence owner is still active or its lock cannot be established"))?;
    if stamp(&fs::symlink_metadata(path).map_err(|e| fail(e.to_string()))?)? != stamp(&meta)? {
        return Err(fail("native recovery lock changed during probe"));
    }
    // Drop releases only this temporary probe lock, never the source's owner.
    Ok(())
}
fn scan(data: &Path, own: &str, capture: bool, check: &media_guard::Check) -> Result<Vec<Journal>> {
    directory(data)?;
    let root = crate::autosave::recovery_root(data);
    directory(&root)?;
    let mut dirs = Vec::new();
    for entry in fs::read_dir(&root).map_err(|e| fail(e.to_string()))? {
        check.check()?;
        let entry = entry.map_err(|e| fail(e.to_string()))?;
        if dirs.len() >= MAX_JOURNALS {
            return Err(fail("native recovery journal inventory exceeds its bound"));
        }
        let name = entry.file_name().into_string().map_err(|_| fail("native recovery directory name is not UTF-8"))?;
        if name == ".DS_Store" && entry.file_type().map_err(|e| fail(e.to_string()))?.is_file() {
            continue;
        }
        if name.is_empty() || name.len() > 128 || !name.bytes().all(|v| v.is_ascii_digit() || v == b'-') {
            return Err(fail("unrecognized native recovery directory"));
        }
        directory(&entry.path())?;
        dirs.push((name, entry.path()));
    }
    dirs.sort_by(|a, b| a.0.cmp(&b.0));
    let mut result = Vec::new();
    for (name, dir) in dirs {
        check.check()?;
        let lock = dir.join("lock");
        let snap = dir.join("snapshot.fcproj");
        let meta = dir.join("session.json");
        let has_snap = exists(&snap)?;
        let has_meta = exists(&meta)?;
        if capture && name == own {
            if !exists(&lock)? {
                return Err(fail("source native persistence lock disappeared"));
            }
        } else if exists(&lock)? {
            unlocked(&lock)?;
        } else if has_snap || has_meta {
            return Err(fail("native recovery journal has no owner lock"));
        }
        if !has_snap && !has_meta {
            continue;
        }
        if !has_snap || !has_meta {
            return Err(fail("incomplete native recovery journal; preserve it for explicit reconciliation"));
        }
        let (snapshot, _) = regular(&snap, MAX_FILE, check)?;
        let (metadata, _) = regular(&meta, MAX_META, check)?;
        let record: crate::autosave::JournalMeta = serde_json::from_slice(&metadata).map_err(|e| fail(e.to_string()))?;
        if record.revision == 0 || record.bytes != snapshot.len() {
            return Err(fail("native recovery metadata does not describe its complete snapshot"));
        }
        let loaded = filmcraft_format::decode(&snapshot).map_err(|e| fail(e.to_string()))?;
        let mut normalized_meta: Value = serde_json::from_slice(&metadata).map_err(|e| fail(e.to_string()))?;
        normalized_meta["cleanExit"] = json!(false);
        let metadata_without_clean_exit_sha256 = Some(sha256(&history_checkpoint::bytes(&normalized_meta, MAX_META)?));
        result.push(Journal {
            directory: name,
            revision: record.revision,
            project_path: record.project_path,
            snapshot_sha256: sha256(&snapshot),
            metadata_sha256: sha256(&metadata),
            project_sha256: graph_hash(&loaded.project)?,
            clean_exit: record.clean_exit,
            metadata_without_clean_exit_sha256,
        });
    }
    Ok(result)
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PreferenceFile {
    path: PathBuf,
    sha256: Option<String>,
    stamp: Option<Stamp>,
}
fn preference_file(path: &Path, check: &media_guard::Check) -> Result<PreferenceFile> {
    directory(path.parent().ok_or_else(|| fail("preference parent missing"))?)?;
    if !exists(path)? {
        return Ok(PreferenceFile { path: path.to_path_buf(), sha256: None, stamp: None });
    }
    let (raw, stamp) = regular(path, 2 * 1024 * 1024, check)?;
    Ok(PreferenceFile { path: path.to_path_buf(), sha256: Some(sha256(&raw)), stamp: Some(stamp) })
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Basis {
    kind: String,
    version: u8,
    scope: crate::project_workspace::ProjectScope,
    saved_file: Baseline,
    file_stamp: Stamp,
    native_session: String,
    revision: u64,
    saved_revision: u64,
    sequence: Option<u64>,
    project_sha256: String,
    data_directory: PathBuf,
    session_directory: String,
    journals: Vec<Journal>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    preferences_file: Option<PreferenceFile>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    journal_companion: Option<JournalCompanion>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct JournalCompanion {
    path: PathBuf,
    sha256: String,
    stamp: Stamp,
}
fn file_matches(b: &Baseline, check: &media_guard::Check) -> Result<Stamp> {
    let path = Path::new(&b.path);
    directory(path.parent().ok_or_else(|| fail("saved native parent required"))?)?;
    if !path.is_absolute()
        || !b.path.ends_with(".fcproj")
        || b.path.len() > 16384
        || b.path.chars().any(char::is_control)
        || b.sha256.len() != 64
        || !b.sha256.bytes().all(|c| c.is_ascii_hexdigit())
    {
        return Err(fail("explicit bounded native saved-file baseline required"));
    }
    let (bytes, stamp) = regular(path, MAX_FILE, check)?;
    if sha256(&bytes) != b.sha256 {
        return Err(fail("saved project bytes changed after native open/save; no recovery basis is valid"));
    }
    Ok(stamp)
}
pub(super) fn capture(s: &mut Session, p: &Value) -> Result<Value> {
    capture_inner(s, p, false)
}
pub(super) fn capture_held(s: &mut Session, p: &Value) -> Result<Value> {
    capture_inner(s, p, true)
}
fn capture_inner(s: &mut Session, p: &Value, held: bool) -> Result<Value> {
    let scope: crate::project_workspace::ProjectScope = parse("native.projectRecoveryBasis", p)?;
    if s.view_only
        || (!held && s.is_display_only())
        || held && !workspace_update::paused_project_owner(s, &scope)
        || s.matches_project_workspace(&scope.project_id, &scope.workspace_id)? != Some(true)
    {
        return Err(fail("actual independent native project owner required"));
    }
    workspace_update::require_idle_writer_jobs(s)?;
    let check = media_guard::Check::new(web_time::Instant::now());
    s.sync_persistence();
    let per = s.persistence.as_ref().ok_or_else(|| fail("native persistence must be running before capture"))?;
    if !held {
        per.pause_workspace(|| check.check().map_err(|e| e.to_string())).map_err(fail)?;
    }
    // Every fallible step is inside this result so the worker resumes on errors.
    let result = (|| {
        s.apply_persistence_events();
        let saved = s
            .saved_file
            .clone()
            .filter(|b| s.path.as_ref() == Some(&b.path))
            .ok_or_else(|| fail("no native-observed saved-file baseline; an arbitrary disk hash cannot substitute"))?;
        let first = file_matches(&saved, &check)?;
        let per = s.persistence.as_ref().ok_or_else(|| fail("source native persistence disappeared"))?;
        let data = per.data_dir.clone();
        let own = per.session_dir.file_name().and_then(|v| v.to_str()).ok_or_else(|| fail("invalid native source session directory"))?.to_owned();
        if per.session_dir != crate::autosave::recovery_root(&data).join(&own) {
            return Err(fail("native persistence session directory is redirected"));
        }
        directory(&per.session_dir)?;
        let own_lock = fs::symlink_metadata(per.session_dir.join("lock")).map_err(|e| fail(e.to_string()))?;
        if !own_lock.is_file() {
            return Err(fail("native source persistence lock is redirected"));
        }
        let pref_path = data.join("preferences.json");
        if s.prefs_path.as_ref() != Some(&pref_path) {
            return Err(fail("native preference owner differs from persistence root"));
        }
        let preferences_file = preference_file(&pref_path, &check)?;
        if preferences_file.sha256.is_some() {
            let (raw, _) = regular(&pref_path, 2 * 1024 * 1024, &check)?;
            let stored: Value = serde_json::from_slice(&raw).map_err(|e| fail(e.to_string()))?;
            if stored != serde_json::to_value(&s.prefs).map_err(|e| fail(e.to_string()))? {
                return Err(fail("persisted native preferences differ from the current owner; nothing captured"));
            }
        }
        let journals = scan(&data, &own, true, &check)?;
        let project_sha256 = graph_hash(&s.project)?;
        if file_matches(&saved, &check)? != first || scan(&data, &own, true, &check)? != journals || preference_file(&pref_path, &check)? != preferences_file {
            return Err(fail("native persistence changed during basis capture"));
        }
        if s.state.active_sequence.is_some_and(|id| id.0 == 0 || id.0 > 9_007_199_254_740_991 || s.project.sequence(id).is_none()) {
            return Err(fail("native recovery view is invalid"));
        }
        if s.revision == 0 || s.saved_revision > s.revision {
            return Err(fail("invalid native recovery revision"));
        }
        let basis = Basis {
            kind: "poema-native-project-recovery-basis".into(),
            version: 1,
            scope,
            saved_file: saved,
            file_stamp: first,
            native_session: history_checkpoint::epoch(s)?,
            revision: s.revision,
            saved_revision: s.saved_revision,
            sequence: s.state.active_sequence.map(|id| id.0),
            project_sha256,
            data_directory: data,
            session_directory: own,
            journals,
            preferences_file: Some(preferences_file),
            journal_companion: None,
        };
        serde_json::to_value(basis).map_err(|e| fail(e.to_string()))
    })();
    if !held {
        s.persistence.as_ref().ok_or_else(|| fail("source persistence disappeared before resume"))?.resume_workspace().map_err(fail)?;
    }
    result
}
pub(super) fn validate(p: &Value) -> Result<()> {
    validate_inner(p, false)
}
pub(super) fn validate_live(s: &Session, p: &Value) -> Result<()> {
    let b: Basis = parse("native.projectRecoveryBasis", p)?;
    if !workspace_update::paused_project_owner(s, &b.scope)
        || s.path.as_deref() != Some(b.saved_file.path.as_str())
        || s.agent_bridge.epoch.as_deref() != Some(b.native_session.as_str())
        || s.revision != b.revision
        || s.saved_revision != b.saved_revision
        || s.state.active_sequence.map(|v| v.0) != b.sequence
        || graph_hash(&s.project)? != b.project_sha256
    {
        return Err(fail("live native owner differs from the paused recovery basis"));
    }
    validate_inner(p, true)
}
fn validate_inner(p: &Value, live: bool) -> Result<()> {
    history_checkpoint::bytes(p, 512 * 1024)?;
    let basis: Basis = parse("native.projectRecoveryBasis", p)?;
    validate_shape(&basis)?;
    let check = media_guard::Check::new(web_time::Instant::now());
    journal_recovery::validate_companion(&basis, &check)?;
    if let Some(pref) = &basis.preferences_file
        && (pref.path != basis.data_directory.join("preferences.json") || preference_file(&pref.path, &check)? != *pref)
    {
        return Err(fail("later native preference changes exist; nothing recovered"));
    }
    if file_matches(&basis.saved_file, &check)? != basis.file_stamp {
        return Err(fail("native saved-file identity changed; preserve later human work"));
    }
    let now = scan(&basis.data_directory, &basis.session_directory, live, &check)?;
    if basis.journals.iter().any(|j| j.directory == basis.session_directory) && !now.iter().any(|j| j.directory == basis.session_directory) {
        return Err(fail("original source journal disappeared after capture; preserve a later human revert or clean state"));
    }
    let mut old = basis.journals.iter().map(|j| (j.directory.as_str(), j)).collect::<BTreeMap<_, _>>();
    if old.len() != basis.journals.len() {
        return Err(fail("duplicate recovery journal identities"));
    }
    for journal in &now {
        let previous = old.remove(journal.directory.as_str());
        if previous == Some(journal) {
            continue;
        }
        if let Some(old) = previous
            && journal.directory == basis.session_directory
            && !old.clean_exit
            && journal.clean_exit
            && old.revision == journal.revision
            && old.project_path == journal.project_path
            && old.snapshot_sha256 == journal.snapshot_sha256
            && old.project_sha256 == journal.project_sha256
            && old.metadata_without_clean_exit_sha256.is_some()
            && old.metadata_without_clean_exit_sha256 == journal.metadata_without_clean_exit_sha256
        {
            continue;
        }

        if journal.directory == basis.session_directory
            && journal.revision == basis.revision
            && journal.project_path.as_deref() == Some(basis.saved_file.path.as_str())
            && journal.project_sha256 == basis.project_sha256
        {
            continue;
        }
        return Err(fail("later or unknown native recovery work exists; no snapshot may replace it"));
    }
    if old.keys().any(|name| *name != basis.session_directory) {
        return Err(fail("another retained native recovery journal changed or disappeared"));
    }
    if file_matches(&basis.saved_file, &check)? != basis.file_stamp || scan(&basis.data_directory, &basis.session_directory, live, &check)? != now {
        return Err(fail("native persistence changed during cold comparison"));
    }
    if let Some(pref) = &basis.preferences_file
        && preference_file(&pref.path, &check)? != *pref
    {
        return Err(fail("native preferences changed during comparison"));
    }
    journal_recovery::validate_companion(&basis, &check)?;
    Ok(())
}

// Exact persisted inputs needed by the original cold snapshot validator. This
// is private storage metadata, never an edit ticket, live lock or startup grant.
pub(super) fn persistence_dependencies(value: &Value) -> Result<Value> {
    validate_inner(value, true)?;
    let basis: Basis = parse("native.projectRecoveryBasis", value)?;
    let check = media_guard::Check::new(web_time::Instant::now());
    let inventory = || -> Result<Value> {
        let root = crate::autosave::recovery_root(&basis.data_directory);
        directory(&root)?;
        let mut directories = vec![root.clone()];
        let mut children = Vec::new();
        for entry in fs::read_dir(&root).map_err(|e| fail(e.to_string()))? {
            check.check()?;
            let entry = entry.map_err(|e| fail(e.to_string()))?;
            let name = entry.file_name().into_string().map_err(|_| fail("non-UTF8 persistence directory"))?;
            if name == ".DS_Store" && entry.file_type().map_err(|e| fail(e.to_string()))?.is_file() {
                continue;
            }
            if children.len() >= MAX_JOURNALS || name.is_empty() || name.len() > 128 || !name.bytes().all(|c| c.is_ascii_digit() || c == b'-') {
                return Err(fail("unrecognized or excessive native persistence directory"));
            }
            directory(&entry.path())?;
            children.push(entry.path());
        }
        children.sort();
        let mut files = Vec::new();
        let mut absent = Vec::new();
        let mut add = |path: &Path, role: &str, max: usize| -> Result<()> {
            if exists(path)? {
                let (raw, _) = regular(path, max, &check)?;
                files.push(json!({"role":role,"path":path,"sha256":sha256(&raw),"bytes":raw.len()}));
            } else {
                absent.push(path.to_path_buf());
            }
            Ok(())
        };
        add(&basis.data_directory.join("preferences.json"), "preferences", 2 * 1024 * 1024)?;
        for dir in &children {
            check.check()?;
            directories.push(dir.clone());
            let mut count = 0usize;
            for entry in fs::read_dir(dir).map_err(|e| fail(e.to_string()))? {
                check.check()?;
                let entry = entry.map_err(|e| fail(e.to_string()))?;
                count = count.checked_add(1).filter(|n| *n <= 5).ok_or_else(|| fail("unindexed native persistence members"))?;
                let name = entry.file_name().into_string().map_err(|_| fail("non-UTF8 persistence member"))?;
                if !["lock", "snapshot.fcproj", "session.json", crate::agent_bridge::NATIVE_JOURNAL_FILE, ".DS_Store"].contains(&name.as_str())
                    || !entry.file_type().map_err(|e| fail(e.to_string()))?.is_file()
                {
                    return Err(fail("unindexed or redirected native persistence member"));
                }
            }
            if exists(&dir.join(crate::agent_bridge::NATIVE_JOURNAL_FILE))? && !exists(&dir.join("snapshot.fcproj"))? {
                return Err(fail("native persistence companion lacks its journal snapshot"));
            }
            for (name, role, max) in [
                ("lock", "journal-lock", MAX_META),
                ("snapshot.fcproj", "journal-snapshot", MAX_FILE),
                ("session.json", "journal-metadata", MAX_META),
                (crate::agent_bridge::NATIVE_JOURNAL_FILE, "journal-companion", 80 * 1024 * 1024),
            ] {
                add(&dir.join(name), role, max)?;
            }
        }
        files.sort_by(|a, b| a["path"].as_str().cmp(&b["path"].as_str()));
        absent.sort();
        Ok(json!({"kind":"native_project_recovery_persistence","version":1,
            "dataDirectory":basis.data_directory,"sessionDirectory":basis.session_directory,
            "directories":directories,"files":files,"absent":absent}))
    };
    let first = inventory()?;
    validate_inner(value, true)?;
    if inventory()? != first {
        return Err(fail("native persistence membership or bytes changed during inventory"));
    }
    check.check()?;
    Ok(first)
}

fn validate_shape(basis: &Basis) -> Result<()> {
    if basis.sequence.is_some_and(|id| id == 0 || id > 9_007_199_254_740_991) {
        return Err(fail("invalid nullable native recovery view"));
    }
    if basis.kind != "poema-native-project-recovery-basis"
        || basis.version != 1
        || !crate::project_workspace::valid_scope(&basis.scope)
        || !filmcraft_project::ClipIdentity::valid_id(&basis.native_session)
        || basis.revision == 0
        || basis.saved_revision > basis.revision
        || basis.project_sha256.len() != 64
        || !basis.project_sha256.bytes().all(|v| v.is_ascii_hexdigit())
        || basis.journals.len() > MAX_JOURNALS
        || basis.session_directory.is_empty()
        || basis.session_directory.len() > 128
        || !basis.session_directory.bytes().all(|v| v.is_ascii_digit() || v == b'-')
    {
        return Err(fail("malformed native recovery comparison basis"));
    }
    Ok(())
}
