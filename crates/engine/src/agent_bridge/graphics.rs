//! Scoped graphics inventory and exact gestures through the native human services.
//! Clip/effect identities are existing persistent carriers, never new filename/index handles.
use super::{Operation, effect_stack, parse, project_audit, sequences};
use crate::{EngineError, Result, Session, commands::bad};
use filmcraft_project::{ClipId, ClipIdentity, EffectInstance, ItemId, ItemKind, TrackId, graphic};
use filmcraft_time::{Tick, TimeRange};
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};

pub(super) const COMMAND: &str = "agent.editGraphics";
const SAFE: i64 = 9_007_199_254_740_991;
const MAX_CLIPS: usize = 4096;
fn error(message: impl Into<String>) -> EngineError {
    bad(COMMAND, message)
}
fn text(s: &str, max: usize) -> bool {
    s.len() <= max && !s.contains('\0')
}
fn id(n: u64) -> bool {
    n > 0 && n <= SAFE as u64
}
fn hash(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
}
fn point(p: &[f64; 2]) -> bool {
    p.iter().all(|v| v.is_finite() && v.abs() <= 100_000.0)
}
fn size(p: &[f64; 2]) -> bool {
    point(p) && p.iter().all(|v| *v >= 1.0)
}
pub(super) fn scope(s: &Session, scope: &sequences::Scope) -> Result<ItemId> {
    let sid = s.state.active_sequence.ok_or(EngineError::NoSequence)?;
    if !sequences::belongs(&s.project, sid, scope)? || s.project.items.len() > 100_000 {
        return Err(error("graphics sequence is outside the explicit workspace"));
    }
    if crate::editorial_workspace::association(&s.project)?.is_none() {
        let mut owner = false;
        for item in s.project.items.values() {
            if let Some(raw) = item.metadata.get(super::BINDING_KEY) {
                if raw.len() > 16 * 1024 {
                    return Err(error("original binding exceeds its bound"));
                }
                let binding: super::Binding = serde_json::from_str(raw).map_err(|e| error(e.to_string()))?;
                owner |= binding.project_id == scope.project_id && binding.role == "original";
            }
        }
        if !owner {
            return Err(error("associate an owning film or retain its original binding before graphics reads"));
        }
    }
    s.active_sequence().ok_or(EngineError::NoSequence)?.settings.validate().map_err(error)?;
    project_audit::bytes(&s.project)?;
    if s.active_sequence().ok_or(EngineError::NoSequence)?.all_tracks().flat_map(|t| &t.items).take(100_001).count() > 100_000 {
        return Err(error("graphics clip inventory exceeds its bound"));
    }
    Ok(sid)
}
fn clips(s: &Session) -> Result<Vec<(usize, &filmcraft_project::TrackItem)>> {
    let q = s.active_sequence().ok_or(EngineError::NoSequence)?;
    let mut rows = Vec::new();
    for (i, t) in q.video_tracks.iter().enumerate() {
        for c in &t.items {
            if crate::graphics::is_graphic(s, q, c.id) {
                if rows.len() >= MAX_CLIPS || c.effects.len() > 4096 {
                    return Err(error("graphics collection exceeds its bound"));
                }
                rows.push((i, c));
            }
        }
    }
    rows.sort_by_key(|(i, c)| (c.start, *i, c.id));
    Ok(rows)
}
pub(super) fn digest(s: &Session) -> Result<String> {
    let q = s.active_sequence().ok_or(EngineError::NoSequence)?;
    let rows = clips(s)?;
    let tracks = q.video_tracks.iter().map(|t| (t.id, t.locked)).collect::<Vec<_>>();
    let mut sources = BTreeMap::new();
    for (_, c) in &rows {
        sources.entry(c.item).or_insert((s.project.item(c.item).ok_or_else(|| error("graphic canvas is missing"))?, s.project.source_graphics.get(&c.item)));
    }
    let sid = s.state.active_sequence.ok_or(EngineError::NoSequence)?;
    Ok(format!("{:x}", Sha256::digest(project_audit::value_bytes(&(q.settings.clone(), effect_tracking(s, sid), tracks, rows, sources))?)))
}
pub(super) fn effect_tracking(s: &Session, sid: ItemId) -> bool {
    s.project.item(sid).is_some_and(|it| it.metadata.get(filmcraft_project::clip_identity::EFFECT_TRACKING_KEY).is_some_and(|v| v == "1"))
}
#[derive(Clone, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum ReadTarget {
    Clips,
    Layers { identity: String },
    Geometry { identity: String, frame: i64 },
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
}
pub(super) fn read(s: &mut Session, p: &Value) -> Result<Value> {
    let r: Read = parse("agent.readGraphics", p)?;
    if r.offset > MAX_CLIPS || !(1..=16).contains(&r.limit) {
        return Err(error("bounded explicit graphics page required"));
    }
    super::validate_metadata_read(s, &r.token, r.projection, "agent.readGraphics")?;
    let sid = scope(s, &r.scope)?;
    let collection = digest(s)?;
    let tracking = s.project.item(sid).is_some_and(|it| it.metadata.get(filmcraft_project::clip_identity::EFFECT_TRACKING_KEY).is_some_and(|v| v == "1"));
    let rows = clips(s)?;
    let (kind, total, rows) = match &r.target {
        ReadTarget::Clips => {
            if r.offset > rows.len() {
                return Err(error("graphics page offset is outside collection"));
            }
            let end = r.offset.saturating_add(r.limit).min(rows.len());
            let q = s.active_sequence().ok_or(EngineError::NoSequence)?;
            let page = rows
                .get(r.offset..end)
                .ok_or_else(|| error("invalid graphics page"))?
                .iter()
                .map(|(i, c)| {
                    let tr = q.video_tracks.get(*i).ok_or_else(|| error("graphic track missing"))?;
                    let canvas = match s.project.item(c.item).map(|i| &i.kind) {
                        Some(ItemKind::Graphic { width, height, rate }) => json!({"width":width,"height":height,"frameRate":rate}),
                        _ => return Err(error("graphic canvas is missing")),
                    };
                    Ok(json!({"clip":c.id.0,"identity":c.identity.as_ref().map(|x|&x.id),"track":tr.id.0,
                    "name":c.name,"canvas":canvas,
                    "locked":tr.locked,"protection":c.identity.as_ref().and_then(|x|x.protection.as_ref()),"start":c.start.0,"duration":c.duration.0,
                    "layers":graphic::layer_indices(&c.effects).len(),"sourceGraphic":s.project.source_graphics.contains_key(&c.item),
                    "stackSha256":effect_stack::digest(&c.effects)?}))
                })
                .collect::<Result<Vec<_>>>()?;
            ("clips", rows.len(), page)
        }
        ReadTarget::Layers { identity } | ReadTarget::Geometry { identity, .. } => {
            if !ClipIdentity::valid_id(identity) {
                return Err(error("persistent graphic identity required"));
            }
            let matches = rows.iter().filter(|(_, c)| c.identity.as_ref().is_some_and(|i| &i.id == identity)).collect::<Vec<_>>();
            if matches.len() != 1 {
                return Err(error("graphic identity is absent or ambiguous"));
            }
            let (_, c) = matches.first().ok_or_else(|| error("graphic missing"))?;
            let layers = graphic::layer_indices(&c.effects);
            if r.offset > layers.len() {
                return Err(error("layer page offset is outside graphic"));
            }
            let end = r.offset.saturating_add(r.limit).min(layers.len());
            let evaluated = if let ReadTarget::Geometry { frame, .. } = &r.target {
                if !(0..=1_000_000).contains(frame) {
                    return Err(error("explicit bounded native evaluation frame required"));
                }
                let value = crate::graphics::list_layers_at(s, c.id, clock(s, *frame)?)?;
                let evaluated = value.get("layers").and_then(Value::as_array).ok_or_else(|| error("native graphic evaluation is missing"))?.clone();
                if evaluated.len() != layers.len() {
                    return Err(error("native graphic evaluation omitted a layer"));
                }
                Some(evaluated)
            } else {
                None
            };
            let page = layers
                .get(r.offset..end)
                .ok_or_else(|| error("invalid layer page"))?
                .iter()
                .enumerate()
                .map(|(n, ei)| {
                    let e = c.effects.get(*ei).ok_or_else(|| error("layer missing"))?;
                    if let Some(evaluated) = &evaluated {
                        let evaluation = evaluated.get(r.offset + n).ok_or_else(|| error("evaluated layer missing"))?;
                        if evaluation.get("layer").and_then(Value::as_u64) != Some((r.offset + n) as u64) {
                            return Err(error("evaluated layer order changed"));
                        }
                        let mut row = json!({"layerId":e.instance_id,"position":r.offset+n});
                        for key in ["kind", "text", "textType", "box", "overflow", "enabled", "anchor", "scale", "scaleWidth", "localBounds", "quad"] {
                            row[key] = evaluation.get(key).ok_or_else(|| error("incomplete evaluated layer"))?.clone();
                        }
                        row["positionPx"] = evaluation.get("position").ok_or_else(|| error("missing evaluated position"))?.clone();
                        return Ok(row);
                    }
                    Ok(json!({"layerId":e.instance_id,"position":r.offset+n,"definition":e.effect,"authored":e,
                    "properties":e.def().map(|d|&d.params)}))
                })
                .collect::<Result<Vec<_>>>()?;
            (if evaluated.is_some() { "geometry" } else { "layers" }, layers.len(), page)
        }
    };
    let end = r.offset.saturating_add(rows.len());
    let mut out = json!({"status":"read","token":r.token,"revision":s.revision,"sequence":sid.0,"scope":r.scope,
        "kind":kind,"offset":r.offset,"total":total,"nextOffset":if end<total{Some(end)}else{None},
        "graphics":rows,"collectionSha256":collection,"effectTracking":tracking,"referenceScope":"exact_read_ticket_only"});
    if r.projection {
        out["requiresSourceValidation"] = json!(true);
    }
    if let ReadTarget::Geometry { identity, frame } = &r.target {
        out["evaluationTarget"] = json!({"identity":identity,"frame":frame});
    }
    if project_audit::value_bytes(&out)?.len() > 64 * 1024 {
        return Err(error("graphics page exceeds 64 KiB; request a smaller page"));
    }
    Ok(out)
}
#[derive(Clone, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", rename_all_fields = "camelCase", deny_unknown_fields)]
enum Content {
    Text { text: String, position: [f64; 2], size: f64, vertical: bool, box_size: Option<[f64; 2]> },
    Shape { shape: String, position: [f64; 2], size: [f64; 2], points: Vec<[f64; 2]> },
}
impl Content {
    fn valid(&self) -> bool {
        match self {
            Self::Text { text: t, position, size: n, vertical, box_size } => {
                text(t, 16 * 1024)
                    && point(position)
                    && n.is_finite()
                    && (1.0..=10_000.0).contains(n)
                    && box_size.as_ref().is_none_or(size)
                    && (!vertical || box_size.is_none())
            }
            Self::Shape { shape, position, size: s, points } => {
                ["rectangle", "ellipse", "polygon", "path"].contains(&shape.as_str())
                    && point(position)
                    && size(s)
                    && points.len() <= 2048
                    && points.iter().all(point)
                    && if shape == "path" { points.len() >= 3 } else { points.is_empty() }
            }
        }
    }
    fn human(&self) -> (&'static str, Value) {
        match self {
            Self::Text { text, position, size, vertical, box_size } => {
                let mut args = json!({"text":text,"position":position,"size":size,"vertical":vertical});
                if let Some(b) = box_size {
                    args["box"] = json!(b);
                }
                ("graphics.newText", args)
            }
            Self::Shape { shape, position, size, points } => ("graphics.newShape", json!({"shape":shape,"position":position,"size":size,"points":points})),
        }
    }
    fn prepared(&self, s: &Session) -> Result<EffectInstance> {
        let (_, p) = self.human();
        match self {
            Self::Text { .. } => Ok(crate::graphics::prepare_text_layer(s, &p)?.1),
            Self::Shape { .. } => crate::graphics::prepare_shape_layer(s, &p),
        }
    }
}
#[derive(Clone, Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Target {
    pub(super) clip: u64,
    pub(super) identity: String,
}
impl Target {
    pub(super) fn valid(&self) -> bool {
        id(self.clip) && ClipIdentity::valid_id(&self.identity)
    }
}
#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", rename_all_fields = "camelCase", deny_unknown_fields)]
enum Edit {
    EnableTracking,
    Create { track: u64, frame: i64, duration_frames: i64, content: Content },
    AddLayer { target: Target, content: Content },
    Set { target: Target, layer_id: String, frame: i64, props: Value },
    DeleteLayer { target: Target, layer_id: String },
    ReorderLayer { target: Target, layer_id: String, before_layer_id: Option<String> },
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Request {
    scope: sequences::Scope,
    before_graphics_sha256: String,
    edit: Edit,
}
pub(super) fn validate(op: &Operation) -> Result<()> {
    let r: Request = parse(COMMAND, &op.params)?;
    let valid = match &r.edit {
        Edit::EnableTracking => op.params.get("edit").and_then(Value::as_object).is_some_and(|edit| edit.len() == 1 && edit.contains_key("type")),
        Edit::Create { track, frame, duration_frames, content } => {
            id(*track) && (0..=1_000_000).contains(frame) && (1..=1_000_000).contains(duration_frames) && content.valid()
        }
        Edit::AddLayer { target, content } => target.valid() && content.valid(),
        Edit::Set { target, layer_id, frame, props } => {
            target.valid()
                && ClipIdentity::valid_id(layer_id)
                && (0..=1_000_000).contains(frame)
                && props.as_object().is_some_and(|p| !p.is_empty() && p.len() <= 32)
        }
        Edit::DeleteLayer { target, layer_id } => target.valid() && ClipIdentity::valid_id(layer_id),
        Edit::ReorderLayer { target, layer_id, before_layer_id } => {
            target.valid()
                && ClipIdentity::valid_id(layer_id)
                && before_layer_id.as_ref().is_none_or(|id| ClipIdentity::valid_id(id) && id != layer_id)
                && op.params.get("edit").is_some_and(|e| e.get("beforeLayerId").is_some())
        }
    };
    if !hash(&r.before_graphics_sha256) || !valid || project_audit::value_bytes(&op.params)?.len() > 128 * 1024 {
        return Err(error("one explicit bounded graphics gesture with a complete collection digest required"));
    }
    Ok(())
}
fn clip<'a>(s: &'a Session, target: &Target) -> Result<&'a filmcraft_project::TrackItem> {
    clip_target(s, target, true)
}
pub(super) fn clip_target<'a>(s: &'a Session, target: &Target, want_graphic: bool) -> Result<&'a filmcraft_project::TrackItem> {
    let q = s.active_sequence().ok_or(EngineError::NoSequence)?;
    let (tr, c) = q.find_item(ClipId(target.clip)).ok_or_else(|| error("explicit current graphic is absent"))?;
    if crate::graphics::is_graphic(s, q, c.id) != want_graphic
        || c.identity.as_ref().is_none_or(|i| i.id != target.identity || i.protection.is_some())
        || q.track(tr).is_none_or(|t| t.locked || t.kind != filmcraft_project::TrackKind::Video)
        || s.project.source_graphics.contains_key(&c.item)
    {
        return Err(error("graphic target is foreign, locked, protected or requires shared-source graphics authoring"));
    }
    if !(0..=SAFE).contains(&c.start.0)
        || !(1..=SAFE).contains(&c.duration.0)
        || c.start.0.checked_add(c.duration.0).is_none_or(|end| end > SAFE)
        || !(0..=SAFE).contains(&c.source_in.0)
        || !c.speed.is_finite()
        || c.speed <= 0.0
    {
        return Err(error("graphic clock exceeds exact transport"));
    }
    Ok(c)
}
pub(super) fn layer(c: &filmcraft_project::TrackItem, id: &str) -> Result<(usize, usize)> {
    graphic::layer_indices(&c.effects)
        .iter()
        .enumerate()
        .find_map(|(l, i)| c.effects.get(*i).filter(|e| e.instance_id.as_deref() == Some(id)).map(|_| (l, *i)))
        .ok_or_else(|| error("persistent graphic layer is absent"))
}
pub(super) fn clock(s: &Session, frame: i64) -> Result<Tick> {
    let rate = s.sequence_rate();
    let t = rate.tick_of(frame);
    if !(0..=SAFE).contains(&t.0) || rate.frame_at(t) != frame || rate.snap(t) != t {
        return Err(error("frame exceeds exact native transport"));
    }
    Ok(t)
}
fn properties(e: &EffectInstance, props: &Value) -> Result<()> {
    let p = props.as_object().ok_or_else(|| error("explicit properties required"))?;
    for (key, v) in p {
        if key == "enabled" {
            if !v.is_boolean() {
                return Err(error("enabled must be a boolean"));
            }
            continue;
        }
        if ["font", "font_style"].contains(&key.as_str()) {
            return Err(error("font resources require their explicit admission contract"));
        }
        let def = e.def().and_then(|d| d.param(key)).ok_or_else(|| error("unknown canonical graphic parameter"))?;
        if let Some(t) = v.as_str()
            && !text(t, 16 * 1024)
        {
            return Err(error("graphic text exceeds its bound"));
        }
        let strict = crate::commands::effect_param_value(def, v, COMMAND)?;
        let human = crate::graphics::to_param(&def.default, key, v).ok_or_else(|| error("graphic value has the wrong type"))?;
        if project_audit::value_bytes(&strict)? != project_audit::value_bytes(&human)? {
            return Err(error("graphic parameter would be clamped or coerced"));
        }
    }
    Ok(())
}
fn assign_expected_ids(expected: &mut [EffectInstance], actual: &[EffectInstance]) -> Result<()> {
    if expected.len() != actual.len() {
        return Err(error("graphic gesture changed effect count unexpectedly"));
    }
    for (e, a) in expected.iter_mut().zip(actual) {
        if e.instance_id.is_none() {
            let id = a.instance_id.as_ref().filter(|id| ClipIdentity::valid_id(id)).ok_or_else(|| error("new layer identity missing"))?;
            e.instance_id = Some(id.clone());
        }
        if e.masks.len() != a.masks.len() {
            return Err(error("effect identity preparation changed mask count"));
        }
        for (mask, actual) in e.masks.iter_mut().zip(&a.masks) {
            if mask.instance_id.is_none() {
                let id = actual.instance_id.as_ref().filter(|id| ClipIdentity::valid_id(id)).ok_or_else(|| error("new mask identity missing"))?;
                mask.instance_id = Some(id.clone());
            }
        }
    }
    Ok(())
}
fn enable_tracking(s: &mut Session, sid: ItemId, r: &Request) -> Result<Value> {
    if effect_tracking(s, sid) || s.project.item(sid).is_none_or(|it| it.metadata.get(filmcraft_project::clip_identity::TRACKING_KEY).is_none_or(|v| v != "1"))
    {
        return Err(error("effect preparation requires an already adopted clip-tracked sequence without effect tracking"));
    }
    let before_total = clips(s)?.len();
    let mut expected = (*s.project).clone();
    expected.item_mut(sid).ok_or(EngineError::NoSequence)?.metadata.insert(filmcraft_project::clip_identity::EFFECT_TRACKING_KEY.into(), "1".into());
    // The existing human command owns allocation, validation and its native
    // history step. The audit adopts only newly allocated identity carriers.
    s.execute("poema.trackEffects", json!({}))?;
    filmcraft_project::clip_identity::validate_project(&s.project).map_err(error)?;
    let actual = s.active_sequence().ok_or(EngineError::NoSequence)?;
    for c in expected.sequence_mut(sid).ok_or(EngineError::NoSequence)?.all_tracks_mut().flat_map(|t| &mut t.items) {
        let after = actual.find_item(c.id).ok_or_else(|| error("effect preparation lost an existing clip"))?.1;
        assign_expected_ids(&mut c.effects, &after.effects)?;
    }
    if project_audit::bytes(&s.project)? != project_audit::bytes(&expected)? || !effect_tracking(s, sid) {
        return Err(error("effect preparation changed authored state outside its identity carriers"));
    }
    let after = digest(s)?;
    if after == r.before_graphics_sha256 {
        return Err(error("effect tracking preparation has no state change"));
    }
    Ok(json!({"beforeGraphicsSha256":r.before_graphics_sha256,"afterGraphicsSha256":after,"beforeCount":before_total,"afterCount":clips(s)?.len(),
        "beforeEffectTracking":false,"afterEffectTracking":true,"clip":null,"identity":null,"changedLayerId":null,"humanCommand":"poema.trackEffects"}))
}
pub(super) fn run(s: &mut Session, op: &Operation) -> Result<Value> {
    validate(op)?;
    let r: Request = parse(COMMAND, &op.params)?;
    let sid = scope(s, &r.scope)?;
    filmcraft_project::clip_identity::validate_project(&s.project).map_err(error)?;
    if digest(s)? != r.before_graphics_sha256 {
        return Err(error("complete graphics collection changed"));
    }
    if matches!(r.edit, Edit::EnableTracking) {
        return enable_tracking(s, sid, &r);
    }
    if !effect_tracking(s, sid) {
        return Err(error("persistent tracking required or complete graphics collection changed"));
    }
    let mut expected = (*s.project).clone();
    let mut changed_layer = None;
    let (command, args, target_clip) = match &r.edit {
        Edit::EnableTracking => return Err(error("effect tracking must use its explicit preparation path")),
        Edit::Create { track, frame, duration_frames, content } => {
            if clips(s)?.len() >= MAX_CLIPS || s.project.next_id == 0 || s.project.next_id.checked_add(3).is_none_or(|n| n > SAFE as u64) {
                return Err(error("graphics collection or allocation counter exceeds its bound"));
            }
            let q = s.active_sequence().ok_or(EngineError::NoSequence)?;
            let ti = q.video_tracks.iter().position(|t| t.id == TrackId(*track)).ok_or_else(|| error("explicit video destination is absent"))?;
            if q.video_tracks.get(ti).is_none_or(|t| t.locked) {
                return Err(error("explicit video destination is locked"));
            }
            let start = clock(s, *frame)?;
            let duration = clock(s, *duration_frames)?;
            if duration <= Tick::ZERO || start.0.checked_add(duration.0).is_none_or(|n| n > SAFE) {
                return Err(error("graphic range exceeds exact transport"));
            }
            let (w, h, rate) = (q.settings.width, q.settings.height, q.settings.frame_rate);
            let src = crate::graphics::graphic_source(&mut expected, w, h, rate);
            let mut c = expected
                .make_track_item(src, filmcraft_project::TrackKind::Video, start, TimeRange::new(Tick::ZERO, duration), rate)
                .ok_or_else(|| error("graphic factory refused"))?;
            let layer = content.prepared(s)?;
            c.name = match &layer.params.get("text").map(|p| &p.value) {
                Some(filmcraft_project::ParamValue::Text(t)) => t.lines().next().filter(|t| !t.trim().is_empty()).unwrap_or("Graphic").to_string(),
                _ => "Shape".into(),
            };
            c.effects.push(layer);
            for e in &mut c.effects {
                filmcraft_project::resolve_auto_points(e, (w, h), (w, h));
            }
            let id = c.id;
            expected.alloc_id(); // The shared placement service allocates its potential track id.
            let tr = expected.sequence_mut(sid).and_then(|q| q.video_tracks.get_mut(ti)).ok_or_else(|| error("expected destination missing"))?;
            tr.items.push(c);
            tr.sort();
            let (command, mut args) = content.human();
            args["track"] = json!(ti);
            args["time"] = json!(start.0);
            args["durationFrames"] = json!(duration_frames);
            (command, args, id)
        }
        Edit::AddLayer { target, content } => {
            let c = clip(s, target)?;
            if c.effects.len() >= 4096 {
                return Err(error("graphic layer stack is full"));
            }
            let mut layer = content.prepared(s)?;
            let q = s.active_sequence().ok_or(EngineError::NoSequence)?;
            let frame = (q.settings.width, q.settings.height);
            filmcraft_project::resolve_auto_points(&mut layer, frame, frame);
            let pos = graphic::layer_indices(&c.effects).last().map_or(c.effects.len(), |i| i + 1);
            expected.sequence_mut(sid).and_then(|q| q.find_item_mut(c.id)).ok_or_else(|| error("expected graphic missing"))?.1.effects.insert(pos, layer);
            let (command, mut args) = content.human();
            args["clip"] = json!(target.clip);
            (command, args, c.id)
        }
        Edit::Set { target, layer_id, frame, props } => {
            let c = clip(s, target)?;
            let (l, ei) = layer(c, layer_id)?;
            let at = clock(s, *frame)?;
            if at < c.start || at >= c.end() {
                return Err(error("explicit frame is outside current graphic"));
            }
            let e = c.effects.get(ei).ok_or_else(|| error("layer missing"))?;
            properties(e, props)?;
            let source_time = c.source_time_at(at);
            if !(-SAFE..=SAFE).contains(&source_time.0) {
                return Err(error("graphic source time exceeds exact transport"));
            }
            let dst = expected
                .sequence_mut(sid)
                .and_then(|q| q.find_item_mut(c.id))
                .and_then(|(_, c)| c.effects.get_mut(ei))
                .ok_or_else(|| error("expected layer missing"))?;
            crate::graphics::apply_props(dst, props.as_object().ok_or_else(|| error("properties missing"))?, source_time)?;
            changed_layer = Some(layer_id.clone());
            ("graphics.set", json!({"clip":target.clip,"layer":l,"props":props,"time":at.0,"merge":false,"begin":false}), c.id)
        }
        Edit::DeleteLayer { target, layer_id } => {
            let c = clip(s, target)?;
            let (l, ei) = layer(c, layer_id)?;
            expected.sequence_mut(sid).and_then(|q| q.find_item_mut(c.id)).ok_or_else(|| error("expected graphic missing"))?.1.effects.remove(ei);
            changed_layer = Some(layer_id.clone());
            ("graphics.deleteLayer", json!({"clip":target.clip,"layer":l}), c.id)
        }
        Edit::ReorderLayer { target, layer_id, before_layer_id } => {
            let c = clip(s, target)?;
            let (from, _) = layer(c, layer_id)?;
            let count = graphic::layer_indices(&c.effects).len();
            let to = match before_layer_id {
                Some(id) => {
                    let (to, _) = layer(c, id)?;
                    if to > from { to - 1 } else { to }
                }
                None => count.saturating_sub(1),
            };
            let dst = &mut expected.sequence_mut(sid).and_then(|q| q.find_item_mut(c.id)).ok_or_else(|| error("expected graphic missing"))?.1.effects;
            crate::graphics::reorder_layers(dst, from, to)?;
            changed_layer = Some(layer_id.clone());
            ("graphics.arrangeLayer", json!({"clip":target.clip,"layer":from,"to":to}), c.id)
        }
    };
    let before_total = clips(s)?.len();
    s.execute(command, args)?;
    let actual = s.active_sequence().and_then(|q| q.find_item(target_clip)).ok_or_else(|| error("graphic disappeared"))?.1;
    let dst = expected.sequence_mut(sid).and_then(|q| q.find_item_mut(target_clip)).ok_or_else(|| error("expected graphic disappeared"))?.1;
    if dst.identity.is_none() {
        let identity = actual
            .identity
            .as_ref()
            .filter(|i| ClipIdentity::valid_id(&i.id) && i.parent.is_none() && i.annotations.is_empty() && i.protection.is_none())
            .ok_or_else(|| error("new graphic identity is missing or inherited"))?;
        dst.identity = Some(identity.clone());
    }
    assign_expected_ids(&mut dst.effects, &actual.effects)?;
    let mut ids = BTreeSet::new();
    if dst.effects.iter().any(|e| e.instance_id.as_ref().is_none_or(|id| !ids.insert(id))) {
        return Err(error("graphic effect identities are missing or reused"));
    }
    if project_audit::bytes(&s.project)? != project_audit::bytes(&expected)? {
        return Err(error("human graphic gesture changed unexpected project state; staging discarded"));
    }
    let after = digest(s)?;
    if after == r.before_graphics_sha256 {
        return Err(error("graphic gesture has no authored change"));
    }
    Ok(json!({"beforeGraphicsSha256":r.before_graphics_sha256,"afterGraphicsSha256":after,
        "beforeCount":before_total,"afterCount":clips(s)?.len(),"clip":target_clip.0,
        "beforeEffectTracking":true,"afterEffectTracking":true,
        "identity":actual.identity.as_ref().map(|i|&i.id),"changedLayerId":changed_layer,"humanCommand":command}))
}
