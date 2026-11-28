//! Explicit program duration is native sequence state and part of native Undo.
use crate::{Result, Session, commands::bad};
use filmcraft_project::ProgramExtentPolicy;
use serde::Deserialize;
use serde_json::{Value, json};

const CMD: &str = "poema.setExtentPolicy";
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
enum Policy {
    Native,
    Poema,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    policy: Policy,
}

pub(crate) fn set(session: &mut Session, params: &Value) -> Result<Value> {
    crate::project_owner::writable(session)?;
    let request: Request = serde_json::from_value(params.clone()).map_err(|error| bad(CMD, error.to_string()))?;
    let policy = match request.policy {
        Policy::Native => None,
        Policy::Poema => Some(ProgramExtentPolicy::Poema),
    };
    let sequence = session.active_sequence().ok_or(crate::EngineError::NoSequence)?;
    if sequence.program_extent == policy {
        return Ok(json!({"changed":false,"duration":sequence.duration().0.to_string()}));
    }
    session.edit_sequence("Set program extent policy", |sequence, _, _| {
        sequence.program_extent = policy;
        sequence.check().map_err(crate::EngineError::Other)?;
        Ok(())
    })?;
    let sequence = session.active_sequence().ok_or(crate::EngineError::NoSequence)?;
    Ok(json!({"changed":true,"duration":sequence.duration().0.to_string()}))
}

#[cfg(test)]
mod tests {
    use super::*;
    use filmcraft_project::{ClipId, ItemId};
    use filmcraft_time::{TICKS_PER_SECOND, Tick};
    use std::sync::Arc;
    fn demo() -> (Session, ItemId, Tick) {
        let mut session = Session::default();
        let (mut project, id) = crate::demo::demo_project(&session.media);
        let tail_id = ClipId(project.alloc_id());
        let sequence = project.sequence_mut(id).unwrap();
        let prior_end = sequence.duration();
        let mut tail = sequence.video_tracks[0].items[0].clone();
        tail.id = tail_id;
        tail.identity = None;
        tail.link = None;
        tail.group = None;
        tail.effects.clear();
        tail.start = prior_end + Tick(TICKS_PER_SECOND);
        tail.duration = Tick(TICKS_PER_SECOND);
        sequence.audio_tracks[0].items = vec![tail];
        let expected = sequence.video_tracks[0].items.iter().map(|clip| clip.start + clip.duration).max().unwrap();
        sequence.check().unwrap();
        session.project = Arc::new(project);
        session.state.active_sequence = Some(id);
        (session, id, expected)
    }
    #[test]
    fn private_policy_matches_independent_project_and_owned_undo_redo() {
        let (mut session, id, expected_duration) = demo();
        let before = serde_json::to_value(&session.project).unwrap();
        let state = serde_json::to_value((&session.state, &session.state.clipboard, &session.prefs)).unwrap();
        let depth = session.history.undo.len();
        let native_duration = session.active_sequence().unwrap().duration();
        assert!(native_duration > expected_duration);
        let basis = session.execute("agent.snapshot", json!({})).unwrap();
        let (_, events, _subscription) = session.command_journal.subscribe().unwrap();
        let preview = session.execute("agent.preview", json!({"token":basis["token"],"operations":[{"command":CMD,"params":{"policy":"poema"}}]})).unwrap();
        assert_eq!(serde_json::to_value(&session.project).unwrap(), before);
        assert_eq!(serde_json::to_value((&session.state, &session.state.clipboard, &session.prefs)).unwrap(), state);
        assert!(events.try_recv().is_err());
        let mut expected = (*session.project).clone();
        expected.sequence_mut(id).unwrap().program_extent = Some(ProgramExtentPolicy::Poema);
        // An independent watcher read must not consume the pending preview.
        session.execute("agent.snapshot", json!({})).unwrap();
        let committed = session.execute("agent.commitPreview", json!({"token":basis["token"],"preview":preview["preview"]})).unwrap();
        assert_eq!(serde_json::to_value(&session.project).unwrap(), serde_json::to_value(&expected).unwrap());
        assert_eq!(session.active_sequence().unwrap().duration(), expected_duration);
        assert_eq!(session.history.undo.len(), depth + 1);
        assert_eq!(events.try_recv().unwrap()["command"], "agent.commitPreview");
        assert!(events.try_recv().is_err());
        let revision = session.revision;
        session.execute(CMD, json!({"policy":"poema"})).unwrap();
        assert_eq!(session.revision, revision);
        assert!(events.try_recv().is_err());
        let ticket = session.execute("agent.snapshot", json!({})).unwrap();
        session.execute("agent.undo", json!({"token":ticket["token"],"commit":committed["commit"]})).unwrap();
        assert_eq!(serde_json::to_value(&session.project).unwrap(), before);
        assert_eq!(events.try_recv().unwrap()["command"], "agent.undo");
        let ticket = session.execute("agent.snapshot", json!({})).unwrap();
        session.execute("agent.redo", json!({"token":ticket["token"],"commit":committed["commit"]})).unwrap();
        assert_eq!(serde_json::to_value(&session.project).unwrap(), serde_json::to_value(&expected).unwrap());
        assert_eq!(events.try_recv().unwrap()["command"], "agent.redo");
        session.execute(CMD, json!({"policy":"native"})).unwrap();
        assert_eq!(events.try_recv().unwrap()["command"], CMD);
        assert!(events.try_recv().is_err());
        assert_eq!(session.active_sequence().unwrap().duration(), native_duration);
        assert!(session.active_sequence().unwrap().program_extent.is_none());
        let encoded = serde_json::to_value(&session.project).unwrap();
        let restored: filmcraft_project::Project = serde_json::from_value(encoded.clone()).unwrap();
        assert_eq!(serde_json::to_value(restored).unwrap(), encoded);
    }
    #[test]
    fn hostile_policy_and_valid_prefix_refuse_atomically() {
        let (mut session, _, _) = demo();
        for params in [
            Value::Null,
            json!({}),
            json!({"policy":null}),
            json!({"policy":false}),
            json!({"policy":1}),
            json!({"policy":"other"}),
            json!({"policy":["poema"]}),
            json!({"policy":"poema","duration":1}),
        ] {
            let before = session.project.clone();
            let revision = session.revision;
            assert!(session.execute(CMD, params.clone()).is_err());
            assert!(Arc::ptr_eq(&before, &session.project));
            assert_eq!(session.revision, revision);
            let ticket = session.execute("agent.snapshot", json!({})).unwrap();
            assert!(
                session
                    .execute(
                        "agent.apply",
                        json!({"token":ticket["token"],"operations":[{"command":CMD,"params":{"policy":"poema"}},{"command":CMD,"params":params}]})
                    )
                    .is_err()
            );
            assert!(Arc::ptr_eq(&before, &session.project));
            assert_eq!(session.revision, revision);
        }
        let mut empty = Session::default();
        assert!(empty.execute(CMD, json!({"policy":"poema"})).is_err());
    }
    #[test]
    fn scoped_policy_preserves_foreground_and_discard_stale_plans() {
        let (mut session, foreground, _) = demo();
        let other = Arc::make_mut(&mut session.project).new_sequence("other", Default::default(), 1, 1, None);
        let before = serde_json::to_value(&session.project).unwrap();
        let state = serde_json::to_value((&session.state, &session.state.clipboard, &session.prefs)).unwrap();
        let basis = session.execute("agent.snapshot", json!({})).unwrap();
        let request = json!({"token":basis["token"],"sequence":other.0,"operations":[{"command":CMD,"params":{"policy":"poema"}}]});
        let preview = session.execute("agent.previewSequence", request.clone()).unwrap();
        assert_eq!(serde_json::to_value(&session.project).unwrap(), before);
        assert_eq!(serde_json::to_value((&session.state, &session.state.clipboard, &session.prefs)).unwrap(), state);
        session.execute("agent.discardPreview", json!({"preview":preview["preview"]})).unwrap();
        let preview = session.execute("agent.previewSequence", request).unwrap();
        let committed = session.execute("agent.commitPreview", json!({"token":basis["token"],"preview":preview["preview"]})).unwrap();
        assert_eq!(session.state.active_sequence, Some(foreground));
        assert!(session.project.sequence(foreground).unwrap().program_extent.is_none());
        assert_eq!(session.project.sequence(other).unwrap().program_extent, Some(ProgramExtentPolicy::Poema));
        let ticket = session.execute("agent.snapshot", json!({})).unwrap();
        session.execute("agent.undo", json!({"token":ticket["token"],"commit":committed["commit"]})).unwrap();
        assert_eq!(serde_json::to_value(&session.project).unwrap(), before);
        let ticket = session.execute("agent.snapshot", json!({})).unwrap();
        let preview = session.execute("agent.preview", json!({"token":ticket["token"],"operations":[{"command":CMD,"params":{"policy":"poema"}}]})).unwrap();
        session.execute(CMD, json!({"policy":"poema"})).unwrap();
        assert!(session.execute("agent.commitPreview", json!({"token":ticket["token"],"preview":preview["preview"]})).is_err());
        assert_eq!(session.project.sequence(foreground).unwrap().program_extent, Some(ProgramExtentPolicy::Poema));
    }
    #[test]
    fn native_harness_split_clear_and_human_undo_drive_program_extent() {
        let (mut session, _, spine_end) = demo();
        let audio = session.active_sequence().unwrap().audio_tracks[0].clone();
        let tail = audio.items[0].clone();
        session.execute("poema.setCarrier", json!({"clip":tail.id.0,"identity":{"id":"extent-anchor","protection":"harness","handoffSpans":[]}})).unwrap();
        session.execute(CMD, json!({"policy":"poema"})).unwrap();
        let before = serde_json::to_value(&session.project).unwrap();
        assert_eq!(session.active_sequence().unwrap().duration(), tail.start + tail.duration);
        let cut = tail.start + Tick(tail.duration.0 / 2);
        session.execute("timeline.razor", json!({"clip":tail.id.0,"track":audio.id.0,"time":cut.0})).unwrap();
        let items = &session.active_sequence().unwrap().audio_tracks[0].items;
        assert_eq!(items.len(), 2);
        assert!(items.iter().all(|clip| clip.identity.as_ref().unwrap().protection.as_deref() == Some("harness")));
        let right = items[1].id;
        assert_eq!(session.active_sequence().unwrap().duration(), tail.start + tail.duration);
        session.execute("edit.clear", json!({"clips":[right.0]})).unwrap();
        assert_eq!(session.active_sequence().unwrap().duration(), cut.max(spine_end));
        session.execute("edit.undo", json!({})).unwrap();
        assert_eq!(session.active_sequence().unwrap().duration(), tail.start + tail.duration);
        session.execute("edit.undo", json!({})).unwrap();
        assert_eq!(serde_json::to_value(&session.project).unwrap(), before);
    }
}
