//! Sole original journal successors for the SAME retained native presentation.
//! These acknowledgements retain physical witnesses, never release a worker.
use super::*;

#[derive(Default)]
pub(super) struct Continuation {
    intent_attempted: bool,
    intent_confirmed: bool,
    publication_attempted: bool,
    publication_confirmed: bool,
}
impl Continuation {
    pub(super) fn checkpoint_ready(&self) -> Result<()> {
        self.resolved()?;
        if !self.intent_confirmed || !self.publication_confirmed {
            return Err(error("the SAME original native-presentation publication must be acknowledged before checkpoint encoding"));
        }
        Ok(())
    }
    pub(super) fn resolved(&self) -> Result<()> {
        if self.intent_attempted && !self.intent_confirmed || self.publication_attempted && !self.publication_confirmed {
            return Err(error("the original application-record continuation is unresolved; no replay or release"));
        }
        Ok(())
    }
    fn begin_intent(&mut self) -> Result<()> {
        if self.intent_attempted {
            return Err(error("the original application-record intent confirmation was already attempted"));
        }
        self.intent_attempted = true;
        Ok(())
    }
    fn begin_publication(&mut self) -> Result<()> {
        if !self.intent_confirmed || self.publication_attempted {
            return Err(error("one same-owner confirmed application-record intent and unused publication observation are required"));
        }
        self.publication_attempted = true;
        Ok(())
    }
}
fn compact_plan(plan: &Value) -> Result<Value> {
    if !fields(
        plan,
        "kind,version,nativePid,nativeSession,privateInput,manifest,catalog,activity,nativeState,editingPaused,workerReleased,writeAuthority,fullApplicationClosure",
    ) || plan["kind"] != "poema-native-Save-application-records-plan"
        || plan["version"] != 1
        || !fields(&plan["catalog"], "observation,value")
        || !fields(&plan["activity"], "observation,value")
        || plan["editingPaused"] != true
        || !false_fields(plan, "workerReleased,writeAuthority,fullApplicationClosure")
    {
        return Err(error("the closed original held application-record plan is required"));
    }
    let mut value = plan.clone();
    let object = value.as_object_mut().ok_or_else(|| error("the retained native record plan must remain an object"))?;
    object.insert("kind".into(), json!("poema-native-Save-application-records-publication-plan"));
    object.insert("catalog".into(), plan["catalog"]["observation"].clone());
    object.insert("activity".into(), plan["activity"]["observation"].clone());
    bytes(&value, CATALOG as usize)?;
    Ok(value)
}
fn published_native(value: &Value, plan: &Value) -> Result<Observed> {
    if !fields(value, "kind,version,plan,nativeState,editingPaused,workerReleased,writeAuthority,fullApplicationClosure")
        || value["kind"] != "poema-original-Save-native-presentation-publication"
        || value["version"] != 1
        || value["plan"] != compact_plan(plan)?
        || value["editingPaused"] != true
        || !false_fields(value, "workerReleased,writeAuthority,fullApplicationClosure")
    {
        return Err(error("the same original native presentation publisher observation is required"));
    }
    let observed: Observed = parse("native.coldSaveApplicationRecordsPublication", &value["nativeState"])?;
    if json!(observed.path) != plan["nativeState"]["path"] || observed.bytes == 0 || observed.bytes > 4096 {
        return Err(error("the actual native presentation crossed its original path or complete byte bound"));
    }
    Ok(observed)
}
fn confirmation(publication: bool, epoch: &Value, receipt: &IntentReceipt, native_state: Option<&Observed>) -> Value {
    json!({"kind":if publication {"native_cold_Save_application_records_publication_confirmation"} else {"native_cold_Save_application_records_intent_confirmation"},
        "version":1,"nativePid":std::process::id(),"nativeSession":epoch,
        "event":receipt.event,"anchor":receipt.anchor,"head":receipt.head,"nativeState":native_state,
        "nativeAcknowledgementConfirmed":true,"editingPaused":true,"workerReleased":false,
        "writeAuthority":false,"startupAuthority":false,"workersMayResume":false,"fullApplicationClosure":false})
}
impl Pending {
    pub(in crate::agent_bridge::history_checkpoint::project_recovery_snapshot::save_startup::runtime_transition) fn confirm_application_records_intent_publication(
        &mut self,
        s: &Session,
        value: &Value,
    ) -> Result<Value> {
        let records = self.application.records_mut()?;
        records.continuation.begin_intent()?;
        let plan = compact_plan(&records.plan)?;
        bytes(value, INPUT)?;
        let receipt: IntentReceipt = parse("native.coldSaveApplicationRecordsIntent", value)?;
        if receipt.kind != "poema-original-Save-native-application-records-intent-receipt"
            || receipt.version != 1
            || receipt.event != json!({"kind":"native-application-records-intent","plan":plan})
        {
            return Err(error("the sole original application-record intent must name this same retained native plan"));
        }
        let check = media_guard::Check::new(web_time::Instant::now());
        let successor = self.read_published_successor(&receipt.anchor, &receipt.head, receipt.event.clone(), &check)?;
        self.successor = Some(successor);
        self.worker_current(s.persistence.as_ref())?;
        self.check_canonical(s)?;
        self.application.records_mut()?.continuation.intent_confirmed = true;
        self.current()?;
        Ok(confirmation(false, &self.preparation["nativeSession"], &receipt, None))
    }
    pub(in crate::agent_bridge::history_checkpoint::project_recovery_snapshot::save_startup::runtime_transition) fn confirm_application_native_presentation_publication(
        &mut self,
        s: &Session,
        value: &Value,
    ) -> Result<Value> {
        let records = self.application.records_mut()?;
        records.continuation.begin_publication()?;
        let plan = records.plan.clone();
        bytes(value, INPUT)?;
        let receipt: IntentReceipt = parse("native.coldSaveApplicationNativePresentation", value)?;
        if receipt.kind != "poema-original-Save-native-application-records-observation-receipt"
            || receipt.version != 1
            || !fields(&receipt.event, "kind,publication")
            || receipt.event["kind"] != "native-application-records-observation"
        {
            return Err(error("the sole original native presentation publication observation is required"));
        }
        let descriptor = published_native(&receipt.event["publication"], &plan)?;
        let check = media_guard::Check::new(web_time::Instant::now());
        let io = || check.check().map_err(|e| io::Error::other(e.to_string()));
        records.parent.current(&io).map_err(error)?;
        let next = records.parent.node_bounded("native.json", 4096, &io).map_err(error)?;
        expected_record_node(&next, &json!(descriptor), 4096, &check)?;
        let actual_control = observed(&descriptor, 4096, &check)?;
        let actual = actual_control.observation()?;
        let parent = records.parent.record();
        let catalog = control(&self.original, 2)?.observation()?;
        if actual_control.value() != &plan["nativeState"]["replacement"]
            || actual.identity.device != parent.anchor.identity.device
            || actual.identity.uid != catalog.identity.uid
            || actual.identity.mode & 0o7777 != 0o600
        {
            return Err(error("the actual native presentation differs from its retained replacement or original private parent"));
        }
        if let NodeState::File(before) = records.native_state.state() {
            if actual.identity.device != before.identity.device || actual.identity.inode == before.identity.inode || actual.identity.uid != before.identity.uid
            {
                return Err(error("the original native presentation publisher must advance its exact retained file generation"));
            }
        }
        records.parent.current(&io).map_err(error)?;
        // Advance only this SAME owner's planned native witness before checking
        // the paired journal successor. Any failure consumes this attempt and
        // leaves its continuation unresolved; no other source is rebaselined.
        records.native_state = next;
        let successor = self.read_published_successor(&receipt.anchor, &receipt.head, receipt.event.clone(), &check)?;
        self.successor = Some(successor);
        self.worker_current(s.persistence.as_ref())?;
        self.check_canonical(s)?;
        self.application.records_mut()?.continuation.publication_confirmed = true;
        self.current()?;
        Ok(confirmation(true, &self.preparation["nativeSession"], &receipt, Some(&descriptor)))
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn checkpoint_encoding_requires_the_actual_same_original_records_intent_and_publication_acknowledgements() {
        let mut c = super::Continuation::default();
        assert!(c.checkpoint_ready().is_err());
        c.intent_attempted = true;
        assert!(c.checkpoint_ready().is_err());
        c.intent_confirmed = true;
        assert!(c.checkpoint_ready().is_err());
        c.publication_attempted = true;
        assert!(c.checkpoint_ready().is_err());
        c.publication_confirmed = true;
        c.checkpoint_ready().unwrap();
    }

    use super::*;
    fn plan() -> Value {
        json!({"kind":"poema-native-Save-application-records-plan","version":1,"nativePid":202,"nativeSession":"a".repeat(32),
            "privateInput":{"retained":true},"manifest":{"retained":true},"catalog":{"observation":{"retainedCatalog":true},"value":{"notRepeated":true}},
            "activity":{"observation":{"retainedActivity":true},"value":{"notRepeated":true}},
            "nativeState":{"path":"/generated/native.json","before":null,"original":null,"replacement":{"pid":202,"projectPath":"/generated/project.fcproj"}},
            "editingPaused":true,"workerReleased":false,"writeAuthority":false,"fullApplicationClosure":false})
    }
    #[test]
    fn compact_publication_plan_preserves_same_native_fields_and_original_control_witnesses_without_repeating_values() {
        let original = plan();
        let value = compact_plan(&original).unwrap();
        assert_eq!(value["catalog"], original["catalog"]["observation"]);
        assert_eq!(value["activity"], original["activity"]["observation"]);
        assert_eq!(value["nativeState"], original["nativeState"]);
        assert_eq!(value["kind"], "poema-native-Save-application-records-publication-plan");
        for key in ["workerReleased", "writeAuthority", "fullApplicationClosure"] {
            let mut changed = original.clone();
            changed[key] = json!(true);
            assert!(compact_plan(&changed).is_err());
        }
        let mut changed = original.clone();
        changed["extra"] = json!(true);
        assert!(compact_plan(&changed).is_err());
    }
    #[test]
    fn native_record_continuation_consumes_uncertain_intent_and_publication_without_replay_or_premature_publication() {
        let mut continuation = Continuation::default();
        continuation.resolved().unwrap();
        assert!(continuation.begin_publication().is_err());
        continuation.begin_intent().unwrap();
        assert!(continuation.resolved().is_err());
        assert!(continuation.begin_intent().is_err());
        continuation.intent_confirmed = true;
        continuation.resolved().unwrap();
        continuation.begin_publication().unwrap();
        assert!(continuation.resolved().is_err());
        assert!(continuation.begin_publication().is_err());
        continuation.publication_confirmed = true;
        continuation.resolved().unwrap();
        assert!(continuation.begin_publication().is_err());
    }
    #[test]
    fn native_record_publication_description_refuses_crossed_compact_plans_paths_and_release_claims() {
        let plan = plan();
        let value = json!({"kind":"poema-original-Save-native-presentation-publication","version":1,"plan":compact_plan(&plan).unwrap(),
            "nativeState":{"path":"/generated/native.json","sha256":"b".repeat(64),"bytes":1,
                "physical":{"dev":"1","ino":"2","mode":"33152","uid":"1","size":"1","mtimeNs":"1","ctimeNs":"1"}},
            "editingPaused":true,"workerReleased":false,"writeAuthority":false,"fullApplicationClosure":false});
        published_native(&value, &plan).unwrap();
        for key in ["workerReleased", "writeAuthority", "fullApplicationClosure"] {
            let mut changed = value.clone();
            changed[key] = json!(true);
            assert!(published_native(&changed, &plan).is_err());
        }
        let mut changed = value.clone();
        changed["plan"]["nativePid"] = json!(303);
        assert!(published_native(&changed, &plan).is_err());
        let mut changed = value.clone();
        changed["nativeState"]["path"] = json!("/foreign/native.json");
        assert!(published_native(&changed, &plan).is_err());
        let mut changed = value.clone();
        changed["nativeState"]["bytes"] = json!(4097);
        assert!(published_native(&changed, &plan).is_err());
    }
}
