//! Bounded, immutable Program and explicitly scoped source-layout evidence. Uses the existing CPU compositor and mixer, never a
//! live seek/screenshot, export job, file write or a new rendering implementation.
use std::sync::{
    Arc, Mutex, Weak,
    atomic::{AtomicBool, AtomicU8, AtomicUsize, Ordering},
};
use std::time::Duration;
use web_time::Instant;

use filmcraft_frame::{AudioBuffer, VideoFrame};
use filmcraft_media::{FrameRequest, MediaInfo, MediaSource, SharedSource};
use filmcraft_project::{ItemId, Project};
use filmcraft_render::{RenderOptions, SourceProvider};
use filmcraft_time::TICKS_PER_SECOND;
use serde::Deserialize;
use serde_json::{Value, json};

use super::{
    BINDING_KEY, Binding, idle,
    media_guard::{self, Check, Plan},
    parse, sequences, token_ok,
};
use crate::{
    Result, Session,
    commands::{CommandSpec, always, bad},
    media_pool::PoolProvider,
};

const START: &str = "agent.inspect";
const MAX_JOBS: usize = 4;
const DEADLINE: Duration = Duration::from_secs(45);
static WORKERS: AtomicUsize = AtomicUsize::new(0);
#[cfg(test)]
pub(super) static TEST_SERIAL: Mutex<()> = Mutex::new(());

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Start {
    token: String,
    job: String,
    scope: super::organization_scope::Scope,
    request: Request,
    #[serde(default)]
    candidate: Option<String>,
    #[serde(default)]
    background: bool,
}
#[derive(Clone, Deserialize)]
#[serde(tag = "kind", deny_unknown_fields)]
enum Request {
    #[serde(rename = "source_audio_layout", rename_all = "camelCase")]
    SourceAudioLayout { item: u64, generation: super::admission::Generation },
    #[serde(rename = "frame", rename_all = "camelCase")]
    Frame { frame: i64, max_side: u32 },
    #[serde(rename = "audio", rename_all = "camelCase")]
    Audio { start_sample: i64, samples: usize },
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Poll {
    job: String,
}

enum State {
    Running,
    Done(std::result::Result<Value, String>),
    Cancelled,
}
pub(super) struct Job {
    id: String,
    request: Value,
    project: Weak<Project>,
    path: Option<String>,
    revision: u64,
    sequence: Option<ItemId>,
    library: Option<crate::project_workspace::ProjectScope>,
    state: Arc<Mutex<State>>,
    stop: Arc<AtomicBool>,
    started: Instant,
    budget: Duration,
    candidate: Option<(String, Weak<Project>)>,
    candidate_operation: Option<String>,
    media: Arc<super::media_guard::Witnesses>,
    verification: Option<Arc<Verification>>,
    source_audio_proofs: Arc<Mutex<Vec<Arc<media_guard::AdmissionWitness>>>>,
}
impl Job {
    fn cancel_work(&self) {
        self.stop.store(true, Ordering::Release);
        if let Some(v) = &self.verification {
            v.check.stop.store(true, Ordering::Release);
        }
    }
}
struct Verification {
    plan: Plan,
    check: Check,
    phase: AtomicU8,
}
impl Verification {
    fn verify(&self, expected: &[Arc<media_guard::Witnesses>]) -> Result<()> {
        self.plan.recheck(&expected.iter().map(Arc::as_ref).collect::<Vec<_>>(), &self.check)
    }
}
impl Drop for Job {
    fn drop(&mut self) {
        self.cancel_work();
    }
}
struct Worker;
impl Worker {
    fn acquire() -> Result<Self> {
        WORKERS
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |n| (n < 2).then_some(n + 1))
            .map(|_| Self)
            .map_err(|_| bad(START, "two native evidence workers are still running; wait before another read"))
    }
}
impl Drop for Worker {
    fn drop(&mut self) {
        WORKERS.fetch_sub(1, Ordering::SeqCst);
    }
}

fn current(s: &Session, j: &Job) -> bool {
    if let Some(scope) = &j.library {
        if super::library_plane::validate_ticket(s, j.request["token"].as_str().unwrap_or(""), scope, true).is_err() {
            return false;
        }
    }
    s.revision == j.revision
        && s.path == j.path
        && s.state.active_sequence == j.sequence
        && j.project.ptr_eq(&Arc::downgrade(&s.project))
        && match &j.verification {
            Some(v) => {
                v.plan.rebuild(&s.project, &s.media).is_ok_and(|p| p == v.plan)
                    && j.candidate.as_ref().is_none_or(|(id, project)| super::candidates::read_basis(s, id).is_ok_and(|(now, _)| now.ptr_eq(project)))
            }
            None => {
                super::media_guard::validate(&s.project, &s.media, &j.media).is_ok()
                    && j.candidate.as_ref().is_none_or(|(id, project)| super::candidates::read_current(s, id, project))
            }
        }
}
// Ownership check for a bounded background postflight. File bytes are checked
// by that worker; the ordinary inspection path retains its synchronous guard.
pub(super) fn read_basis(s: &Session, id: &str, token: &str, candidate: Option<&str>) -> Result<Arc<super::media_guard::Witnesses>> {
    let j = s.agent_bridge.inspections.iter().find(|j| j.id == id).ok_or_else(|| bad(START, "unknown/expired inspection"))?;
    if let Some(scope) = &j.library {
        super::library_plane::validate_ticket(s, token, scope, true)?;
    }
    if j.request["token"] != token
        || j.candidate.as_ref().map(|c| c.0.as_str()) != candidate
        || j.started.elapsed() >= j.budget
        || j.stop.load(Ordering::Relaxed)
        || s.revision != j.revision
        || s.path != j.path
        || s.state.active_sequence != j.sequence
        || !j.project.ptr_eq(&Arc::downgrade(&s.project))
        || !matches!(&*j.state.lock().unwrap_or_else(|e| e.into_inner()), State::Done(Ok(_)))
    {
        return Err(bad(START, "inspection is not a complete current read owned by this exact ticket/view"));
    }
    if let Some((id, project)) = &j.candidate {
        let (current, _) = super::candidates::read_basis(s, id)?;
        if !project.ptr_eq(&current) {
            return Err(bad(START, "candidate inspection basis changed"));
        }
    }
    Ok(j.media.clone())
}
pub(super) fn admission_basis(s: &Session, id: &str, token: &str, candidate: Option<&str>) -> Result<Vec<Arc<media_guard::AdmissionWitness>>> {
    read_basis(s, id, token, candidate)?;
    let j = s.agent_bridge.inspections.iter().find(|j| j.id == id).ok_or_else(|| bad(START, "source inspection disappeared"))?;
    let proofs = j.source_audio_proofs.lock().unwrap_or_else(|e| e.into_inner()).clone();
    if j.request["request"]["kind"] == "source_audio_layout" && proofs.len() != 1 {
        return Err(bad(START, "source layout has no complete original-byte proof"));
    }
    Ok(proofs)
}
fn status(s: &Session, j: &Job) -> Result<Value> {
    if !current(s, j) {
        j.cancel_work();
        *j.state.lock().unwrap_or_else(|e| e.into_inner()) = State::Cancelled;
        return Err(bad(START, "native document/sequence or original inputs changed during inspection; discard the evidence and read again"));
    }
    let mut state = j.state.lock().unwrap_or_else(|e| e.into_inner());
    if j.started.elapsed() >= j.budget {
        j.cancel_work();
        *state = State::Cancelled;
    }
    let mut result = match &*state {
        State::Running => json!({"job": j.id, "status": "running"}),
        State::Cancelled => json!({"job": j.id, "status": "cancelled"}),
        State::Done(Err(message)) => json!({"job": j.id, "status": "failed", "error": message}),
        State::Done(Ok(result)) => json!({"job": j.id, "status": "complete", "revision": j.revision, "sequence": j.sequence.map(|id|id.0), "result": result}),
    };
    if let Some(scope) = &j.library {
        result["scope"] = json!(scope);
    }
    if let Some(v) = &j.verification {
        if let Some(policy) = v.check.policy() {
            result["verification_policy"] = policy;
        }
        result["background"] = json!(true);
        result["requiresPostflight"] = json!(true);
        result["phase"] = json!(match v.phase.load(Ordering::Relaxed) {
            0 => "verifying_inputs",
            1 if j.request["request"]["kind"] == "source_audio_layout" => "reading_source_layout",
            1 => "rendering",
            2 => "verifying_output",
            _ => "complete",
        });
        result["progress"] = json!({"completed_checks":v.check.files.load(Ordering::Relaxed),"total_checks":v.plan.validation_total().saturating_mul(2),"bytes_read":v.check.bytes.load(Ordering::Relaxed).to_string()});
    }
    if let Some((id, _)) = &j.candidate {
        result["candidate"] = json!(id);
        result["view"] = json!("condense_candidate");
        result["candidateOperation"] = json!(j.candidate_operation);
    } else if j.request["request"]["kind"] == "source_audio_layout" {
        result["view"] = json!("source_audio_layout");
    }
    Ok(result)
}

fn validate(project: &Project, sequence: Option<ItemId>, r: &Request) -> Result<()> {
    if let Request::SourceAudioLayout { item, generation } = r {
        return super::source_audio::validate(project, None, *item, generation);
    }
    let sequence = sequence.ok_or(crate::EngineError::NoSequence)?;
    let q = project.sequence(sequence).ok_or(crate::EngineError::NoSequence)?;
    q.settings.validate().map_err(|e| bad(START, e))?;
    validate_nests(project, sequence, &mut Vec::new(), &mut std::collections::BTreeSet::new(), &mut 100_000)?;
    match r {
        Request::SourceAudioLayout { .. } => return Err(bad(START, "source layout must use its source validator")),
        Request::Frame { frame, max_side } => {
            if !(0..=1_000_000).contains(frame) || !(64..=960).contains(max_side) {
                return Err(bad(START, "need an explicit frame and maxSide 64–960"));
            }
            let n = i128::from(*frame) * i128::from(TICKS_PER_SECOND) * i128::from(q.settings.frame_rate.den);
            let d = i128::from(q.settings.frame_rate.num);
            if n % d != 0 || n / d > i128::from(i64::MAX) || n / d >= i128::from(q.duration().0) {
                return Err(bad(START, "frame is outside this sequence or not exactly representable"));
            }
        }
        Request::Audio { start_sample, samples } => {
            let end = start_sample.checked_add(i64::try_from(*samples).map_err(|_| bad(START, "audio range overflow"))?);
            if *start_sample < 0
                || *samples == 0
                || *samples > 240_000
                || *samples > q.settings.sample_rate as usize * 5
                || end.is_none_or(|end| i128::from(end) * i128::from(TICKS_PER_SECOND) > i128::from(q.duration().0) * i128::from(q.settings.sample_rate))
            {
                return Err(bad(START, "audio range must fit the sequence, five seconds and 240000 samples"));
            }
        }
    }
    Ok(())
}

// The reference renderer bounds recursion by returning black/silence. For evidence, an
// unsupported/cyclic nest must be an explicit failure rather than a valid empty scene.
pub(super) fn validate_nests(
    project: &Project,
    id: ItemId,
    path: &mut Vec<ItemId>,
    seen: &mut std::collections::BTreeSet<(ItemId, usize)>,
    budget: &mut usize,
) -> Result<()> {
    if path.len() >= filmcraft_render::MAX_NEST_DEPTH as usize || path.contains(&id) {
        return Err(bad(START, "cyclic or overly deep sequence nest cannot be evidence"));
    }
    if !seen.insert((id, path.len())) {
        return Ok(());
    }
    let q = project.sequence(id).ok_or(crate::EngineError::NoSequence)?;
    q.settings.validate().map_err(|e| bad(START, e))?;
    path.push(id);
    for item in q.all_tracks().flat_map(|t| &t.items) {
        *budget = budget.checked_sub(1).ok_or_else(|| bad(START, "sequence dependency scan exceeds the 100000-item inspection limit"))?;
        if project.sequence(item.item).is_some() {
            validate_nests(project, item.item, path, seen, budget)?;
        }
    }
    path.pop();
    Ok(())
}

fn start(s: &mut Session, p: &Value) -> Result<Value> {
    if p.get("candidate").is_some_and(|v| v.as_str().is_none_or(|id| !token_ok(id))) {
        return Err(bad(START, "an explicit candidate must be a non-null valid candidate ID; no live Program fallback"));
    }
    let r: Start = parse(START, p)?;
    if !r.scope.valid() {
        return Err(bad(START, "exact closed project or sequence scope required"));
    }
    if matches!(&r.scope, super::organization_scope::Scope::Project(_))
        && (!matches!(&r.request, Request::SourceAudioLayout { .. }) || !r.background || r.candidate.is_some())
    {
        return Err(bad(START, "independent Root inspection is only background source audio layout; never Program, sequence audio or a candidate"));
    }
    if let Request::SourceAudioLayout { item, generation } = &r.request {
        if !r.background || r.candidate.is_some() {
            return Err(bad(START, "source audio layout requires a background original-source read, never a candidate"));
        }
        super::source_audio::validate(&s.project, Some(r.scope.project_id()), *item, generation)?;
    }
    if !token_ok(&r.job) {
        return Err(bad(START, "need a fresh random job ID"));
    }
    if s.agent_bridge.stopped_inspections.contains(&r.job) {
        return Ok(json!({"job":r.job,"status":"cancelled"}));
    }
    if let Some(j) = s.agent_bridge.inspections.iter().find(|j| j.id == r.job) {
        return if &j.request == p { status(s, j) } else { Err(bad(START, "job ID already describes a different read")) };
    }
    idle(s)?;
    let t = s.agent_bridge.tickets.iter().find(|t| t.token == r.token).ok_or_else(|| bad(START, "unknown/expired read ticket"))?;
    let library = match &r.scope {
        super::organization_scope::Scope::Sequence(scope) => {
            if t.revision != s.revision
                || t.path != s.path
                || s.state.active_sequence != t.sequence
                || !t.project.ptr_eq(&Arc::downgrade(&s.project))
                || !sequences::belongs(&s.project, t.sequence_id()?, scope)?
            {
                return Err(bad(START, "stale or foreign Program inspection scope"));
            }
            None
        }
        super::organization_scope::Scope::Project(scope) => {
            super::library_plane::validate_ticket(s, &r.token, scope, true)?;
            let Request::SourceAudioLayout { item, .. } = &r.request else { return Err(bad(START, "a Root source-layout request is required")) };
            if !t.source_library.as_ref().is_some_and(|ids| ids.contains(item)) {
                return Err(bad(START, "source layout item was not declared by this exact library read"));
            }
            Some(scope.clone())
        }
    };
    let sequence = if library.is_some() { t.sequence } else { Some(t.sequence_id()?) };
    let mut expected = vec![t.media.clone()];
    let (project, candidate_operation) = match &r.candidate {
        Some(id) if r.background => {
            let view = super::candidates::for_background_read(
                s,
                id,
                match &r.scope {
                    super::organization_scope::Scope::Sequence(scope) => scope,
                    _ => return Err(bad(START, "Root inspection cannot borrow a sequence candidate")),
                },
            )?;
            expected.push(view.media);
            (view.project, Some(view.operation))
        }
        Some(id) => (
            super::candidates::for_read(
                s,
                id,
                match &r.scope {
                    super::organization_scope::Scope::Sequence(scope) => scope,
                    _ => return Err(bad(START, "Root inspection cannot borrow a sequence candidate")),
                },
            )?,
            Some(super::candidates::operation(s, id)?),
        ),
        None => (s.project.clone(), None),
    };
    let bound = project.items.values().filter(|item| item.as_media().is_some()).any(|item| {
        item.metadata
            .get(BINDING_KEY)
            .and_then(|raw| serde_json::from_str::<Binding>(raw).ok())
            .is_some_and(|b| b.project_id == r.scope.project_id() && b.role == "original")
    });
    if !bound && !matches!(&r.request, Request::SourceAudioLayout { .. }) {
        return Err(bad(START, "workspace has no original sources bound to this Poema project"));
    }
    if r.background {
        for witness in &expected {
            media_guard::admissible(witness)?;
        }
    } else {
        super::media_guard::validate(&s.project, &s.media, &t.media)?;
    }
    validate(&project, sequence, &r.request)?;
    // Never evict a live worker to start another. Cancelled workers keep their global permit
    // until they actually exit, even if a decoder cannot be interrupted immediately.
    if s.agent_bridge.inspections.len() >= MAX_JOBS {
        let disposable = s.agent_bridge.inspections.iter().position(|j| !matches!(*j.state.lock().unwrap_or_else(|e| e.into_inner()), State::Running));
        if let Some(i) = disposable {
            s.agent_bridge.inspections.remove(i);
        } else {
            return Err(bad(START, "inspection cache is busy"));
        }
    }
    let permit = Worker::acquire()?;
    let state = Arc::new(Mutex::new(State::Running));
    let stop = Arc::new(AtomicBool::new(false));
    let candidate = r.candidate.as_ref().map(|id| (id.clone(), Arc::downgrade(&project)));
    let media = t.media.clone();
    let started = Instant::now();
    let mut bytes = expected.iter().fold(0u64, |bytes, witness| bytes.saturating_add(witness.source_bytes()));
    if let Request::SourceAudioLayout { item, .. } = &r.request {
        bytes = bytes.saturating_add(project.item(ItemId(*item)).and_then(|it| it.as_media()).and_then(|m| m.info.file_size).unwrap_or(0));
    }
    let budget = if r.background { Duration::from_millis(media_guard::source_deadline(bytes)) } else { DEADLINE };
    let verification = if r.background {
        Some(Arc::new(Verification {
            plan: Plan::for_witnesses(&s.project, &s.media, &expected)?,
            check: Check::for_sources(started, stop.clone(), bytes),
            phase: AtomicU8::new(0),
        }))
    } else {
        None
    };
    let provider = StrictSources {
        base: s.media.full_res_provider(project.clone(), s.services.clone()).recording(),
        project_id: r.scope.project_id().to_owned(),
        stop: stop.clone(),
        error: Arc::new(AtomicBool::new(false)),
        deadline: started,
        budget,
        witnesses: media.clone(),
    };
    let out = state.clone();
    let cancel = stop.clone();
    let worker_verification = verification.clone();
    let source_audio_proofs = Arc::new(Mutex::new(Vec::new()));
    let worker_proofs = source_audio_proofs.clone();
    let pool = s.media.clone();
    let services = s.services.clone();
    std::thread::Builder::new()
        .name("poema-program-read".into())
        .spawn(move || {
            let _permit = permit;
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                if let Some(v) = &worker_verification {
                    v.verify(&expected).map_err(|e| e.to_string())?;
                    v.phase.store(1, Ordering::Relaxed);
                }
                let image = match r.request {
                    Request::SourceAudioLayout { item, generation } => {
                        let v = worker_verification.as_ref().ok_or("source layout requires background verification")?;
                        let (view, proof) =
                            super::source_audio::read(&project, item, &generation, &pool, services.as_ref(), &v.check).map_err(|e| e.to_string())?;
                        worker_proofs.lock().unwrap_or_else(|e| e.into_inner()).push(proof);
                        view
                    }
                    other => render(&project, sequence.ok_or("Program inspection requires a real sequence")?, other, &provider)?,
                };
                if let Some(v) = &worker_verification {
                    v.phase.store(2, Ordering::Relaxed);
                    v.verify(&expected).map_err(|e| e.to_string())?;
                    v.phase.store(3, Ordering::Relaxed);
                }
                Ok(image)
            }))
            .unwrap_or_else(|_| Err("native evidence worker failed; no project edit was made".into()));
            let mut out = out.lock().unwrap_or_else(|e| e.into_inner());
            if cancel.load(Ordering::Relaxed) {
                *out = State::Cancelled;
            } else {
                *out = State::Done(result);
            }
        })
        .map_err(|_| bad(START, "could not start a native evidence worker"))?;
    let j = Job {
        id: r.job,
        request: p.clone(),
        project: Arc::downgrade(&s.project),
        path: s.path.clone(),
        revision: s.revision,
        sequence,
        library,
        state,
        stop,
        started,
        budget,
        candidate,
        candidate_operation,
        media,
        verification,
        source_audio_proofs,
    };
    let response = status(s, &j)?;
    s.agent_bridge.inspections.push_back(j);
    Ok(response)
}
fn poll(s: &mut Session, p: &Value) -> Result<Value> {
    let r: Poll = parse("agent.inspection", p)?;
    let j = s.agent_bridge.inspections.iter().find(|j| j.id == r.job).ok_or_else(|| bad(START, "unknown/expired inspection job"))?;
    status(s, j)
}
fn cancel(s: &mut Session, p: &Value) -> Result<Value> {
    let r: Poll = parse("agent.cancelInspection", p)?;
    if !token_ok(&r.job) {
        return Err(bad(START, "explicit owned inspection ID required"));
    }
    if let Some(j) = s.agent_bridge.inspections.iter().find(|j| j.id == r.job) {
        j.cancel_work();
        *j.state.lock().unwrap_or_else(|e| e.into_inner()) = State::Cancelled;
    }
    if !s.agent_bridge.stopped_inspections.contains(&r.job) {
        s.agent_bridge.stopped_inspections.push_back(r.job.clone());
    }
    while s.agent_bridge.stopped_inspections.len() > super::TICKET_LIMIT {
        s.agent_bridge.stopped_inspections.pop_front();
    }
    Ok(json!({"job": r.job, "status": "cancelled"}))
}

pub(super) fn discard_candidate(s: &mut Session, id: &str) {
    for j in &s.agent_bridge.inspections {
        if j.candidate.as_ref().is_some_and(|(candidate, _)| candidate == id) {
            j.cancel_work();
            *j.state.lock().unwrap_or_else(|e| e.into_inner()) = State::Cancelled;
        }
    }
}

// A playback fallback slate/silence is useful to humans, but cannot be evidence. Remember
// every source/decode failure even where the reference renderer intentionally skips it.
struct StrictSources {
    base: PoolProvider,
    project_id: String,
    stop: Arc<AtomicBool>,
    error: Arc<AtomicBool>,
    deadline: Instant,
    budget: Duration,
    witnesses: Arc<media_guard::Witnesses>,
}
impl SourceProvider for StrictSources {
    fn reuse_audio_state(&self) -> bool {
        false
    }
    fn source(&self, item: ItemId) -> Option<SharedSource> {
        let result = (|| {
            if self.stop.load(Ordering::Relaxed) || self.deadline.elapsed() >= self.budget {
                return None;
            }
            let (id, media, _) = self.base.project.resolve_media(item)?;
            if self.witnesses.is_scoped() && matches!(media.media, filmcraft_project::MediaRef::File { .. }) && self.witnesses.qualification(id.0) != "verified"
            {
                return None;
            }
            let raw = self.base.project.item(id)?.metadata.get(BINDING_KEY)?;
            let b: Binding = serde_json::from_str(raw).ok()?;
            if b.project_id != self.project_id || b.role != "original" || media.offline {
                return None;
            }
            Some(Arc::new(CheckedSource { inner: self.base.source(item)?, error: self.error.clone(), stop: self.stop.clone() }) as SharedSource)
        })();
        if result.is_none() {
            self.error.store(true, Ordering::Relaxed);
        }
        result
    }
}
struct CheckedSource {
    inner: SharedSource,
    error: Arc<AtomicBool>,
    stop: Arc<AtomicBool>,
}
impl CheckedSource {
    fn check<T>(&self, f: impl FnOnce() -> filmcraft_media::Result<T>) -> filmcraft_media::Result<T> {
        let result = if self.stop.load(Ordering::Relaxed) { Err(filmcraft_media::MediaError::Decode("inspection stopped".into())) } else { f() };
        if result.is_err() {
            self.error.store(true, Ordering::Relaxed);
        }
        result
    }
}
impl MediaSource for CheckedSource {
    fn info(&self) -> &MediaInfo {
        self.inner.info()
    }
    fn video_frame(&self, r: FrameRequest) -> filmcraft_media::Result<Arc<VideoFrame>> {
        self.check(|| self.inner.video_frame(r))
    }
    fn audio(&self, start: i64, n: usize, rate: u32) -> filmcraft_media::Result<AudioBuffer> {
        self.check(|| self.inner.audio(start, n, rate))
    }
    fn audio_stream(&self, stream: usize, start: i64, n: usize, rate: u32) -> filmcraft_media::Result<AudioBuffer> {
        self.check(|| self.inner.audio_stream(stream, start, n, rate))
    }
}
fn render(project: &Project, sequence: ItemId, request: Request, sources: &StrictSources) -> std::result::Result<Value, String> {
    let q = project.sequence(sequence).ok_or("missing sequence")?;
    let result = match request {
        Request::SourceAudioLayout { .. } => return Err("source layout cannot use the Program renderer".into()),
        Request::Frame { frame, max_side } => {
            let scale = (max_side as f32 / q.settings.width.max(q.settings.height) as f32).min(1.0);
            let (w, h) = filmcraft_render::output_size(q, scale);
            if w > 960 || h > 960 {
                return Err("render size exceeds evidence limit".into());
            }
            filmcraft_project::validate_frame_size(w as u32, h as u32)?;
            let time = q.settings.frame_rate.tick_of(frame);
            let opts = RenderOptions { scale, captions: true, ..Default::default() };
            let image = filmcraft_render::render_sequence(project, sequence, time, opts, sources);
            let png = filmcraft_export::encode_png(image.over_black_rgba8(), w as u32, h as u32).map_err(|e| e.to_string())?;
            json!({"kind":"frame","frame":frame,"ticks":time.0,"width":w,"height":h,"mime":"image/png",
                "data":filmcraft_project::gtemplate::base64_encode(&png),"color":"SDR display output over black","effects":true,"captions":true,"proxies":false})
        }
        Request::Audio { start_sample, samples } => {
            let audio = filmcraft_render::mixer::mix_graph(project, q, start_sample, samples, sources, None);
            let count = audio.channels.len();
            if ![2, 6].contains(&count) || audio.frames() != samples || audio.channels.iter().any(|ch| ch.len() != samples || ch.iter().any(|s| !s.is_finite()))
            {
                return Err("native mixer returned invalid or incomplete audio".into());
            }
            let rms: Vec<f64> = audio.channels.iter().map(|ch| (ch.iter().map(|s| f64::from(*s).powi(2)).sum::<f64>() / samples as f64).sqrt()).collect();
            let wav = filmcraft_export::write_wav(&audio.interleaved(), count as u16, audio.sample_rate, filmcraft_export::SampleFormat::Float32)?;
            json!({"kind":"audio","startSample":start_sample,"samples":samples,"sampleRate":audio.sample_rate,
                "channelOrder":if count==6 { vec!["L","R","C","LFE","Ls","Rs"] } else { vec!["L","R"] },
                "peaks":audio.peaks(),"rms":rms,"mime":"audio/wav","encoding":"pcm_f32le",
                "data":filmcraft_project::gtemplate::base64_encode(&wav),"proxies":false,"liveOverrides":false,
                "dspState":"independent random-access preroll; no playback cache or live meters"})
        }
    };
    if sources.error.load(Ordering::Relaxed)
        || !sources.base.offline_used().is_empty()
        || sources.deadline.elapsed() >= sources.budget
        || sources.stop.load(Ordering::Relaxed)
    {
        return Err("native evidence incomplete: source is unbound, offline, unreadable, stopped or timed out; no scene/silence inference is valid".into());
    }
    Ok(result)
}

pub(super) fn commands() -> Vec<CommandSpec> {
    let spec = |id, label, params, run| CommandSpec { id, label, menu: &[], shortcut: None, params, enabled: always, run, journal: false };
    vec![
        spec(
            START,
            "Inspect Native Evidence",
            r#"{"token":readToken,"job":randomId,"scope":sequenceScopeOrRootLibraryScope,"request":frameOrAudioOrSourceAudioLayout,"candidate"?:candidateId,"background"?:bool}"#,
            start,
        ),
        spec("agent.inspection", "Native Inspection Result", r#"{"job":randomId}"#, poll),
        spec("agent.cancelInspection", "Cancel Native Inspection", r#"{"job":randomId}"#, cancel),
    ]
}

#[cfg(test)]
mod tests;
