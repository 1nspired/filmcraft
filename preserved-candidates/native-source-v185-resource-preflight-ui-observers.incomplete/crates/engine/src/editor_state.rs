//! Original editor-reference repair shared by Session and pure typed guards.
//! No history publication, file I/O, media cache, RPC or alternate editor owner.
use crate::EditorState;
use filmcraft_project::Project;

/// Original mask identity remap for both ordinary and merged commits.
pub(crate) fn remap_committed_mask(before: &Project, after: &Project, previous: &EditorState, next: &mut EditorState) {
    if next.active_sequence == previous.active_sequence && next.selected_mask == previous.selected_mask {
        next.selected_mask = crate::masks::remap_selection(before, after, previous);
    }
}

/// Source-monitor MRU normalization, with the original duplicate/limit policy.
pub(crate) fn track_source_history(project: &Project, state: &mut EditorState) {
    let current = state.source_item;
    let history = &mut state.source_history;
    history.retain(|i| project.item(*i).is_some());
    if let Some(cur) = current.filter(|i| project.item(*i).is_some())
        && history.first() != Some(&cur)
    {
        history.retain(|i| *i != cur);
        history.insert(0, cur);
    }
    history.truncate(crate::source_monitor::SOURCE_HISTORY_MAX);
}

/// Keep the original ordering: validate mask/effect in the old active view,
/// then choose a surviving sequence and remove dangling source/selection refs.
pub(crate) fn repair(project: &Project, state: &mut EditorState) {
    if let Some(m) = state.selected_mask {
        let ok = state
            .active_sequence
            .and_then(|id| project.sequence(id))
            .and_then(|q| q.find_item(m.clip))
            .and_then(|(_, it)| it.effects.get(m.effect))
            .is_some_and(|e| m.mask < e.masks.len());
        if !ok {
            state.selected_mask = None;
        }
    }
    if let Some(e) = &state.selected_effect
        && state.active_sequence.and_then(|id| project.sequence(id)).and_then(|q| q.find_item(e.clip)).and_then(|(_, it)| e.index(&it.effects)).is_none()
    {
        state.selected_effect = None;
    }
    if let Some(s) = state.active_sequence
        && project.sequence(s).is_none()
    {
        state.active_sequence = project.sequences().next().map(|i| i.id);
    }
    state.open_sequences.retain(|s| project.sequence(*s).is_some());
    state.timeline_views.retain(|s, _| project.sequence(*s).is_some());
    if let Some(s) = state.source_item
        && project.item(s).is_none()
    {
        state.source_item = None;
    }
    track_source_history(project, state);
    if let Some(seq) = state.active_sequence.and_then(|s| project.sequence(s)) {
        state.selection.retain(|c| seq.find_item(*c).is_some());
        state.caption_selection.retain(|c| seq.find_caption(*c).is_some());
    } else {
        state.selection.clear();
        state.caption_selection.clear();
    }
    state.project_selection.retain(|i| project.item(*i).is_some());
    drop_stale_gap(project, state);
}

pub(crate) fn drop_stale_gap(project: &Project, state: &mut EditorState) {
    if let Some(g) = state.gap_selection
        && !state
            .active_sequence
            .and_then(|id| project.sequence(id))
            .and_then(|q| q.track(g.track))
            .is_some_and(|tr| filmcraft_edit::through::track_gaps(tr).contains(&g.range))
    {
        state.gap_selection = None;
    }
}
