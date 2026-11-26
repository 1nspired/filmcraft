//! A sole private-journal successor of the SAME actual Cold runtime. Keeping a
//! plan does not mint a release key; the original owner, worker and Commit stay
//! retained. Source observations are actual bounded native file witnesses.
use super::*;
mod checkpoint;
mod current_release;
mod publication;
mod records;
mod root;
mod settlement;

#[derive(Default)]
pub(super) struct Application {
    prepared: Option<Value>,
    checkpoint_attempted: bool,
    checkpoint: Option<checkpoint::Prepared>,
    attempted: bool,
    confirmed: bool,
    source: Option<Arc<Source>>,
    root_attempted: bool,
    root: Option<Arc<root::Root>>,
    records_attempted: bool,
    records: Option<Arc<records::Records>>,
    publication_source: Option<Arc<publication::SourceWitness>>,
    image_completion_attempted: bool,
    settlement_attempted: bool,
    settlement: Option<settlement::Prepared>,
}
struct Source {
    startup: Control,
    handoff: Control,
    pid: u32,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct IntentReceipt {
    kind: String,
    version: u8,
    event: Value,
    anchor: Observed,
    head: Observed,
}
fn fields(v: &Value, names: &str) -> bool {
    v.as_object().is_some_and(|o| o.len() == names.split(',').count() && names.split(',').all(|key| o.contains_key(key)))
}
fn false_fields(v: &Value, names: &str) -> bool {
    names.split(',').all(|key| v.get(key) == Some(&Value::Bool(false)))
}
fn observation_time(v: &Value) -> bool {
    // Original source producers write bounded ISO strings, not numeric native
    // ticks. Preserve them as opaque observations; they grant no authority.
    v.as_str().is_some_and(|s| !s.is_empty() && s.len() <= 128 && !s.chars().any(char::is_control))
}
impl Application {
    fn root_mut(&mut self) -> Result<&mut root::Root> {
        self.root
            .as_mut()
            .and_then(Arc::get_mut)
            .ok_or_else(|| error("the actual Root is missing or already shared with the checkpoint worker; no mutation or replay"))
    }
    fn records_mut(&mut self) -> Result<&mut records::Records> {
        self.records
            .as_mut()
            .and_then(Arc::get_mut)
            .ok_or_else(|| error("the actual records are missing or already shared with the checkpoint worker; no mutation or replay"))
    }
    pub(super) fn current_checkpoint(&self, s: &Session) -> Result<()> {
        if let Some(checkpoint) = &self.checkpoint {
            checkpoint.current(s)?;
        }
        Ok(())
    }
    pub(super) fn manifest(&self) -> Option<&Control> {
        self.root.as_ref().and_then(|root| root.manifest())
    }
    pub(super) fn current(&self) -> Result<()> {
        if self.publication_source.is_some() && self.checkpoint.as_ref().is_none_or(|p| !p.publication_complete()) {
            return Err(error("the original controller checkpoint publication is unresolved; no replay or release"));
        }
        if let Some(checkpoint) = &self.checkpoint {
            checkpoint.resolved()?;
            checkpoint.archive_intent_resolved()?;
        }
        if self.checkpoint_attempted && self.checkpoint.is_none() || self.prepared.is_some() && self.checkpoint.is_none() {
            return Err(error("original current-image checkpoint capture is unresolved; retain the same owner without replay"));
        }
        if self.attempted && !self.confirmed || self.confirmed && (self.prepared.is_none() || self.source.is_none()) {
            return Err(error("application intent confirmation is unresolved; retain its original owner without replay"));
        }
        if self.root_attempted && self.root.is_none() {
            return Err(error("application Root preparation is unresolved; no second attempt"));
        }
        if self.records_attempted && self.records.is_none() {
            return Err(error("application records preparation is unresolved; no replay"));
        }
        if let Some(root) = &self.root {
            root.current()?;
        }
        if let Some(records) = &self.records {
            records.resolved()?;
        }
        Ok(())
    }
    pub(super) fn source_current(&self, check: &media_guard::Check) -> Result<()> {
        if let Some(records) = &self.records {
            records.current(check)?;
        }
        if let Some(source) = &self.source {
            source.current(check)?;
        }
        Ok(())
    }
    fn begin_confirmation(&mut self) -> Result<Value> {
        if self.attempted {
            return Err(error("application intent confirmation was already attempted; no replay"));
        }
        let prepared = self.prepared.as_ref().ok_or_else(|| error("the same actual native application preparation is required"))?.clone();
        self.attempted = true;
        Ok(prepared)
    }
}
impl Source {
    fn current(&self, check: &media_guard::Check) -> Result<()> {
        self.read(check, true)
    }
    fn fence_after_read(&self, check: &media_guard::Check) -> Result<()> {
        self.read(check, false)
    }
    fn read(&self, check: &media_guard::Check, read_bytes: bool) -> Result<()> {
        check.check()?;
        check_owner_process(self.pid).map_err(error)?;
        for control in [&self.startup, &self.handoff] {
            if read_bytes {
                control.current(check)?;
            }
            control.fence(check)?;
        }
        check_owner_process(self.pid).map_err(error)?;
        check.check()
    }
    fn capture(original: &Input, value: &Value, check: &media_guard::Check) -> Result<Self> {
        let b = &original.body.basis;
        let owner = &b.owner;
        let source = &value["source"];
        let identity = &value["identity"];
        let pid = source["pid"].as_u64().filter(|n| *n > 1 && *n <= i32::MAX as u64).ok_or_else(|| error("bounded actual replacement PID required"))?;
        let pid = u32::try_from(pid).map_err(error)?;
        if pid == std::process::id() || pid == original.body.manager.pid || pid == b.frame.native_pid {
            return Err(error("the actual original replacement must be distinct from the manager and native actors"));
        }
        if !fields(
            value,
            "kind,version,phase,identity,source,stoppedDataRootLeaseRetained,writeAuthority,nativeStartupAuthority,workersMayResume,fullApplicationClosure",
        ) || value["kind"] != "poema-original-data-held-handoff-description"
            || value["version"] != 1
            || value["phase"] != "original-replacement-observed"
            || !false_fields(value, "stoppedDataRootLeaseRetained,writeAuthority,nativeStartupAuthority,workersMayResume,fullApplicationClosure")
            || !fields(identity, "ownerId,origin,dataRoot,projectId,workspaceId,operationId,old,nextBuild,ledgerFile,bootstrapFile")
            || !fields(
                source,
                "kind,version,ownerId,origin,dataRoot,runtimeRoot,projectId,workspaceId,stateFile,applicationRelease,ready,pid,readiness,runtimeReadiness,writeAuthority,nativeStartupAuthority,workersMayResume,fullApplicationClosure",
            )
            || source["kind"] != "poema-original-editorial-ready-observation-description"
            || source["version"] != 1
            || source["readiness"] != "observed-at-original-producer"
            || source["runtimeReadiness"] != "not-inferred-from-observation"
            || !(fields(&source["ready"], "ownerId,workspaceId,projectId,origin,status,pid,reused,checked")
                || fields(&source["ready"], "ownerId,workspaceId,projectId,origin,status,pid,started,reused,checked"))
            || !observation_time(&source["ready"]["checked"])
            || source["ready"].get("started").is_some_and(|v| !observation_time(v))
            || source["ready"]["reused"].as_bool().is_none()
            || !false_fields(source, "writeAuthority,nativeStartupAuthority,workersMayResume,fullApplicationClosure")
        {
            return Err(error("exact non-authorizing original source handoff observation required"));
        }
        for (key, expected) in [
            ("ownerId", json!(owner.owner_id)),
            ("origin", json!(owner.origin)),
            ("dataRoot", json!(owner.data_root)),
            ("projectId", json!(owner.project_id)),
            ("workspaceId", json!(owner.workspace_id)),
        ] {
            if identity[key] != expected || source[key] != expected || key != "dataRoot" && source["ready"][key] != expected {
                return Err(error("replacement source crossed the actual original owner scope"));
            }
        }
        if identity["ledgerFile"] != json!(owner.ledger_file)
            || identity["bootstrapFile"] != json!(owner.bootstrap_file)
            || !identity["operationId"].as_str().is_some_and(crate::editorial_workspace::uuid)
            || !identity["nextBuild"].as_str().is_some_and(hash_ok)
            || source["applicationRelease"] != identity["nextBuild"]
            || source["ready"]["pid"] != pid
            || source["ready"]["status"] != "ready"
            || source["runtimeRoot"] != control(original, 0)?.value()["editorialServer"]["runtimeRoot"]
        {
            return Err(error("replacement source release or original private scope changed"));
        }
        let old = &identity["old"];
        let old_pid = old["pid"].as_u64().filter(|n| *n > 1 && *n <= i32::MAX as u64).ok_or_else(|| error("actual retired source PID required"))?;
        if !fields(old, "kind,version,bootId,pid,build")
            || old["kind"] != "poema-original-service-instance"
            || old["version"] != 1
            || !old["bootId"].as_str().is_some_and(crate::editorial_workspace::uuid)
            || !old["build"].as_str().is_some_and(hash_ok)
            || old_pid == u64::from(pid)
        {
            return Err(error("the exact retired original source epoch is required"));
        }
        check_owner_exited(u32::try_from(old_pid).map_err(error)?).map_err(error)?;
        let context =
            b.manifest_path.parent().ok_or_else(|| error("original manifest parent missing"))?.join(format!("workspace-state-{}", b.frame.scope.workspace_id));
        let startup_path = context.join("editorial-service.json");
        if source["stateFile"] != json!(startup_path) {
            return Err(error("the same original source startup control path is required"));
        }
        let startup = Control::capture(&startup_path, CATALOG, check)?;
        if startup.value() != &source["ready"] {
            return Err(error("the actual source startup control differs from its original producer observation"));
        }
        let handoff = Control::capture(&context.join("editorial-save-recovery-handoff.json"), CATALOG, check)?;
        let record = handoff.value();
        if !fields(record, "kind,version,ownerId,workspaceId,projectId,origin,dataDir,operationId,old,nextBuild,phase,started,drained,replacement,completed")
            || record["kind"] != "poema-managed-original-service-handoff"
            || record["version"] != 1
            || record["phase"] != "complete"
            || record["dataDir"] != json!(owner.data_root)
            || record["old"] != *old
            || record["operationId"] != identity["operationId"]
            || record["nextBuild"] != identity["nextBuild"]
            || !fields(&record["replacement"], "pid,build")
            || record["replacement"]["pid"] != pid
            || record["replacement"]["build"] != identity["nextBuild"]
            || ["ownerId", "workspaceId", "projectId", "origin"].iter().any(|key| record[*key] != identity[*key])
        {
            return Err(error("actual original source handoff control does not confirm its same replacement"));
        }
        let result = Self { startup, handoff, pid };
        result.current(check)?;
        Ok(result)
    }
}
fn file_closure(value: &Value, preparation: &Value, rows: u64, source_pid: u32) -> Result<()> {
    if !fields(
        value,
        "kind,version,status,files,bytes,rows,sourcePid,nativePid,nativeSession,originalArchiveReverified,archiveFullyReadBeforeNative,originalArchiveGenerationsReobservedAfterSource,liveFilesRehashed,liveFilesRehashedBeforeNative,liveFilesRehashedAfterSource,liveFileGenerationsReobservedAfterSource,originalCompletedByteVerificationRetained,hashPhase,completeNamespacesRechecked,canonicalPublicationReobserved,reservedNativeRuntimeTreeReobserved,stoppedDataRootLeaseRetained,current,writeAuthority,nativeStartupAuthority,workersMayResume,fullApplicationClosure,runtimeReadiness,nativeAcknowledgement",
    ) || value["kind"] != "poema-original-held-Save-file-closure"
        || value["version"] != 1
        || value["status"] != "verified-complete-declared-file-union"
        || value["nativePid"] != std::process::id()
        || value["nativeSession"] != preparation["nativeSession"]
        || value["sourcePid"] != source_pid
        || value["rows"] != rows
        || rows > 2_000_000
        || value["files"].as_u64().is_none_or(|n| n > rows)
        || value["bytes"].as_u64().is_none_or(|n| n > 4 * 1024_u64.pow(4))
        || value["hashPhase"] != "original-complete-restoration-before-native"
        || value["runtimeReadiness"] != "not-inferred-from-hashes"
        || value["nativeAcknowledgement"] != "not-renewed-by-file-verification"
        || !false_fields(
            value,
            "liveFilesRehashedAfterSource,stoppedDataRootLeaseRetained,current,writeAuthority,nativeStartupAuthority,workersMayResume,fullApplicationClosure",
        )
        || [
            "originalArchiveReverified",
            "archiveFullyReadBeforeNative",
            "originalArchiveGenerationsReobservedAfterSource",
            "liveFilesRehashed",
            "liveFilesRehashedBeforeNative",
            "liveFileGenerationsReobservedAfterSource",
            "originalCompletedByteVerificationRetained",
            "completeNamespacesRechecked",
            "canonicalPublicationReobserved",
            "reservedNativeRuntimeTreeReobserved",
        ]
        .iter()
        .any(|key| value[*key] != true)
    {
        return Err(error("the same bounded complete held-file observation without renewed lease or release authority is required"));
    }
    Ok(())
}
impl Pending {
    pub(super) fn retain_application(&mut self, s: &Session, value: &Value) -> Result<Value> {
        if self.application.prepared.is_some() || self.application.attempted || self.application.checkpoint_attempted {
            return Err(error("the actual application preparation was already retained; no second preparation"));
        }
        self.current()?;
        self.worker_current(s.persistence.as_ref())?;
        self.check_canonical(s)?;
        if value["status"] != "actual-native-cold-application-prepared"
            || value["pid"] != std::process::id()
            || value["nativeSession"] != self.preparation["nativeSession"]
            || value["privateInput"] != self.preparation["privateInput"]
            || value["owner"]["runtime"] != self.description()?
            || value["projectPath"] != json!(s.path)
            || value["revision"] != s.revision
            || value["savedRevision"] != s.saved_revision
        {
            return Err(error("the actual original native application preparation changed"));
        }
        self.application.checkpoint_attempted = true;
        let checkpoint = checkpoint::Prepared::capture(s, &self.original, &self.preparation)?;
        checkpoint.current(s)?;
        let mut prepared = value.clone();
        let fields = prepared.as_object_mut().ok_or_else(|| error("actual native application preparation required"))?;
        fields.insert("checkpoint".into(), checkpoint.description());
        self.application.checkpoint = Some(checkpoint);
        self.application.prepared = Some(prepared.clone());
        self.current()?;
        Ok(prepared)
    }
    pub(super) fn prepare_application_checkpoint_bytes(&mut self, s: &Session, request: &Value) -> Result<Value> {
        bytes(request, 16384)?;
        if !fields(request, "kind,version,nativeSession,privateInput")
            || request["kind"] != "poema-original-Save-native-application-checkpoint-prepare"
            || request["version"] != 1
            || request["nativeSession"] != self.preparation["nativeSession"]
            || request["privateInput"] != self.preparation["privateInput"]
        {
            return Err(error("the same closed original checkpoint request and private native pipe are required"));
        }
        self.current()?;
        self.worker_current(s.persistence.as_ref())?;
        self.check_canonical(s)?;
        if !self.application.confirmed {
            return Err(error("the same original application intent must be acknowledged before checkpoint encoding"));
        }
        let source = self.application.source.as_ref().ok_or_else(|| error("the original running source controls are required"))?;
        let root = self.application.root.as_ref().ok_or_else(|| error("the original acknowledged application Root is required"))?;
        root.acknowledged_manifest()?;
        let records = self.application.records.as_ref().ok_or_else(|| error("the original acknowledged application records are required"))?;
        records.checkpoint_ready()?;
        let check = media_guard::Check::new(web_time::Instant::now());
        let current = || {
            check.check()?;
            source.current(&check)?;
            root.current()?;
            records.current(&check)?;
            check.check()
        };
        current()?;
        let checkpoint = self.application.checkpoint.as_mut().ok_or_else(|| error("the original complete captured COW checkpoint is required"))?;
        let result = checkpoint.encode(s, &current)?;
        // These are the same complete original source, worker, canonical and
        // COW checks as before the pure read, including the published Root and
        // exact acknowledged journal successor. No fresh baseline is created.
        check.check()?;
        self.current()?;
        self.worker_current(s.persistence.as_ref())?;
        self.check_canonical(s)?;
        check.check()?;
        Ok(result)
    }
    pub(super) fn prepare_application_checkpoint_archive_plan(&mut self, s: &Session, request: &Value) -> Result<Value> {
        bytes(request, 16384)?;
        if !fields(request, "kind,version,nativeSession,privateInput")
            || request["kind"] != "poema-original-Save-native-application-checkpoint-archive-prepare"
            || request["version"] != 1
            || request["nativeSession"] != self.preparation["nativeSession"]
            || request["privateInput"] != self.preparation["privateInput"]
        {
            return Err(error("the same closed original checkpoint archive request and private native pipe are required"));
        }
        self.current()?;
        self.worker_current(s.persistence.as_ref())?;
        self.check_canonical(s)?;
        if !self.application.confirmed {
            return Err(error("the original application intent must remain acknowledged before archive preparation"));
        }
        let source = self.application.source.as_ref().ok_or_else(|| error("the original source controls are required"))?;
        let root = self.application.root.as_ref().ok_or_else(|| error("the original acknowledged Root is required"))?;
        root.acknowledged_manifest()?;
        let records = self.application.records.as_ref().ok_or_else(|| error("the original acknowledged records are required"))?;
        records.checkpoint_ready()?;
        let check = media_guard::Check::new(web_time::Instant::now());
        let current = || {
            check.check()?;
            source.current(&check)?;
            root.current()?;
            records.current(&check)?;
            check.check()
        };
        current()?;
        let checkpoint = self.application.checkpoint.as_mut().ok_or_else(|| error("the same original captured checkpoint is required"))?;
        let plan = checkpoint.prepare_archive(s, &self.original, &self.preparation, &current)?;
        check.check()?;
        self.current()?;
        self.worker_current(s.persistence.as_ref())?;
        self.check_canonical(s)?;
        check.check()?;
        Ok(plan)
    }
    pub(super) fn confirm_application_checkpoint_archive_intent_successor(&mut self, s: &Session, value: &Value) -> Result<Value> {
        // The host has advanced the old journal head. Consume this SAME plan
        // before observing its exact successor; do not recapture an old head.
        let plan = self.application.checkpoint.as_mut().ok_or_else(|| error("the SAME actual retained checkpoint is required"))?.begin_archive_intent(s)?;
        bytes(value, INPUT)?;
        let receipt: IntentReceipt = parse("native.coldSaveCheckpointArchiveIntent", value)?;
        if receipt.kind != "poema-original-Save-native-checkpoint-archive-intent-receipt"
            || receipt.version != 1
            || !fields(&receipt.event, "kind,plan")
            || receipt.event["kind"] != "native-checkpoint-archive-intent"
            || receipt.event["plan"] != plan
        {
            return Err(error("the sole original checkpoint archive intent must name this exact actual retained plan"));
        }
        let check = media_guard::Check::new(web_time::Instant::now());
        let next = self.read_published_successor(&receipt.anchor, &receipt.head, receipt.event.clone(), &check)?;
        self.successor = Some(next);
        self.worker_current(s.persistence.as_ref())?;
        self.check_canonical(s)?;
        self.source_current()?;
        self.application.checkpoint.as_mut().ok_or_else(|| error("the actual checkpoint disappeared"))?.confirm_archive_intent()?;
        self.current()?;
        Ok(json!({"kind":"native_cold_Save_checkpoint_archive_intent_confirmation","version":1,
            "nativePid":std::process::id(),"nativeSession":self.preparation["nativeSession"],"event":receipt.event,"anchor":receipt.anchor,"head":receipt.head,
            "nativeAcknowledgementConfirmed":true,"editingPaused":true,"archiveCreated":false,"filesWritten":false,"workerReleased":false,
            "writeAuthority":false,"startupAuthority":false,"workersMayResume":false,"fullApplicationClosure":false}))
    }
    pub(super) fn confirm_application(&mut self, s: &Session, value: &Value) -> Result<Value> {
        let prepared = self.application.begin_confirmation()?;
        bytes(value, INPUT)?;
        let receipt: IntentReceipt = parse("native.coldSaveApplicationIntent", value)?;
        if receipt.kind != "poema-original-Save-native-application-intent-receipt"
            || receipt.version != 1
            || !fields(&receipt.event, "kind,application,source,fileClosure")
            || receipt.event["kind"] != "native-application-intent"
            || receipt.event["application"] != prepared
        {
            return Err(error("the sole original application intent must name this actual retained native preparation"));
        }
        let check = media_guard::Check::new(web_time::Instant::now());
        let source = Source::capture(&self.original, &receipt.event["source"], &check)?;
        let rows = self.original.body.basis.whole_journal.binding["rows"].as_u64().ok_or_else(|| error("original complete file-union row count required"))?;
        file_closure(&receipt.event["fileClosure"], &self.preparation, rows, source.pid)?;
        let next = self.read_published_successor(&receipt.anchor, &receipt.head, receipt.event.clone(), &check)?;
        self.successor = Some(next);
        self.application.source = Some(Arc::new(source));
        self.worker_current(s.persistence.as_ref())?;
        self.check_canonical(s)?;
        self.source_current()?;
        self.application.confirmed = true;
        self.current()?;
        Ok(json!({"kind":"native_cold_Save_application_intent_confirmation","version":1,
            "event":receipt.event,"anchor":receipt.anchor,"head":receipt.head,
            "nativePid":std::process::id(),"nativeSession":self.preparation["nativeSession"],
            "nativeAcknowledgementConfirmed":true,"editingPaused":true,"workerReleased":false,
            "writeAuthority":false,"startupAuthority":false,"workersMayResume":false,"fullApplicationClosure":false}))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn original_source_final_read_fence_keeps_actual_control_generations_pid_and_stop() {
        let root = crate::temp_dir().join(format!("poema-original-source-final-fence-{}", std::process::id()));
        std::fs::DirBuilder::new().mode(0o700).create(&root).unwrap();
        for name in ["startup.json", "handoff.json"] {
            let mut file = std::fs::OpenOptions::new().create_new(true).write(true).mode(0o600).open(root.join(name)).unwrap();
            file.write_all(b"{\"generated\":true}").unwrap();
            file.sync_all().unwrap();
        }
        let check = media_guard::Check::new(web_time::Instant::now());
        let source = Source {
            startup: Control::capture(&root.join("startup.json"), 4096, &check).unwrap(),
            handoff: Control::capture(&root.join("handoff.json"), 4096, &check).unwrap(),
            pid: std::process::id(),
        };
        source.current(&check).unwrap();
        source.fence_after_read(&check).unwrap();
        let stop = media_guard::Check::with_stop(web_time::Instant::now(), Arc::new(std::sync::atomic::AtomicBool::new(true)));
        assert!(source.fence_after_read(&stop).is_err());
        let path = root.join("handoff.json");
        let bytes = std::fs::read(&path).unwrap();
        std::fs::rename(&path, root.join("held-handoff.json")).unwrap();
        let mut file = std::fs::OpenOptions::new().create_new(true).write(true).mode(0o600).open(&path).unwrap();
        file.write_all(&bytes).unwrap();
        file.sync_all().unwrap();
        assert!(source.fence_after_read(&check).is_err());
        assert_eq!(std::fs::read(path).unwrap(), bytes);
    }
    #[test]
    fn failed_original_checkpoint_capture_keeps_the_same_application_unresolved_without_a_description_or_replay() {
        let mut application = Application::default();
        application.checkpoint_attempted = true;
        assert!(application.current().is_err());
        assert!(application.begin_confirmation().is_err());
        assert!(application.checkpoint.is_none());
    }
    #[test]
    fn serialized_application_preparation_cannot_supply_an_actual_retained_checkpoint() {
        let mut application = Application::default();
        application.prepared = Some(json!({"checkpoint":{"captured":true}}));
        assert!(application.current().is_err());
        assert!(application.checkpoint.is_none());
    }
    use std::io::Write;
    use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};

    fn closure() -> (Value, Value) {
        let preparation = json!({"nativeSession":"a".repeat(32)});
        let value = json!({"kind":"poema-original-held-Save-file-closure","version":1,"status":"verified-complete-declared-file-union",
            "files":4,"bytes":100,"rows":8,"sourcePid":101,"nativePid":std::process::id(),"nativeSession":preparation["nativeSession"],
            "originalArchiveReverified":true,"archiveFullyReadBeforeNative":true,"originalArchiveGenerationsReobservedAfterSource":true,
            "liveFilesRehashed":true,"liveFilesRehashedBeforeNative":true,"liveFilesRehashedAfterSource":false,
            "liveFileGenerationsReobservedAfterSource":true,"originalCompletedByteVerificationRetained":true,
            "hashPhase":"original-complete-restoration-before-native","completeNamespacesRechecked":true,
            "canonicalPublicationReobserved":true,"reservedNativeRuntimeTreeReobserved":true,
            "stoppedDataRootLeaseRetained":false,"current":false,"writeAuthority":false,"nativeStartupAuthority":false,
            "workersMayResume":false,"fullApplicationClosure":false,"runtimeReadiness":"not-inferred-from-hashes",
            "nativeAcknowledgement":"not-renewed-by-file-verification"});
        (value, preparation)
    }
    #[test]
    fn application_intent_requires_closed_complete_read_observations_without_renewing_the_stopped_lease() {
        let (value, preparation) = closure();
        file_closure(&value, &preparation, 8, 101).unwrap();
        for key in [
            "liveFilesRehashedAfterSource",
            "stoppedDataRootLeaseRetained",
            "current",
            "writeAuthority",
            "nativeStartupAuthority",
            "workersMayResume",
            "fullApplicationClosure",
        ] {
            let mut changed = value.clone();
            changed[key] = json!(true);
            assert!(file_closure(&changed, &preparation, 8, 101).is_err());
        }
        let mut unknown = value.clone();
        unknown["grant"] = json!(true);
        assert!(file_closure(&unknown, &preparation, 8, 101).is_err());
    }
    #[test]
    fn application_intent_refuses_crossed_native_source_row_and_archive_observations() {
        let (value, preparation) = closure();
        for (key, changed) in [
            ("nativePid", json!(0)),
            ("sourcePid", json!(102)),
            ("nativeSession", json!("b".repeat(32))),
            ("rows", json!(7)),
            ("files", json!(9)),
            ("bytes", json!(-1)),
            ("originalArchiveReverified", json!(false)),
            ("originalCompletedByteVerificationRetained", json!(false)),
            ("canonicalPublicationReobserved", json!(false)),
            ("reservedNativeRuntimeTreeReobserved", json!(false)),
        ] {
            let mut wrong = value.clone();
            wrong[key] = changed;
            assert!(file_closure(&wrong, &preparation, 8, 101).is_err());
        }
    }
    #[test]
    fn application_confirmation_consumes_one_actual_attempt_and_failed_descriptions_cannot_reissue_it() {
        let mut application = Application::default();
        assert!(application.begin_confirmation().is_err());
        application.prepared = Some(json!({"generatedDescriptionOnly":true}));
        application.begin_confirmation().unwrap();
        assert!(application.current().is_err());
        assert!(application.begin_confirmation().is_err());
        application.confirmed = true;
        // A boolean without the retained actual source controls is insufficient.
        assert!(application.current().is_err());
    }
    fn put(path: &Path, bytes: &[u8]) {
        let mut f = std::fs::OpenOptions::new().create_new(true).write(true).mode(0o600).open(path).unwrap();
        f.write_all(bytes).unwrap();
        f.sync_all().unwrap();
    }
    #[test]
    fn retained_application_source_controls_keep_stop_deadline_and_same_byte_replacement_fences() {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let root =
            crate::temp_dir().join(format!("poema-application-source-{}-{}", std::process::id(), NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)));
        std::fs::DirBuilder::new().mode(0o700).create(&root).unwrap();
        let startup = root.join("startup.json");
        let handoff = root.join("handoff.json");
        put(&startup, b"{\"generated\":true}");
        put(&handoff, b"{\"generated\":true}");
        let check = media_guard::Check::new(web_time::Instant::now());
        // Synthetic native file-witness composition only. Source::capture still
        // requires the actual foreign producer and original complete records.
        let source = Source {
            startup: Control::capture(&startup, CATALOG, &check).unwrap(),
            handoff: Control::capture(&handoff, CATALOG, &check).unwrap(),
            pid: std::process::id(),
        };
        source.current(&check).unwrap();
        let stop = media_guard::Check::with_stop(web_time::Instant::now(), Arc::new(std::sync::atomic::AtomicBool::new(true)));
        assert!(source.current(&stop).is_err());
        let expired = media_guard::Check::new(web_time::Instant::now() - std::time::Duration::from_secs(46));
        assert!(source.current(&expired).is_err());
        std::fs::rename(&startup, root.join("held-original-startup.json")).unwrap();
        put(&startup, b"{\"generated\":true}");
        assert!(source.current(&check).is_err());
        assert_eq!(std::fs::read(root.join("held-original-startup.json")).unwrap(), std::fs::read(&startup).unwrap());
    }
}
