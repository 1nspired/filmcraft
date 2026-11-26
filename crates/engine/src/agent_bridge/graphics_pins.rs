//! Authored pin references, shared native dependency analysis and complete basis guards.
//! No geometry, source-file qualification, UID preparation or editor-state mutation.
use super::{graphics_layout, project_audit};
use crate::{Result, Session, commands::bad};
use filmcraft_project::{
    ClipIdentity, TrackItem, graphic,
    graphic_design::{PinNode, PinTarget, RollMode, pin_graph},
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;

pub(super) struct Inventory {
    pub rows: Vec<Value>,
    pub design: Value,
    pub sha256: String,
    pub invalid: usize,
    pub ambiguous: bool,
    pub temporal: bool,
}
pub(super) fn inspect(s: &Session, c: &TrackItem) -> Result<Inventory> {
    if c.effects.len() > 4096 {
        return Err(bad("agent.readGraphicsLayout", "pin effect stack exceeds its bound"));
    }
    let indices = graphic::layer_indices(&c.effects);
    let effects: Vec<_> =
        indices.iter().map(|i| c.effects.get(*i).ok_or_else(|| bad("agent.readGraphicsLayout", "pin layer missing"))).collect::<Result<_>>()?;
    if effects.iter().any(|e| e.instance_id.as_ref().is_some_and(|id| !ClipIdentity::valid_id(id))) {
        return Err(bad("agent.readGraphicsLayout", "pin layer persistent identity is invalid"));
    }
    let nodes: Vec<_> = effects
        .iter()
        .map(|e| PinNode { uid: e.layer.as_ref().map_or(0, |l| l.uid), pin: e.layer.as_ref().and_then(|l| l.pin.clone()), bounds: [0.0; 4], resizable: false })
        .collect();
    let graph = pin_graph(&nodes);
    let mut seen = BTreeSet::new();
    let ambiguous = nodes.iter().filter(|n| n.uid != 0).any(|n| !seen.insert(n.uid));
    let mut invalid = 0;
    let mut active = 0;
    let mut pins = 0;
    // Graph indices and effects were built from the same bounded layer slice.
    let rows: Vec<_> = effects
        .iter()
        .enumerate()
        .map(|(i, e)| {
            let pin = nodes[i].pin.as_ref().map(|p| {
                pins += 1;
                if p.any() {
                    active += 1;
                }
                let target = match p.to {
                    PinTarget::Frame => json!({"kind":"frame"}),
                    PinTarget::Layer(uid) => {
                        let fallback = |kind| json!({"kind":kind,"uid":format!("{uid:016x}")});
                        if graph.ambiguous[i] {
                            fallback("ambiguous")
                        } else {
                            match graph.targets[i] {
                                None => fallback("missing"),
                                Some(j) => match effects[j].instance_id.as_ref() {
                                    Some(id) => json!({"kind":"layer","layerId":id}),
                                    None => fallback("untracked"),
                                },
                            }
                        }
                    }
                };
                let finite = p.offsets.iter().all(|v| v.is_finite());
                let state = if !finite {
                    "invalid_offsets"
                } else if !p.any() {
                    "inactive"
                } else if graph.ambiguous[i] {
                    "ambiguous_target"
                } else if target["kind"] == "missing" {
                    "missing_target"
                } else if target["kind"] == "untracked" {
                    "untracked_target"
                } else if graph.cyclic[i] {
                    "cycle"
                } else {
                    "valid"
                };
                if !matches!(state, "valid" | "inactive") {
                    invalid += 1;
                }
                let edges: Vec<_> = [("left", p.left), ("top", p.top), ("right", p.right), ("bottom", p.bottom)]
                    .into_iter()
                    .filter_map(|(name, on)| on.then_some(name))
                    .collect();
                json!({"target":target,"edges":edges,"offsets":if finite {Some(p.offsets)}else{None},"state":state})
            });
            json!({"layerId":e.instance_id,"position":i,"definition":e.effect,"enabled":e.enabled,"pin":pin})
        })
        .collect();
    let (responsive, roll) = c.graphic.as_ref().map_or((false, RollMode::Off), |g| (g.has_responsive_time(), g.roll.mode));
    let temporal = responsive || roll != RollMode::Off;
    let design = json!({"pinCount":pins,"activePinCount":active,"invalidPinCount":invalid,"uidAmbiguity":ambiguous,
        "responsiveTime":responsive,"roll":roll,"sharedSource":s.project.source_graphics.contains_key(&c.item)});
    let mut hash = Sha256::new();
    hash.update(b"poema-graphics-pins-v1\0");
    hash.update(graphics_layout::digest(s)?);
    hash.update(project_audit::value_bytes(&(c.id, c.identity.as_ref().map(|i| &i.id), &rows, &design))?);
    Ok(Inventory { rows, design, sha256: format!("{:x}", hash.finalize()), invalid, ambiguous, temporal })
}
