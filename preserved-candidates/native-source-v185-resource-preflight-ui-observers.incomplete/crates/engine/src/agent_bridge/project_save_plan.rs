//! Original Save plan split into a live owner guard and immutable worker data.
//! Workers cannot access Session, Services, media/preview stores or publication.
use super::{media_guard::Check, parse, project_plane};
use crate::{
    Result, Session,
    commands::bad,
    project_save::{
        self,
        intent::{Intent, Resolution},
    },
    project_workspace::ProjectScope,
};
use filmcraft_project::Project;
use serde::Deserialize;
use serde_json::{Value, json};
use std::sync::Arc;
pub(super) const PLAN: &str = "agent.planProjectSave";
fn error(m: impl Into<String>) -> crate::EngineError {
    bad(PLAN, m)
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Query {
    token: String,
    scope: ProjectScope,
    before_project_sha256: String,
    intent: Value,
}
#[derive(Clone)]
pub(super) struct Computation {
    token: String,
    scope: ProjectScope,
    before: String,
    revision: u64,
    sequence: Option<u64>,
    project: Arc<Project>,
    intent: Intent,
    resolution: Value,
    input: Option<project_save::Input>,
}
/// Original worker result. Not Clone/Deserialize; the actual Vec is transferred.
pub(super) struct Completed {
    description: Value,
    encoded: Option<project_save::Encoded>,
}
impl Completed {
    pub(super) fn description(&self) -> &Value {
        &self.description
    }
    pub(super) fn has_destination(&self) -> bool {
        self.encoded.is_some()
    }
    #[cfg(test)]
    pub(super) fn payload_identity(&self) -> Option<(usize, usize)> {
        self.encoded.as_ref().map(|v| v.payload_identity())
    }
}
pub(super) struct Prepared {
    guard: project_save::Guard,
    compute: Computation,
}
pub(super) fn prepare(s: &Session, p: &Value) -> Result<Prepared> {
    let q: Query = parse(PLAN, p)?;
    project_plane::validate_ticket(s, &q.token, &q.scope)?;
    if q.before_project_sha256.len() != 64 || !q.before_project_sha256.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)) {
        return Err(error("Exact current complete native project basis required."));
    }
    let intent = Intent::parse(&q.intent)?;
    let guard = project_save::guard(s)?;
    let (resolution, input) = match intent.resolve(s)? {
        Resolution::NeedsDestination { human_command, native_dialog } => {
            (json!({"humanCommand":human_command,"nativeDialog":native_dialog,"pathSelected":false,"saveAllScope":"this_native_session_only"}), None)
        }
        Resolution::Destination { path, adopt, human_command, uses_current_path } => (
            json!({"humanCommand":human_command,"usesCurrentPath":uses_current_path,"adopt":adopt,"saveAllScope":"this_native_session_only","relativePathRequiresCurrentDirectoryAdmission":!std::path::Path::new(&path).is_absolute()}),
            Some(project_save::input(s, &path, adopt)),
        ),
    };
    guard.current(s)?;
    Ok(Prepared {
        guard,
        compute: Computation {
            token: q.token,
            scope: q.scope,
            before: q.before_project_sha256,
            revision: s.revision,
            sequence: s.state.active_sequence.map(|id| id.0),
            project: s.project.clone(),
            intent,
            resolution,
            input,
        },
    })
}
impl Prepared {
    pub(super) fn bind(&self, s: &Session, completed: Completed) -> Result<project_save::Prepared> {
        self.live(s)?;
        let project_save::intent::Resolution::Destination { path, adopt, .. } = self.compute.intent.resolve(s)? else {
            return Err(error("Original Save still requires its native human destination."));
        };
        let encoded = completed.encoded.ok_or_else(|| error("A metadata-only destination request has no encoded Save to consume."))?;
        self.guard.bind_encoded(s, encoded, &path, adopt)
    }
    pub(super) fn token(&self) -> &str {
        &self.compute.token
    }
    pub(super) fn computation(&self) -> Computation {
        self.compute.clone()
    }
    pub(super) fn live(&self, s: &Session) -> Result<()> {
        project_plane::validate_ticket(s, &self.compute.token, &self.compute.scope)?;
        self.guard.current(s)
    }
}
impl Computation {
    pub(super) fn compute(&self, check: &impl Fn() -> Result<()>) -> Result<Value> {
        Ok(self.compute_owned(check)?.description)
    }
    pub(super) fn compute_owned(&self, check: &impl Fn() -> Result<()>) -> Result<Completed> {
        check()?;
        if project_plane::project_basis(&self.project)? != self.before {
            return Err(error("Exact current complete native project basis required."));
        }
        check()?;
        let encoded = self.input.as_ref().map(|input| input.compute(check)).transpose()?;
        check()?;
        let save = encoded.as_ref().map(|v| v.description()).unwrap_or(Value::Null);
        check()?;
        let result = json!({"status":if self.input.is_some(){"planned"}else{"needs_destination"},"kind":"native_project_save_plan","version":1,"token":self.token,"scope":self.scope,"revision":self.revision,"sequence":self.sequence,"beforeProjectSha256":self.before,"intent":self.intent,"resolution":self.resolution,"save":save,"projectUnchanged":true,"historyUnchanged":true,"sourceValidation":"not-performed","mediaEvidence":false,"writeAuthority":false,"fileCustody":false,"filesystemQualification":"not-performed","startupAuthority":false});
        if super::project_audit::value_bytes(&result)?.len() > 64 * 1024 {
            return Err(error("Complete native Save description exceeds the transport bound; nothing truncated."));
        }
        check()?;
        Ok(Completed { description: result, encoded })
    }
}
pub(super) fn plan(s: &mut Session, p: &Value) -> Result<Value> {
    let check = Check::new(web_time::Instant::now());
    check.check()?;
    let prepared = prepare(s, p)?;
    let result = prepared.computation().compute(&|| check.check())?;
    prepared.live(s)?;
    check.check()?;
    Ok(result)
}
