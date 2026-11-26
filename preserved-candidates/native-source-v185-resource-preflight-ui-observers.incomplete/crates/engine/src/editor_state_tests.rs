//! Frozen original repair versus shared pure repair, including nonserialized
//! clipboard payloads and original ordinary/merged commit behavior.
use crate::{EditorState, GapSelection, Session};
use filmcraft_project::{ClipId, ItemId, TrackKind};
use filmcraft_time::{Tick, TimeRange};
use serde_json::json;
use std::sync::Arc;

fn legacy_source_history(s: &mut Session) {
    let current = s.state.source_item;
    let project = &s.project;
    let history = &mut s.state.source_history;
    history.retain(|i| project.item(*i).is_some());
    if let Some(cur) = current.filter(|i| project.item(*i).is_some())
        && history.first() != Some(&cur)
    {
        history.retain(|i| *i != cur);
        history.insert(0, cur);
    }
    history.truncate(crate::source_monitor::SOURCE_HISTORY_MAX);
}
fn legacy_drop_gap(s: &mut Session) {
    if let Some(g) = s.state.gap_selection
        && !s.active_sequence().and_then(|q| q.track(g.track)).is_some_and(|tr| filmcraft_edit::through::track_gaps(tr).contains(&g.range))
    {
        s.state.gap_selection = None;
    }
}
fn legacy_repair(session: &mut Session) {
    let p = session.project.clone();
    if let Some(m) = session.state.selected_mask {
        let ok =
            session.active_sequence().and_then(|q| q.find_item(m.clip)).and_then(|(_, it)| it.effects.get(m.effect)).is_some_and(|e| m.mask < e.masks.len());
        if !ok {
            session.state.selected_mask = None;
        }
    }
    if let Some(e) = &session.state.selected_effect
        && session.active_sequence().and_then(|q| q.find_item(e.clip)).and_then(|(_, it)| e.index(&it.effects)).is_none()
    {
        session.state.selected_effect = None;
    }
    if let Some(s) = session.state.active_sequence
        && p.sequence(s).is_none()
    {
        session.state.active_sequence = p.sequences().next().map(|i| i.id);
    }
    session.state.open_sequences.retain(|s| p.sequence(*s).is_some());
    session.state.timeline_views.retain(|s, _| p.sequence(*s).is_some());
    if let Some(s) = session.state.source_item
        && p.item(s).is_none()
    {
        session.state.source_item = None;
    }
    legacy_source_history(session);
    if let Some(seq) = session.state.active_sequence.and_then(|s| p.sequence(s)) {
        session.state.selection.retain(|c| seq.find_item(*c).is_some());
        session.state.caption_selection.retain(|c| seq.find_caption(*c).is_some());
    } else {
        session.state.selection.clear();
        session.state.caption_selection.clear();
    }
    session.state.project_selection.retain(|i| p.item(*i).is_some());
    legacy_drop_gap(session);
}
fn demo() -> Session {
    let mut s = Session::default();
    s.execute("file.openDemoProject", json!({})).unwrap();
    s
}
fn same_editor(a: &EditorState, b: &EditorState) {
    assert_eq!(serde_json::to_value(a).unwrap(), serde_json::to_value(b).unwrap());
    // Clipboard is deliberately skipped by EditorState serde; compare it too.
    assert_eq!(a.clipboard, b.clipboard);
}

#[test]
fn editor_repair_matches_frozen_original_for_64_dangling_view_source_mask_and_selection_states() {
    for seed in 0..64usize {
        let mut s = demo();
        let sequences = s.project.sequences().map(|i| i.id).collect::<Vec<_>>();
        let media = s.project.items.values().filter(|i| i.as_media().is_some()).map(|i| i.id).collect::<Vec<_>>();
        let sequence = *sequences.first().unwrap();
        let track = s.project.sequence(sequence).unwrap().video_tracks.first().unwrap();
        let clip = track.items.first().unwrap().clone();
        let track_id = track.id;
        s.state.clipboard = vec![(TrackKind::Video, 2, clip.clone())];
        s.state.selection = vec![clip.id, ClipId(999_999)];
        s.state.caption_selection = vec![ClipId(999_998)];
        s.state.project_selection = media.iter().copied().chain([ItemId(999_997)]).collect();
        s.state.open_sequences = sequences.iter().copied().chain(sequences.iter().copied()).chain([ItemId(999_996)]).collect();
        for id in &s.state.open_sequences {
            s.state.timeline_views.insert(
                *id,
                filmcraft_project::SequenceView { pps: 50.0, scroll: 0.0, v_scroll: 12.0, a_scroll: 7.0, video_track_h: 90.0, audio_track_h: 40.0 },
            );
            s.state.playheads.insert(*id, Tick(17));
        }
        s.state.source_history = (0..27).map(|i| if i % 5 == 0 { ItemId(999_995) } else { media[i % media.len()] }).collect();
        s.state.source_item = match seed % 3 {
            0 => None,
            1 => Some(media[0]),
            _ => Some(ItemId(999_994)),
        };
        s.state.active_sequence = match seed % 4 {
            0 => None,
            1 => Some(sequence),
            2 => Some(*sequences.last().unwrap()),
            _ => Some(ItemId(999_993)),
        };
        s.state.selected_mask = Some(crate::masks::MaskSel { clip: clip.id, effect: 0, mask: seed % 3 });
        s.state.selected_effect = crate::EffectSel::of(clip.id, &clip.effects, 0);
        s.state.gap_selection = Some(GapSelection { track: track_id, range: TimeRange::new(Tick::ZERO, Tick(1)) });
        let project = Arc::make_mut(&mut s.project);
        if seed % 2 == 0 {
            project.items.remove(&sequence);
            project.root.remove_item(sequence);
        }
        if seed % 3 == 0 {
            project.items.remove(&media[0]);
            project.root.remove_item(media[0]);
        }
        let before = s.project.clone();
        let revision = s.revision;
        let history = s.history.undo.len();
        let journal = s.journal.clone();
        let mut legacy = Session::default();
        legacy.project = s.project.clone();
        legacy.state = s.state.clone();
        legacy_repair(&mut legacy);
        let mut predicted = s.state.clone();
        crate::editor_state::repair(&s.project, &mut predicted);
        same_editor(&predicted, &legacy.state);
        assert!(Arc::ptr_eq(&s.project, &before));
        assert_eq!(s.revision, revision);
        assert_eq!(s.history.undo.len(), history);
        assert_eq!(s.journal, journal);
        s.fix_state();
        same_editor(&s.state, &legacy.state);
        let once = s.state.clone();
        s.fix_state();
        same_editor(&s.state, &once);
    }
}

#[test]
fn pure_clear_project_and_shared_editor_prediction_match_the_actual_human_command_with_clipboard_and_sequence_absence() {
    for mode in 0..6 {
        let mut s = demo();
        let sequence = s.state.active_sequence.unwrap();
        let clip = s.active_sequence().unwrap().video_tracks[0].items[0].clone();
        let media = clip.item;
        s.state.clipboard = vec![(TrackKind::Video, 1, clip.clone())];
        s.state.source_item = Some(media);
        s.state.source_history = vec![media];
        s.state.project_selection = vec![media];
        s.state.selection = vec![clip.id];
        s.execute("masks.add", json!({"clip":clip.id.0,"effect":"opacity","shape":"ellipse"})).unwrap();
        let sequences = s.project.sequences().map(|i| i.id).collect::<Vec<_>>();
        let bin = s.execute("file.newBin", json!({"name":"Clear editor fixture"})).unwrap()["bin"].as_u64().unwrap();
        s.execute("project.moveToBin", json!({"items":[media.0],"bin":bin})).unwrap();
        let asked = match mode {
            0 => vec![media],
            1 => vec![sequence],
            2 => sequences,
            3 => vec![ItemId(bin)],
            4 => vec![sequence, media],
            _ => vec![ItemId(s.project.root.id.0)],
        };
        let before = s.project.clone();
        let state = s.state.clone();
        let revision = s.revision;
        let history = s.history.undo.len();
        let (mut predicted, _) = crate::project_clear::predict(&s.project, &asked).unwrap();
        crate::clip_identity::reconcile(&s.project, &mut predicted).unwrap();
        let mut editor = state.clone();
        editor.project_selection.clear();
        crate::editor_state::remap_committed_mask(&s.project, &predicted, &state, &mut editor);
        crate::editor_state::drop_stale_gap(&predicted, &mut editor);
        crate::editor_state::repair(&predicted, &mut editor);
        assert!(Arc::ptr_eq(&s.project, &before));
        same_editor(&s.state, &state);
        assert_eq!(s.revision, revision);
        assert_eq!(s.history.undo.len(), history);
        s.execute("project.delete", json!({"items":asked.iter().map(|id|id.0).collect::<Vec<_>>()})).unwrap();
        assert_eq!(s.project.to_json(), predicted.to_json());
        same_editor(&s.state, &editor);
        assert_eq!(s.history.undo.len(), history + 1);
        if mode == 2 {
            assert!(s.state.active_sequence.is_none());
            assert!(s.state.open_sequences.is_empty());
        }
    }
}

#[test]
fn shared_mask_remap_is_used_by_both_ordinary_and_merged_human_commits_without_an_extra_history_step() {
    let mut s = demo();
    let clip = s.active_sequence().unwrap().video_tracks[0].items[0].id;
    s.execute("masks.add", json!({"clip":clip.0,"effect":"opacity","shape":"ellipse"})).unwrap();
    let selected = s.state.selected_mask.unwrap();
    let history = s.history.undo.len();
    s.edit_merged("Shared mask fixture", "shared-mask-editor-key", |project, _| {
        project.name = "First shared gesture".into();
        Ok(())
    })
    .unwrap();
    assert_eq!(s.state.selected_mask, Some(selected));
    assert_eq!(s.history.undo.len(), history + 1);
    s.edit_merged("Shared mask fixture", "shared-mask-editor-key", |project, _| {
        for item in project.items.values_mut() {
            if let Some(sequence) = item.as_sequence_mut() {
                for track in sequence.all_tracks_mut() {
                    for item in &mut track.items {
                        if item.id == clip {
                            for effect in &mut item.effects {
                                effect.masks.clear();
                            }
                        }
                    }
                }
            }
        }
        Ok(())
    })
    .unwrap();
    assert!(s.state.selected_mask.is_none());
    assert_eq!(s.history.undo.len(), history + 1);
    assert_eq!(s.history.undo.last().unwrap().0, "Shared mask fixture");
}
