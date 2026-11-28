//! Native project lifecycle primitives; host parity supplies the independent legacy reference.
use crate::Session;
use filmcraft_project::{ItemId, MonoSourceLaw, ProgramExtentPolicy, StereoBalanceLaw};
use filmcraft_time::FrameRate;
use serde_json::{Value, json};
use std::sync::Arc;

fn session() -> Session {
    let mut s = Session::default();
    let (project, sequence) = crate::demo::demo_project(&s.media);
    s.project = Arc::new(project);
    s.state.active_sequence = Some(sequence);
    s.state.open_sequences = vec![sequence];
    s
}
fn empty() -> Value {
    json!({"name":"Created sequence","frame":{"width":96,"height":64,"rate":{"num":24000,"den":1001}},"activate":false})
}
#[test]
fn native_sequence_create_preserves_foreground_and_is_one_project_undo() {
    let mut s = session();
    let before = s.project.clone();
    let active = s.state.active_sequence;
    let tabs = s.state.open_sequences.clone();
    let editor = serde_json::to_value(&s.state).unwrap();
    let depth = s.history.undo.len();
    let (_, events, _lease) = s.command_journal.subscribe().unwrap();
    let result = s.execute("poema.createSequence", empty()).unwrap();
    let id = ItemId(result["sequence"].as_u64().unwrap());
    assert_eq!(s.project.items.len(), before.items.len() + 1);
    assert_eq!(s.state.active_sequence, active);
    assert_eq!(s.state.open_sequences, tabs);
    assert_eq!(serde_json::to_value(&s.state).unwrap(), editor);
    assert_eq!(s.history.undo.len(), depth + 1);
    let q = s.project.sequence(id).unwrap();
    assert_eq!((q.settings.width, q.settings.height, q.settings.frame_rate), (96, 64, FrameRate::FPS_23_976));
    assert_eq!((q.video_tracks.len(), q.audio_tracks.len(), q.caption_tracks.len()), (1, 1, 1));
    assert_eq!(q.program_extent, Some(ProgramExtentPolicy::Poema));
    assert_eq!(q.audio_tracks[0].mixer.stereo_balance, StereoBalanceLaw::Linear);
    assert_eq!(q.audio_tracks[0].mixer.mono_source, MonoSourceLaw::EqualPower);
    assert_eq!(events.try_recv().unwrap()["command"], "poema.createSequence");
    assert!(events.try_recv().is_err());
    let after = s.project.clone();
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(s.project, before);
    s.execute("edit.redo", json!({})).unwrap();
    assert_eq!(s.project, after);
}
#[test]
fn native_sequence_clone_copies_the_complete_current_item_without_touching_its_source() {
    let mut s = session();
    let active = s.state.active_sequence.unwrap();
    Arc::make_mut(&mut s.project).item_mut(active).unwrap().metadata.insert("fixture".into(), "must survive".into());
    {
        let q = Arc::make_mut(&mut s.project).sequence_mut(active).unwrap();
        let clip = &mut q.video_tracks[0].items[0];
        clip.identity = Some(Box::new(filmcraft_project::ClipIdentity {
            id: "c_original".into(),
            parent: None,
            annotations: Default::default(),
            protection: Some("human".into()),
            handoff_spans: vec![filmcraft_project::HandoffSpan {
                carrier: "poema:s_clone poema:b_clone".into(),
                source_in: clip.source_in,
                source_out: clip.source_in + clip.duration,
            }],
        }));
    }
    let source = s.project.item(active).unwrap().clone();
    let before = s.project.clone();
    let depth = s.history.undo.len();
    let result = s.execute("poema.createSequence", json!({"name":"Working copy","from":active.0,"activate":true})).unwrap();
    let id = ItemId(result["sequence"].as_u64().unwrap());
    assert_ne!(id, active);
    assert_eq!(s.project.item(active), Some(&source));
    let new = s.project.item(id).unwrap();
    assert_eq!(new.name, "Working copy");
    assert_eq!(new.kind, source.kind);
    assert_eq!(new.metadata, source.metadata);
    assert_eq!(s.state.active_sequence, Some(id));
    assert!(s.state.open_sequences.contains(&id));
    assert_eq!(s.history.undo.len(), depth + 1);
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(s.project, before);
    assert!(s.state.active_sequence.is_some_and(|id| s.project.sequence(id).is_some()));
}
#[test]
fn native_sequence_creation_hostile_arguments_and_allocator_overflow_are_atomic() {
    let mut s = session();
    for params in [
        json!({}),
        json!({"name":3}),
        json!({"name":"x","from":0}),
        json!({"name":"x","from":-1}),
        json!({"name":"x","from":999999}),
        json!({"name":"x","from":s.state.active_sequence.unwrap().0,"frame":{"width":96,"height":64,"rate":{"num":24,"den":1}}}),
        json!({"name":"x","frame":{"width":0,"height":64,"rate":{"num":24,"den":1}}}),
        json!({"name":"x","frame":{"width":96,"height":64,"rate":{"num":24,"den":0}}}),
        json!({"name":"x","frame":{"width":4294967295u32,"height":4294967295u32,"rate":{"num":24,"den":1}}}),
        json!({"name":"x","paths":["must-not-read.mov"]}),
        json!({"name":"x".repeat(1025),"from":s.state.active_sequence.unwrap().0}),
    ] {
        let before = s.project.clone();
        let depth = s.history.undo.len();
        assert!(s.execute("poema.createSequence", params).is_err());
        assert_eq!(s.project, before);
        assert_eq!(s.history.undo.len(), depth);
    }
    Arc::make_mut(&mut s.project).next_id = u64::MAX - 2;
    let before = s.project.clone();
    assert!(std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| s.execute("poema.createSequence", empty()))).unwrap().is_err());
    assert_eq!(s.project, before);
}

#[test]
fn inactive_metadata_targets_do_not_switch_the_editor_and_scoped_creation_is_forbidden() {
    let mut s = session();
    let active = s.state.active_sequence;
    let result = s.execute("poema.createSequence", empty()).unwrap();
    let other = result["sequence"].as_u64().unwrap();
    let meta = json!({"schema":1,"project_id":"p_fixture","sequence_id":"q_new","root":{"version":0},"settings":{"width":96,"height":64,"fps":23.976},"clock":{"num":24000,"den":1001},"fields":["version","settings","tracks"],"tracks":{},"assets":{},"links":{}});
    s.execute("poema.setSequenceMetadata", json!({"sequence":other,"metadata":meta})).unwrap();
    assert_eq!(s.state.active_sequence, active);
    assert_eq!(serde_json::from_str::<Value>(&s.project.item(ItemId(other)).unwrap().metadata["poema.editorial.v1"]).unwrap()["sequence_id"], "q_new");
    let basis = s.execute("agent.snapshot", json!({})).unwrap();
    let before = s.project.clone();
    let depth = s.history.undo.len();
    assert!(
        s.execute(
            "agent.previewSequence",
            json!({"token":basis["token"],"sequence":active.unwrap().0,"operations":[{"command":"poema.createSequence","params":empty()}]})
        )
        .is_err()
    );
    assert_eq!(s.project, before);
    assert_eq!(s.history.undo.len(), depth);
    for target in [0, 999999] {
        assert!(s.execute("poema.setSequenceMetadata", json!({"sequence":target,"metadata":meta})).is_err());
        assert_eq!(s.project, before);
    }
}

#[test]
fn sequence_record_is_native_editorial_state_with_no_geometry_or_held_version_shadow() {
    let mut s = session();
    let created = s.execute("poema.createSequence", empty()).unwrap();
    let other = created["sequence"].as_u64().unwrap();
    let meta = json!({"schema":1,"project_id":"p_fixture","sequence_id":"q_new","root":{"version":0},"settings":{"width":96,"height":64,"fps":23.976},"clock":{"num":24000,"den":1001},"fields":["version","settings","tracks"],"tracks":{},"assets":{},"links":{}});
    s.execute("poema.setSequenceMetadata", json!({"sequence":other,"metadata":meta})).unwrap();
    let p = json!({"sequence":other,"projectId":"p_fixture","sequenceId":"q_new","props":{"created":"2026-11-28T10:00:00.000Z","bin":"b_reels"},"name":"Named copy"});
    let before = s.project.clone();
    let depth = s.history.undo.len();
    s.execute("poema.setSequenceRecord", p.clone()).unwrap();
    assert_eq!(s.history.undo.len(), depth + 1);
    assert_eq!(s.project.item(ItemId(other)).unwrap().name, "Named copy");
    let after = s.project.clone();
    assert_eq!(s.execute("poema.setSequenceRecord", p.clone()).unwrap()["changed"], false);
    assert_eq!(s.project, after);
    assert_eq!(s.history.undo.len(), depth + 1);
    for key in ["id", "name", "currentVersion", "timeline", "tracks", "clips", "settings", "duration", "versions", "held_finish"] {
        let mut wrong = p.clone();
        wrong["props"][key] = json!({"must":"refuse"});
        assert!(s.execute("poema.setSequenceRecord", wrong).is_err());
        assert_eq!(s.project, after);
    }
    let mut foreign = p.clone();
    foreign["projectId"] = json!("p_foreign");
    assert!(s.execute("poema.setSequenceRecord", foreign).is_err());
    assert_eq!(s.project, after);
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(s.project, before);
}

#[test]
fn sequence_delete_is_one_project_undo_with_explicit_active_fallback() {
    let mut s = session();
    let active = s.state.active_sequence.unwrap();
    let made = s.execute("poema.createSequence", empty()).unwrap();
    let other = made["sequence"].as_u64().unwrap();
    s.execute("sequence.open", json!({"item":other})).unwrap();
    let before = s.project.clone();
    let depth = s.history.undo.len();
    for p in [json!({"sequence":other}), json!({"sequence":other,"fallback":other}), json!({"sequence":other,"fallback":999999})] {
        assert!(s.execute("poema.deleteSequence", p).is_err());
        assert_eq!(s.project, before);
    }
    s.execute("poema.deleteSequence", json!({"sequence":other,"fallback":active.0})).unwrap();
    assert!(s.project.sequence(ItemId(other)).is_none());
    assert_eq!(s.state.active_sequence, Some(active));
    assert_eq!(s.history.undo.len(), depth + 1);
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(s.project, before);
    assert_eq!(s.state.active_sequence, Some(active));
    let mut deep = json!(null);
    for _ in 0..80 {
        deep = json!([deep]);
    }
    let held = s.project.clone();
    assert!(s.execute("poema.createSequence", json!({"name":deep})).is_err());
    assert_eq!(s.project, held);
}

#[test]
fn native_sequence_display_name_and_document_title_are_distinct_checked_undoable_values() {
    let mut s = session();
    let made = s.execute("poema.createSequence", empty()).unwrap();
    let other = made["sequence"].as_u64().unwrap();
    let meta = json!({"schema":1,"project_id":"p_fixture","sequence_id":"q_new","root":{"version":0,"name":"Created sequence"},"settings":{"width":96,"height":64,"fps":23.976},"clock":{"num":24000,"den":1001},"fields":["version","name","settings","tracks"],"tracks":{},"assets":{},"links":{}});
    s.execute("poema.setSequenceMetadata", json!({"sequence":other,"metadata":meta})).unwrap();
    let before = s.project.clone();
    let depth = s.history.undo.len();
    let mut params = json!({"sequence":other,"projectId":"p_fixture","sequenceId":"q_new","props":{"bin":"b_reels"},"name":"Sequence display label"});
    assert!(s.execute("poema.setSequenceRecord", params.clone()).is_err());
    assert_eq!(s.project, before);
    assert_eq!(s.history.undo.len(), depth);
    params["documentTitle"] = json!("Created sequence");
    s.execute("poema.setSequenceRecord", params.clone()).unwrap();
    let after = s.project.clone();
    let item = s.project.item(ItemId(other)).unwrap();
    assert_eq!(item.name, "Sequence display label");
    assert_eq!(serde_json::from_str::<Value>(&item.metadata[crate::poema_sequences::RECORD_KEY]).unwrap()["document_title"], "Created sequence");
    assert_eq!(s.history.undo.len(), depth + 1);
    assert_eq!(s.execute("poema.setSequenceRecord", params.clone()).unwrap()["changed"], false);
    assert_eq!(s.project, after);
    assert_eq!(s.history.undo.len(), depth + 1);
    for wrong in [json!(5), json!("x".repeat(1025)), json!(null)] {
        let mut invalid = params.clone();
        invalid["documentTitle"] = wrong;
        assert!(s.execute("poema.setSequenceRecord", invalid).is_err());
        assert_eq!(s.project, after);
    }
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(s.project, before);
}
