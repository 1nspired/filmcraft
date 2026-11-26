//! Native data record for an already computed candidate, not a replay recipe or permission.
//! Only seal() can bind live COW identities. Decoding yields historical data only.
use super::{admission, error, project_audit, project_plane};
use crate::project_workspace::{self, ProjectScope};
use crate::{EditorState, Result, Session};
use filmcraft_project::{ItemId, Project};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::sync::{Arc, Weak};
const KIND: &str = "poema-native-multicam-computed-result";
const MAX_BYTES: usize = 7 * 1024 * 1024;
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct FileProof {
    item: u64,
    generation: admission::Generation,
    binding_sha256: String,
    admission_sha256: String,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Payload {
    kind: String,
    version: u32,
    schema: u32,
    scope: ProjectScope,
    native_epoch: String,
    read_token: String,
    revision: u64,
    project_path: String,
    before_project_sha256: String,
    after_project_sha256: String,
    before_editor_sha256: String,
    after_editor_sha256: String,
    input_request_sha256: String,
    input_projection_sha256: String,
    files: Vec<FileProof>,
    human_command: String,
    human_params: Value,
    result: Value,
    before_sequence: Option<u64>,
    after_sequence: Option<u64>,
    native_linked_selection: bool,
    write_authority: bool,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Envelope {
    payload: Payload,
    checksum_sha256: String,
}
/// No Serialize/Deserialize: these weak identities can originate only from the same live producer.
pub(super) struct Sealed {
    data: Envelope,
    before: Weak<Project>,
    after: Weak<Project>,
}
/// A parsed archive has data validation only; it cannot construct a Sealed live owner.
pub(super) struct Historical {
    data: Envelope,
}
fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
fn hash(v: &str) -> bool {
    v.len() == 64 && v.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
fn editor(v: &EditorState) -> Result<String> {
    Ok(digest(&project_audit::value_bytes(v)?))
}
fn file_proofs(project: &Project, scope: &ProjectScope, ids: &[u64]) -> Result<Vec<FileProof>> {
    if ids.len() > 2048 || ids.contains(&0) || ids.windows(2).any(|p| p[0] >= p[1]) {
        return Err(error("computed record requires complete sorted original dependencies"));
    }
    ids.iter()
        .map(|id| {
            let item = project.item(ItemId(*id)).ok_or_else(|| error("computed record original is missing"))?;
            let generation = admission::current_generation(item, scope)?;
            let binding = item.metadata.get(super::super::BINDING_KEY).ok_or_else(|| error("computed record binding missing"))?;
            let marker = item.metadata.get(admission::KEY).ok_or_else(|| error("computed record admission missing"))?;
            Ok(FileProof { item: *id, generation, binding_sha256: digest(binding.as_bytes()), admission_sha256: digest(marker.as_bytes()) })
        })
        .collect()
}
impl Envelope {
    fn validate_shape(&self) -> Result<()> {
        let p = &self.payload;
        if p.kind != KIND
            || p.version != 1
            || p.schema != filmcraft_format::SCHEMA_VERSION
            || !project_workspace::valid_scope(&p.scope)
            || p.native_epoch.len() != 32
            || !p.native_epoch.bytes().all(|b| b.is_ascii_hexdigit())
            || !super::super::token_ok(&p.read_token)
            || p.revision == 0
            || p.revision >= u64::MAX - 1
            || p.project_path.len() > 4096
            || !p.project_path.starts_with('/')
            || p.project_path.chars().any(char::is_control)
            || ![
                &p.before_project_sha256,
                &p.after_project_sha256,
                &p.before_editor_sha256,
                &p.after_editor_sha256,
                &p.input_request_sha256,
                &p.input_projection_sha256,
                &self.checksum_sha256,
            ]
            .iter()
            .all(|v| hash(v))
            || p.files.len() > 2048
            || p.files.iter().any(|f| f.item == 0 || !f.generation.valid() || !hash(&f.binding_sha256) || !hash(&f.admission_sha256))
            || p.files.windows(2).any(|v| v[0].item >= v[1].item)
            || !matches!(p.human_command.as_str(), "clip.synchronize" | "clip.mergeClips" | "clip.createMulticam")
            || !p.human_params.is_object()
            || !p.result.is_object()
            || p.write_authority
            || self.checksum_sha256 != digest(&project_audit::value_bytes(p)?)
        {
            return Err(error("computed native record shape, scope, schema or complete checksum differs"));
        }
        Ok(())
    }
    fn validate_inputs(&self, s: &Session, request: &super::Request, projection: &Value, dependencies: &[u64]) -> Result<()> {
        let p = &self.payload;
        self.validate_origin(request, projection, dependencies)?;
        if s.agent_bridge.epoch.as_ref() != Some(&p.native_epoch) || s.revision != p.revision || s.path.as_ref() != Some(&p.project_path) {
            return Err(error("computed native record differs from the current native epoch, revision or saved path"));
        }
        Ok(())
    }
    fn validate_origin(&self, request: &super::Request, projection: &Value, dependencies: &[u64]) -> Result<()> {
        self.validate_shape()?;
        let p = &self.payload;
        if request.token != p.read_token
            || request.scope != p.scope
            || request.project_path != p.project_path
            || digest(&project_audit::value_bytes(request)?) != p.input_request_sha256
            || digest(&project_audit::value_bytes(projection)?) != p.input_projection_sha256
            || p.files.iter().map(|f| f.item).collect::<Vec<_>>() != dependencies
        {
            return Err(error("computed native record differs from the captured qualified input producer"));
        }
        Ok(())
    }
    fn bytes(&self) -> Result<Vec<u8>> {
        self.validate_shape()?;
        let raw = project_audit::value_bytes(self)?;
        if raw.len() > MAX_BYTES {
            return Err(error("complete computed native record exceeds its bound; nothing truncated"));
        }
        Ok(raw)
    }
    fn validate_content(
        &self,
        before: &Project,
        after: &Project,
        before_editor: &EditorState,
        after_editor: &EditorState,
        command: &str,
        params: &Value,
        result: &Value,
    ) -> Result<()> {
        self.validate_shape()?;
        let p = &self.payload;
        for project in [before, after] {
            if project_workspace::identity(project)?.is_none_or(|root| root.scope != p.scope) {
                return Err(error("computed native record Root differs from its snapshots"));
            }
        }
        if p.before_project_sha256 != project_plane::project_basis(before)?
            || p.after_project_sha256 != project_plane::project_basis(after)?
            || p.before_editor_sha256 != editor(before_editor)?
            || p.after_editor_sha256 != editor(after_editor)?
            || p.before_sequence != before_editor.active_sequence.map(|v| v.0)
            || p.after_sequence != after_editor.active_sequence.map(|v| v.0)
            || p.native_linked_selection != before_editor.linked_selection
            || p.human_command != command
            || p.human_params != *params
            || p.result != *result
        {
            return Err(error("computed native record differs from the exact captured snapshots, editor or result"));
        }
        super::collateral::validate(before, after, before_editor, &p.human_command, &p.human_params, &p.result)?;
        crate::multicam::production::validate_computed_transition(before, after, before_editor, after_editor, &p.human_command, &p.human_params, &p.result)?;
        let ids = p.files.iter().map(|f| f.item).collect::<Vec<_>>();
        let expected = file_proofs(before, &p.scope, &ids)?;
        if project_audit::value_bytes(&expected)? != project_audit::value_bytes(&p.files)? {
            return Err(error("computed native record original-generation evidence differs"));
        }
        for id in ids {
            if project_audit::value_bytes(&before.item(ItemId(id)))? != project_audit::value_bytes(&after.item(ItemId(id)))? {
                return Err(error("source production cannot rewrite its original admitted media"));
            }
        }
        Ok(())
    }
}
pub(super) fn seal(s: &Session, token: &str, candidate: &super::super::Staged, command: &str, params: &Value, result: &Value) -> Result<Sealed> {
    let ticket = s.agent_bridge.tickets.iter().find(|t| t.token == token).ok_or_else(|| error("computed record needs the actual qualified input ticket"))?;
    let owner = ticket.multicam_production.as_ref().ok_or_else(|| error("metadata/admission cannot supply computed-source record ownership"))?;
    super::current(s, token, &owner.request.scope)?;
    if !owner.attempted || owner.prepared.is_some() || ticket.committed.is_some() {
        return Err(error("computed record belongs only to this single native preparation attempt"));
    }
    let payload = Payload {
        kind: KIND.into(),
        version: 1,
        schema: filmcraft_format::SCHEMA_VERSION,
        scope: owner.request.scope.clone(),
        native_epoch: s.agent_bridge.epoch.clone().ok_or_else(|| error("computed record native epoch missing"))?,
        read_token: token.into(),
        revision: s.revision,
        project_path: s.path.clone().ok_or_else(|| error("computed record saved path missing"))?,
        before_project_sha256: project_plane::basis(s)?,
        after_project_sha256: project_plane::project_basis(&candidate.project)?,
        before_editor_sha256: editor(&s.state)?,
        after_editor_sha256: editor(&candidate.state)?,
        input_request_sha256: digest(&project_audit::value_bytes(&owner.request)?),
        input_projection_sha256: digest(&project_audit::value_bytes(&owner.projection)?),
        files: file_proofs(&s.project, &owner.request.scope, &owner.dependencies)?,
        human_command: command.into(),
        human_params: params.clone(),
        result: result.clone(),
        before_sequence: s.state.active_sequence.map(|v| v.0),
        after_sequence: candidate.state.active_sequence.map(|v| v.0),
        native_linked_selection: s.state.linked_selection,
        write_authority: false,
    };
    let checksum_sha256 = digest(&project_audit::value_bytes(&payload)?);
    let sealed = Sealed { data: Envelope { payload, checksum_sha256 }, before: Arc::downgrade(&s.project), after: Arc::downgrade(&candidate.project) };
    sealed.validate_live(s, candidate)?;
    sealed.data.bytes()?;
    Ok(sealed)
}
impl Sealed {
    pub(super) fn validate_live(&self, s: &Session, candidate: &super::super::Staged) -> Result<()> {
        if !self.before.ptr_eq(&Arc::downgrade(&s.project))
            || !self.after.ptr_eq(&Arc::downgrade(&candidate.project))
            || s.agent_bridge.epoch.as_ref() != Some(&self.data.payload.native_epoch)
            || s.path.as_ref() != Some(&self.data.payload.project_path)
            || s.revision != self.data.payload.revision
        {
            return Err(error("serialized computed data cannot replace the original live native producer"));
        }
        let ticket = super::current(s, &self.data.payload.read_token, &self.data.payload.scope)?;
        let owner = ticket.multicam_production.as_ref().ok_or_else(|| error("computed record qualified purpose missing"))?;
        self.data.validate_inputs(s, &owner.request, &owner.projection, &owner.dependencies)?;
        let result = candidate.results.first().filter(|_| candidate.results.len() == 1).ok_or_else(|| error("one computed native result required"))?;
        self.data.validate_content(
            &s.project,
            &candidate.project,
            &s.state,
            &candidate.state,
            &self.data.payload.human_command,
            &self.data.payload.human_params,
            result,
        )
    }
    pub(super) fn metadata(&self) -> Result<Value> {
        let bytes = self.data.bytes()?;
        Ok(
            json!({"kind":KIND,"version":1,"sha256":digest(&bytes),"bytes":bytes.len(),"writeAuthority":false,"sourceValidation":"not-performed","execution":"not-applied"}),
        )
    }
    pub(super) fn bytes(&self) -> Result<Vec<u8>> {
        self.data.bytes()
    }
}
impl Historical {
    pub(super) fn validate_result(&self, result: &Value) -> Result<()> {
        if &self.data.payload.result != result {
            return Err(error("historical native source result differs"));
        }
        Ok(())
    }

    pub(super) fn validate_origin(&self, request: &super::Request, projection: &Value, dependencies: &[u64]) -> Result<()> {
        self.data.validate_origin(request, projection, dependencies)
    }
    pub(super) fn validate_identity(&self, scope: &ProjectScope, path: &str, token: &str, revision: u64) -> Result<()> {
        self.data.validate_shape()?;
        let p = &self.data.payload;
        if &p.scope != scope || p.project_path != path || p.read_token != token || p.revision != revision {
            return Err(error("historical source record crosses its original receipt identity"));
        }
        Ok(())
    }
    pub(super) fn validate_original_content(&self, before: &Project, after: &Project, before_editor: &EditorState, after_editor: &EditorState) -> Result<()> {
        let p = &self.data.payload;
        self.data.validate_content(before, after, before_editor, after_editor, &p.human_command, &p.human_params, &p.result)
    }

    pub(super) fn decode(raw: &[u8]) -> Result<Self> {
        if raw.is_empty() || raw.len() > MAX_BYTES {
            return Err(error("bounded complete computed native record required"));
        }
        let data: Envelope = serde_json::from_slice(raw).map_err(|e| error(e.to_string()))?;
        data.validate_shape()?;
        if data.bytes()? != raw {
            return Err(error("computed native record is not its exact canonical encoding"));
        }
        Ok(Self { data })
    }
    pub(super) fn validate_inputs(&self, s: &Session, request: &super::Request, projection: &Value, dependencies: &[u64]) -> Result<()> {
        self.data.validate_inputs(s, request, projection, dependencies)
    }
    pub(super) fn validate_captured(
        &self,
        before: &Project,
        after: &Project,
        before_editor: &EditorState,
        after_editor: &EditorState,
        command: &str,
        params: &Value,
        result: &Value,
    ) -> Result<()> {
        self.data.validate_content(before, after, before_editor, after_editor, command, params, result)
    }
    pub(super) fn metadata(&self) -> Result<Value> {
        let bytes = self.data.bytes()?;
        Ok(
            json!({"kind":KIND,"version":1,"sha256":digest(&bytes),"bytes":bytes.len(),"writeAuthority":false,"sourceValidation":"not-performed","execution":"not-applied"}),
        )
    }
}

#[cfg(test)]
pub(super) fn canonical_reseal_for_test(value: &Value) -> Result<Vec<u8>> {
    let mut data: Envelope = serde_json::from_value(value.clone()).map_err(|e| error(e.to_string()))?;
    data.checksum_sha256 = digest(&project_audit::value_bytes(&data.payload)?);
    data.bytes()
}
