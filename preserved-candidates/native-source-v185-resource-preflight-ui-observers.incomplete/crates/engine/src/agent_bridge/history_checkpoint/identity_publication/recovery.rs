//! Trusted cold startup only. No RPC, edit recipe or live-session restoration.
use super::*;
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Startup {
    project_path: String,
    scope: sequences::Scope,
    workspace: String,
    migration: String,
    expected: Expected,
    sha256: String,
    activation_token: String,
}
pub(super) fn open(s: &mut Session, p: &Value) -> Result<Value> {
    let r: Startup = parse("identity recovery startup", p)?;
    super::super::super::native_editor::require_fresh(s)?;
    if !hash_ok(&r.sha256) || !token_ok(&r.activation_token) {
        return Err(fail("exact immutable decision and fresh activation token required"));
    }
    let check = media_guard::Check::new(web_time::Instant::now());
    let mut candidate = Session::new(s.services.clone());
    candidate.path = Some(r.project_path.clone());
    let request = ReadPublication {
        token: r.activation_token.clone(),
        scope: CheckpointScope::Sequence(r.scope.clone()),
        workspace: r.workspace.clone(),
        migration: r.migration.clone(),
        expected: r.expected,
    };
    let (mut b, hash, size) = decision(&candidate, &request, &check)?;
    if hash != r.sha256 || r.activation_token == b.token {
        return Err(fail("exact immutable decision hash and a fresh activation token required"));
    }
    if workspace_update::project_file_hash(Path::new(&r.project_path))? != b.saved_file_sha256 {
        return Err(fail("saved project changed after identity decision; nothing recovered"));
    }
    let revision = b.source.revision.checked_add(1).ok_or_else(|| fail("native revision exhausted"))?;
    let detail = descriptor(&b, &hash, size);
    let project = workspace_project(&b.candidate)?;
    candidate.prefs = b.preferences.clone();
    candidate.media.set_use_proxies(candidate.prefs.media.enable_proxies);
    crate::commands::install_project(&mut candidate, project, Some(r.project_path.clone()), true);
    candidate.saved_file = Some(crate::saved_file::Baseline { path: r.project_path.clone(), sha256: b.saved_file_sha256.clone() });
    candidate.state = b.editor.clone();
    candidate.state.clipboard = std::mem::take(&mut b.clipboard);
    candidate.state.clipboard_markers = std::mem::take(&mut b.clipboard_markers);
    candidate.revision = revision;
    candidate.saved_revision = revision;
    // Rebase the capsule's metadata revision only. Original historical receipts
    // stay byte-identical and retain their original epoch and exact graph links.
    b.candidate.revision = revision;
    let capsule_hash = digest(&bytes(&b.candidate, MAX_BYTES)?);
    let hydrate = Request {
        token: r.activation_token.clone(),
        scope: b.candidate.scope.clone(),
        workspace: r.workspace.clone(),
        checkpoint: r.migration.clone(),
        sha256: None,
        metadata_basis: false,
    };
    let prepared = prepare_restore(&candidate, &hydrate, &b.candidate, &capsule_hash, candidate.state.active_sequence)?;
    candidate.history = prepared.history;
    candidate.agent_bridge.application_history = prepared.application;
    candidate.agent_bridge.application_ui = prepared.application_ui;
    candidate.history.merge_key = b.merge_key;
    candidate.agent_bridge.tickets = prepared.tickets;
    candidate.agent_bridge.head = prepared.owned_head.then(|| (revision, Arc::downgrade(&candidate.project)));
    candidate.saved_revision =
        if b.source.projects.first() == b.candidate.projects.first() && b.saved_revision == b.source.revision { revision } else { b.saved_revision };
    candidate.loaded_schema = b.loaded_schema;
    retain_history_media(&candidate);
    remember_workspace(&mut candidate, &r.workspace, &r.migration, &capsule_hash, &b.source.epoch)?;
    candidate.agent_bridge.history_checkpoints.back_mut().ok_or_else(|| fail("recovered native graph guard missing"))?.identity_publication =
        Some(hash.clone());
    if let Some(aux) = &b.auxiliary {
        aux.install(&mut candidate)?;
    }
    if let Some(journal) = &b.command_journal { journal.install_cold(&mut candidate)?; }
    let fence = workspace_update::hold_candidate(&mut candidate, r.scope, r.workspace, r.migration, r.activation_token, b.saved_file_sha256.clone())?;
    // The disk baseline is checked again after complete preparation; no original
    // file, immutable decision or history companion is written by recovery.
    if workspace_update::project_file_hash(Path::new(&r.project_path))? != b.saved_file_sha256 {
        return Err(fail("saved project changed before recovery publication"));
    }
    check.check()?;
    let result = json!({"status":"identity_recovery_candidate","kind":"native_identity_recovery","version":1,"publication":detail,"fence":fence,"editingPaused":true,"savedFileUnchanged":true,"sourceValidation":"not-performed","mediaEvidence":false,"writeAuthority":false,"execution":"snapshot-restored-paused"});
    *s = candidate;
    Ok(result)
}
pub(super) fn matches_recovered(s: &Session, b: &Commit, r: &ReadPublication, record_hash: &str) -> Result<bool> {
    if s.agent_bridge.epoch.as_deref() == Some(b.source.epoch.as_str()) || s.revision != b.source.revision.saturating_add(1) || !editor_matches(s, b)? {
        return Ok(false);
    }
    let request = Request {
        token: r.token.clone(),
        scope: b.candidate.scope.clone(),
        workspace: r.workspace.clone(),
        checkpoint: r.migration.clone(),
        sha256: None,
        metadata_basis: true,
    };
    let Ok(mut current) = capsule(s, &request, s.state.active_sequence) else { return Ok(false) };
    // A native-only origin guard binds the real COW pool and merge state. Equal
    // saved JSON or borrowed receipt metadata cannot certify a recovered graph.
    let Ok((pool, _, _)) = graph(s) else { return Ok(false) };
    current.epoch = b.source.epoch.clone();
    let rebased_hash = digest(&bytes(&current, MAX_BYTES)?);
    if !s.agent_bridge.history_checkpoints.iter().any(|g| {
        g.workspace == r.workspace
            && g.checkpoint == r.migration
            && g.identity_publication.as_deref() == Some(record_hash)
            && g.sha256 == rebased_hash
            && g.origin_epoch.as_deref() == Some(b.source.epoch.as_str())
            && g.merge_key == s.history.merge_key
            && g.projects.len() == pool.len()
            && g.projects.iter().zip(&pool).all(|(a, b)| a.ptr_eq(&Arc::downgrade(b)))
    }) {
        return Ok(false);
    }
    current.revision = b.source.revision;
    Ok(bytes(&current, MAX_BYTES)? == bytes(&b.candidate, MAX_BYTES)?)
}

pub(super) fn has_origin(s: &Session, r: &ReadPublication, hash: &str) -> bool {
    s.agent_bridge
        .history_checkpoints
        .iter()
        .any(|g| g.workspace == r.workspace && g.checkpoint == r.migration && g.identity_publication.as_deref() == Some(hash))
}

// Confirm a companion saved after publication when its reply was lost before a
// cold restart. No companion-specific guard is minted and nothing is installed.
// By-value equality alone cannot adopt a human replacement of any COW node.
pub(super) fn inspect_checkpoint(s: &Session, r: &Request, c: &Capsule, hash: &str, sequence: Option<ItemId>) -> Result<bool> {
    if !matches!(r.scope, CheckpointScope::Project(_)) || s.agent_bridge.epoch.as_deref() == Some(c.epoch.as_str()) || s.revision != c.revision {
        return Ok(false);
    }
    let (pool, _, _) = graph(s)?;
    let mut current = capsule(s, r, sequence)?;
    current.epoch = c.epoch.clone();
    if bytes(&current, MAX_BYTES)? != bytes(c, MAX_BYTES)? {
        return Ok(false);
    }
    let mut origin = false;
    for g in &s.agent_bridge.history_checkpoints {
        if g.workspace != r.workspace
            || g.origin_epoch.as_deref() != Some(c.epoch.as_str())
            || !g.identity_publication.as_deref().is_some_and(hash_ok)
            || g.merge_key != s.history.merge_key
            || g.projects.len() != pool.len()
            || !g.projects.iter().zip(&pool).all(|(a, b)| a.ptr_eq(&Arc::downgrade(b)))
        {
            continue;
        }
        current.checkpoint = g.checkpoint.clone();
        if digest(&bytes(&current, MAX_BYTES)?) == g.sha256 {
            origin = true;
            break;
        }
    }
    if !origin {
        return Ok(false);
    }
    // Run the complete graph, recipe-shape, collateral and receipt validator
    // against a virtual clean session. Its prepared result is discarded; the
    // real native session retains exactly the same graph, tickets and guards.
    let mut validation = Session::new(s.services.clone());
    validation.project = s.project.clone();
    validation.path = s.path.clone();
    validation.state.active_sequence = sequence;
    validation.revision = s.revision;
    validation.saved_revision = s.revision;
    prepare_restore(&validation, r, c, hash, sequence)?;
    Ok(true)
}
