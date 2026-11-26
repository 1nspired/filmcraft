//! Original pre-command trim settlement shared by human and prepared native callers.
//!
//! A prepared settlement binds the actual live gesture, its original project and
//! history, not only visible offset values. It grants no source evidence, wire
//! capability, checkpoint ownership or authority to Undo a human trim. Guarded
//! adapters must also validate their complete native ticket and independent Stop
//! before applying it. No serialized state can reconstruct this live owner.
use super::{DynamicTrim, EditPoint, PlayAround, TrimPlayback, TrimShift, dynamic_commit_label_for};
use crate::{EngineError, History, Result, Session, commands};
use filmcraft_project::{ItemId, Project};
use filmcraft_time::Tick;
use std::sync::Arc;

const LIVE: [&str; 5] = ["trim.shuttle", "trim.tick", "trim.shuttleStop", "trim.cancelDynamic", "trim.playAround"];

/// Descriptive native effects of settling the already existing gesture.
/// A committed dynamic trim belongs to that original gesture, not a new agent edit.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Summary {
    pub dynamic_finished: bool,
    pub dynamic_committed: bool,
    pub undo_entry_retained: bool,
    pub redo_discarded: bool,
    pub loop_stopped: bool,
    pub restored_before_project: bool,
    pub offset: Tick,
    pub label: Option<&'static str>,
}

fn describe(project: &Arc<Project>, points: &[EditPoint], playback: &TrimPlayback, history: &History, command: &str) -> Summary {
    if LIVE.contains(&command) {
        return Summary::default();
    }
    describe_snapshot(project, points, playback.dynamic.as_ref().map(|d| (&d.before, d.offset)), playback.around.is_some(), history, command)
}

// No live owner, clock, grant or mutation can be reconstructed from these values.
pub(super) fn describe_snapshot(
    project: &Arc<Project>,
    points: &[EditPoint],
    dynamic: Option<(&Arc<Project>, Tick)>,
    around: bool,
    history: &History,
    command: &str,
) -> Summary {
    if LIVE.contains(&command) {
        return Summary::default();
    }
    let label = dynamic.and_then(|(before, offset)| dynamic_commit_label_for(offset, Arc::ptr_eq(project, before), points));
    Summary {
        dynamic_finished: dynamic.is_some(),
        dynamic_committed: label.is_some(),
        undo_entry_retained: label.is_some() && (history.limit > 0 || !history.undo.is_empty()),
        redo_discarded: label.is_some() && !history.redo.is_empty(),
        loop_stopped: !command.starts_with("trim.") && around,
        restored_before_project: dynamic.is_some_and(|(before, _)| label.is_none() && !Arc::ptr_eq(project, before)),
        offset: dynamic.map(|(_, offset)| offset).unwrap_or(Tick::ZERO),
        label,
    }
}

/// The exact original top-level behavior. The caller has already resolved a
/// journaled command. Preparation uses this same service after validating its owner.
pub(crate) fn settle_before_command(s: &mut Session, command: &str) -> Summary {
    let summary = describe(&s.project, &s.state.edit_points, &s.trim_play, &s.history, command);
    if LIVE.contains(&command) {
        return summary;
    }
    if s.trim_play.dynamic.is_some() {
        super::commit(s);
    }
    if !command.starts_with("trim.") {
        s.trim_play.around = None;
    }
    summary
}

/// Immutable in-memory preparation; cloning preserves its live-owner identity.
/// This is not serializable and cannot be rebuilt from a path, digest or receipt.
#[derive(Clone)]
#[must_use]
pub struct Prepared {
    pub(super) command: &'static str,
    pub(super) project: Arc<Project>,
    pub(super) playback: TrimPlayback,
    pub(super) sequence: Option<ItemId>,
    pub(super) points: Vec<EditPoint>,
    pub(super) shift: TrimShift,
    pub(super) revision: u64,
    pub(super) saved_revision: u64,
    pub(super) history: History,
    pub(super) summary: Summary,
}

fn same_dynamic(a: &Option<DynamicTrim>, b: &Option<DynamicTrim>) -> bool {
    match (a, b) {
        (None, None) => true,
        (Some(a), Some(b)) => {
            Arc::ptr_eq(&a.owner, &b.owner)
                && Arc::ptr_eq(&a.before, &b.before)
                && a.shift_before == b.shift_before
                && a.offset == b.offset
                && a.speed.to_bits() == b.speed.to_bits()
                && a.anchor_clock.to_bits() == b.anchor_clock.to_bits()
                && a.anchor_offset == b.anchor_offset
                && a.at_limit == b.at_limit
        }
        _ => false,
    }
}
fn same_loop(a: &Option<PlayAround>, b: &Option<PlayAround>) -> bool {
    match (a, b) {
        (None, None) => true,
        (Some(a), Some(b)) => {
            Arc::ptr_eq(&a.owner, &b.owner) && a.start == b.start && a.end == b.end && a.looping == b.looping && a.clock0.to_bits() == b.clock0.to_bits()
        }
        _ => false,
    }
}
pub(super) fn same_stack(a: &[(String, Arc<Project>)], b: &[(String, Arc<Project>)]) -> bool {
    a.len() == b.len() && a.iter().zip(b).all(|((al, ap), (bl, bp))| al == bl && Arc::ptr_eq(ap, bp))
}

/// Read-only preparation for a real registered journaled native command.
/// The surrounding adapter still owns complete editor/context and Stop checks.
pub fn prepare(s: &Session, command: &str) -> Result<Prepared> {
    s.require_command_owner(command)?;
    let spec = commands::find(command).ok_or_else(|| EngineError::UnknownCommand(command.into()))?;
    if !spec.journal {
        return Err(EngineError::Other("pre-command trim settlement requires a journaled native command".into()));
    }
    Ok(Prepared {
        command: spec.id,
        project: s.project.clone(),
        playback: s.trim_play.clone(),
        sequence: s.state.active_sequence,
        points: s.state.edit_points.clone(),
        shift: s.state.trim_shift,
        revision: s.revision,
        saved_revision: s.saved_revision,
        history: s.history.clone(),
        summary: describe(&s.project, &s.state.edit_points, &s.trim_play, &s.history, command),
    })
}
impl Prepared {
    pub fn summary(&self) -> &Summary {
        &self.summary
    }
    pub fn project_after(&self) -> &Arc<Project> {
        if !LIVE.contains(&self.command) && !self.summary.dynamic_committed {
            if let Some(d) = &self.playback.dynamic {
                return &d.before;
            }
        }
        &self.project
    }
    pub fn trim_shift_after(&self) -> TrimShift {
        if !LIVE.contains(&self.command) && !self.summary.dynamic_committed {
            if let Some(d) = &self.playback.dynamic {
                return d.shift_before;
            }
        }
        self.shift
    }
    /// Validate every dependency before publishing; a restarted identical trim refuses.
    pub fn validate(&self, s: &Session) -> Result<()> {
        if !Arc::ptr_eq(&self.project, &s.project)
            || self.revision != s.revision
            || self.saved_revision != s.saved_revision
            || self.sequence != s.state.active_sequence
            || self.points != s.state.edit_points
            || self.shift != s.state.trim_shift
            || !same_dynamic(&self.playback.dynamic, &s.trim_play.dynamic)
            || !same_loop(&self.playback.around, &s.trim_play.around)
            || self.history.limit != s.history.limit
            || self.history.merge_key != s.history.merge_key
            || !same_stack(&self.history.undo, &s.history.undo)
            || !same_stack(&self.history.redo, &s.history.redo)
        {
            return Err(EngineError::Other("prepared trim settlement expired after live owner, project, edit point or history changed".into()));
        }
        Ok(())
    }
    pub fn apply(self, s: &mut Session) -> Result<Summary> {
        s.require_command_owner(self.command)?;
        self.validate(s)?;
        Ok(settle_before_command(s, self.command))
    }
}

#[cfg(test)]
mod tests;

/// Readonly history evidence for native compound receipts. It retains every
/// referenced snapshot, independently of the live Undo limit. Serialized evidence
/// has no path into a live `Prepared` owner and cannot apply a command.
pub mod historical;
