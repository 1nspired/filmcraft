//! Native human trim protection is part of the same checked sequence edit.
//! Agent-staged commands retain their explicit author policy.
use filmcraft_project::{ClipId, ProgramExtentPolicy, Sequence};
use filmcraft_time::Tick;

pub(crate) struct Basis {
    id: ClipId,
    geometry: (Tick, Tick, Tick, Tick),
}

pub(crate) fn capture(sequence: &Sequence, clips: &[ClipId], human: bool) -> Vec<Basis> {
    if !human || sequence.program_extent != Some(ProgramExtentPolicy::Poema) {
        return Vec::new();
    }
    clips
        .iter()
        .filter_map(|id| {
            let (_, clip) = sequence.find_item(*id)?;
            clip.identity.as_ref()?;
            Some(Basis { id: *id, geometry: (clip.start, clip.duration, clip.source_in, clip.source_end_offset) })
        })
        .collect()
}

pub(crate) fn protect(sequence: &mut Sequence, before: Vec<Basis>) {
    for old in before {
        if let Some((_, clip)) = sequence.find_item_mut(old.id) {
            let after = (clip.start, clip.duration, clip.source_in, clip.source_end_offset);
            if after != old.geometry
                && let Some(identity) = clip.identity.as_mut()
            {
                identity.protection = Some("human".into());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::Session;
    use filmcraft_project::{ClipId, ClipIdentity, HandoffSpan, ItemId, ProgramExtentPolicy, Project};
    use filmcraft_time::{TICKS_PER_SECOND, Tick};
    use serde_json::json;
    use std::{collections::BTreeMap, sync::Arc};
    fn fixture() -> (Session, ItemId, ClipId, ClipId, ClipId, Tick) {
        let mut session = Session::default();
        let (mut project, id) = crate::demo::demo_project(&session.media);
        let q = project.sequence_mut(id).unwrap();
        q.program_extent = Some(ProgramExtentPolicy::Poema);
        q.video_tracks.truncate(1);
        q.video_tracks[0].items.truncate(2);
        q.audio_tracks.truncate(1);
        q.audio_tracks[0].items.truncate(1);
        q.audio_tracks[0].items[0].start = Tick(30 * TICKS_PER_SECOND);
        q.audio_tracks[0].sync_lock = false;
        q.markers.clear();
        for track in q.all_tracks_mut() {
            track.transitions.clear();
            for clip in &mut track.items {
                clip.link = None;
                clip.group = None;
                clip.effects.clear();
                clip.markers.clear();
                clip.identity = Some(Box::new(ClipIdentity {
                    id: format!("fixture_{}", clip.id.0),
                    parent: Some("original_parent".into()),
                    protection: None,
                    annotations: BTreeMap::from([("editorial".into(), "retain all fields".into())]),
                    handoff_spans: vec![HandoffSpan {
                        carrier: format!("poema:sel_{} poema:bite_{}", clip.id.0, clip.id.0),
                        source_in: clip.source_in,
                        source_out: clip.source_out(),
                    }],
                }));
            }
        }
        let clip = q.video_tracks[0].items[0].id;
        let next = q.video_tracks[0].items[1].id;
        let audio = q.audio_tracks[0].items[0].id;
        let frame = q.settings.frame_rate.frame_duration();
        q.check().unwrap();
        session.project = Arc::new(project);
        session.state.active_sequence = Some(id);
        (session, id, clip, next, audio, frame)
    }
    fn shorter(project: &Project, id: ItemId, clip: ClipId, frame: Tick, human: bool) -> Project {
        let mut expected = project.clone();
        let c = expected.sequence_mut(id).unwrap().find_item_mut(clip).unwrap().1;
        c.duration -= frame;
        if human {
            c.identity.as_mut().unwrap().protection = Some("human".into());
        }
        expected
    }
    fn trim(session: &mut Session, clip: ClipId) {
        session.execute("timeline.trim", json!({"clip":clip.0,"edge":"out","deltaFrames":-1})).unwrap();
    }
    #[test]
    fn raw_human_trim_is_one_independent_project_edit_and_human_undo_redo() {
        let (mut session, id, clip, _, _, frame) = fixture();
        let before = (*session.project).clone();
        let expected = shorter(&before, id, clip, frame, true);
        let depth = session.history.undo.len();
        let (_, events, _subscription) = session.command_journal.subscribe().unwrap();
        trim(&mut session, clip);
        assert_eq!(*session.project, expected);
        assert_eq!(session.history.undo.len(), depth + 1);
        assert_eq!(events.try_recv().unwrap()["command"], "timeline.trim");
        assert!(events.try_recv().is_err());
        session.execute("edit.undo", json!({})).unwrap();
        assert_eq!(*session.project, before);
        assert_eq!(events.try_recv().unwrap()["command"], "edit.undo");
        session.execute("edit.redo", json!({})).unwrap();
        assert_eq!(*session.project, expected);
        assert_eq!(events.try_recv().unwrap()["command"], "edit.redo");
        assert!(events.try_recv().is_err());
    }
    #[test]
    fn staged_agent_preview_commit_and_owned_history_do_not_invent_human_protection() {
        let (mut session, id, clip, _, _, frame) = fixture();
        let before = (*session.project).clone();
        let expected = shorter(&before, id, clip, frame, false);
        let basis = session.execute("agent.snapshot", json!({})).unwrap();
        let (_, events, _subscription) = session.command_journal.subscribe().unwrap();
        let preview = session
            .execute(
                "agent.preview",
                json!({"token":basis["token"],"operations":[{"command":"timeline.trim","params":{"clip":clip.0,"edge":"out","deltaFrames":-1}}]}),
            )
            .unwrap();
        assert_eq!(*session.project, before);
        assert!(events.try_recv().is_err());
        session.execute("agent.snapshot", json!({})).unwrap();
        let committed = session.execute("agent.commitPreview", json!({"token":basis["token"],"preview":preview["preview"]})).unwrap();
        assert_eq!(*session.project, expected);
        assert_eq!(events.try_recv().unwrap()["command"], "agent.commitPreview");
        let basis = session.execute("agent.snapshot", json!({})).unwrap();
        session.execute("agent.undo", json!({"token":basis["token"],"commit":committed["commit"]})).unwrap();
        assert_eq!(*session.project, before);
        assert_eq!(events.try_recv().unwrap()["command"], "agent.undo");
        let basis = session.execute("agent.snapshot", json!({})).unwrap();
        session.execute("agent.redo", json!({"token":basis["token"],"commit":committed["commit"]})).unwrap();
        assert_eq!(*session.project, expected);
        assert_eq!(events.try_recv().unwrap()["command"], "agent.redo");
        assert!(events.try_recv().is_err());
    }
    #[test]
    fn actual_human_trim_invalidates_a_pending_unprotected_agent_preview() {
        let (mut session, id, clip, _, _, frame) = fixture();
        let before = (*session.project).clone();
        let expected = shorter(&before, id, clip, frame, true);
        let basis = session.execute("agent.snapshot", json!({})).unwrap();
        let preview = session
            .execute(
                "agent.preview",
                json!({"token":basis["token"],"operations":[{"command":"timeline.trim","params":{"clip":clip.0,"edge":"out","deltaFrames":-1}}]}),
            )
            .unwrap();
        trim(&mut session, clip);
        assert_eq!(*session.project, expected);
        let depth = session.history.undo.len();
        assert!(session.execute("agent.commitPreview", json!({"token":basis["token"],"preview":preview["preview"]})).is_err());
        assert_eq!(*session.project, expected);
        assert_eq!(session.history.undo.len(), depth);
    }
    #[test]
    fn ordinary_policy_and_identity_free_native_clips_keep_prior_behavior() {
        for missing_identity in [false, true] {
            let (mut session, id, clip, _, _, frame) = fixture();
            let mut project = (*session.project).clone();
            let q = project.sequence_mut(id).unwrap();
            if missing_identity {
                q.find_item_mut(clip).unwrap().1.identity = None;
            } else {
                q.program_extent = None;
            }
            session.project = Arc::new(project);
            let before = (*session.project).clone();
            let expected = shorter(&before, id, clip, frame, false);
            trim(&mut session, clip);
            assert_eq!(*session.project, expected);
        }
    }
    #[test]
    fn only_changed_linked_partners_are_protected_and_locked_audio_stays_parked() {
        for locked in [false, true] {
            let (mut session, id, clip, _, audio, frame) = fixture();
            let mut project = (*session.project).clone();
            let q = project.sequence_mut(id).unwrap();
            q.find_item_mut(clip).unwrap().1.link = Some(7001);
            q.find_item_mut(audio).unwrap().1.link = Some(7001);
            q.audio_tracks[0].locked = locked;
            q.check().unwrap();
            session.project = Arc::new(project);
            let before = (*session.project).clone();
            let mut expected = shorter(&before, id, clip, frame, true);
            if !locked {
                expected = shorter(&expected, id, audio, frame, true);
            }
            trim(&mut session, clip);
            assert_eq!(*session.project, expected);
            session.execute("edit.undo", json!({})).unwrap();
            assert_eq!(*session.project, before);
        }
    }
    #[test]
    fn trimming_a_harness_as_human_updates_its_protection_and_native_program_in_same_undo() {
        let (mut session, id, _, _, audio, frame) = fixture();
        let mut project = (*session.project).clone();
        project.sequence_mut(id).unwrap().find_item_mut(audio).unwrap().1.identity.as_mut().unwrap().protection = Some("harness".into());
        session.project = Arc::new(project);
        let before = (*session.project).clone();
        let expected = shorter(&before, id, audio, frame, true);
        assert!(before.sequence(id).unwrap().duration() > expected.sequence(id).unwrap().duration());
        trim(&mut session, audio);
        assert_eq!(*session.project, expected);
        session.execute("edit.undo", json!({})).unwrap();
        assert_eq!(*session.project, before);
    }
    #[test]
    fn locked_invalid_and_unchanged_geometry_never_add_protection() {
        let (mut session, id, clip, _, _, _) = fixture();
        let before = (*session.project).clone();
        for params in [json!({}), json!({"clip":null}), json!({"clip":999999999u64,"edge":"out","deltaFrames":-1})] {
            assert!(session.execute("timeline.trim", params).is_err());
            assert_eq!(*session.project, before);
        }
        session.execute("timeline.trim", json!({"clip":clip.0,"edge":"out","deltaFrames":0})).unwrap();
        assert_eq!(*session.project, before);
        let mut project = (*session.project).clone();
        project.sequence_mut(id).unwrap().video_tracks[0].locked = true;
        session.project = Arc::new(project);
        let before = (*session.project).clone();
        assert!(session.execute("timeline.trim", json!({"clip":clip.0,"edge":"out","deltaFrames":-1})).is_err());
        assert_eq!(*session.project, before);
    }
    #[test]
    fn ripple_moves_following_picture_without_claiming_it_was_human_trimmed() {
        let (mut session, id, clip, next, _, frame) = fixture();
        let before = (*session.project).clone();
        let mut expected = shorter(&before, id, clip, frame, true);
        expected.sequence_mut(id).unwrap().find_item_mut(next).unwrap().1.start -= frame;
        session.execute("timeline.trim", json!({"clip":clip.0,"edge":"out","mode":"ripple","deltaFrames":-1})).unwrap();
        assert_eq!(*session.project, expected);
        session.execute("edit.undo", json!({})).unwrap();
        assert_eq!(*session.project, before);
    }
}
