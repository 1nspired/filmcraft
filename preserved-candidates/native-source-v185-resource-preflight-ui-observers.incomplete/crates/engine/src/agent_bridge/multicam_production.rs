//! Distinct qualified native input/preparation purpose for original multicam services.
//! It is never a metadata/admission ticket, serialized grant, generic executor or history writer.
use super::{Ticket, context, parse, project_plane};
#[cfg(not(target_arch = "wasm32"))]
use super::{admission, media_guard, media_scope, project_audit};
use crate::{Result, Session, commands::bad, project_workspace::ProjectScope};
use filmcraft_project::ItemId;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{collections::BTreeSet, sync::Arc};
pub(super) const COMMIT: &str = "agent.publishMulticamProduction";
pub(super) const READ: &str = "agent.readMulticamInputs";
pub(super) const PLAN: &str = "agent.prepareMulticamProduction";
pub(super) const STATUS: &str = "agent.readMulticamProductionPlan";
#[cfg(not(target_arch = "wasm32"))]
mod collateral;
#[cfg(not(target_arch = "wasm32"))]
pub(super) mod origin;
#[cfg(not(target_arch = "wasm32"))]
mod publication;
const SAFE: u64 = 9_007_199_254_740_991;
fn error(m: impl Into<String>) -> crate::EngineError {
    bad(READ, m)
}
#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum Targets {
    Sources { items: Vec<u64> },
    Timeline { sequence: u64, identities: Vec<String> },
}
#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct Request {
    pub(super) token: String,
    scope: ProjectScope,
    project_path: String,
    targets: Targets,
}
pub(super) struct ReadOwner {
    request: Request,
    projection: Value,
    dependencies: Vec<u64>,
    attempted: bool,
    preferences: crate::autosave::Preferences,
    saved_revision: u64,
    saved_file: Option<crate::saved_file::Baseline>,
    #[cfg(not(target_arch = "wasm32"))]
    prepared: Option<Box<PreparedPlan>>,
}
#[cfg(not(target_arch = "wasm32"))]
struct PreparedPlan {
    candidate: super::Staged,
    summary: Value,
    check: Arc<media_guard::Check>,
    record: publication::Sealed,
    origin: origin::Retained,
}
#[cfg(not(target_arch = "wasm32"))]
struct Resolved {
    projection: Value,
    dependencies: Vec<u64>,
}
fn identity(v: &str) -> bool {
    v.len() == 32 && v.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
#[cfg(not(target_arch = "wasm32"))]
impl Request {
    fn resolve(&self, s: &Session) -> Result<Resolved> {
        project_plane::require_scope(s, &self.scope)?;
        if self.project_path.len() > 4096
            || !self.project_path.starts_with('/')
            || self.project_path.chars().any(char::is_control)
            || s.path.as_ref() != Some(&self.project_path)
        {
            return Err(error("exact saved independent Root path required"));
        }
        filmcraft_project::clip_identity::validate_project(&s.project).map_err(error)?;
        let mut roots = Vec::new();
        let projection = match &self.targets {
            Targets::Sources { items } => {
                if items.is_empty()
                    || items.len() > 2048
                    || items.iter().any(|i| !(1..=SAFE).contains(i))
                    || items.iter().collect::<BTreeSet<_>>().len() != items.len()
                {
                    return Err(error("distinct explicit source items in native input order required"));
                }
                let mut rows = Vec::new();
                for id in items {
                    let id = ItemId(*id);
                    crate::sync::project_clip(&s.project, id)
                        .ok_or_else(|| error("source is not a clip accepted by the original human source-production service"))?;
                    let item = s.project.item(id).ok_or_else(|| error("source item missing"))?;
                    roots.push(id);
                    rows.push(serde_json::to_value(item).map_err(|e| error(e.to_string()))?);
                }
                json!({"kind":"sources","items":rows})
            }
            Targets::Timeline { sequence, identities } => {
                if !(1..=SAFE).contains(sequence)
                    || s.state.active_sequence != Some(ItemId(*sequence))
                    || identities.is_empty()
                    || identities.len() > 2048
                    || identities.iter().any(|i| !identity(i))
                    || identities.iter().collect::<BTreeSet<_>>().len() != identities.len()
                {
                    return Err(error("current explicit sequence and distinct persistent instance identities required"));
                }
                let association = crate::editorial_workspace::association(&s.project)?;
                let ordinary = match &association {
                    Some(a) if a.scope.project_id == self.scope.project_id => super::sequences::belongs(&s.project, ItemId(*sequence), &a.scope)?,
                    _ => false,
                };
                let derived = s.project.item(ItemId(*sequence)).is_some_and(|item| {
                    item.as_sequence().is_some_and(|q| q.multicam.is_some() || q.merged.is_some())
                        && !item.metadata.contains_key(crate::editorial_workspace::KEY)
                        && !item.metadata.contains_key(crate::editorial_workspace::SEQUENCE_KEY)
                });
                if !ordinary && !derived {
                    return Err(error("timeline is outside the actual original owning sequence/Root or an explicit native derived source"));
                }
                if derived && !ordinary {
                    // Existing bounded original physical-tree validator; source flags
                    // alone never supply membership, an owning anchor or file witnesses.
                    super::bins::require_visible_targets(&s.project, &[*sequence])?;
                }
                let q = s.project.sequence(ItemId(*sequence)).ok_or_else(|| error("timeline sequence missing"))?;
                let mut selected = Vec::new();
                for id in identities {
                    let matches =
                        q.all_tracks().flat_map(|t| &t.items).filter(|c| c.identity.as_ref().is_some_and(|i| &i.id == id)).map(|c| c.id).collect::<Vec<_>>();
                    if matches.len() != 1 {
                        return Err(error("persistent instance identity missing or ambiguous"));
                    }
                    selected.push(*matches.first().ok_or_else(|| error("instance missing"))?);
                }
                let complete = crate::commands::linked_clips(s, &selected);
                let mut rows = Vec::new();
                for id in complete {
                    let (tid, c) = q.find_item(id).ok_or_else(|| error("linked instance disappeared"))?;
                    let track = q.track(tid).ok_or_else(|| error("linked track disappeared"))?;
                    if c.identity.is_none() {
                        return Err(error("a linked partner has no persistent identity; track it through the original human service first"));
                    }
                    roots.push(c.item);
                    rows.push(json!({"track":tid.0,"locked":track.locked,"requested":selected.contains(&id),"clip":c}));
                }
                json!({"kind":"timeline","sequence":sequence,"instances":rows,"settings":q.settings,"nativeLinkedSelection":s.state.linked_selection})
            }
        };
        // Keep the SAME original monitor/current-sequence dependencies as normal scoped work.
        if let Some(active) = s.state.active_sequence {
            roots.push(active);
        }
        if let Some(source) = s.state.source_item.filter(|id| media_scope::monitor_dependency(&s.project, *id)) {
            roots.push(source);
        }
        let dependencies = media_scope::dependencies(&s.project, roots)?;
        for id in &dependencies {
            admission::current_generation(s.project.item(ItemId(*id)).ok_or_else(|| error("original dependency missing"))?, &self.scope)?;
        }
        if project_audit::value_bytes(&projection)?.len() > 7 * 1024 * 1024 {
            return Err(error("complete multicam input projection exceeds its bound; nothing truncated"));
        }
        Ok(Resolved { projection, dependencies })
    }
}
#[cfg(not(target_arch = "wasm32"))]
pub(super) fn read(s: &mut Session, p: &Value) -> Result<Value> {
    let request: Request = parse(READ, p)?;
    super::fresh_snapshot(s, &request.token)?;
    let resolved = request.resolve(s)?;
    super::snapshot_jobs::start_production(s, request, resolved.dependencies)
}
#[cfg(not(target_arch = "wasm32"))]
fn owner_generations(s: &Session, scope: &ProjectScope, files: &[u64], media: &media_guard::Witnesses, check: &media_guard::Check) -> Result<()> {
    media.require(files)?;
    for id in files {
        check.check()?;
        let item = s.project.item(ItemId(*id)).ok_or_else(|| error("original source disappeared"))?;
        let generation = admission::current_generation(item, scope)?;
        let file = item
            .as_media()
            .and_then(|m| if let filmcraft_project::MediaRef::File { path } = &m.media { Some(path) } else { None })
            .ok_or_else(|| error("original source is not file media"))?;
        media.require_generation(*id, file, generation.bytes, &generation.sha256, check)?;
        let observed = s.media.probe_file(file, s.services.as_ref()).map_err(|e| error(e.to_string()))?;
        if observed.info() != &item.as_media().ok_or_else(|| error("original media disappeared"))?.info {
            return Err(error("current imported stream metadata differs from original native source bytes"));
        }
        check.check()?;
    }
    media_guard::validate_checked(&s.project, &s.media, media, check)?;
    Ok(())
}
#[cfg(not(target_arch = "wasm32"))]
pub(super) fn deliver(s: &mut Session, r: Request, media: Arc<media_guard::Witnesses>) -> Result<Value> {
    super::fresh_snapshot(s, &r.token)?;
    let resolved = r.resolve(s)?;
    let check = media_guard::Check::new(web_time::Instant::now());
    owner_generations(s, &r.scope, &resolved.dependencies, &media, &check)?;
    let epoch = super::history_checkpoint::epoch(s)?;
    let doc = json!({"kind":"native_multicam_production_inputs","version":1,"protocol":1,"token":r.token,"scope":r.scope,"projectPath":s.path,"nativeSession":epoch,"revision":s.revision,"targets":resolved.projection,"mediaScope":media.scope_json(),"fileDependencies":resolved.dependencies,
 "capabilities":{"multicamProductionInputs":true,"multicamProductionPreparation":s.worker_available&&super::operation_cancellation_available(s),"multicamProductionPublication":s.worker_available&&super::operation_cancellation_available(s),"backgroundSnapshots":true,"sourceFileGuard":true,"operationCancellation":super::operation_cancellation_available(s),"guardedOperationDeadlineMs":media_guard::OPERATION_DEADLINE_MS,"largeSourceVerification":true,"largeSourceVerificationDeadlineMs":media_guard::LARGE_SOURCE_DEADLINE_MS,"largeSourceVerificationThresholdBytes":media_guard::LARGE_SOURCE_BYTES},"writeAuthority":false});
    media_guard::forget_changed(&s.media, &media, s.agent_bridge.media.as_deref());
    s.agent_bridge.media = Some(media.clone());
    let editor = context(s)?;
    super::register_ticket(
        s,
        Ticket {
            token: r.token.clone(),
            revision: s.revision,
            sequence: s.state.active_sequence,
            project_scope: Some(r.scope.clone()),
            source_library: None,
            #[cfg(not(target_arch = "wasm32"))]
            multicam_history: None,
            multicam_production: Some(Box::new(ReadOwner {
                request: r,
                projection: resolved.projection,
                dependencies: resolved.dependencies,
                attempted: false,
                preferences: s.prefs.clone(),
                saved_revision: s.saved_revision,
                saved_file: s.saved_file.clone(),
                prepared: None,
            })),
            editor_control: None,
            sequence_views: None,
            project: Arc::downgrade(&s.project),
            path: s.path.clone(),
            context: editor,
            committed: None,
            after: None,
            after_sequence: None,
            redo_consumed: false,
            media,
            preparation: None,
        },
    )?;
    Ok(doc)
}
/// A completed intent with no edit is data, never an owned history step.
pub(super) fn unchanged_result(result: &Value) -> bool {
    result["action"] == "multicam_production" && result["outcome"] == "unchanged"
}
pub(super) fn source_purpose(t: &Ticket) -> bool {
    #[cfg(not(target_arch = "wasm32"))]
    if t.multicam_history.is_some() {
        return true;
    }
    t.multicam_production.is_some()
}
pub(super) fn is_ticket(s: &Session, token: &str) -> bool {
    s.agent_bridge.tickets.iter().any(|t| t.token == token && source_purpose(t))
}
pub(super) fn command_scope(s: &Session, id: &str, p: &Value) -> Result<bool> {
    let token = p.get("token").or_else(|| p.get("batch").and_then(|b| b.get("token"))).and_then(Value::as_str);
    if !token.is_some_and(|t| is_ticket(s, t)) {
        return Ok(false);
    }
    if !matches!(id, PLAN | STATUS | "agent.apply" | "agent.receipt" | "agent.readHistory" | "agent.undo" | "agent.redo") {
        return Err(error("source-qualified purposes cannot borrow metadata, admission, navigation or generic executors"));
    }
    if id == "agent.apply" {
        let batch: super::Batch = parse(id, p)?;
        let ticket = s.agent_bridge.tickets.iter().find(|t| Some(t.token.as_str()) == token).ok_or_else(|| error("source purpose expired"))?;
        validate_batch(ticket.project_scope.as_ref().ok_or_else(|| error("source Root missing"))?, &batch)?;
    }
    Ok(true)
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CommitRequest {
    scope: ProjectScope,
    record_sha256: String,
}
pub(super) fn validate_operation(op: &super::Operation) -> Result<()> {
    let r: CommitRequest = parse(COMMIT, &op.params)?;
    if op.command != COMMIT
        || !crate::project_workspace::valid_scope(&r.scope)
        || r.record_sha256.len() != 64
        || !r.record_sha256.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err(error("one original source scope and complete retained computed-record hash required"));
    }
    Ok(())
}
pub(super) fn is_batch(b: &super::Batch) -> bool {
    b.operations.iter().any(|op| op.command == COMMIT)
}
pub(super) fn validate_batch(scope: &ProjectScope, b: &super::Batch) -> Result<()> {
    if b.authority != super::document_authority::Authority::MediaInputs
        || !b.exact
        || !b.validate_at_publication
        || b.admission_preparation.is_some()
        || b.operations.len() != 1
    {
        return Err(error("source publication requires one exact MediaInputs operation with final native validation"));
    }
    let op = b.operations.first().ok_or_else(|| error("source operation missing"))?;
    validate_operation(op)?;
    let r: CommitRequest = parse(COMMIT, &op.params)?;
    if &r.scope != scope {
        return Err(error("source publication Root differs"));
    }
    Ok(())
}
#[cfg(not(target_arch = "wasm32"))]
pub(super) fn validate_read(s: &Session, token: &str, scope: &ProjectScope, projection: bool) -> Result<()> {
    let t = current(s, token, scope)?;
    if !projection {
        owner_generations(
            s,
            scope,
            &t.multicam_production.as_ref().ok_or_else(|| error("fresh qualified input purpose required"))?.dependencies,
            &t.media,
            &media_guard::Check::new(web_time::Instant::now()),
        )?;
    }
    Ok(())
}
#[cfg(target_arch = "wasm32")]
pub(super) fn validate_read(_: &Session, _: &str, _: &ProjectScope, _: bool) -> Result<()> {
    Err(error("qualified source history requires native input verification"))
}
#[cfg(not(target_arch = "wasm32"))]
fn live_plan<'a>(s: &'a Session, b: &super::Batch) -> Result<&'a PreparedPlan> {
    let t = s.agent_bridge.tickets.iter().find(|t| t.token == b.token).ok_or_else(|| error("source input expired"))?;
    validate_batch(t.project_scope.as_ref().ok_or_else(|| error("source Root missing"))?, b)?;
    if !s.worker_available || !super::operation_cancellation_available(s) {
        return Err(error("installed original native worker and independent Stop required"));
    }
    let r: CommitRequest = parse(COMMIT, &b.operations.first().ok_or_else(|| error("source operation missing"))?.params)?;
    let t = current(s, &b.token, &r.scope)?;
    let owner = t.multicam_production.as_ref().ok_or_else(|| error("historical origin cannot replace fresh qualified input"))?;
    let p = owner.prepared.as_deref().ok_or_else(|| error("the same original producer has no retained preparation"))?;
    p.check.check()?;
    p.record.validate_live(s, &p.candidate)?;
    if p.record.metadata()?["sha256"] != r.record_sha256 {
        return Err(error("retained computed record differs; nothing was recomputed or published"));
    }
    let check = s.agent_bridge.operations.check().ok_or_else(|| error("original native operation fence required"))?;
    owner_generations(s, &r.scope, &owner.dependencies, &t.media, &check)?;
    media_scope::require_transition(
        &s.project,
        &p.candidate.project,
        &t.media,
        media_scope::Transition {
            before_active: s.state.active_sequence,
            after_active: p.candidate.state.active_sequence,
            before_source: s.state.source_item,
            after_source: p.candidate.state.source_item,
            admitted: BTreeSet::new(),
        },
    )?;
    check.check()?;
    Ok(p)
}
#[cfg(not(target_arch = "wasm32"))]
pub(super) fn stage(s: &Session, b: &super::Batch) -> Result<super::Staged> {
    Ok(live_plan(s, b)?.candidate.clone())
}
#[cfg(not(target_arch = "wasm32"))]
pub(super) struct Publication {
    pub(super) origin: origin::Retained,
    pub(super) record: Value,
    pub(super) origin_metadata: Value,
    pub(super) journal: (String, Value),
    pub(super) unchanged: bool,
}
#[cfg(not(target_arch = "wasm32"))]
pub(super) fn publication(s: &Session, b: &super::Batch, staged: &super::Staged) -> Result<Option<Publication>> {
    if !is_batch(b) {
        return Ok(None);
    }
    let p = live_plan(s, b)?;
    if !Arc::ptr_eq(&p.candidate.project, &staged.project)
        || project_audit::value_bytes(&p.candidate.state)? != project_audit::value_bytes(&staged.state)?
        || p.candidate.results != staged.results
    {
        return Err(error("publication substituted the retained native candidate"));
    }
    let origin = p.origin.clone();
    let record = p.record.metadata()?;
    let origin_metadata = origin.metadata()?;
    let journal = ("native.multicamComputed".into(), origin.passive_entry()?);
    let unchanged = Arc::ptr_eq(&s.project, &staged.project);
    if unchanged {
        if p.summary["humanCommand"] != "clip.synchronize"
            || staged.results.first().is_none_or(|v| v["moved"] != 0)
            || project_audit::value_bytes(&s.state)? != project_audit::value_bytes(&staged.state)?
            || !staged.events.is_empty()
        {
            return Err(error("no-change acknowledgment requires the exact original unchanged synchronization outcome"));
        }
    } else {
        super::native_editor::command_journal::CommandJournal::check_append(s, &journal)?;
    }
    Ok(Some(Publication { origin, record, origin_metadata, journal, unchanged }))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Status {
    token: String,
    scope: ProjectScope,
}
#[cfg(not(target_arch = "wasm32"))]
fn current<'a>(s: &'a Session, token: &str, scope: &ProjectScope) -> Result<&'a Ticket> {
    super::check_edit_ticket_basis(s, token)?;
    project_plane::require_scope(s, scope)?;
    let ticket = s.agent_bridge.tickets.iter().find(|t| t.token == token).ok_or_else(|| error("multicam input ticket expired"))?;
    let owner = ticket.multicam_production.as_ref().ok_or_else(|| error("a distinct qualified multicam input purpose is required"))?;
    if &owner.request.scope != scope {
        return Err(error("multicam input Root differs"));
    }
    if owner.preferences != s.prefs || owner.saved_revision != s.saved_revision || owner.saved_file != s.saved_file {
        return Err(error("native preferences or exact saved owner changed; qualify fresh inputs"));
    }
    let now = owner.request.resolve(s)?;
    if now.projection != owner.projection || now.dependencies != owner.dependencies {
        return Err(error("multicam input dependency basis changed"));
    }
    Ok(ticket)
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Options {
    method: Option<String>,
    ignore_hours: Option<bool>,
    marker: Option<String>,
    offset: Option<i64>,
    reference: Option<Value>,
    track: Option<Value>,
    name: Option<String>,
    remove_video_audio: Option<bool>,
    audio: Option<String>,
    camera_names: Option<String>,
    processed_bin: Option<bool>,
}
#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum Gesture {
    Synchronize,
    Merge,
    CreateMulticam,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Edit {
    gesture: Gesture,
    options: Options,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PlanRequest {
    token: String,
    scope: ProjectScope,
    edit: Edit,
}
#[cfg(not(target_arch = "wasm32"))]
fn human(s: &Session, owner: &ReadOwner, edit: &Edit) -> Result<(&'static str, Value)> {
    let o = &edit.options;
    let mut p = json!({});
    for (k, v) in [
        ("method", o.method.as_ref().map(|v| json!(v))),
        ("ignoreHours", o.ignore_hours.map(|v| json!(v))),
        ("marker", o.marker.as_ref().map(|v| json!(v))),
        ("offset", o.offset.map(|v| json!(v))),
    ] {
        if let Some(v) = v {
            p[k] = v;
        }
    }
    let command = match (&owner.request.targets, &edit.gesture) {
        (Targets::Timeline { .. }, Gesture::Synchronize) => {
            if o.name.is_some() || o.remove_video_audio.is_some() || o.audio.is_some() || o.camera_names.is_some() || o.processed_bin.is_some() {
                return Err(error("source-production options cannot be borrowed by timeline synchronization"));
            }
            let rows = owner.projection["instances"].as_array().ok_or_else(|| error("current explicit linked instance group missing"))?;
            p["clips"] = json!(rows.iter().filter(|v| v["requested"] == true).map(|v| v["clip"]["id"].clone()).collect::<Vec<_>>());
            if let Some(reference) = &o.reference {
                let identity =
                    reference.as_str().filter(|v| identity(v)).ok_or_else(|| error("timeline reference must be a retained persistent instance identity"))?;
                let row = rows
                    .iter()
                    .find(|v| v["clip"]["identity"]["id"] == identity)
                    .ok_or_else(|| error("timeline reference is outside retained linked instances"))?;
                p["reference"] = row["clip"]["id"].clone();
            }
            if let Some(track) = &o.track {
                p["track"] = track.clone();
                crate::commands::track_p(s, &p, "track", "clip.synchronize")?;
            }
            "clip.synchronize"
        }
        (Targets::Sources { items }, Gesture::Merge | Gesture::CreateMulticam) => {
            if o.track.is_some() {
                return Err(error("a timeline reference track cannot target Project sources"));
            }
            p["items"] = json!(items);
            if let Some(name) = &o.name {
                p["name"] = json!(name);
            }
            if matches!(edit.gesture, Gesture::Merge) {
                if o.reference.is_some() || o.audio.is_some() || o.camera_names.is_some() || o.processed_bin.is_some() {
                    return Err(error("multicam-only options cannot target Merge Clips"));
                }
                if let Some(remove) = o.remove_video_audio {
                    p["removeVideoAudio"] = json!(remove);
                }
                "clip.mergeClips"
            } else {
                if o.remove_video_audio.is_some() {
                    return Err(error("Merge-only audio removal cannot target multicam creation"));
                }
                if let Some(reference) = &o.reference {
                    let id = reference
                        .as_u64()
                        .filter(|id| items.contains(id))
                        .ok_or_else(|| error("source reference is outside the exact retained source order"))?;
                    p["reference"] = json!(id);
                }
                if let Some(audio) = &o.audio {
                    p["audio"] = json!(audio);
                }
                if let Some(names) = &o.camera_names {
                    p["cameraNames"] = json!(names);
                }
                if let Some(bin) = o.processed_bin {
                    p["processedBin"] = json!(bin);
                }
                "clip.createMulticam"
            }
        }
        _ => return Err(error("gesture target plane differs from qualified inputs")),
    };
    Ok((command, p))
}
#[cfg(not(target_arch = "wasm32"))]
pub(super) fn prepare(s: &mut Session, p: &Value) -> Result<Value> {
    if project_audit::value_bytes(p)?.len() > 7 * 1024 * 1024 {
        return Err(error("complete human gesture exceeds the native input bound"));
    }
    let r: PlanRequest = parse(PLAN, p)?;
    if !s.worker_available || !super::operation_cancellation_available(s) {
        return Err(error("multicam preparation requires the installed original worker and independent Stop"));
    }
    let ticket = current(s, &r.token, &r.scope)?;
    let owner = ticket.multicam_production.as_ref().ok_or_else(|| error("qualified purpose missing"))?;
    if owner.attempted {
        return Err(error("preparation was already attempted; read its retained result or use fresh inputs, never recompute after an unknown reply"));
    }
    if s.agent_bridge.tickets.iter().filter_map(|t| t.multicam_production.as_ref()).filter(|o| o.prepared.is_some()).count() >= 2 {
        return Err(error("two retained native multicam plans are still owned; cancel one before another preparation"));
    }
    let (command, params) = human(s, owner, &r.edit)?;
    let media = ticket.media.clone();
    let dependencies = owner.dependencies.clone();
    let stop = s.agent_bridge.operations.preparation_stop(&r.token)?;
    let check = Arc::new(media_guard::Check::for_sources(web_time::Instant::now(), stop, media.source_bytes()));
    let ticket = s.agent_bridge.tickets.iter_mut().find(|t| t.token == r.token).ok_or_else(|| error("qualified purpose expired"))?;
    ticket.multicam_production.as_mut().ok_or_else(|| error("qualified purpose missing"))?.attempted = true;
    owner_generations(s, &r.scope, &dependencies, &media, &check)?;
    media_guard::validate_checked(&s.project, &s.media, &media, &check)?;
    let before = project_plane::basis(s)?;
    let mut stage = Session::new(s.services.clone());
    stage.project = s.project.clone();
    stage.media = s.media.clone();
    stage.state = s.state.clone();
    stage.prefs = s.prefs.clone();
    stage.revision = s.revision;
    stage.saved_revision = s.saved_revision;
    stage.path = s.path.clone();
    let spec = crate::commands::find(command).ok_or_else(|| error("original human gesture is unavailable"))?;
    stage.enabled_for(spec, &params).map_err(|why| error(format!("original human gesture is disabled: {why}")))?;
    let prepared = crate::multicam::production::prepare(&stage, command, &params)?;
    check.check()?;
    let result = prepared.apply(&mut stage)?;
    check.check()?;
    crate::clip_identity::enforce_protection(&s.project, &stage.project)?;
    for item in s.project.sequences() {
        if let Some(q) = item.as_sequence() {
            for track in q.all_tracks().filter(|t| t.locked) {
                let after = stage.project.sequence(item.id).and_then(|q| q.track(track.id));
                if project_audit::value_bytes(&Some(track))? != project_audit::value_bytes(&after)? {
                    return Err(error("prepared synchronization would change a locked track"));
                }
            }
        }
    }
    media_scope::require_transition(
        &s.project,
        &stage.project,
        &media,
        media_scope::Transition {
            before_active: s.state.active_sequence,
            after_active: stage.state.active_sequence,
            before_source: s.state.source_item,
            after_source: stage.state.source_item,
            admitted: BTreeSet::new(),
        },
    )?;
    current(s, &r.token, &r.scope)?;
    owner_generations(s, &r.scope, &dependencies, &media, &check)?;
    check.check()?;
    let after = project_plane::project_basis(&stage.project)?;
    let mut summary = json!({"kind":"native_multicam_production_plan","version":1,"status":"prepared","token":r.token,"scope":r.scope,"humanCommand":command,"humanParams":params,"nativeLinkedSelection":s.state.linked_selection,"beforeProjectSha256":before,"afterProjectSha256":after,"result":result,"fileDependencies":dependencies,"writeAuthority":false,"publicationSupported":s.worker_available&&super::operation_cancellation_available(s)});
    if project_audit::value_bytes(&summary)?.len() > 7 * 1024 * 1024 {
        return Err(error("complete prepared report exceeds the transport bound; nothing truncated"));
    }
    let candidate = super::Staged {
        project: stage.project,
        state: stage.state,
        events: stage.events,
        results: vec![result],
        effective: vec![json!({"command":command,"params":params})],
        admission: Vec::new(),
    };
    let record = publication::seal(s, &r.token, &candidate, command, &params, &summary["result"])?;
    let origin = origin::capture(s, &r.token, &candidate, &record)?;
    summary["retainedOrigin"] = origin.metadata()?;
    summary["computedRecord"] = record.metadata()?;
    if project_audit::value_bytes(&summary)?.len() > 7 * 1024 * 1024 {
        return Err(error("complete prepared report including its record exceeds the transport bound"));
    }
    check.check()?;
    let ticket = s.agent_bridge.tickets.iter_mut().find(|t| t.token == r.token).ok_or_else(|| error("qualified purpose expired"))?;
    ticket.multicam_production.as_mut().ok_or_else(|| error("qualified purpose missing"))?.prepared =
        Some(Box::new(PreparedPlan { candidate, summary: summary.clone(), check, record, origin }));
    Ok(summary)
}
#[cfg(not(target_arch = "wasm32"))]
pub(super) fn status(s: &mut Session, p: &Value) -> Result<Value> {
    let r: Status = parse(STATUS, p)?;
    let ticket = current(s, &r.token, &r.scope)?;
    let owner = ticket.multicam_production.as_ref().ok_or_else(|| error("qualified purpose missing"))?;
    let _stop = s.agent_bridge.operations.preparation_stop(&r.token)?;
    let check = media_guard::Check::new(web_time::Instant::now());
    owner_generations(s, &r.scope, &owner.dependencies, &ticket.media, &check)?;
    if let Some(plan) = &owner.prepared {
        plan.check.check()?;
        plan.record.validate_live(s, &plan.candidate)?;
        let pool = [s.project.clone(), plan.candidate.project.clone()];
        let encoded = plan.origin.saved(&pool)?.encoded()?;
        let saved = origin::Saved::decode(&encoded)?;
        let historical_origin = saved.restore(&pool, &r.scope, s.path.as_deref().ok_or_else(|| error("saved path missing"))?, &r.token, s.revision)?;
        if historical_origin.metadata()? != plan.origin.metadata()? || plan.summary["retainedOrigin"] != historical_origin.metadata()? {
            return Err(error("retained origin metadata differs"));
        }

        // Parse the exact native journal data now, without replay or a live-owner constructor.
        let historical = publication::Historical::decode(&plan.record.bytes()?)?;
        historical.validate_inputs(s, &owner.request, &owner.projection, &owner.dependencies)?;
        historical.validate_captured(
            &s.project,
            &plan.candidate.project,
            &s.state,
            &plan.candidate.state,
            plan.summary["humanCommand"].as_str().ok_or_else(|| error("computed human command missing"))?,
            &plan.summary["humanParams"],
            &plan.summary["result"],
        )?;
        if plan.summary["computedRecord"] != historical.metadata()? {
            return Err(error("retained computed record metadata differs"));
        }
        if plan.summary["afterProjectSha256"] != project_plane::project_basis(&plan.candidate.project)? {
            return Err(error("retained native candidate differs"));
        }
        Ok(plan.summary.clone())
    } else {
        Ok(
            json!({"kind":"native_multicam_production_plan","version":1,"token":r.token,"scope":r.scope,"status":if owner.attempted{"failed"}else{"unprepared"},"writeAuthority":false,"publicationSupported":s.worker_available&&super::operation_cancellation_available(s)}),
        )
    }
}
#[cfg(target_arch = "wasm32")]
pub(super) fn read(_: &mut Session, _: &Value) -> Result<Value> {
    Err(error("qualified multicam source preparation requires the native engine"))
}
#[cfg(target_arch = "wasm32")]
pub(super) fn prepare(_: &mut Session, _: &Value) -> Result<Value> {
    Err(error("qualified multicam source preparation requires the native engine"))
}
#[cfg(target_arch = "wasm32")]
pub(super) fn status(_: &mut Session, _: &Value) -> Result<Value> {
    Err(error("qualified multicam source preparation requires the native engine"))
}
#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests;
