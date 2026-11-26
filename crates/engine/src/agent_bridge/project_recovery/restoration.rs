//! Explicit private current-publisher admission. Opaque snapshots stay unchanged.
//! The trusted startup constructor owns current stopped-data custody; native
//! code independently compares the original baseline and actual physical files.
use super::*;
pub(in crate::agent_bridge) fn restore_basis(value: &Value, description: &Value, snapshot: &str, activation: &str) -> Result<Value> {
    #[cfg(unix)]
    {
        unix::restore(value, description, snapshot, activation)
    }
    #[cfg(not(unix))]
    {
        let _ = (value, description, snapshot, activation);
        Err(fail("explicit native restoration needs supported physical file identities"))
    }
}
#[cfg(unix)]
mod unix {
    use super::*;
    use std::os::unix::fs::MetadataExt;
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase", deny_unknown_fields)]
    struct Restoration {
        kind: String,
        version: u8,
        snapshot_sha256: String,
        activation_token: String,
        files: Vec<File>,
    }
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct File {
        role: String,
        path: String,
        sha256: String,
        bytes: u64,
        mode: u32,
        physical: Physical,
    }
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase", deny_unknown_fields)]
    struct Physical {
        dev: String,
        ino: String,
        size: String,
        mtime_ns: String,
        ctime_ns: String,
        mode: String,
        uid: String,
    }
    fn published(file: &File, observed: Stamp) -> Result<Stamp> {
        let p = &file.physical;
        let ctime = p.ctime_ns.parse::<u128>().map_err(|_| fail("bounded native publication time required"))?;
        let expected = Stamp {
            length: file.bytes,
            modified: p.mtime_ns.clone(),
            device: p.dev.clone(),
            inode: p.ino.clone(),
            changed: format!("{}:{}", ctime / 1_000_000_000, ctime % 1_000_000_000),
        };
        let current = fs::symlink_metadata(&file.path).map_err(|e| fail(e.to_string()))?;
        if !current.is_file()
            || observed != expected
            || stamp(&current)? != observed
            || p.size != file.bytes.to_string()
            || p.ctime_ns != ctime.to_string()
            || file.mode > 0o777
            || current.mode() & 0o777 != file.mode
            || p.mode != current.mode().to_string()
            || p.uid != current.uid().to_string()
        {
            return Err(fail("published native file identity changed; preserve later human work"));
        }
        Ok(observed)
    }
    pub(super) fn restore(value: &Value, description: &Value, snapshot: &str, activation: &str) -> Result<Value> {
        history_checkpoint::bytes(description, 16384)?;
        let r: Restoration = parse("native.projectRecoveryRestoration", description)?;
        if r.kind != "native_project_recovery_restoration"
            || r.version != 1
            || r.snapshot_sha256 != snapshot
            || r.activation_token != activation
            || r.files.is_empty()
            || r.files.len() > 2
        {
            return Err(fail("current private restoration belongs to another exact snapshot or activation"));
        }
        let mut b: Basis = parse("native.projectRecoveryBasis", value)?;
        validate_shape(&b)?;
        let check = media_guard::Check::new(web_time::Instant::now());
        let mut seen = std::collections::BTreeSet::new();
        for file in r.files {
            check.check()?;
            if !seen.insert(file.role.clone())
                || file.path.len() > 4096
                || !Path::new(&file.path).is_absolute()
                || file.path.chars().any(char::is_control)
                || file.bytes == 0
            {
                return Err(fail("exact distinct bounded native restoration roles required"));
            }
            match file.role.as_str() {
                "saved-project" => {
                    if file.path != b.saved_file.path || file.sha256 != b.saved_file.sha256 || file.bytes > MAX_FILE as u64 {
                        return Err(fail("restored saved project differs from the original native baseline"));
                    }
                    b.file_stamp = published(&file, file_matches(&b.saved_file, &check)?)?;
                }
                "preferences" => {
                    let pref = b.preferences_file.as_mut().ok_or_else(|| fail("original persisted preference baseline required"))?;
                    if pref.path != b.data_directory.join("preferences.json")
                        || Path::new(&file.path) != pref.path.as_path()
                        || pref.sha256.as_deref() != Some(file.sha256.as_str())
                        || file.bytes > 2 * 1024 * 1024
                    {
                        return Err(fail("restored preferences differ from the original native baseline"));
                    }
                    let actual = preference_file(&pref.path, &check)?;
                    if actual.sha256 != pref.sha256 {
                        return Err(fail("restored preference bytes differ from the original native baseline"));
                    }
                    pref.stamp = Some(published(&file, actual.stamp.ok_or_else(|| fail("restored preference file missing"))?)?);
                }
                _ => return Err(fail("unknown native restoration role; no authority accepted")),
            }
        }
        // Only explicitly published file stamps differ in this in-memory basis.
        // The caller selects a legitimate newer journal first, then runs the
        // original complete cold validator before install and publication.
        let fresh = serde_json::to_value(b).map_err(|e| fail(e.to_string()))?;
        check.check()?;
        Ok(fresh)
    }
}
