//! Source-bound journal selection; never cleans or writes original recovery data.
use super::*;
const MAX_COMPANION: usize = 80 * 1024 * 1024;
pub(in crate::agent_bridge) struct JournalIdentity {
    pub(in crate::agent_bridge) scope: crate::project_workspace::ProjectScope,
    pub(in crate::agent_bridge) saved_file: Baseline,
    pub(in crate::agent_bridge) source_pid: u32,
    pub(in crate::agent_bridge) native_session: String,
    pub(in crate::agent_bridge) data_directory: PathBuf,
    pub(in crate::agent_bridge) session_directory: String,
    pub(in crate::agent_bridge) revision: u64,
    pub(in crate::agent_bridge) saved_revision: u64,
    pub(in crate::agent_bridge) sequence: Option<u64>,
    pub(in crate::agent_bridge) project_sha256: String,
}
pub(in crate::agent_bridge) struct JournalCandidate {
    pub(in crate::agent_bridge) raw: Vec<u8>,
    pub(in crate::agent_bridge) project: Vec<u8>,
    basis: Basis,
    expected_pid: u32,
    journal: Journal,
    current: Vec<Journal>,
    companion: JournalCompanion,
}
fn private_companion(path: &Path, check: &media_guard::Check) -> Result<(Vec<u8>, Stamp)> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if fs::symlink_metadata(path).map_err(|e| fail(e.to_string()))?.permissions().mode() & 0o077 != 0 {
            return Err(fail("complete native journal is not private"));
        }
    }
    regular(path, MAX_COMPANION, check)
}
pub(super) fn validate_companion(b: &Basis, check: &media_guard::Check) -> Result<()> {
    let Some(c) = &b.journal_companion else { return Ok(()) };
    if c.path != crate::autosave::recovery_root(&b.data_directory).join(&b.session_directory).join(crate::agent_bridge::NATIVE_JOURNAL_FILE)
        || c.sha256.len() != 64
        || !c.sha256.bytes().all(|v| v.is_ascii_hexdigit())
    {
        return Err(fail("native journal witness belongs to another source"));
    }
    let (raw, stamp) = private_companion(&c.path, check)?;
    if stamp != c.stamp || sha256(&raw) != c.sha256 {
        return Err(fail("accepted native journal changed; candidate remains paused"));
    }
    Ok(())
}
pub(in crate::agent_bridge) fn read_newer_journal(value: &Value, expected_pid: u32) -> Result<Option<JournalCandidate>> {
    history_checkpoint::bytes(value, 512 * 1024)?;
    let basis: Basis = parse("native.projectRecoveryBasis", value)?;
    validate_shape(&basis)?;
    if expected_pid == 0 {
        return Err(fail("original native source PID required"));
    }
    let check = media_guard::Check::new(web_time::Instant::now());
    validate_companion(&basis, &check)?;
    if file_matches(&basis.saved_file, &check)? != basis.file_stamp {
        return Err(fail("original saved-file identity changed; no newer journal recovered"));
    }
    if let Some(pref) = &basis.preferences_file
        && (pref.path != basis.data_directory.join("preferences.json") || preference_file(&pref.path, &check)? != *pref)
    {
        return Err(fail("later preference state needs explicit reconciliation; no newer journal recovered"));
    }
    let now = scan(&basis.data_directory, &basis.session_directory, false, &check)?;
    let mut old = basis.journals.iter().map(|j| (j.directory.as_str(), j)).collect::<BTreeMap<_, _>>();
    if old.len() != basis.journals.len() {
        return Err(fail("duplicate original journal identities"));
    }
    let mut latest = None;
    for j in &now {
        let before = old.remove(j.directory.as_str());
        if j.directory == basis.session_directory {
            if j.revision > basis.revision && j.revision < u64::MAX - 1 && j.project_path.as_deref() == Some(basis.saved_file.path.as_str()) {
                latest = Some(j.clone());
            }
        } else if before != Some(j) {
            return Err(fail("another or unknown native journal changed; nothing recovered"));
        }
    }
    if old.keys().any(|id| *id != basis.session_directory) {
        return Err(fail("another retained journal disappeared; nothing recovered"));
    }
    let Some(journal) = latest else {
        validate(value)?;
        return Ok(None);
    };
    let directory = crate::autosave::recovery_root(&basis.data_directory).join(&basis.session_directory);
    let (project, _) = regular(&directory.join("snapshot.fcproj"), MAX_FILE, &check)?;
    let (metadata, _) = regular(&directory.join("session.json"), MAX_META, &check)?;
    let record: crate::autosave::JournalMeta = serde_json::from_slice(&metadata).map_err(|e| fail(e.to_string()))?;
    if sha256(&project) != journal.snapshot_sha256
        || sha256(&metadata) != journal.metadata_sha256
        || record.pid != expected_pid
        || record.revision != journal.revision
        || record.project_path != journal.project_path
    {
        return Err(fail("newer native journal metadata does not match the original source"));
    }
    let path = directory.join(crate::agent_bridge::NATIVE_JOURNAL_FILE);
    let (raw, stamp) = private_companion(&path, &check)?;
    let companion = JournalCompanion { path, sha256: sha256(&raw), stamp };
    if file_matches(&basis.saved_file, &check)? != basis.file_stamp || scan(&basis.data_directory, &basis.session_directory, false, &check)? != now {
        return Err(fail("native source changed while reading newer journal"));
    }
    let result = JournalCandidate { raw, project, basis, expected_pid, journal, current: now, companion };
    validate_companion(&Basis { journal_companion: Some(result.companion.clone()), ..result.basis.clone() }, &check)?;
    Ok(Some(result))
}
impl JournalCandidate {
    pub(in crate::agent_bridge) fn finish(self, id: JournalIdentity) -> Result<Value> {
        if id.scope != self.basis.scope
            || id.saved_file != self.basis.saved_file
            || id.source_pid != self.expected_pid
            || id.native_session != self.basis.native_session
            || id.data_directory != self.basis.data_directory
            || id.session_directory != self.basis.session_directory
            || id.revision != self.journal.revision
            || id.saved_revision > id.revision
            || id.project_sha256 != self.journal.project_sha256
        {
            return Err(fail("complete journal belongs to another original source, scope, baseline or graph"));
        }
        let basis = Basis {
            revision: id.revision,
            saved_revision: id.saved_revision,
            sequence: id.sequence,
            project_sha256: id.project_sha256,
            journals: self.current,
            journal_companion: Some(self.companion),
            ..self.basis
        };
        let value = serde_json::to_value(basis).map_err(|e| fail(e.to_string()))?;
        validate(&value)?;
        Ok(value)
    }
}
