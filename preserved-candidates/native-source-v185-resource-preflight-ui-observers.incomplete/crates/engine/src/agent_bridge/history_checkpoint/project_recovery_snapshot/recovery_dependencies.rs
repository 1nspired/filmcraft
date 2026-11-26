//! Private storage dependency metadata for one immutable staged native cut.
//! Original native decoding and path policy own interpretation. No media proof,
//! read/edit ticket, source binding, project change or executable authority.
use super::*;
use std::collections::BTreeMap;
const COMMAND: &str = "agent.readProjectRecoveryDependencies";
const ARCHIVE: &str = "agent.readArchivedRecoveryDependencies";
const MAX_FILES: usize = 65_536;
const MAX_USES: usize = 262_144;
const MAX_REPLY: usize = 8 * 1024 * 1024;
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Read {
    scope: crate::project_workspace::ProjectScope,
    workspace: String,
    update: String,
    pause_token: String,
    native_session: String,
    stage_sha256: String,
}
fn error(message: impl Into<String>) -> crate::EngineError {
    bad(COMMAND, message)
}
fn path_ok(path: &str) -> bool {
    !path.is_empty() && path.len() <= 4096 && !path.chars().any(char::is_control) && Path::new(path).is_absolute()
}
struct Inventory {
    rows: BTreeMap<String, Value>,
    uses: usize,
}
impl Inventory {
    fn add(&mut self, origin: Option<&Option<PathBuf>>, stored: &str, path: &str, kind: &str, usage: &Value) -> Result<()> {
        if stored.is_empty() || stored.len() > 4096 || stored.chars().any(char::is_control) || !path_ok(path) {
            return Err(error("native dependency has an invalid or unresolved bounded path"));
        }
        let origin = match origin {
            None => json!({"kind":"picked","document":null}),
            Some(document) => {
                let document = document.as_ref().map(|p| p.to_str().ok_or_else(|| error("non-UTF8 document origin"))).transpose()?;
                if document.is_some_and(|p| !path_ok(p)) {
                    return Err(error("native dependency document origin is not a bounded absolute path"));
                }
                json!({"kind":"document","document":document})
            }
        };
        let key = serde_json::to_string(&(kind, stored, path, &origin)).map_err(|e| error(e.to_string()))?;
        if !self.rows.contains_key(&key) {
            if self.rows.len() >= MAX_FILES {
                return Err(error("complete native dependency files exceed their bound; no rows truncated"));
            }
            self.rows.insert(key.clone(), json!({"kind":kind,"storedPath":stored,"path":path,"origin":origin,"uses":[]}));
        }
        let uses = self.rows.get_mut(&key).and_then(|r| r.get_mut("uses")).and_then(Value::as_array_mut).ok_or_else(|| error("dependency index missing"))?;
        if !uses.contains(usage) {
            self.uses = self.uses.checked_add(1).filter(|n| *n <= MAX_USES).ok_or_else(|| error("complete native dependency references exceed their bound"))?;
            uses.push(usage.clone());
        }
        Ok(())
    }
}
fn staged_owner(s: &Session, r: &Read) -> Result<(Snapshot, usize)> {
    let fence = workspace_update::view(s).ok_or_else(|| error("exact staged original native owner required"))?;
    if !workspace_update::paused_project_owner(s, &r.scope)
        || r.workspace != r.scope.workspace_id
        || fence["workspace"] != r.workspace
        || fence["update"] != r.update
        || fence["token"] != r.pause_token
        || fence["nativeSession"] != r.native_session
        || !hash_ok(&r.stage_sha256)
        || fence["stage"]["status"] != "staged"
        || fence["stage"]["sha256"] != r.stage_sha256
    {
        return Err(error("dependency read crossed its actual paused owner, epoch or staged snapshot"));
    }
    // The same original reader used immediately before native retirement owns
    // the snapshot, current editor, saved-file and persistence validation.
    read_validated_staged(s)
}
fn enumerate(s: &Session, projects: &[String], document: Option<&Path>, check: &media_guard::Check) -> Result<Inventory> {
    if projects.is_empty() || projects.len() > MAX_STEPS + 1 {
        return Err(error("complete native project graph exceeds its bound"));
    }
    let mut inventory = Inventory { rows: BTreeMap::new(), uses: 0 };
    let mut visits = 0usize;
    for (graph, raw) in projects.iter().enumerate() {
        check.check()?;
        let project = project(raw)?;
        for item in project.items.values() {
            visits = visits.checked_add(1).filter(|n| *n <= MAX_USES).ok_or_else(|| error("complete native graph scan exceeds its bound"))?;
            check.check()?;
            let Some(media) = item.as_media() else { continue };
            if !(1..=9_007_199_254_740_991).contains(&item.id.0) {
                return Err(error("native dependency item exceeds exact transport"));
            }
            let usage = json!({"project":graph,"item":item.id.0,"offline":media.offline});
            for (kind, reference) in [("original", Some(&media.media)), ("proxy", media.proxy.as_ref())] {
                let Some(filmcraft_project::MediaRef::File { path: stored }) = reference else { continue };
                if stored.is_empty() || stored.len() > 4096 || stored.chars().any(char::is_control) {
                    return Err(error("native stored path exceeds its bounded metadata policy"));
                }
                let origin = match document {
                    Some(path) => Some(Some(path.to_path_buf())),
                    None => s.media.path_origin(stored),
                };
                let resolved = match document {
                    Some(_) => crate::media_pool::follow_origin(stored, origin.as_ref()),
                    None => s.media.follow(stored),
                }
                .map_err(|e| error(e.to_string()))?;
                if !path_ok(&resolved) {
                    return Err(error("native dependency cannot fall back to a working directory"));
                }
                if kind == "original" && media.info.kind == filmcraft_media::MediaKind::ImageSequence {
                    // Exact existing renderer numbering/gap policy; never
                    // invent siblings from a directory or decode footage.
                    let frames = crate::media_pool::image_sequence_frames(&resolved, &*s.services)?;
                    for frame in frames.iter().flatten() {
                        check.check()?;
                        inventory.add(origin.as_ref(), stored, frame, "sequence-frame", &usage)?;
                    }
                } else {
                    inventory.add(origin.as_ref(), stored, &resolved, kind, &usage)?;
                }
            }
        }
    }
    check.check()?;
    Ok(inventory)
}
// Same original enumerator for an already-retained current graph. The caller
// owns the paused image and performs the surrounding native identity checks.
#[cfg(any(target_vendor = "apple", target_os = "linux"))]
pub(super) fn current_dependencies(s: &Session, projects: &[String], check: &media_guard::Check) -> Result<Value> {
    let inventory = enumerate(s, projects, None, check)?;
    let value = json!({"projects":projects.len(),"files":inventory.rows.into_values().collect::<Vec<_>>(),"references":inventory.uses});
    bytes(&value, MAX_REPLY)?;
    Ok(value)
}
fn read(s: &mut Session, params: &Value) -> Result<Value> {
    let r: Read = parse(COMMAND, params)?;
    let (b, size) = staged_owner(s, &r)?;
    let check = media_guard::Check::new(web_time::Instant::now());
    let inventory = enumerate(s, &b.history.projects, None, &check)?;
    let persistence = project_recovery::persistence_dependencies(&b.basis)?;
    let result = json!({"kind":"native_project_recovery_dependencies","version":2,"status":"metadata",
        "scope":r.scope,"workspace":r.workspace,"update":r.update,"nativeSession":b.history.epoch,
        "nativePid":b.source_pid,"revision":b.history.revision,"snapshotSha256":r.stage_sha256,"snapshotBytes":size,
        "savedProject":{"path":b.history.path,"sha256":b.basis["saved_file"]["sha256"]},"projects":b.history.projects.len(),
        "files":inventory.rows.into_values().collect::<Vec<_>>(),"references":inventory.uses,"persistence":persistence,
        "sourceValidation":"not-performed","mediaEvidence":false,"writeAuthority":false,"projectUnchanged":true,"historyUnchanged":true});
    bytes(&result, MAX_REPLY)?;
    Ok(result)
}
pub(super) fn command() -> CommandSpec {
    CommandSpec {
        id: COMMAND,
        label: "Read Staged Native Recovery Dependencies",
        menu: &[],
        shortcut: None,
        params: "{scope:{projectId,workspaceId},workspace,update,pauseToken,nativeSession,stageSha256} — private staged-owner storage metadata only",
        enabled: always,
        run: read,
        journal: false,
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Archived {
    checkpoint: String,
    sha256: String,
    basis: InspectionBasis,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ReadArchive {
    owner: Read,
    checkpoint: Archived,
}
fn read_archive(s: &mut Session, params: &Value) -> Result<Value> {
    let r: ReadArchive = parse(ARCHIVE, params)?;
    let (staged, staged_size) = staged_owner(s, &r.owner)?;
    if params["checkpoint"]["basis"].get("sequence").is_none()
        || !uuid(&r.checkpoint.checkpoint)
        || !hash_ok(&r.checkpoint.sha256)
        || !filmcraft_project::ClipIdentity::valid_id(&r.checkpoint.basis.native_session)
        || r.checkpoint.basis.revision == 0
    {
        return Err(bad(ARCHIVE, "exact immutable checkpoint digest and original nullable save basis required"));
    }
    let request = Request {
        token: r.owner.pause_token.clone(),
        scope: CheckpointScope::Project(r.owner.scope.clone()),
        workspace: r.owner.workspace.clone(),
        checkpoint: r.checkpoint.checkpoint.clone(),
        sha256: Some(r.checkpoint.sha256.clone()),
        metadata_basis: false,
    };
    validate_association(&s.project, &request.scope, &request.workspace)?;
    let sequence = checkpoint_workspace(s, &request.scope, true)?;
    let check = media_guard::Check::new(web_time::Instant::now());
    let (capsule, size, hash) = load_with_digest(&path(s, &request)?, Some(&r.checkpoint.sha256), Some(&check))?;
    if capsule.epoch != r.checkpoint.basis.native_session || capsule.revision != r.checkpoint.basis.revision || capsule.sequence != r.checkpoint.basis.sequence
    {
        return Err(bad(ARCHIVE, "archive differs from its exact original save basis"));
    }
    let realms = validate_archived_provenance(s, &request, &capsule, &hash, sequence, &r.owner.scope)?;
    let inventory = enumerate(s, &capsule.projects, Some(Path::new(&capsule.path)), &check)?;
    check.check()?;
    let result = json!({"kind":"native_archived_recovery_dependencies","version":1,"status":"metadata",
        "scope":r.owner.scope,"workspace":r.owner.workspace,"update":r.owner.update,"nativeSession":staged.history.epoch,"nativePid":staged.source_pid,"revision":staged.history.revision,
        "stageSha256":r.owner.stage_sha256,"stageBytes":staged_size,"projectPath":capsule.path,
        "checkpoint":summary(&capsule,&hash,size),"basis":{"nativeSession":capsule.epoch,"revision":capsule.revision,"sequence":capsule.sequence},
        "owningRealms":realms.into_iter().map(|anchor|json!({"projectId":r.owner.scope.project_id,"anchorSequence":anchor})).collect::<Vec<_>>(),
        "projects":capsule.projects.len(),"files":inventory.rows.into_values().collect::<Vec<_>>(),"references":inventory.uses,
        "historyValidation":"validated_archive_only","sourceValidation":"not-performed","mediaEvidence":false,"writeAuthority":false,"projectUnchanged":true,"historyUnchanged":true});
    bytes(&result, MAX_REPLY)?;
    Ok(result)
}
pub(super) fn archive_command() -> CommandSpec {
    CommandSpec {
        id: ARCHIVE,
        label: "Read Archived Native Recovery Dependencies",
        menu: &[],
        shortcut: None,
        params: "{owner:{scope,workspace,update,pauseToken,nativeSession,stageSha256},checkpoint:{checkpoint,sha256,basis:{nativeSession,revision,sequence}}} — private exact historical storage metadata",
        enabled: always,
        run: read_archive,
        journal: false,
    }
}
