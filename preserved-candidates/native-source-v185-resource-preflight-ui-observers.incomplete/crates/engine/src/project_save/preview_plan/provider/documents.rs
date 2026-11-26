//! Same original receiver roles, complete byte frames and document credits.
//! Observations and capacity are not Root/overwrite/directory custody grants.
use super::*;
use crate::project_save::{Prepared, retained_effects};
use filmcraft_format::retained::{self, preview_context::NodeFrame};
use serde_json::{Value, json};
pub(super) struct Row {
    pub role: retained_effects::Role,
    pub literal: String,
    pub frame: NodeFrame,
    pub payload: Option<(u64, String)>,
    pub assigned: bool,
    pub errors: Vec<String>,
}
pub(super) struct PreferencesDirectory {
    pub literal: PathBuf,
    pub before: Arc<DirectoryFrame>,
    pub publication: Option<directory_effect::Publication>,
    pub attempted: bool,
}
pub(crate) struct Documents {
    pub(super) rows: Vec<Row>,
    pub(super) source: Option<NodeFrame>,
    pub(super) preferences: Option<PreferencesDirectory>,
}
fn node(plan: &Plan, path: &Path, check: &impl Fn() -> io::Result<()>) -> io::Result<NodeFrame> {
    if !plan.volumes.iter().any(|v| path.starts_with(&v.spec.root)) {
        return Err(error("document path has no explicit working root; no fallback"));
    }
    let parent = path.parent().ok_or_else(|| error("original document parent missing"))?;
    let name = path.file_name().and_then(|v| v.to_str()).ok_or_else(|| error("original document name cannot be represented"))?;
    let frame = DirectoryFrame::capture(parent, check)?;
    plan.volume_for(&frame).map_err(from_engine)?;
    frame.node(name, check)
}
fn physical(cwd: &Path, path: &Path) -> PathBuf {
    if path.is_absolute() { path.into() } else { cwd.join(path) }
}
impl Documents {
    pub(super) fn capture(prepared: &Prepared, plan: &Plan, check: &impl Fn() -> io::Result<()>) -> io::Result<Self> {
        let expected = retained_effects::expected(prepared, &plan.cwd)?;
        let source = prepared.input.path.as_ref().map(|path| node(plan, &physical(&plan.cwd, Path::new(path)), check)).transpose()?;
        if let Some(baseline) = &prepared.input.saved_file {
            if prepared.input.path.as_deref() != Some(&baseline.path) {
                return Err(error("original saved baseline path differs from native current project"));
            }
            if !matches!(source.as_ref().map(|v|v.state()),Some(NodeState::File(file)) if file.sha256==baseline.sha256) {
                return Err(error("complete original saved bytes changed or disappeared before Save"));
            }
        }
        let mut rows = Vec::new();
        let mut preferences = None;
        for e in expected {
            check()?;
            let path = physical(&plan.cwd, Path::new(&e.literal));
            let frame = node(plan, &path, check)?;
            let payload = match e.role {
                retained_effects::Role::SchemaBackup => match source.as_ref().map(|v| v.state()) {
                    Some(NodeState::File(file)) => Some((file.bytes, file.sha256.clone())),
                    _ => None,
                },
                _ => Some((
                    u64::try_from(e.bytes.ok_or_else(|| error("original encoded file size missing"))?).map_err(|_| error("original file size overflow"))?,
                    e.sha256.ok_or_else(|| error("original encoded file hash missing"))?,
                )),
            };
            if e.role == retained_effects::Role::Preferences {
                let literal = Path::new(&e.literal).parent().ok_or_else(|| error("original preferences parent missing"))?.to_path_buf();
                let before = DirectoryFrame::capture(&physical(&plan.cwd, &literal), check)?;
                preferences = Some(PreferencesDirectory { literal, before, publication: None, attempted: false });
            }
            // Multiple original roles or preview names cannot silently overwrite one
            // another through path/case/hardlink aliases. Full causal alias parity remains open.
            for old in &rows {
                let old: &Row = old;
                if old.frame.path() == path
                    || matches!((old.frame.state(),frame.state()),(NodeState::File(a),NodeState::File(b)) if a.identity.device==b.identity.device&&a.identity.inode==b.identity.inode)
                {
                    return Err(error("original document roles alias; no file published"));
                }
            }
            for entry in &plan.entries {
                if entry.source.path() == path || entry.target.path() == path {
                    return Err(error("original document and indexed preview roles collide; no file published"));
                }
                for other in [&entry.source, &entry.target] {
                    if matches!((frame.state(),other.state()),(NodeState::File(a),NodeState::File(b)) if a.identity.device==b.identity.device&&a.identity.inode==b.identity.inode)
                    {
                        return Err(error("document and preview inodes alias; full causal namespace admission required"));
                    }
                }
            }
            rows.push(Row { role: e.role, literal: e.literal, frame, payload, assigned: false, errors: Vec::new() });
        }
        let value = Self { rows, source, preferences };
        value.current(check)?;
        Ok(value)
    }
    pub(super) fn charge(&self, plan: &Plan, ledger: &mut Ledger, check: &impl Fn() -> io::Result<()>) -> io::Result<()> {
        for row in &self.rows {
            let path = row.frame.path();
            let parent = DirectoryFrame::capture(path.parent().ok_or_else(|| error("document budget parent missing"))?, check)?;
            let volume = &plan.volumes[plan.volume_for(&parent).map_err(from_engine)?];
            let bytes = if let Some((bytes, _)) = &row.payload {
                add(
                    mul(2, allocation(*bytes, volume.before.allocation_unit)?)?,
                    mul(8, allocation(super::super::METADATA_BOUND, volume.before.allocation_unit)?)?,
                )?
            } else {
                0
            };
            ledger.charge(Role { name: String::new(), kind: Kind::Document(row.role) }, &volume.spec.device, bytes)?;
        }
        if let Some(directory) = &self.preferences {
            let volume = &plan.volumes[plan.volume_for(&directory.before).map_err(from_engine)?];
            let bytes = mul(
                u64::try_from(directory.before.record().missing.len()).map_err(|_| error("preferences directory depth overflow"))?,
                mul(8, volume.before.allocation_unit)?,
            )?;
            ledger.charge(Role { name: String::new(), kind: Kind::PreferencesDirectories }, &volume.spec.device, bytes)?;
        }
        Ok(())
    }
    pub(super) fn current(&self, check: &impl Fn() -> io::Result<()>) -> io::Result<()> {
        if let Some(source) = &self.source {
            source.current(check)?;
        }
        for row in &self.rows {
            row.frame.current(check)?;
        }
        if let Some(directory) = &self.preferences {
            directory.before.current(check)?;
        }
        self.fence(check)
    }
    pub(super) fn fence(&self, check: &impl Fn() -> io::Result<()>) -> io::Result<()> {
        if let Some(source) = &self.source {
            source.fence(check)?;
        }
        for row in &self.rows {
            row.frame.fence(check)?;
        }
        check()
    }
    pub(super) fn index(&self, role: retained_effects::Role, path: &Path, bytes: &[u8], check: &impl Fn() -> io::Result<()>) -> io::Result<usize> {
        let index = self.rows.iter().position(|row| row.role == role).ok_or_else(|| error("file role is outside exact original Save catalog"))?;
        let row = self.rows.get(index).ok_or_else(|| error("document role disappeared"))?;
        if row.assigned || row.frame.path() != path {
            return Err(error("original document role changed path or already staged"));
        }
        let (size, hash) = row.payload.as_ref().ok_or_else(|| error("original schema backup has no readable source payload; no unexpected file write"))?;
        if u64::try_from(bytes.len()).map_err(|_| error("document payload length overflow"))? != *size {
            return Err(error("original complete document payload size changed"));
        }
        let mut digest = Sha256::new();
        for chunk in bytes.chunks(65536) {
            check()?;
            digest.update(chunk);
        }
        check()?;
        if format!("{:x}", digest.finalize()) != *hash {
            return Err(error("original complete document payload bytes changed"));
        }
        if role == retained_effects::Role::SchemaBackup {
            if let Some(source) = &self.source {
                source.current(check)?;
            }
        }
        row.frame.current(check)?;
        Ok(index)
    }
    pub(crate) fn description(&self) -> Value {
        json!({"kind":"native_original_document_file_admission","rows":self.rows.iter().map(|r|json!({"role":format!("{:?}",r.role),"literal":r.literal,"physical":r.frame.path(),"before":r.frame.state(),"payloadBytes":r.payload.as_ref().map(|v|v.0),"payloadSha256":r.payload.as_ref().map(|v|&v.1),"stageAssigned":r.assigned,"errors":r.errors})).collect::<Vec<_>>(),"preferencesDirectory":self.preferences.as_ref().map(|p|json!({"literal":p.literal,"before":p.before.record(),"attempted":p.attempted,"publication":p.publication})),"allDocumentBudgetsIncluded":true,"writeAuthority":false,"overwriteAuthority":false,"directoryCustody":false,"spaceReserved":false,"nativeComplete":false,"startupAuthority":false})
    }
}
impl Provider<'_> {
    pub(in crate::project_save) fn new_complete<'a>(
        plan: Plan,
        prepared: &Prepared,
        operation: &str,
        delegate: &'a mut dyn FileEffects,
        check: &'a dyn Fn() -> io::Result<()>,
    ) -> io::Result<Provider<'a>> {
        let mut value = Provider::new(plan, operation, delegate, check)?;
        let documents = Documents::capture(prepared, &value.plan, &|| value.guard())?;
        documents.charge(&value.plan, &mut value.ledger, &|| check())?;
        value.documents = Some(documents);
        value.initial_demand = value.ledger.remaining.clone();
        value.guard()?;
        Ok(value)
    }
    pub(in crate::project_save) fn complete_current(&self, s: &crate::Session, key: &crate::project_save::operation::Key) -> crate::Result<()> {
        super::admission::current(&self.plan, &self.ledger, self.documents.as_ref(), s, key, self.check)
    }
    pub(super) fn stage_complete_document(
        &mut self,
        role: retained_effects::Role,
        path: &Path,
        operation: &str,
        bytes: &[u8],
        check: &dyn Fn() -> io::Result<()>,
    ) -> io::Result<retained::StagedWrite> {
        self.guard()?;
        check()?;
        let expected = format!("{:x}", Sha256::digest(format!("{}:{role:?}", self.operation).as_bytes()));
        if operation != expected {
            return Err(error("document staging id differs from exact original native operation/role"));
        }
        let result = (|| {
            let index = self.documents.as_ref().ok_or_else(|| error("complete document admission absent"))?.index(role, path, bytes, &|| {
                self.guard()?;
                check()
            })?;
            self.ledger.enter(Role { name: String::new(), kind: Kind::Document(role) })?;
            let row = self.documents.as_mut().and_then(|d| d.rows.get_mut(index)).ok_or_else(|| error("original document role disappeared before staging"))?;
            row.assigned = true;
            let frame = &self.documents.as_ref().and_then(|d| d.rows.get(index)).ok_or_else(|| error("original document frame unavailable"))?.frame;
            retained::stage_from_frame(frame, operation, bytes, &|| {
                self.guard()?;
                check()
            })
        })();
        if let Err(e) = &result {
            if let Some(row) = self.documents.as_mut().and_then(|d| d.rows.iter_mut().find(|r| r.role == role)) {
                row.errors.push(e.to_string());
            }
        }
        result
    }
    pub(super) fn preferences_directories(&mut self, path: &Path) -> io::Result<()> {
        self.guard()?;
        let before = {
            let planned =
                self.documents.as_ref().and_then(|d| d.preferences.as_ref()).ok_or_else(|| error("original Save did not plan a preferences directory"))?;
            if planned.attempted || planned.literal != path {
                return Err(error("unexpected or repeated original preferences directory callback"));
            }
            planned.before.clone()
        };
        self.documents.as_mut().and_then(|d| d.preferences.as_mut()).ok_or_else(|| error("preferences directory plan disappeared"))?.attempted = true;
        self.ledger.enter(Role { name: String::new(), kind: Kind::PreferencesDirectories })?;
        let staged = directory_effect::stage(before.clone(), &|| self.guard())?;
        let publication = staged.publish(&|| self.guard());
        let complete = publication.complete;
        let failure = publication.error.clone();
        let observed = publication.observed.last().cloned().unwrap_or(before.record().anchor);
        self.documents.as_mut().and_then(|d| d.preferences.as_mut()).ok_or_else(|| error("preferences directory receipt owner disappeared"))?.publication =
            Some(publication);
        if !complete {
            return Err(error(failure.unwrap_or_else(|| "preferences directory effects remain unconfirmed".into())));
        }
        let current = DirectoryFrame::capture(&before.record().path, &|| self.guard())?;
        if current.record().anchor != observed {
            return Err(error("preferences directory changed after observed calls; no adoption"));
        }
        let docs = self.documents.as_ref().ok_or_else(|| error("preferences document owner disappeared"))?;
        let index = docs.rows.iter().position(|r| r.role == retained_effects::Role::Preferences).ok_or_else(|| error("preferences original role absent"))?;
        let row = docs.rows.get(index).ok_or_else(|| error("preferences role missing"))?;
        let name = row.frame.path().file_name().and_then(|v| v.to_str()).ok_or_else(|| error("preferences leaf unavailable"))?.to_string();
        let frame = current.node(&name, &|| self.guard())?;
        if frame.state() != row.frame.state() {
            return Err(error("original preferences target changed during directory preparation"));
        }
        self.documents.as_mut().and_then(|d| d.rows.get_mut(index)).ok_or_else(|| error("preferences frame owner disappeared"))?.frame = frame;
        Ok(())
    }
}
