//! The actual cold runtime owns its original canonical Commit. Descriptions
//! never reconstruct it, and no transition releases editing or the worker.
use super::*;

pub(super) struct Canonical {
    pub(super) commit: Arc<workspace_update::ColdCommit>,
    plan: Value,
    intent_attempted: bool,
    intent_confirmed: bool,
    publication_attempted: bool,
    result: Option<Value>,
    observation_attempted: bool,
    observation_confirmed: bool,
    binding_result: Option<Value>,
    binding_observation_attempted: bool,
    binding_observation_confirmed: bool,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct IntentReceipt {
    kind: String,
    version: u8,
    plan: Value,
    anchor: Observed,
    head: Observed,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ObservationReceipt {
    kind: String,
    version: u8,
    plan: Value,
    result: Value,
    anchor: Observed,
    head: Observed,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct BindingObservationReceipt {
    kind: String,
    version: u8,
    plan: Value,
    result: Value,
    bindings: Value,
    anchor: Observed,
    head: Observed,
}
impl Canonical {
    pub(super) fn retained_published(&self) -> Result<Arc<workspace_update::ColdCommit>> {
        self.transition_current()?;
        if !self.intent_confirmed
            || !self.publication_attempted
            || !self.observation_confirmed
            || self.result.as_ref().is_none_or(|v| v["published"] != true)
            || !self.binding_observation_confirmed
            || self.binding_result.as_ref().is_none_or(|v| v["bound"] != true)
        {
            return Err(error("the actual original canonical publication and service-binding acknowledgements are required before sharing"));
        }
        Ok(self.commit.clone())
    }
    fn binding_receipt(&self, value: &Value) -> Result<BindingObservationReceipt> {
        if !self.observation_confirmed || self.commit.service_binding_description().is_none_or(|v| v["bound"] != true) {
            return Err(error("actual original bound canonical owner required"));
        }
        let result = self.result.as_ref().filter(|v| v["published"] == true).ok_or_else(|| error("actual complete canonical result required"))?;
        let bindings = self.binding_result.as_ref().ok_or_else(|| error("actual original service binding result required"))?;
        bytes(value, 768 * 1024)?;
        let receipt: BindingObservationReceipt = parse("native.coldSaveServiceBindings", value)?;
        if receipt.kind != "poema-original-Save-native-service-bindings-receipt"
            || receipt.version != 1
            || receipt.plan != self.plan
            || &receipt.result != result
            || &receipt.bindings != bindings
        {
            return Err(error("exact original native service binding observation receipt required"));
        }
        Ok(receipt)
    }
    pub(super) fn transition_current(&self) -> Result<()> {
        if self.intent_attempted && !self.intent_confirmed
            || self.publication_attempted && self.result.is_none()
            || self.observation_attempted && !self.observation_confirmed
            || self.binding_observation_attempted && !self.binding_observation_confirmed
            || self.result.as_ref().is_some_and(|r| r["published"] != true)
        {
            return Err(error("cold canonical publication is unresolved; retain the actual owner without replay"));
        }
        Ok(())
    }
    pub(super) fn description(&self) -> Value {
        let mut value = json!({"plan":self.plan,"intentConfirmed":self.intent_confirmed,
            "publicationAttempted":self.publication_attempted,"result":self.result,
            "observationConfirmed":self.observation_confirmed,
            "editingPaused":true,"workersMayResume":false,"writeAuthority":false,"startupAuthority":false});
        if let Some(bindings) = self.commit.service_binding_description() {
            value["serviceBindings"] = bindings;
            value["bindingObservationAttempted"] = json!(self.binding_observation_attempted);
            value["bindingObservationConfirmed"] = json!(self.binding_observation_confirmed);
        }
        value
    }
}
impl Pending {
    /// Only reject already-used actual continuations. No JSON, source read,
    /// baseline replacement or startup/write authority is accepted here.
    pub(super) fn refuse_repeated_canonical_effect(&self, action: crate::project_save::operation::CanonicalAction<'_>) -> Result<()> {
        use crate::project_save::operation::CanonicalAction;
        match action {
            CanonicalAction::Prepare if self.canonical.is_some() => Err(error("canonical preparation already exists; it cannot be reissued")),
            CanonicalAction::Publish if self.canonical.as_ref().is_some_and(|c| c.publication_attempted) => {
                Err(error("one confirmed original canonical intent and unused actual Commit required"))
            }
            CanonicalAction::BindServices if self.canonical.as_ref().is_some_and(|c| c.commit.service_binding_description().is_some()) => {
                Err(error("original cold service binding was already attempted; retain its actual owner without replay"))
            }
            _ => Ok(()),
        }
    }
    pub(super) fn bind_canonical_services(&mut self, s: &mut Session) -> Result<Value> {
        let c = self.canonical.as_ref().ok_or_else(|| error("actual original canonical preparation required"))?;
        c.transition_current()?;
        let WorkerState::Created { journal, confirmed: true, .. } = &self.worker else {
            return Err(error("the same original confirmed gated worker required before binding"));
        };
        if !c.commit.same_original_journal(journal) {
            return Err(error("binding crossed the original retained worker journal"));
        }
        if !c.observation_confirmed || c.result.as_ref().is_none_or(|result| result["published"] != true) {
            return Err(error("exact complete original canonical observation must be confirmed before binding"));
        }
        let original = self.original.clone();
        let successor = self.successor.clone();
        let preference = c.commit.preference();
        let check = media_guard::Check::new(web_time::Instant::now());
        let io = || original.current_with_runtime(&check, successor.as_ref(), Some(&preference)).map_err(|e| io::Error::other(e.to_string()));
        let c = self.canonical.as_mut().ok_or_else(|| error("actual canonical owner disappeared"))?;
        // The original Commit fences the complete source, actual worker,
        // canonical files and memory immediately before and after binding.
        // Do not repeat those read-only fences above this same consuming call.
        let result =
            Arc::get_mut(&mut c.commit).ok_or_else(|| error("the actual canonical owner is already shared; no binding replay"))?.bind_services(s, &io)?;
        c.binding_result = Some(result.clone());
        Ok(result)
    }
    pub(super) fn confirm_service_binding_observation(&mut self, s: &Session, value: &Value) -> Result<Value> {
        let receipt = {
            let c = self.canonical.as_mut().ok_or_else(|| error("actual original canonical owner required"))?;
            if c.binding_observation_attempted
                || !c.observation_confirmed
                || c.binding_result.is_none()
                || c.commit.service_binding_description().is_none_or(|v| v["bound"] != true)
            {
                return Err(error("one actual original bound service continuation and unused observation required"));
            }
            c.binding_observation_attempted = true;
            c.binding_receipt(value)?
        };
        // Read only the sole successor of the original retained controls.
        let check = media_guard::Check::new(web_time::Instant::now());
        let next = self.read_published_successor(
            &receipt.anchor,
            &receipt.head,
            json!({"kind":"native-service-bindings-observation","plan":receipt.plan,"result":receipt.result,"bindings":receipt.bindings}),
            &check,
        )?;
        self.successor = Some(next);
        self.worker_current(s.persistence.as_ref())?;
        let io = || self.source_current().map_err(|e| io::Error::other(e.to_string()));
        self.canonical.as_ref().ok_or_else(|| error("actual canonical owner disappeared"))?.commit.verify_published(s, &io)?;
        self.source_current()?;
        self.canonical.as_mut().ok_or_else(|| error("actual canonical owner disappeared"))?.binding_observation_confirmed = true;
        self.current()?;
        Ok(self.canonical.as_ref().ok_or_else(|| error("actual canonical owner disappeared"))?.description())
    }
    pub(super) fn check_canonical(&self, s: &Session) -> Result<()> {
        self.application.current_checkpoint(s)?;
        let Some(c) = &self.canonical else {
            return Ok(());
        };
        c.transition_current()?;
        let check = || self.source_current().map_err(|e| io::Error::other(e.to_string()));
        if c.publication_attempted { c.commit.verify_published(s, &check) } else { c.commit.prepare_description(s, &check).map(|_| ()) }
    }
    pub(super) fn prepare_canonical_publication(&mut self, s: &Session) -> Result<Value> {
        self.current()?;
        self.worker_current(s.persistence.as_ref())?;
        if self.canonical.is_some() {
            return Err(error("canonical preparation already exists; it cannot be reissued"));
        }
        let WorkerState::Created { description, confirmed: true, .. } = &self.worker else {
            return Err(error("the actual original worker receipt must be confirmed first"));
        };
        let check = || self.source_current().map_err(|e| io::Error::other(e.to_string()));
        let root = DirectoryFrame::capture(&self.cfg.data_dir, &check).map_err(error)?;
        let commit = workspace_update::ColdCommit::prepare(s, root, &self.original.body.activation_token, &check)?;
        let effects = commit.prepare_description(s, &check)?;
        let input = self.original.input.observation()?;
        let private_input = json!({"path":self.original.input_directory.record().path.join("input.json"),
            "sha256":input.sha256,"bytes":input.bytes,"physical":{"dev":input.identity.device,
            "ino":input.identity.inode,"mode":input.identity.mode.to_string(),"uid":input.identity.uid.to_string(),
            "size":input.bytes.to_string(),"mtimeNs":nanos(&input.modified)?,"ctimeNs":nanos(&input.changed)?}});
        let plan = json!({"kind":"poema-native-Save-canonical-preparation","version":1,
            "preparation":self.preparation,"worker":description,"privateInput":private_input,"effects":effects,
            "editingPaused":true,"workersMayResume":false,"startupAuthority":false,"writeAuthority":false,"fullApplicationClosure":false});
        bytes(&plan, 512 * 1024)?;
        self.canonical = Some(Canonical {
            commit: Arc::new(commit),
            plan: plan.clone(),
            intent_attempted: false,
            intent_confirmed: false,
            publication_attempted: false,
            result: None,
            observation_attempted: false,
            observation_confirmed: false,
            binding_result: None,
            binding_observation_attempted: false,
            binding_observation_confirmed: false,
        });
        self.check_canonical(s)?;
        Ok(plan)
    }
    pub(super) fn confirm_canonical_publication_intent(&mut self, value: &Value) -> Result<Value> {
        let c = self.canonical.as_mut().ok_or_else(|| error("actual original canonical preparation required"))?;
        if c.intent_attempted {
            return Err(error("canonical intent confirmation already attempted; no replay"));
        }
        c.intent_attempted = true;
        let expected = c.plan.clone();
        bytes(value, 768 * 1024)?;
        let receipt: IntentReceipt = parse("native.coldSaveCanonicalIntent", value)?;
        if receipt.kind != "poema-original-Save-canonical-intent-receipt" || receipt.version != 1 || receipt.plan != expected {
            return Err(error("exact original canonical intent receipt required"));
        }
        let check = media_guard::Check::new(web_time::Instant::now());
        let next = self.read_published_successor(&receipt.anchor, &receipt.head, json!({"kind":"native-canonical-intent","plan":expected}), &check)?;
        self.successor = Some(next);
        self.canonical.as_mut().ok_or_else(|| error("canonical owner disappeared"))?.intent_confirmed = true;
        self.current()?;
        Ok(self.canonical.as_ref().ok_or_else(|| error("canonical owner disappeared"))?.description())
    }
    pub(super) fn publish_canonical_publication(&mut self, s: &Session) -> Result<Value> {
        self.current()?;
        self.worker_current(s.persistence.as_ref())?;
        self.check_canonical(s)?;
        let c = self.canonical.as_mut().ok_or_else(|| error("actual original canonical preparation required"))?;
        if !c.intent_confirmed || c.publication_attempted {
            return Err(error("one confirmed original canonical intent and unused actual Commit required"));
        }
        c.publication_attempted = true;
        let original = self.original.clone();
        let successor = self.successor.clone();
        let preference = c.commit.preference();
        let check = media_guard::Check::new(web_time::Instant::now());
        let io = || original.current_with_runtime(&check, successor.as_ref(), Some(&preference)).map_err(|e| io::Error::other(e.to_string()));
        let result =
            Arc::get_mut(&mut c.commit).ok_or_else(|| error("the actual canonical owner is already shared; no publication replay"))?.publish(s, &io)?;
        c.result = Some(result.clone());
        // A partial result is returned for the original host's durable evidence
        // writer. Its actual native owner remains stopped and cannot be replayed.
        Ok(result)
    }
    pub(super) fn confirm_canonical_publication_observation(&mut self, value: &Value) -> Result<Value> {
        let c = self.canonical.as_mut().ok_or_else(|| error("actual canonical publication required"))?;
        if c.observation_attempted || !c.publication_attempted {
            return Err(error("one original canonical observation confirmation required"));
        }
        c.observation_attempted = true;
        let expected_plan = c.plan.clone();
        let expected_result = c.result.clone().ok_or_else(|| error("actual canonical result is unresolved"))?;
        bytes(value, 768 * 1024)?;
        let receipt: ObservationReceipt = parse("native.coldSaveCanonicalObservation", value)?;
        if receipt.kind != "poema-original-Save-canonical-observation-receipt"
            || receipt.version != 1
            || receipt.plan != expected_plan
            || receipt.result != expected_result
        {
            return Err(error("exact original canonical observation receipt required"));
        }
        let check = media_guard::Check::new(web_time::Instant::now());
        let next = self.read_published_successor(
            &receipt.anchor,
            &receipt.head,
            json!({"kind":"native-canonical-observation","plan":expected_plan,"result":expected_result}),
            &check,
        )?;
        self.successor = Some(next);
        self.canonical.as_mut().ok_or_else(|| error("canonical owner disappeared"))?.observation_confirmed = true;
        self.current()?;
        Ok(self.canonical.as_ref().ok_or_else(|| error("canonical owner disappeared"))?.description())
    }
}

#[cfg(test)]
mod tests;
