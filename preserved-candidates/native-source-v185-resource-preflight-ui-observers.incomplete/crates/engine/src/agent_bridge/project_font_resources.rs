//! Whole-project font proposal/resource binding through original native services.
//! Every authored target is included; sampling instants are explicit and never all-frame proof.
use super::{graphics, parse, project_audit, project_font_replacement, project_plane, sequences};
use crate::{Result, Session, commands::bad, project_workspace::ProjectScope};
use filmcraft_project::{ClipIdentity, EffectInstance, ItemId, Project, TrackItem, graphic};
use filmcraft_time::Tick;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
pub(super) const PLAN: &str = "agent.planProjectFontResources";
const MAX_SAMPLES: usize = 100_000;
fn error(s: impl Into<String>) -> crate::EngineError {
    bad(PLAN, s)
}
fn hash(v: &impl Serialize) -> Result<String> {
    Ok(format!("{:x}", Sha256::digest(project_audit::value_bytes(v)?)))
}
fn feed(h: &mut Sha256, v: &Value) -> Result<()> {
    let b = project_audit::value_bytes(v)?;
    h.update((b.len() as u64).to_le_bytes());
    h.update(b);
    Ok(())
}
fn number(v: &Value, key: &str) -> Result<u64> {
    v[key].as_u64().filter(|n| (1..=9_007_199_254_740_991).contains(n)).ok_or_else(|| error("exact bounded native collateral identity missing"))
}
fn index(v: &Value, key: &str) -> Result<usize> {
    v[key].as_u64().and_then(|n| usize::try_from(n).ok()).ok_or_else(|| error("native collateral index missing"))
}
#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Query {
    token: String,
    scope: ProjectScope,
    before_project_sha256: String,
    replacement: Value,
    offset: usize,
    limit: usize,
}
fn owned(project: &Project, sid: ItemId, scope: &ProjectScope) -> Result<bool> {
    let a = crate::editorial_workspace::association(project)?;
    match a {
        Some(a) if a.workspace_id == scope.workspace_id && a.scope.project_id == scope.project_id => sequences::belongs(project, sid, &a.scope),
        _ => Ok(false),
    }
}
fn editable_clip(project: &Project, sid: ItemId, locked: bool, c: &TrackItem, e: &EffectInstance, sequence_owned: bool) -> Result<Vec<&'static str>> {
    let mut reasons = Vec::new();
    if locked {
        reasons.push("locked_video_track");
    }
    if !sequence_owned {
        reasons.push("sequence_outside_owning_workspace");
    }
    if c.identity.as_ref().is_none_or(|id| !ClipIdentity::valid_id(&id.id)) {
        reasons.push("missing_clip_identity");
    }
    if c.identity.as_ref().is_some_and(|id| id.protection.is_some()) {
        reasons.push("protected_clip");
    }
    if e.instance_id.as_deref().is_none_or(|id| !ClipIdentity::valid_id(id)) {
        reasons.push("missing_layer_identity");
    }
    if project.item(sid).and_then(|i| i.metadata.get(filmcraft_project::clip_identity::EFFECT_TRACKING_KEY)).is_none_or(|v| v != "1") {
        reasons.push("untracked_sequence_effects");
    }
    Ok(reasons)
}
fn cached_owned(project: &Project, sid: ItemId, scope: &ProjectScope, cache: &mut BTreeMap<ItemId, bool>) -> Result<bool> {
    if let Some(found) = cache.get(&sid) {
        return Ok(*found);
    }
    let found = owned(project, sid, scope)?;
    cache.insert(sid, found);
    Ok(found)
}
fn member_index(
    project: &Project,
    masters: &BTreeSet<ItemId>,
    scope: &ProjectScope,
    cache: &mut BTreeMap<ItemId, bool>,
    check: &dyn Fn() -> Result<()>,
) -> Result<BTreeMap<ItemId, Vec<Value>>> {
    let mut out: BTreeMap<ItemId, Vec<Value>> = BTreeMap::new();
    let mut count = 0usize;
    for item in project.sequences() {
        check()?;
        let seq = project.sequence(item.id).ok_or_else(|| error("shared sequence missing"))?;
        for track in &seq.video_tracks {
            for c in &track.items {
                check()?;
                if !masters.contains(&c.item) {
                    continue;
                }
                count = count.checked_add(1).filter(|n| *n <= MAX_SAMPLES).ok_or_else(|| error("complete shared instance inventory exceeds bound"))?;
                let master = project.source_graphics.get(&c.item).ok_or_else(|| error("shared master missing"))?;
                let layers = c.effects.iter().filter(|e| graphic::is_layer(e)).collect::<Vec<_>>();
                let consistent = project_audit::value_bytes(&(&layers, c.graphic.as_deref()))? == project_audit::value_bytes(&(&master.layers, &master.meta))?;
                let mut reasons = Vec::new();
                for e in c.effects.iter().filter(|e| e.effect == graphic::TEXT_LAYER) {
                    reasons.extend(editable_clip(project, item.id, track.locked, c, e, cached_owned(project, item.id, scope, cache)?)?);
                }
                if !consistent {
                    reasons.push("source_master_instance_drift");
                }
                reasons.sort_unstable();
                reasons.dedup();
                out.entry(c.item).or_default().push(json!({"sequence":item.id.0,"track":track.id.0,"clip":c.id.0,"identity":c.identity.as_ref().map(|i|&i.id),"consistent":consistent,"eligible":reasons.is_empty(),"reasons":reasons}));
            }
        }
    }
    Ok(out)
}
struct Stream {
    started: web_time::Instant,
    before: Sha256,
    after: Sha256,
    sampling: Sha256,
    eligibility: Sha256,
    total: usize,
    eligible: bool,
    offset: usize,
    limit: usize,
    page: Vec<Value>,
}
impl Stream {
    fn new(offset: usize, limit: usize, started: web_time::Instant) -> Self {
        let mut h = Sha256::new();
        h.update(b"poema-project-font-samples-v1");
        let mut s = Sha256::new();
        s.update(b"poema-project-font-sampling-v1");
        let mut e = Sha256::new();
        e.update(b"poema-project-font-eligibility-v1");
        Self { started, before: h.clone(), after: h, sampling: s, eligibility: e, total: 0, eligible: true, offset, limit, page: Vec::new() }
    }
    fn add(&mut self, collateral: usize, target: Value, sampling: Value, before: Value, after: Value, reasons: Vec<&'static str>, scope: Value) -> Result<()> {
        if self.started.elapsed().as_millis() >= super::media_guard::OPERATION_DEADLINE_MS as u128 {
            return Err(error("original native operation budget expired during complete resource planning"));
        }
        if self.total >= MAX_SAMPLES {
            return Err(error("complete native renderer samples exceed100000; nothing truncated"));
        }
        let eligible = reasons.is_empty() && scope["eligible"] != false;
        let basis = json!({"collateralPosition":collateral,"target":target,"sampling":sampling});
        feed(&mut self.sampling, &basis)?;
        feed(&mut self.before, &json!({"sample":basis,"font":before}))?;
        feed(&mut self.after, &json!({"sample":basis,"font":after}))?;
        let eligibility = json!({"sample":basis,"eligible":eligible,"reasons":reasons,"scope":scope});
        feed(&mut self.eligibility, &eligibility)?;
        self.eligible &= eligible;
        if self.total >= self.offset && self.page.len() < self.limit {
            self.page.push(json!({"position":self.total,"collateralPosition":collateral,"target":target,"sampling":sampling,"before":before,"after":after,"eligibility":eligibility}));
        }
        self.total += 1;
        Ok(())
    }
}
fn samples(
    before: &Project,
    after: &Project,
    rows: &[Value],
    scope: &ProjectScope,
    offset: usize,
    limit: usize,
    started: web_time::Instant,
    observe: bool,
    check: &dyn Fn() -> Result<()>,
) -> Result<Stream> {
    let mut masters = BTreeSet::new();
    for row in rows {
        let l = &row["locator"];
        if l["kind"] == "source_master" {
            masters.insert(ItemId(number(l, "item")?));
        }
    }
    let mut before_ownership = BTreeMap::new();
    let mut after_ownership = BTreeMap::new();
    let members = member_index(before, &masters, scope, &mut before_ownership, check)?;
    let next_members = member_index(after, &masters, scope, &mut after_ownership, check)?;
    let mut out = Stream::new(offset, limit, started);
    for (position, row) in rows.iter().enumerate() {
        check()?;
        let l = &row["locator"];
        if l["kind"] == "source_master" {
            let id = ItemId(number(l, "item")?);
            let layer = index(l, "layerPosition")?;
            let b = before.source_graphics.get(&id).and_then(|g| g.layers.get(layer)).ok_or_else(|| error("source master layer missing"))?;
            let a = after.source_graphics.get(&id).and_then(|g| g.layers.get(layer)).ok_or_else(|| error("predicted source master missing"))?;
            let empty = Vec::new();
            let bm = members.get(&id).unwrap_or(&empty);
            let am = next_members.get(&id).unwrap_or(&empty);
            let eligible = bm.iter().chain(am.iter()).all(|m| m["eligible"] == true);
            let shared = json!({"eligible":eligible,"memberCount":bm.len(),"beforeMembersSha256":hash(bm)?,"afterMembersSha256":hash(am)?,"referenceScope":"complete_current_project_source_instances"});
            out.add(
                position + 1,
                l.clone(),
                json!({"kind":"source_master_origin","nativeGraphicTimeTicks":"0"}),
                if observe { graphics::observe_project_master_font(before, id, b, Tick(0))? } else { Value::Null },
                if observe { graphics::observe_project_master_font(after, id, a, Tick(0))? } else { Value::Null },
                vec![],
                shared,
            )?;
        } else {
            let sid = ItemId(number(l, "sequence")?);
            let track = number(l, "track")?;
            let bq = before.sequence(sid).ok_or_else(|| error("sequence missing"))?;
            let aq = after.sequence(sid).ok_or_else(|| error("predicted sequence missing"))?;
            bq.settings.validate().map_err(error)?;
            if row["kind"] == "text_layer" {
                let cid = number(l, "clip")?;
                let bt = bq.video_tracks.iter().find(|t| t.id.0 == track).ok_or_else(|| error("video track missing"))?;
                let at = aq.video_tracks.iter().find(|t| t.id.0 == track).ok_or_else(|| error("predicted video track missing"))?;
                let bc = bt.items.iter().find(|c| c.id.0 == cid).ok_or_else(|| error("clip missing"))?;
                let ac = at.items.iter().find(|c| c.id.0 == cid).ok_or_else(|| error("predicted clip missing"))?;
                let layer = index(l, "layerPosition")?;
                let be = bc.effects.get(layer).ok_or_else(|| error("text layer missing"))?;
                let ae = ac.effects.get(layer).ok_or_else(|| error("predicted layer missing"))?;
                let reasons = editable_clip(before, sid, bt.locked, bc, be, cached_owned(before, sid, scope, &mut before_ownership)?)?;
                out.add(
                    position + 1,
                    l.clone(),
                    json!({"kind":"exact_clip_entry","timelineTicks":bc.start.0.to_string()}),
                    if observe { graphics::observe_project_graphic_font(before, bc, be, bc.start)? } else { Value::Null },
                    if observe { graphics::observe_project_graphic_font(after, ac, ae, bc.start)? } else { Value::Null },
                    reasons,
                    Value::Null,
                )?;
            } else {
                let bt = bq.caption_tracks.iter().find(|t| t.id.0 == track).ok_or_else(|| error("caption track missing"))?;
                let at = aq.caption_tracks.iter().find(|t| t.id.0 == track).ok_or_else(|| error("predicted caption track missing"))?;
                let mut reasons = Vec::new();
                if bt.locked {
                    reasons.push("locked_caption_track");
                }
                if !cached_owned(before, sid, scope, &mut before_ownership)? {
                    reasons.push("sequence_outside_owning_workspace");
                }
                let (w, h) = (bq.settings.width as usize, bq.settings.height as usize);
                if bt.captions.is_empty() {
                    out.add(
                        position + 1,
                        l.clone(),
                        json!({"kind":"empty_caption_track_declared_style","caption":null}),
                        if observe { graphics::observe_project_caption_style(&bt.style, w, h)? } else { Value::Null },
                        if observe { graphics::observe_project_caption_style(&at.style, w, h)? } else { Value::Null },
                        reasons,
                        Value::Null,
                    )?;
                } else {
                    if bt.captions.len() != at.captions.len() {
                        return Err(error("predicted caption structure changed"));
                    }
                    for (index, c) in bt.captions.iter().enumerate() {
                        check()?;
                        let a = at.captions.get(index).filter(|v| v.id == c.id).ok_or_else(|| error("predicted caption cue missing or reordered"))?;
                        out.add(position+1,l.clone(),json!({"kind":"one_exact_caption_canvas","caption":c.id.0,"startTicks":c.start.0.to_string(),"durationTicks":c.duration.0.to_string()}),if observe { graphics::observe_caption_fonts(c,&bt.style,w,h)? } else { Value::Null },if observe { graphics::observe_caption_fonts(a,&at.style,w,h)? } else { Value::Null },reasons.clone(),Value::Null)?;
                    }
                }
            }
        }
    }
    Ok(out)
}
pub(super) fn plan(s: &mut Session, p: &Value) -> Result<Value> {
    let prepared = prepare(s, "replacement_plan", p)?;
    let computed = prepared.compute(&|| Ok(()))?;
    prepared.live(s)?;
    Ok(computed.value)
}
fn compute_plan(b: &Prepared, q: &Query, check: &dyn Fn() -> Result<()>) -> Result<Computed> {
    check()?;
    let started = b.started;
    let before = project_plane::project_basis(&b.project)?;
    if before != q.before_project_sha256 {
        return Err(error("complete project changed; read again"));
    }
    let (predicted, replaced, rows, totals) = project_font_replacement::prediction(&b.project, &q.replacement)?;
    let after = hash(&predicted)?;
    let collateral = hash(&rows)?;
    let proposal = hash(&("project_font_resource_proposal_v1", &q.token, &q.scope, &before, &q.replacement))?;
    check()?;
    for _ in 0..2 {
        let generation = filmcraft_text::fonts::registry_revision();
        let stream = samples(&b.project, &predicted, &rows, &q.scope, q.offset, q.limit, started, true, check)?;
        if filmcraft_text::fonts::registry_revision() != generation {
            continue;
        }
        if q.offset > stream.total {
            return Err(error("resource sample offset outside complete collection"));
        }
        let proof = json!({"version":1,"readToken":q.token,"proposalSha256":proposal,"beforeProjectSha256":before,"afterProjectSha256":after,"collateralSha256":collateral,"beforeResourcesSha256":format!("{:x}",stream.before.finalize()),"afterResourcesSha256":format!("{:x}",stream.after.finalize()),"samplingSha256":format!("{:x}",stream.sampling.finalize()),"eligibilitySha256":format!("{:x}",stream.eligibility.finalize())});
        let end = q.offset.saturating_add(stream.page.len());
        let result = json!({"status":"planned","kind":"native_project_font_resource_plan","version":1,"token":q.token,"scope":q.scope,"revision":b.revision,"sequence":b.sequence.map(|id|id.0),"replacement":q.replacement,"fontPlan":proof,"collateralTotal":rows.len(),"totals":totals,"humanCommand":"file.replaceFonts","humanResult":{"replaced":replaced},"changesAuthoredState":before!=after,"writePreconditionsSatisfied":stream.eligible&&before!=after,"offset":q.offset,"total":stream.total,"nextOffset":if end<stream.total{Some(end)}else{None},"rows":stream.page,"rendererSampling":"all_authored_targets_at_explicit_clip_entry_master_origin_and_each_caption_canvas","allFramesVerified":false,"sourceValidation":"not-performed","fontFileState":"not-inspected","embeddingRights":"not-established","writeAuthority":false});
        if project_audit::value_bytes(&result)?.len() > 64 * 1024 {
            return Err(error("complete resource page exceeds64KiB; request fewer rows"));
        }
        check()?;
        return Ok(Computed {
            value: result,
            #[cfg(not(target_arch = "wasm32"))]
            registry: generation,
        });
    }
    Err(error("native font registry changed across complete project plan; refresh"))
}

pub(super) struct Binding {
    pub(super) proof: Value,
    pub(super) predicted: Project,
    pub(super) human_result: Value,
    pub(super) totals: Value,
    pub(super) eligible: bool,
}
pub(super) fn binding(
    project: &Project,
    token: &str,
    scope: &ProjectScope,
    replacement: &Value,
    observe: bool,
    check: &dyn Fn() -> Result<()>,
) -> Result<Binding> {
    if crate::project_workspace::identity(project)?.is_none_or(|i| i.scope != *scope) {
        return Err(error("font binding requires the exact existing Root workspace"));
    }
    let before = hash(project)?;
    let (predicted, replaced, rows, totals) = project_font_replacement::prediction(project, replacement)?;
    let after = hash(&predicted)?;
    let collateral = hash(&rows)?;
    for _ in 0..2 {
        let generation = if observe { filmcraft_text::fonts::registry_revision() } else { 0 };
        check()?;
        let stream = samples(project, &predicted, &rows, scope, 0, 0, web_time::Instant::now(), observe, check)?;
        if observe && filmcraft_text::fonts::registry_revision() != generation {
            continue;
        }
        let proof = json!({"version":1,"readToken":token,"proposalSha256":hash(&("project_font_resource_proposal_v1",token,scope,&before,replacement))?,"beforeProjectSha256":before,"afterProjectSha256":after,"collateralSha256":collateral,"beforeResourcesSha256":format!("{:x}",stream.before.finalize()),"afterResourcesSha256":format!("{:x}",stream.after.finalize()),"samplingSha256":format!("{:x}",stream.sampling.finalize()),"eligibilitySha256":format!("{:x}",stream.eligibility.finalize())});
        return Ok(Binding { proof, predicted, human_result: json!({"replaced":replaced}), totals, eligible: stream.eligible && before != after });
    }
    Err(error("native fonts changed across complete global binding"))
}

pub(super) const READ: &str = "agent.readProjectFontResources";
#[derive(Clone, Copy, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum Side {
    Before,
    After,
}
#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Readback {
    token: String,
    scope: ProjectScope,
    commit: String,
    side: Side,
    offset: usize,
    limit: usize,
}
/// Fresh renderer observations for the original operation's complete target set.
/// The saved graph only locates targets; it cannot renew old resources or history ownership.
pub(super) fn read(s: &mut Session, p: &Value) -> Result<Value> {
    let prepared = prepare(s, "operation_readback", p)?;
    let computed = prepared.compute(&|| Ok(()))?;
    prepared.live(s)?;
    Ok(computed.value)
}
fn compute_read(b: &Prepared, q: &Readback, original: &Original, check: &dyn Fn() -> Result<()>) -> Result<Computed> {
    check()?;
    let started = b.started;
    let request = &original.request;
    let receipt = &original.receipt;
    let before = &original.before;
    let after = &original.after;
    let batch: super::Batch = parse(READ, request)?;
    if batch.token != q.commit || receipt["token"] != q.commit || receipt["status"] != "committed" || receipt["validated"] != true {
        return Err(error("original native font correlation differs"));
    }
    let results = receipt["results"].as_array().ok_or_else(|| error("original native results missing"))?;
    super::project_font_write::document_transition(before, after, &batch, results)?;
    let op = batch.operations.first().ok_or_else(|| error("original font operation missing"))?;
    let result = results.first().ok_or_else(|| error("original font result missing"))?;
    let (_, _, rows, _) = project_font_replacement::prediction(before, &op.params["replacement"])?;
    let collateral = hash(&rows)?;
    if result["collateralSha256"] != collateral {
        return Err(error("complete original target inventory differs"));
    }
    let current = project_plane::project_basis(&b.project)?;
    let expected = match q.side {
        Side::Before => &result["beforeSha256"],
        Side::After => &result["afterSha256"],
    };
    if expected != &json!(current) {
        return Err(error("current complete authored project differs from requested original side; take a fresh plan"));
    }
    let historical = match q.side {
        Side::Before => &result["fontResources"]["beforeResourcesSha256"],
        Side::After => &result["fontResources"]["afterResourcesSha256"],
    };
    check()?;
    for _ in 0..2 {
        let generation = filmcraft_text::fonts::registry_revision();
        let stream = samples(&b.project, &b.project, &rows, &q.scope, q.offset, q.limit, started, true, check)?;
        if filmcraft_text::fonts::registry_revision() != generation {
            continue;
        }
        if q.offset > stream.total {
            return Err(error("readback offset outside complete original target samples"));
        }
        let resource = format!("{:x}", stream.before.finalize());
        let sampling = format!("{:x}", stream.sampling.finalize());
        if result["fontResources"]["samplingSha256"] != sampling {
            return Err(error("current original target sampling differs"));
        }
        let end = q.offset.saturating_add(stream.page.len());
        let page = stream.page.into_iter().map(|row| json!({"position":row["position"],"collateralPosition":row["collateralPosition"],"target":row["target"],"sampling":row["sampling"],"resources":row["before"],"eligibility":row["eligibility"]})).collect::<Vec<_>>();
        let out = json!({"status":"read","kind":"native_project_font_resource_readback","version":1,"token":q.token,"scope":q.scope,"revision":b.revision,"sequence":b.sequence.map(|id|id.0),"commit":q.commit,"side":q.side,"projectSha256":current,"collateralSha256":collateral,"resourcesSha256":resource,"samplingSha256":sampling,"eligibilitySha256":format!("{:x}",stream.eligibility.finalize()),"historicalResourcesSha256":historical,"matchesHistoricalResources":historical==&json!(resource),"collateralTotal":rows.len(),"offset":q.offset,"total":stream.total,"nextOffset":if end<stream.total{Some(end)}else{None},"rows":page,"referenceScope":"complete_original_font_operation_targets_in_exact_current_authored_side","rendererSampling":"all_authored_targets_at_explicit_clip_entry_master_origin_and_each_caption_canvas","allFramesVerified":false,"sourceValidation":"not-performed","fontFileState":"not-inspected","embeddingRights":"not-established","historyOwnership":"not-established","writeAuthority":false,"projectUnchanged":true,"historyUnchanged":true});
        if project_audit::value_bytes(&out)?.len() > 64 * 1024 {
            return Err(error("complete current resource page exceeds64KiB; request fewer rows"));
        }
        check()?;
        return Ok(Computed {
            value: out,
            #[cfg(not(target_arch = "wasm32"))]
            registry: generation,
        });
    }
    Err(error("native fonts changed during fresh current resource readback; refresh"))
}

#[derive(Clone)]
struct Original {
    before: std::sync::Arc<Project>,
    after: std::sync::Arc<Project>,
    request: Value,
    receipt: Value,
}
#[derive(Clone)]
enum Input {
    Plan(Query),
    Read(Readback, Original),
}
#[derive(Clone)]
pub(super) struct Prepared {
    project: std::sync::Arc<Project>,
    revision: u64,
    sequence: Option<ItemId>,
    path: Option<String>,
    started: web_time::Instant,
    input: Input,
}
#[derive(Clone)]
pub(super) struct Computed {
    pub(super) value: Value,
    #[cfg(not(target_arch = "wasm32"))]
    pub(super) registry: usize,
}
fn original(s: &Session, q: &Readback) -> Result<Original> {
    let t = s
        .agent_bridge
        .tickets
        .iter()
        .find(|t| t.token == q.commit)
        .filter(|t| super::project_font_write::is_commit(t) && t.project_scope.as_ref() == Some(&q.scope) && t.source_library.is_none() && t.path == s.path)
        .ok_or_else(|| error("exact retained original global font operation in this Root is required; no receipt-only target authority"))?;
    if super::document_authority::committed(t)? != super::document_authority::Authority::DocumentState {
        return Err(error("original font operation lacks exact authored document authority"));
    }
    let (request, receipt) = t.committed.as_ref().ok_or_else(|| error("original native receipt missing"))?;
    Ok(Original {
        before: t.project.upgrade().ok_or_else(|| error("original before-project no longer retained"))?,
        after: t.after.as_ref().and_then(std::sync::Weak::upgrade).ok_or_else(|| error("original after-project no longer retained"))?,
        request: request.clone(),
        receipt: receipt.clone(),
    })
}
pub(super) fn prepare(s: &Session, kind: &str, p: &Value) -> Result<Prepared> {
    let started = web_time::Instant::now();
    let input = match kind {
        "replacement_plan" => {
            let q: Query = parse(PLAN, p)?;
            project_plane::validate_ticket(s, &q.token, &q.scope)?;
            if q.offset > MAX_SAMPLES || !(1..=16).contains(&q.limit) {
                return Err(error("explicit bounded project renderer page required"));
            }
            Input::Plan(q)
        }
        "operation_readback" => {
            let q: Readback = parse(READ, p)?;
            project_plane::validate_ticket(s, &q.token, &q.scope)?;
            if !super::token_ok(&q.commit) || q.token == q.commit || q.offset > MAX_SAMPLES || !(1..=16).contains(&q.limit) {
                return Err(error("fresh Root read, distinct retained font commit, explicit side and bounded page required"));
            }
            let o = original(s, &q)?;
            Input::Read(q, o)
        }
        _ => return Err(error("only exact replacement_plan or operation_readback is supported")),
    };
    Ok(Prepared { project: s.project.clone(), revision: s.revision, sequence: s.state.active_sequence, path: s.path.clone(), started, input })
}
impl Prepared {
    pub(super) fn token(&self) -> &str {
        match &self.input {
            Input::Plan(q) => &q.token,
            Input::Read(q, _) => &q.token,
        }
    }
    pub(super) fn live(&self, s: &Session) -> Result<()> {
        let scope = match &self.input {
            Input::Plan(q) => &q.scope,
            Input::Read(q, _) => &q.scope,
        };
        project_plane::validate_ticket(s, self.token(), scope)?;
        if self.revision != s.revision || self.sequence != s.state.active_sequence || self.path != s.path || !std::sync::Arc::ptr_eq(&self.project, &s.project)
        {
            return Err(error("native project/context changed during font work; read again"));
        }
        if let Input::Read(q, expected) = &self.input {
            let current = original(s, q)?;
            if !std::sync::Arc::ptr_eq(&current.before, &expected.before)
                || !std::sync::Arc::ptr_eq(&current.after, &expected.after)
                || current.request != expected.request
                || current.receipt != expected.receipt
            {
                return Err(error("retained original font operation changed during readback"));
            }
        }
        Ok(())
    }
    pub(super) fn compute(&self, check: &dyn Fn() -> Result<()>) -> Result<Computed> {
        check()?;
        match &self.input {
            Input::Plan(q) => compute_plan(self, q, check),
            Input::Read(q, original) => compute_read(self, q, original, check),
        }
    }
}
