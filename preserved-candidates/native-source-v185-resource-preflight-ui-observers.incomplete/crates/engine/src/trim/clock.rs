//! Readonly evidence of the ORIGINAL human trim clock's latest derived state.
//! Private live values only: never serialized, restored, a ticket or a writer.
use super::{TrimShift, command_settlement};
use crate::{EngineError, Result, Session};
use filmcraft_project::{ClipId, ItemId, Project};
use filmcraft_time::Tick;
use serde_json::Value;
use std::sync::{Arc, Weak};

#[derive(Clone, Debug, PartialEq, Eq)]
enum Motion {
    Dynamic { offset: Tick, speed: u64, clock: u64, anchor: Tick, limit: bool },
    Around { start: Tick, end: Tick, looping: bool, clock: u64 },
}
fn motion(s: &Session) -> Option<Motion> {
    match (&s.trim_play.dynamic, &s.trim_play.around) {
        (Some(d), None) => {
            Some(Motion::Dynamic { offset: d.offset, speed: d.speed.to_bits(), clock: d.anchor_clock.to_bits(), anchor: d.anchor_offset, limit: d.at_limit })
        }
        (None, Some(a)) => Some(Motion::Around { start: a.start, end: a.end, looping: a.looping, clock: a.clock0.to_bits() }),
        _ => None,
    }
}
#[derive(Clone, Debug)]
pub(super) struct Derived {
    project: Weak<Project>,
    revision: u64,
    saved: u64,
    merge_key: Option<String>,
    sequence: Option<ItemId>,
    playhead: Option<Tick>,
    shift: TrimShift,
    selection: Option<Vec<ClipId>>,
    motion: Motion,
}
impl Derived {
    fn capture(s: &Session) -> Option<Self> {
        // Optional following evidence must never add an unbounded per-frame
        // allocation to the original human gesture. Oversized state keeps
        // playing normally but cannot acquire this private clock evidence.
        if s.history.merge_key.as_ref().is_some_and(|k| k.len() > 2048) || s.state.selection_follows_playhead && s.state.selection.len() > 4096 {
            return None;
        }
        Some(Self {
            project: Arc::downgrade(&s.project),
            revision: s.revision,
            saved: s.saved_revision,
            merge_key: s.history.merge_key.clone(),
            sequence: s.state.active_sequence,
            playhead: s.state.active_sequence.and_then(|id| s.state.playheads.get(&id).copied()),
            shift: s.state.trim_shift,
            selection: s.state.selection_follows_playhead.then(|| s.state.selection.clone()),
            motion: motion(s)?,
        })
    }
    fn base_matches(&self, s: &Session) -> bool {
        Weak::ptr_eq(&self.project, &Arc::downgrade(&s.project))
            && self.revision == s.revision
            && self.saved == s.saved_revision
            && self.merge_key == s.history.merge_key
            && self.sequence == s.state.active_sequence
            && self.shift == s.state.trim_shift
            && Some(&self.motion) == motion(s).as_ref()
    }
    fn matches(&self, s: &Session) -> bool {
        self.base_matches(s)
            && self.playhead == s.state.active_sequence.and_then(|id| s.state.playheads.get(&id).copied())
            && self
                .selection
                .as_ref()
                .map_or(!s.state.selection_follows_playhead, |selection| s.state.selection_follows_playhead && selection == &s.state.selection)
    }
}
fn latest(s: &Session) -> Option<&Derived> {
    match (&s.trim_play.dynamic, &s.trim_play.around) {
        (Some(d), None) => d.clock.as_ref(),
        (None, Some(a)) => a.clock.as_ref(),
        _ => None,
    }
}
pub(super) fn current(s: &Session) -> bool {
    latest(s).is_some_and(|d| d.matches(s))
}
// Before the original setter can overwrite playhead or selection, seal all
// fields of its predecessor. A later real tick cannot bless a foreign change.
pub(super) fn current_base(s: &mut Session) -> bool {
    let valid = current(s);
    if !valid {
        // An original setter can return to the old byte values (especially a
        // repeated loop tick). Once broken, the clock lineage stays broken
        // until an actual new original human gesture creates its owner.
        if let Some(d) = &mut s.trim_play.dynamic {
            d.clock = None;
        }
        if let Some(a) = &mut s.trim_play.around {
            a.clock = None;
        }
    }
    valid
}
// Called only after an actual original human start, re-derived dynamic update,
// or a loop tick whose unchanged project/clock basis has first been checked.
pub(super) fn record(s: &mut Session) {
    let derived = Derived::capture(s);
    if let Some(d) = &mut s.trim_play.dynamic {
        d.clock = derived;
    } else if let Some(a) = &mut s.trim_play.around {
        a.clock = derived;
    }
}
fn error() -> EngineError {
    EngineError::Other("the original human clock lineage or a non-clock dependency changed".into())
}
fn stable_editor(s: &Session, selection_owned: bool) -> Result<Value> {
    stable_editor_values(&s.state, &s.prefs, selection_owned)
}
fn stable_editor_values(state: &crate::EditorState, prefs: &impl serde::Serialize, selection_owned: bool) -> Result<Value> {
    // Serialize the real skipped clipboard fields separately. Native JSON keeps
    // exact i64/u64 values; these private bytes never pass through JavaScript.
    let bytes = command_settlement::historical::bounded_bytes(&(state, &state.clipboard, &state.clipboard_markers, prefs))?;
    let mut value: Value = serde_json::from_slice(&bytes).map_err(|e| EngineError::Other(e.to_string()))?;
    let editor = value.as_array_mut().and_then(|a| a.first_mut()).and_then(Value::as_object_mut).ok_or_else(error)?;
    editor.remove("trim_shift");
    if let Some(id) = state.active_sequence {
        if let Some(playheads) = editor.get_mut("playheads").and_then(Value::as_object_mut) {
            playheads.remove(&id.0.to_string());
        }
    }
    if selection_owned {
        editor.remove("selection");
    }
    Ok(value)
}
/// Closed native view family. No string or serialized command can widen it.
#[cfg(not(target_arch = "wasm32"))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SequenceViewCommand {
    Open,
    Close,
    CloseOthers,
    MoveTab,
}
#[cfg(not(target_arch = "wasm32"))]
impl SequenceViewCommand {
    pub(crate) const ALL: [Self; 4] = [Self::Open, Self::Close, Self::CloseOthers, Self::MoveTab];
    pub(crate) fn id(self) -> &'static str {
        match self {
            Self::Open => "sequence.open",
            Self::Close => "sequence.close",
            Self::CloseOthers => "sequence.closeOthers",
            Self::MoveTab => "sequence.moveTab",
        }
    }
}
/// A distinct, non-serializable original-human-clock preparation.
/// Existing editor defaults and the finite view family keep separate constructors.
/// It produces an exact CURRENT original preparation, never applies a setting.
/// The guarded adapter must separately authenticate its native scope, target,
/// current publication basis and independent Stop. Old Prepared stays strict.
pub struct Following {
    original: command_settlement::Prepared,
    editor: Value,
    path: Option<String>,
    selection_owned: bool,
}
impl Following {
    pub fn capture(s: &Session) -> Result<Self> {
        Self::capture_command(s, "effects.setDefaultTransition")
    }
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn capture_sequence_views(s: &Session) -> Result<Self> {
        Self::capture_command(s, SequenceViewCommand::Open.id())
    }
    fn capture_command(s: &Session, command: &'static str) -> Result<Self> {
        let original = command_settlement::prepare(s, command)?;
        if s.trim_play.active() && !current(s) {
            return Err(error());
        }
        let selection_owned = s.trim_play.active() && s.state.selection_follows_playhead;
        Ok(Self { original, editor: stable_editor(s, selection_owned)?, path: s.path.clone(), selection_owned })
    }
    /// Private readonly continuation, projected BEFORE publication from an
    /// actual validated in-memory preparation. No serialized recipe can call
    /// this with a live owner. The consumer uses it only for outcome readback.
    pub(crate) fn after_command(s: &Session, prepared: &command_settlement::Prepared, editor: &crate::EditorState, preserve: bool) -> Result<Self> {
        prepared.validate(s)?;
        let mut original = prepared.clone();
        if !preserve {
            let capture = prepared.capture_history()?;
            original.project = capture.historical().after_project().clone();
            original.history = capture.historical().after_history().clone();
            original.playback = super::TrimPlayback::default();
            original.shift = capture.historical().trim_shift_after();
            original.summary =
                command_settlement::describe_snapshot(&original.project, &original.points, None, false, &original.history, "effects.setDefaultTransition");
        }
        if original.sequence != editor.active_sequence || original.points != editor.edit_points || original.shift != editor.trim_shift {
            return Err(error());
        }
        let selection_owned = preserve && original.playback.active() && editor.selection_follows_playhead;
        Ok(Self { original, editor: stable_editor_values(editor, &s.prefs, selection_owned)?, path: s.path.clone(), selection_owned })
    }
    pub fn prepare_current(&self, s: &Session) -> Result<command_settlement::Prepared> {
        let before = &self.original;
        // The original raw trim edit clears the merge marker. Accept only a
        // clearing sealed by the uninterrupted original clock chain. A foreign
        // merge change before that tick breaks its pre-derivation attestation.
        let owned_merge_clear = before.playback.active() && s.history.merge_key.is_none() && current(s);
        if self.path != s.path
            || before.saved_revision != s.saved_revision
            || before.sequence != s.state.active_sequence
            || before.points != s.state.edit_points
            || before.history.limit != s.history.limit
            || before.history.merge_key != s.history.merge_key && !owned_merge_clear
            || !command_settlement::same_stack(&before.history.undo, &s.history.undo)
            || !command_settlement::same_stack(&before.history.redo, &s.history.redo)
            || self.editor != stable_editor(s, self.selection_owned)?
        {
            return Err(error());
        }
        match (&before.playback.dynamic, &s.trim_play.dynamic, &before.playback.around, &s.trim_play.around) {
            (Some(a), Some(b), None, None)
                if Arc::ptr_eq(&a.owner, &b.owner) && Arc::ptr_eq(&a.before, &b.before) && a.shift_before == b.shift_before && current(s) => {}
            (None, None, Some(a), Some(b))
                if Arc::ptr_eq(&a.owner, &b.owner) && Arc::ptr_eq(&before.project, &s.project) && before.revision == s.revision && current(s) => {}
            (None, None, None, None) => before.validate(s)?,
            _ => return Err(error()),
        }
        command_settlement::prepare(s, before.command)
    }
    /// Validate the same actual clock first, then project one explicit human view
    /// prelude. All four are non-trim journaled commands with the same settlement.
    /// No default-setting owner can borrow this separate finite view purpose.
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn prepare_current_sequence_view(&self, s: &Session, command: SequenceViewCommand) -> Result<command_settlement::Prepared> {
        if self.original.command != SequenceViewCommand::Open.id() {
            return Err(error());
        }
        let original = self.prepare_current(s)?;
        let current = command_settlement::prepare(s, command.id())?;
        if original.summary() != current.summary() {
            return Err(error());
        }
        Ok(current)
    }
}
#[cfg(test)]
mod tests;
