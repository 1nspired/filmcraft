//! Closed authored-state envelope around historical computed source data.
//! No disk access, decoding, sync analysis, command execution or publication.
use super::{error, project_audit};
use crate::{EditorState, Result};
use filmcraft_project::{BinEntry, ItemId, Project};
use serde_json::Value;
fn same<T: serde::Serialize>(a: &T, b: &T) -> Result<bool> {
    Ok(project_audit::value_bytes(a)? == project_audit::value_bytes(b)?)
}
pub(super) fn validate(before: &Project, after: &Project, editor: &EditorState, command: &str, params: &Value, result: &Value) -> Result<()> {
    let mut envelope = after.clone();
    envelope.items = before.items.clone();
    envelope.root = before.root.clone();
    envelope.next_id = before.next_id;
    if !same(&envelope, before)? {
        return Err(error("source production changes unrelated global authored project state"));
    }
    if command == "clip.synchronize" {
        if !same(&before.root, &after.root)? || before.items.len() != after.items.len() {
            return Err(error("timeline synchronization changes project organization or source inventory"));
        }
        let active = editor.active_sequence.ok_or_else(|| error("historical synchronization has no original active timeline"))?;
        for (id, item) in &before.items {
            let observed = after.item(*id).ok_or_else(|| error("historical synchronization removed a source"))?;
            if *id != active {
                if !same(item, observed)? {
                    return Err(error("synchronization changes an unrelated source or sequence"));
                }
            } else {
                let mut candidate = observed.clone();
                let q = candidate.as_sequence_mut().ok_or_else(|| error("historical timeline missing"))?;
                let old = item.as_sequence().ok_or_else(|| error("original timeline missing"))?;
                if q.video_tracks.len() != old.video_tracks.len() || q.audio_tracks.len() != old.audio_tracks.len() {
                    return Err(error("synchronization cannot add/remove tracks"));
                }
                for (track, original) in q.video_tracks.iter_mut().zip(&old.video_tracks).chain(q.audio_tracks.iter_mut().zip(&old.audio_tracks)) {
                    track.items = original.items.clone();
                }
                if !same(item, &candidate)? {
                    return Err(error("synchronization changes timeline or track settings outside native placements"));
                }
            }
        }
        return Ok(());
    }
    if !matches!(command, "clip.mergeClips" | "clip.createMulticam") {
        return Err(error("unknown source-production envelope"));
    }
    let created = result[if command == "clip.mergeClips" { "item" } else { "sequence" }].as_u64().ok_or_else(|| error("historical created source missing"))?;
    if before.items.contains_key(&ItemId(created)) || after.items.len() != before.items.len().saturating_add(1) {
        return Err(error("source production must create exactly its original next native item"));
    }
    for (id, item) in &before.items {
        if !same(&Some(item), &after.item(*id))? {
            return Err(error("source production rewrites a pre-existing source or sequence"));
        }
    }
    let source = after.item(ItemId(created)).and_then(|i| i.as_sequence()).ok_or_else(|| error("new source is not a native sequence"))?;
    if (command == "clip.mergeClips" && (source.merged.is_none() || source.multicam.is_some()))
        || (command == "clip.createMulticam" && (source.multicam.is_none() || source.merged.is_some()))
    {
        return Err(error("created source type differs from its exact native gesture"));
    }
    source.check().map_err(error)?;
    let rows =
        result["clips"].as_array().filter(|v| !v.is_empty() && v.len() <= 2048).ok_or_else(|| error("complete original computed source order missing"))?;
    let order = rows
        .iter()
        .map(|v| v["item"].as_u64().filter(|id| before.item(ItemId(*id)).is_some()).ok_or_else(|| error("computed source refers outside the original project")))
        .collect::<Result<Vec<_>>>()?;
    let parent =
        if command == "clip.mergeClips" { order.first().and_then(|id| before.root.parent_of(ItemId(*id))).filter(|id| *id != before.root.id) } else { None };
    let mut tree = before.clone();
    let created_item = after.item(ItemId(created)).ok_or_else(|| error("created native source missing"))?;
    let expected = tree.new_sequence(&created_item.name, source.settings.clone(), source.video_tracks.len(), source.audio_tracks.len(), parent);
    if expected != ItemId(created) {
        return Err(error("created source differs from original native track/item allocation"));
    }
    if command == "clip.createMulticam" && params["processedBin"] == true {
        let existing = tree.root.children.iter().find_map(|c| match c {
            BinEntry::Bin(b) if b.name == "Processed Clips" => Some(b.id),
            _ => None,
        });
        let bin = if let Some(id) = existing {
            id
        } else {
            tree.next_id = after.next_id.checked_sub(1).ok_or_else(|| error("processed-bin allocation underflow"))?;
            tree.add_bin("Processed Clips", None)
        };
        for id in order {
            tree.root.remove_item(ItemId(id));
            tree.root.find_bin_mut(bin).ok_or_else(|| error("processed bin missing"))?.children.push(BinEntry::Item(ItemId(id)));
        }
    }
    if !same(&tree.root, &after.root)? {
        return Err(error("source production changes unrelated project-tree entries"));
    }
    Ok(())
}
