//! Original stopped-owner identity is descriptive recovery data, never a grant.
use super::*;

pub(super) fn validate(value: &Value, manifest: &Value, owner: &Owner, directory: &Path, after_frame: &str) -> Result<()> {
    let source_owner = &value["owner"];
    let expected = serde_json::to_value(directory).map_err(error)?;
    if value["kind"] != "poema-original-Save-owner-recovery-manifest"
        || value["version"] != 1
        || value["directory"] != expected
        || value["source"]["manifest"] != *manifest
        || value["source"]["afterFrameSha256"] != after_frame
        || source_owner["ownerId"] != owner.owner_id
        || source_owner["origin"] != owner.origin
        || source_owner["dataRoot"] != serde_json::to_value(&owner.data_root).map_err(error)?
        || source_owner["projectId"] != owner.project_id
        || source_owner["workspaceId"] != owner.workspace_id
        || source_owner["ledgerFile"] != serde_json::to_value(&owner.ledger_file).map_err(error)?
        || source_owner["bootstrapFile"] != serde_json::to_value(&owner.bootstrap_file).map_err(error)?
    {
        return Err(error("original owner recovery manifest differs from the selected source scope"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn example() -> (Value, Value, Owner) {
        let owner = Owner {
            owner_id: "generated-owner".into(),
            origin: "http://127.0.0.1:19080".into(),
            data_root: "/generated/data".into(),
            project_id: "generated-project".into(),
            workspace_id: "generated-workspace".into(),
            ledger_file: Some("/generated/data/ledger.jsonl".into()),
            bootstrap_file: Some("/generated/data/bootstrap.json".into()),
        };
        let manifest = json!({"kind":"poema-saved-native-workspace","editorialServer":{"dataDir":"/generated/data","ownerFile":"/generated/private-owner.json","runtimeRoot":"/generated/runtime","logFile":"/generated/service.log"}});
        let value = json!({"kind":"poema-original-Save-owner-recovery-manifest","version":1,"directory":"/generated/recovery","source":{"manifest":manifest,"afterFrameSha256":"generated-frame"},"owner":{"ownerId":owner.owner_id,"origin":owner.origin,"dataRoot":owner.data_root,"projectId":owner.project_id,"workspaceId":owner.workspace_id,"ledgerFile":owner.ledger_file,"bootstrapFile":owner.bootstrap_file,"operationId":"retained-original-operation"}});
        (value, manifest, owner)
    }

    #[test]
    fn original_manifest_without_server_origin_uses_retained_owner_identity() {
        let (value, manifest, owner) = example();
        assert!(manifest["editorialServer"].get("origin").is_none());
        assert!(validate(&value, &manifest, &owner, Path::new("/generated/recovery"), "generated-frame").is_ok());
    }

    #[test]
    fn retained_owner_scope_rejects_crossed_identity_paths_source_and_after_frame() {
        let (value, manifest, owner) = example();
        for name in ["ownerId", "origin", "dataRoot", "projectId", "workspaceId", "ledgerFile", "bootstrapFile"] {
            let mut changed = value.clone();
            changed["owner"][name] = json!("crossed");
            assert!(validate(&changed, &manifest, &owner, Path::new("/generated/recovery"), "generated-frame").is_err(), "{name}");
        }
        for (name, replacement) in [("kind", json!("copied-status")), ("version", json!(2)), ("directory", json!("/generated/other"))] {
            let mut changed = value.clone();
            changed[name] = replacement;
            assert!(validate(&changed, &manifest, &owner, Path::new("/generated/recovery"), "generated-frame").is_err(), "{name}");
        }
        let mut changed = value.clone();
        changed["source"]["manifest"]["editorialServer"]["dataDir"] = json!("/generated/other");
        assert!(validate(&changed, &manifest, &owner, Path::new("/generated/recovery"), "generated-frame").is_err());
        assert!(validate(&value, &manifest, &owner, Path::new("/generated/recovery"), "different-frame").is_err());
    }
}
