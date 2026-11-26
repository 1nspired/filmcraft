//! Redo the exact native snapshot removed by an owned guarded Undo. No edit
//! recipe replay, compensating command or borrowed human history ownership.
use super::*;
use filmcraft_project::Project;

// Shared by the read contract and the mutating commands. A historical receipt
// alone is never current history ownership.
pub(super) fn owns_undo(s: &Session, read: &Ticket, commit: &Ticket) -> bool {
    let live = std::sync::Arc::downgrade(&s.project);
    commit.committed.as_ref().is_some_and(|(p, _)| p.get("operations").is_some())
        && commit.path == s.path
        && (commit.project_scope.is_some() || commit.after_sequence == read.sequence)
        && commit.project_scope == read.project_scope
        && commit.source_library.is_some() == read.source_library.is_some()
        && commit.after.as_ref().is_some_and(|project| project.ptr_eq(&live))
        && s.agent_bridge.head.as_ref().is_some_and(|(rev, project)| *rev == s.revision && project.ptr_eq(&live))
        && s.history.undo.last().is_some_and(|(_, project)| commit.project.ptr_eq(&std::sync::Arc::downgrade(project)))
}

fn owns_redo(s: &Session, read: &Ticket, commit: &Ticket, undo: &Ticket) -> bool {
    if read.project_scope != commit.project_scope || read.source_library.is_some() != commit.source_library.is_some() {
        return false;
    }
    let live = std::sync::Arc::downgrade(&s.project);
    let owned_head = s.agent_bridge.head.as_ref().is_some_and(|(rev, q)| *rev == s.revision && q.ptr_eq(&live));
    owns_redo_graph(&s.project, &s.path, read.sequence, &s.history, owned_head, commit, undo)
}

fn owns_redo_graph(
    project: &std::sync::Arc<Project>,
    path: &Option<String>,
    sequence: Option<ItemId>,
    history: &crate::History,
    owned_head: bool,
    commit: &Ticket,
    undo: &Ticket,
) -> bool {
    let live = std::sync::Arc::downgrade(project);
    !undo.redo_consumed
        && undo.committed.as_ref().is_some_and(|(_, v)| v.get("action").and_then(Value::as_str) == Some("undo") && v["results"][0]["undone"] == commit.token)
        && commit.committed.as_ref().is_some_and(|(p, _)| p.get("operations").is_some())
        && &undo.path == path
        && &commit.path == path
        && (commit.project_scope.is_some() || undo.after_sequence == sequence)
        && undo.project_scope == commit.project_scope
        && undo.source_library.is_some() == commit.source_library.is_some()
        && undo.after.as_ref().is_some_and(|q| q.ptr_eq(&live))
        && commit.after.as_ref().is_some_and(|q| q.ptr_eq(&undo.project))
        && owned_head
        && history.redo.last().is_some_and(|(_, q)| undo.project.ptr_eq(&std::sync::Arc::downgrade(q)))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct HistoryRead {
    token: String,
    commit: String,
    undo: Option<String>,
    scope: organization_scope::Scope,
    #[serde(default)]
    projection: bool,
}

pub(super) fn has_original(project: &Project, project_id: Option<&str>) -> Result<bool> {
    if project.items.len() > 100_000 {
        return Err(bad("agent.readHistory", "source inventory exceeds its bounded workspace"));
    }
    let mut bound = false;
    for raw in project.items.values().filter_map(|it| it.metadata.get(BINDING_KEY)) {
        let b: Binding = serde_json::from_str(raw).map_err(|e| bad("agent.readHistory", e.to_string()))?;
        bound |= b.role == "original" && project_id.is_none_or(|id| b.project_id == id);
    }
    Ok(bound)
}

/// Applies to a live graph or a fully validated checkpoint graph. Historical
/// source/association data only supplies scope through the exact owned Redo
/// relationship. A current explicit association establishes identity without
/// qualifying any footage or granting a historical edit ticket.
pub(super) fn graph_workspace_scope(
    project: &std::sync::Arc<Project>,
    path: &Option<String>,
    sequence: ItemId,
    history: &crate::History,
    tickets: &VecDeque<Ticket>,
    owned_head: bool,
    scope: &sequences::Scope,
) -> Result<bool> {
    if crate::editorial_workspace::matches(project, scope, None)? || has_original(project, Some(&scope.project_id))? {
        return Ok(true);
    }
    let Some((_, target)) = history.redo.last() else { return Ok(false) };
    if !sequences::belongs(target, sequence, scope)?
        || !(crate::editorial_workspace::matches(target, scope, None)? || has_original(target, Some(&scope.project_id))?)
    {
        return Ok(false);
    }
    for undo in tickets {
        let Some(commit_id) = undo.committed.as_ref().and_then(|(_, v)| v["results"][0]["undone"].as_str()) else { continue };
        if tickets.iter().find(|t| t.token == commit_id).is_some_and(|commit| owns_redo_graph(project, path, Some(sequence), history, owned_head, commit, undo))
        {
            return Ok(true);
        }
    }
    Ok(false)
}

pub(super) fn workspace_scope(s: &Session, scope: &sequences::Scope) -> Result<bool> {
    let sequence = s.state.active_sequence.ok_or(crate::EngineError::NoSequence)?;
    let live = std::sync::Arc::downgrade(&s.project);
    let owned_head = s.agent_bridge.head.as_ref().is_some_and(|(rev, q)| *rev == s.revision && q.ptr_eq(&live));
    graph_workspace_scope(&s.project, &s.path, sequence, &s.history, &s.agent_bridge.tickets, owned_head, scope)
}

/// Read live ownership without consuming a ticket, adopting history or editing
/// anything. This is a prerequisite for host recovery, not durable history.
pub(super) fn read(s: &mut Session, p: &Value) -> Result<Value> {
    let r: HistoryRead = parse("agent.readHistory", p)?;
    if p.get("undo").is_none()
        || !token_ok(&r.token)
        || !token_ok(&r.commit)
        || r.token == r.commit
        || r.undo.as_ref().is_some_and(|u| !token_ok(u) || *u == r.token || *u == r.commit)
    {
        return Err(bad("agent.readHistory", "distinct read/commit/optional Undo tickets and explicit null Undo required"));
    }
    r.scope.read(s, &r.token, r.projection)?;
    super::check_edit_ticket_basis(s, &r.token)?;
    let sequence = r.scope.require(s)?;
    if s.project.items.len() > 100_000 {
        return Err(bad("agent.readHistory", "project exceeds bounded ownership inventory"));
    }
    let ticket = s.agent_bridge.tickets.iter().find(|t| t.token == r.token).ok_or_else(|| bad("agent.readHistory", "expired read ticket"))?;
    let commit = s.agent_bridge.tickets.iter().find(|t| t.token == r.commit);
    let undo = r.undo.as_ref().and_then(|id| s.agent_bridge.tickets.iter().find(|t| &t.token == id));
    let writable = s.revision < u64::MAX - 1;
    let undo_available = writable && commit.is_some_and(|commit| owns_undo(s, ticket, commit));
    let redo_available = writable && commit.zip(undo).is_some_and(|(commit, undo)| owns_redo(s, ticket, commit, undo));
    if let organization_scope::Scope::Sequence(scope) = &r.scope {
        let sequence = sequence.ok_or(crate::EngineError::NoSequence)?;
        if !crate::editorial_workspace::matches(&s.project, scope, None)? && !has_original(&s.project, Some(&scope.project_id))? {
            // Undo of the first admission legitimately removes the last binding.
            // Only the exact still-owned Redo target may supply that missing scope;
            // an arbitrary historical receipt/project must never establish it.
            let restored = match s.history.redo.last() {
                Some((_, project)) if redo_available => {
                    sequences::belongs(project, sequence, scope)?
                        && (crate::editorial_workspace::matches(project, scope, None)? || has_original(project, Some(&scope.project_id))?)
                }
                _ => false,
            };
            if !restored {
                return Err(bad("agent.readHistory", "no current or owned restored original belongs to this owning film"));
            }
        }
    }
    let mut result = json!({"status":"read","token":r.token,"revision":s.revision,"sequence":sequence.map(|id|id.0),"scope":r.scope,
        "commit":r.commit,"undo":r.undo,"undoAvailable":undo_available,"redoAvailable":redo_available,
        "ownership":"live-native-session","writeAuthority":false,"execution":"not-applied"});
    if let Some(commit) = commit.filter(|t| t.committed.as_ref().is_some_and(|(request, _)| request.get("authority") == Some(&json!("document_state")))) {
        document_authority::committed(commit)?;
        result["publicationAuthority"] = json!("document_state");
    }
    if r.scope.source_validation_owed(r.projection) {
        result["requiresSourceValidation"] = json!(true);
    }
    Ok(result)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Redo {
    token: String,
    commit: String,
    undo: String,
}
pub(super) fn redo(s: &mut Session, p: &Value) -> Result<Value> {
    let r: Redo = parse("agent.redo", p)?;
    if [&r.token, &r.commit, &r.undo].iter().any(|v| !token_ok(v)) || r.token == r.commit || r.token == r.undo || r.commit == r.undo {
        return Err(bad("agent.redo", "distinct fresh read, original commit and owned Undo tokens required"));
    }
    let read = s.agent_bridge.tickets.iter().find(|t| t.token == r.token).ok_or_else(|| bad("agent.redo", "unknown/expired read token"))?;
    if let Some((old, result)) = &read.committed {
        return if old == p { Ok(result.clone()) } else { Err(bad("agent.redo", "read token already consumed")) };
    }
    check_edit_ticket_basis(s, &r.token)?;
    let undo = s.agent_bridge.tickets.iter().find(|t| t.token == r.undo).ok_or_else(|| bad("agent.redo", "unknown/expired owned Undo"))?;
    let commit = s.agent_bridge.tickets.iter().find(|t| t.token == r.commit).ok_or_else(|| bad("agent.redo", "unknown/expired original commit"))?;
    if !owns_redo(s, read, commit, undo) {
        return Err(bad("agent.redo", "not the current owned Redo branch; human history, document replacement or an already used Undo intervened"));
    }
    let authority = document_authority::committed(commit)?;
    if authority == document_authority::Authority::MediaInputs {
        check_edit_ticket(s, &r.token)?;
    }
    if s.revision >= u64::MAX - 1 {
        return Err(bad("agent.redo", "revision counter exhausted"));
    }
    // Project metadata ownership is independent of tabs. Preserve the current
    // human view except when restoring the unique initial sequence itself.
    let clear_commit = project_clear::is_commit(commit);
    let target_sequence = if commit.project_scope.is_some() {
        if commit.committed.as_ref().is_some_and(|(_, r)| r["action"] == "project_sequence_create") { commit.after_sequence } else { s.state.active_sequence }
    } else {
        undo.sequence
    };
    let before_sequence = s.state.active_sequence;
    let before = s.revision;
    if authority == document_authority::Authority::MediaInputs {
        validate_media(s, &read.media)?;
    }
    #[cfg(not(target_arch = "wasm32"))]
    let admitted = if authority == document_authority::Authority::DocumentState {
        Vec::new()
    } else {
        super::admission::guard_restored(s, s.history.redo.last().map(|(_, p)| p.as_ref()).ok_or_else(|| bad("agent.redo", "owned target disappeared"))?)?
    };
    #[cfg(not(target_arch = "wasm32"))]
    let restored_original = admitted.iter().copied().collect();
    #[cfg(target_arch = "wasm32")]
    let restored_original = Default::default();
    if authority == document_authority::Authority::DocumentState {
        project_clear::owned_transition(
            &s.project,
            s.history.redo.last().map(|(_, p)| p.as_ref()).ok_or_else(|| bad("agent.redo", "owned target disappeared"))?,
            commit,
        )?;
    } else {
        super::media_scope::require_transition(
            &s.project,
            s.history.redo.last().map(|(_, project)| project.as_ref()).ok_or_else(|| bad("agent.redo", "owned history target disappeared"))?,
            &read.media,
            super::media_scope::Transition {
                before_active: s.state.active_sequence,
                after_active: target_sequence,
                before_source: s.state.source_item,
                after_source: s.state.source_item,
                admitted: restored_original,
            },
        )?;
    }
    enter_publication(s, &r.token)?;
    let label = s.redo().ok_or_else(|| bad("agent.redo", "no owned native Redo step"))?;
    #[cfg(not(target_arch = "wasm32"))]
    for item in admitted {
        s.media.remove(item);
    }
    if let Some(target) = target_sequence.filter(|id| !clear_commit && s.project.sequence(*id).is_some()) {
        s.state.active_sequence = Some(target);
        if !s.state.open_sequences.contains(&target) {
            s.state.open_sequences.push(target);
        }
    }
    if before_sequence != s.state.active_sequence
        && let Some(id) = s.state.active_sequence
    {
        s.events.push(crate::Event::OpenSequence(id));
    }
    let mut result = json!({"status":"committed","token":r.token,"beforeRevision":before,"revision":s.revision,
        "results":[{"redone":r.commit,"undo":r.undo,"label":label}],"sequence":s.state.active_sequence.map(|id|id.0),"validated":true,"action":"redo"});
    document_authority::mark(&mut result, authority)?;
    for t in &mut s.agent_bridge.tickets {
        if t.token == r.undo {
            t.redo_consumed = true;
        }
        if t.token == r.token {
            t.committed = Some((p.clone(), result.clone()));
            t.after = Some(std::sync::Arc::downgrade(&s.project));
            t.after_sequence = s.state.active_sequence;
        }
    }
    s.agent_bridge.head = Some((s.revision, std::sync::Arc::downgrade(&s.project)));
    s.journal.push(("edit.redo".into(), json!({})));
    if s.journal.len() > 10_000 {
        s.journal.drain(..s.journal.len() - 10_000);
    }
    Ok(result)
}
