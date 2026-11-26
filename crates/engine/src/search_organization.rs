//! Shared saved-search authoring and bounded project-metadata matching.
//! Human commands and guarded integrations use this service, never a second query engine.
use crate::{Result, commands::bad};
use filmcraft_project::{Bin, BinEntry, BinId, FindQuery, ItemId, Project, SearchBin};
use serde_json::Value;
use std::collections::BTreeSet;
const COMMAND: &str = "project.searchBins";
const MAX_ENTRIES: usize = 100_000;
fn text(s: &str, maximum: usize) -> bool {
    s.len() <= maximum && !s.contains('\0')
}
fn valid_name(s: &str) -> Result<()> {
    if !text(s, 1024) || s.trim().is_empty() {
        return Err(bad(COMMAND, "a nonempty search-bin name of at most 1024 UTF-8 bytes without NUL is required"));
    }
    Ok(())
}
pub(crate) fn validate_query(q: &FindQuery) -> Result<()> {
    if q.rows.len() > 32 || q.rows.iter().any(|r| !text(&r.column, 256) || !text(&r.text, 4096)) {
        return Err(bad(COMMAND, "at most 32 query rows, 256-byte columns and 4096-byte text without NUL are supported"));
    }
    Ok(())
}
/// Validate raw human parameters before reusing the existing Find query parser.
/// Explicit malformed options never become defaults or an accidentally broader query.
pub(crate) fn query_params(p: &Value, command: &str) -> Result<FindQuery> {
    let row = |v: &Value| -> Result<()> {
        let o = v.as_object().ok_or_else(|| bad(command, "each search row must be an object"))?;
        for (key, maximum) in [("column", 256), ("operator", 256), ("op", 256), ("text", 4096)] {
            if let Some(v) = o.get(key)
                && !v.as_str().is_some_and(|s| text(s, maximum))
            {
                return Err(bad(command, "search row fields must be bounded text without NUL"));
            }
        }
        if let (Some(a), Some(b)) = (o.get("operator"), o.get("op"))
            && filmcraft_project::FindOp::from_name(a.as_str().unwrap_or_default()) != filmcraft_project::FindOp::from_name(b.as_str().unwrap_or_default())
        {
            return Err(bad(command, "conflicting search operator aliases"));
        }
        Ok(())
    };
    if !p.is_object() {
        return Err(bad(command, "search parameters must be an object"));
    }
    for key in ["matchAll", "caseSensitive"] {
        if p.get(key).is_some_and(|v| !v.is_boolean()) {
            return Err(bad(command, "search match flags must be booleans"));
        }
    }
    if let Some(rows) = p.get("rows") {
        let rows = rows.as_array().ok_or_else(|| bad(command, "rows must be an explicit list"))?;
        if rows.len() > 32 {
            return Err(bad(command, "at most 32 search rows are supported"));
        }
        if ["column", "operator", "op", "text"].iter().any(|key| p.get(*key).is_some()) {
            return Err(bad(command, "use rows or one legacy row, not both"));
        }
        for r in rows {
            row(r)?;
            if r.as_object().is_some_and(|o| o.keys().any(|k| !["column", "operator", "op", "text"].contains(&k.as_str()))) {
                return Err(bad(command, "unknown search row field"));
            }
        }
    } else {
        row(p)?;
    }
    let q = crate::project_tools::query_p(p, command)?;
    validate_query(&q)?;
    Ok(q)
}
/// Bound the traversal without recursive all_items or a quadratic orphan scan.
/// The order and matcher remain FindQuery's human Project-panel semantics.
fn ordered_items(project: &Project) -> Result<Vec<ItemId>> {
    if project.items.len().saturating_add(project.search_bins.len()) >= MAX_ENTRIES {
        return Err(bad(COMMAND, "project search inventory exceeds its bound"));
    }
    let mut ids = BTreeSet::new();
    for b in &project.search_bins {
        if b.id.0 == 0 || !ids.insert(b.id.0) {
            return Err(bad(COMMAND, "ambiguous saved search identity"));
        }
    }
    for (id, item) in &project.items {
        if id.0 == 0 || *id != item.id || !ids.insert(id.0) {
            return Err(bad(COMMAND, "ambiguous project item identity"));
        }
    }
    enum Entry<'a> {
        Bin(&'a Bin, usize),
        Item(ItemId),
    }
    let mut pending = vec![Entry::Bin(&project.root, 0)];
    let mut order = Vec::new();
    let mut seen = BTreeSet::new();
    let mut visited = project.items.len().saturating_add(project.search_bins.len());
    while let Some(entry) = pending.pop() {
        visited = visited.saturating_add(1);
        if visited.saturating_add(pending.len()) > MAX_ENTRIES {
            return Err(bad(COMMAND, "project search tree exceeds its entry bound"));
        }
        match entry {
            Entry::Bin(bin, depth) => {
                if depth > 64 || (bin.id.0 == 0 && depth != 0) || !ids.insert(bin.id.0) {
                    return Err(bad(COMMAND, "invalid, deep or ambiguous physical bin tree"));
                }
                if visited.saturating_add(pending.len()).saturating_add(bin.children.len()) > MAX_ENTRIES {
                    return Err(bad(COMMAND, "project search queue exceeds its bound"));
                }
                for child in bin.children.iter().rev() {
                    pending.push(match child {
                        BinEntry::Bin(b) => Entry::Bin(b, depth + 1),
                        BinEntry::Item(id) => Entry::Item(*id),
                    });
                }
            }
            Entry::Item(id) => {
                if !project.items.contains_key(&id) || !seen.insert(id) {
                    return Err(bad(COMMAND, "missing or repeated physical project item"));
                }
                order.push(id);
            }
        }
    }
    for id in project.items.keys() {
        if seen.insert(*id) {
            order.push(*id);
        }
    }
    Ok(order)
}
pub(crate) fn validate_inventory(project: &Project) -> Result<()> {
    ordered_items(project).map(|_| ())
}
pub(crate) fn results(project: &Project, q: &FindQuery) -> Result<Vec<ItemId>> {
    validate_query(q)?;
    Ok(ordered_items(project)?.into_iter().filter(|id| project.item(*id).is_some_and(|item| q.matches(project, item))).collect())
}
fn index(project: &Project, id: BinId) -> Result<usize> {
    ordered_items(project)?;
    project.search_bins.iter().position(|b| b.id == id).ok_or_else(|| bad(COMMAND, "no such saved search"))
}
pub(crate) fn get(project: &Project, id: BinId) -> Result<SearchBin> {
    let i = index(project, id)?;
    let b = project.search_bins.get(i).ok_or_else(|| bad(COMMAND, "saved search changed"))?;
    if !text(&b.name, 1024) {
        return Err(bad(COMMAND, "saved search name exceeds supported bounds"));
    }
    validate_query(&b.query)?;
    Ok(b.clone())
}
pub(crate) fn create(project: &mut Project, name: Option<&str>, query: FindQuery) -> Result<SearchBin> {
    validate_query(&query)?;
    if query.is_empty() {
        return Err(bad(COMMAND, "enter text to search for"));
    }
    let name = name
        .filter(|name| !name.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| query.rows.iter().find(|r| !r.text.is_empty()).map(|r| r.text.clone()).unwrap_or_default());
    valid_name(&name)?;
    ordered_items(project)?;
    let id = crate::project_ids::allocation_start(project, 1)?;
    let next = id.checked_add(1).ok_or_else(|| bad(COMMAND, "project ID space exhausted"))?;
    let bin = SearchBin { id: BinId(id), name, query };
    project.next_id = next;
    project.search_bins.push(bin.clone());
    Ok(bin)
}
pub(crate) fn update(project: &mut Project, id: BinId, name: Option<&str>, query: Option<FindQuery>) -> Result<SearchBin> {
    let i = index(project, id)?;
    let previous = project.search_bins.get(i).ok_or_else(|| bad(COMMAND, "saved search changed"))?;
    valid_name(name.unwrap_or(&previous.name))?;
    validate_query(query.as_ref().unwrap_or(&previous.query))?;
    let b = project.search_bins.get_mut(i).ok_or_else(|| bad(COMMAND, "saved search changed"))?;
    if let Some(n) = name {
        b.name = n.into();
    }
    if let Some(q) = query {
        b.query = q;
    }
    Ok(b.clone())
}
pub(crate) fn delete(project: &mut Project, id: BinId) -> Result<()> {
    let i = index(project, id)?;
    project.search_bins.remove(i);
    Ok(())
}
