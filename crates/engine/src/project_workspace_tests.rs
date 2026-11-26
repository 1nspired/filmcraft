use crate::{
    Session, editorial_workspace,
    project_workspace::{self, KEY},
};
use filmcraft_project::{BinEntry, ItemId};
use serde_json::{Value, json};
use std::{fs, sync::Arc};

const WORKSPACE: &str = "6ae17621-32f1-4e2b-8940-aef47d1279a5";
fn params() -> Value {
    json!({"name":"Empty project","workspace":{"projectId":"p_empty","workspaceId":WORKSPACE}})
}
fn setup() -> Session {
    let mut s = Session::default();
    s.execute("file.newProject", params()).unwrap();
    s
}
fn state(s: &Session) -> Value {
    json!({"project":s.project,"editor":s.state,"revision":s.revision,"saved":s.saved_revision,"undo":s.history.undo,"redo":s.history.redo,"path":s.path})
}
fn create(s: &mut Session) -> ItemId {
    ItemId(
        s.execute(
            "file.newSequence",
            json!({"name":"First sequence","width":3840,"height":2160,"fps":{"num":24000,"den":1001},"sampleRate":96000,"mix":"Mono","video":2,"audio":4}),
        )
        .unwrap()["sequence"]
            .as_u64()
            .unwrap(),
    )
}
#[test]
fn project_workspace_new_project_has_real_saved_identity_without_a_sequence_or_synthetic_item() {
    let s = setup();
    assert!(s.project.items.is_empty());
    assert!(s.state.active_sequence.is_none());
    assert!(s.project.root.children.is_empty());
    assert_eq!(s.project.next_id, 1);
    assert!(s.history.undo.is_empty() && s.history.redo.is_empty());
    assert!(s.path.is_none());
    let identity = project_workspace::identity(&s.project).unwrap().unwrap();
    assert_eq!(identity.scope.project_id, "p_empty");
    assert_eq!(identity.scope.workspace_id, WORKSPACE);
    assert!(editorial_workspace::association(&s.project).unwrap().is_none());
    let saved = filmcraft_format::encode(&s.project, false);
    let loaded = filmcraft_format::decode(&saved).unwrap();
    assert_eq!(loaded.project, *s.project);
    assert_eq!(loaded.schema_version, 18);
}
#[test]
fn project_workspace_malformed_creation_preserves_existing_edit_path_history_and_redo() {
    let mut s = setup();
    create(&mut s);
    s.path = Some("/generated/retained.fcproj".into());
    s.execute("file.newBin", json!({"name":"Later human"})).unwrap();
    s.undo().unwrap();
    let before = state(&s);
    for p in [
        json!(null),
        json!([]),
        json!({"name":null}),
        json!({"name":""}),
        json!({"name":"bad\nname"}),
        json!({"name":"a".repeat(1025)}),
        json!({"workspace":null}),
        json!({"workspace":{}}),
        json!({"workspace":{"projectId":"foreign/path","workspaceId":WORKSPACE}}),
        json!({"workspace":{"projectId":"p_empty","workspaceId":"wrong"}}),
        json!({"workspace":{"projectId":"p_empty","workspaceId":WORKSPACE,"anchorSequence":1}}),
        json!({"workspace":{"projectId":"a".repeat(257),"workspaceId":WORKSPACE}}),
        json!({"name":"New","extra":true}),
    ] {
        assert!(s.execute("file.newProject", p).is_err());
        assert_eq!(state(&s), before);
    }
    s.redo().unwrap();
    assert!(s.project.root.children.iter().any(|c| matches!(c,BinEntry::Bin(b) if b.name=="Later human")));
    s.revision = u64::MAX;
    let before = state(&s);
    assert!(s.execute("file.newProject", params()).is_err());
    assert_eq!(state(&s), before);
}
#[test]
fn project_workspace_first_human_sequence_preserves_organized_imports_notes_and_exact_settings() {
    let mut s = setup();
    s.execute("file.newBin", json!({"name":"Already organized"})).unwrap();
    s.execute("file.newSearchBin", json!({"name":"Saved query","text":"source"})).unwrap();
    let (demo, _) = crate::demo::demo_project(&s.media);
    let mut source = demo.items.values().find(|it| it.as_media().is_some()).unwrap().clone();
    source.id = ItemId(8000);
    s.edit("Generated source import", |p, _| {
        p.items.insert(source.id, source.clone());
        p.root.children.push(BinEntry::Item(source.id));
        p.notes = "Keep this human instruction".into();
        p.metadata.insert("human.custom".into(), "preserve".into());
        Ok(())
    })
    .unwrap();
    let before = s.project.clone();
    let history = s.history.undo.len();
    let id = create(&mut s);
    assert!(id.0 > 8000);
    let seq = s.project.sequence(id).unwrap();
    assert_eq!((seq.settings.width, seq.settings.height, seq.settings.sample_rate), (3840, 2160, 96000));
    assert_eq!(seq.settings.frame_rate, filmcraft_time::FrameRate::FPS_23_976);
    assert_eq!(seq.video_tracks.len(), 2);
    assert_eq!(seq.audio_tracks.len(), 4);
    assert!(seq.all_tracks().all(|t| t.items.is_empty()));
    let record = editorial_workspace::association(&s.project).unwrap().unwrap();
    assert_eq!(record.scope.anchor_sequence, id.0);
    assert_eq!(record.scope.project_id, "p_empty");
    assert_eq!(record.workspace_id, WORKSPACE);
    assert_eq!(s.project.item(id).unwrap().metadata[filmcraft_project::clip_identity::TRACKING_KEY], "1");
    assert_eq!(s.project.item(id).unwrap().metadata[filmcraft_project::clip_identity::EFFECT_TRACKING_KEY], "1");
    let mut stripped = (*s.project).clone();
    stripped.items.remove(&id);
    stripped.root.children.retain(|c| !matches!(c,BinEntry::Item(i) if *i==id));
    stripped.next_id = before.next_id;
    assert_eq!(stripped, *before);
    assert_eq!(s.history.undo.len(), history + 1);
}
#[test]
fn project_workspace_first_sequence_undo_returns_to_real_no_sequence_and_redo_keeps_identity() {
    let mut s = setup();
    s.execute("file.newBin", json!({"name":"Keep before any cut"})).unwrap();
    let before = s.project.clone();
    let id = create(&mut s);
    let after = s.project.clone();
    s.undo().unwrap();
    assert_eq!(s.project, before);
    assert!(s.state.active_sequence.is_none());
    assert!(project_workspace::identity(&s.project).unwrap().is_some());
    assert!(editorial_workspace::association(&s.project).unwrap().is_none());
    s.redo().unwrap();
    assert_eq!(s.project, after);
    assert_eq!(s.state.active_sequence, Some(id));
}
#[test]
fn project_workspace_saved_empty_and_organized_project_reopens_and_first_sequence_binds_once() {
    struct Directory(std::path::PathBuf);
    impl Drop for Directory {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    let path = std::env::temp_dir().join(format!(
        "fc-project-workspace-{}-{}",
        std::process::id(),
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
    ));
    fs::create_dir(&path).unwrap();
    let dir = Directory(path);
    let file = dir.0.join("empty.fcproj");
    let mut s = setup();
    s.execute("file.newBin", json!({"name":"Before save"})).unwrap();
    s.execute("file.saveAs", json!({"path":file})).unwrap();
    let bytes = fs::read(&file).unwrap();
    let mut reopened = Session::default();
    reopened.execute("file.open", json!({"path":file})).unwrap();
    assert!(reopened.state.active_sequence.is_none());
    assert_eq!(reopened.project, s.project);
    assert_eq!(fs::read(&file).unwrap(), bytes);
    let id = create(&mut reopened);
    let before = reopened.project.clone();
    reopened.execute("file.save", json!({})).unwrap();
    let mut next = Session::default();
    next.execute("file.open", json!({"path":file})).unwrap();
    assert_eq!(next.project, before);
    let other = next.execute("file.newSequence", json!({"name":"Second","fps":{"num":30000,"den":1001}})).unwrap()["sequence"].as_u64().unwrap();
    let scope = editorial_workspace::association(&next.project).unwrap().unwrap().scope;
    assert_eq!(scope.anchor_sequence, id.0);
    assert_eq!(next.project.item(ItemId(other)).unwrap().metadata[editorial_workspace::SEQUENCE_KEY], serde_json::to_string(&scope).unwrap());
    assert_eq!(next.project.items.values().filter(|it| it.metadata.contains_key(editorial_workspace::KEY)).count(), 1);
    assert_eq!(next.project.item(id), before.item(id));
}
#[test]
fn project_workspace_corrupt_or_foreign_root_and_anchor_cannot_create_a_new_sequence() {
    let mut original = setup();
    let id = create(&mut original);
    for variant in 0..7 {
        let mut s = Session { project: original.project.clone(), state: original.state.clone(), ..Session::default() };
        let project = Arc::make_mut(&mut s.project);
        match variant {
            0 => {
                project.metadata.insert(KEY.into(), "broken".into());
            }
            1 => {
                let mut v: Value = serde_json::from_str(&project.metadata[KEY]).unwrap();
                v["version"] = json!(2);
                project.metadata.insert(KEY.into(), v.to_string());
            }
            2 => {
                let mut v: Value = serde_json::from_str(&project.metadata[KEY]).unwrap();
                v["scope"]["projectId"] = json!("p_other");
                project.metadata.insert(KEY.into(), v.to_string());
            }
            3 => {
                project.metadata.insert(KEY.into(), "x".repeat(2049));
            }
            4 => {
                let raw = project.metadata[KEY].clone();
                project.item_mut(id).unwrap().metadata.insert(KEY.into(), raw);
            }
            5 => {
                project.item_mut(id).unwrap().metadata.remove(editorial_workspace::KEY);
            }
            _ => {
                let mut v: Value = serde_json::from_str(&project.metadata[KEY]).unwrap();
                v["extra"] = json!(true);
                project.metadata.insert(KEY.into(), v.to_string());
            }
        }
        let before = state(&s);
        assert!(s.execute("file.newSequence", json!({"name":"Refuse"})).is_err());
        assert_eq!(state(&s), before);
    }
}
#[test]
fn project_workspace_hostile_first_sequence_settings_and_exhausted_ids_do_not_publish_or_discard_redo() {
    let mut s = setup();
    s.execute("file.newBin", json!({"name":"Redo"})).unwrap();
    s.undo().unwrap();
    let before = state(&s);
    for p in [
        json!({"width":0}),
        json!({"height":40000}),
        json!({"sampleRate":0}),
        json!({"video":257}),
        json!({"audio":-1}),
        json!({"fps":null}),
        json!({"fps":"23.976"}),
        json!({"fps":{"num":24000,"den":0}}),
        json!({"fps":{"num":24000,"den":1001,"extra":true}}),
        json!({"fps":{"num":4294967296u64,"den":1}}),
        json!({"fps":1001}),
        json!({"name":null}),
        json!({"mix":false}),
        json!({"trackType":null}),
    ] {
        assert!(s.execute("file.newSequence", p).is_err());
        assert_eq!(state(&s), before);
    }
    s.redo().unwrap();
    for next in [0, 9_007_199_254_740_991, u64::MAX] {
        Arc::make_mut(&mut s.project).next_id = next;
        let before = state(&s);
        assert!(s.execute("file.newSequence", json!({"name":"Too many"})).is_err());
        assert_eq!(state(&s), before);
    }
}
#[test]
fn project_workspace_legacy_creation_and_sequence_defaults_do_not_opt_in_or_create_metadata() {
    let mut s = Session::default();
    s.execute("file.newProject", json!({})).unwrap();
    assert_eq!(s.project.name, "Untitled");
    assert!(s.project.metadata.is_empty());
    let serialized = serde_json::to_value(&s.project).unwrap();
    assert!(serialized.get("metadata").is_none());
    let id = ItemId(s.execute("file.newSequence", json!({"name":"Legacy"})).unwrap()["sequence"].as_u64().unwrap());
    assert!(s.project.item(id).unwrap().metadata.is_empty());
    assert!(project_workspace::identity(&s.project).unwrap().is_none());
}
#[test]
fn project_workspace_exact_human_rates_survive_initial_settings_and_later_settings_without_rounding() {
    let mut s = setup();
    let id = ItemId(s.execute("file.newSequence", json!({"fps":{"num":48001,"den":2000}})).unwrap()["sequence"].as_u64().unwrap());
    assert_eq!(s.project.sequence(id).unwrap().settings.frame_rate, filmcraft_time::FrameRate { num: 48001, den: 2000 });
    s.execute("sequence.settings", json!({"fps":{"num":120000,"den":1001}})).unwrap();
    assert_eq!(s.project.sequence(id).unwrap().settings.frame_rate, filmcraft_time::FrameRate::FPS_119_88);
    let before = state(&s);
    assert!(s.execute("sequence.settings", json!({"fps":"unknown"})).is_err());
    assert_eq!(state(&s), before);
}

#[test]
fn project_workspace_native_presentation_matches_real_identity_before_creation_after_closure_and_after_undo() {
    let mut s = setup();
    let before = state(&s);
    assert_eq!(s.matches_project_workspace("p_empty", WORKSPACE).unwrap(), Some(true));
    assert_eq!(s.matches_project_workspace("other", WORKSPACE).unwrap(), Some(false));
    assert_eq!(state(&s), before);
    let anchor = create(&mut s);
    s.execute("sequence.close", json!({"item":anchor.0})).unwrap();
    assert!(s.state.active_sequence.is_none());
    assert_eq!(s.matches_project_workspace("p_empty", WORKSPACE).unwrap(), Some(true));
    s.undo().unwrap();
    assert!(s.project.sequences().next().is_none());
    assert_eq!(s.matches_project_workspace("p_empty", WORKSPACE).unwrap(), Some(true));
    let mut legacy = Session::default();
    legacy.execute("file.newProject", json!({"name":"Legacy"})).unwrap();
    assert_eq!(legacy.matches_project_workspace("p_empty", WORKSPACE).unwrap(), None);
}
#[test]
fn project_workspace_native_presentation_refuses_malformed_duplicate_and_inconsistent_root_identity_without_mutation() {
    for mode in ["malformed", "duplicate", "foreign_anchor"] {
        let mut s = setup();
        let anchor = create(&mut s);
        match mode {
            "malformed" => {
                Arc::make_mut(&mut s.project).metadata.insert(KEY.into(), "{".into());
            }
            "duplicate" => {
                let raw = s.project.metadata[KEY].clone();
                Arc::make_mut(&mut s.project).item_mut(anchor).unwrap().metadata.insert(KEY.into(), raw);
            }
            "foreign_anchor" => {
                let p = Arc::make_mut(&mut s.project);
                let item = p.item_mut(anchor).unwrap();
                let mut association: Value = serde_json::from_str(&item.metadata[editorial_workspace::KEY]).unwrap();
                association["workspaceId"] = json!("11111111-1111-4111-8111-111111111111");
                item.metadata.insert(editorial_workspace::KEY.into(), association.to_string());
            }
            _ => unreachable!(),
        }
        let before = state(&s);
        assert!(s.matches_project_workspace("p_empty", WORKSPACE).is_err(), "{mode}");
        assert_eq!(state(&s), before);
    }
    let s = setup();
    for (p, w) in [("../foreign", WORKSPACE), ("p_empty", "bad")] {
        assert!(s.matches_project_workspace(p, w).is_err());
    }
}
