//! Bounded descriptive history, separate from the private live trim owner.
//!
//! Local snapshot indices encode Arc identity, including distinct equal-byte
//! projects. A caller mapping these into a checkpoint must preserve that identity
//! relation. Decoding validates a historical transition; it grants no current
//! writer, source witness, clock, clipboard, checkpoint or Undo authority.
use super::{Prepared, Summary, describe_snapshot, same_stack};
use crate::{
    EngineError, History, Result, commands,
    trim::{EditPoint, TrimShift, push_dynamic_history},
};
use filmcraft_project::{ItemId, Project};
use filmcraft_time::Tick;
use serde::{Deserialize, Serialize};
use std::{collections::BTreeSet, io::Write, sync::Arc};

const MAX_STEPS: usize = 256;
const MAX_PROJECTS: usize = MAX_STEPS + 1;
const MAX_POINTS: usize = 4096;
const MAX_BYTES: usize = 4 * 1024 * 1024;
fn error(message: impl Into<String>) -> EngineError {
    EngineError::Other(message.into())
}

// Keep old human editor serde contracts unchanged; this private evidence is strict.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Shift {
    out_shift: Tick,
    in_shift: Tick,
}
impl From<TrimShift> for Shift {
    fn from(s: TrimShift) -> Self {
        Self { out_shift: s.out_shift, in_shift: s.in_shift }
    }
}
impl From<Shift> for TrimShift {
    fn from(s: Shift) -> Self {
        Self { out_shift: s.out_shift, in_shift: s.in_shift }
    }
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
struct Point {
    clip: filmcraft_project::ClipId,
    out: bool,
    kind: crate::trim::TrimKind,
}
impl From<EditPoint> for Point {
    fn from(p: EditPoint) -> Self {
        Self { clip: p.clip, out: p.out, kind: p.kind }
    }
}
impl From<Point> for EditPoint {
    fn from(p: Point) -> Self {
        Self { clip: p.clip, out: p.out, kind: p.kind }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
struct Step {
    label: String,
    project: usize,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
struct Stack {
    undo: Vec<Step>,
    redo: Vec<Step>,
    limit: usize,
    merge_key: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Dynamic {
    before: usize,
    offset: Tick,
    shift_before: Shift,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Recipe {
    version: u32,
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "preservation_policy")]
    preserve: Option<bool>,
    command: String,
    snapshots: usize,
    before_project: usize,
    after_project: usize,
    sequence: Option<ItemId>,
    points: Vec<Point>,
    before_shift: Shift,
    after_shift: Shift,
    dynamic: Option<Dynamic>,
    around: bool,
    before_history: Stack,
    after_history: Stack,
}
fn preservation_policy<'de, D: serde::Deserializer<'de>>(d: D) -> std::result::Result<Option<bool>, D::Error> {
    bool::deserialize(d).map(Some)
}
/// An opaque historical recipe; no live preparation or mutation methods.
#[derive(Clone)]
pub struct Evidence {
    recipe: Recipe,
}
/// Strong snapshots keep discarded Redo and limit-zero trim states recoverable.
/// This capture does not install those snapshots in the live history.
pub struct Capture {
    evidence: Evidence,
    projects: Vec<Arc<Project>>,
    historical: Historical,
}
/// A checked historical description. It never reconstructs a live gesture.
pub struct Historical {
    sequence: Option<ItemId>,
    points: Vec<EditPoint>,
    before_shift: TrimShift,
    before_project: Arc<Project>,
    after_project: Arc<Project>,
    before_history: History,
    after_history: History,
    after_shift: TrimShift,
    summary: Summary,
}
impl Historical {
    /// Bind the descriptive trim recipe to the independently checked original
    /// editor proof. A historical mode cannot smuggle a trim-shift or label change.
    pub fn validate_editor_basis(&self, sequence: Option<ItemId>, points: &[EditPoint], shift: TrimShift) -> Result<()> {
        if self.sequence != sequence || self.points != points || self.before_shift != shift {
            return Err(error("historical trim recipe differs from the original editor basis"));
        }
        Ok(())
    }
    pub fn before_project(&self) -> &Arc<Project> {
        &self.before_project
    }
    pub fn after_project(&self) -> &Arc<Project> {
        &self.after_project
    }
    pub fn before_history(&self) -> &History {
        &self.before_history
    }
    pub fn after_history(&self) -> &History {
        &self.after_history
    }
    pub fn trim_shift_after(&self) -> TrimShift {
        self.after_shift
    }
    pub fn summary(&self) -> &Summary {
        &self.summary
    }
}
impl Capture {
    pub fn evidence(&self) -> &Evidence {
        &self.evidence
    }
    pub fn projects(&self) -> &[Arc<Project>] {
        &self.projects
    }
    pub fn historical(&self) -> &Historical {
        &self.historical
    }
}
fn push(pool: &mut Vec<Arc<Project>>, project: &Arc<Project>) -> Result<usize> {
    if let Some(index) = pool.iter().position(|p| Arc::ptr_eq(p, project)) {
        return Ok(index);
    }
    if pool.len() >= MAX_PROJECTS {
        return Err(error("trim history evidence exceeds its complete snapshot bound"));
    }
    pool.push(project.clone());
    Ok(pool.len() - 1)
}
fn stack(history: &History, pool: &mut Vec<Arc<Project>>) -> Result<Stack> {
    if history.undo.len().saturating_add(history.redo.len()) > MAX_STEPS || history.limit > 1000 || history.merge_key.as_ref().is_some_and(|k| k.len() > 2048) {
        return Err(error("trim history exceeds bounded native evidence"));
    }
    let mut entries = |steps: &[(String, Arc<Project>)]| -> Result<Vec<Step>> {
        steps
            .iter()
            .map(|(label, project)| {
                if label.len() > 2048 {
                    return Err(error("trim history label exceeds its bound"));
                }
                Ok(Step { label: label.clone(), project: push(pool, project)? })
            })
            .collect()
    };
    Ok(Stack { undo: entries(&history.undo)?, redo: entries(&history.redo)?, limit: history.limit, merge_key: history.merge_key.clone() })
}
fn project(pool: &[Arc<Project>], index: usize) -> Result<&Arc<Project>> {
    pool.get(index).ok_or_else(|| error("trim evidence has an invalid snapshot reference"))
}
fn unstack(saved: &Stack, pool: &[Arc<Project>]) -> Result<History> {
    if saved.undo.len().saturating_add(saved.redo.len()) > MAX_STEPS || saved.limit > 1000 || saved.merge_key.as_ref().is_some_and(|k| k.len() > 2048) {
        return Err(error("historical trim stack exceeds its bound"));
    }
    let entries = |steps: &[Step]| -> Result<Vec<(String, Arc<Project>)>> {
        steps
            .iter()
            .map(|s| {
                if s.label.len() > 2048 {
                    return Err(error("historical trim label exceeds its bound"));
                }
                Ok((s.label.clone(), project(pool, s.project)?.clone()))
            })
            .collect()
    };
    Ok(History { undo: entries(&saved.undo)?, redo: entries(&saved.redo)?, limit: saved.limit, merge_key: saved.merge_key.clone() })
}
pub(crate) fn same_history(a: &History, b: &History) -> bool {
    a.limit == b.limit && a.merge_key == b.merge_key && same_stack(&a.undo, &b.undo) && same_stack(&a.redo, &b.redo)
}
impl Prepared {
    /// Describe the same original command without executing it or renewing its
    /// live preparation. Call `validate` again at the actual publication fence.
    pub fn capture_history(&self) -> Result<Capture> {
        self.capture_for(self.command, false)
    }
    /// Explicit version-two evidence for the non-journaled human clip-mode
    /// command. It binds the same observed trim/history but projects no settlement.
    pub fn capture_preserved_history(&self, command: &str) -> Result<Capture> {
        if command != "clipMixer.setMode" || commands::find(command).is_none_or(|c| c.journal) {
            return Err(error("preserved history requires the original non-journaled clip-mode command"));
        }
        self.capture_for(command, true)
    }
    fn capture_for(&self, command: &str, preserve: bool) -> Result<Capture> {
        if self.points.len() > MAX_POINTS {
            return Err(error("trim edit-point evidence exceeds its bound"));
        }
        let mut pool = Vec::new();
        let before_project = push(&mut pool, &self.project)?;
        let before_history = stack(&self.history, &mut pool)?;
        let dynamic = self
            .playback
            .dynamic
            .as_ref()
            .map(|d| -> Result<Dynamic> { Ok(Dynamic { before: push(&mut pool, &d.before)?, offset: d.offset, shift_before: d.shift_before.into() }) })
            .transpose()?;
        let after_project = push(&mut pool, if preserve { &self.project } else { self.project_after() })?;
        let mut after = self.history.clone();
        if let Some(label) = self.summary.label.filter(|_| !preserve) {
            let d = self.playback.dynamic.as_ref().ok_or_else(|| error("committed trim lacks its original snapshot"))?;
            push_dynamic_history(&mut after, label, d.before.clone());
        }
        let after_history = stack(&after, &mut pool)?;
        let evidence = Evidence {
            recipe: Recipe {
                version: if preserve { 2 } else { 1 },
                preserve: preserve.then_some(true),
                command: command.into(),
                snapshots: pool.len(),
                before_project,
                after_project,
                sequence: self.sequence,
                points: self.points.iter().copied().map(Point::from).collect(),
                before_shift: self.shift.into(),
                after_shift: if preserve { self.shift.into() } else { self.trim_shift_after().into() },
                dynamic,
                around: self.playback.around.is_some(),
                before_history,
                after_history,
            },
        };
        let historical = evidence.validate(&pool, command)?;
        if historical.summary != if preserve { Summary::default() } else { self.summary.clone() } {
            return Err(error("trim evidence differs from the original shared decision"));
        }
        Ok(Capture { evidence, projects: pool, historical })
    }
}
struct Bounded {
    bytes: Vec<u8>,
}
impl Write for Bounded {
    fn write(&mut self, input: &[u8]) -> std::io::Result<usize> {
        if self.bytes.len().checked_add(input.len()).is_none_or(|n| n > MAX_BYTES) {
            return Err(std::io::Error::other("trim evidence exceeds its wire bound"));
        }
        self.bytes.extend_from_slice(input);
        Ok(input.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
pub(in crate::trim) fn bounded_bytes(value: &impl serde::Serialize) -> Result<Vec<u8>> {
    let mut out = Bounded { bytes: Vec::new() };
    serde_json::to_writer(&mut out, value).map_err(|e| error(e.to_string()))?;
    Ok(out.bytes)
}
impl Evidence {
    pub fn encode(&self) -> Result<Vec<u8>> {
        bounded_bytes(&self.recipe)
    }
    pub fn decode(bytes: &[u8]) -> Result<Self> {
        if bytes.len() > MAX_BYTES {
            return Err(error("trim evidence exceeds its wire bound"));
        }
        let recipe: Recipe = serde_json::from_slice(bytes).map_err(|e| error(e.to_string()))?;
        let evidence = Self { recipe };
        evidence.shape()?;
        Ok(evidence)
    }
    fn shape(&self) -> Result<()> {
        let r = &self.recipe;
        let spec = commands::find(&r.command).ok_or_else(|| error("unknown historical trim command"))?;
        if !(r.version == 1 && r.preserve.is_none() && spec.journal
            || r.version == 2 && r.preserve == Some(true) && r.command == "clipMixer.setMode" && !spec.journal)
            || r.command.len() > 256
            || r.snapshots == 0
            || r.snapshots > MAX_PROJECTS
            || r.before_project != 0
            || r.points.len() > MAX_POINTS
        {
            return Err(error("invalid historical trim evidence shape or version"));
        }
        for h in [&r.before_history, &r.after_history] {
            if h.undo.len().saturating_add(h.redo.len()) > MAX_STEPS
                || h.limit > 1000
                || h.merge_key.as_ref().is_some_and(|k| k.len() > 2048)
                || h.undo.iter().chain(&h.redo).any(|s| s.label.len() > 2048 || s.project >= r.snapshots)
            {
                return Err(error("invalid bounded historical trim stack"));
            }
        }
        if r.after_project >= r.snapshots || r.dynamic.as_ref().is_some_and(|d| d.before >= r.snapshots) {
            return Err(error("invalid historical trim project reference"));
        }
        let mut used = r
            .before_history
            .undo
            .iter()
            .chain(&r.before_history.redo)
            .chain(&r.after_history.undo)
            .chain(&r.after_history.redo)
            .map(|s| s.project)
            .collect::<BTreeSet<_>>();
        used.extend([r.before_project, r.after_project]);
        if let Some(d) = &r.dynamic {
            used.insert(d.before);
        }
        if used.len() != r.snapshots {
            return Err(error("detached snapshot in historical trim evidence"));
        }
        Ok(())
    }
    pub fn validate(&self, pool: &[Arc<Project>], command: &str) -> Result<Historical> {
        self.shape()?;
        let r = &self.recipe;
        if r.command != command || pool.len() != r.snapshots {
            return Err(error("foreign command or incomplete historical trim snapshots"));
        }
        // Never collapse distinct original snapshots merely because their bytes match.
        for (i, p) in pool.iter().enumerate() {
            if pool.get(..i).is_none_or(|prior| prior.iter().any(|q| Arc::ptr_eq(p, q))) {
                return Err(error("historical trim snapshot identity relation was collapsed"));
            }
        }
        let before = project(pool, r.before_project)?;
        if r.sequence.is_some_and(|id| before.sequence(id).is_none()) {
            return Err(error("historical trim sequence is absent from its original snapshot"));
        }
        let before_history = unstack(&r.before_history, pool)?;
        let dynamic = r.dynamic.as_ref().map(|d| -> Result<_> { Ok((project(pool, d.before)?, d.offset)) }).transpose()?;
        let points = r.points.iter().copied().map(EditPoint::from).collect::<Vec<_>>();
        let summary = if r.version == 2 { Summary::default() } else { describe_snapshot(before, &points, dynamic, r.around, &before_history, command) };
        let mut expected_history = before_history.clone();
        if let Some(label) = summary.label {
            let d = r.dynamic.as_ref().ok_or_else(|| error("committed historical trim lacks its original snapshot"))?;
            push_dynamic_history(&mut expected_history, label, project(pool, d.before)?.clone());
        }
        let preserves = r.version == 2 || super::LIVE.contains(&command);
        let (after_project, after_shift) = if !preserves && !summary.dynamic_committed {
            if let Some(d) = &r.dynamic { (project(pool, d.before)?, d.shift_before) } else { (before, r.before_shift) }
        } else {
            (before, r.before_shift)
        };
        let after_history = unstack(&r.after_history, pool)?;
        if !Arc::ptr_eq(after_project, project(pool, r.after_project)?) || after_shift != r.after_shift || !same_history(&expected_history, &after_history) {
            return Err(error("historical trim differs from the original native history transition"));
        }
        Ok(Historical {
            sequence: r.sequence,
            points,
            before_shift: r.before_shift.into(),
            before_project: before.clone(),
            after_project: after_project.clone(),
            before_history,
            after_history,
            after_shift: after_shift.into(),
            summary,
        })
    }
    /// Pure description of the original DEFAULT forecast, even when the
    /// actual setting preserves the live trim. This creates no live owner.
    pub fn default_forecast(&self, pool: &[Arc<Project>]) -> Result<Summary> {
        self.validate(pool, &self.recipe.command)?;
        let r = &self.recipe;
        let points = r.points.iter().copied().map(EditPoint::from).collect::<Vec<_>>();
        let dynamic = r.dynamic.as_ref().map(|d| -> Result<_> { Ok((project(pool, d.before)?, d.offset)) }).transpose()?;
        Ok(describe_snapshot(project(pool, r.before_project)?, &points, dynamic, r.around, &unstack(&r.before_history, pool)?, "effects.setDefaultTransition"))
    }
    pub fn playback_kind(&self) -> (bool, bool) {
        (self.recipe.dynamic.is_some(), self.recipe.around)
    }
    /// Historical correspondence only, never a recovered Following or writer.
    pub fn validate_following_to(&self, pool: &[Arc<Project>], next: &Self, next_pool: &[Arc<Project>]) -> Result<()> {
        self.validate_following_from(pool, next, next_pool, "effects.setDefaultTransition")
    }
    /// Same immutable correspondence for the closed sequence-view family.
    /// It never reconstructs the private original clock or a publication owner.
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn validate_sequence_view_following_to(&self, pool: &[Arc<Project>], next: &Self, next_pool: &[Arc<Project>]) -> Result<()> {
        if !super::super::clock::SequenceViewCommand::ALL.iter().any(|command| command.id() == next.recipe.command) {
            return Err(error("historical view command is outside the finite family"));
        }
        self.validate_following_from(pool, next, next_pool, "sequence.open")
    }
    fn validate_following_from(&self, pool: &[Arc<Project>], next: &Self, next_pool: &[Arc<Project>], original_command: &str) -> Result<()> {
        let a = self.validate(pool, original_command)?;
        let b = next.validate(next_pool, &next.recipe.command)?;
        let ar = &self.recipe;
        let br = &next.recipe;
        if ar.dynamic.is_some() && ar.around
            || br.dynamic.is_some() && br.around
            || ar.sequence != br.sequence
            || ar.points != br.points
            || ar.around != br.around
            || a.before_history.limit != b.before_history.limit
            || !same_stack(&a.before_history.undo, &b.before_history.undo)
            || !same_stack(&a.before_history.redo, &b.before_history.redo)
        {
            return Err(error("original/publication history or edit-point identity differs"));
        }
        let dynamic = match (&ar.dynamic, &br.dynamic) {
            (Some(x), Some(y)) => {
                let points = ar.points.iter().copied().map(EditPoint::from).collect::<Vec<_>>();
                if !Arc::ptr_eq(project(pool, x.before)?, project(next_pool, y.before)?)
                    || x.shift_before != y.shift_before
                    || super::super::shifted(&points, x.shift_before.into(), x.offset) != ar.before_shift.into()
                    || super::super::shifted(&points, y.shift_before.into(), y.offset) != br.before_shift.into()
                {
                    return Err(error("following historical dynamic baseline or native shift differs"));
                }
                true
            }
            (None, None) if Arc::ptr_eq(a.before_project(), b.before_project()) && ar.before_shift == br.before_shift => false,
            _ => return Err(error("following historical playback kind or project relation differs")),
        };
        if a.before_history.merge_key != b.before_history.merge_key && !(dynamic && b.before_history.merge_key.is_none()) {
            return Err(error("following history changed a non-clock merge marker"));
        }
        Ok(())
    }
    /// Rebind only descriptive local indices into an independently loaded capsule
    /// pool. The injective mapping preserves identity even for identical bytes.
    pub fn validate_mapped(&self, pool: &[Arc<Project>], mapping: &[usize], command: &str) -> Result<Historical> {
        self.shape()?;
        if pool.len() > MAX_PROJECTS || mapping.len() != self.recipe.snapshots || mapping.iter().copied().collect::<BTreeSet<_>>().len() != mapping.len() {
            return Err(error("incomplete or collapsed historical trim snapshot mapping"));
        }
        let local = mapping.iter().map(|i| project(pool, *i).cloned()).collect::<Result<Vec<_>>>()?;
        self.validate(&local, command)
    }
}

#[cfg(test)]
mod tests;
