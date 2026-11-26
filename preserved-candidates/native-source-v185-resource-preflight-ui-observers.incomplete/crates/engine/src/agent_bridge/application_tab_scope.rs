//! Nonserializable original-native read ownership for application tab preparation.
//! Read ownership can prepare only one finite application-tab action under the
//! original native Stop fence. Bytes cannot construct this owner or permit.
//! Readonly receipts retain native snapshots,never a new media/history writer.
pub use super::history_checkpoint::application_tabs::ApplicationTabHistory;
use crate::{Result, Session, commands::bad};
use serde_json::{Value, json};
use std::sync::Arc;

pub struct ApplicationTabScope {
    token: String,
    original: Value,
    physical: Arc<()>,
    session_owner: super::operation_control::AgentOperationControl,
    origin: Snapshot,
}
fn error(value: impl Into<String>) -> crate::EngineError {
    bad("native application tab preparation", value)
}
impl ApplicationTabScope {
    /// A correlation identity is descriptive; only the private scope below owns
    /// its actual native read. Generating this string grants no authority.
    pub fn new_instance_identity() -> Result<String> {
        let mut raw = [0u8; 16];
        getrandom::fill(&mut raw).map_err(|e| error(e.to_string()))?;
        Ok(raw.iter().map(|b| format!("{b:02x}")).collect())
    }
    pub fn document(&self) -> &Value {
        &self.original
    }
    pub fn token(&self) -> &str {
        &self.token
    }
    pub fn observe(&self, session: &mut Session) -> Result<Value> {
        if session.is_display_only() {
            return Err(error("a display projection cannot own application tab preparation"));
        }
        {
            let ticket =
                session.agent_bridge.tickets.iter().find(|t| t.token == self.token).ok_or_else(|| error("original native application read expired"))?;
            let owner = ticket.sequence_views.as_ref().ok_or_else(|| error("native application view purpose missing"))?;
            let physical =
                owner.application_scope.as_ref().and_then(std::sync::Weak::upgrade).ok_or_else(|| error("no live physical application read owner"))?;
            if owner.change || ticket.committed.is_some() || !Arc::ptr_eq(&physical, &self.physical) || owner.original != self.original {
                return Err(error("foreign,consumed or historical native application read"));
            }
        }
        session.execute(
            "agent.observeSequenceViews",
            json!({"token":self.token,"scope":self.original["scope"],
            "beforeSha256":self.original["projectState"]["sha256"],"beforeEditorSha256":self.original["editorStateSha256"]}),
        )
    }
}
impl Session {
    /// Called by the owning application, not a serialized control command.
    /// The original readonly view Ticket and exact native clock do all native
    /// validation. Its ordinary writer capability stays false.
    pub fn prepare_application_tab_scope(&mut self, params: &Value) -> Result<ApplicationTabScope> {
        if self.is_display_only() {
            return Err(error("a display projection cannot prepare application tabs"));
        }
        let original = self.execute("agent.readSequenceViews", params.clone())?;
        let token = original["token"].as_str().ok_or_else(|| error("native application read token missing"))?.to_string();
        let physical = Arc::new(());
        let ticket = self.agent_bridge.tickets.iter_mut().find(|t| t.token == token).ok_or_else(|| error("native application read was not retained"))?;
        let owner = ticket.sequence_views.as_mut().ok_or_else(|| error("native application view owner missing"))?;
        if owner.change || owner.application_scope.is_some() || ticket.committed.is_some() || owner.original != original {
            return Err(error("only a fresh original readonly native view may prepare application scope"));
        }
        owner.application_scope = Some(Arc::downgrade(&physical));
        Ok(ApplicationTabScope { token, original, physical, session_owner: self.agent_operation_control(), origin: Snapshot::capture(self)? })
    }
}

const LIMIT: usize = 4 * 1024 * 1024;
const COMMAND: &str = "native application tab activation";
pub(super) struct Snapshot {
    pub(super) project: Arc<filmcraft_project::Project>,
    pub(super) history: crate::History,
    pub(super) editor: Value,
    pub(super) clipboard: String,
    pub(super) native: Option<super::native_editor::Snapshot>,
    pub(super) native_error: Option<String>,
    pub(super) path: Option<String>,
    pub(super) epoch: String,
    pub(super) revision: u64,
    pub(super) saved_revision: u64,
    pub(super) forecast: Option<crate::trim::command_settlement::historical::Capture>,
}
impl Snapshot {
    fn capture(s: &Session) -> Result<Self> {
        let editor = serde_json::to_value(&s.state).map_err(|e| error(e.to_string()))?;
        if serde_json::to_vec(&editor).map_err(|e| error(e.to_string()))?.len() > LIMIT {
            return Err(error("complete native editor exceeds application tab bound"));
        }
        let mut result = Self::metadata_snapshot(s, editor)?;
        let prepared = crate::trim::command_settlement::prepare(s, "sequence.open")?;
        result.forecast = Some(prepared.capture_history()?);
        result.capture_native(s);
        if result.retained_bytes()? > 2 * LIMIT {
            return Err(error("complete native snapshot evidence exceeds8MiB"));
        }
        Ok(result)
    }
    fn metadata_snapshot(s: &Session, editor: Value) -> Result<Self> {
        Ok(Self {
            project: s.project.clone(),
            history: s.history.clone(),
            editor,
            clipboard: super::project_clear::clipboard_hash(&s.state)?,
            native: None,
            native_error: None,
            path: s.path.clone(),
            epoch: s.agent_bridge.epoch.clone().ok_or_else(|| error("native application epoch missing"))?,
            revision: s.revision,
            saved_revision: s.saved_revision,
            forecast: None,
        })
    }
    // Readonly current values use the original metadata encoder. No Following,
    // settlement forecast, native read ticket or UI owner is created.
    pub(super) fn current_metadata(s: &Session) -> Result<Value> {
        let editor = serde_json::to_value(&s.state).map_err(|e| error(e.to_string()))?;
        if serde_json::to_vec(&editor).map_err(|e| error(e.to_string()))?.len() > LIMIT {
            return Err(error("complete native editor exceeds application tab bound"));
        }
        Self::metadata_snapshot(s, editor)?.metadata()
    }
    fn retained_bytes(&self) -> Result<usize> {
        let editor = serde_json::to_vec(&self.editor).map_err(|e| error(e.to_string()))?.len();
        let forecast = self.forecast.as_ref().map(|f| f.evidence().encode()).transpose()?.map_or(0, |v| v.len());
        let native = self.native.as_ref().map(|n| super::history_checkpoint::bytes(n, 2 * LIMIT)).transpose()?.map_or(0, |v| v.len());
        let missing = self.native_error.as_ref().map(|v| serde_json::to_vec(v).map(|v| v.len()).map_err(|e| error(e.to_string()))).transpose()?.unwrap_or(0);
        total_bytes(&[editor, forecast, native, missing])
    }
    // Use the original native editor/auxiliary codec. Busy journals retain raw
    // clipboard/preferences with auxiliary=None; failure remains explicit data.
    // This sidecar neither adds a live action dependency nor grants a writer.
    fn capture_native(&mut self, s: &Session) {
        self.native = None;
        self.native_error = None;
        let captured = (|| -> Result<super::native_editor::Snapshot> {
            let remaining = (2 * LIMIT)
                .checked_sub(self.retained_bytes()?)
                .and_then(|n| n.checked_sub(8192))
                .ok_or_else(|| error("raw native editor does not fit original8MiB snapshot bound"))?;
            let native = super::native_editor::Snapshot::capture_journal(s)?;
            super::history_checkpoint::bytes(&native, remaining)?;
            Ok(native)
        })();
        match captured {
            Ok(native) => self.native = Some(native),
            Err(e) => self.native_error = Some(e.to_string().chars().take(512).collect()),
        }
    }
    pub(super) fn metadata(&self) -> Result<Value> {
        let editor: crate::EditorState = serde_json::from_value(self.editor.clone()).map_err(|e| error(e.to_string()))?;
        Ok(json!({"nativeSession":self.epoch,"projectPath":self.path,"revision":self.revision,"savedRevision":self.saved_revision,
            "sequence":editor.active_sequence.map(|id|id.0),"projectSha256":super::project_plane::project_basis(&self.project)?,
            "editorSha256":super::project_clear::editor_hash_with(&editor,&self.clipboard)?,"history":super::compound_history::history_state(&self.project,&self.history)?}))
    }
    fn validate(&self, s: &Session) -> Result<()> {
        if s.is_display_only()
            || !Arc::ptr_eq(&self.project, &s.project)
            || !crate::trim::command_settlement::historical::same_history(&self.history, &s.history)
            || self.editor != serde_json::to_value(&s.state).map_err(|e| error(e.to_string()))?
            || self.clipboard != super::project_clear::clipboard_hash(&s.state)?
            || self.path != s.path
            || s.agent_bridge.epoch.as_ref() != Some(&self.epoch)
            || self.revision != s.revision
            || self.saved_revision != s.saved_revision
        {
            return Err(error("native application tab state differs from its exact original human result"));
        }
        Ok(())
    }
}
/// Prepared expected effect only. It cannot execute a command; entering its
/// finite original native operation checks the complete current state again.
pub struct ApplicationTabPreparedActivation {
    scope: ApplicationTabScope,
    before: Snapshot,
    after: Snapshot,
    prepared: crate::trim::command_settlement::Prepared,
    result: Value,
    switched: bool,
}
pub struct ApplicationTabPublication {
    action: ApplicationTabPreparedActivation,
    operation: super::operation_control::ApplicationTabOperation,
}
/// Readonly evidence retains all original/current/published native snapshot
/// roots,including zero-history trim roots. No method imports a live writer.
pub struct ApplicationTabReceipt {
    value: Value,
    confirmed: bool,
    _actual: Option<Snapshot>,
    _scope: ApplicationTabScope,
    _before: Snapshot,
    _after: Snapshot,
}
impl ApplicationTabScope {
    pub fn retained_bytes(&self) -> Result<usize> {
        self.origin
            .retained_bytes()?
            .checked_add(serde_json::to_vec(&self.original).map_err(|e| error(e.to_string()))?.len())
            .ok_or_else(|| error("application scope size overflow"))
    }
    pub fn prepare_activation(self, session: &mut Session, item: u64) -> Result<ApplicationTabPreparedActivation> {
        if !(1..=9_007_199_254_740_991).contains(&item) {
            return Err(error("exact observed native tab item required"));
        }
        let observed = self.observe(session)?;
        let listed = |d: &Value| d["openTabs"].as_array().is_some_and(|rows| rows.iter().any(|r| r["item"].as_u64() == Some(item)));
        if !listed(&self.original)
            || !listed(&observed["current"])
            || !session.state.open_sequences.contains(&filmcraft_project::ItemId(item))
            || session.project.sequence(filmcraft_project::ItemId(item)).is_none()
        {
            return Err(error("application activation requires the same actually observed open native tab"));
        }
        let prepared = {
            let ticket = session.agent_bridge.tickets.iter().find(|t| t.token == self.token).ok_or_else(|| error("native application ticket expired"))?;
            ticket
                .sequence_views
                .as_ref()
                .ok_or_else(|| error("native application purpose missing"))?
                .current(session, crate::trim::clock::SequenceViewCommand::Open)?
        };
        let before = Snapshot::capture(session)?;
        let switched = session.state.active_sequence != Some(filmcraft_project::ItemId(item));
        let mut editor: crate::EditorState = serde_json::from_value(before.editor.clone()).map_err(|e| error(e.to_string()))?;
        let mut after = Snapshot {
            project: before.project.clone(),
            history: before.history.clone(),
            editor: before.editor.clone(),
            clipboard: before.clipboard.clone(),
            native: before.native.clone(),
            native_error: before.native_error.clone(),
            path: before.path.clone(),
            epoch: before.epoch.clone(),
            revision: before.revision,
            saved_revision: before.saved_revision,
            forecast: None,
        };
        let summary = if switched {
            session.require_command_owner("sequence.open")?;
            let spec = crate::commands::find("sequence.open").ok_or_else(|| error("original sequence.open missing"))?;
            session.enabled_for(spec, &json!({"item":item})).map_err(error)?;
            let capture = prepared.capture_history()?;
            let h = capture.historical();
            after.project = h.after_project().clone();
            after.history = h.after_history().clone();
            editor.trim_shift = h.trim_shift_after();
            crate::commands::open_sequence_state(&after.project, &mut editor, filmcraft_project::ItemId(item))?;
            after.editor = serde_json::to_value(&editor).map_err(|e| error(e.to_string()))?;
            after.forecast = Some(capture);
            prepared.summary().clone()
        } else {
            crate::trim::command_settlement::Summary::default()
        };
        if let Some(native) = &mut after.native {
            native.editor = editor;
            native.merge_key = after.history.merge_key.clone();
        }
        let value = historical_result(&self.original, &before, &after, &summary, item, switched)?;
        Ok(ApplicationTabPreparedActivation { scope: self, before, after, prepared, result: value, switched })
    }
}
impl ApplicationTabPreparedActivation {
    pub fn document(&self) -> &Value {
        &self.result
    }
    pub fn retained_bytes(&self) -> Result<usize> {
        total_bytes(&[
            self.scope.retained_bytes()?,
            self.before.retained_bytes()?,
            self.after.retained_bytes()?,
            serde_json::to_vec(&self.result).map_err(|e| error(e.to_string()))?.len(),
        ])
    }
    pub fn enter_publication(self, session: &mut Session) -> Result<ApplicationTabPublication> {
        self.scope.observe(session)?;
        self.before.validate(session)?;
        self.prepared.validate(session)?;
        let operation = session.agent_bridge.operations.begin_application_tab(&self.scope.token)?;
        Ok(ApplicationTabPublication { action: self, operation })
    }
}
impl ApplicationTabPublication {
    fn retain(mut self, session: &Session, failed: Option<String>) -> ApplicationTabReceipt {
        let confirmed = failed.is_none();
        // Primary native result is already validated by the original service.
        // Retain the actual successful raw editor instead of inferring it from
        // preparation (preferences/auxiliary data may have legitimately changed).
        if confirmed {
            self.action.after.capture_native(session);
        }
        let (actual, actual_error) = if confirmed {
            (None, None)
        } else {
            match Snapshot::capture(session) {
                Ok(s) => (Some(s), None),
                Err(e) => (None, Some(e.to_string().chars().take(512).collect::<String>())),
            }
        };
        let value = match failed {
            None => self.action.result,
            Some(message) => {
                json!({"kind":"native_application_tab_uncertain","version":1,"status":"unknown","expected":self.action.result,"error":message.chars().take(512).collect::<String>(),"actualSnapshotRetained":actual.is_some(),"actualSnapshotError":actual_error,"canReplay":false,"mediaEvidence":false,"writeAuthority":false})
            }
        };
        let receipt =
            ApplicationTabReceipt { value, confirmed, _actual: actual, _scope: self.action.scope, _before: self.action.before, _after: self.action.after };
        if confirmed {
            self.operation.complete();
        }
        receipt
    }
    pub fn abandon(self, session: &Session, message: String) -> ApplicationTabReceipt {
        self.retain(session, Some(message))
    }
    pub fn finish(self, session: &Session) -> ApplicationTabReceipt {
        let valid = self.action.after.validate(session).and_then(|()| {
            if !self.action.switched {
                self.action.prepared.validate(session)
            } else if session.trim_play.active() {
                Err(error("original tab switch did not finish its original human trim"))
            } else {
                Ok(())
            }
        });
        self.retain(session, valid.err().map(|e| e.to_string()))
    }
}
impl ApplicationTabReceipt {
    pub fn confirmed(&self) -> bool {
        self.confirmed
    }
    pub fn document(&self) -> &Value {
        &self.value
    }
    pub fn retained_bytes(&self) -> Result<usize> {
        total_bytes(&[
            self._scope.retained_bytes()?,
            self._before.retained_bytes()?,
            self._after.retained_bytes()?,
            self._actual.as_ref().map(Snapshot::retained_bytes).transpose()?.unwrap_or(0),
            serde_json::to_vec(&self.value).map_err(|e| error(e.to_string()))?.len(),
        ])
    }
}

fn total_bytes(values: &[usize]) -> Result<usize> {
    values.iter().try_fold(0usize, |n, v| n.checked_add(*v).ok_or_else(|| error("complete native application evidence size overflow")))
}

pub(super) fn historical_result(
    original: &Value,
    before: &Snapshot,
    after: &Snapshot,
    summary: &crate::trim::command_settlement::Summary,
    item: u64,
    switched: bool,
) -> Result<Value> {
    let before_editor: crate::EditorState = serde_json::from_value(before.editor.clone()).map_err(|e| error(e.to_string()))?;
    let editor: crate::EditorState = serde_json::from_value(after.editor.clone()).map_err(|e| error(e.to_string()))?;
    let proof = super::editor_proof::encode(&before_editor, &editor, &before.clipboard, "native_application_tab_editor_proof", LIMIT, COMMAND)?;
    let value = json!({"kind":"native_application_tab_result","version":1,"status":"activated","token":original["token"],"scope":original["scope"],"item":item,"switched":switched,
        "originalNativeReadSha256":super::history_checkpoint::digest(&serde_json::to_vec(&original).map_err(|e|error(e.to_string()))?),"before":before.metadata()?,"after":after.metadata()?,
        "editorProof":proof,"humanCommand":if switched{Some("sequence.open")}else{None},"humanTrim":super::editor_control::settlement_json(&summary),
        "agentTimelineCommits":0,"agentUndoOwned":false,"sourceValidation":"not-performed","mediaEvidence":false,"writeAuthority":false});
    if serde_json::to_vec(&value).map_err(|e| error(e.to_string()))?.len() > LIMIT - 8192 {
        return Err(error("complete native application tab receipt exceeds4MiB"));
    }
    Ok(value)
}

pub(super) struct HistoryParts<'a> {
    pub(super) original: &'a Value,
    pub(super) origin: &'a Snapshot,
    pub(super) before: &'a Snapshot,
    pub(super) after: &'a Snapshot,
    pub(super) actual: Option<&'a Snapshot>,
}
impl ApplicationTabReceipt {
    pub(super) fn history_parts(&self) -> HistoryParts<'_> {
        HistoryParts { original: &self._scope.original, origin: &self._scope.origin, before: &self._before, after: &self._after, actual: self._actual.as_ref() }
    }
    /// Historical immutable graph only. It cannot restore a live application scope.
    pub fn capture_history(&self) -> Result<ApplicationTabHistory> {
        super::history_checkpoint::application_tabs::capture(self)
    }
}

impl ApplicationTabReceipt {
    pub(super) fn validate_original_capture_owner(&self, session: &Session) -> Result<()> {
        let root = crate::project_workspace::identity(&session.project)?.ok_or_else(|| error("application Root identity missing"))?;
        let root = serde_json::to_value(root.scope).map_err(|e| error(e.to_string()))?;
        if session.is_display_only()
            || !self._scope.session_owner.same_original_owner(&session.agent_operation_control())
            || session.agent_bridge.epoch.as_ref() != Some(&self._scope.origin.epoch)
            || self._scope.original.get("scope") != Some(&root)
        {
            return Err(error("application history capture requires the same original physical Session and Root"));
        }
        Ok(())
    }
}
