//! Original metadata settlement on the SAME completed Cold controller. Neither
//! plans nor receipts reconstruct a native owner or admit an editable desktop.
use super::*;
const PLAN_FIELDS: &str = "kind,version,status,nativePid,nativeSession,privateInput,before,selected,requiredProjectPath,pathAdoptionRequired,remainingPendingSaveIds,saveAllScope,proposed,requiresSameWorkerReleaseAcknowledgement,requiresOriginalPublisherReceipts,catalogUnchangedBeforeRelease,activityUnchangedBeforeRelease,editingPaused,workerReleased,current,writeAuthority,nativeComplete,startupAuthority,workersMayResume,requestReplay,fullApplicationClosure";

pub(super) struct Prepared {
    plan: Value,
    intent_attempted: bool,
    intent_confirmed: bool,
    publication_attempted: bool,
    publication_confirmed: bool,
    catalog: Option<Arc<Control>>,
    activity: Option<Arc<Control>>,
    confirmation: Option<Value>,
}
impl Prepared {
    pub(super) fn completed_controls(&self) -> Result<Option<(&Control, &Control)>> {
        if self.intent_attempted && !self.intent_confirmed || self.publication_attempted && !self.publication_confirmed {
            return Err(error("original metadata settlement is unresolved; no replay or completed admission"));
        }
        if !self.publication_confirmed {
            return Ok(None);
        }
        Ok(Some((
            self.catalog.as_deref().ok_or_else(|| error("actual settled catalog missing"))?,
            self.activity.as_deref().ok_or_else(|| error("actual settled activity missing"))?,
        )))
    }
}
fn next_activity(original: &Value, catalog: &Value, digest: &str) -> Result<Value> {
    let rows = catalog["operations"].as_array().filter(|r| r.len() <= 4).ok_or_else(|| error("bounded original remaining Save operations required"))?;
    let operations: Vec<Value> = rows
        .iter()
        .map(|row| json!({"id":row["id"],"action":row["action"],"request":row["request"],"nativeSession":row["basis"]["nativeSession"],"phase":row["phase"]}))
        .collect();
    let mut next = original.clone();
    let object = next.as_object_mut().ok_or_else(|| error("complete original accepted Send activity required"))?;
    object.insert(
        "nativeSave".into(),
        json!({"kind":"original_Save_activity","version":1,"catalog":"native-save-evidence.json","sha256":digest,"operations":operations,
        "current":false,"writeAuthority":false,"nativeComplete":false,"startupAuthority":false,"requestReplay":false}),
    );
    Ok(next)
}
fn validate_plan(
    plan: &Value,
    original: &Input,
    preparation: &Value,
    records: &records::Records,
    root: &root::Root,
    project_path: &str,
    check: &media_guard::Check,
) -> Result<()> {
    bytes(plan, INPUT)?;
    if !fields(plan, PLAN_FIELDS)
        || plan["kind"] != "poema-original-Save-deferred-application-settlement-plan"
        || plan["version"] != 1
        || plan["status"] != "awaiting-same-original-worker-release"
        || plan["nativePid"] != std::process::id()
        || plan["nativeSession"] != preparation["nativeSession"]
        || plan["privateInput"] != preparation["privateInput"]
        || plan["pathAdoptionRequired"] != false
        || plan["remainingPendingSaveIds"] != json!([])
        || plan["saveAllScope"] != "this_native_session_only"
        || ![
            "requiresSameWorkerReleaseAcknowledgement",
            "requiresOriginalPublisherReceipts",
            "catalogUnchangedBeforeRelease",
            "activityUnchangedBeforeRelease",
            "editingPaused",
        ]
        .iter()
        .all(|k| plan[*k] == true)
        || !false_fields(plan, "workerReleased,current,writeAuthority,nativeComplete,startupAuthority,workersMayResume,requestReplay,fullApplicationClosure")
        || !fields(&plan["before"], "manifest,catalog,activity,nativeState")
        || !fields(&plan["selected"], "id,action,request,intent,originalNativePid,originalNativeSession,reconciliationSha256")
        || !fields(&plan["proposed"], "catalog,catalogSha256,catalogBytes,activity")
    {
        return Err(error("closed SAME original deferred settlement without path adoption or another pending Save required"));
    }
    let basis = &original.body.basis;
    let manifest = control(original, 0)?.value();
    let catalog = control(original, 2)?;
    let activity = records.original_activity();
    let directory = basis.input.catalog_file.parent().ok_or_else(|| error("original catalog parent missing"))?;
    if plan["before"]["manifest"] != control_description(root.acknowledged_manifest()?, &basis.manifest_path)?
        || plan["before"]["catalog"] != control_description(catalog, &basis.input.catalog_file)?
        || plan["before"]["activity"] != control_description(activity, &directory.join("activity.json"))?
        || plan["requiredProjectPath"] != project_path
        || manifest["projectPath"] != project_path
        || plan["selected"]["id"] != basis.input.evidence_id
        || plan["selected"]["request"] != json!(basis.input.request)
    {
        return Err(error("settlement crossed its SAME original input, Root, path or retained metadata generations"));
    }
    records.assert_native_description(&plan["before"]["nativeState"], check)?;
    let rows = catalog.value()["operations"].as_array().filter(|r| r.len() <= 4).ok_or_else(|| error("complete original bounded catalog required"))?;
    let mut selected = None;
    let mut remaining = Vec::new();
    for row in rows {
        if row["id"] == basis.input.evidence_id {
            if selected.replace(row).is_some() {
                return Err(error("duplicate original selected Save"));
            }
        } else {
            if row["phase"] != "cancelled" && row["phase"] != "refused" {
                return Err(error("other original Saves still require their own settlement"));
            }
            remaining.push(row.clone());
        }
    }
    let row = selected.ok_or_else(|| error("the original selected matched Save is missing"))?;
    let dispatch = &row["dispatchIntent"];
    let refs = row["observations"].as_array().filter(|r| r.len() <= 9).ok_or_else(|| error("complete original bounded resource tuple required"))?;
    let matched = refs.iter().filter(|r| r["role"] == "file_reconciliation").collect::<Vec<_>>();
    if catalog.value()["version"] != 8
        || row["phase"] != "needs_observation"
        || !dispatch.is_object()
        || row["request"] != plan["selected"]["request"]
        || row["intent"] != plan["selected"]["intent"]
        || row["action"] != plan["selected"]["action"]
        || row["basis"]["nativeSession"] != plan["selected"]["originalNativeSession"]
        || dispatch["nativePid"] != plan["selected"]["originalNativePid"]
        || dispatch["nativePid"] != manifest["nativePid"]
        || matched.len() != 1
        || matched.first().is_none_or(|r| r["sha256"] != plan["selected"]["reconciliationSha256"])
        || activity.value()["status"] != "active"
        || activity.value()["action"] != "agent"
        || activity.value()["operationId"] != row["action"]
        || activity.value()["job"]["id"] != dispatch["sendJob"]
        || activity.value()["originalInput"] == true
        || activity.value()["mediaPreparation"] == true
        || activity.value()["sourceAnalysis"]["phase"] == "waiting"
    {
        return Err(error("the SAME complete original matched Save and still active accepted Send are required"));
    }
    let mut next_catalog = catalog.value().clone();
    next_catalog.as_object_mut().ok_or_else(|| error("original complete catalog object required"))?.insert("operations".into(), json!(remaining));
    let digest = plan["proposed"]["catalogSha256"].as_str().filter(|s| hash_ok(s)).ok_or_else(|| error("exact original proposed catalog digest required"))?;
    if plan["proposed"]["catalogBytes"].as_u64().is_none_or(|n| n == 0 || n > CATALOG)
        || plan["proposed"]["catalog"] != next_catalog
        || plan["proposed"]["activity"] != next_activity(activity.value(), &next_catalog, digest)?
    {
        return Err(error("settlement may remove only the exact selected row and update its original activity pointer"));
    }
    check.check()
}
enum ConfirmationKind {
    ImageObservation,
    SettlementIntent,
    SettlementPublication,
}
pub(super) fn assert_current_image_observation_description(checkpoint: &checkpoint::Prepared, event: &Value) -> Result<()> {
    if *event != json!({"kind":"native-current-image-observation","checkpoint":checkpoint.description(),"image":checkpoint.actual_current_image_files()?}) {
        return Err(error("the SAME actual completed original image description is required"));
    }
    Ok(())
}
fn confirmation(epoch: &Value, receipt: &IntentReceipt, kind: ConfirmationKind) -> Value {
    let kind = match kind {
        ConfirmationKind::ImageObservation => "native_cold_Save_current_image_observation_confirmation",
        ConfirmationKind::SettlementIntent => "native_cold_Save_application_settlement_intent_confirmation",
        ConfirmationKind::SettlementPublication => "native_cold_Save_application_settlement_publication_confirmation",
    };
    json!({"kind":kind,"version":1,
        "nativePid":std::process::id(),"nativeSession":epoch,"event":receipt.event,"anchor":receipt.anchor,"head":receipt.head,
        "nativeAcknowledgementConfirmed":true,"acceptedSendRetained":true,"workerReleased":true,"editingPaused":true,"directSynchronizationHeld":true,
        "writeAuthority":false,"startupAuthority":false,"workersMayResume":false,"fullApplicationClosure":false})
}
impl Pending {
    pub(in super::super) fn prepare_actual_application_settlement(&mut self, s: &Session, request: &Value) -> Result<Value> {
        if self.application.settlement_attempted {
            return Err(error("the SAME original metadata settlement was already attempted; no replay"));
        }
        self.application.settlement_attempted = true;
        bytes(request, INPUT)?;
        let with_image = request["version"] == 2;
        let request_fields =
            if with_image { "kind,version,nativeSession,privateInput,plan,imageObservation" } else { "kind,version,nativeSession,privateInput,plan" };
        if !fields(request, request_fields)
            || request["kind"] != "poema-original-Save-native-application-settlement-prepare"
            || request["version"] != 1 && request["version"] != 2
            || request["nativeSession"] != self.preparation["nativeSession"]
            || request["privateInput"] != self.preparation["privateInput"]
        {
            return Err(error("one exact actual private native settlement request required"));
        }
        let check = media_guard::Check::new(web_time::Instant::now());
        let image_observation = if with_image {
            let receipt: IntentReceipt = parse("native.coldSaveCurrentImageObservation", &request["imageObservation"])?;
            let checkpoint = self.application.checkpoint.as_ref().ok_or_else(|| error("the SAME actual completed checkpoint is required"))?;
            if receipt.kind != "poema-original-Save-native-current-image-observation-receipt" || receipt.version != 1 {
                return Err(error("the SAME independently observed original current image and journal receipt are required"));
            }
            assert_current_image_observation_description(checkpoint, &receipt.event)?;
            // Validate the actual published successor before inspecting its new
            // head. Retain the SAME source Controls, completed checkpoint frame
            // and canonical/worker owner through the existing completed reader.
            let next = self.read_published_successor(&receipt.anchor, &receipt.head, receipt.event.clone(), &check)?;
            let original = self.original.clone();
            let records = self.application.records.clone().ok_or_else(|| error("the SAME actual original records disappeared"))?;
            self.adopt_completed_settlement_successor(s, next, control(&original, 2)?, records.original_activity(), &check)?;
            Some(receipt)
        } else {
            None
        };
        self.inspect_actual_completed_current_image(s, &check)?;
        let records = self.application.records.as_ref().ok_or_else(|| error("the SAME original records required"))?;
        let root = self.application.root.as_ref().ok_or_else(|| error("the SAME original Root required"))?;
        validate_plan(
            &request["plan"],
            &self.original,
            &self.preparation,
            records,
            root,
            s.path.as_deref().ok_or_else(|| error("the SAME actual original project path is required"))?,
            &check,
        )?;
        self.application.settlement = Some(Prepared {
            plan: request["plan"].clone(),
            intent_attempted: false,
            intent_confirmed: false,
            publication_attempted: false,
            publication_confirmed: false,
            catalog: None,
            activity: None,
            confirmation: None,
        });
        let mut preparation = json!({"kind":"native_cold_Save_application_settlement_preparation","version":if with_image {2} else {1},"nativePid":std::process::id(),"nativeSession":self.preparation["nativeSession"],
            "plan":request["plan"],"workerReleased":true,"editingPaused":true,"directSynchronizationHeld":true,"acceptedSendRetained":true,
            "writeAuthority":false,"startupAuthority":false,"workersMayResume":false,"fullApplicationClosure":false});
        if let Some(receipt) = image_observation {
            preparation
                .as_object_mut()
                .ok_or_else(|| error("closed native settlement preparation object required"))?
                .insert("imageObservation".into(), confirmation(&self.preparation["nativeSession"], &receipt, ConfirmationKind::ImageObservation));
        }
        check.check()?;
        Ok(preparation)
    }
    pub(in super::super) fn confirm_actual_application_settlement_intent(&mut self, s: &Session, value: &Value) -> Result<Value> {
        let retained = self.application.settlement.as_mut().ok_or_else(|| error("the SAME prepared actual original settlement required"))?;
        if retained.intent_attempted {
            return Err(error("original settlement intent was already attempted; no replay"));
        }
        retained.intent_attempted = true;
        let plan = retained.plan.clone();
        bytes(value, INPUT)?;
        let receipt: IntentReceipt = parse("native.coldSaveSettlementIntent", value)?;
        if receipt.kind != "poema-original-Save-native-application-settlement-intent-receipt"
            || receipt.version != 1
            || receipt.event
                != json!({"kind":"native-application-settlement-intent","plan":plan,"image":self.application.checkpoint.as_ref().ok_or_else(||error("the SAME actual completed image is required"))?.actual_current_image_files()?})
        {
            return Err(error("the exact same original settlement intent journal event required"));
        }
        let check = media_guard::Check::new(web_time::Instant::now());
        let next = self.read_published_successor(&receipt.anchor, &receipt.head, receipt.event.clone(), &check)?;
        let original = self.original.clone();
        let records = self.application.records.clone().ok_or_else(|| error("the SAME actual original records disappeared"))?;
        self.adopt_completed_settlement_successor(s, next, control(&original, 2)?, records.original_activity(), &check)?;
        let retained = self.application.settlement.as_mut().ok_or_else(|| error("the actual original settlement disappeared"))?;
        retained.intent_confirmed = true;
        check.check()?;
        Ok(confirmation(&self.preparation["nativeSession"], &receipt, ConfirmationKind::SettlementIntent))
    }
    pub(in super::super) fn confirm_actual_application_settlement_publication(&mut self, s: &Session, value: &Value) -> Result<Value> {
        let retained = self.application.settlement.as_mut().ok_or_else(|| error("the SAME prepared original settlement required"))?;
        if !retained.intent_confirmed || retained.publication_attempted {
            return Err(error("the SAME acknowledged unused original settlement publication required"));
        }
        retained.publication_attempted = true;
        let plan = retained.plan.clone();
        bytes(value, INPUT)?;
        let receipt: IntentReceipt = parse("native.coldSaveSettlementPublication", value)?;
        let publication = &receipt.event["publication"];
        if receipt.kind != "poema-original-Save-native-application-settlement-observation-receipt"
            || receipt.version != 1
            || !fields(&receipt.event, "kind,publication")
            || receipt.event["kind"] != "native-application-settlement-observation"
            || !fields(
                publication,
                "kind,version,plan,catalog,activity,nativeState,acceptedSendRetained,nativeAcknowledgementConfirmed,editingPaused,writeAuthority,startupAuthority,workersMayResume,fullApplicationClosure",
            )
            || publication["kind"] != "poema-original-Save-application-settlement-publication"
            || publication["version"] != 1
            || publication["plan"] != plan
            || publication["nativeState"] != plan["before"]["nativeState"]
            || publication["acceptedSendRetained"] != true
            || publication["editingPaused"] != true
            || !false_fields(publication, "nativeAcknowledgementConfirmed,writeAuthority,startupAuthority,workersMayResume,fullApplicationClosure")
        {
            return Err(error("the SAME actual original catalog/activity publisher and exact complete receipt required"));
        }
        let check = media_guard::Check::new(web_time::Instant::now());
        let records = self.application.records.clone().ok_or_else(|| error("the SAME actual original records required"))?;
        records.assert_native_description(&publication["nativeState"], &check)?;
        let catalog = Arc::new(records.observe_settlement_control("native-save-evidence.json", &publication["catalog"], &check)?);
        let activity = Arc::new(records.observe_settlement_control("activity.json", &publication["activity"], &check)?);
        for (role, actual, old) in [("catalog", catalog.as_ref(), control(&self.original, 2)?), ("activity", activity.as_ref(), records.original_activity())] {
            let after = actual.observation()?;
            let before = old.observation()?;
            if after.identity.device != before.identity.device
                || after.identity.uid != before.identity.uid
                || after.identity.inode == before.identity.inode
                || after.identity.mode & 0o7777 != 0o600
                || actual.value() != &plan["proposed"][role]
            {
                return Err(error("the actual original settlement must advance only its own exact private catalog/activity generations"));
            }
        }
        let observed_catalog = catalog.observation()?;
        if json!(observed_catalog.sha256) != plan["proposed"]["catalogSha256"] || json!(observed_catalog.bytes) != plan["proposed"]["catalogBytes"] {
            return Err(error("the actual original compact catalog differs from its predeclared complete bytes"));
        }
        let next = self.read_published_successor_candidate(&receipt.anchor, &receipt.head, receipt.event.clone(), &check)?;
        self.adopt_completed_settlement_successor(s, next, &catalog, &activity, &check)?;
        let result = confirmation(&self.preparation["nativeSession"], &receipt, ConfirmationKind::SettlementPublication);
        let retained = self.application.settlement.as_mut().ok_or_else(|| error("the actual original settlement disappeared"))?;
        retained.catalog = Some(catalog);
        retained.activity = Some(activity);
        retained.publication_confirmed = true;
        retained.confirmation = Some(result.clone());
        self.inspect_actual_completed_current_image(s, &check)?;
        check.check()?;
        Ok(result)
    }
    pub(in super::super) fn inspect_actual_application_settlement(&self, s: &Session) -> Result<Value> {
        let check = media_guard::Check::new(web_time::Instant::now());
        self.inspect_actual_completed_current_image(s, &check)?;
        let result = self
            .application
            .settlement
            .as_ref()
            .and_then(|r| r.confirmation.as_ref())
            .ok_or_else(|| error("the SAME actually acknowledged original settlement is required"))?;
        check.check()?;
        Ok(result.clone())
    }
}
