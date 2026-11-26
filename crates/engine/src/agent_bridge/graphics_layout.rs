//! Exact native layout gestures over observed persistent targets. The shared
//! human preparation owns geometry; this boundary owns scope and collateral.
use super::{Operation, graphics, graphics_pins, parse, project_audit, sequences};
use crate::{EngineError, Result, Session, commands::bad};
use filmcraft_project::{ClipIdentity, TrackItem};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;

pub(super) const COMMAND: &str = "agent.layoutGraphics";
const READ: &str = "agent.readGraphicsLayout";
const LIMIT: usize = 4096;
fn error(message: impl Into<String>) -> EngineError {
    bad(COMMAND, message)
}
fn hash(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
}
fn rows(s: &Session) -> Result<Vec<(usize, &TrackItem)>> {
    let q = s.active_sequence().ok_or(EngineError::NoSequence)?;
    let mut rows = Vec::new();
    for (i, t) in q.video_tracks.iter().enumerate() {
        for c in &t.items {
            if rows.len() >= LIMIT || c.effects.len() > LIMIT {
                return Err(error("complete video layout inventory exceeds its bound"));
            }
            rows.push((i, c));
        }
    }
    rows.sort_by_key(|(i, c)| (c.start, *i, c.id));
    Ok(rows)
}
pub(super) fn digest(s: &Session) -> Result<String> {
    let sid = s.state.active_sequence.ok_or(EngineError::NoSequence)?;
    let mut h = Sha256::new();
    h.update(b"poema-graphics-layout-v1\0");
    h.update(sid.0.to_le_bytes());
    h.update(project_audit::bytes(&s.project)?);
    Ok(format!("{:x}", h.finalize()))
}
fn design(c: &TrackItem) -> bool {
    c.effects.iter().any(|e| e.layer.as_ref().and_then(|l| l.pin.as_ref()).is_some_and(|p| p.any()))
        || c.graphic.as_ref().is_some_and(|g| g.has_responsive_time() || g.roll.mode != filmcraft_project::graphic_design::RollMode::Off)
}
#[derive(Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum ReadTarget {
    Clips,
    Layers { identity: String },
    Pins { identity: String },
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Read {
    token: String,
    scope: sequences::Scope,
    target: ReadTarget,
    offset: usize,
    limit: usize,
    #[serde(default)]
    projection: bool,
    #[serde(default)]
    index: bool,
}
pub(super) fn read(s: &mut Session, p: &Value) -> Result<Value> {
    let r: Read = parse(READ, p)?;
    if r.offset > LIMIT || !(1..=16).contains(&r.limit) || r.index && r.offset != 0 {
        return Err(error("explicit bounded layout page required"));
    }
    super::validate_metadata_read(s, &r.token, r.projection, READ)?;
    let sid = graphics::scope(s, &r.scope)?;
    let all = rows(s)?;
    let q = s.active_sequence().ok_or(EngineError::NoSequence)?;
    let mut pin_design = None;
    let (total, page) = match &r.target {
        ReadTarget::Clips => {
            if r.offset > all.len() {
                return Err(error("layout page offset is outside the complete collection"));
            }
            let end = if r.index { all.len() } else { r.offset.saturating_add(r.limit).min(all.len()) };
            let page = all
                .get(r.offset..end)
                .ok_or_else(|| error("layout page missing"))?
                .iter()
                .map(|(i, c)| {
                    let track = q.video_tracks.get(*i).ok_or_else(|| error("layout track missing"))?;
                    let motion = c.effects.iter().filter(|e| e.effect == "motion").collect::<Vec<_>>();
                    let motion_id = motion.first().filter(|_| motion.len() == 1).and_then(|e| e.instance_id.as_ref());
                    if r.index {
                        if c.identity.as_ref().is_some_and(|i| !ClipIdentity::valid_id(&i.id)) || motion_id.is_some_and(|id| !ClipIdentity::valid_id(id)) {
                            return Err(error("layout index contains an invalid persistent identity"));
                        }
                        return Ok(json!({"clip":c.id.0,"identity":c.identity.as_ref().map(|v|&v.id),
                            "graphic":crate::graphics::is_graphic(s,q,c.id),"locked":track.locked,
                            "protected":c.identity.as_ref().is_some_and(|v|v.protection.is_some()),
                            "sharedSource":s.project.source_graphics.contains_key(&c.item),"designWorkflowRequired":design(c),"motionId":motion_id}));
                    }
                    let par = s.project.source_par(c.item);
                    if par.is_some_and(|(num, den)| num == 0 || den == 0) {
                        return Err(error("layout source pixel aspect ratio is invalid"));
                    }
                    Ok(json!({"clip":c.id.0,"identity":c.identity.as_ref().map(|v|&v.id),"name":c.name,"track":track.id.0,
                    "locked":track.locked,"protection":c.identity.as_ref().and_then(|v|v.protection.as_ref()),"start":c.start.0,"duration":c.duration.0,
                    "graphic":crate::graphics::is_graphic(s,q,c.id),"layers":filmcraft_project::graphic::layer_indices(&c.effects).len(),
                    "motionId":motion_id,"sharedSource":s.project.source_graphics.contains_key(&c.item),"designWorkflowRequired":design(c),
                    "sourceSize":s.project.source_size(c.item),"sourcePar":par}))
                })
                .collect::<Result<Vec<_>>>()?;
            (all.len(), page)
        }
        ReadTarget::Layers { identity } | ReadTarget::Pins { identity } => {
            if !ClipIdentity::valid_id(identity) {
                return Err(error("observed graphic identity required"));
            }
            let matches = all.iter().filter(|(_, c)| c.identity.as_ref().is_some_and(|v| &v.id == identity)).collect::<Vec<_>>();
            let (_, c) = matches.first().filter(|_| matches.len() == 1).ok_or_else(|| error("graphic identity is absent or ambiguous"))?;
            if !crate::graphics::is_graphic(s, q, c.id) {
                return Err(error("layout layers require a graphic clip"));
            }
            let layers = filmcraft_project::graphic::layer_indices(&c.effects);
            if r.offset > layers.len() {
                return Err(error("layout layer page is outside the graphic"));
            }
            let end = if r.index { layers.len() } else { r.offset.saturating_add(r.limit).min(layers.len()) };
            let page = if matches!(&r.target, ReadTarget::Pins { .. }) {
                let inventory = graphics_pins::inspect(s, c)?;
                let page = inventory.rows.get(r.offset..end).ok_or_else(|| error("pin page missing"))?.to_vec();
                pin_design = Some((inventory.design, inventory.sha256));
                page
            } else {
                layers
                    .get(r.offset..end)
                    .ok_or_else(|| error("layout layers missing"))?
                    .iter()
                    .enumerate()
                    .map(|(n, ei)| {
                        let e = c.effects.get(*ei).ok_or_else(|| error("layout layer missing"))?;
                        Ok(json!({"layerId":e.instance_id,"position":r.offset+n,"definition":e.effect,"enabled":e.enabled}))
                    })
                    .collect::<Result<Vec<_>>>()?
            };
            (layers.len(), page)
        }
    };
    let end = r.offset.saturating_add(page.len());
    let mut out = json!({"status":"read","token":r.token,"revision":s.revision,"sequence":sid.0,"scope":r.scope,"target":r.target,
        "offset":r.offset,"total":total,"nextOffset":if end<total{Some(end)}else{None},"rows":page,
        "collectionSha256":digest(s)?,"effectTracking":graphics::effect_tracking(s,sid),"referenceScope":"exact_read_ticket_only"});
    if r.projection {
        out["requiresSourceValidation"] = json!(true);
    }
    if r.index {
        out["index"] = json!(true);
    }
    if let Some((design, sha256)) = pin_design {
        out["pinDesign"] = design;
        out["pinCollectionSha256"] = json!(sha256);
    }
    if project_audit::value_bytes(&out)?.len() > if r.index { 2 * 1024 * 1024 } else { 64 * 1024 } {
        return Err(error(if r.index { "complete layout index exceeds its 2 MiB bound" } else { "layout page exceeds 64 KiB; request a smaller page" }));
    }
    Ok(out)
}
#[derive(Clone, Copy, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum Direction {
    Left,
    Hcenter,
    Right,
    Top,
    Vcenter,
    Bottom,
}
impl Direction {
    fn name(self) -> &'static str {
        match self {
            Self::Left => "left",
            Self::Hcenter => "hcenter",
            Self::Right => "right",
            Self::Top => "top",
            Self::Vcenter => "vcenter",
            Self::Bottom => "bottom",
        }
    }
}
#[derive(Clone, Copy, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum Mode {
    Frame,
    Group,
    Selection,
}
impl Mode {
    fn name(self) -> &'static str {
        match self {
            Self::Frame => "frame",
            Self::Group => "group",
            Self::Selection => "selection",
        }
    }
    fn native(self) -> crate::graphics::AlignTo {
        match self {
            Self::Frame => crate::graphics::AlignTo::Frame,
            Self::Group => crate::graphics::AlignTo::FrameGroup,
            Self::Selection => crate::graphics::AlignTo::Selection,
        }
    }
}
#[derive(Clone, Copy, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum Axis {
    Horizontal,
    Vertical,
}
#[derive(Clone, Copy, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum Spacing {
    Centers,
    Gaps,
}
#[derive(Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", rename_all_fields = "camelCase", deny_unknown_fields)]
enum PinTo {
    Frame,
    Layer { layer_id: String },
}
#[derive(Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
enum Edge {
    Left,
    Top,
    Right,
    Bottom,
}
#[derive(Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "snake_case", rename_all_fields = "camelCase", deny_unknown_fields)]
enum Edit {
    AlignLayers { target: graphics::Target, layer_ids: Vec<String>, frame: i64, align: Direction, mode: Mode },
    DistributeLayers { target: graphics::Target, layer_ids: Vec<String>, frame: i64, axis: Axis, spacing: Spacing },
    AlignClips { targets: Vec<graphics::Target>, frame: i64, align: Direction, mode: Mode },
    PinLayer { target: graphics::Target, layer_id: String, to: PinTo, edges: Vec<Edge>, frame: i64 },
    UnpinLayer { target: graphics::Target, layer_id: String, frame: i64 },
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PinBasis {
    identity: String,
    sha256: String,
    total: usize,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Request {
    scope: sequences::Scope,
    before_layout_sha256: String,
    #[serde(default)]
    pin_basis: Option<PinBasis>,
    edit: Edit,
}
fn valid_layers(target: &graphics::Target, layers: &[String], frame: i64, min: usize) -> bool {
    target.valid()
        && (0..=1_000_000).contains(&frame)
        && (min..=LIMIT).contains(&layers.len())
        && layers.iter().all(|s| ClipIdentity::valid_id(s))
        && layers.iter().collect::<BTreeSet<_>>().len() == layers.len()
}
pub(super) fn validate(op: &Operation) -> Result<()> {
    let r: Request = parse(COMMAND, &op.params)?;
    let valid = match &r.edit {
        Edit::AlignLayers { target, layer_ids, frame, .. } => valid_layers(target, layer_ids, *frame, 1),
        Edit::DistributeLayers { target, layer_ids, frame, .. } => valid_layers(target, layer_ids, *frame, 3),
        Edit::PinLayer { target, layer_id, to, edges, frame } => {
            valid_layers(target, std::slice::from_ref(layer_id), *frame, 1)
                && edges.len() <= 4
                && edges.iter().collect::<BTreeSet<_>>().len() == edges.len()
                && match to {
                    PinTo::Frame => true,
                    PinTo::Layer { layer_id: to } => ClipIdentity::valid_id(to) && to != layer_id,
                }
        }
        Edit::UnpinLayer { target, layer_id, frame } => valid_layers(target, std::slice::from_ref(layer_id), *frame, 1),
        Edit::AlignClips { targets, frame, .. } => {
            r.pin_basis.is_none()
                && (0..=1_000_000).contains(frame)
                && (1..=LIMIT).contains(&targets.len())
                && targets.iter().all(|t| t.valid())
                && targets.iter().map(|t| t.clip).collect::<BTreeSet<_>>().len() == targets.len()
                && targets.iter().map(|t| &t.identity).collect::<BTreeSet<_>>().len() == targets.len()
        }
    };
    if !valid
        || !hash(&r.before_layout_sha256)
        || r.pin_basis.as_ref().is_some_and(|b| !ClipIdentity::valid_id(&b.identity) || !hash(&b.sha256) || b.total > LIMIT)
        || project_audit::value_bytes(&op.params)?.len() > 512 * 1024
    {
        return Err(error("one bounded explicit layout gesture and a complete current layout digest required"));
    }
    Ok(())
}
fn pin_basis(s: &Session, t: &graphics::Target, basis: Option<&PinBasis>, repair: bool) -> Result<()> {
    let c = graphics::clip_target(s, t, true)?;
    let b = basis.ok_or_else(|| error("read the complete explicit pin constraints on this layout basis before a design operation"))?;
    let inventory = graphics_pins::inspect(s, c)?;
    if b.identity != t.identity || b.sha256 != inventory.sha256 || b.total != inventory.rows.len() {
        return Err(error("pin constraint basis differs from the complete current native graph"));
    }
    if inventory.temporal
        || inventory.ambiguous
        || !repair && inventory.invalid > 0
        || c.frame_hold.is_some_and(|t| !(0..=9_007_199_254_740_991).contains(&t.0))
        || s.project.source_size(c.item).is_none_or(|(w, h)| w == 0 || h == 0)
    {
        return Err(error("pin graph is ambiguous, has unsupported time/roll or invalid layout constraints; repair explicit pins before layout"));
    }
    Ok(())
}
fn layer_indices(s: &Session, t: &graphics::Target, ids: &[String], basis: Option<&PinBasis>) -> Result<Vec<usize>> {
    let c = graphics::clip_target(s, t, true)?;
    if design(c) || basis.is_some() {
        pin_basis(s, t, basis, false)?;
    }
    ids.iter()
        .map(|id| {
            let (index, effect) = graphics::layer(c, id)?;
            if c.effects.get(effect).is_none_or(|e| !e.enabled) {
                return Err(error("every layout target must be enabled; no silent filtering"));
            }
            Ok(index)
        })
        .collect()
}
pub(super) fn run(s: &mut Session, op: &Operation) -> Result<Value> {
    validate(op)?;
    let r: Request = parse(COMMAND, &op.params)?;
    let sid = graphics::scope(s, &r.scope)?;
    filmcraft_project::clip_identity::validate_project(&s.project).map_err(error)?;
    if !graphics::effect_tracking(s, sid) || digest(s)? != r.before_layout_sha256 {
        return Err(error("prepare persistent effects explicitly and read a fresh complete layout collection"));
    }
    let before_count = rows(s)?.len();
    let (command, args, plan) = match &r.edit {
        Edit::AlignLayers { target, layer_ids, frame, align, mode } => {
            let layers = layer_indices(s, target, layer_ids, r.pin_basis.as_ref())?;
            let at = graphics::clock(s, *frame)?;
            let args = json!({"clip":target.clip,"layers":layers,"align":align.name(),"to":mode.name(),"time":at.0});
            let plan = crate::graphics::prepare_alignment(s, &args, align.name(), mode.native(), at)?;
            ("graphics.align", args, plan)
        }
        Edit::DistributeLayers { target, layer_ids, frame, axis, spacing } => {
            let layers = layer_indices(s, target, layer_ids, r.pin_basis.as_ref())?;
            let at = graphics::clock(s, *frame)?;
            let vertical = matches!(axis, Axis::Vertical);
            let gaps = matches!(spacing, Spacing::Gaps);
            let args = json!({"clip":target.clip,"layers":layers,"axis":if vertical{"vertical"}else{"horizontal"},"space":gaps,"time":at.0});
            let plan = crate::graphics::prepare_distribution(s, &args, vertical, gaps, at)?;
            ("graphics.distribute", args, plan)
        }
        Edit::AlignClips { targets, frame, align, mode } => {
            for target in targets {
                graphics::clip_target(s, target, false)?;
            }
            let at = graphics::clock(s, *frame)?;
            let args = json!({"clips":targets.iter().map(|t|t.clip).collect::<Vec<_>>(),"align":align.name(),"to":mode.name(),"time":at.0});
            let plan = crate::graphics::prepare_alignment(s, &args, align.name(), mode.native(), at)?;
            ("graphics.align", args, plan)
        }
        Edit::PinLayer { target, layer_id, to, edges, frame } => {
            pin_basis(s, target, r.pin_basis.as_ref(), true)?;
            let c = graphics::clip_target(s, target, true)?;
            let layer = graphics::layer(c, layer_id)?.0;
            let to = match to {
                PinTo::Frame => json!("frame"),
                PinTo::Layer { layer_id } => json!(graphics::layer(c, layer_id)?.0),
            };
            let at = graphics::clock(s, *frame)?;
            let args = json!({"clip":target.clip,"layer":layer,"to":to,"edges":edges,"time":at.0});
            let plan = crate::graphic_templates::prepare_pinning(s, &args, at)?;
            ("graphics.pin", args, plan)
        }
        Edit::UnpinLayer { target, layer_id, frame } => {
            pin_basis(s, target, r.pin_basis.as_ref(), true)?;
            let c = graphics::clip_target(s, target, true)?;
            let (layer, effect) = graphics::layer(c, layer_id)?;
            if c.effects.get(effect).and_then(|e| e.layer.as_ref()).and_then(|l| l.pin.as_ref()).is_none() {
                return Err(error("layer has no pin to remove; no identity bookkeeping was requested"));
            }
            let at = graphics::clock(s, *frame)?;
            let args = json!({"clip":target.clip,"layer":layer,"to":"none","time":at.0});
            let plan = crate::graphic_templates::prepare_pinning(s, &args, at)?;
            ("graphics.pin", args, plan)
        }
    };
    s.execute(command, args)?;
    if project_audit::bytes(&s.project)? != project_audit::bytes(&plan.expected)? {
        return Err(error("human layout changed state beyond its prepared position/pin plan; staging discarded"));
    }
    let after = digest(s)?;
    if after == r.before_layout_sha256 {
        return Err(error("layout gesture has no authored change"));
    }
    Ok(
        json!({"beforeLayoutSha256":r.before_layout_sha256,"afterLayoutSha256":after,"beforeCount":before_count,"afterCount":rows(s)?.len(),"edit":r.edit,"humanCommand":command}),
    )
}
