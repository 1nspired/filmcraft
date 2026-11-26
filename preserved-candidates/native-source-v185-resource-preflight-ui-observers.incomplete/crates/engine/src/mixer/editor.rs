//! Original session-only mixer/default settings. Human command parsing,
//! journaling and live gesture ownership remain in the original command path.
//! This service grants no ticket, history step, persistence or model authority.
use crate::{EditorState, Result, Session, commands::bad};
use filmcraft_project::{AutomationMode, EffectKind, ItemId, Project, TrackId, find_effect};
use serde_json::{Value, json};

enum Edit {
    Mode { sequence: ItemId, track: TrackId, mode: AutomationMode },
    DefaultTransition { audio: bool, definition: &'static str },
}
pub(crate) struct Change {
    edit: Edit,
}
fn audio_track(project: &Project, state: &EditorState, track: TrackId) -> Result<ItemId> {
    let sequence = state.active_sequence.ok_or(crate::EngineError::NoSequence)?;
    if !project.sequence(sequence).ok_or(crate::EngineError::NoSequence)?.audio_tracks.iter().any(|t| t.id == track) {
        return Err(bad("clipMixer.setMode", "the Audio Clip Mixer has audio tracks only"));
    }
    Ok(sequence)
}
pub(crate) fn prepare_mode(s: &Session, track: TrackId, mode: AutomationMode) -> Result<Change> {
    prepare_mode_for(&s.project, &s.state, track, mode)
}
pub(crate) fn prepare_mode_for(project: &Project, state: &EditorState, track: TrackId, mode: AutomationMode) -> Result<Change> {
    Ok(Change { edit: Edit::Mode { sequence: audio_track(project, state, track)?, track, mode } })
}
pub(crate) fn prepare_default_transition(id: &str) -> Result<Change> {
    use filmcraft_project::vtransition::find_transition;
    let def = find_effect(id)
        .or_else(|| find_transition(id, EffectKind::VideoTransition))
        .or_else(|| find_transition(id, EffectKind::AudioTransition))
        .or_else(|| filmcraft_project::effect_defs().iter().find(|d| d.name.eq_ignore_ascii_case(id)))
        .ok_or_else(|| bad("effects.setDefaultTransition", format!("no effect `{id}`")))?;
    let audio = match def.kind {
        EffectKind::AudioTransition => true,
        EffectKind::VideoTransition => false,
        _ => return Err(bad("effects.setDefaultTransition", format!("`{}` is not a transition", def.name))),
    };
    Ok(Change { edit: Edit::DefaultTransition { audio, definition: def.id } })
}
impl Change {
    // Preview and apply use the same native service. A historical caller can
    // validate testimony against its immutable graph without a dummy Session.
    pub(crate) fn preview_for(&self, project: &Project, state: &EditorState) -> Result<EditorState> {
        let mut expected = state.clone();
        self.apply_to(project, &mut expected)?;
        Ok(expected)
    }
    fn apply_to(&self, project: &Project, state: &mut EditorState) -> Result<Value> {
        match &self.edit {
            Edit::Mode { sequence, track, mode } => {
                if state.active_sequence != Some(*sequence) {
                    return Err(bad("clipMixer.setMode", "prepared Clip Mixer mode belongs to another current sequence"));
                }
                audio_track(project, state, *track)?;
                state.clip_mixer_modes.insert(track.0, *mode);
                Ok(json!({"track": track.0, "mode": mode.label()}))
            }
            Edit::DefaultTransition { audio, definition } => {
                if *audio {
                    state.default_audio_transition = definition.to_string();
                } else {
                    state.default_video_transition = definition.to_string();
                }
                Ok(json!({"video": state.default_video_transition, "audio": state.default_audio_transition}))
            }
        }
    }
    pub(crate) fn apply(&self, s: &mut Session) -> Result<Value> {
        self.apply_to(&s.project, &mut s.state)
    }
}

#[cfg(test)]
mod tests;
