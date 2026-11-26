//! Identity allocation on a staged native edit, never on read/load/render.
use super::{error, fresh};
use crate::Result;
use filmcraft_project::TrackItem;
use std::collections::BTreeSet;

pub(super) fn reconcile(old: Option<&TrackItem>, clip: &mut TrackItem) -> Result<()> {
    // Candidate duplicates cannot be disambiguated by ordinal or parameter
    // equality. Real copy/preset handlers clear new instances before reaching us.
    filmcraft_project::clip_identity::validate_effects(clip, false, &mut 1_000_000usize).map_err(error)?;
    let mut reserved: BTreeSet<String> = old.into_iter().flat_map(|c| &c.effects).filter_map(|e| e.instance_id.clone()).collect();
    reserved.extend(clip.effects.iter().filter_map(|e| e.instance_id.clone()));
    let mut seen = BTreeSet::new();
    for e in &mut clip.effects {
        if e.instance_id.is_none() {
            e.instance_id = Some(fresh(&reserved, &seen)?);
        }
        let id = e.instance_id.as_ref().ok_or_else(|| error("effect identity allocation failed"))?;
        if !seen.insert(id.clone()) {
            return Err(error("duplicate effect identity after reconciliation"));
        }
        let previous = old.into_iter().flat_map(|c| &c.effects).find(|p| p.instance_id.as_ref() == Some(id));
        let mut reserved: BTreeSet<String> = previous.into_iter().flat_map(|e| &e.masks).filter_map(|m| m.instance_id.clone()).collect();
        reserved.extend(e.masks.iter().filter_map(|m| m.instance_id.clone()));
        let mut seen = BTreeSet::new();
        for m in &mut e.masks {
            if m.instance_id.is_none() {
                m.instance_id = Some(fresh(&reserved, &seen)?);
            }
            if !seen.insert(m.instance_id.clone().ok_or_else(|| error("mask identity allocation failed"))?) {
                return Err(error("duplicate mask identity after reconciliation"));
            }
        }
    }
    Ok(())
}
