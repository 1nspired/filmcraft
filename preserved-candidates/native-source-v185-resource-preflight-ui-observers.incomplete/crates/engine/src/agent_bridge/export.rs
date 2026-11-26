//! Typed native export delivery. The existing human export builder, encoder and jobs own
//! settings/rendering. Files stay in an exclusive private stage until explicit publication.
//! A delivery folder is complete only when its durable completion manifest exists. No native
//! project/history mutation, raw command forwarding, overwrite, or unknown-job replay.
use std::collections::BTreeSet;
use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, atomic::Ordering};

use filmcraft_export::ExportSettings;
use filmcraft_project::{ItemId, Project};
use filmcraft_time::{TICKS_PER_SECOND, Tick, TimeRange};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use super::{
    check_edit_ticket_basis, inspection,
    media_guard::{self, Check, Plan, Witnesses},
    media_scope, parse, sequences, token_ok,
};
use crate::{
    Result, Session,
    commands::{CommandSpec, always, bad},
    export_tools,
};

const START: &str = "agent.startExport";
const PUBLISH: &str = "agent.publishExport";
const MAX_RECORDS: usize = 16;
const MAX_FILES: usize = 10_000;
const MANIFEST: &str = "poema-export.json";
fn error(message: impl Into<String>) -> crate::EngineError {
    bad(START, message)
}
fn io(error: std::io::Error) -> crate::EngineError {
    self::error(format!("native export I/O: {error}"))
}
fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum Range {
    Entire,
    InOut,
    WorkArea,
    Frames { start: i64, end: i64 },
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Options {
    token: String,
    scope: sequences::Scope,
    sequence: u64,
    name: String,
    range: Range,
    #[serde(default)]
    preset: Option<String>,
    #[serde(default = "empty_object")]
    settings: Value,
}
fn empty_object() -> Value {
    json!({})
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Start {
    token: String,
    options: Options,
    job: String,
    root: String,
    settings_sha256: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Target {
    job: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Publish {
    token: String,
    job: String,
    scope: sequences::Scope,
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct FileProof {
    name: String,
    bytes: u64,
    sha256: String,
}
fn valid_proof(p: &FileProof) -> bool {
    artifact_name(&p.name) && p.name != MANIFEST && p.bytes > 0 && p.sha256.len() == 64 && p.sha256.bytes().all(|c| c.is_ascii_hexdigit())
}

pub(super) struct Record {
    id: String,
    request: Value,
    scope: sequences::Scope,
    sequence: ItemId,
    revision: u64,
    project: Arc<Project>,
    path: Option<String>,
    media: Arc<Witnesses>,
    inputs: Plan,
    stage: PathBuf,
    destination: PathBuf,
    root: PathBuf,
    job: u64,
    progress: Arc<filmcraft_export::Progress>,
    intent: Value,
    settings: ExportSettings,
    publication_attempted: bool,
    publication_refused: bool,
    publication_token: Option<String>,
    receipt: Option<Value>,
}
impl Drop for Record {
    fn drop(&mut self) {
        self.progress.cancel.store(true, Ordering::Release);
    }
}
fn name_ok(name: &str) -> bool {
    name.len() <= 160 && artifact_name(name)
}
fn artifact_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 240
        && name.trim() == name
        && !name.starts_with('.')
        && !name.ends_with('.')
        && !name.chars().any(|c| c.is_control() || "/\\:*?\"<>|".contains(c))
}
fn root_path(root: &str) -> Result<PathBuf> {
    if root.len() > 4096 || root.chars().any(char::is_control) || !Path::new(root).is_absolute() {
        return Err(error("an explicit absolute export root is required"));
    }
    let root = PathBuf::from(root);
    if !std::fs::symlink_metadata(&root).map_err(io)?.is_dir() || std::fs::canonicalize(&root).map_err(io)? != root {
        return Err(error("export root must be an existing real directory without redirected ancestors"));
    }
    Ok(root)
}
fn small_json(path: &Path) -> Result<Value> {
    let bytes = regular_bytes(path, 8 * 1024 * 1024)?;
    serde_json::from_slice(&bytes).map_err(|e| error(format!("invalid export record: {e}")))
}
fn open_regular(path: &Path) -> Result<File> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let file = options.open(path).map_err(io)?;
    if !file.metadata().map_err(io)?.is_file() || !std::fs::symlink_metadata(path).map_err(io)?.is_file() {
        return Err(error("export input/output must be a regular file"));
    }
    Ok(file)
}
fn regular_bytes(path: &Path, max: u64) -> Result<Vec<u8>> {
    let mut file = open_regular(path)?;
    let before = file.metadata().map_err(io)?;
    let stamp = media_guard::stamp(path, &before).map_err(io)?;
    if before.len() > max {
        return Err(error("export record/resource exceeds its byte bound"));
    }
    let mut bytes = Vec::new();
    Read::by_ref(&mut file).take(max + 1).read_to_end(&mut bytes).map_err(io)?;
    if bytes.len() as u64 != before.len()
        || media_guard::stamp(path, &file.metadata().map_err(io)?).map_err(io)? != stamp
        || media_guard::stamp(path, &std::fs::symlink_metadata(path).map_err(io)?).map_err(io)? != stamp
    {
        return Err(error("export record/resource changed while reading"));
    }
    Ok(bytes)
}
fn write_new(path: &Path, bytes: &[u8]) -> Result<()> {
    let mut file = OpenOptions::new().write(true).create_new(true).open(path).map_err(io)?;
    file.write_all(bytes).map_err(io)?;
    file.sync_all().map_err(io)
}
fn write_json_new(path: &Path, value: &Value) -> Result<()> {
    write_new(path, &serde_json::to_vec(value).map_err(|e| error(e.to_string()))?)
}
fn sync_dir(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        File::open(path).and_then(|f| f.sync_all()).map_err(io)?;
    }
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}
fn resolve(s: &Session, o: &Options) -> Result<(ExportSettings, Value)> {
    check_edit_ticket_basis(s, &o.token)?;
    let sequence = ItemId(o.sequence);
    if !name_ok(&o.name)
        || !sequences::belongs(&s.project, sequence, &o.scope)?
        || !o.settings.is_object()
        || serde_json::to_vec(&o.settings).map_err(|e| error(e.to_string()))?.len() > 65_536
    {
        return Err(error("need a bounded delivery name, native settings object and explicit workspace sequence"));
    }
    for key in o.settings.as_object().into_iter().flat_map(|p| p.keys()) {
        if ["path", "range", "partOfBatch", "part_of_batch", "sink", "h264_pass", "signal"].contains(&key.as_str()) {
            return Err(error("paths, range and batch state belong to the typed delivery contract"));
        }
    }
    let params = json!({"preset":o.preset,"settings":o.settings});
    let (preset, mut settings) = export_tools::settings_from_params(s, &params, START)?;
    if settings.effects.image_overlay.enabled
        && preset.as_ref().is_none_or(|preset| settings.effects.image_overlay.path != preset.settings.effects.image_overlay.path)
    {
        return Err(error("export image resources must come from the selected configured native preset"));
    }
    let q = s.project.sequence(sequence).ok_or(crate::EngineError::NoSequence)?;
    inspection::validate_nests(&s.project, sequence, &mut Vec::new(), &mut BTreeSet::new(), &mut 100_000)?;
    let frame = |n: i64| -> Result<Tick> {
        let ticks = i128::from(n) * i128::from(TICKS_PER_SECOND) * i128::from(q.settings.frame_rate.den);
        let num = i128::from(q.settings.frame_rate.num);
        if n < 0 || ticks % num != 0 {
            return Err(error("range must use exact nonnegative sequence frames"));
        }
        Ok(Tick(i64::try_from(ticks / num).map_err(|_| error("range exceeds native tick bounds"))?))
    };
    settings.range = Some(match o.range {
        Range::Entire => TimeRange::new(Tick::ZERO, q.duration()),
        Range::Frames { start, end } => {
            if end <= start {
                return Err(error("range end must follow start"));
            }
            let a = frame(start)?;
            let b = frame(end)?;
            TimeRange::new(a, Tick(b.0 - a.0))
        }
        Range::InOut => {
            let mut full = settings.clone();
            full.range = None;
            filmcraft_export::export_range(&s.project, sequence, &full).map_err(|e| error(e.to_string()))?
        }
        Range::WorkArea => q.work_area.ok_or_else(|| error("this sequence has no work area"))?,
    });
    let range = settings.range.ok_or_else(|| error("missing export range"))?;
    filmcraft_export::validate_range(range).map_err(|e| error(e.to_string()))?;
    if range.end() > q.duration() {
        return Err(error("export range exceeds the current sequence"));
    }
    settings.path = format!("{}.{}", o.name, settings.extension());
    if settings.path == MANIFEST {
        return Err(error("delivery name collides with its completion manifest"));
    }
    settings.part_of_batch = false;
    settings.validate().map_err(|e| error(e.to_string()))?;
    if !filmcraft_export::available(settings.format) {
        return Err(error("the selected native encoder is unavailable"));
    }
    if settings.caption_sidecar.is_some() && q.caption_tracks.len() != 1 {
        return Err(error("sidecar export requires exactly one explicit caption track; no implicit first-track choice"));
    }
    let mut description = export_tools::describe_export(s, sequence, &settings, preset.as_ref().map(|p| p.name.as_str()))?;
    description["streams"] = json!({"video":settings.has_video(),"audio":settings.has_audio(),"imageSequence":settings.is_image_sequence()});
    if settings.is_image_sequence() && description["frames"].as_u64().is_none_or(|n| n == 0 || n > MAX_FILES as u64 - 1) {
        return Err(error("image delivery exceeds the bounded 9999-frame file set"));
    }
    let resolved = settings.resolve(q.settings.width, q.settings.height, q.settings.frame_rate, q.settings.sample_rate);
    if settings.has_audio() && i128::from(range.duration.0) * i128::from(resolved.sample_rate) < i128::from(TICKS_PER_SECOND) {
        return Err(error("export range contains no complete output audio sample"));
    }
    filmcraft_project::validate_frame_size(resolved.width, resolved.height).map_err(error)?;
    Ok((settings, description))
}
fn options(s: &mut Session, p: &Value) -> Result<Value> {
    let o: Options = parse("agent.resolveExport", p)?;
    let (_, description) = resolve(s, &o)?;
    let bytes = serde_json::to_vec(&description).map_err(|e| error(e.to_string()))?;
    Ok(json!({"status":"resolved","revision":s.revision,"sequence":o.sequence,"settingsSha256":digest(&bytes),"resolved":description,
        "publication":"new_directory_with_completion_manifest","existingFiles":"refuse","nativeUndo":false}))
}
fn catalog(s: &mut Session, p: &Value) -> Result<Value> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Catalog {
        token: String,
        scope: sequences::Scope,
        #[serde(default)]
        offset: usize,
    }
    let r: Catalog = parse("agent.exportOptions", p)?;
    check_edit_ticket_basis(s, &r.token)?;
    let sequence = s.state.active_sequence.ok_or(crate::EngineError::NoSequence)?;
    if !sequences::belongs(&s.project, sequence, &r.scope)? || r.offset > 10_000 {
        return Err(error("invalid export catalog scope/page"));
    }
    let presets = s.export_presets.all();
    let page: Vec<Value> = presets
        .iter()
        .skip(r.offset)
        .take(20)
        .map(|p| {
            json!({"name":p.name,"description":p.description,"settings":p.settings,
        "available":filmcraft_export::available(p.settings.format)})
        })
        .collect();
    let next = (r.offset + page.len() < presets.len()).then_some(r.offset + page.len());
    let formats = s.execute("export.formats", json!({}))?;
    Ok(json!({"status":"read","sequence":sequence.0,"revision":s.revision,"formats":formats,"presets":page,"nextOffset":next,
        "defaults":ExportSettings::default(),"rangeUnits":"exact_sequence_frames_half_open","nativeUndo":false,
        "destinationPolicy":"new delivery directory; refuse existing; completion manifest last"}))
}
pub(super) fn reconciling(s: &Session, id: &str, p: &Value) -> bool {
    let job = p.get("job").and_then(Value::as_str);
    s.agent_bridge.exports.iter().any(|r| Some(r.id.as_str()) == job && (id == START || id == PUBLISH && r.publication_attempted))
}
fn start(s: &mut Session, p: &Value) -> Result<Value> {
    let r: Start = parse(START, p)?;
    if r.token != r.options.token || !s.agent_bridge.operations.available() {
        return Err(error("export requires the same read token and an installed independent Stop door"));
    }
    if !token_ok(&r.job) {
        return Err(error("fresh random export job ID required"));
    }
    if s.agent_bridge.stopped_exports.contains(&r.job) {
        return Ok(json!({"job":r.job,"status":"cancelled","replayed":false}));
    }
    if let Some(old) = s.agent_bridge.exports.iter().find(|j| j.id == r.job) {
        if old.request != *p {
            return Err(error("export ID already belongs to a different request"));
        }
        return status(s, old);
    }
    if s.agent_bridge.exports.iter().filter(|r| !r.progress.finished.load(Ordering::Acquire)).count() >= 2 {
        return Err(error("two native exports are running; wait before starting another"));
    }
    if s.agent_bridge.exports.len() >= MAX_RECORDS {
        let disposable = s.agent_bridge.exports.iter().position(|r| {
            r.receipt.is_some()
                || status(s, r).is_ok_and(|v| v["status"] == "failed")
                || r.progress.finished.load(Ordering::Acquire) && r.progress.cancel.load(Ordering::Acquire)
        });
        if let Some(i) = disposable {
            s.agent_bridge.exports.remove(i);
        } else {
            return Err(error("retain/reconcile the pending export receipts before another export"));
        }
    }
    let (mut settings, description) = resolve(s, &r.options)?;
    if digest(&serde_json::to_vec(&description).map_err(|e| error(e.to_string()))?) != r.settings_sha256 {
        return Err(error("resolved export settings changed before start"));
    }
    let root = root_path(&r.root)?;
    let destination = root.join(&r.options.name);
    if destination.try_exists().map_err(io)? {
        return Err(error("delivery destination already exists; choose a new name"));
    }
    let t = s.agent_bridge.tickets.iter().find(|t| t.token == r.options.token).ok_or_else(|| error("export read ticket expired"))?;
    let sequence = ItemId(r.options.sequence);
    let dependencies = media_scope::dependencies(&s.project, [sequence])?;
    let media = Witnesses::retained_subset(&t.media, dependencies.clone()).ok_or_else(|| error("read must qualify every export original dependency"))?;
    media_guard::admissible(&media)?;
    let inputs = Plan::scoped(&s.project, &s.media, dependencies)?;
    let check = s.agent_bridge.operations.check().ok_or_else(|| error("native export requires an independent operation cancellation door"))?;
    inputs.recheck(&[&media], &check)?;
    let stage = root.join(format!(".poema-export-{}", r.job));
    std::fs::create_dir(&stage).map_err(io)?;
    let files = stage.join("files");
    std::fs::create_dir(&files).map_err(io)?;
    if settings.effects.image_overlay.enabled {
        let bytes = regular_bytes(Path::new(&settings.effects.image_overlay.path), 64 * 1024 * 1024)?;
        let resource = stage.join("overlay-resource");
        write_new(&resource, &bytes)?;
        settings.effects.image_overlay.path = resource.to_string_lossy().into_owned();
    }
    let intent = json!({"kind":"poema-native-export","version":1,"job":r.job,"scope":r.options.scope,"sequence":r.options.sequence,
        "revision":s.revision,"settingsSha256":r.settings_sha256,"resolved":description,"root":r.root,"name":r.options.name,
        "requestSha256":digest(&serde_json::to_vec(p).map_err(|e|error(e.to_string()))?)});
    write_json_new(&stage.join("intent.json"), &intent)?;
    sync_dir(&stage)?;
    sync_dir(&root)?;
    settings.path = files.join(Path::new(&settings.path).file_name().ok_or_else(|| error("missing native output name"))?).to_string_lossy().into_owned();
    inputs.recheck(&[&media], &check)?;
    let owned_settings = settings.clone();
    s.agent_bridge.operations.enter_publication(&r.options.token)?;
    let job = export_tools::spawn_export(s, s.project.clone(), sequence, settings, format!("Poema export {}", r.options.name), false)?;
    let progress = s.jobs.iter().find(|j| j.id == job).ok_or_else(|| error("native export job disappeared"))?.progress.clone();
    s.agent_bridge.exports.push_back(Record {
        id: r.job.clone(),
        request: p.clone(),
        scope: r.options.scope,
        sequence,
        revision: s.revision,
        project: s.project.clone(),
        path: s.path.clone(),
        media,
        inputs,
        stage,
        destination,
        root,
        job,
        progress,
        intent,
        settings: owned_settings,
        publication_attempted: false,
        publication_refused: false,
        publication_token: None,
        receipt: None,
    });
    let record = s.agent_bridge.exports.back().ok_or_else(|| error("native export record disappeared"))?;
    let mut reply = status(s, record)?;
    reply["started"] = json!(true);
    Ok(reply)
}
fn status(s: &Session, r: &Record) -> Result<Value> {
    if let Some(receipt) = &r.receipt {
        return Ok(json!({"job":r.id,"status":"delivered","receipt":receipt}));
    }
    let job = s.jobs.iter().find(|j| j.id == r.job).ok_or_else(|| error("native export job is no longer owned; do not replay it"))?;
    let native = export_tools::job_json(s, job);
    let status = if r.publication_refused {
        "failed"
    } else if r.publication_attempted {
        "publication_uncertain"
    } else if r.progress.cancel.load(Ordering::Acquire) {
        if native["finished"] == true { "cancelled" } else { "cancelling" }
    } else if native["finished"] == true {
        if native["result"]["error"].is_string() || native["offlineMedia"].as_array().is_none_or(|a| !a.is_empty()) { "failed" } else { "ready" }
    } else {
        "running"
    };
    let phase = r.publication_token.as_deref().map(|token| s.agent_bridge.operations.progress(token));
    Ok(json!({"job":r.id,"status":status,"sequence":r.sequence.0,"revision":r.revision,"nativeJob":native,
        "deliveryAttempted":r.publication_attempted,"publicationPhase":phase.map(|p|p["phase"].clone()),
        "settingsSha256":r.intent["settingsSha256"],"requestSha256":r.intent["requestSha256"],"nativeUndo":false,"delivered":false}))
}
fn poll(s: &mut Session, p: &Value) -> Result<Value> {
    let r: Target = parse("agent.exportStatus", p)?;
    if !token_ok(&r.job) {
        return Err(error("valid export job ID required"));
    }
    if let Some(record) = s.agent_bridge.exports.iter().find(|j| j.id == r.job) {
        return status(s, record);
    }
    Ok(json!({"job":r.job,"status":if s.agent_bridge.stopped_exports.contains(&r.job){"cancelled"}else{"unknown"},"replayed":false}))
}
fn cancel(s: &mut Session, p: &Value) -> Result<Value> {
    let r: Target = parse("agent.cancelExport", p)?;
    if !token_ok(&r.job) {
        return Err(error("valid export job ID required"));
    }
    if let Some(record) = s.agent_bridge.exports.iter().find(|j| j.id == r.job) {
        if record.publication_attempted {
            return status(s, record);
        }
        record.progress.cancel.store(true, Ordering::Release);
        return status(s, record);
    }
    if !s.agent_bridge.stopped_exports.contains(&r.job) {
        s.agent_bridge.stopped_exports.push_back(r.job.clone());
    }
    while s.agent_bridge.stopped_exports.len() > super::TICKET_LIMIT {
        s.agent_bridge.stopped_exports.pop_front();
    }
    Ok(json!({"job":r.job,"status":"cancelled","replayed":false}))
}
fn files(stage: &Path, check: &Check) -> Result<Vec<FileProof>> {
    let mut proofs = Vec::new();
    let mut buffer = vec![0u8; 128 * 1024];
    for entry in std::fs::read_dir(stage).map_err(io)? {
        check.check()?;
        let entry = entry.map_err(io)?;
        if proofs.len() >= MAX_FILES || !entry.file_type().map_err(io)?.is_file() {
            return Err(error("export file set exceeds its bound or includes a redirect/directory"));
        }
        let name = entry.file_name().into_string().map_err(|_| error("export filename is not UTF-8"))?;
        if name == MANIFEST || !artifact_name(&name) {
            return Err(error("unexpected export artifact name"));
        }
        let mut file = open_regular(&entry.path())?;
        let before = file.metadata().map_err(io)?;
        let stamp = media_guard::stamp(&entry.path(), &before).map_err(io)?;
        let mut hash = Sha256::new();
        let mut bytes = 0u64;
        loop {
            check.check()?;
            let n = file.read(&mut buffer).map_err(io)?;
            if n == 0 {
                break;
            }
            hash.update(&buffer[..n]);
            bytes = bytes.checked_add(n as u64).ok_or_else(|| error("file size overflow"))?;
        }
        let after = file.metadata().map_err(io)?;
        if bytes == 0
            || bytes != before.len()
            || media_guard::stamp(&entry.path(), &after).map_err(io)? != stamp
            || media_guard::stamp(&entry.path(), &std::fs::symlink_metadata(entry.path()).map_err(io)?).map_err(io)? != stamp
        {
            return Err(error("encoded output is empty or changed during verification"));
        }
        file.sync_all().map_err(io)?;
        proofs.push(FileProof { name, bytes, sha256: format!("{:x}", hash.finalize()) });
    }
    if proofs.is_empty() {
        return Err(error("native encoder produced no output files"));
    }
    proofs.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(proofs)
}
// Fresh encoded-file readers, separate from the encoder and source/provider cache. Samples
// establish readable output and declared geometry/layout/duration, not whole-film semantics.
fn decode_output(s: &Session, record: &Record, proofs: &[FileProof], check: &Check) -> Result<Value> {
    let settings = &record.settings;
    let q = record.project.sequence(record.sequence).ok_or(crate::EngineError::NoSequence)?;
    let resolved = settings.resolve(q.settings.width, q.settings.height, q.settings.frame_rate, q.settings.sample_rate);
    let range = settings.range.ok_or_else(|| error("missing immutable output range"))?;
    let (first, last) = filmcraft_export::frame_span(resolved.rate, range);
    let count = u64::try_from(last - first).map_err(|_| error("invalid output frame count"))?;
    let native = s.jobs.iter().find(|j| j.id == record.job).ok_or_else(|| error("native encoder report disappeared"))?;
    let report = native.result.lock().unwrap_or_else(|e| e.into_inner()).clone().ok_or_else(|| error("native encoder is still running"))?.map_err(error)?;
    let mut expected = BTreeSet::new();
    let main = Path::new(&settings.path);
    let filename = |path: &Path| -> Result<String> {
        if path.parent() != Some(record.stage.join("files").as_path()) {
            return Err(error("encoder reported a file outside its private output set"));
        }
        Ok(path.file_name().and_then(|s| s.to_str()).ok_or_else(|| error("invalid encoder filename"))?.to_string())
    };
    if settings.is_image_sequence() {
        for i in 0..count {
            expected.insert(filename(Path::new(&filmcraft_export::image_sequence_path(&settings.path, i, count)))?);
        }
    } else {
        expected.insert(filename(main)?);
    }
    for path in &report.extra_files {
        expected.insert(filename(Path::new(path))?);
    }
    if let Some(kind) = &settings.caption_sidecar {
        expected.insert(filename(&main.with_extension(kind))?);
    }
    if expected != proofs.iter().map(|p| p.name.clone()).collect() {
        return Err(error("encoded file set does not match the native format/sidecar contract"));
    }
    if settings.has_video() && report.frames != count {
        return Err(error("native encoder frame accounting differs from the resolved range"));
    }
    let audio_padding = if !settings.has_audio() {
        0
    } else if settings.audio_codec() == filmcraft_export::AudioCodec::Aac {
        2048
    } else {
        1
    };
    let duration_tolerance =
        resolved.rate.frame_duration().0.saturating_add(TICKS_PER_SECOND.saturating_mul(audio_padding) / i64::from(resolved.sample_rate.max(1)));
    let mut video_samples = Vec::new();
    let mut audio_samples = Vec::new();
    let indexes = [0, count.saturating_sub(1) / 2, count.saturating_sub(1)].into_iter().collect::<BTreeSet<_>>();
    let mut paths = if settings.is_image_sequence() {
        indexes.iter().map(|i| PathBuf::from(filmcraft_export::image_sequence_path(&settings.path, *i, count))).collect::<Vec<_>>()
    } else {
        vec![PathBuf::from(&settings.path)]
    };
    paths.extend(report.extra_files.iter().map(PathBuf::from));
    let atom = settings.format == filmcraft_export::Format::MxfOpAtom;
    for (path_index, path) in paths.iter().enumerate() {
        check.check()?;
        let source = s.media.probe_file(&path.to_string_lossy(), s.services.as_ref()).map_err(|e| error(format!("encoded output cannot be opened: {e}")))?;
        let info = source.info();
        let picture = settings.has_video() && (!atom || path_index == 0);
        let audio = settings.has_audio() && (!atom || path_index > 0);
        if info.has_video() != picture || info.has_audio() != audio {
            return Err(error("encoded video/audio streams differ from the native resolved settings"));
        }
        if !settings.is_image_sequence() && info.duration.0.abs_diff(range.duration.0) > duration_tolerance as u64 {
            return Err(error("encoded duration differs from the requested range beyond frame/codec padding"));
        }
        if picture {
            let video = info.video.as_ref().ok_or_else(|| error("encoded video stream is missing"))?;
            if video.width != resolved.width || video.height != resolved.height {
                return Err(error("encoded frame dimensions differ from the resolved export"));
            }
            for index in if settings.is_image_sequence() { BTreeSet::from([0]) } else { indexes.clone() } {
                check.check()?;
                let time = resolved.rate.tick_of(i64::try_from(index).map_err(|_| error("encoded frame index overflow"))?);
                let frame = source.video_frame(filmcraft_media::FrameRequest::full(time)).map_err(|e| error(format!("encoded picture decode failed: {e}")))?;
                if frame.width != resolved.width
                    || frame.height != resolved.height
                    || matches!(&frame.data,filmcraft_frame::PixelData::RgbaF32(p) if p.iter().any(|x|!x.is_finite()))
                {
                    return Err(error("encoded frame is malformed"));
                }
                video_samples.push(json!({"file":filename(path)?,"frame":index,"rgbaSha256":digest(&frame.to_rgba8())}));
            }
        }
        if audio {
            let audio = info.audio().ok_or_else(|| error("encoded audio stream is missing"))?;
            let channels = if atom { 1 } else { resolved.channels };
            if audio.sample_rate != resolved.sample_rate || audio.channels != channels {
                return Err(error("encoded audio rate/channel layout differs from resolved export"));
            }
            let total = i128::from(range.duration.0) * i128::from(resolved.sample_rate) / i128::from(TICKS_PER_SECOND);
            let samples = usize::try_from(total.clamp(1, 4096)).map_err(|_| error("encoded audio sample bound"))?;
            let end = i64::try_from(total.saturating_sub(samples as i128).max(0)).map_err(|_| error("encoded audio duration overflow"))?;
            for start in BTreeSet::from([0, end / 2, end]) {
                check.check()?;
                let buffer = source.audio(start, samples, resolved.sample_rate).map_err(|e| error(format!("encoded sound decode failed: {e}")))?;
                if buffer.sample_rate != resolved.sample_rate
                    || buffer.channels.len() != channels as usize
                    || buffer.channels.iter().any(|c| c.len() != samples || c.iter().any(|x| !x.is_finite()))
                {
                    return Err(error("encoded audio samples/layout are incomplete or non-finite"));
                }
                let peaks: Vec<f32> = buffer.channels.iter().map(|c| c.iter().fold(0.0f32, |peak, s| peak.max(s.abs()))).collect();
                audio_samples.push(json!({"file":filename(path)?,"startSample":start,"samples":samples,"peaks":peaks}));
            }
        }
    }
    check.check()?;
    Ok(json!({"kind":"native_fresh_encoded_file_decode","videoSamples":video_samples,"audioSamples":audio_samples,
        "durationToleranceTicks":duration_tolerance,"wholeProgramSemanticVerification":false}))
}
pub(super) fn publication_ticket(s: &Session, p: &Value) -> bool {
    s.agent_bridge.exports.iter().any(|r| {
        p["job"] == r.id
            && r.publication_token.as_deref() == p["token"].as_str()
            && serde_json::to_value(&r.scope).is_ok_and(|scope| p["scope"] == scope)
            && !r.publication_attempted
    })
}
fn prepare_delivery(s: &mut Session, p: &Value) -> Result<Value> {
    let r: Publish = parse("agent.prepareExportDelivery", p)?;
    if !token_ok(&r.token) || !s.agent_bridge.operations.available() {
        return Err(error("fresh export publication token and independent Stop required"));
    }
    let index = s.agent_bridge.exports.iter().position(|j| j.id == r.job).ok_or_else(|| error("unknown export; encoding was not repeated"))?;
    let record = s.agent_bridge.exports.get(index).ok_or_else(|| error("export disappeared"))?;
    if record.scope != r.scope || record.publication_attempted || status(s, record)?["status"] != "ready" {
        return Err(error("only this owned ready export can prepare delivery"));
    }
    if let Some(previous) = record.publication_token.as_deref() {
        // Close an earlier unused/cancelled operation door before replacing it. The immutable
        // job still owns its captured cut; these metadata atomics renew no file witnesses.
        let phase = s.agent_bridge.operations.progress(previous);
        if matches!(phase["phase"].as_str(), Some("verifying_inputs" | "publishing" | "committed")) {
            return Err(error("export publication is active or already attempted"));
        }
        let _ = s.agent_bridge.operations.cancel(&json!({"token":previous}))?;
    }
    s.agent_bridge.operations.register(&r.token)?;
    let record = s.agent_bridge.exports.get_mut(index).ok_or_else(|| error("export disappeared"))?;
    record.publication_token = Some(r.token.clone());
    Ok(json!({"kind":"native_export_publication_basis","version":1,"token":r.token,"job":record.id,"scope":record.scope,
        "sequence":record.sequence.0,"revision":record.revision,"settingsSha256":record.intent["settingsSha256"],"requestSha256":record.intent["requestSha256"],
        "requiresSourceValidation":true,"capabilities":{"nativeExportDelivery":true,"sourceFileGuard":true,"operationCancellation":true,"guardedOperationDeadlineMs":45000}}))
}
fn publish(s: &mut Session, p: &Value) -> Result<Value> {
    let r: Publish = parse(PUBLISH, p)?;
    let i = s.agent_bridge.exports.iter().position(|j| j.id == r.job).ok_or_else(|| error("unknown export; reconcile without restarting encoding"))?;
    let record = s.agent_bridge.exports.get(i).ok_or_else(|| error("export disappeared"))?;
    if record.scope != r.scope {
        return Err(error("foreign export scope"));
    }
    if record.publication_attempted {
        return status(s, record);
    }
    if record.publication_token.as_deref() != Some(r.token.as_str()) || status(s, record)?["status"] != "ready" {
        return Err(error("export is not ready or lacks its own publication door; nothing was delivered"));
    }
    if record.inputs.rebuild(&record.project, &s.media)? != record.inputs {
        return Err(error("export original resolution changed"));
    }
    let check = s.agent_bridge.operations.check().ok_or_else(|| error("delivery requires independent Stop"))?;
    record.inputs.recheck(&[&record.media], &check)?;
    let files_path = record.stage.join("files");
    let proofs = files(&files_path, &check)?;
    if root_path(&record.stage.to_string_lossy())? != record.stage || root_path(&files_path.to_string_lossy())? != files_path {
        return Err(error("private export stage was redirected"));
    }
    let decode = decode_output(s, record, &proofs, &check)?;
    // Decoding must not renew or replace the exact files whose hashes are delivered.
    if serde_json::to_value(files(&files_path, &check)?).map_err(|e| error(e.to_string()))?
        != serde_json::to_value(&proofs).map_err(|e| error(e.to_string()))?
    {
        return Err(error("encoded files changed while their picture/sound were verified"));
    }
    record.inputs.recheck(&[&record.media], &check)?;
    if root_path(&record.root.to_string_lossy())? != record.root {
        return Err(error("export root changed"));
    }
    let destination = record.destination.clone();
    let stage = record.stage.clone();
    let receipt = json!({"kind":"poema-native-export-delivery","version":1,"job":record.id,"scope":record.scope,
        "sequence":record.sequence.0,"revision":record.revision,"currentRevisionAtDelivery":s.revision,
        "cutChangedAfterRequest":record.path!=s.path||s.revision!=record.revision||!Arc::ptr_eq(&record.project,&s.project),"settingsSha256":record.intent["settingsSha256"],
        "requestSha256":record.intent["requestSha256"],"resolved":record.intent["resolved"],"directory":destination,
        "files":proofs,"sourceQualified":true,"offlinePlaceholders":false,"nativeUndo":false,
        "encodedBytesVerified":true,"nativeDecodeVerified":true,"decode":decode,"independentlyDecoded":false,"complete":true});
    // Prepare and fsync the marker before entering the short publication section. A hard link
    // installs it last with atomic no-replace semantics, after every member is durable.
    let completion = stage.join("completion.json");
    if completion.try_exists().map_err(io)? {
        if small_json(&completion)? != receipt {
            return Err(error("prepared delivery marker differs; reconcile without replacing it"));
        }
    } else {
        write_json_new(&completion, &receipt)?;
        sync_dir(&files_path)?;
        sync_dir(&stage)?;
    }
    s.agent_bridge.operations.enter_publication(&r.token)?;
    s.agent_bridge.exports.get_mut(i).ok_or_else(|| error("export disappeared"))?.publication_attempted = true;
    if let Err(failure) = std::fs::create_dir(&destination) {
        s.agent_bridge.exports.get_mut(i).ok_or_else(|| error("export disappeared"))?.publication_refused = true;
        let record = s.agent_bridge.exports.get(i).ok_or_else(|| error("export disappeared"))?;
        let mut result = status(s, record)?;
        result["deliveryError"] = json!(format!("The delivery directory could not be created; existing files were preserved: {failure}"));
        return Ok(result);
    }
    for proof in &proofs {
        std::fs::hard_link(files_path.join(&proof.name), destination.join(&proof.name)).map_err(io)?;
    }
    sync_dir(&destination)?;
    std::fs::hard_link(stage.join("completion.json"), destination.join(MANIFEST)).map_err(io)?;
    sync_dir(&destination)?;
    if let Some(parent) = destination.parent() {
        sync_dir(parent)?;
    }
    s.agent_bridge.exports.get_mut(i).ok_or_else(|| error("export disappeared"))?.receipt = Some(receipt.clone());
    Ok(json!({"job":r.job,"status":"delivered","receipt":receipt}))
}
fn receipt(s: &mut Session, p: &Value) -> Result<Value> {
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase", deny_unknown_fields)]
    struct Read {
        job: String,
        root: String,
        name: String,
        scope: sequences::Scope,
        sequence: u64,
        revision: u64,
        settings_sha256: String,
        #[serde(default)]
        request_sha256: Option<String>,
    }
    let r: Read = parse("agent.exportReceipt", p)?;
    let hash = |s: &str| s.len() == 64 && s.bytes().all(|c| c.is_ascii_hexdigit());
    if !token_ok(&r.job) || !name_ok(&r.name) || !hash(&r.settings_sha256) || r.request_sha256.as_deref().is_some_and(|h| !hash(h)) {
        return Err(error("bounded retained export identity required"));
    }
    let root = root_path(&r.root)?;
    let intent = root.join(format!(".poema-export-{}", r.job)).join("intent.json");
    if !intent.try_exists().map_err(io)? {
        return Ok(json!({"job":r.job,"status":"unknown","replayed":false}));
    }
    let intent = small_json(&intent)?;
    if intent["job"] != r.job
        || intent["scope"] != serde_json::to_value(&r.scope).map_err(|e| error(e.to_string()))?
        || r.request_sha256.as_deref().is_some_and(|h| intent["requestSha256"] != h)
        || intent["settingsSha256"] != r.settings_sha256
        || intent["sequence"] != r.sequence
        || intent["revision"] != r.revision
        || intent["name"] != r.name
        || intent["root"] != r.root
    {
        return Err(error("retained export identity does not match intent"));
    }
    let path = root.join(&r.name).join(MANIFEST);
    if !path.try_exists().map_err(io)? {
        return Ok(json!({"job":r.job,"status":"unowned","mayBeRunning":true,"replayed":false,"intent":intent}));
    }
    let receipt = small_json(&path)?;
    if receipt["kind"] != "poema-native-export-delivery"
        || receipt["version"] != 1
        || receipt["job"] != r.job
        || receipt["scope"] != intent["scope"]
        || receipt["requestSha256"] != intent["requestSha256"]
        || receipt["settingsSha256"] != intent["settingsSha256"]
        || receipt["complete"] != true
        || receipt["directory"] != json!(root.join(&r.name))
        || receipt["nativeDecodeVerified"] != true
        || receipt["encodedBytesVerified"] != true
        || receipt["files"].as_array().is_none_or(|files| {
            files.is_empty() || files.len() > MAX_FILES || files.iter().any(|p| !serde_json::from_value::<FileProof>(p.clone()).is_ok_and(|f| valid_proof(&f)))
        })
    {
        return Err(error("completion manifest does not match this export"));
    }
    let _ = s;
    Ok(json!({"job":r.job,"status":"delivered","historical":true,"currentFileBytesVerified":false,"receipt":receipt}))
}
pub(super) fn commands() -> Vec<CommandSpec> {
    let spec = |id, label, params, run| CommandSpec { id, label, menu: &[], shortcut: None, params, enabled: always, run, journal: false };
    vec![
        spec("agent.exportOptions", "Read Native Export Options", "{token,scope,offset?}", catalog),
        spec("agent.resolveExport", "Resolve Native Export", "{token,scope,sequence,name,range,preset?,settings?}", options),
        spec(START, "Start Native Export", "{token,options,job,root,settingsSha256}", start),
        spec("agent.exportStatus", "Read Native Export Status", "{job}", poll),
        spec("agent.cancelExport", "Cancel Native Export", "{job}", cancel),
        spec("agent.prepareExportDelivery", "Prepare Captured Export Delivery", "{token,job,scope}", prepare_delivery),
        spec(PUBLISH, "Publish Native Export", "{token,job,scope}", publish),
        spec("agent.exportReceipt", "Reconcile Native Export Delivery", "{job,root,name,scope,sequence,revision,settingsSha256,requestSha256?}", receipt),
    ]
}
