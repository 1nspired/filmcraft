use super::{
    assembly_tests,
    tests::{exact_batch, read, token},
};
use crate::Session;
use filmcraft_project::{ItemId, MediaRef};
use serde_json::{Value, json};
use std::{
    fs,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};
static NEXT: AtomicUsize = AtomicUsize::new(0);
struct Scratch(PathBuf);
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn setup() -> (Session, Scratch) {
    let (mut s, item) = assembly_tests::fixture();
    let dir = Scratch(std::env::temp_dir().join(format!("filmcraft-read-projection-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed))));
    fs::create_dir(&dir.0).unwrap();
    let file = dir.0.join("original.bin");
    fs::write(&file, b"original generation").unwrap();
    Arc::make_mut(&mut s.project).item_mut(ItemId(item)).unwrap().as_media_mut().unwrap().media = MediaRef::File { path: file.to_string_lossy().into_owned() };
    (s, dir)
}
fn scope(s: &Session) -> Value {
    json!({"projectId":"p_test","anchorSequence":s.state.active_sequence.unwrap().0})
}
fn state(s: &Session) -> Value {
    json!({"project":s.project,"state":s.state,"revision":s.revision,"saved":s.saved_revision,"undo":s.history.undo.len(),"redo":s.history.redo.len(),"journal":s.journal,"events":s.events.len()})
}

#[test]
fn source_catalog_needs_no_original_file_and_creates_no_read_or_edit_ticket() {
    let (mut s, d) = setup();
    s.path = Some(d.0.join("copy.fcproj").to_string_lossy().into_owned());
    let request = json!({"scope":scope(&s)});
    let first = s.execute("agent.readSourceCatalog", request.clone()).unwrap();
    fs::remove_file(d.0.join("original.bin")).unwrap();
    let before = state(&s);
    let after = s.execute("agent.readSourceCatalog", request).unwrap();
    assert_eq!(first, after);
    assert_eq!(after["kind"], "native_source_catalog");
    assert_eq!(after["requiresSourceValidation"], true);
    assert_eq!(after["writeAuthority"], false);
    assert!(after.get("token").is_none());
    assert!(s.agent_bridge.tickets.is_empty());
    assert!(s.agent_bridge.media.is_none());
    assert!(after["sources"].as_array().unwrap().iter().any(|row| row["file"] == true));
    for row in after["sources"].as_array().unwrap() {
        assert!(row.get("media").is_none());
        assert!(row.get("fileVerification").is_none());
    }
    let clip = s.active_sequence().unwrap().video_tracks[0].items[0].id.0;
    assert!(
        s.execute(
            "agent.apply",
            exact_batch(73900, vec![json!({"command":"timeline.trim","params":{"clip":clip,"edge":"out","mode":"regular","deltaFrames":-1}})])
        )
        .is_err()
    );
    assert_eq!(state(&s), before);
}

#[test]
fn source_catalog_rejects_wrong_scope_unknown_fields_and_oversized_metadata_without_state_changes() {
    let (mut s, _d) = setup();
    let request = json!({"scope":scope(&s)});
    let before = state(&s);
    for p in [Value::Null, json!({"scope":scope(&s),"token":token(73901)}), json!({"scope":{"projectId":"p_test","anchorSequence":9_000_000}})] {
        assert!(s.execute("agent.readSourceCatalog", p).is_err());
        assert_eq!(state(&s), before);
    }
    let item = s.project.items.values().find(|it| it.as_media().is_some()).unwrap().id;
    Arc::make_mut(&mut s.project).item_mut(item).unwrap().name = "x".repeat(1025);
    let before = state(&s);
    assert!(s.execute("agent.readSourceCatalog", request.clone()).is_err());
    assert_eq!(state(&s), before);
    Arc::make_mut(&mut s.project).item_mut(item).unwrap().name = "Original".into();
    Arc::make_mut(&mut s.project).item_mut(item).unwrap().metadata.insert(super::BINDING_KEY.into(), " ".repeat(8193));
    let before = state(&s);
    assert!(s.execute("agent.readSourceCatalog", request).is_err());
    assert_eq!(state(&s), before);
    assert!(s.agent_bridge.tickets.is_empty());
    assert!(s.agent_bridge.media.is_none());
}
#[test]
fn metadata_basis_clock_and_caption_projections_do_not_renew_changed_file_authority() {
    let (mut s, d) = setup();
    let root = s.state.active_sequence.unwrap().0;
    let scope = scope(&s);
    let snapshot = read(&mut s, 73000);
    assert_eq!(snapshot["capabilities"]["historyAvailabilityProjection"], true);
    let before = state(&s);
    let tickets = s.agent_bridge.tickets.len();
    let basis = s.execute("agent.readBasis", json!({"token":token(73000)})).unwrap();
    assert_eq!(
        basis,
        json!({"status":"basis","kind":"native_read_basis","token":token(73000),"revision":s.revision,"sequence":root,"requiresSourceValidation":true,"writeAuthority":false})
    );
    let clock = json!({"token":token(73000),"scope":scope,"frames":[0],"projection":true});
    let caption = json!({"token":token(73000),"scope":scope,"sequence":root,"projection":true});
    let coordinates = s.execute("agent.sourceFrames", clock.clone()).unwrap();
    let destination = s.execute("agent.readCaptionDestination", caption.clone()).unwrap();
    assert_eq!(coordinates["requiresSourceValidation"], true);
    assert_eq!(destination["requiresSourceValidation"], true);
    fs::remove_file(d.0.join("original.bin")).unwrap();
    assert_eq!(s.execute("agent.readBasis", json!({"token":token(73000)})).unwrap(), basis);
    assert_eq!(s.execute("agent.sourceFrames", clock).unwrap(), coordinates);
    assert_eq!(s.execute("agent.readCaptionDestination", caption).unwrap(), destination);
    assert!(s.execute("agent.validateSnapshot", json!({"token":token(73000)})).is_err());
    assert!(s.execute("agent.sourceFrames", json!({"token":token(73000),"scope":scope,"frames":[0]})).is_err());
    assert!(s.execute("agent.readCaptionDestination", json!({"token":token(73000),"scope":scope,"sequence":root})).is_err());
    let clip = s.active_sequence().unwrap().video_tracks[0].items[0].id.0;
    assert!(
        s.execute(
            "agent.apply",
            exact_batch(73000, vec![json!({"command":"timeline.trim","params":{"clip":clip,"edge":"out","mode":"regular","deltaFrames":-1}})])
        )
        .is_err()
    );
    assert_eq!(state(&s), before);
    assert_eq!(s.agent_bridge.tickets.len(), tickets);
}
#[test]
fn projected_metadata_refuses_replaced_documents_human_edits_and_revoked_tickets() {
    for mode in ["replacement", "human", "revoked"] {
        let (mut s, _d) = setup();
        let scope = scope(&s);
        let root = s.state.active_sequence.unwrap().0;
        read(&mut s, 73010);
        match mode {
            "replacement" => s.project = Arc::new((*s.project).clone()),
            "human" => {
                let clip = s.active_sequence().unwrap().video_tracks[0].items[0].id.0;
                s.execute("timeline.trim", json!({"clip":clip,"edge":"out","mode":"regular","deltaFrames":-1})).unwrap();
            }
            _ => {
                s.execute("agent.cancelSnapshot", json!({"job":token(73010)})).unwrap();
            }
        }
        let before = state(&s);
        for (command, params) in [
            ("agent.readBasis", json!({"token":token(73010)})),
            ("agent.sourceFrames", json!({"token":token(73010),"scope":scope,"frames":[0],"projection":true})),
            ("agent.readCaptionDestination", json!({"token":token(73010),"scope":scope,"sequence":root,"projection":true})),
        ] {
            assert!(s.execute(command, params).is_err(), "{mode} {command}");
        }
        assert_eq!(state(&s), before);
    }
}
#[test]
fn caption_projection_still_requires_explicit_destination_file_scope() {
    let (mut s, _d) = setup();
    let root = s.state.active_sequence.unwrap();
    let scope = scope(&s);
    s.execute("file.newSequence", json!({"name":"Current sibling","video":1,"audio":1})).unwrap();
    let child = s.state.active_sequence.unwrap();
    Arc::make_mut(&mut s.project).item_mut(child).unwrap().metadata.insert(super::sequences::KEY.into(), scope.to_string());
    s.execute("agent.snapshot", json!({"token":token(73020),"mediaScope":{}})).unwrap();
    let before = state(&s);
    assert!(s.execute("agent.readCaptionDestination", json!({"token":token(73020),"scope":scope,"sequence":root.0,"projection":true})).is_err());
    assert_eq!(state(&s), before);
    s.execute("agent.snapshot", json!({"token":token(73021),"mediaScope":{"sequences":[root.0]}})).unwrap();
    assert_eq!(
        s.execute("agent.readCaptionDestination", json!({"token":token(73021),"scope":scope,"sequence":root.0,"projection":true})).unwrap()["requiresSourceValidation"],
        true
    );
    assert_eq!(s.state.active_sequence, Some(child));
}
#[test]
fn malformed_projection_shapes_never_create_a_new_read_or_touch_native_history() {
    let (mut s, _d) = setup();
    let root = s.state.active_sequence.unwrap().0;
    let scope = scope(&s);
    read(&mut s, 73030);
    let before = state(&s);
    let tickets = s.agent_bridge.tickets.len();
    for projection in [Value::Null, json!(1), json!("true")] {
        assert!(s.execute("agent.sourceFrames", json!({"token":token(73030),"scope":scope,"frames":[0],"projection":projection})).is_err());
        assert!(s.execute("agent.readCaptionDestination", json!({"token":token(73030),"scope":scope,"sequence":root,"projection":projection})).is_err());
    }
    for p in [json!({"token":token(73030),"job":token(73031)}), json!({"token":token(73030),"candidate":token(73031)}), json!({"token":"unknown"})] {
        assert!(s.execute("agent.readBasis", p).is_err());
    }
    assert_eq!(state(&s), before);
    assert_eq!(s.agent_bridge.tickets.len(), tickets);
}

fn empty_library_project() -> (Session, Value) {
    let mut s = Session::default();
    let scope = json!({"projectId":"p_empty_library","workspaceId":"11111111-1111-4111-8111-111111111111"});
    s.execute("file.newProject", json!({"name":"Generated independent source library","workspace":scope})).unwrap();
    s.path = Some("/generated/unavailable/library.fcproj".into());
    (s, scope)
}
#[test]
fn project_source_catalog_has_real_absence_exact_root_identity_and_no_ticket_or_source_authority() {
    let (mut s, scope) = empty_library_project();
    let before = state(&s);
    let value = s.execute("agent.readSourceCatalog", json!({"scope":scope})).unwrap();
    assert_eq!(value["version"], 2);
    assert_eq!(value["scope"], scope);
    assert_eq!(value["sequence"], Value::Null);
    assert_eq!(value["requiresSourceValidation"], true);
    assert_eq!(value["writeAuthority"], false);
    assert!(value.get("token").is_none());
    assert!(s.agent_bridge.tickets.is_empty());
    assert!(s.agent_bridge.media.is_none());
    assert_eq!(state(&s), before);
}
#[test]
fn project_source_catalog_observes_missing_imported_original_metadata_without_resolving_or_reading_its_path() {
    let (source, id) = assembly_tests::fixture();
    let mut item = source.project.item(ItemId(id)).unwrap().clone();
    item.id = ItemId(1000);
    item.metadata.clear();
    item.as_media_mut().unwrap().media = MediaRef::File { path: "/generated/unavailable/original.MXF".into() };
    let (mut s, scope) = empty_library_project();
    Arc::make_mut(&mut s.project).items.insert(item.id, item);
    let before = state(&s);
    let value = s.execute("agent.readSourceCatalog", json!({"scope":scope})).unwrap();
    assert_eq!(value["sequence"], Value::Null);
    assert_eq!(value["sources"].as_array().unwrap().len(), 1);
    assert_eq!(value["sources"][0]["item"], 1000);
    assert_eq!(value["sources"][0]["file"], true);
    assert!(value["sources"][0].get("media").is_none());
    assert!(value["sources"][0].get("fileVerification").is_none());
    assert!(s.agent_bridge.tickets.is_empty());
    assert!(s.agent_bridge.media.is_none());
    assert_eq!(state(&s), before);
}
#[test]
fn project_source_catalog_refuses_foreign_ambiguous_or_unknown_scope_fields_without_changing_the_empty_project() {
    let (mut s, scope) = empty_library_project();
    let before = state(&s);
    for request in [
        json!({"scope":{"projectId":"p_foreign","workspaceId":scope["workspaceId"]}}),
        json!({"scope":{"projectId":scope["projectId"],"workspaceId":"22222222-2222-4222-8222-222222222222"}}),
        json!({"scope":{"projectId":scope["projectId"],"workspaceId":scope["workspaceId"],"anchorSequence":1}}),
        json!({"scope":scope,"token":"must-not-mint-a-source-ticket"}),
        json!({"scope":{"projectId":scope["projectId"],"anchorSequence":0}}),
    ] {
        assert!(s.execute("agent.readSourceCatalog", request).is_err());
        assert_eq!(state(&s), before);
    }
    assert!(s.agent_bridge.tickets.is_empty());
    assert!(s.agent_bridge.media.is_none());
}
#[test]
fn project_source_catalog_reports_the_actual_open_or_closed_view_without_needing_it_as_a_library_anchor() {
    let (mut s, scope) = empty_library_project();
    let id = s.execute("file.newSequence", json!({"name":"Human sequence","fps":24})).unwrap()["sequence"].as_u64().unwrap();
    let open = s.execute("agent.readSourceCatalog", json!({"scope":scope})).unwrap();
    assert_eq!(open["sequence"], id);
    assert_eq!(open["version"], 2);
    s.state.active_sequence = None;
    s.state.open_sequences.clear();
    let before = state(&s);
    let closed = s.execute("agent.readSourceCatalog", json!({"scope":scope})).unwrap();
    assert_eq!(closed["sequence"], Value::Null);
    assert_eq!(s.project.sequences().count(), 1);
    assert_eq!(state(&s), before);
}
