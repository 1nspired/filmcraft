//! Explicit dependency selection for file witnesses. Discovery metadata is not
//! file proof. Every scoped mutation checks both sides of its native transition.
use std::collections::{BTreeMap, BTreeSet};

use filmcraft_project::{ItemId, ItemKind, MediaRef, Project};
use serde::Deserialize;

use super::{BINDING_KEY, Binding, media_guard};
use crate::{Result, commands::bad};

const SAFE: u64 = 9_007_199_254_740_991;
const BUDGET: usize = 100_000;

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct Asset {
    project_id: String,
    asset_id: String,
}

#[derive(Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct Request {
    #[serde(default)]
    sequences: Vec<u64>,
    #[serde(default)]
    sources: Vec<u64>,
    #[serde(default)]
    assets: Vec<Asset>,
}

fn error(s: &str) -> crate::EngineError {
    bad("agent.mediaInputs", s)
}
fn ids(values: &[u64], limit: usize) -> Result<()> {
    if values.len() > limit || values.iter().any(|id| !(1..=SAFE).contains(id)) || values.iter().collect::<BTreeSet<_>>().len() != values.len() {
        return Err(error("source scope requires unique bounded positive IDs"));
    }
    Ok(())
}

/// Resolves native IDs and explicit bindings without resolving or reading paths.
/// The active sequence and open source are always dependencies of this read.
pub(super) fn resolve(project: &Project, active: ItemId, source: Option<ItemId>, request: &Request) -> Result<Vec<u64>> {
    ids(&request.sequences, 128)?;
    ids(&request.sources, 2048)?;
    if project.items.len() > BUDGET || request.assets.len() > 2048 {
        return Err(error("source scope exceeds its bounded inventory"));
    }
    if project.sequence(active).is_none() || request.sequences.iter().any(|id| project.sequence(ItemId(*id)).is_none()) {
        return Err(error("explicit source scope contains a missing sequence"));
    }
    if request.sources.iter().any(|id| project.item(ItemId(*id)).is_none_or(|item| item.as_media().is_none())) {
        return Err(error("explicit source scope contains a missing direct media item"));
    }
    let mut roots: BTreeSet<ItemId> = request.sequences.iter().chain(&request.sources).copied().map(ItemId).collect();
    roots.insert(active);
    if let Some(source) = source.filter(|id| monitor_dependency(project, *id)) {
        roots.insert(source);
    }
    let mut requested = BTreeSet::new();
    for asset in &request.assets {
        if [&asset.project_id, &asset.asset_id].iter().any(|s| s.is_empty() || s.len() > 256 || s.chars().any(char::is_control))
            || !requested.insert((asset.project_id.clone(), asset.asset_id.clone()))
        {
            return Err(error("source scope requires unique bounded original asset identities"));
        }
    }
    let mut found = BTreeMap::new();
    for item in project.items.values().filter(|item| item.as_media().is_some()) {
        if let Some(raw) = item.metadata.get(BINDING_KEY) {
            let binding: Binding = serde_json::from_str(raw).map_err(|_| error("malformed original binding in source inventory"))?;
            let key = (binding.project_id, binding.asset_id);
            if binding.role == "original" && requested.contains(&key) && found.insert(key, item.id).is_some() {
                return Err(error("source scope has an ambiguous original asset binding"));
            }
        }
    }
    if found.len() != requested.len() {
        return Err(error("source scope names an unavailable original asset"));
    }
    roots.extend(found.into_values());
    dependencies(project, roots)
}

fn monitor_dependency(project: &Project, source: ItemId) -> bool {
    project
        .item(source)
        .is_some_and(|item| !item.as_media().is_some_and(|media| matches!(media.media, MediaRef::File { .. })) || item.metadata.contains_key(BINDING_KEY))
}

/// Include every referenced track item, even disabled/muted ones. Nest/subclip
/// traversal is bounded and detects cycles rather than assuming a direct source.
pub(super) fn dependencies(project: &Project, roots: impl IntoIterator<Item = ItemId>) -> Result<Vec<u64>> {
    let mut stack: Vec<_> = roots.into_iter().map(|id| (id, false)).collect();
    let mut colors = BTreeMap::new();
    let mut files = BTreeSet::new();
    let mut remaining = BUDGET;
    while let Some((id, exit)) = stack.pop() {
        remaining = remaining.checked_sub(1).ok_or_else(|| error("source dependency graph exceeds its bounded traversal"))?;
        if exit {
            colors.insert(id, 2u8);
            continue;
        }
        match colors.get(&id) {
            Some(1) => return Err(error("cyclic sequence or subclip source dependency")),
            Some(2) => continue,
            _ => {}
        }
        let item = project.item(id).ok_or_else(|| error("source dependency is missing from the native project"))?;
        colors.insert(id, 1);
        stack.push((id, true));
        match &item.kind {
            ItemKind::Media(media) if matches!(media.media, MediaRef::File { .. }) => {
                let binding: Binding =
                    serde_json::from_str(item.metadata.get(BINDING_KEY).ok_or_else(|| error("scoped file dependency lacks an explicit original binding"))?)
                        .map_err(|_| error("malformed scoped original binding"))?;
                if binding.role != "original"
                    || [&binding.project_id, &binding.asset_id, &binding.media_id]
                        .iter()
                        .any(|s| s.is_empty() || s.len() > 256 || s.chars().any(char::is_control))
                {
                    return Err(error("scoped file dependency requires its own original correspondence"));
                }
                files.insert(id.0);
                if files.len() > 2048 {
                    return Err(error("source dependency count exceeds the complete verification bound"));
                }
            }
            ItemKind::Sequence(sequence) => {
                for source in super::condense_restore::source_dependencies(project, id)? {
                    if stack.len() >= BUDGET {
                        return Err(error("saved source dependency queue exceeds its traversal bound"));
                    }
                    stack.push((source, false));
                }
                for track in sequence.all_tracks() {
                    for clip in &track.items {
                        if stack.len() >= BUDGET {
                            return Err(error("source dependency queue exceeds its traversal bound"));
                        }
                        stack.push((clip.item, false));
                    }
                }
            }
            ItemKind::Subclip { parent, .. } => stack.push((*parent, false)),
            _ => {}
        }
    }
    Ok(files.into_iter().collect())
}

pub(super) fn require_sequence(project: &Project, witnesses: &media_guard::Witnesses, sequence: ItemId) -> Result<()> {
    if witnesses.is_scoped() {
        witnesses.require(&dependencies(project, [sequence])?)?;
    }
    Ok(())
}

/// A source outside this ticket may not enter a published cut, monitor, changed
/// inactive sequence or restored history. Source admission supplies a separate
/// complete-byte witnesses for exactly its admitted originals.
pub(super) struct Transition {
    pub before_active: Option<ItemId>,
    pub after_active: Option<ItemId>,
    pub before_source: Option<ItemId>,
    pub after_source: Option<ItemId>,
    pub admitted: BTreeSet<ItemId>,
}
pub(super) fn require_transition(before: &Project, after: &Project, witnesses: &media_guard::Witnesses, transition: Transition) -> Result<()> {
    if !witnesses.is_scoped() {
        return Ok(());
    }
    if before.items.len() > BUDGET || after.items.len() > BUDGET {
        return Err(error("changed native source inventory exceeds its traversal bound"));
    }
    super::project_audit::bytes(before)?;
    super::project_audit::bytes(after)?;
    let mut old: BTreeSet<ItemId> = transition.before_active.into_iter().chain(transition.before_source.filter(|id| monitor_dependency(before, *id))).collect();
    let mut new: BTreeSet<ItemId> = transition.after_active.into_iter().chain(transition.after_source.filter(|id| monitor_dependency(after, *id))).collect();
    for id in before.items.keys().chain(after.items.keys()).copied().collect::<BTreeSet<_>>() {
        let previous = before.item(id);
        let next = after.item(id);
        let shared_sequence = match (previous, next) {
            (Some(a), Some(b)) => match (&a.kind, &b.kind) {
                (ItemKind::Sequence(x), ItemKind::Sequence(y)) => std::sync::Arc::ptr_eq(x, y) && a.metadata == b.metadata,
                _ => false,
            },
            _ => false,
        };
        // Saved native history reconstructs Arcs; authored automatic NaNs also
        // make derived PartialEq unsuitable. Use the existing bounded audit.
        let changed = !shared_sequence && previous != next && super::project_audit::value_bytes(&previous)? != super::project_audit::value_bytes(&next)?;
        if changed {
            if previous.is_some() && previous.is_some_and(|i| i.as_sequence().is_some() || i.metadata.contains_key(BINDING_KEY)) {
                old.insert(id);
            }
            if next.is_some() && next.is_some_and(|i| i.as_sequence().is_some() || i.metadata.contains_key(BINDING_KEY)) && !transition.admitted.contains(&id) {
                new.insert(id);
            }
        }
    }
    witnesses.require(&dependencies(before, old)?)?;
    witnesses.require(&dependencies(after, new)?)
}
