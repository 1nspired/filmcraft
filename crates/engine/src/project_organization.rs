//! Shared native bin creation, rename and move services, reused by human commands.
//! No editor selection, RPC, model executor or independent project owner.
use std::collections::{BTreeMap, BTreeSet};

use crate::{Result, commands::bad};
use filmcraft_project::{BinEntry, BinId, ItemId, Project};

const MAX_BIN_DEPTH: usize = 64;
const MAX_TREE_ENTRIES: usize = 100_000;

fn inventory(project: &Project, command: &str) -> Result<BTreeMap<BinId, usize>> {
    let mut bins = BTreeMap::new();
    let mut pending = vec![(&project.root, 0usize)];
    let mut entries = 0usize;
    while let Some((bin, depth)) = pending.pop() {
        entries = entries.saturating_add(1);
        if entries > MAX_TREE_ENTRIES || depth > MAX_BIN_DEPTH {
            return Err(bad(command, "project bin tree exceeds supported depth or entry bounds"));
        }
        if (bin.id.0 == 0 && depth != 0) || bins.insert(bin.id, depth).is_some() {
            return Err(bad(command, "bin identity is invalid or duplicated"));
        }
        for child in &bin.children {
            match child {
                BinEntry::Bin(child) => pending.push((child, depth + 1)),
                BinEntry::Item(_) => entries = entries.saturating_add(1),
            }
            if entries.saturating_add(pending.len()) > MAX_TREE_ENTRIES {
                return Err(bad(command, "project bin tree exceeds supported entry bounds"));
            }
        }
    }
    Ok(bins)
}
fn name(value: &str, command: &str) -> Result<()> {
    if value.trim().is_empty() || value.len() > 1024 || value.contains('\0') {
        return Err(bad(command, "a nonempty bin name of at most 1024 UTF-8 bytes without NUL is required"));
    }
    Ok(())
}

/// Validate everything before mutation. Missing explicit parents never select root.
/// Native `None` deliberately means root. Existing names/IDs are never rewritten.
/// The caller publishes the candidate through its ordinary native transaction.
pub(crate) fn create(project: &mut Project, value: &str, parent: Option<BinId>) -> Result<BinId> {
    const COMMAND: &str = "file.newBin";
    name(value, COMMAND)?;
    let bins = inventory(project, COMMAND)?;
    let target = parent.unwrap_or(project.root.id);
    let depth = bins.get(&target).ok_or_else(|| bad(COMMAND, "no such parent bin"))?;
    if *depth >= MAX_BIN_DEPTH {
        return Err(bad(COMMAND, "creating a child would exceed the supported bin depth"));
    }
    // The existing allocator also bounds the complete project inventory and
    // reserves above legacy/imported IDs before add_bin can increment next_id.
    let first = crate::project_ids::allocation_start(project, 1)?;
    project.next_id = first;
    Ok(project.add_bin(value, Some(target)))
}

/// Match the human inline rename's trimming, without allocating any identity.
pub(crate) fn rename(project: &mut Project, bin: BinId, value: &str) -> Result<()> {
    const COMMAND: &str = "project.renameBin";
    let value = value.trim();
    name(value, COMMAND)?;
    let bins = inventory(project, COMMAND)?;
    if bin == project.root.id {
        return Err(bad(COMMAND, "the project's root bin is named after the project"));
    }
    if !bins.contains_key(&bin) {
        return Err(bad(COMMAND, "no such bin"));
    }
    project.root.find_bin_mut(bin).ok_or_else(|| bad(COMMAND, "no such bin"))?.name = value.into();
    Ok(())
}

/// Shared move semantics: the human command keeps its documented skip policy;
/// the guarded adapter requires every explicit entry to move exactly once.
/// Candidate-only work makes a rejected partial/cyclic/deep move atomic even
/// when this service is called directly rather than through Session::edit.
pub(crate) fn move_entries(project: &mut Project, entries: &[ItemId], parent: Option<BinId>, exact: bool) -> Result<usize> {
    const COMMAND: &str = "project.moveToBin";
    let bins = inventory(project, COMMAND)?;
    if entries.is_empty() || entries.len() > if exact { 4096 } else { MAX_TREE_ENTRIES } {
        return Err(bad(COMMAND, "a bounded nonempty explicit entry set is required"));
    }
    if !bins.contains_key(&parent.unwrap_or(project.root.id)) {
        return Err(bad(COMMAND, "no such bin"));
    }
    if exact && entries.iter().collect::<BTreeSet<_>>().len() != entries.len() {
        return Err(bad(COMMAND, "duplicate explicit move entries"));
    }
    let mut candidate = project.clone();
    let moved = candidate.move_to_bin(entries, parent).ok_or_else(|| bad(COMMAND, "no such bin"))?;
    if exact && moved != entries.len() {
        return Err(bad(COMMAND, "one or more explicit entries could not move; no partial move"));
    }
    inventory(&candidate, COMMAND)?;
    *project = candidate;
    Ok(moved)
}

/// The original New Bin From Selection gesture: containing bin of the first
/// item (or root if the first selection is a bin), a globally available default
/// name, then moves in selection order.
/// Work stays in a candidate so exact rejection cannot leave an empty new bin.
pub(crate) fn group_selection(project: &mut Project, entries: &[ItemId], value: Option<&str>, exact: bool) -> Result<(BinId, usize)> {
    const COMMAND: &str = "file.newBinFromSelection";
    inventory(project, COMMAND)?;
    let first = entries.first().ok_or_else(|| bad(COMMAND, "select items in the Project panel"))?;
    if entries.len() > if exact { 4096 } else { MAX_TREE_ENTRIES } {
        return Err(bad(COMMAND, "project selection exceeds its supported bound"));
    }
    let parent = project.root.parent_of(*first).filter(|id| *id != project.root.id);
    let value = match value {
        Some(value) => {
            name(value, COMMAND)?;
            value.to_string()
        }
        None => {
            let mut names = BTreeSet::new();
            let mut pending = vec![&project.root];
            // inventory already bounds this immutable tree's depth and size.
            while let Some(bin) = pending.pop() {
                for child in &bin.children {
                    if let BinEntry::Bin(child) = child {
                        names.insert(child.name.as_str());
                        pending.push(child);
                    }
                }
            }
            (1..=MAX_TREE_ENTRIES.saturating_add(1))
                .map(|number| format!("Bin {number:02}"))
                .find(|value| !names.contains(value.as_str()))
                .ok_or_else(|| bad(COMMAND, "no bounded default bin name is available"))?
        }
    };
    let mut candidate = project.clone();
    let bin = create(&mut candidate, &value, parent)?;
    let moved = move_entries(&mut candidate, entries, Some(bin), exact)?;
    *project = candidate;
    Ok((bin, moved))
}
