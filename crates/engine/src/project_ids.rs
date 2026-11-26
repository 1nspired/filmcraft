//! Bounded numeric allocation shared by native importing, marker and track services.
use crate::{Result, commands::bad};
use filmcraft_project::Project;

fn error(message: impl Into<String>) -> crate::EngineError {
    bad("project.ids", message)
}

/// Reserve above the existing numeric inventory without renumbering legacy IDs.
/// Older/demo documents contain fixed marker/transition IDs above next_id. Their
/// content is valid; advancing the counter for this successful edit prevents a
/// collision without treating those old IDs as an import failure.
pub(crate) fn allocation_start(project: &Project, needed: u64) -> Result<u64> {
    if project.next_id == 0 || needed == 0 {
        return Err(error("invalid project ID allocation"));
    }
    let mut visited = 0usize;
    let mut highest = project.next_id - 1;
    let mut check = |id: u64| {
        visited = visited.saturating_add(1);
        if visited > 100_000 {
            return Err(error("project ID inventory exceeds 100000 references"));
        }
        highest = highest.max(id);
        Ok(())
    };
    for id in crate::project_workspace::retired_anchor_ids(project)? {
        check(id)?;
    }
    let mut bins = vec![(&project.root, 0usize)];
    while let Some((bin, depth)) = bins.pop() {
        if depth > 64 {
            return Err(error("project bin depth exceeds supported project bounds"));
        }
        check(bin.id.0)?;
        for entry in &bin.children {
            match entry {
                filmcraft_project::BinEntry::Item(id) => check(id.0)?,
                filmcraft_project::BinEntry::Bin(child) => {
                    check(child.id.0)?;
                    bins.push((child, depth + 1));
                }
            }
        }
    }
    for bin in &project.search_bins {
        check(bin.id.0)?;
    }
    for (key, item) in &project.items {
        check(key.0)?;
        check(item.id.0)?;
        if let Some(media) = item.as_media() {
            for m in &media.markers {
                check(m.id.0)?;
            }
        }
        if let Some(q) = item.as_sequence() {
            for t in q.all_tracks().chain(&q.submix_tracks) {
                check(t.id.0)?;
                for c in &t.items {
                    check(c.id.0)?;
                    check(c.item.0)?;
                    if let Some(link) = c.link {
                        check(link)?;
                    }
                    if let Some(group) = c.group {
                        check(group)?;
                    }
                    for m in &c.markers {
                        check(m.id.0)?;
                    }
                }
                for tr in &t.transitions {
                    check(tr.id.0)?;
                    if let Some(id) = tr.from {
                        check(id.0)?;
                    }
                    if let Some(id) = tr.to {
                        check(id.0)?;
                    }
                }
            }
            for t in &q.caption_tracks {
                check(t.id.0)?;
                for c in &t.captions {
                    check(c.id.0)?;
                }
            }
            for m in &q.markers {
                check(m.id.0)?;
            }
        }
    }
    let start = highest.checked_add(1).ok_or_else(|| error("project ID space exhausted"))?;
    start.checked_add(needed).ok_or_else(|| error("project ID space exhausted"))?;
    Ok(start)
}
