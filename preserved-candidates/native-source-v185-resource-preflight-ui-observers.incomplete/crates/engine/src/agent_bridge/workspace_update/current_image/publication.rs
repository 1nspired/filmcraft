//! Current-image publication retains the shared original file writer and real
//! manager controls. JSON is an intent, never a replacement native owner.
use super::*;
use crate::agent_bridge::save_operation::managed_dispatch::Control;
use filmcraft_format::retained::{preview_context::check_owner_process, private_record::Record};
pub(super) const COMMAND: &str = "agent.pausedProjectRecoveryRecord";
const META: u64 = 128 * 1024;
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Request {
    token: String,
    scope: crate::project_workspace::ProjectScope,
    workspace: String,
    update: String,
    project_path: String,
    activation_token: String,
    native_session: String,
    capture: String,
    action: String,
    manifest_path: PathBuf,
    reserve_bytes: u64,
}
struct Manager {
    manifest: Control,
    lease: Control,
    pid: u32,
    path: PathBuf,
}
fn fail(e: impl std::fmt::Display) -> crate::EngineError {
    bad(COMMAND, e.to_string())
}
impl Manager {
    fn capture(s: &Session, c: &Context, path: &Path, check: &media_guard::Check) -> Result<Self> {
        let manifest = Control::capture(path, META, check).map_err(fail)?;
        let value = manifest.value();
        if value["kind"] != "poema-saved-native-workspace"
            || value["id"] != c.workspace
            || value["projectId"] != c.scope.project_id
            || value["projectPath"] != c.project_path
        {
            return Err(fail("actual current Root manager controls required"));
        }
        let path_text = path.to_str().ok_or_else(|| fail("literal manager path required"))?;
        let lease = Control::capture(Path::new(&format!("{path_text}.lock")), 4096, check).map_err(fail)?;
        let value = lease.value();
        if value.as_object().is_none_or(|v| v.len() != 2 || !v.contains_key("pid") || !v.contains_key("identity"))
            || !value["identity"].as_str().is_some_and(uuid)
        {
            return Err(fail("actual original manager lease required"));
        }
        let pid = value["pid"]
            .as_u64()
            .and_then(|v| u32::try_from(v).ok())
            .filter(|v| *v > 0 && *v != std::process::id())
            .ok_or_else(|| fail("live separate original manager process required"))?;
        if s.path.as_deref() != Some(c.project_path.as_str()) {
            return Err(fail("native Root path changed"));
        }
        let manager = Self { manifest, lease, pid, path: path.into() };
        manager.current(check)?;
        Ok(manager)
    }
    fn current(&self, check: &media_guard::Check) -> Result<()> {
        self.manifest.current(check).map_err(fail)?;
        self.lease.current(check).map_err(fail)?;
        check_owner_process(self.pid).map_err(fail)?;
        self.manifest.fence(check).map_err(fail)?;
        self.lease.fence(check).map_err(fail)?;
        check.check()
    }
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::agent_bridge::workspace_update) struct Retirement {
    kind: &'static str,
    version: u8,
    status: &'static str,
    token: String,
    scope: crate::project_workspace::ProjectScope,
    workspace: String,
    update: String,
    project_path: String,
    activation_token: String,
    native_session: String,
    native_pid: u32,
    revision: u64,
    capture: String,
    frame_sha256: String,
    frame_bytes: usize,
    manifest_path: PathBuf,
    reserve_bytes: u64,
    intent_path: PathBuf,
    publication_path: PathBuf,
    editing_paused: bool,
    workers_may_resume: bool,
    project_unchanged: bool,
    history_unchanged: bool,
    write_authority: bool,
    startup_authority: bool,
    source_validation: &'static str,
    media_evidence: bool,
}
impl Retirement {
    pub(in crate::agent_bridge::workspace_update) fn value(&self) -> Value {
        json!(self)
    }
}

pub(super) struct Publication {
    manager: Manager,
    record: Record,
    intent_path: PathBuf,
    reserve: u64,
    journal: Option<Control>,
}
fn request(s: &Session, p: &Value) -> Result<(Request, Arc<Image>)> {
    let r: Request = parse(COMMAND, p)?;
    let h = s
        .agent_bridge
        .workspace_update
        .as_ref()
        .filter(|h| {
            h.candidate
                && !h.candidate_cancelled
                && !h.retiring
                && !s.view_only
                && s.worker_available
                && operation_cancellation_available(s)
                && s.agent_bridge.epoch.as_deref() == Some(h.native_session.as_str())
        })
        .ok_or_else(|| fail("actual original paused native candidate required"))?;
    let image = h.current_image.clone().ok_or_else(|| fail("actual retained current image required; no metadata import or implicit recapture"))?;
    let c = &image.context;
    if !token_ok(&r.token)
        || r.token == c.activation_token
        || !matches!(r.action.as_str(), "prepare" | "publish" | "read" | "retire")
        || r.scope != c.scope
        || r.workspace != c.workspace
        || r.update != c.update
        || r.project_path != c.project_path
        || r.activation_token != c.activation_token
        || r.native_session != c.native_session
        || r.capture != image.capture
        || s.path.as_deref() != Some(c.project_path.as_str())
        || !r.manifest_path.is_absolute()
        || !r.manifest_path.components().all(|c| matches!(c, std::path::Component::RootDir | std::path::Component::Normal(_)))
        || r.manifest_path.to_str().is_none_or(|v| v.len() > 16384 || v.chars().any(char::is_control))
        || r.reserve_bytes == 0
        || r.reserve_bytes > 9_007_199_254_740_991
        || history_checkpoint::has_basis(s, &r.token)
        || s.agent_bridge.tickets.iter().any(|t| t.token == r.token)
    {
        return Err(fail("exact original current-image purpose and explicit original manager/reserve required"));
    }
    Ok((r, image))
}
pub(super) fn prepare(s: &mut Session, p: &Value) -> Result<()> {
    let (r, _) = request(s, p)?;
    s.agent_bridge.operations.register(&r.token)
}
impl Publication {
    fn prepare(s: &Session, image: &Image, r: &Request, check: &media_guard::Check) -> Result<Self> {
        image.current(s, check)?;
        let manager = Manager::capture(s, &image.context, &r.manifest_path, check)?;
        let parent = Path::new(&image.context.project_path).parent().ok_or_else(|| fail("actual project parent required"))?;
        let target = parent.join(format!(".poema-project-recovery-{}-{}.json", image.context.workspace, image.capture));
        let manager_parent = manager.path.parent().ok_or_else(|| fail("manager parent required"))?;
        let intent_path = manager_parent.join(format!("workspace-state-{}", image.context.workspace)).join(format!("paused-recovery-{}.json", image.capture));
        if target.starts_with(&image.context.data_directory)
            || intent_path.starts_with(&image.context.data_directory)
            || image.files.source.covers(&target)
            || image.files.source.covers(&intent_path)
        {
            return Err(fail("publication names must be outside the complete retained resource namespace"));
        }
        let io = || {
            check.check().map_err(|e| io::Error::other(e.to_string()))?;
            manager.current(check).map_err(|e| io::Error::other(e.to_string()))
        };
        let directory = DirectoryFrame::capture(parent, &io).map_err(fail)?;
        let name = target.file_name().and_then(|n| n.to_str()).ok_or_else(|| fail("literal private name required"))?;
        let frame = directory.node(name, &io).map_err(fail)?;
        let raw = encode(&image.context, &image.history, &image.editor, &image.files, check)?;
        if raw.len() != image.bytes || digest(&raw) != image.sha256 {
            return Err(fail("complete original current-image bytes differ"));
        }
        let record = Record::prepare(&frame, &image.capture, &raw, r.reserve_bytes, &io).map_err(fail)?;
        image.current_against(s, &raw, check)?;
        manager.current(check)?;
        Ok(Self { manager, record, intent_path, reserve: r.reserve_bytes, journal: None })
    }
    fn matches(&self, r: &Request) -> Result<()> {
        if self.manager.path != r.manifest_path || self.reserve != r.reserve_bytes {
            return Err(fail("original manager or reserve cannot be replaced"));
        }
        Ok(())
    }
    fn intent(&self, image: &Image) -> Value {
        json!({"kind":"poema-paused-recovery-publication-intent","version":1,"managerPid":self.manager.pid,
            "manifestPath":self.manager.path,"capture":image.capture,"nativePid":std::process::id(),"nativeSession":image.context.native_session,
            "scope":image.context.scope,"update":image.context.update,"projectPath":image.context.project_path,
            "frameSha256":image.sha256,"intent":self.record.intent()})
    }
    fn publish(&mut self, s: &Session, image: &Image, r: &Request, check: &media_guard::Check) -> Result<()> {
        self.matches(r)?;
        image.current(s, check)?;
        self.manager.current(check)?;
        if self.journal.is_none() {
            let journal = Control::capture(&self.intent_path, META, check).map_err(fail)?;
            if journal.value() != &self.intent(image) {
                return Err(fail("complete durably bound original publication intent required"));
            }
            self.journal = Some(journal);
        }
        let journal = self.journal.as_ref().ok_or_else(|| fail("original publication intent generation unavailable"))?;
        journal.current(check).map_err(fail)?;
        let raw = encode(&image.context, &image.history, &image.editor, &image.files, check)?;
        let io = || {
            check.check().map_err(|e| io::Error::other(e.to_string()))?;
            self.manager.current(check).map_err(|e| io::Error::other(e.to_string()))?;
            journal.current(check).map_err(|e| io::Error::other(e.to_string()))
        };
        let before_link = || {
            io()?;
            image.current_against(s, &raw, check).map_err(|e| io::Error::other(e.to_string()))?;
            io()
        };
        // Once I/O may begin, Stop cannot fabricate rollback or permit replay.
        s.agent_bridge.operations.enter_publication(&r.token)?;
        self.record.publish(&raw, &io, &before_link).map_err(fail)?;
        image.current_against(s, &raw, check)?;
        self.manager.current(check)?;
        journal.current(check).map_err(fail)
    }

    fn retirement(&self, s: &Session, image: &Image, r: &Request, check: &media_guard::Check) -> Result<Retirement> {
        self.matches(r)?;
        image.current(s, check)?;
        self.manager.current(check)?;
        let journal = self.journal.as_ref().ok_or_else(|| fail("the original published intent generation is not retained"))?;
        if journal.value() != &self.intent(image) {
            return Err(fail("the original publication intent differs"));
        }
        let io = || {
            check.check().map_err(|e| io::Error::other(e.to_string()))?;
            self.manager.current(check).map_err(|e| io::Error::other(e.to_string()))?;
            journal.current(check).map_err(|e| io::Error::other(e.to_string()))?;
            journal.fence(check).map_err(|e| io::Error::other(e.to_string()))
        };
        io().map_err(fail)?;
        let directory = DirectoryFrame::capture(&self.record.intent().parent.path, &io).map_err(fail)?;
        let retained = directory.record();
        if !retained.missing.is_empty() || retained.anchor != self.record.intent().parent {
            return Err(fail("the original owned publication parent changed before retirement"));
        }
        let space = directory.space(&io).map_err(fail)?;
        if space.read_only || space.device != self.record.intent().parent.identity.device || space.free < self.reserve.saturating_add(MAX_BODY as u64) {
            return Err(fail("the original publication volume or working reserve changed"));
        }
        let observation = self.record.inspect(&io).map_err(fail)?;
        if !observation.matches_published
            || !observation.receipt.directory_synced
            || !observation.receipt.call_attempted
            || observation.output.as_ref().is_none_or(|v| v.sha256 != image.sha256 || v.bytes != image.bytes as u64)
        {
            return Err(fail("only this same fully published native Record can retire its paused producer"));
        }
        let raw = encode(&image.context, &image.history, &image.editor, &image.files, check)?;
        image.current_against(s, &raw, check)?;
        io().map_err(fail)?;
        let c = &image.context;
        Ok(Retirement {
            kind: "native_paused_current_retirement",
            version: 1,
            status: "retiring",
            token: r.token.clone(),
            scope: c.scope.clone(),
            workspace: c.workspace.clone(),
            update: c.update.clone(),
            project_path: c.project_path.clone(),
            activation_token: c.activation_token.clone(),
            native_session: c.native_session.clone(),
            native_pid: std::process::id(),
            revision: c.revision,
            capture: image.capture.clone(),
            frame_sha256: image.sha256.clone(),
            frame_bytes: image.bytes,
            manifest_path: self.manager.path.clone(),
            reserve_bytes: self.reserve,
            intent_path: self.intent_path.clone(),
            publication_path: self.record.intent().destination.clone(),
            editing_paused: true,
            workers_may_resume: false,
            project_unchanged: true,
            history_unchanged: true,
            write_authority: false,
            startup_authority: false,
            source_validation: "not-performed",
            media_evidence: false,
        })
    }

    fn receipt(&self, image: &Image, r: &Request, check: &media_guard::Check) -> Result<Value> {
        let record = self.record.inspect(&|| check.check().map_err(|e| io::Error::other(e.to_string()))).map_err(fail)?;
        Ok(json!({"status":"recorded","kind":"native_paused_recovery_record","version":1,"token":r.token,
            "scope":image.context.scope,"workspace":image.context.workspace,"update":image.context.update,"projectPath":image.context.project_path,
            "nativeSession":image.context.native_session,"nativePid":std::process::id(),"activationToken":image.context.activation_token,
            "capture":image.capture,"frameSha256":image.sha256,"frameBytes":image.bytes,"manifestPath":self.manager.path,
            "reserveBytes":self.reserve,"intentPath":self.intent_path,"intent":self.intent(image),"record":record,
            "current":false,"writeAuthority":false,"startupAuthority":false,"retirementAuthority":false,"workersMayResume":false,
            "sourceValidation":"not-performed","mediaEvidence":false}))
    }
}
pub(super) fn run(s: &mut Session, p: &Value) -> Result<Value> {
    let (r, image) = request(s, p)?;
    let check = s.agent_bridge.operations.check().ok_or_else(|| fail("original guarded publication operation required"))?;
    let mut slot = image.publication.lock().unwrap_or_else(|e| e.into_inner());
    if slot.is_none() {
        if r.action != "prepare" {
            return Err(fail("no original live publication exists; descriptions cannot recreate it"));
        }
        let publication = Publication::prepare(s, &image, &r, &check)?;
        s.agent_bridge.operations.enter_publication(&r.token)?;
        *slot = Some(publication);
    }
    let publication = slot.as_mut().ok_or_else(|| fail("original publication unavailable"))?;
    publication.matches(&r)?;
    if r.action == "prepare" {
        image.current(s, &check)?;
        publication.manager.current(&check)?;
    }
    if r.action == "publish" {
        publication.publish(s, &image, &r, &check)?;
    }

    if r.action == "retire" {
        let retirement = publication.retirement(s, &image, &r, &check)?;
        drop(slot);
        // The real Session is owned by the existing native worker. Stop may
        // refuse before this fence; after it, no command or autosave may resume.
        s.agent_bridge.operations.enter_publication(&r.token)?;
        let h = s
            .agent_bridge
            .workspace_update
            .as_mut()
            .filter(|h| h.candidate && !h.candidate_cancelled && !h.retiring && h.current_image.as_ref().is_some_and(|held| Arc::ptr_eq(held, &image)))
            .ok_or_else(|| fail("the original paused producer changed before retirement"))?;
        h.current_image_retirement = Some(retirement);
        h.retiring = true;
        // The existing native writer/UI delivers this receipt then closes itself.
        // No PID is signalled and no image owner or persistence key is decoded.
        return super::super::view(s).ok_or_else(|| fail("the retired native producer lost its fence"));
    }

    // Read observes the same original physical record even if its source or
    // manager later changes. It grants no currentness, write or startup claim.
    publication.receipt(&image, &r, &check)
}
