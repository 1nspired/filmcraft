//! Sole original journal successors and one expected manifest replacement.
//! Only the actual retained Root plan on the SAME Cold owner can observe them.
//! The original worker, canonical Commit and Session remain held throughout.
use super::*;

fn confirmation(kind: &str, native_session: &Value, receipt: &IntentReceipt, manifest: Option<&Observed>) -> Value {
    json!({"kind":kind,"version":1,"nativePid":std::process::id(),"nativeSession":native_session,
        "event":receipt.event,"anchor":receipt.anchor,"head":receipt.head,"manifest":manifest,
        "nativeAcknowledgementConfirmed":true,"editingPaused":true,"workerReleased":false,
        "writeAuthority":false,"startupAuthority":false,"workersMayResume":false,"fullApplicationClosure":false})
}
fn publication_manifest(value: &Value, plan: &Value) -> Result<Observed> {
    if !fields(value, "kind,version,plan,manifest,editingPaused,workerReleased,writeAuthority,fullApplicationClosure")
        || value["kind"] != "poema-original-Save-application-root-publication"
        || value["version"] != 1
        || value["plan"] != *plan
        || value["editingPaused"] != true
        || !false_fields(value, "workerReleased,writeAuthority,fullApplicationClosure")
    {
        return Err(error("the same original Root publisher observation is required; no status can grant release"));
    }
    let manifest: Observed = parse("native.coldSaveApplicationRoot", &value["manifest"])?;
    if json!(manifest.path) != plan["manifest"]["path"] {
        return Err(error("the Root publication crossed its original manifest path"));
    }
    Ok(manifest)
}
impl Pending {
    pub(in crate::agent_bridge::history_checkpoint::project_recovery_snapshot::save_startup::runtime_transition) fn confirm_application_root_intent_publication(
        &mut self,
        s: &Session,
        value: &Value,
    ) -> Result<Value> {
        let root = self.application.root_mut()?;
        if root.intent_attempted {
            return Err(error("the original Root intent confirmation was already attempted"));
        }
        root.intent_attempted = true;
        let plan = root.plan.clone();
        bytes(value, INPUT)?;
        let receipt: IntentReceipt = parse("native.coldSaveApplicationRootIntent", value)?;
        if receipt.kind != "poema-original-Save-native-application-root-intent-receipt"
            || receipt.version != 1
            || receipt.event != json!({"kind":"native-application-root-intent","plan":plan})
        {
            return Err(error("the sole original Root intent must name this same retained native plan"));
        }
        let check = media_guard::Check::new(web_time::Instant::now());
        let successor = self.read_published_successor(&receipt.anchor, &receipt.head, receipt.event.clone(), &check)?;
        self.successor = Some(successor);
        self.worker_current(s.persistence.as_ref())?;
        self.check_canonical(s)?;
        self.application.root_mut()?.intent_confirmed = true;
        self.current()?;
        Ok(confirmation("native_cold_Save_application_root_intent_confirmation", &self.preparation["nativeSession"], &receipt, None))
    }
    pub(in crate::agent_bridge::history_checkpoint::project_recovery_snapshot::save_startup::runtime_transition) fn confirm_application_root_file_publication(
        &mut self,
        s: &Session,
        value: &Value,
    ) -> Result<Value> {
        let root = self.application.root_mut()?;
        if !root.intent_confirmed || root.publication_attempted {
            return Err(error("one same-owner acknowledged Root intent and unused publication observation are required"));
        }
        root.publication_attempted = true;
        let plan = root.plan.clone();
        bytes(value, INPUT)?;
        let receipt: IntentReceipt = parse("native.coldSaveApplicationRootObservation", value)?;
        if receipt.kind != "poema-original-Save-native-application-root-observation-receipt"
            || receipt.version != 1
            || !fields(&receipt.event, "kind,publication")
            || receipt.event["kind"] != "native-application-root-observation"
        {
            return Err(error("the sole original Root publication observation is required"));
        }
        let descriptor = publication_manifest(&receipt.event["publication"], &plan)?;
        let check = media_guard::Check::new(web_time::Instant::now());
        let manifest = observed(&descriptor, CATALOG, &check)?;
        let before = control(&self.original, 0)?.observation()?;
        let after = manifest.observation()?;
        if manifest.value() != &plan["replacement"]
            || after.identity.device != before.identity.device
            || after.identity.inode == before.identity.inode
            || after.identity.uid != before.identity.uid
            || after.identity.mode & 0o777 != 0o600
        {
            return Err(error("the actual original manifest replacement differs from its retained plan or publisher generation"));
        }
        // Retain only this actual, expected replacement before verifying the
        // paired original journal successor. Failure leaves the one-use attempt
        // unresolved; it never rebaselines another control, file or worker.
        self.application.root_mut()?.manifest = Some(Arc::new(manifest));
        let successor = self.read_published_successor(&receipt.anchor, &receipt.head, receipt.event.clone(), &check)?;
        self.successor = Some(successor);
        self.worker_current(s.persistence.as_ref())?;
        self.check_canonical(s)?;
        self.application.root_mut()?.publication_confirmed = true;
        self.current()?;
        Ok(confirmation("native_cold_Save_application_root_publication_confirmation", &self.preparation["nativeSession"], &receipt, Some(&descriptor)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn root_publication_observation_refuses_changed_plans_paths_and_claimed_release() {
        let plan = json!({"manifest":{"path":"/generated/workspace.json"}});
        let value = json!({"kind":"poema-original-Save-application-root-publication","version":1,"plan":plan,
            "manifest":{"path":"/generated/workspace.json","sha256":"a".repeat(64),"bytes":1,
                "physical":{"dev":"1","ino":"2","mode":"33152","uid":"1","size":"1","mtimeNs":"1","ctimeNs":"1"}},
            "editingPaused":true,"workerReleased":false,"writeAuthority":false,"fullApplicationClosure":false});
        assert!(publication_manifest(&value, &plan).is_ok());
        for flag in ["workerReleased", "writeAuthority", "fullApplicationClosure"] {
            let mut changed = value.clone();
            changed[flag] = json!(true);
            assert!(publication_manifest(&changed, &plan).is_err());
        }
        let mut changed = value.clone();
        changed["plan"]["extra"] = json!(true);
        assert!(publication_manifest(&changed, &plan).is_err());
        let mut changed = value.clone();
        changed["manifest"]["path"] = json!("/foreign/workspace.json");
        assert!(publication_manifest(&changed, &plan).is_err());
    }
}
