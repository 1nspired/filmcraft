//! Bounded lost cue fragments owned by exact native condense gaps. These are
//! inverse content, not a saved caption-track snapshot or a second timeline.
use crate::{Result, commands::bad};
use filmcraft_edit::{EditCtx, captions::RestoreFragment};
use filmcraft_project::{Caption, CaptionTrack, ItemId, Project, Sequence, TrackId};
use filmcraft_time::{Tick, TimeRange};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

const SAFE: u64 = 9_007_199_254_740_991;
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct Fragment {
    track: TrackId,
    cue: Caption,
    may_create: bool,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct Gap {
    fragments: Vec<Fragment>,
}
fn error(message: impl Into<String>) -> crate::EngineError {
    bad("agent.restoreCondense", message)
}
fn text(s: &str, max: usize) -> bool {
    s.len() <= max && !s.contains('\0')
}
pub(super) fn validate(archive: &[Gap], gaps: &[TimeRange]) -> Result<()> {
    if archive.is_empty() {
        return Ok(());
    }
    if archive.len() != gaps.len() || archive.len() > 272 {
        return Err(error("caption inverse does not match its owned gaps"));
    }
    let mut count = 0usize;
    let mut locations = BTreeMap::new();
    for (saved, gap) in archive.iter().zip(gaps) {
        let mut ids = BTreeSet::new();
        for f in &saved.fragments {
            let c = &f.cue;
            count = count.saturating_add(1);
            if count > 100000
                || f.track.0 == 0
                || f.track.0 > SAFE
                || c.id.0 == 0
                || c.id.0 > SAFE
                || !ids.insert(c.id)
                || c.start < Tick::ZERO
                || c.duration <= Tick::ZERO
                || c.start.0.checked_add(c.duration.0).is_none_or(|end| end > gap.duration.0)
                || !text(&c.text, 8192)
                || !text(&c.settings, 2048)
                || c.speaker.as_ref().is_some_and(|s| !text(s, 1024))
                || c.cue_id.as_ref().is_some_and(|s| !text(s, 1024))
                || locations.insert(c.id, f.track).is_some_and(|t| t != f.track)
            {
                return Err(error("invalid, duplicate or oversized lost caption fragment"));
            }
        }
    }
    Ok(())
}
pub(super) fn capture(before: &Sequence, after: &Sequence, gaps: &[TimeRange]) -> Result<Vec<Gap>> {
    if before.caption_tracks.is_empty() {
        return Ok(vec![]);
    }
    let mut archive = Vec::new();
    for gap in gaps {
        let mut fragments = Vec::new();
        for track in before.caption_tracks.iter().filter(|t| !t.locked && t.sync_lock) {
            for original in track.captions.iter().filter(|c| c.start < gap.end() && c.end() > gap.start) {
                let first = original.start.max(gap.start);
                let last = original.end().min(gap.end());
                let mut cue = original.clone();
                cue.start = first - gap.start;
                cue.duration = last - first;
                fragments.push(Fragment { track: track.id, cue, may_create: after.find_caption(original.id).is_none() });
            }
        }
        archive.push(Gap { fragments });
    }
    validate(&archive, gaps)?;
    Ok(archive)
}
// A subsequent OWNED removal can erase a cue surviving an earlier removal.
// Preserve its latest human words, and distinguish that cause from human deletion.
pub(super) fn refresh(archive: &mut [Gap], before: &Sequence, after: &Sequence) {
    for f in archive.iter_mut().flat_map(|g| &mut g.fragments) {
        if let Some((track, current)) = before.find_caption(f.cue.id).filter(|(track, _)| *track == f.track) {
            let _ = track;
            f.cue.text = current.text.clone();
            f.cue.speaker = current.speaker.clone();
            f.cue.cue_id = current.cue_id.clone();
            f.cue.settings = current.settings.clone();
            if after.find_caption(current.id).is_none() {
                f.may_create = true;
            }
        }
    }
}
pub(super) fn assert_locations(project: &Project, sequence: ItemId, archive: &[Gap]) -> Result<()> {
    let wanted: BTreeMap<_, _> = archive.iter().flat_map(|g| &g.fragments).map(|f| (f.cue.id.0, f.track)).collect();
    if wanted.is_empty() {
        return Ok(());
    }
    let mut count = 0usize;
    for id in project.items.keys() {
        if wanted.contains_key(&id.0) {
            return Err(error("a lost caption identity is now a project item"));
        }
        let Some(q) = project.sequence(*id) else { continue };
        for track in q.all_tracks() {
            if wanted.contains_key(&track.id.0) || track.items.iter().any(|c| wanted.contains_key(&c.id.0)) {
                return Err(error("a lost caption identity is now media or a track"));
            }
            count = count.saturating_add(track.items.len());
        }
        for track in &q.caption_tracks {
            if wanted.contains_key(&track.id.0) {
                return Err(error("a lost cue identity is now a caption track"));
            }
            for cue in &track.captions {
                if wanted.get(&cue.id.0).is_some_and(|t| *id != sequence || *t != track.id) {
                    return Err(error("a lost cue identity is now in another sequence or caption track"));
                }
            }
            count = count.saturating_add(track.captions.len());
        }
        if count > 1000000 {
            return Err(error("caption inverse global identity audit exceeds one million instances"));
        }
    }
    Ok(())
}
pub(super) fn restore_gap(tracks: &mut [CaptionTrack], gap: TimeRange, saved: Option<&Gap>, ctx: &mut EditCtx) -> Result<()> {
    if let Some(saved) = saved {
        for f in &saved.fragments {
            if !tracks.iter().any(|t| t.id == f.track) {
                return Err(error("an owned caption track was removed"));
            }
        }
    }
    for track in tracks {
        let fragments: Vec<_> = saved
            .into_iter()
            .flat_map(|g| &g.fragments)
            .filter(|f| f.track == track.id)
            .map(|f| RestoreFragment { caption: f.cue.clone(), may_create: f.may_create })
            .collect();
        filmcraft_edit::captions::restore_gap(track, gap.start, gap.duration, &fragments, ctx)?;
    }
    Ok(())
}
