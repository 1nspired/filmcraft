//! Original application records from the SAME retained Cold owner.
//! These are complete before observations and one exact native presentation
//! proposal. Nothing is published, settled, resumed or transferred here.
use super::*;
mod publication;

pub(super) struct Records {
    activity: Control,
    parent: Arc<DirectoryFrame>,
    native_state: NodeFrame,
    plan: Value,
    continuation: publication::Continuation,
}
impl Records {
    pub(super) fn checkpoint_ready(&self) -> Result<()> {
        self.continuation.checkpoint_ready()
    }
    pub(super) fn resolved(&self) -> Result<()> {
        self.continuation.resolved()
    }
    pub(super) fn current(&self, check: &media_guard::Check) -> Result<()> {
        self.read(check, true)
    }
    pub(super) fn fence_after_read(&self, check: &media_guard::Check) -> Result<()> {
        self.read(check, false)
    }
    fn read(&self, check: &media_guard::Check, read_bytes: bool) -> Result<()> {
        self.read_with_activity(check, read_bytes, &self.activity)
    }
    pub(super) fn read_with_activity(&self, check: &media_guard::Check, read_bytes: bool, activity: &Control) -> Result<()> {
        let io = || check.check().map_err(|e| io::Error::other(e.to_string()));
        if read_bytes {
            activity.current(check)?;
        }
        self.native_state.fence(&io).map_err(error)?;
        // A retained absence has no bytes to read. Refuse an occupied name
        // through its original native fence before observing any foreign file.
        if read_bytes && !matches!(self.native_state.state(), NodeState::Absent) {
            self.native_state.current(&io).map_err(error)?;
        }
        self.native_state.fence(&io).map_err(error)?;
        activity.fence(check)?;
        check.check()
    }
    pub(super) fn original_activity(&self) -> &Control {
        &self.activity
    }
    pub(super) fn assert_native_description(&self, description: &Value, check: &media_guard::Check) -> Result<()> {
        expected_record_node(&self.native_state, description, 4096, check)
    }
    pub(super) fn observe_settlement_control(&self, name: &str, description: &Value, check: &media_guard::Check) -> Result<Control> {
        if !matches!(name, "native-save-evidence.json" | "activity.json") {
            return Err(error("only the same original catalog and activity may receive an actual settlement successor"));
        }
        let io = || check.check().map_err(|e| io::Error::other(e.to_string()));
        self.parent.current(&io).map_err(error)?;
        let node = self.parent.node_bounded(name, CATALOG, &io).map_err(error)?;
        expected_record_node(&node, description, CATALOG, check)?;
        let descriptor: Observed = parse("native.coldSaveSettlementControl", description)?;
        let result = observed(&descriptor, CATALOG, check)?;
        node.fence(&io).map_err(error)?;
        self.parent.current(&io).map_err(error)?;
        check.check()?;
        Ok(result)
    }
}
fn native_replacement(original: &Value, old_pid: &Value, project_path: &Value, pid: u32, started: &Value) -> Result<Value> {
    if !observation_time(started)
        || pid < 2
        || pid > i32::MAX as u32
        || *old_pid == pid
        || project_path.as_str().is_none_or(|s| !literal(Path::new(s)))
        || !original.is_null() && (original.as_object().is_none() || original["pid"] != *old_pid || original["projectPath"] != *project_path)
    {
        return Err(error("the actual original native presentation and same new child start observation are required"));
    }
    bytes(original, 4096)?;
    let mut value = if original.is_null() { json!({"projectPath":project_path}) } else { original.clone() };
    let object = value.as_object_mut().ok_or_else(|| error("the original native presentation must remain an object"))?;
    object.insert("pid".into(), json!(pid));
    object.insert("started".into(), started.clone());
    bytes(&value, 4096)?;
    Ok(value)
}
fn activity_current(activity: &Value, catalog: &Value, digest: &str, selected: &str) -> Result<()> {
    bytes(activity, CATALOG as usize)?;
    let rows = catalog["operations"].as_array().filter(|v| v.len() <= 4).ok_or_else(|| error("the original bounded Save catalog is required"))?;
    if rows.iter().filter(|r| r["id"] == selected).count() != 1 {
        return Err(error("the original selected Save operation is missing or duplicated"));
    }
    let mut operations = Vec::new();
    for row in rows {
        operations
            .push(json!({"id":row["id"],"action":row["action"],"request":row["request"],"nativeSession":row["basis"]["nativeSession"],"phase":row["phase"]}));
        if row["phase"] != "cancelled"
            && row["phase"] != "refused"
            && (row["action"] != activity["operationId"]
                || row["dispatchIntent"].get("sendJob").is_some_and(|j| *j != activity["job"]["id"])
                || !["idle", "active", "uncertain", "editorial_uncertain"].iter().any(|s| activity["status"] == *s))
        {
            return Err(error("the original unresolved Save activity belongs to another actor or accepted Send"));
        }
    }
    let expected = json!({"kind":"original_Save_activity","version":1,"catalog":"native-save-evidence.json","sha256":digest,"operations":operations,
        "current":false,"writeAuthority":false,"nativeComplete":false,"startupAuthority":false,"requestReplay":false});
    if activity["nativeSave"] != expected {
        return Err(error("the original complete Save activity pointer differs from its retained catalog"));
    }
    Ok(())
}
// Bind the actual parent's original observations before interpreting or
// returning record contents. A fresh path observation is not a replacement
// source baseline and cannot turn a changed/foreign file into an owner.
fn expected_record_node(node: &NodeFrame, expected: &Value, max: u64, check: &media_guard::Check) -> Result<()> {
    let io = || check.check().map_err(|e| io::Error::other(e.to_string()));
    node.fence(&io).map_err(error)?;
    match node.state() {
        NodeState::Absent if expected.is_null() => {}
        NodeState::File(actual) => {
            let observed: Observed = parse("native.coldSaveApplicationRecords.expected", expected)?;
            if observed.path != node.path() || observed.bytes == 0 || observed.bytes > max || observed.sha256 != actual.sha256 || observed.bytes != actual.bytes
            {
                return Err(error("the actual application record differs from the same parent's retained bytes or path"));
            }
            observed.physical.matches(actual)?;
        }
        _ => return Err(error("the actual application record type differs from its retained original presence or absence")),
    }
    node.fence(&io).map_err(error)?;
    check.check()
}

impl Pending {
    pub(in crate::agent_bridge::history_checkpoint::project_recovery_snapshot::save_startup::runtime_transition) fn prepare_application_records_plan(
        &mut self,
        s: &Session,
        request: &Value,
    ) -> Result<Value> {
        if self.application.records_attempted {
            return Err(error("the original application records preparation was already attempted; no replay"));
        }
        self.current()?;
        self.worker_current(s.persistence.as_ref())?;
        self.check_canonical(s)?;
        let root = self.application.root.as_ref().ok_or_else(|| error("the same actual published original Root is required"))?;
        root.acknowledged_manifest()?;
        self.application.records_attempted = true;
        bytes(request, 32768)?;
        if !fields(request, "kind,version,nativeSession,privateInput,started,records")
            || request["kind"] != "poema-original-Save-native-application-records-prepare"
            || request["version"] != 1
            || request["nativeSession"] != self.preparation["nativeSession"]
            || request["privateInput"] != self.preparation["privateInput"]
            || !observation_time(&request["started"])
            || !fields(&request["records"], "activity,nativeState")
            || !fields(&request["records"]["nativeState"], "path,before")
        {
            return Err(error("the same original private child and bounded native start description are required"));
        }
        let check = media_guard::Check::new(web_time::Instant::now());
        let io = || check.check().map_err(|e| io::Error::other(e.to_string()));
        let basis = &self.original.body.basis;
        let directory = basis.input.catalog_file.parent().ok_or_else(|| error("the original catalog parent is missing"))?;
        if !self.original.within(directory) || basis.input.catalog_file.file_name().and_then(|s| s.to_str()) != Some("native-save-evidence.json") {
            return Err(error("the original application record directory differs from its retained catalog"));
        }
        let parent = DirectoryFrame::capture(directory, &io).map_err(error)?;
        let native_state = parent.node_bounded("native.json", 4096, &io).map_err(error)?;
        if request["records"]["nativeState"]["path"] != json!(native_state.path()) {
            return Err(error("the retained native presentation path crossed the original catalog directory"));
        }
        expected_record_node(&native_state, &request["records"]["nativeState"]["before"], 4096, &check)?;
        let (native_original, native_before) = match native_state.state() {
            NodeState::Absent => (Value::Null, Value::Null),
            NodeState::File(_) => {
                let value: Value = serde_json::from_slice(&native_state.read_private_bytes(4096, &io).map_err(error)?).map_err(error)?;
                if !value.is_object() {
                    return Err(error("an existing native presentation must remain a complete object; JSON null is not an absence"));
                }
                let actual = Control::capture(&native_state.path(), 4096, &check)?;
                (value, control_description(&actual, &native_state.path())?)
            }
            NodeState::Directory(_) => return Err(error("the original native presentation path is a directory; nothing was replaced")),
        };
        let catalog = control(&self.original, 2)?;
        let activity_node = parent.node_bounded("activity.json", CATALOG, &io).map_err(error)?;
        expected_record_node(&activity_node, &request["records"]["activity"], CATALOG, &check)?;
        let activity_descriptor: Observed = parse("native.coldSaveApplicationRecords.activity", &request["records"]["activity"])?;
        let activity = observed(&activity_descriptor, CATALOG, &check)?;
        activity_node.fence(&io).map_err(error)?;
        activity_current(activity.value(), catalog.value(), &catalog.observation()?.sha256, &basis.input.evidence_id)?;
        let original_manifest = control(&self.original, 0)?.value();
        let replacement = native_replacement(&native_original, &original_manifest["nativePid"], &json!(s.path), std::process::id(), &request["started"])?;
        let root = self.application.root.as_ref().ok_or_else(|| error("the same actual original Root disappeared"))?;
        let manifest = root.acknowledged_manifest()?;
        let plan = json!({"kind":"poema-native-Save-application-records-plan","version":1,
            "nativePid":std::process::id(),"nativeSession":self.preparation["nativeSession"],"privateInput":self.preparation["privateInput"],
            "manifest":control_description(manifest,&basis.manifest_path)?,
            "catalog":{"observation":control_description(catalog,&basis.input.catalog_file)?,"value":catalog.value()},
            "activity":{"observation":control_description(&activity,&directory.join("activity.json"))?,"value":activity.value()},
            "nativeState":{"path":native_state.path(),"before":native_before,"original":native_original,"replacement":replacement},
            "editingPaused":true,"workerReleased":false,"writeAuthority":false,"fullApplicationClosure":false});
        bytes(&plan, INPUT)?;
        let retained = Records { activity, parent, native_state, plan, continuation: publication::Continuation::default() };
        retained.current(&check)?;
        self.application.records = Some(Arc::new(retained));
        self.current()?;
        self.worker_current(s.persistence.as_ref())?;
        self.check_canonical(s)?;
        let records = self.application.records.as_ref().ok_or_else(|| error("the actual application records disappeared"))?;
        Ok(records.plan.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn original_record_final_read_fence_refuses_same_byte_native_and_activity_replacements() {
        use std::io::Write;
        use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
        for name in ["activity.json", "native.json"] {
            let root = crate::temp_dir().join(format!("poema-records-final-fence-{}-{name}", std::process::id()));
            std::fs::DirBuilder::new().mode(0o700).create(&root).unwrap();
            let put = |path: &Path| {
                let mut f = std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(path).unwrap();
                f.write_all(b"{\"generated\":true}").unwrap();
                f.sync_all().unwrap();
            };
            put(&root.join("activity.json"));
            put(&root.join("native.json"));
            let check = media_guard::Check::new(web_time::Instant::now());
            let parent = DirectoryFrame::capture(&root, &|| Ok(())).unwrap();
            let records = Records {
                activity: Control::capture(&root.join("activity.json"), CATALOG, &check).unwrap(),
                native_state: parent.node_bounded("native.json", 4096, &|| Ok(())).unwrap(),
                parent,
                plan: json!({"generatedReadOnlyWitness":true}),
                continuation: publication::Continuation::default(),
            };
            records.current(&check).unwrap();
            records.fence_after_read(&check).unwrap();
            let stop = media_guard::Check::with_stop(web_time::Instant::now(), Arc::new(std::sync::atomic::AtomicBool::new(true)));
            assert!(records.fence_after_read(&stop).is_err());
            std::fs::rename(root.join(name), root.join("held-original.json")).unwrap();
            put(&root.join(name));
            assert!(records.fence_after_read(&check).is_err());
            assert_eq!(std::fs::read(root.join(name)).unwrap(), b"{\"generated\":true}");
        }
    }
    #[test]
    fn original_host_record_observations_bind_file_bytes_generations_types_and_paths_before_content_delivery() {
        use std::io::Write;
        use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
        let root = crate::temp_dir().join(format!("poema-application-record-binding-{}", std::process::id()));
        std::fs::DirBuilder::new().mode(0o700).create(&root).unwrap();
        let path = root.join("native.json");
        let mut file = std::fs::OpenOptions::new().create_new(true).write(true).mode(0o600).open(&path).unwrap();
        file.write_all(b"{\"generated\":true}").unwrap();
        file.sync_all().unwrap();
        let check = media_guard::Check::new(web_time::Instant::now());
        let io = || check.check().map_err(|e| std::io::Error::other(e.to_string()));
        let parent = DirectoryFrame::capture(&root, &io).unwrap();
        let node = parent.node_bounded("native.json", 4096, &io).unwrap();
        let control = Control::capture(&path, 4096, &check).unwrap();
        let expected = control_description(&control, &path).unwrap();
        expected_record_node(&node, &expected, 4096, &check).unwrap();
        for value in [Value::Null, json!({}), json!({"path":"/foreign/native.json"})] {
            assert!(expected_record_node(&node, &value, 4096, &check).is_err());
        }
        for (key, value) in [("sha256", json!("f".repeat(64))), ("path", json!("/foreign/native.json")), ("bytes", json!(4097))] {
            let mut wrong = expected.clone();
            wrong[key] = value;
            assert!(expected_record_node(&node, &wrong, 4096, &check).is_err());
        }
        let mut wrong = expected.clone();
        wrong["physical"]["ino"] = json!("0");
        assert!(expected_record_node(&node, &wrong, 4096, &check).is_err());
        let absent = parent.node_bounded("missing.json", 4096, &io).unwrap();
        expected_record_node(&absent, &Value::Null, 4096, &check).unwrap();
        assert!(expected_record_node(&absent, &expected, 4096, &check).is_err());
    }
    #[test]
    fn application_record_witnesses_refuse_stop_changed_absence_and_same_byte_activity_replacement() {
        use std::io::Write;
        use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        fn put(p: &Path, bytes: &[u8]) {
            let mut f = std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(p).unwrap();
            f.write_all(bytes).unwrap();
            f.sync_all().unwrap();
        }
        for absent in [true, false] {
            let root =
                crate::temp_dir().join(format!("poema-application-records-{}-{}", std::process::id(), NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)));
            std::fs::DirBuilder::new().mode(0o700).create(&root).unwrap();
            let activity = root.join("activity.json");
            put(&activity, b"{\"generated\":true}");
            let native = root.join("native.json");
            if !absent {
                put(&native, b"{\"generated\":true}");
            }
            let check = media_guard::Check::new(web_time::Instant::now());
            let io = || check.check().map_err(|e| std::io::Error::other(e.to_string()));
            let parent = DirectoryFrame::capture(&root, &io).unwrap();
            let records = Records {
                activity: Control::capture(&activity, CATALOG, &check).unwrap(),
                native_state: parent.node_bounded("native.json", 4096, &io).unwrap(),
                parent,
                plan: json!({"generatedWitnessCompositionOnly":true}),
                continuation: publication::Continuation::default(),
            };
            records.current(&check).unwrap();
            let stop = media_guard::Check::with_stop(web_time::Instant::now(), std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true)));
            assert!(records.current(&stop).is_err());
            records.fence_after_read(&check).unwrap();
            assert!(records.fence_after_read(&stop).is_err());
            let expired = media_guard::Check::new(web_time::Instant::now() - std::time::Duration::from_secs(46));
            assert!(records.current(&expired).is_err());
            assert!(records.fence_after_read(&expired).is_err());
            if absent {
                put(&native, b"{\"generated\":true}");
                assert!(records.current(&check).is_err());
                assert!(records.fence_after_read(&check).is_err());
            } else {
                std::fs::rename(&activity, root.join("held-original-activity.json")).unwrap();
                put(&activity, b"{\"generated\":true}");
                assert!(records.current(&check).is_err());
                assert!(records.fence_after_read(&check).is_err());
                assert_eq!(std::fs::read(&activity).unwrap(), std::fs::read(root.join("held-original-activity.json")).unwrap());
            }
        }
    }
    #[test]
    fn native_presentation_replacement_preserves_original_fields_and_only_advances_the_actual_child() {
        let original = json!({"pid":101,"projectPath":"/generated/project.fcproj","started":"original","future":{"kept":true}});
        let next = native_replacement(&original, &json!(101), &json!("/generated/project.fcproj"), 202, &json!("actual-child-start")).unwrap();
        assert_eq!(next, json!({"pid":202,"projectPath":"/generated/project.fcproj","started":"actual-child-start","future":{"kept":true}}));
        assert_eq!(original["pid"], 101);
        assert!(native_replacement(&Value::Null, &json!(101), &json!("/generated/project.fcproj"), 202, &json!("actual-child-start")).is_ok());
        for pid in [0, 1, 101, u32::MAX] {
            assert!(native_replacement(&original, &json!(101), &json!("/generated/project.fcproj"), pid, &json!("start")).is_err());
        }
        for (old, path, start) in [
            (json!(102), json!("/generated/project.fcproj"), json!("start")),
            (json!(101), json!("/foreign/project.fcproj"), json!("start")),
            (json!(101), json!("/generated/project.fcproj"), json!("bad\nstart")),
        ] {
            assert!(native_replacement(&original, &old, &path, 202, &start).is_err());
        }
    }
    #[test]
    fn original_save_activity_records_require_the_exact_catalog_pointer_and_accepted_send() {
        let row = json!({"id":"selected","action":"accepted-action","request":{"job":"sdk-job","operation":"sdk-operation"},"basis":{"nativeSession":"original-epoch"},"phase":"needs_observation","dispatchIntent":{"sendJob":"human-send"}});
        let catalog = json!({"operations":[row]});
        let pointer = json!({"kind":"original_Save_activity","version":1,"catalog":"native-save-evidence.json","sha256":"a".repeat(64),
            "operations":[{"id":"selected","action":"accepted-action","request":{"job":"sdk-job","operation":"sdk-operation"},"nativeSession":"original-epoch","phase":"needs_observation"}],
            "current":false,"writeAuthority":false,"nativeComplete":false,"startupAuthority":false,"requestReplay":false});
        let value = json!({"status":"active","operationId":"accepted-action","job":{"id":"human-send"},"nativeSave":pointer});
        activity_current(&value, &catalog, &"a".repeat(64), "selected").unwrap();
        let mut wrong = value.clone();
        wrong["job"]["id"] = json!("sdk-job");
        assert!(activity_current(&wrong, &catalog, &"a".repeat(64), "selected").is_err());
        let mut wrong = value.clone();
        wrong["nativeSave"]["nativeComplete"] = json!(true);
        assert!(activity_current(&wrong, &catalog, &"a".repeat(64), "selected").is_err());
        assert!(activity_current(&value, &catalog, &"b".repeat(64), "selected").is_err());
        assert!(activity_current(&value, &catalog, &"a".repeat(64), "other").is_err());
    }
}
