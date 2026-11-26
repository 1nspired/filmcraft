//! Shared component construction for human Source/Project placement and native
//! sequence assembly. Intrinsics, auto points, PAR and scaling stay identical.
use filmcraft_project::{ItemId, ItemKind, Project, SequenceSettings, TrackItem, TrackKind, resolve_auto_points};
use filmcraft_time::{Tick, TimeRange};

use crate::{Result, commands::bad};

pub(crate) fn component(
    project: &mut Project,
    item: ItemId,
    kind: TrackKind,
    at: Tick,
    source: TimeRange,
    settings: &SequenceSettings,
    scaling: &str,
    label: &str,
) -> Result<TrackItem> {
    let pi = project.item(item).ok_or_else(|| bad(label, "missing placement item"))?;
    let size = match &pi.kind {
        ItemKind::Media(m) => m.info.video.as_ref().map(|v| (v.width, v.height)),
        ItemKind::Sequence(q) => Some((q.settings.width, q.settings.height)),
        ItemKind::AdjustmentLayer { width, height, .. } => Some((*width, *height)),
        _ => None,
    }
    .unwrap_or((1920, 1080));
    let is_media = matches!(pi.kind, ItemKind::Media(_) | ItemKind::Subclip { .. });
    let par = project.source_par(item).unwrap_or(settings.par);
    let mut clip = project.make_track_item(item, kind, at, source, settings.frame_rate).ok_or_else(|| bad(label, "bad placement item"))?;
    if kind == TrackKind::Video {
        let frame = (settings.width, settings.height);
        for effect in &mut clip.effects {
            resolve_auto_points(effect, frame, size);
        }
        let shown = filmcraft_project::conformed_size(size, par, settings.par);
        if is_media && shown != (f64::from(frame.0), f64::from(frame.1)) {
            crate::settings::apply_media_scaling(&mut clip, scaling, frame, shown);
        }
    }
    Ok(clip)
}
