//! Explicit native range editing shared by human commands and guarded adapters.
use crate::{EngineError, Result, Session, commands::bad};
use filmcraft_edit as edit;
use filmcraft_project::{ItemId, Project, TrackId};
use filmcraft_time::{Tick, TimeRange};
use std::sync::Arc;

#[derive(Clone, Copy)]
pub(crate) enum Kind {
    Lift,
    Extract,
}
pub(crate) struct RangePlan {
    basis: Arc<Project>,
    sequence: ItemId,
    tracks: Vec<TrackId>,
    range: TimeRange,
    kind: Kind,
    markers: bool,
    budget: u64,
}
pub(crate) fn prepare(s: &Session, tracks: Vec<TrackId>, range: TimeRange, kind: Kind, markers: bool) -> Result<RangePlan> {
    let fail = |m| bad("sequence.range", m);
    let q = s.active_sequence().ok_or(EngineError::NoSequence)?;
    let valid = |a: Tick, d: Tick| a >= Tick::ZERO && d > Tick::ZERO && a.0.checked_add(d.0).is_some_and(|end| end <= Tick::MAX.0);
    if q.settings.frame_rate.num <= 0
        || q.settings.frame_rate.den <= 0
        || q.settings.frame_rate.frame_duration() <= Tick::ZERO
        || q.all_tracks().flat_map(|t| &t.items).any(|c| !valid(c.start, c.duration) || c.source_in < Tick::ZERO || !c.speed.is_finite())
        || q.caption_tracks.iter().flat_map(|t| &t.captions).any(|c| !valid(c.start, c.duration))
    {
        return Err(fail("unsupported native range clocks"));
    }
    if q.settings.frame_rate.frame_duration() > Tick::MAX {
        return Err(fail("unsupported native range frame"));
    }
    for c in q.all_tracks().flat_map(|t| &t.items) {
        crate::clip_ops::replacement_source_out(c, c.source_in)?;
    }
    if range.start < Tick::ZERO
        || range.duration <= Tick::ZERO
        || range.start.0.checked_add(range.duration.0).is_none_or(|n| n > Tick::MAX.0)
        || tracks.len() > 4096
        || tracks.iter().collect::<std::collections::BTreeSet<_>>().len() != tracks.len()
        || tracks.iter().any(|id| q.track(*id).is_none())
    {
        return Err(fail("need a positive bounded range and distinct current tracks"));
    }
    let objects = q
        .all_tracks()
        .map(|t| t.items.len().saturating_add(t.transitions.len()))
        .chain(q.caption_tracks.iter().map(|t| t.captions.len()))
        .fold(0usize, usize::saturating_add);
    if objects > 100_000 {
        return Err(fail("range editing exceeds 100000 objects"));
    }
    let crosses = |a: Tick, b: Tick| (a < range.start && b > range.start) || (a < range.end() && b > range.end());
    let affected = |t: &filmcraft_project::Track| !t.locked && (tracks.contains(&t.id) || (matches!(kind, Kind::Extract) && t.sync_lock));
    let splits = q.all_tracks().filter(|t| affected(t)).flat_map(|t| &t.items).any(|c| crosses(c.start, c.end()))
        || matches!(kind, Kind::Extract)
            && q.caption_tracks.iter().filter(|t| !t.locked && t.sync_lock).flat_map(|t| &t.captions).any(|c| crosses(c.start, c.end()));
    let budget = if splits {
        objects.checked_mul(8).and_then(|n| n.checked_add(64)).and_then(|n| u64::try_from(n).ok()).ok_or_else(|| fail("range allocation budget overflow"))?
    } else {
        0
    };
    if budget > 0 {
        crate::project_ids::allocation_start(&s.project, budget)?;
    }
    Ok(RangePlan { basis: s.project.clone(), sequence: s.state.active_sequence.ok_or(EngineError::NoSequence)?, tracks, range, kind, markers, budget })
}
impl RangePlan {
    pub(crate) fn execute(self, s: &mut Session) -> Result<Vec<u64>> {
        if !Arc::ptr_eq(&self.basis, &s.project) || s.state.active_sequence != Some(self.sequence) {
            return Err(bad("sequence.range", "prepared range expired after document/sequence change"));
        }
        let pool = s.media.clone();
        let label = match self.kind {
            Kind::Lift => "Lift",
            Kind::Extract => "Extract",
        };
        let reserved = if self.budget > 0 { crate::project_ids::allocation_start(&s.project, self.budget)? } else { s.project.next_id };
        let removed = s.edit(label, |p, st| {
            let snapshot = p.clone();
            let starts = snapshot.clone();
            let durations = |id| crate::media_duration(&snapshot, &pool, id);
            let media_starts = |id| crate::media_start(&starts, id);
            let q = p.sequence_mut(self.sequence).ok_or(EngineError::NoSequence)?;
            let mut next = reserved;
            let mut ctx = edit::EditCtx {
                next_id: &mut next,
                media_duration: &durations,
                media_start: &media_starts,
                min_duration: q.settings.frame_rate.frame_duration(),
            };
            let removed = match self.kind {
                Kind::Lift => edit::lift(q, &self.tracks, self.range, &mut ctx),
                Kind::Extract => {
                    let removed = edit::extract(q, &self.tracks, self.range, &mut ctx);
                    if self.markers {
                        crate::sequence_tools::ripple_markers(&mut q.markers, self.range.end(), -self.range.duration);
                    }
                    q.mark_in = None;
                    q.mark_out = None;
                    q.split = Default::default();
                    removed
                }
            };
            q.check().map_err(EngineError::Other)?;
            crate::move_ops::repair_clip_state(q, st);
            if next.checked_sub(reserved).is_none_or(|used| used > self.budget) {
                return Err(bad("sequence.range", "range exceeded its reserved identity budget"));
            }
            if next != reserved {
                p.next_id = next;
            }
            Ok(removed.iter().map(|id| id.0).collect())
        })?;
        if matches!(self.kind, Kind::Extract) {
            s.set_playhead(self.range.start);
        }
        Ok(removed)
    }
}
