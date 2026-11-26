//! Shared Project-panel Clear: a bounded pure project prediction followed by
//! the original single Session edit, cache eviction, relink and view repair.
//! No file deletion, source qualification, alternate history or model executor.
use std::collections::{BTreeMap, BTreeSet};

use filmcraft_project::{Bin, BinEntry, BinId, ItemId, ItemKind, MediaRef, Project};
use serde_json::{Value, json};

use crate::{Result, Session, commands::bad};

const COMMAND: &str = "project.delete";
const MAX_ENTRIES: usize = 100_000;
const MAX_DEPTH: usize = 64;

/// Complete affected sets, in the original human order. Top-level duplicate
/// bin requests retain the human receipt count; expanded items are deduplicated.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ClearEffects {
    pub bins: Vec<BinId>,
    pub removed: Vec<ItemId>,
    pub offlined: Vec<ItemId>,
    pub retained_subclips: Vec<ItemId>,
}
impl ClearEffects {
    fn human_result(&self) -> Value {
        json!({"items":self.removed.len(),"bins":self.bins.len(),"offline":self.offlined.len()})
    }
    fn apply(&self, project: &mut Project) {
        for id in &self.offlined {
            if let Some(media) = project.item_mut(*id).and_then(filmcraft_project::ProjectItem::as_media_mut) {
                media.offline = true;
            }
        }
        let removed = self.removed.iter().copied().collect::<BTreeSet<_>>();
        let bins = self.bins.iter().copied().collect::<BTreeSet<_>>();
        let mut present = BTreeSet::new();
        // inventory bounds recursion and rejects ambiguous physical IDs before
        // this linear pruning pass. Never search the whole tree per target.
        prune(&mut project.root, &bins, &removed, &mut present);
        for id in self.offlined.iter().chain(&self.retained_subclips) {
            if present.insert(*id) {
                project.root.children.push(BinEntry::Item(*id));
            }
        }
        for id in &self.removed {
            project.items.remove(id);
        }
        let sequences = project.sequences().map(|item| item.id).collect::<Vec<_>>();
        for id in sequences {
            if let Some(sequence) = project.sequence_mut(id) {
                for track in sequence.all_tracks_mut() {
                    track.items.retain(|clip| !removed.contains(&clip.item));
                    filmcraft_edit::remove_orphan_transitions(track);
                }
            }
        }
    }
}

fn prune(bin: &mut Bin, bins: &BTreeSet<BinId>, removed: &BTreeSet<ItemId>, present: &mut BTreeSet<ItemId>) {
    bin.children.retain_mut(|entry| match entry {
        BinEntry::Bin(child) if bins.contains(&child.id) => false,
        BinEntry::Bin(child) => {
            prune(child, bins, removed, present);
            true
        }
        BinEntry::Item(id) => {
            if removed.contains(id) {
                false
            } else {
                present.insert(*id);
                true
            }
        }
    });
}

// Validate before any recursive tree helper is used. A malformed/deep tree or
// oversized selection must refuse atomically, including when called directly.
fn inventory<'a>(project: &'a Project, asked: &[ItemId]) -> Result<BTreeMap<BinId, (&'a Bin, Option<BinId>)>> {
    if project.items.len() > MAX_ENTRIES || asked.len() > MAX_ENTRIES {
        return Err(bad(COMMAND, "project or Clear selection exceeds its supported entry bound"));
    }
    let mut bins = BTreeMap::new();
    let mut physical = BTreeSet::new();
    let mut pending = vec![(&project.root, None, 0usize)];
    let mut count = 0usize;
    while let Some((bin, parent, depth)) = pending.pop() {
        count = count.saturating_add(1);
        if depth > MAX_DEPTH
            || count > MAX_ENTRIES
            || bin.children.len() > MAX_ENTRIES
            || bin.id.0 == 0 && parent.is_some()
            || !physical.insert(bin.id.0)
            || bins.insert(bin.id, (bin, parent)).is_some()
        {
            return Err(bad(COMMAND, "Clear requires a bounded unambiguous project bin tree"));
        }
        for child in &bin.children {
            match child {
                BinEntry::Bin(child) => pending.push((child, Some(bin.id), depth.saturating_add(1))),
                BinEntry::Item(id) => {
                    if id.0 == 0 || !physical.insert(id.0) {
                        return Err(bad(COMMAND, "Clear requires unambiguous physical item identities"));
                    }
                    count = count.saturating_add(1);
                }
            }
            if count.saturating_add(pending.len()) > MAX_ENTRIES {
                return Err(bad(COMMAND, "Clear project tree exceeds its supported entry bound"));
            }
        }
    }
    Ok(bins)
}

/// Plan complete Project effects, including offline-only declaration changes.
/// No Session, media pool, source bytes, filesystem or history is touched here.
fn plan(project: &Project, asked: &[ItemId]) -> Result<ClearEffects> {
    let tree = inventory(project, asked)?;
    let named = asked.iter().map(|id| BinId(id.0)).filter(|id| *id != project.root.id && tree.contains_key(id)).collect::<Vec<_>>();
    let selected = named.iter().copied().collect::<BTreeSet<_>>();
    let bins = named
        .iter()
        .copied()
        .filter(|id| {
            let mut parent = tree.get(id).and_then(|(_, parent)| *parent);
            for _ in 0..=MAX_DEPTH {
                let Some(id) = parent else { return true };
                if selected.contains(&id) {
                    return false;
                }
                parent = tree.get(&id).and_then(|(_, parent)| *parent);
            }
            false // inventory already rejects a tree beyond this bound.
        })
        .collect::<Vec<_>>();
    // Preserve human skip/count semantics for explicit unknown IDs and root.
    // A future exact typed adapter must independently reject unread targets.
    let mut items = asked.iter().copied().filter(|id| !selected.contains(&BinId(id.0))).collect::<BTreeSet<_>>();
    for id in bins.iter().copied().collect::<BTreeSet<_>>() {
        if let Some((bin, _)) = tree.get(&id) {
            let mut expanded = Vec::new();
            bin.all_items(&mut expanded);
            items.extend(expanded);
        }
    }
    let mut used = BTreeSet::new();
    let mut count = 0usize;
    for item in project.sequences().filter(|item| !items.contains(&item.id)) {
        if let Some(sequence) = item.as_sequence() {
            for track in sequence.all_tracks() {
                count = count.saturating_add(1).saturating_add(track.items.len());
                if count > MAX_ENTRIES {
                    return Err(bad(COMMAND, "Clear sequence dependency inventory exceeds its supported entry bound"));
                }
                for clip in track
                    .items
                    .iter()
                    .filter(|clip| !items.contains(&clip.item) || project.item(clip.item).is_some_and(|item| matches!(item.kind, ItemKind::Media(_))))
                {
                    if !project.resolve_media(clip.item).is_some_and(|(_, media, _)| matches!(media.media, MediaRef::File { .. })) {
                        continue;
                    }
                    let mut id = clip.item;
                    for _ in 0..16 {
                        used.insert(id);
                        match project.item(id).map(|item| &item.kind) {
                            Some(ItemKind::Subclip { parent, .. }) => id = *parent,
                            _ => break,
                        }
                    }
                }
            }
        }
    }
    let mut effects = ClearEffects { bins, removed: Vec::new(), offlined: Vec::new(), retained_subclips: Vec::new() };
    for id in items {
        let keep = used.contains(&id)
            && project.item(id).and_then(filmcraft_project::ProjectItem::as_media).is_some_and(|media| matches!(media.media, MediaRef::File { .. }));
        if keep {
            effects.offlined.push(id);
        } else if used.contains(&id) && project.item(id).is_some_and(|item| matches!(item.kind, ItemKind::Subclip { .. })) {
            effects.retained_subclips.push(id);
        } else {
            effects.removed.push(id);
        }
    }
    Ok(effects)
}

/// Pure complete-project prediction for the same human Clear plan.
pub(crate) fn predict(project: &Project, asked: &[ItemId]) -> Result<(Project, ClearEffects)> {
    let effects = plan(project, asked)?;
    let mut candidate = project.clone();
    effects.apply(&mut candidate);
    crate::project_workspace::repair_cleared_anchor(project, &mut candidate)?;
    Ok((candidate, effects))
}

/// The same human command owns publication and all non-project side effects.
/// Pure predictions never evict caches, probe files, repair views or own Undo.
pub(crate) fn run(session: &mut Session, asked: &[ItemId]) -> Result<Value> {
    let effects = plan(&session.project, asked)?;
    let before = session.project.clone();
    session.edit("Clear", |project, state| {
        effects.apply(project);
        crate::project_workspace::repair_cleared_anchor(&before, project)?;
        state.project_selection.clear();
        Ok(())
    })?;
    for id in effects.removed.iter().chain(&effects.offlined) {
        session.media.remove(*id);
    }
    crate::relink::refresh(session);
    session.fix_state();
    Ok(effects.human_result())
}
