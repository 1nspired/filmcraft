//! Optional persistent timeline-instance data (schema v16) and clip-scoped effect/mask
//! identities (schema v17) for the local Poema experiment.
//! Native numeric ids remain edit handles; random instance ids are annotation keys. A new
//! split/copy gets its own id and records its immediate parent, never a filename-derived match.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

pub const TRACKING_KEY: &str = "poema.tracking.v1";
pub const EFFECT_TRACKING_KEY: &str = "poema.effectTracking.v1";

/// Identities are local to a persistent clip (and masks to their effect).
/// Moving/splitting a clip therefore needs no global effect-id rewrite.
pub fn validate_effects(clip: &crate::TrackItem, required: bool, budget: &mut usize) -> Result<(), String> {
    if clip.effects.len() > 4096 {
        return Err("effect identity stack exceeds 4096 instances".into());
    }
    let mut effects = std::collections::BTreeSet::new();
    for e in &clip.effects {
        *budget = budget.checked_sub(1).ok_or("effect identity inventory exceeds its bound")?;
        if required && e.instance_id.is_none() {
            return Err("tracked effect is missing its identity".into());
        }
        if let Some(id) = &e.instance_id
            && (!ClipIdentity::valid_id(id) || !effects.insert(id))
        {
            return Err("invalid or duplicate clip-scoped effect identity".into());
        }
        if e.masks.len() > 4096 {
            return Err("mask identity stack exceeds its bound".into());
        }
        let mut masks = std::collections::BTreeSet::new();
        for m in &e.masks {
            *budget = budget.checked_sub(1).ok_or("effect identity inventory exceeds its bound")?;
            if required && m.instance_id.is_none() {
                return Err("tracked mask is missing its identity".into());
            }
            if let Some(id) = &m.instance_id
                && (!ClipIdentity::valid_id(id) || !masks.insert(id))
            {
                return Err("invalid or duplicate effect-scoped mask identity".into());
            }
        }
    }
    Ok(())
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ClipIdentity {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<String>,
    /// Whole-instance notes/references only; timed selects/bites need a separate range model.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub annotations: BTreeMap<String, String>,
    /// Editor's reason to freeze this instance against agent changes. Humans can still edit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub protection: Option<String>,
}

impl ClipIdentity {
    pub fn valid_id(id: &str) -> bool {
        id.len() == 32 && id.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    }

    pub fn validate(&self) -> Result<(), String> {
        if !Self::valid_id(&self.id) || self.parent.as_ref().is_some_and(|id| !Self::valid_id(id) || id == &self.id) {
            return Err("invalid persistent clip identity or parent".into());
        }
        if self.annotations.len() > 64 || self.annotations.iter().any(|(k, v)| k.is_empty() || k.len() > 64 || v.len() > 1024) {
            return Err("clip annotations exceed 64 keys / 64-byte keys / 1024-byte values".into());
        }
        if self.protection.as_ref().is_some_and(|v| v.trim().is_empty() || v.len() > 1024) {
            return Err("clip protection needs a nonblank reason of at most 1024 bytes".into());
        }
        Ok(())
    }
}

/// Refuse damaged identity data, not silently detach notes/protections. This validates existing
/// carriers; it does not opt legacy projects into tracking or mint ids while loading a file.
pub fn validate_project(project: &crate::Project) -> Result<(), String> {
    let mut seen = std::collections::BTreeSet::new();
    let mut effect_budget = 1_000_000usize;
    for item in project.sequences() {
        let Some(q) = item.as_sequence() else { continue };
        let tracking = item.metadata.get(TRACKING_KEY);
        let effects = item.metadata.get(EFFECT_TRACKING_KEY);
        if effects.is_some_and(|v| v != "1") || effects.is_some() && tracking.is_none() {
            return Err("unsupported or unowned effect tracking version".into());
        }
        if tracking.is_some_and(|v| v != "1") {
            return Err("unsupported instance tracking version".into());
        }
        for clip in q.all_tracks().flat_map(|t| &t.items) {
            validate_effects(clip, effects.is_some(), &mut effect_budget)?;
            if tracking.is_some() && clip.identity.is_none() {
                return Err("tracked clip is missing its persistent identity".into());
            }
            if let Some(identity) = &clip.identity {
                identity.validate()?;
                if seen.len() >= 100_000 || !seen.insert(identity.id.as_str()) {
                    return Err("duplicate persistent clip identity or more than 100000 tracked instances".into());
                }
            }
        }
    }
    Ok(())
}
