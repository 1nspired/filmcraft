//! Complete private journal companion on the existing native persistence worker.
//! Capture COW graph identities on the editor; serialize/validate/write off-thread.
//! This is not a startup/admission command and grants no current edit authority.
use super::super::native_editor;
use super::*;
use std::io::Write;
pub(crate) const FILE: &str = "native-state.json";
const LIMIT: usize = 80 * 1024 * 1024;
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Payload {
    pub(super) kind: String,
    pub(super) version: u8,
    pub(super) schema: u32,
    pub(super) source_pid: u32,
    pub(super) data_directory: PathBuf,
    pub(super) session_directory: String,
    pub(super) saved_file: crate::saved_file::Baseline,
    pub(super) project_file_sha256: String,
    pub(super) history_sha256: String,
    pub(super) history: Capsule,
    pub(super) editor: native_editor::Snapshot,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Envelope {
    payload: Payload,
    sha256: String,
}
pub(crate) struct Pending {
    history: PendingCapsule,
    editor: native_editor::Snapshot,
    saved_file: crate::saved_file::Baseline,
    data_directory: PathBuf,
    session_directory: String,
    source_pid: u32,
}
pub(crate) fn capture(s: &mut Session) -> Result<Option<Pending>> {
    let Some(identity) = crate::project_workspace::identity(&s.project)? else { return Ok(None) };
    let Some(saved) = s.saved_file.clone().filter(|b| s.path.as_ref() == Some(&b.path)) else { return Ok(None) };
    let Some(per) = &s.persistence else { return Ok(None) };
    let data = per.data_dir.clone();
    let own = per.session_dir.file_name().and_then(|v| v.to_str()).ok_or_else(|| error("native journal session directory is not UTF-8"))?.to_owned();
    if !data.is_absolute() || per.session_dir != crate::autosave::recovery_root(&data).join(&own) {
        return Err(error("native journal owner directory differs from persistence"));
    }
    let scope = identity.scope.clone();
    epoch(s)?;
    let r = Request {
        token: "native-journal-capture-only".into(),
        scope: CheckpointScope::Project(scope.clone()),
        workspace: scope.workspace_id.clone(),
        checkpoint: scope.workspace_id,
        sha256: None,
        metadata_basis: false,
    };
    Ok(Some(Pending {
        history: pending_capsule(s, &r, s.state.active_sequence)?,
        editor: native_editor::Snapshot::capture(s)?,
        saved_file: saved,
        data_directory: data,
        session_directory: own,
        source_pid: std::process::id(),
    }))
}
fn hash_ok(v: &str) -> bool {
    v.len() == 64 && v.bytes().all(|v| v.is_ascii_digit() || (b'a'..=b'f').contains(&v))
}
pub(super) fn decode(raw: &[u8], project_file: &[u8]) -> Result<Payload> {
    if raw.len() > LIMIT || project_file.len() > MAX_BYTES {
        return Err(error("native journal exceeds its complete byte bound"));
    }
    let e: Envelope = serde_json::from_slice(raw).map_err(|e| error(e.to_string()))?;
    if bytes(&e, LIMIT)? != raw || !hash_ok(&e.sha256) || digest(&bytes(&e.payload, LIMIT)?) != e.sha256 {
        return Err(error("native journal checksum or exact native encoding differs"));
    }
    let p = &e.payload;
    let scope = match &p.history.scope {
        CheckpointScope::Project(scope) => scope,
        _ => return Err(error("native journal requires an independent Root owner")),
    };
    let loaded = filmcraft_format::decode(project_file).map_err(|e| error(e.to_string()))?;
    if p.kind != "poema-native-journal"
        || p.version != 1
        || p.schema != filmcraft_format::SCHEMA_VERSION
        || p.source_pid == 0
        || !p.data_directory.is_absolute()
        || p.session_directory.is_empty()
        || p.session_directory.len() > 128
        || !p.session_directory.bytes().all(|v| v.is_ascii_digit() || v == b'-')
        || p.saved_file.path != p.history.path
        || !Path::new(&p.saved_file.path).is_absolute()
        || !p.saved_file.path.ends_with(".fcproj")
        || !hash_ok(&p.saved_file.sha256)
        || p.project_file_sha256 != digest(project_file)
        || !hash_ok(&p.history_sha256)
        || p.history_sha256 != digest(&bytes(&p.history, MAX_BYTES)?)
        || p.history.revision == 0
        || p.history.revision >= u64::MAX - 1
        || p.history.projects.first().map(|p| p.as_bytes()) != Some(project_bytes(&loaded.project, MAX_BYTES)?.as_slice())
        || crate::project_workspace::identity(&loaded.project)?.is_none_or(|v| v.scope != *scope)
    {
        return Err(error("crossed or incompatible complete native journal"));
    }
    p.editor.validate(p.history.revision, p.history.sequence)?;
    let mut candidate = Session::default();
    crate::commands::install_project(&mut candidate, loaded.project, Some(p.history.path.clone()), true);
    p.editor.install(&mut candidate);
    candidate.revision = p.history.revision;
    candidate.saved_revision = candidate.revision;
    let r = Request {
        token: "native-journal-verification-only".into(),
        scope: p.history.scope.clone(),
        workspace: p.history.workspace.clone(),
        checkpoint: p.history.checkpoint.clone(),
        sha256: None,
        metadata_basis: false,
    };
    // The same complete graph/receipt validator used by native cold snapshots;
    // no editor, operation token, source witness or model authority is exposed.
    prepare_restore(&candidate, &r, &p.history, &p.history_sha256, candidate.state.active_sequence)?;
    Ok(e.payload)
}
fn validate(raw: &[u8], project_file: &[u8]) -> Result<()> {
    decode(raw, project_file).map(|_| ())
}
impl Pending {
    fn encode(&self, project_file: &[u8]) -> Result<Vec<u8>> {
        let history = self.history.encode()?;
        let p = Payload {
            kind: "poema-native-journal".into(),
            version: 1,
            schema: filmcraft_format::SCHEMA_VERSION,
            source_pid: self.source_pid,
            data_directory: self.data_directory.clone(),
            session_directory: self.session_directory.clone(),
            saved_file: self.saved_file.clone(),
            project_file_sha256: digest(project_file),
            history_sha256: digest(&bytes(&history, MAX_BYTES)?),
            history,
            editor: self.editor.clone(),
        };
        let e = Envelope { sha256: digest(&bytes(&p, LIMIT)?), payload: p };
        let raw = bytes(&e, LIMIT)?;
        validate(&raw, project_file)?;
        Ok(raw)
    }
    pub(crate) fn write(&self, directory: &Path, project_file: &[u8]) -> Result<()> {
        if self.source_pid != std::process::id()
            || directory != crate::autosave::recovery_root(&self.data_directory).join(&self.session_directory)
            || !fs::symlink_metadata(directory).map_err(|e| error(e.to_string()))?.is_dir()
            || fs::canonicalize(directory).map_err(|e| error(e.to_string()))? != directory
        {
            return Err(error("native journal write belongs to another or redirected persistence owner"));
        }
        let raw = self.encode(project_file)?;
        let target = directory.join(FILE);
        match fs::symlink_metadata(&target) {
            Ok(meta) => {
                if !meta.is_file() {
                    return Err(error("native journal companion is redirected or nonregular"));
                }
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    if meta.permissions().mode() & 0o077 != 0 {
                        return Err(error("native journal companion is not private"));
                    }
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(error(e.to_string())),
        }
        let mut nonce = [0u8; 16];
        getrandom::fill(&mut nonce).map_err(|e| error(e.to_string()))?;
        let name = nonce.iter().map(|v| format!("{v:02x}")).collect::<String>();
        let temporary = directory.join(format!(".native-state-{name}.tmp"));
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
        }
        let mut file = options.open(&temporary).map_err(|e| error(e.to_string()))?;
        let result = (|| {
            file.write_all(&raw).and_then(|_| file.sync_all()).map_err(|e| error(e.to_string()))?;
            fs::rename(&temporary, &target).map_err(|e| error(e.to_string()))?;
            #[cfg(unix)]
            File::open(directory).and_then(|d| d.sync_all()).map_err(|e| error(e.to_string()))?;
            Ok(())
        })();
        if result.is_err() {
            let _ = fs::remove_file(&temporary);
        }
        result
    }
}
#[cfg(test)]
#[path = "journal_tests.rs"]
mod tests;
