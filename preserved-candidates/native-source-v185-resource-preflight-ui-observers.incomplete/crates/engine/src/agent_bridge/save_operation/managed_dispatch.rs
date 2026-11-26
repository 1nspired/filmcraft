//! One consuming native call, admitted by actual current private Root controls.
//! No receipt/descriptor can reconstruct the original Key or file Admission.
use super::{Owner, Request, error};
use crate::{Result, Session, agent_bridge::media_guard::Check, project_save::operation};
use filmcraft_format::retained::preview_context::{DirectoryFrame, NodeFrame, NodeState, check_owner_process};
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    io,
    path::{Path, PathBuf},
};
pub(super) const COMMAND: &str = "agent.dispatchProjectSaveOperation";
const META: u64 = 128 * 1024;
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct Params {
    pub(super) job: String,
    pub(super) operation: String,
    capture: String,
    manifest_path: String,
    catalog_sha256: String,
    intent_id: String,
}
pub(super) fn parameters(p: &Value) -> Result<Request> {
    let q: Params = super::super::parse(COMMAND, p)?;
    let r = super::request(&json!({"job":q.job,"operation":q.operation}))?;
    if !crate::editorial_workspace::uuid(&q.capture)
        || !crate::editorial_workspace::uuid(&q.intent_id)
        || !hash(&q.catalog_sha256)
        || !literal_path(&q.manifest_path)
    {
        return Err(error("exact current manager path, catalog, intent and before capture required"));
    }
    Ok(r)
}
fn hash(v: &str) -> bool {
    v.len() == 64 && v.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
fn literal_path(v: &str) -> bool {
    !v.is_empty()
        && v.len() <= 16384
        && !v.chars().any(char::is_control)
        && Path::new(v).is_absolute()
        && Path::new(v).components().all(|c| matches!(c, std::path::Component::RootDir | std::path::Component::Normal(_)))
}
fn sha(v: &[u8]) -> String {
    format!("{:x}", Sha256::digest(v))
}
fn encoded(v: &Value) -> Result<Vec<u8>> {
    serde_json::to_vec(v).map_err(|e| error(e.to_string()))
}
fn keys(v: &Value, names: &str) -> bool {
    v.as_object().is_some_and(|o| o.len() == names.split(',').count() && names.split(',').all(|k| o.contains_key(k)))
}
fn denies(v: &Value, names: &str) -> bool {
    names.split(',').all(|k| v.get(k) == Some(&Value::Bool(false)))
}
fn text<'a>(v: &'a Value, k: &str) -> Result<&'a str> {
    v.get(k).and_then(Value::as_str).ok_or_else(|| error(format!("exact original {k} required")))
}
// The JS producer writes JSON.stringify with indentation. Remove only JSON
// whitespace outside strings, preserving original key order and exact values.
fn compact(raw: &[u8]) -> Vec<u8> {
    let (mut inside, mut escaped) = (false, false);
    let mut out = Vec::with_capacity(raw.len());
    for &b in raw {
        if inside {
            out.push(b);
            if escaped {
                escaped = false;
            } else if b == b'\\' {
                escaped = true;
            } else if b == b'"' {
                inside = false;
            }
        } else if b == b'"' {
            inside = true;
            out.push(b);
        } else if !matches!(b, b' ' | b'\n' | b'\r' | b'\t') {
            out.push(b);
        }
    }
    out
}
fn same(a: &Value, b: &Value) -> bool {
    fn walk(a: &Value, b: &Value, depth: usize, left: &mut usize) -> bool {
        if depth > 64 || *left == 0 {
            return false;
        }
        *left -= 1;
        match (a, b) {
            (Value::Array(x), Value::Array(y)) => x.len() == y.len() && x.iter().zip(y).all(|(a, b)| walk(a, b, depth + 1, left)),
            (Value::Object(x), Value::Object(y)) => x.len() == y.len() && x.iter().all(|(k, a)| y.get(k).is_some_and(|b| walk(a, b, depth + 1, left))),
            (Value::Number(x), Value::Number(y)) => {
                if x == y {
                    return true;
                }
                let safe = |n: &serde_json::Number| n.as_f64().filter(|f| f.is_finite() && f.abs() <= 9_007_199_254_740_991.0);
                safe(x).zip(safe(y)).is_some_and(|(a, b)| a == b)
            }
            _ => a == b,
        }
    }
    walk(a, b, 0, &mut (32 * 1024 * 1024))
}
pub(in crate::agent_bridge) struct Control {
    frame: NodeFrame,
    raw: Vec<u8>,
    value: Value,
}
impl Control {
    // Descriptive access to this original reader's actual retained frame. No
    // caller can supply a Record, Commit or owner through an observation.
    pub(in crate::agent_bridge) fn observation(&self) -> Result<&filmcraft_format::retained::Observation> {
        match self.frame.state() {
            NodeState::File(value) => Ok(value),
            _ => Err(error("a regular original private control observation is required")),
        }
    }
    pub(in crate::agent_bridge) fn value(&self) -> &Value {
        &self.value
    }
    pub(in crate::agent_bridge) fn capture(path: &Path, max: u64, check: &Check) -> Result<Self> {
        let io = || check.check().map_err(|e| io::Error::other(e.to_string()));
        let parent = path.parent().ok_or_else(|| error("original private parent missing"))?;
        let name = path.file_name().and_then(|v| v.to_str()).ok_or_else(|| error("original private name must be UTF-8"))?;
        let directory = DirectoryFrame::capture(parent, &io).map_err(|e| error(e.to_string()))?;
        let frame = directory.node_bounded(name, max, &io).map_err(|e| error(e.to_string()))?;
        let raw = frame.read_private_bytes(max, &io).map_err(|e| error(e.to_string()))?;
        let value = serde_json::from_slice(&raw).map_err(|e| error(format!("private control JSON: {e}")))?;
        Ok(Self { frame, raw, value })
    }
    pub(in crate::agent_bridge) fn current(&self, check: &Check) -> Result<()> {
        self.frame.current(&|| check.check().map_err(|e| io::Error::other(e.to_string()))).map_err(|e| error(e.to_string()))
    }
    pub(in crate::agent_bridge) fn fence(&self, check: &Check) -> Result<()> {
        self.frame.fence(&|| check.check().map_err(|e| io::Error::other(e.to_string()))).map_err(|e| error(e.to_string()))
    }
}

pub(in crate::agent_bridge) struct Origin {
    manifest: PathBuf,
    lease: Control,
    scope: Value,
    manager: u32,
}
pub(super) struct ManagerBasis {
    origin: std::sync::Arc<Origin>,
    manifest: std::sync::Arc<Control>,
    action: Value,
    send_job: Value,
}
impl ManagerBasis {
    pub(super) fn current(&self, check: &Check) -> Result<()> {
        self.origin.current(check)?;
        self.manifest.current(check)
    }
}
impl Origin {
    fn current(&self, check: &Check) -> Result<()> {
        self.lease.current(check)?;
        check_owner_process(self.manager).map_err(|e| error(format!("original startup manager unavailable: {e}")))?;
        check.check()
    }
    pub(super) fn bind(self: &std::sync::Arc<Self>, s: &Session) -> Result<Option<ManagerBasis>> {
        let check = Check::new(web_time::Instant::now());
        self.current(&check)?;
        let manifest = Control::capture(&self.manifest, META, &check)?;
        if manifest.value.get("id") != self.scope.get("workspaceId")
            || manifest.value.get("projectId") != self.scope.get("projectId")
            || manifest.value.get("projectPath").and_then(Value::as_str) != s.path.as_deref()
        {
            return Err(error("startup manager Root changed before original Save begin"));
        }
        let parent = self.manifest.parent().ok_or_else(|| error("startup manager parent missing"))?;
        let activity = Control::capture(&parent.join(format!("workspace-state-{}", text(&self.scope, "workspaceId")?)).join("activity.json"), META, &check)?;
        if activity.value.get("status") != Some(&json!("active")) || activity.value.get("action") != Some(&json!("agent")) {
            return Ok(None);
        }
        let action = activity
            .value
            .get("operationId")
            .cloned()
            .filter(|v| v.as_str().is_some_and(crate::editorial_workspace::uuid))
            .ok_or_else(|| error("original accepted Send action required"))?;
        let send_job = activity
            .value
            .get("job")
            .and_then(|v| v.get("id"))
            .cloned()
            .filter(|v| v.as_str().is_some_and(|s| !s.is_empty() && s.len() <= 128 && !s.chars().any(char::is_control)))
            .ok_or_else(|| error("original accepted Send job required"))?;
        self.current(&check)?;
        manifest.current(&check)?;
        Ok(Some(ManagerBasis { origin: self.clone(), manifest: std::sync::Arc::new(manifest), action, send_job }))
    }
}
impl Origin {
    fn capture(s: &Session, manifest_path: &Path) -> Result<std::sync::Arc<Self>> {
        let path = manifest_path.to_str().filter(|v| literal_path(v)).ok_or_else(|| error("canonical startup manager path required"))?;
        let check = Check::new(web_time::Instant::now());
        let manifest = Control::capture(manifest_path, META, &check)?;
        let scope = crate::project_workspace::identity(&s.project)?.ok_or_else(|| error("actual saved Root required before manager admission"))?.scope;
        let scope = serde_json::to_value(scope).map_err(|e| error(e.to_string()))?;
        if manifest.value.get("kind") != Some(&json!("poema-saved-native-workspace"))
            || manifest.value.get("id") != scope.get("workspaceId")
            || manifest.value.get("projectId") != scope.get("projectId")
            || manifest.value.get("projectPath").and_then(Value::as_str) != s.path.as_deref()
        {
            return Err(error("startup manifest is not the actual native Root"));
        }
        let lease = Control::capture(Path::new(&format!("{path}.lock")), 4096, &check)?;
        if !keys(&lease.value, "pid,identity") || !crate::editorial_workspace::uuid(text(&lease.value, "identity")?) {
            return Err(error("actual original startup manager lease required"));
        }
        let manager = lease
            .value
            .get("pid")
            .and_then(Value::as_u64)
            .and_then(|v| u32::try_from(v).ok())
            .filter(|v| *v > 0)
            .ok_or_else(|| error("original manager PID required"))?;
        let origin = Origin { manifest: manifest_path.into(), lease, scope, manager };
        origin.current(&check)?;
        manifest.current(&check)?;
        Ok(std::sync::Arc::new(origin))
    }
    fn matches_session(&self, s: &Session) -> Result<()> {
        let check = Check::new(web_time::Instant::now());
        self.current(&check)?;
        let m = Control::capture(&self.manifest, META, &check)?;
        let scope = crate::project_workspace::identity(&s.project)?.ok_or_else(|| error("same independent startup Root required"))?.scope;
        if serde_json::to_value(scope).map_err(|e| error(e.to_string()))? != self.scope
            || m.value.get("id") != self.scope.get("workspaceId")
            || m.value.get("projectId") != self.scope.get("projectId")
            || m.value.get("projectPath").and_then(Value::as_str) != s.path.as_deref()
        {
            return Err(error("original startup Root changed before activation"));
        }
        self.current(&check)?;
        m.current(&check)
    }
}
impl Session {
    /// Trusted native startup only; never installed by a control request.
    pub fn install_managed_save_root(&mut self, manifest_path: &Path) -> Result<()> {
        self.require_owner()?;
        if self.save_operation_held() || self.agent_bridge.manager_save_origin.is_some() || self.agent_bridge.manager_save_startup.is_some() {
            return Err(error("startup Save manager is already bound or a native Save remains pending"));
        }
        self.agent_bridge.manager_save_origin = Some(Origin::capture(self, manifest_path)?);
        Ok(())
    }
    /// Trusted paused startup constructor. Retains original controls but does
    /// not activate this Session, resume persistence, or advertise dispatch.
    pub fn retain_managed_save_startup_root(&mut self, manifest_path: &Path) -> Result<()> {
        super::super::workspace_update::require_save_startup_candidate(self)?;
        if self.save_operation_held() || self.agent_bridge.manager_save_origin.is_some() || self.agent_bridge.manager_save_startup.is_some() {
            return Err(error("original startup Save binding cannot be replaced or installed late"));
        }
        self.agent_bridge.manager_save_startup = Some(Origin::capture(self, manifest_path)?);
        Ok(())
    }
    pub fn managed_save_dispatch_available(&self) -> bool {
        self.worker_available
            && super::super::operation_cancellation_available(self)
            && self.require_owner().is_ok()
            && self.agent_bridge.manager_save_origin.as_ref().is_some_and(|o| o.matches_session(self).is_ok())
    }
}
pub(in crate::agent_bridge) fn check_startup(s: &Session) -> Result<()> {
    if let Some(origin) = &s.agent_bridge.manager_save_startup {
        if s.agent_bridge.manager_save_origin.is_some() {
            return Err(error("startup manager already active"));
        }
        super::super::workspace_update::require_save_startup_candidate(s)?;
        origin.matches_session(s)?;
    }
    Ok(())
}
pub(in crate::agent_bridge) fn commit_startup(s: &mut Session) {
    // Same single-threaded original activation already validated this Arc;
    // only successful original persistence startup reaches this point.
    if let Some(origin) = s.agent_bridge.manager_save_startup.take() {
        s.agent_bridge.manager_save_origin = Some(origin);
    }
}

struct Custody {
    controls: Vec<Control>,
    resources: Vec<Control>,
    manager: u32,
    origin: std::sync::Arc<Origin>,
    original_manifest: std::sync::Arc<Control>,
}
impl Custody {
    fn current(&self, check: &Check) -> Result<()> {
        self.origin.current(check)?;
        self.original_manifest.current(check)?;
        check.check()?;
        check_owner_process(self.manager).map_err(|e| error(format!("original manager is unavailable: {e}")))?;
        for c in &self.controls {
            c.current(check)?;
        }
        // These are the same complete resource reads from this consuming call.
        // Full original native file/admission checks are separate and mandatory.
        for c in &self.resources {
            c.fence(check)?;
        }
        check.check()
    }
    fn capture(s: &Session, owner: &Owner, q: &Params, check: &Check) -> Result<Self> {
        let bound = owner.manager.as_ref().ok_or_else(|| error("original startup manager binding required; controls or receipts cannot install one"))?;
        if bound.origin.manifest != Path::new(&q.manifest_path) {
            return Err(error("dispatch names another actual startup manager"));
        }
        bound.origin.current(check)?;
        bound.manifest.current(check)?;
        let before = owner.recovery.as_ref().ok_or_else(|| error("original published before frame required"))?;
        before.require_published_private_record(s, &owner.key, check)?;
        let frame = before.describe(&q.job, &q.operation);
        if frame.get("capture") != Some(&json!(q.capture)) || frame.get("phase") != Some(&json!("Prepared")) {
            return Err(error("dispatch belongs to another original before frame"));
        }
        let slot = owner.admission.as_ref().ok_or_else(|| error("same retained native file admission required"))?;
        let original = slot.original.as_ref().ok_or_else(|| error("original file admission already consumed or failed"))?;
        operation::validate_file_admission(s, &owner.key, original, &|| check.check().map_err(|e| io::Error::other(e.to_string())))?;
        let manifest = Control::capture(Path::new(&q.manifest_path), META, check)?;
        let m = &manifest.value;
        let scope = frame.get("scope").ok_or_else(|| error("actual Root scope missing"))?;
        if m.get("kind") != Some(&json!("poema-saved-native-workspace"))
            || !m.get("version").and_then(Value::as_u64).is_some_and(|v| (1..=5).contains(&v))
            || m.get("id") != scope.get("workspaceId")
            || m.get("projectId") != scope.get("projectId")
            || m.get("projectPath") != frame.get("projectPath")
            || m.get("nativePid").and_then(Value::as_u64).is_none_or(|p| p != 0 && p != u64::from(std::process::id()))
        {
            return Err(error("current manager manifest does not own this actual native Root"));
        }
        let volumes = serde_json::to_value(&slot.volumes).map_err(|e| error(e.to_string()))?;
        if !volumes
            .as_array()
            .is_some_and(|v| v.iter().any(|r| r.get("root").and_then(Value::as_str).is_some_and(|p| Path::new(&q.manifest_path).starts_with(p))))
        {
            return Err(error("manager control has no retained explicit working root"));
        }
        let parent = Path::new(&q.manifest_path).parent().ok_or_else(|| error("manager parent missing"))?;
        let directory = parent.join(format!("workspace-state-{}", text(scope, "workspaceId")?));
        let lease = Control::capture(Path::new(&format!("{}.lock", q.manifest_path)), 4096, check)?;
        let activity = Control::capture(&directory.join("activity.json"), META, check)?;
        let catalog = Control::capture(&directory.join("native-save-evidence.json"), META, check)?;
        if sha(&catalog.raw) != q.catalog_sha256
            || !keys(&catalog.value, "kind,version,identity,operations,current,writeAuthority,nativeComplete,startupAuthority,requestReplay")
            || catalog.value.get("kind") != Some(&json!("poema-native-Save-evidence"))
            || catalog.value.get("version") != Some(&json!(7))
            || !denies(&catalog.value, "current,writeAuthority,nativeComplete,startupAuthority,requestReplay")
        {
            return Err(error("exact complete durable original catalog7 required"));
        }
        let identity = catalog.value.get("identity").ok_or_else(|| error("catalog Root identity missing"))?;
        if identity.get("workspaceId") != m.get("id")
            || identity.get("projectId") != m.get("projectId")
            || identity.get("projectPath") != m.get("projectPath")
            || identity.get("readPlane") != Some(&m.get("readPlane").cloned().unwrap_or(json!("sequence")))
        {
            return Err(error("durable Save catalog belongs to another manager Root"));
        }
        let rows = catalog
            .value
            .get("operations")
            .and_then(Value::as_array)
            .filter(|r| !r.is_empty() && r.len() <= 4)
            .ok_or_else(|| error("bounded original Save rows required"))?;
        let matching = rows.iter().filter(|r| r.get("request") == Some(&json!({"job":q.job,"operation":q.operation}))).collect::<Vec<_>>();
        if matching.len() != 1 {
            return Err(error("one exact original native request required"));
        }
        let row = matching.first().copied().ok_or_else(|| error("original row missing"))?;
        if !keys(row, "id,action,request,intent,basis,phase,nativeObservation,observations,dispatchIntent")
            || row.get("phase") != Some(&json!("before_dispatch"))
        {
            return Err(error("durable before-dispatch row required"));
        }
        if row.get("action") != Some(&bound.action) {
            return Err(error("dispatch belongs to another original accepted Send"));
        }
        let basis = row.get("basis").ok_or_else(|| error("original native basis missing"))?;
        let directory_basis = basis.get("nativeDirectory").ok_or_else(|| error("explicit native directory observation required"))?;
        let native_cwd = slot.description.as_ref().and_then(|v| v.get("planned")).and_then(|v| v.get("nativeCwd"));
        let output = owner
            .plan_description
            .get("save")
            .and_then(|v| v.get("output"))
            .and_then(|v| v.get("path"))
            .and_then(Value::as_str)
            .ok_or_else(|| error("original output missing"))?;
        if !directory_basis.is_null() && Some(directory_basis) != native_cwd
            || !Path::new(output).is_absolute() && (directory_basis.is_null() || Some(directory_basis) != native_cwd)
        {
            return Err(error("relative dispatch requires the actual original native directory context"));
        }
        let expected_basis = json!({"kind":"archived_original_Save_basis","version":1,"token":owner.plan_description.get("token"),"scope":scope,"nativeSession":frame.get("nativeSession"),"revision":frame.get("revision"),"sequence":frame.get("sequence"),"projectSha256":frame.get("projectReadSha256"),"projectPath":frame.get("projectPath"),"nativeDirectory":directory_basis});
        if !same(basis, &expected_basis) || row.get("intent") != owner.plan_description.get("intent") {
            return Err(error("durable row does not retain the complete original native basis and Save intent"));
        }
        let intent = row.get("dispatchIntent").ok_or_else(|| error("original Send intent missing"))?;
        if !keys(
            intent,
            "kind,version,id,action,sendJob,request,scope,nativeSession,nativePid,planSha256,filesSha256,admissionSha256,before,manager,overwritePolicy,current,fileCustody,overwriteAuthority,writeAuthority,nativeComplete,startupAuthority,workersMayResume,requestReplay",
        ) || encoded(intent)?.len() > 4096
            || intent.get("kind") != Some(&json!("original_Save_dispatch_intent"))
            || intent.get("version") != Some(&json!(1))
            || intent.get("id") != Some(&json!(q.intent_id))
            || intent.get("action") != row.get("action")
            || !crate::editorial_workspace::uuid(text(intent, "action")?)
            || intent.get("request") != row.get("request")
            || intent.get("scope") != Some(scope)
            || intent.get("nativeSession") != frame.get("nativeSession")
            || intent.get("nativePid") != Some(&json!(std::process::id()))
            || intent.get("overwritePolicy") != Some(&json!("original_observed_generation_only"))
            || !denies(intent, "current,fileCustody,overwriteAuthority,writeAuthority,nativeComplete,startupAuthority,workersMayResume,requestReplay")
        {
            return Err(error("foreign, malformed or authority-bearing original Send intent"));
        }
        if intent.get("sendJob") != Some(&bound.send_job) {
            return Err(error("dispatch belongs to another original Send job"));
        }
        let manager = intent.get("manager").ok_or_else(|| error("original manager binding missing"))?;
        let manager_pid = manager
            .get("pid")
            .and_then(Value::as_u64)
            .and_then(|p| u32::try_from(p).ok())
            .filter(|p| *p > 0)
            .ok_or_else(|| error("original manager PID required"))?;
        if !keys(manager, "pid,manifestSha256,generation")
            || manager.get("manifestSha256") != Some(&json!(sha(&compact(&manifest.raw))))
            || !keys(&lease.value, "pid,identity")
            || lease.value.get("pid") != manager.get("pid")
            || !crate::editorial_workspace::uuid(text(&lease.value, "identity")?)
        {
            return Err(error("actual original manager lease or manifest binding differs"));
        }
        let NodeState::File(observed) = manifest.frame.state() else {
            return Err(error("complete private manifest file required"));
        };
        let g = manager.get("generation").ok_or_else(|| error("original manifest generation missing"))?;
        let nanos = |v: &str| -> Result<String> {
            let (s, n) = v.split_once(':').ok_or_else(|| error("original file time required"))?;
            let s = s.parse::<i128>().map_err(|e| error(e.to_string()))?;
            let n = n.parse::<i128>().map_err(|e| error(e.to_string()))?;
            Ok(s.checked_mul(1_000_000_000).and_then(|v| v.checked_add(n)).ok_or_else(|| error("original file time overflow"))?.to_string())
        };
        let expected = json!({"dev":observed.identity.device,"ino":observed.identity.inode,"mode":observed.identity.mode.to_string(),"uid":observed.identity.uid.to_string(),"size":observed.bytes.to_string(),"mtimeNs":nanos(&observed.modified)?,"ctimeNs":nanos(&observed.changed)?});
        if !same(g, &expected) {
            return Err(error("original manager manifest generation changed"));
        }
        if manager_pid != bound.origin.manager {
            return Err(error("original startup manager process changed"));
        }
        let a = &activity.value;
        if a.get("status") != Some(&json!("active"))
            || a.get("action") != Some(&json!("agent"))
            || a.get("operationId") != intent.get("action")
            || a.get("job").and_then(|j| j.get("id")) != intent.get("sendJob")
            || a.get("originalInput") == Some(&json!(true))
            || a.get("mediaPreparation") == Some(&json!(true))
            || a.get("sourceAnalysis").and_then(|v| v.get("phase")) == Some(&json!("waiting"))
        {
            return Err(error("same currently accepted native Send required"));
        }
        let operations=rows.iter().map(|r|json!({"id":r.get("id"),"action":r.get("action"),"request":r.get("request"),"nativeSession":r.get("basis").and_then(|b|b.get("nativeSession")),"phase":r.get("phase")})).collect::<Vec<_>>();
        let pointer = json!({"kind":"original_Save_activity","version":1,"catalog":"native-save-evidence.json","sha256":q.catalog_sha256,"operations":operations,"current":false,"writeAuthority":false,"nativeComplete":false,"startupAuthority":false,"requestReplay":false});
        if !a.get("nativeSave").is_some_and(|v| same(v, &pointer)) {
            return Err(error("actual current activity lost the complete catalog pointer"));
        }
        let refs = row.get("observations").and_then(Value::as_array).filter(|r| r.len() <= 6).ok_or_else(|| error("complete original resources required"))?;
        let mut resources = Vec::new();
        let mut values = BTreeMap::new();
        let mut digests = BTreeMap::new();
        let evidence_id = text(row, "id")?;
        if !crate::editorial_workspace::uuid(evidence_id) {
            return Err(error("original evidence UUID required"));
        }
        for r in refs {
            let role = text(r, "role")?;
            let limit = match role {
                "plan" | "recovery_preparation" | "recovery_record" => 65536,
                "files" => 32 * 1024 * 1024,
                "effects" => 32 * 1024 * 1024 + 65536,
                "admission" => 32 * 1024 * 1024 + 512 * 1024,
                _ => return Err(error("unknown original Save role")),
            };
            let digest = text(r, "sha256")?;
            let name = text(r, "name")?;
            let bytes =
                r.get("bytes").and_then(Value::as_u64).filter(|v| *v > 0 && *v <= limit).ok_or_else(|| error("complete original role bound required"))?;
            if !keys(r, "role,name,sha256,bytes")
                || !hash(digest)
                || name != format!("native-save-observation-{evidence_id}-{role}-{digest}.json")
                || values.contains_key(role)
            {
                return Err(error("foreign or repeated complete resource"));
            }
            let c = Control::capture(&directory.join(name), limit, check)?;
            if c.raw.len() as u64 != bytes || sha(&c.raw) != digest {
                return Err(error("complete original resource bytes differ"));
            }
            digests.insert(role.to_owned(), digest.to_owned());
            values.insert(role.to_owned(), c.value.clone());
            resources.push(c);
        }
        for role in ["plan", "files", "admission", "recovery_preparation", "recovery_record"] {
            if !values.contains_key(role) {
                return Err(error("original dispatch prerequisite resource missing"));
            }
        }
        for (role, field) in [("plan", "planSha256"), ("files", "filesSha256"), ("admission", "admissionSha256")] {
            if intent.get(field) != digests.get(role).map(|v| json!(v)).as_ref() {
                return Err(error("original intent resource binding changed"));
            }
        }
        let file_value = values.get("files").ok_or_else(|| error("complete original file observation missing"))?;
        if !keys(
            file_value,
            "kind,version,planSha256,token,scope,revision,storageRoots,parents,absences,entries,knownProjectOutputBytes,writeAuthority,fileCustody,overwriteAuthority,startupAuthority,workerQuiescence,allEffectsVerified,publicationSpaceQualified",
        ) || file_value.get("kind") != Some(&json!("native_project_save_files"))
            || file_value.get("version") != Some(&json!(1))
            || file_value.get("scope") != Some(scope)
            || file_value.get("revision") != frame.get("revision")
            || file_value.get("token") != owner.plan_description.get("token")
            || file_value.get("planSha256") != digests.get("plan").map(|v| json!(v)).as_ref()
            || !denies(
                file_value,
                "writeAuthority,fileCustody,overwriteAuthority,startupAuthority,workerQuiescence,allEffectsVerified,publicationSpaceQualified",
            )
            || !file_value.get("storageRoots").and_then(Value::as_array).is_some_and(|v| !v.is_empty() && v.len() <= 16)
            || !file_value.get("entries").and_then(Value::as_array).is_some_and(|v| !v.is_empty() && v.len() <= 100000)
            || ["parents", "absences"].iter().any(|k| !file_value.get(k).and_then(Value::as_array).is_some_and(|v| v.len() <= 1000000))
        {
            return Err(error("malformed or authority-bearing complete original file observations"));
        }
        if !values.get("plan").is_some_and(|v| same(v, &owner.plan_description)) {
            return Err(error("complete original native plan was replaced"));
        }
        let body = slot.description.as_ref().ok_or_else(|| error("complete original admission description unavailable"))?;
        let expected_admission = json!({"kind":"native_Save_file_admission_read","version":1,"job":q.job,"operation":q.operation,"capture":q.capture,"scope":scope,"nativeSession":frame.get("nativeSession"),"nativePid":std::process::id(),"volumes":volumes,"phase":"Prepared","current":false,"fileCustody":false,"overwriteAuthority":false,"spaceReserved":false,"writeAuthority":false,"nativeComplete":false,"startupAuthority":false,"workersMayResume":false,"admission":body});
        if !values.get("admission").is_some_and(|v| same(v, &expected_admission)) {
            return Err(error("complete native admission was replaced"));
        }
        let mut expected_recovery = frame.clone();
        expected_recovery["retainedFrames"] = json!([frame.clone()]);
        if !values.get("recovery_preparation").is_some_and(|v| same(v, &expected_recovery)) {
            return Err(error("complete original native before frame differs"));
        }
        let inspected = before.inspect_private_record(check)?;
        let record = json!({"kind":"native_Save_recovery_record","version":1,"job":q.job,"operation":q.operation,"scope":scope,"nativeSession":frame.get("nativeSession"),"nativePid":std::process::id(),"capture":q.capture,"phase":"Prepared","frameSha256":frame.get("sha256"),"record":inspected,"current":false,"writeAuthority":false,"nativeComplete":false,"startupAuthority":false,"workersMayResume":false});
        let reserve = record
            .get("record")
            .and_then(|v| v.get("receipt"))
            .and_then(|v| v.get("intent"))
            .and_then(|v| v.get("reserve_bytes"))
            .ok_or_else(|| error("actual original private reserve missing"))?;
        let expected_records = json!({"kind":"native_Save_recovery_records","version":1,"reserveBytes":reserve,"records":[record],"current":false,"writeAuthority":false,"nativeComplete":false,"startupAuthority":false,"workersMayResume":false});
        if !values.get("recovery_record").is_some_and(|v| same(v, &expected_records)) {
            return Err(error("actual complete before Record differs from dispatch evidence"));
        }
        let actual_intent = inspected.get("receipt").and_then(|v| v.get("intent")).ok_or_else(|| error("actual original private intent missing"))?;
        let expected_before =
            json!({"capture":q.capture,"sha256":frame.get("sha256"),"bytes":frame.get("bytes"),"recordIntentSha256":sha(&encoded(actual_intent)?)});
        if !intent.get("before").is_some_and(|v| same(v, &expected_before)) {
            return Err(error("dispatch names another actual original private before frame"));
        }
        if !same(&lease.value, &bound.origin.lease.value) {
            return Err(error("current manager lease is not the startup lease"));
        }
        let value = Self {
            controls: vec![manifest, lease, activity, catalog],
            resources,
            manager: manager_pid,
            origin: bound.origin.clone(),
            original_manifest: bound.manifest.clone(),
        };
        value.current(check)?;
        Ok(value)
    }
}
pub(super) fn run(s: &mut Session, p: &Value) -> Result<Value> {
    let r = parameters(p)?;
    let q: Params = super::super::parse(COMMAND, p)?;
    let task = s.save_lifecycle_control.clone();
    let fallback = Check::new(web_time::Instant::now());
    let check = task.as_ref().map_or(&fallback, |t| t.read_check());
    let current = || {
        check.check()?;
        task.as_ref().map_or(Ok(()), |t| t.current(COMMAND, &r))
    };
    dispatch(s, &q, check, &current, &|| Ok(()))
}
fn dispatch(s: &mut Session, q: &Params, check: &Check, current: &impl Fn() -> Result<()>, observe: &impl Fn() -> Result<()>) -> Result<Value> {
    current()?;
    let mut owner = s.agent_bridge.save_operation.take().ok_or_else(|| error("same actual native Save owner required; no imported key"))?;
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        if owner.request != (Request { job: q.job.clone(), operation: q.operation.clone() }) || owner.dispatch_attempted {
            return Err(error("foreign or already attempted native dispatch; inspect without replay"));
        }
        owner.dispatch_attempted = true;
        let custody = Custody::capture(s, &owner, q, check)?;
        let guard = || {
            current()?;
            observe()?;
            custody.current(check)
        };
        guard()?;
        owner.recovery.as_ref().ok_or_else(|| error("original before frame disappeared"))?.require_published_private_record(s, &owner.key, check)?;
        let slot = owner.admission.as_mut().ok_or_else(|| error("original admission disappeared"))?;
        let original = slot.original.take().ok_or_else(|| error("original admission already consumed; never recapture"))?;
        operation::dispatch_prepared_files(
            s,
            &owner.key,
            original,
            &mut crate::project_save::effects::Original,
            &|| guard().map_err(|e| io::Error::other(e.to_string())),
            &guard,
        )?;
        super::status_for_owner(s, &owner, &guard)
    }))
    .unwrap_or_else(|_| Err(error("managed native Save failed; original owner and attempt retained")));
    s.agent_bridge.save_operation = Some(owner);
    result
}
#[cfg(test)]
pub(super) fn observed_dispatch(s: &mut Session, p: &Value, observe: &impl Fn() -> Result<()>) -> Result<Value> {
    let _ = parameters(p)?;
    let q: Params = super::super::parse(COMMAND, p)?;
    let check = Check::new(web_time::Instant::now());
    dispatch(s, &q, &check, &|| check.check(), observe)
}

// Actual fresh startup custody for stopped current images. The metadata is
// compared, never deserialized into an old owner, Record, Commit or grant.
pub(in crate::agent_bridge) struct CurrentImageControls {
    origin: std::sync::Arc<Origin>,
    manifest: Control,
    activity: Control,
    journal: Control,
    evidence: Control,
    source_pid: Option<u32>,
}
impl CurrentImageControls {
    pub(in crate::agent_bridge) fn current(&self, check: &Check) -> Result<()> {
        self.origin.current(check)?;
        self.origin.lease.fence(check)?;
        if let Some(pid) = self.source_pid {
            check_owner_process(pid).map_err(|e| error(format!("original cold source unavailable: {e}")))?;
        }
        for frame in [&self.manifest, &self.activity, &self.journal, &self.evidence] {
            frame.current(check)?;
            frame.fence(check)?;
        }
        self.origin.current(check)
    }
}

// Cold activation belongs to this actual new native startup manager and actual
// independently loaded restored Input. An archived publisher intent is not an
// owner and cannot satisfy or bypass this branch.
fn cold_current_image_controls(
    expected: &Value,
    check: &Check,
    origin: std::sync::Arc<Origin>,
    manifest: Control,
    activity: Control,
    journal: Control,
) -> Result<CurrentImageControls> {
    let j = journal.value();
    let a = &j["coldCurrentActivation"];
    let workspace = text(expected, "workspace")?;
    let update = text(expected, "update")?;
    let producer = &expected["producer"];
    let candidate = &expected["candidate"];
    let directory = origin.manifest.parent().ok_or_else(|| error("actual cold manager parent required"))?.join(format!("workspace-state-{workspace}"));
    let attempt = text(j, "recoveryAttempt")?;
    let actual_root = directory.join(format!("update-{update}"));
    let actual_data = actual_root.join(format!("recovery-{attempt}-state"));
    if expected["restoredFiles"] != true
        || !crate::editorial_workspace::uuid(attempt)
        || !same(&origin.scope, &expected["scope"])
        || manifest.value()["id"] != expected["workspace"]
        || manifest.value()["projectId"] != expected["scope"]["projectId"]
        || manifest.value()["projectPath"] != expected["projectPath"]
        || activity.value()["status"] != "active"
        || activity.value()["action"] != "workspace-update"
        || activity.value()["operationId"] != update
        || j["kind"] != "poema-native-workspace-update"
        || j["version"] != 7
        || j["phase"] != "activating"
        || j["workspace"] != workspace
        || j["update"] != update
        || j["scope"] != expected["scope"]
        || j["original"]["projectPath"] != expected["projectPath"]
        || j["root"] != json!(actual_root)
        || candidate["dataDir"] != json!(actual_data)
        || j["preparedSnapshot"]["sha256"] != expected["sourceSnapshot"]
        || ["currentImageRetirement", "currentImageRestart", "currentImageActivation", "journalAdoption"].iter().any(|k| j.get(*k).is_some())
        || !keys(a, "kind,version,capture,frameSha256,frameBytes,sourceSnapshot,producer,candidate,sourceCompletion")
        || a["kind"] != "poema-cold-current-activation"
        || a["version"] != 1
        || a["capture"] != expected["capture"]
        || a["frameSha256"] != expected["sha256"]
        || a["frameBytes"] != expected["bytes"]
        || a["sourceSnapshot"] != expected["sourceSnapshot"]
        || !same(&a["producer"], producer)
        || !same(&a["candidate"], candidate)
        || j["candidate"]["pid"] != candidate["pid"]
        || j["candidate"]["nativeSession"] != candidate["nativeSession"]
        || j["candidate"]["dataDir"] != candidate["dataDir"]
        || j["activationToken"] != candidate["activationToken"]
        || !keys(&j["activation"], "nativeSession,pid,finish")
        || j["activation"]["pid"] != candidate["pid"]
        || j["activation"]["nativeSession"] != candidate["nativeSession"]
        || !((j["candidateBuild"] == "next" && j["activation"]["finish"] == "complete")
            || (j["candidateBuild"] == "previous" && j["activation"]["finish"] == "rolled_back"))
        || !keys(&a["sourceCompletion"], "file,sha256")
        || !hash(text(&a["sourceCompletion"], "sha256")?)
    {
        return Err(error("cold current activation requires its actual restored input, new startup manager and exact original activating intent"));
    }
    let file = text(&a["sourceCompletion"], "file")?;
    if !literal_path(file) || Path::new(file).parent() != Some(directory.as_path()) {
        return Err(error("cold source completion must remain in the exact original manager namespace"));
    }
    let name = Path::new(file).file_name().and_then(|v| v.to_str()).ok_or_else(|| error("cold source completion name required"))?;
    let stem = name
        .strip_prefix("application-cold-source-")
        .and_then(|v| v.strip_suffix("-completion.json"))
        .ok_or_else(|| error("original cold source completion name required"))?;
    let (operation, ordinal) = stem.rsplit_once('-').ok_or_else(|| error("bounded original source completion slot required"))?;
    let slot = ordinal.parse::<u32>().map_err(|_| error("bounded original source completion slot required"))?;
    if !crate::editorial_workspace::uuid(operation) || slot >= 64 || ordinal != format!("{slot:04}") {
        return Err(error("cold source completion crossed its original bounded operation"));
    }
    let completion = Control::capture(Path::new(file), 256 * 1024, check)?;
    let v = completion.value();
    let activation = sha(text(candidate, "activationToken")?.as_bytes());
    if sha(&completion.raw) != text(&a["sourceCompletion"], "sha256")?
        || v["kind"] != "poema-application-cold-source-completion"
        || v["version"] != 1
        || v["ordinal"] != slot
        || v["application"]["workspaceId"] != workspace
        || v["application"]["projectId"] != expected["scope"]["projectId"]
        || v["application"]["manifestPath"] != json!(origin.manifest)
        || !keys(&v["native"], "update,snapshotSha256,activationSha256,candidate")
        || v["native"]["update"] != update
        || v["native"]["snapshotSha256"] != expected["sourceSnapshot"]
        || v["native"]["activationSha256"] != activation
        || !same(&v["native"]["candidate"], &j["candidate"])
        || v["handoff"]["record"]["phase"] != "complete"
    {
        return Err(error("cold source completion does not bind this exact actual native candidate and original source acknowledgment"));
    }
    let pid = v["handoff"]["record"]["replacement"]["pid"]
        .as_u64()
        .and_then(|v| u32::try_from(v).ok())
        .filter(|v| *v > 0)
        .ok_or_else(|| error("actual original replacement source PID required"))?;
    if pid == origin.manager || pid == std::process::id() || producer["pid"] == pid {
        return Err(error("original source acknowledgment must name its separate actual service"));
    }
    let result = CurrentImageControls { origin, manifest, activity, journal, evidence: completion, source_pid: Some(pid) };
    result.current(check)?;
    Ok(result)
}

pub(in crate::agent_bridge) fn current_image_controls(s: &Session, expected: &Value, check: &Check) -> Result<CurrentImageControls> {
    super::super::workspace_update::require_save_startup_candidate(s)?;
    let origin =
        s.agent_bridge.manager_save_startup.as_ref().ok_or_else(|| error("current-image activation requires its actual native startup manager"))?.clone();
    origin.matches_session(s)?;
    let root = origin.manifest.parent().ok_or_else(|| error("original startup manager parent missing"))?;
    let workspace = text(expected, "workspace")?;
    let update = text(expected, "update")?;
    let directory = root.join(format!("workspace-state-{workspace}"));
    let manifest = Control::capture(&origin.manifest, META, check)?;
    let activity = Control::capture(&directory.join("activity.json"), META, check)?;
    let journal = Control::capture(&directory.join("workspace-update.json"), 4 * 1024 * 1024, check)?;
    if journal.value().get("coldCurrentActivation").is_some() {
        return cold_current_image_controls(expected, check, origin, manifest, activity, journal);
    }
    let capture = text(expected, "capture")?;
    let intent = Control::capture(&directory.join(format!("paused-recovery-{capture}.json")), META, check)?;
    let j = journal.value();
    let r = &j["currentImageRetirement"];
    let next = &j["currentImageRestart"];
    let a = &j["currentImageActivation"];
    let producer = &expected["producer"];
    let candidate = &expected["candidate"];
    if !same(&origin.scope, &expected["scope"])
        || manifest.value()["id"] != expected["workspace"]
        || manifest.value()["projectId"] != expected["scope"]["projectId"]
        || manifest.value()["projectPath"] != expected["projectPath"]
        || activity.value()["status"] != "active"
        || activity.value()["action"] != "workspace-update"
        || activity.value()["operationId"] != update
        || j["kind"] != "poema-native-workspace-update"
        || ![4, 5, 6, 7].iter().any(|v| j["version"] == *v)
        || j["phase"] != "current_image_activating"
        || j["workspace"] != workspace
        || j["update"] != update
        || j["scope"] != expected["scope"]
        || j["original"]["projectPath"] != expected["projectPath"]
        || j["preparedSnapshot"]["sha256"] != expected["sourceSnapshot"]
        || j["candidate"]["pid"] != producer["pid"]
        || j["candidate"]["nativeSession"] != producer["nativeSession"]
        || r["kind"] != "poema-current-image-retirement"
        || r["version"] != 1
        || r["stage"] != "exited"
        || r["capture"] != capture
        || r["sha256"] != expected["sha256"]
        || r["bytes"] != expected["bytes"]
        || r["nativePid"] != producer["pid"]
        || r["nativeSession"] != producer["nativeSession"]
        || r["activationToken"] != producer["activationToken"]
        || r["exit"] != json!({"pid":producer["pid"],"code":0,"signal":null})
        || next["kind"] != "poema-current-image-restart"
        || next["version"] != 1
        || next["stage"] != "ready"
        || next["capture"] != capture
        || next["activationToken"] != candidate["activationToken"]
        || next["revision"] != candidate["revision"]
        || next["dirty"] != s.is_dirty()
        || next["candidate"]["pid"] != candidate["pid"]
        || next["candidate"]["nativeSession"] != candidate["nativeSession"]
        || next["candidate"]["dataDir"] != candidate["dataDir"]
        || !keys(a, "kind,version,stage,capture,activationToken,nativePid,nativeSession,dataDirectory")
        || a["kind"] != "poema-current-image-activation"
        || a["version"] != 1
        || a["stage"] != "requested"
        || a["capture"] != capture
        || a["activationToken"] != candidate["activationToken"]
        || a["nativePid"] != candidate["pid"]
        || a["nativeSession"] != candidate["nativeSession"]
        || a["dataDirectory"] != candidate["dataDir"]
        || intent.value()["kind"] != "poema-paused-recovery-publication-intent"
        || intent.value()["version"] != 1
        || intent.value()["managerPid"] != origin.manager
        || intent.value()["manifestPath"] != origin.manifest.to_string_lossy().as_ref()
        || intent.value()["capture"] != capture
        || intent.value()["frameSha256"] != expected["sha256"]
        || intent.value()["nativePid"] != producer["pid"]
        || intent.value()["nativeSession"] != producer["nativeSession"]
        || intent.value()["scope"] != expected["scope"]
        || intent.value()["update"] != update
        || intent.value()["projectPath"] != expected["projectPath"]
    {
        return Err(error("current-image activation needs the same original publisher, exact explicit manager intent and actual fresh native candidate"));
    }
    let result = CurrentImageControls { origin, manifest, activity, journal, evidence: intent, source_pid: None };
    result.current(check)?;
    Ok(result)
}

#[cfg(test)]
mod bounded_control_tests {
    use super::*;
    #[test]
    fn original_private_control_bound_refuses_oversized_metadata_before_the_original_complete_payload_hash() {
        let root = crate::temp_dir().join(format!(
            "poema-control-bound-{}-{}",
            std::process::id(),
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
        ));
        std::fs::create_dir(&root).unwrap();
        let root = std::fs::canonicalize(root).unwrap();
        let file = root.join("generated-oversized-private-control.json");
        let owned = std::fs::OpenOptions::new().write(true).create_new(true).open(&file).unwrap();
        owned.set_len(128 * 1024 + 1).unwrap();
        drop(owned);
        let before = std::fs::metadata(&file).unwrap();
        let check = Check::new(web_time::Instant::now());
        let result = Control::capture(&file, 128 * 1024, &check);
        assert!(
            matches!(&result,Err(error) if error.to_string().contains("over its complete byte bound")),
            "the initial original bounded observer must reject before hashing the oversized payload"
        );
        assert_eq!(std::fs::metadata(&file).unwrap().len(), before.len());
        assert!(file.is_file());
        std::fs::remove_file(&file).unwrap();
        std::fs::remove_dir(&root).unwrap();
    }
}
