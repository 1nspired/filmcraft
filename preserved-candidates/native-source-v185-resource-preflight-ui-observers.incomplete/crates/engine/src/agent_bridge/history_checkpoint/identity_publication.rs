//! Durable native identity transaction; never a model tool or file replacement.
use super::*;
mod recovery;
const PUBLISH: &str = "agent.publishIdentityMigration";
const READ: &str = "agent.readIdentityPublication";
const KIND: &str = "poema-native-identity-commit";
const LIMIT: usize = 80 * 1024 * 1024;
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Expected {
    source_graph_sha256: String,
    candidate_graph_sha256: String,
    saved_file_sha256: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Publish {
    token: String,
    scope: sequences::Scope,
    workspace: String,
    migration: String,
    expected: Expected,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ReadPublication {
    token: String,
    scope: CheckpointScope,
    workspace: String,
    migration: String,
    expected: Expected,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Commit {
    kind: String,
    version: u8,
    schema: u32,
    workspace: String,
    migration: String,
    token: String,
    expected: Expected,
    source: Capsule,
    candidate: Capsule,
    editor: crate::EditorState,
    clipboard: Vec<(filmcraft_project::TrackKind, usize, filmcraft_project::TrackItem)>,
    clipboard_markers: Vec<filmcraft_project::Marker>,
    preferences: crate::autosave::Preferences,
    merge_key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    auxiliary: Option<super::super::native_auxiliary::Snapshot>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    command_journal: Option<super::super::native_editor::CommandJournal>,
    saved_revision: u64,
    loaded_schema: u32,
    saved_file_sha256: String,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Envelope {
    payload: Commit,
    sha256: String,
}
fn fail(message: impl Into<String>) -> crate::EngineError {
    bad(PUBLISH, message)
}
fn hash_ok(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|v| v.is_ascii_digit() || (b'a'..=b'f').contains(&v))
}
fn address(s: &Session, workspace: &str, migration: &str, expected: &Expected) -> Result<PathBuf> {
    if !crate::editorial_workspace::uuid(workspace)
        || !crate::editorial_workspace::uuid(migration)
        || !hash_ok(&expected.source_graph_sha256)
        || !hash_ok(&expected.candidate_graph_sha256)
        || !hash_ok(&expected.saved_file_sha256)
    {
        return Err(fail("exact workspace/migration UUIDs and both inspected graph hashes required"));
    }
    Ok(workspace_directory(s)?.join(format!(".poema-identity-{workspace}-{migration}.json")))
}
pub(super) fn publish_record(path: &Path, raw: &[u8], token: &str) -> Result<()> {
    #[cfg(any(target_vendor = "apple", target_os = "linux"))]
    {
        // The original private snapshot/identity publisher shares the same
        // exclusive, descriptor-anchored physical writer as pending Save frames.
        // Existing caller policy is preserved; this wrapper creates no native grant.
        let parent = path.parent().ok_or_else(|| fail("identity parent missing"))?;
        let name = path.file_name().and_then(|n| n.to_str()).ok_or_else(|| fail("invalid identity record name"))?;
        let directory = filmcraft_format::retained::preview_context::DirectoryFrame::capture(parent, &|| Ok(())).map_err(|e| fail(e.to_string()))?;
        let frame = directory.node(name, &|| Ok(())).map_err(|e| fail(e.to_string()))?;
        let mut record = filmcraft_format::retained::private_record::Record::prepare(&frame, token, raw, 0, &|| Ok(())).map_err(|e| fail(e.to_string()))?;
        record
            .publish(raw, &|| Ok(()), &|| Ok(()))
            .map_err(|e| fail(format!("private identity/snapshot publication incomplete: {e}; inspect without replay")))?;
        Ok(())
    }
    #[cfg(not(any(target_vendor = "apple", target_os = "linux")))]
    {
        let candidate = path.with_file_name(format!(
            "{}.{}.candidate",
            path.file_name().and_then(|v| v.to_str()).ok_or_else(|| fail("invalid identity record name"))?,
            token
        ));
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&candidate).map_err(|e| fail(format!("exclusive identity candidate: {e}; nothing overwritten")))?;
        file.write_all(raw).and_then(|_| file.sync_all()).map_err(|e| fail(format!("identity candidate incomplete: {e}; inspect retained evidence")))?;
        // Only a complete fsynced record becomes the durable commit decision. A
        // partial candidate remains inert; an existing decision is never replaced.
        fs::hard_link(&candidate, path).map_err(|e| fail(format!("exclusive identity decision: {e}; inspect, never replay publication")))?;
        File::open(path.parent().ok_or_else(|| fail("identity parent missing"))?)
            .and_then(|v| v.sync_all())
            .map_err(|e| fail(format!("identity decision durability unconfirmed: {e}; inspect before recovery")))?;
        Ok(())
    }
}
fn descriptor(b: &Commit, hash: &str, size: usize) -> Value {
    json!({"workspace":b.workspace,"migration":b.migration,"sourceNativeSession":b.source.epoch,"publicationToken":b.token,
 "beforeRevision":b.source.revision,"revision":b.source.revision.saturating_add(1),"sequence":b.source.sequence,"scope":b.source.scope,"projectScope":b.candidate.scope,"projectPath":b.source.path,
 "sha256":hash,"bytes":size,"sourceGraphSha256":b.expected.source_graph_sha256,"candidateGraphSha256":b.expected.candidate_graph_sha256,
 "sourceProjectSha256":digest(b.source.projects.first().map_or(&[],|v|v.as_bytes())),"candidateProjectSha256":digest(b.candidate.projects.first().map_or(&[],|v|v.as_bytes())),
 "snapshots":b.source.projects.len(),"undoSteps":b.source.undo.len(),"redoSteps":b.source.redo.len(),"retainedTickets":b.source.tickets.len(),"ownedHead":b.source.owned_head,"savedFileSha256":b.saved_file_sha256})
}
fn publish(s: &mut Session, p: &Value) -> Result<Value> {
    let input: Publish = parse(PUBLISH, p)?;
    if !s.worker_available || !operation_cancellation_available(s) {
        return Err(fail("exclusive worker and independent Stop required"));
    }
    workspace_update::require_idle_writer_jobs(s)?;
    if s.persistence.is_some() {
        return Err(fail("identity publication requires a candidate without active persistence; keep the original owner paused"));
    }
    let mut next_hold = workspace_update::identity_candidate_hold(s, &input.scope, &input.workspace, &input.migration)?;
    let path = address(s, &input.workspace, &input.migration, &input.expected)?;
    if fs::symlink_metadata(&path).is_ok() {
        return Err(fail("identity decision already exists; read its publication receipt, never replay"));
    }
    let source_request = Request {
        token: input.token.clone(),
        scope: CheckpointScope::Sequence(input.scope.clone()),
        workspace: input.workspace.clone(),
        checkpoint: input.workspace.clone(),
        sha256: None,
        metadata_basis: true,
    };
    let sequence = scope(s, &source_request, false)?;
    let check = s.agent_bridge.operations.check().ok_or_else(|| fail("current one-attempt metadata operation required"))?;
    let inspection = IdentityInspection { token: input.token.clone(), scope: input.scope.clone(), workspace: input.workspace.clone() };
    let mut prepared = identity_candidate(s, &source_request, &inspection, sequence, &check)?;
    let projected = bytes(&prepared.candidate, MAX_BYTES)?;
    if digest(&prepared.source_bytes) != input.expected.source_graph_sha256 || digest(&projected) != input.expected.candidate_graph_sha256 {
        return Err(fail("native identity graph changed since inspection"));
    }
    let revision = s.revision.checked_add(1).ok_or_else(|| fail("native revision exhausted"))?;
    let saved_file_sha256 = workspace_update::project_file_hash(Path::new(s.path.as_deref().ok_or_else(|| fail("saved path required"))?))?;
    if saved_file_sha256 != input.expected.saved_file_sha256 {
        return Err(fail("saved project changed since identity preparation"));
    }
    prepared.original.checkpoint = input.migration.clone();
    prepared.candidate.checkpoint = input.migration.clone();
    if s.history.merge_key.as_ref().is_some_and(|v| v.len() > 2048) {
        return Err(fail("identity history merge key exceeds its bound"));
    }
    let editor = super::super::native_editor::Snapshot::capture_complete(s)?;
    let commit = Commit {
        kind: KIND.into(),
        version: 1,
        schema: filmcraft_format::SCHEMA_VERSION,
        workspace: input.workspace.clone(),
        migration: input.migration.clone(),
        token: input.token.clone(),
        expected: input.expected.clone(),
        source: prepared.original,
        candidate: prepared.candidate,
        editor: editor.editor,
        clipboard: editor.clipboard,
        clipboard_markers: editor.clipboard_markers,
        preferences: editor.preferences,
        merge_key: editor.merge_key,
        auxiliary: editor.auxiliary,
        command_journal: editor.command_journal,
        saved_revision: editor.saved_revision,
        loaded_schema: editor.loaded_schema,
        saved_file_sha256,
    };
    let payload = bytes(&commit, LIMIT)?;
    let envelope = Envelope { payload: commit, sha256: digest(&payload) };
    let raw = bytes(&envelope, LIMIT)?;
    let decoded: Envelope = serde_json::from_slice(&raw).map_err(|e| fail(e.to_string()))?;
    if bytes(&decoded, LIMIT)? != raw || digest(&bytes(&decoded.payload, LIMIT)?) != decoded.sha256 {
        return Err(fail("complete identity commit would not round-trip"));
    }
    let commit = envelope.payload;
    let unchanged_current = commit.source.projects.first() == commit.candidate.projects.first();
    let saved_revision = if unchanged_current && s.saved_revision == s.revision { revision } else { s.saved_revision };
    if let Some(held) = next_hold.as_mut() {
        // Capture the next lifecycle guard from the same candidate Arc/history
        // before the durable decision; no fallible guard repair follows it.
        let mut projection = Session::new(s.services.clone());
        projection.project = prepared.current.clone();
        projection.path = s.path.clone();
        projection.state = s.state.clone();
        projection.revision = revision;
        projection.saved_revision = saved_revision;
        projection.history.undo = prepared.history.history.undo.clone();
        projection.history.redo = prepared.history.history.redo.clone();
        projection.history.limit = prepared.history.history.limit;
        projection.history.merge_key = commit.merge_key.clone();
        // Rotate activation before publishing: an old queued legacy activation
        // or Cancel must never unlock the newly independent candidate.
        let mut nonce = [0u8; 32];
        getrandom::fill(&mut nonce).map_err(|e| fail(e.to_string()))?;
        let activation_token = nonce.iter().map(|v| format!("{v:02x}")).collect::<String>();
        s.agent_bridge.operations.register(&activation_token)?;
        let basis = capture_basis(&projection, &activation_token, &source_request.scope)?;
        held.promoted_identity(basis, revision, projection.is_dirty(), activation_token);
    }
    validate_basis(s, &input.token, &source_request.scope)?;
    check.check()?;
    let hash = digest(&raw);
    let detail = descriptor(&commit, &hash, raw.len());
    let result = json!({"status":"committed","kind":"native_identity_publication","version":1,"token":input.token,"beforeRevision":s.revision,"revision":revision,"nativeSession":commit.source.epoch,"sequence":sequence.map(|v|v.0),"publication":detail,"savedFileUnchanged":true,"viewUnchanged":true,"sourceValidation":"not-performed","mediaEvidence":false,"writeAuthority":false,"execution":"snapshot-published"});
    if workspace_update::project_file_hash(Path::new(s.path.as_deref().ok_or_else(|| fail("saved path missing"))?))? != commit.saved_file_sha256 {
        return Err(fail("saved project changed before identity commit"));
    }
    check.check()?;
    enter_publication(s, &input.token)?;
    publish_record(&path, &raw, &input.token)?;
    // After the durable decision there is no fallible preparation or recipe.
    // A crash before these swaps is reconciled from that same immutable graph.
    s.project = prepared.current;
    s.history = prepared.history.history;
    s.agent_bridge.application_history = prepared.history.application;
    s.agent_bridge.application_ui = prepared.history.application_ui;
    s.history.merge_key = commit.merge_key;
    s.agent_bridge.tickets = prepared.history.tickets;
    s.agent_bridge.head = prepared.history.owned_head.then(|| (revision, Arc::downgrade(&s.project)));
    s.agent_bridge.media = None;
    s.agent_bridge.history_bases.clear();
    s.agent_bridge.history_checkpoints.clear();
    s.revision = revision;
    s.saved_revision = saved_revision;
    s.agent_bridge.workspace_update = next_hold;
    retain_history_media(s);
    s.events.push(crate::Event::ProjectChanged { revision });
    Ok(result)
}
fn validate_commit(b: &Commit, request: &ReadPublication, check: &media_guard::Check) -> Result<()> {
    if b.kind != KIND
        || b.version != 1
        || b.schema != filmcraft_format::SCHEMA_VERSION
        || b.workspace != request.workspace
        || b.migration != request.migration
        || b.expected.source_graph_sha256 != request.expected.source_graph_sha256
        || b.expected.candidate_graph_sha256 != request.expected.candidate_graph_sha256
        || b.expected.saved_file_sha256 != request.expected.saved_file_sha256
        || b.saved_file_sha256 != b.expected.saved_file_sha256
        || !token_ok(&b.token)
        || !hash_ok(&b.saved_file_sha256)
        || b.source.checkpoint != b.migration
        || b.candidate.checkpoint != b.migration
        || b.source.workspace != b.workspace
        || b.candidate.workspace != b.workspace
        || b.source.revision == u64::MAX
        || b.saved_revision > b.source.revision
        || b.loaded_schema > filmcraft_format::SCHEMA_VERSION
        || b.source.path != b.candidate.path
        || b.merge_key.as_ref().is_some_and(|v| v.len() > 2048)
        || b.editor.active_sequence.map(|v| v.0) != b.source.sequence
    {
        return Err(fail("foreign or malformed identity decision"));
    }
    let CheckpointScope::Sequence(source_scope) = &b.source.scope else {
        return Err(fail("original legacy graph scope required"));
    };
    let CheckpointScope::Project(target) = &b.candidate.scope else {
        return Err(fail("independent candidate scope required"));
    };
    if target.project_id != source_scope.project_id || target.workspace_id != b.workspace {
        return Err(fail("crossed identity graph scopes"));
    }
    match &request.scope {
        CheckpointScope::Sequence(value) if value == source_scope => {}
        CheckpointScope::Project(value) if value == target => {}
        _ => return Err(fail("identity receipt belongs to another native owner")),
    }
    let current = b.source.projects.first().ok_or_else(|| fail("source graph has no current node"))?;
    let mut source = Session::default();
    source.project = Arc::new(project(current)?);
    source.path = Some(b.source.path.clone());
    source.state = b.editor.clone();
    source.saved_revision = source.revision;
    let r = Request {
        token: request.token.clone(),
        scope: b.source.scope.clone(),
        workspace: b.workspace.clone(),
        checkpoint: b.migration.clone(),
        sha256: None,
        metadata_basis: false,
    };
    let prepared = prepare_restore(&source, &r, &b.source, "", source.state.active_sequence)?;
    source.history = prepared.history;
    source.agent_bridge.application_history = prepared.application;
    source.agent_bridge.application_ui = prepared.application_ui;
    source.agent_bridge.tickets = prepared.tickets;
    source.revision = b.source.revision;
    source.saved_revision = b.saved_revision;
    source.agent_bridge.epoch = Some(b.source.epoch.clone());
    source.agent_bridge.head = prepared.owned_head.then(|| (source.revision, Arc::downgrade(&source.project)));
    let base = Request { checkpoint: b.workspace.clone(), ..r };
    let input = IdentityInspection { token: request.token.clone(), scope: source_scope.clone(), workspace: b.workspace.clone() };
    let mut converted = identity_candidate(&source, &base, &input, source.state.active_sequence, check)?;
    if digest(&converted.source_bytes) != b.expected.source_graph_sha256
        || digest(&bytes(&converted.candidate, MAX_BYTES)?) != b.expected.candidate_graph_sha256
    {
        return Err(fail("identity source/candidate differs from original inspection"));
    }
    converted.candidate.checkpoint = b.migration.clone();
    if bytes(&converted.candidate, MAX_BYTES)? != bytes(&b.candidate, MAX_BYTES)? {
        return Err(fail("identity candidate differs from shared native conversion"));
    }
    super::super::native_editor::preferences(&b.preferences)?;
    if let Some(journal) = &b.command_journal { journal.validate()?; }
    if let Some(aux) = &b.auxiliary {
        aux.validate()?;
    }
    Ok(())
}
fn read(s: &mut Session, p: &Value) -> Result<Value> {
    let input: ReadPublication = parse(READ, p)?;
    if !s.worker_available || !operation_cancellation_available(s) {
        return Err(fail("exclusive worker and independent Stop required"));
    }
    validate_basis(s, &input.token, &input.scope)?;
    let check = s.agent_bridge.operations.check().ok_or_else(|| fail("current metadata operation required"))?;
    let (bundle, hash, size) = decision(s, &input, &check)?;
    let mut state = "recorded";
    if s.agent_bridge.epoch.as_deref() == Some(bundle.source.epoch.as_str()) {
        state = "changed";
        let scope = bundle.candidate.scope.clone();
        let r = Request {
            token: input.token.clone(),
            scope,
            workspace: input.workspace.clone(),
            checkpoint: input.migration.clone(),
            sha256: None,
            metadata_basis: true,
        };
        if s.revision == bundle.source.revision.saturating_add(1) && editor_matches(s, &bundle)? {
            if let Ok(mut current) = capsule(s, &r, s.state.active_sequence) {
                current.revision = bundle.source.revision;
                if bytes(&current, MAX_BYTES)? == bytes(&bundle.candidate, MAX_BYTES)? {
                    state = "applied";
                }
            }
        } else if s.revision == bundle.source.revision && editor_matches(s, &bundle)? {
            let r = Request { scope: bundle.source.scope.clone(), ..r };
            if let Ok(current) = capsule(s, &r, s.state.active_sequence)
                && bytes(&current, MAX_BYTES)? == bytes(&bundle.source, MAX_BYTES)?
            {
                state = "recorded";
            }
        }
    }
    if state == "recorded" && recovery::has_origin(s, &input, &hash) {
        state = if recovery::matches_recovered(s, &bundle, &input, &hash)? { "recovered" } else { "changed" };
    }
    validate_basis(s, &input.token, &input.scope)?;
    check.check()?;
    Ok(
        json!({"status":"read","kind":"native_identity_publication_record","version":1,"token":input.token,"nativeSession":epoch(s)?,"revision":s.revision,"sequence":s.state.active_sequence.map(|v|v.0),"scope":input.scope,"publication":descriptor(&bundle,&hash,size),"liveState":state,"projectUnchanged":true,"historyUnchanged":true,"sourceValidation":"not-performed","mediaEvidence":false,"writeAuthority":false,"execution":"not-applied"}),
    )
}
pub(super) fn commands() -> Vec<CommandSpec> {
    vec![
        CommandSpec {
            id: PUBLISH,
            label: "Publish Native Identity Migration",
            menu: &[],
            shortcut: None,
            params: r#"{token:privateBasis,scope,workspace:uuid,migration:uuid,expected:{sourceGraphSha256,candidateGraphSha256,savedFileSha256}}"#,
            enabled: always,
            run: publish,
            journal: false,
        },
        CommandSpec {
            id: READ,
            label: "Read Native Identity Publication",
            menu: &[],
            shortcut: None,
            params: r#"{token:privateBasis,scope,workspace:uuid,migration:uuid,expected:{sourceGraphSha256,candidateGraphSha256,savedFileSha256}}"#,
            enabled: always,
            run: read,
            journal: false,
        },
    ]
}

fn decision(s: &Session, input: &ReadPublication, check: &media_guard::Check) -> Result<(Commit, String, usize)> {
    let path = address(s, &input.workspace, &input.migration, &input.expected)?;
    let raw = private_native_bytes(&path, LIMIT, Some(check))?;
    let envelope: Envelope = serde_json::from_slice(&raw).map_err(|e| fail(e.to_string()))?;
    if bytes(&envelope, LIMIT)? != raw || digest(&bytes(&envelope.payload, LIMIT)?) != envelope.sha256 {
        return Err(fail("identity commit checksum or canonical data changed"));
    }
    let bundle = envelope.payload;
    if bundle.source.path != s.path.as_deref().unwrap_or("") {
        return Err(fail("identity decision belongs to another saved project"));
    }
    validate_commit(&bundle, input, check)?;
    Ok((bundle, digest(&raw), raw.len()))
}
fn editor_matches(s: &Session, b: &Commit) -> Result<bool> {
    Ok(context(s)? == serde_json::to_value(&b.editor).map_err(|e| fail(e.to_string()))?
        && bytes(&s.state.clipboard, LIMIT)? == bytes(&b.clipboard, LIMIT)?
        && bytes(&s.state.clipboard_markers, LIMIT)? == bytes(&b.clipboard_markers, LIMIT)?
        && bytes(&s.prefs, LIMIT)? == bytes(&b.preferences, LIMIT)?
        && s.loaded_schema == b.loaded_schema
        && match &b.command_journal { Some(journal) => journal.matches(s)?, None => true }
        && match &b.auxiliary {
            Some(aux) => aux.matches(s)?,
            None => true,
        })
}
pub(super) fn open_candidate(s: &mut Session, p: &Value) -> Result<Value> {
    recovery::open(s, p)
}

// Only the read-only companion inspector may use this origin proof. Never
// expose it as a restore, retained-history or current editing capability.
pub(super) fn inspect_recovered_checkpoint(s: &Session, r: &Request, c: &Capsule, hash: &str, sequence: Option<ItemId>) -> Result<bool> {
    recovery::inspect_checkpoint(s, r, c, hash, sequence)
}
