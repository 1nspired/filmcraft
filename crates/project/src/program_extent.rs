//! Optional native program extent. The ordinary editor retains its existing
//! furthest-item duration; Poema explicitly selects its picture/harness law.
use crate::{Sequence, TrackItem};
use filmcraft_time::Tick;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ProgramExtentPolicy {
    Poema,
}

fn furthest<'a>(items: impl Iterator<Item = &'a TrackItem>) -> Tick {
    items.map(TrackItem::end).max().unwrap_or(Tick::ZERO)
}

pub(crate) fn poema_duration(sequence: &Sequence) -> Tick {
    let audio = || sequence.audio_tracks.iter().flat_map(|track| track.items.iter());
    let harness = furthest(audio().filter(|clip| clip.identity.as_ref().and_then(|id| id.protection.as_deref()) == Some("harness")));
    match sequence.video_tracks.first().filter(|track| !track.items.is_empty()) {
        Some(spine) => spine.end().max(harness),
        None => furthest(audio()).max(harness),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Caption, CaptionFormat, CaptionTrack, ClipId, ClipIdentity, ItemId, Label, Project, SequenceSettings, TrackId};
    use filmcraft_time::{FrameRate, TICKS_PER_SECOND};
    use std::collections::BTreeMap;
    fn clip(id: u64, start: Tick, duration: Tick) -> TrackItem {
        TrackItem {
            identity: None,
            source_end_offset: Tick::ZERO,
            id: ClipId(id),
            item: ItemId(900),
            name: format!("fixture{id}"),
            label: Label::Iris,
            start,
            duration,
            source_in: Tick::ZERO,
            speed: 1.0,
            reverse: false,
            enabled: true,
            link: None,
            group: None,
            effects: Vec::new(),
            markers: Vec::new(),
            gain_db: 0.0,
            frame_hold: None,
            scale_to_frame: false,
            essential: None,
            multicam: None,
            time_interpolation: Default::default(),
            hold_filters: false,
            field_options: None,
            source_channels: Vec::new(),
            audio_stream: 0,
            graphic: None,
        }
    }
    fn harness(clip: &mut TrackItem) {
        clip.identity = Some(Box::new(ClipIdentity {
            id: format!("anchor{}", clip.id.0),
            parent: None,
            protection: Some("harness".into()),
            annotations: BTreeMap::new(),
            handoff_spans: Vec::new(),
        }));
    }
    fn sequence() -> Sequence {
        let mut project = Project::new("Extent fixture");
        let id = project.new_sequence("q", SequenceSettings::default(), 2, 1, None);
        project.sequence(id).unwrap().clone()
    }
    #[test]
    fn native_defaults_and_poema_extents_remain_distinct_serde_state() {
        let second = Tick(TICKS_PER_SECOND);
        let mut q = sequence();
        q.video_tracks[0].items.push(clip(11, Tick::ZERO, Tick(3 * TICKS_PER_SECOND)));
        q.video_tracks[1].items.push(clip(12, Tick(4 * TICKS_PER_SECOND), Tick(3 * TICKS_PER_SECOND)));
        q.audio_tracks[0].items.push(clip(13, Tick(4 * TICKS_PER_SECOND), Tick(2 * TICKS_PER_SECOND)));
        let mut captions = CaptionTrack::new(TrackId(50), "captions".into(), CaptionFormat::Subtitle);
        captions.captions.push(Caption {
            id: ClipId(51),
            start: Tick(7 * TICKS_PER_SECOND),
            duration: second,
            text: "fixture".into(),
            speaker: None,
            cue_id: None,
            settings: String::new(),
        });
        q.caption_tracks.push(captions);
        let old = serde_json::to_value(&q).unwrap();
        assert!(old.get("program_extent").is_none());
        assert_eq!(q.duration(), Tick(8 * TICKS_PER_SECOND));
        let restored: Sequence = serde_json::from_value(old.clone()).unwrap();
        assert_eq!(serde_json::to_value(restored).unwrap(), old);
        q.program_extent = Some(ProgramExtentPolicy::Poema);
        assert_eq!(q.duration(), Tick(3 * TICKS_PER_SECOND));
        q.video_tracks[0].enabled = false;
        q.video_tracks[0].items[0].enabled = false;
        q.audio_tracks[0].muted = true;
        assert_eq!(q.duration(), Tick(3 * TICKS_PER_SECOND));
        q.video_tracks[0].items.clear();
        assert_eq!(q.duration(), Tick(6 * TICKS_PER_SECOND));
        q.audio_tracks[0].items.clear();
        assert_eq!(q.duration(), Tick::ZERO); // overlay/captions do not invent a program.
        q.video_tracks[0].items.push(clip(14, Tick::ZERO, second));
        q.audio_tracks[0].items.push(clip(15, Tick(4 * TICKS_PER_SECOND), Tick(2 * TICKS_PER_SECOND)));
        harness(&mut q.audio_tracks[0].items[0]);
        assert_eq!(q.duration(), Tick(6 * TICKS_PER_SECOND));
        let saved = serde_json::to_value(&q).unwrap();
        assert_eq!(saved["program_extent"], "poema");
        let restored: Sequence = serde_json::from_value(saved.clone()).unwrap();
        assert_eq!(serde_json::to_value(restored).unwrap(), saved);
        let mut invalid = saved;
        invalid["program_extent"] = serde_json::json!("other");
        assert!(std::panic::catch_unwind(|| serde_json::from_value::<Sequence>(invalid)).unwrap().is_err());
    }
    #[test]
    fn rational_harness_and_picture_ends_follow_actual_native_items() {
        let mut q = sequence();
        q.settings.frame_rate = FrameRate::new(24000, 1001);
        let frame = q.settings.frame_rate.frame_duration();
        q.program_extent = Some(ProgramExtentPolicy::Poema);
        q.video_tracks[0].items.push(clip(1, Tick::ZERO, Tick(frame.0 * 72)));
        q.audio_tracks[0].items.push(clip(2, Tick(frame.0 * 36), Tick(frame.0 * 48)));
        assert_eq!(q.duration(), Tick(frame.0 * 72));
        harness(&mut q.audio_tracks[0].items[0]);
        assert_eq!(q.duration(), Tick(frame.0 * 84));
        q.audio_tracks[0].items[0].duration = Tick(frame.0 * 24);
        assert_eq!(q.duration(), Tick(frame.0 * 72));
        q.video_tracks[0].items[0].duration = Tick(frame.0 * 48);
        assert_eq!(q.duration(), Tick(frame.0 * 60));
        q.audio_tracks[0].items[0].identity.as_mut().unwrap().protection = Some("human".into());
        assert_eq!(q.duration(), Tick(frame.0 * 48));
    }
    #[test]
    fn extreme_ticks_and_missing_spine_do_not_panic() {
        let mut q = sequence();
        q.program_extent = Some(ProgramExtentPolicy::Poema);
        q.video_tracks[0].items.push(clip(1, Tick(i64::MAX - 5), Tick(20)));
        assert_eq!(std::panic::catch_unwind(|| q.duration()).unwrap(), Tick(i64::MAX));
        q.video_tracks.clear();
        q.audio_tracks[0].items.push(clip(2, Tick(i64::MAX - 5), Tick(20)));
        assert_eq!(std::panic::catch_unwind(|| q.duration()).unwrap(), Tick(i64::MAX));
        q.audio_tracks.clear();
        assert_eq!(q.duration(), Tick::ZERO);
    }
    #[test]
    fn content_extent_retains_every_stored_tail_without_mutating_project_state() {
        let mut q = sequence();
        q.settings.frame_rate = FrameRate::new(24000, 1001);
        let frame = q.frame_rate().frame_duration();
        q.video_tracks[0].items.push(clip(61, Tick::ZERO, Tick(frame.0 * 72)));
        q.audio_tracks[0].items.push(clip(62, Tick(frame.0 * 21600), Tick(frame.0 * 48)));
        q.video_tracks[1].items.push(clip(63, Tick(frame.0 * 22000), frame));
        let mut captions = CaptionTrack::new(TrackId(64), "tail captions".into(), CaptionFormat::Subtitle);
        captions.captions.push(Caption {
            id: ClipId(65),
            start: Tick(frame.0 * 23000),
            duration: frame,
            text: "synthetic".into(),
            speaker: None,
            cue_id: None,
            settings: String::new(),
        });
        q.caption_tracks.push(captions);
        let expected = Tick(frame.0 * 23001);
        assert_eq!(q.duration(), expected);
        assert_eq!(q.content_duration(), expected);
        q.program_extent = Some(ProgramExtentPolicy::Poema);
        q.video_tracks[1].enabled = false;
        q.audio_tracks[0].muted = true;
        let before = q.clone();
        assert_eq!(q.duration(), Tick(frame.0 * 72));
        assert_eq!(q.content_duration(), expected);
        assert_eq!(q, before);
        let restored: Sequence = serde_json::from_value(serde_json::to_value(&q).unwrap()).unwrap();
        assert_eq!(restored.content_duration(), expected);
        assert_eq!(restored.duration(), Tick(frame.0 * 72));
        q.caption_tracks.clear();
        assert_eq!(q.content_duration(), Tick(frame.0 * 22001));
        q.video_tracks[1].items.clear();
        assert_eq!(q.content_duration(), Tick(frame.0 * 21648));
        q.audio_tracks[0].items.clear();
        assert_eq!(q.content_duration(), Tick(frame.0 * 72));
        q.video_tracks[0].items.clear();
        assert_eq!(q.content_duration(), Tick::ZERO);
        assert_eq!(q.duration(), Tick::ZERO);
        q.audio_tracks[0].items.push(clip(66, Tick(i64::MAX - 5), Tick(20)));
        assert_eq!(std::panic::catch_unwind(|| q.content_duration()).unwrap(), Tick(i64::MAX));
    }
}
