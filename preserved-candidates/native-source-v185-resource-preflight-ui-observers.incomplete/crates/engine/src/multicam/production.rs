//! Shared preparation for the three original synchronization/source-production gestures.
//! Preparation calls the existing sync/decoder functions once, without publishing a project.
//! This is an in-memory human service, NOT a media witness, bridge ticket or write grant.
//! A future agent purpose must additionally qualify original media, Stop and owned publication.
use super::*;
use std::sync::Weak;
mod history;
pub(crate) use history::validate_computed_transition;
fn checked_add(a: Tick, b: Tick) -> Result<Tick> {
    a.0.checked_add(b.0).map(Tick).ok_or_else(|| bad("multicam.geometry", "time addition overflow"))
}
fn checked_sub(a: Tick, b: Tick) -> Result<Tick> {
    a.0.checked_sub(b.0).map(Tick).ok_or_else(|| bad("multicam.geometry", "time subtraction overflow"))
}
fn finite_tick(v: f64) -> Result<Tick> {
    if !v.is_finite() || v < i64::MIN as f64 || v >= i64::MAX as f64 {
        return Err(bad("multicam.geometry", "scaled time is outside exact native range"));
    }
    Ok(Tick(v as i64))
}
fn computed_anchors(
    s: &Session,
    clips: &[SyncClip],
    reference: usize,
    method: &Method,
    offset: Tick,
    observed: Option<&[history::Row]>,
    rate: FrameRate,
) -> Result<Vec<sync::Anchor>> {
    match observed {
        None => sync::anchors(s, clips, reference, method, offset),
        Some(rows) => history::anchors(clips, reference, method, offset, rows, rate),
    }
}

struct Basis {
    project: Weak<Project>,
    media: Weak<MediaPool>,
    revision: u64,
    saved_revision: u64,
    path: Option<String>,
    editor: Value,
    preferences: Value,
    multicam_pass: u64,
    gestures: (bool, bool, bool),
}
impl Basis {
    fn capture(s: &Session) -> Result<Self> {
        Ok(Self {
            project: Arc::downgrade(&s.project),
            media: Arc::downgrade(&s.media),
            revision: s.revision,
            saved_revision: s.saved_revision,
            path: s.path.clone(),
            editor: serde_json::to_value(&s.state).map_err(|e| bad("multicam.prepare", e.to_string()))?,
            preferences: serde_json::to_value(&s.prefs).map_err(|e| bad("multicam.prepare", e.to_string()))?,
            multicam_pass: s.mcrec.pass,
            gestures: (s.mcrec.active(), s.mixrec.active(), s.trim_play.active()),
        })
    }
    fn validate(&self, s: &Session) -> Result<()> {
        if !self.project.ptr_eq(&Arc::downgrade(&s.project))
            || !self.media.ptr_eq(&Arc::downgrade(&s.media))
            || self.revision != s.revision
            || self.saved_revision != s.saved_revision
            || self.path != s.path
            || self.editor != serde_json::to_value(&s.state).map_err(|e| bad("multicam.prepare", e.to_string()))?
            || self.preferences != serde_json::to_value(&s.prefs).map_err(|e| bad("multicam.prepare", e.to_string()))?
            || self.multicam_pass != s.mcrec.pass
            || self.gestures != (s.mcrec.active(), s.mixrec.active(), s.trim_play.active())
        {
            return Err(bad("multicam.prepare", "native project, editor, source pool or gesture changed; prepare again without replay"));
        }
        Ok(())
    }
}

struct Synchronize {
    seq_id: ItemId,
    method: Method,
    moved: usize,
    placements: Vec<(TrackId, TrackItem)>,
    remove: Vec<ClipId>,
    report: Value,
    reference: usize,
}
struct Merge {
    clips: Vec<SyncClip>,
    settings: SequenceSettings,
    starts: Vec<Tick>,
    method: Method,
    name: String,
    video: Option<ItemId>,
    vid_audio: bool,
    audio_items: Vec<usize>,
    n_audio: usize,
    report: Value,
    bin: Option<filmcraft_project::BinId>,
}
struct CreateMulticam {
    clips: Vec<SyncClip>,
    settings: SequenceSettings,
    starts: Vec<Tick>,
    method: Method,
    audio: MulticamAudio,
    name: String,
    move_processed: bool,
    flags: Vec<(bool, bool)>,
    names: Vec<String>,
    nv: usize,
    na: usize,
    report: Value,
}
enum Edit {
    Synchronize(Synchronize),
    Merge(Merge),
    CreateMulticam(CreateMulticam),
}
/// Opaque, single-consumption prepared native gesture. Never serialized or reconstituted.
pub(crate) struct Prepared {
    basis: Basis,
    edit: Edit,
}
impl Prepared {
    /// Same original human commit/history function; no staging, ownership or file checks bypassed.
    pub(crate) fn apply(self, s: &mut Session) -> Result<Value> {
        self.basis.validate(s)?;
        match self.edit {
            Edit::Synchronize(p) => apply_synchronize(s, p),
            Edit::Merge(p) => apply_merge(s, p),
            Edit::CreateMulticam(p) => apply_multicam(s, p),
        }
    }
}
/// The caller supplies one of the three original human command ids. Parameters keep all
/// original aliases/defaults. This function grants no agent authority and performs no publication.
pub(crate) fn prepare(s: &Session, command: &str, params: &Value) -> Result<Prepared> {
    history::numeric(&s.project, 2048)?;
    let basis = Basis::capture(s)?;
    let edit = match command {
        "clip.synchronize" => Edit::Synchronize(prepare_synchronize(s, params)?),
        "clip.mergeClips" => Edit::Merge(prepare_merge(s, params)?),
        "clip.createMulticam" => Edit::CreateMulticam(prepare_multicam(s, params)?),
        _ => return Err(bad("multicam.prepare", "unsupported source-production gesture")),
    };
    basis.validate(s)?;
    Ok(Prepared { basis, edit })
}

fn prepare_synchronize(s: &Session, p: &Value) -> Result<Synchronize> {
    prepare_synchronize_observed(s, p, None)
}
fn prepare_synchronize_observed(s: &Session, p: &Value, observed: Option<&[history::Row]>) -> Result<Synchronize> {
    let cmd = "clip.synchronize";
    let method = method_p(p, cmd)?;
    let seq_id = s.state.active_sequence.ok_or(EngineError::NoSequence)?;
    let q = s.active_sequence().ok_or(EngineError::NoSequence)?.clone();
    let rate = q.settings.frame_rate;
    let sel = with_links(s, &clips_p(s, p)?);
    // group into link units
    let mut units: Vec<Unit> = Vec::new();
    let mut seen: Vec<ClipId> = Vec::new();
    let pos = |c: ClipId| -> Option<(TrackKind, usize, TrackItem)> {
        for (k, ts) in [(TrackKind::Video, &q.video_tracks), (TrackKind::Audio, &q.audio_tracks)] {
            for (ti, t) in ts.iter().enumerate() {
                if let Some(it) = t.item(c) {
                    return Some((k, ti, it.clone()));
                }
            }
        }
        None
    };
    for &c in &sel {
        if seen.contains(&c) {
            continue;
        }
        let Some((_, _, it)) = pos(c) else { continue };
        let members: Vec<ClipId> = match it.link {
            Some(l) => sel.iter().copied().filter(|o| pos(*o).is_some_and(|x| x.2.link == Some(l))).collect(),
            None => vec![c],
        };
        seen.extend(&members);
        let mut cands: Vec<(TrackKind, usize, TrackItem)> = members.iter().filter_map(|m| pos(*m)).collect();
        // the representative: audio for audio sync, else video; lowest track first
        let audio_first = method == Method::Audio;
        cands.sort_by_key(|(k, ti, _)| (if (*k == TrackKind::Audio) == audio_first { 0 } else { 1 }, *ti));
        let (k, ti, rep) = cands.remove(0);
        units.push(Unit { members, rep, rep_track: (k, ti) });
    }
    if units.len() < 2 {
        return Err(bad(cmd, "select clips from at least two different sources"));
    }
    // reference unit
    let reference = if let Some(c) = u64_p(p, "reference")?.map(ClipId) {
        units.iter().position(|u| u.members.contains(&c)).ok_or_else(|| bad(cmd, "`reference` is not among the clips"))?
    } else if let Some(t) = track_p(s, p, "track", cmd)? {
        units
            .iter()
            .position(|u| u.members.iter().any(|m| q.track(t).is_some_and(|tr| tr.item(*m).is_some())))
            .ok_or_else(|| bad(cmd, "no selected clip on the reference track"))?
    } else {
        (0..units.len()).min_by_key(|&i| (if units[i].rep_track.0 == TrackKind::Video { 0 } else { 1 }, units[i].rep_track.1)).unwrap_or(0)
    };
    let clips: Vec<SyncClip> = units.iter().map(|u| sync::timeline_clip(&s.project, &u.rep)).collect();
    let anchors = computed_anchors(s, &clips, reference, &method, offset_p(p, rate), observed, rate)?;
    let at = |u: &Unit, a: Tick| -> Result<Tick> {
        let delta = checked_sub(a, u.rep.source_in)?;
        let scaled = finite_tick((delta.0 as f64 / u.rep.speed.abs().max(1e-9)).round())?;
        checked_add(u.rep.start, scaled)
    };
    let reference_unit = units.get(reference).ok_or_else(|| bad(cmd, "reference missing"))?;
    let reference_anchor = anchors.get(reference).ok_or_else(|| bad(cmd, "reference anchor missing"))?;
    let t_ref = at(reference_unit, reference_anchor.anchor)?;
    let mut deltas = units.iter().zip(&anchors).map(|(u, a)| checked_sub(t_ref, at(u, a.anchor)?)).collect::<Result<Vec<_>>>()?;
    let mut min_start = None;
    for (u, d) in units.iter().zip(&deltas) {
        for m in &u.members {
            if let Some((_, _, it)) = pos(*m) {
                let start = checked_add(it.start, *d)?;
                min_start = Some(min_start.map_or(start, |old: Tick| old.min(start)));
            }
        }
    }
    if let Some(minimum) = min_start.filter(|t| *t < Tick::ZERO) {
        for d in &mut deltas {
            *d = checked_sub(*d, minimum)?;
        }
    }
    let mut moved = 0usize;
    let mut placements: Vec<(TrackId, TrackItem)> = Vec::new();
    let mut remove: Vec<ClipId> = Vec::new();
    for (u, d) in units.iter().zip(&deltas) {
        if *d == Tick::ZERO {
            continue;
        }
        moved += 1;
        // frame-aligned start, sub-frame remainder taken from the source In
        let (start, shift) = frame_align(checked_add(u.rep.start, *d)?, rate).ok_or_else(|| bad(cmd, "frame alignment overflow"))?;
        let dd = checked_sub(start, u.rep.start)?;
        for m in &u.members {
            let Some((tid, it)) = q.find_item(*m) else { continue };
            let mut it = it.clone();
            it.start = checked_add(it.start, dd)?;
            if shift > Tick::ZERO {
                it.source_in = checked_add(it.source_in, finite_tick((shift.0 as f64 * it.speed.abs()).round())?)?;
                it.duration = checked_sub(it.duration, rate.frame_duration())?.max(rate.frame_duration());
            }
            remove.push(*m);
            placements.push((tid, it));
        }
    }
    let starts = units.iter().zip(&deltas).map(|(u, d)| checked_add(u.rep.start, *d)).collect::<Result<Vec<_>>>()?;
    let rep = sync::report(&clips, &anchors, &starts, rate);
    Ok(Synchronize { seq_id, method, moved, placements, remove, report: rep, reference })
}

fn apply_synchronize(s: &mut Session, plan: Synchronize) -> Result<Value> {
    let result = synchronize_result(&plan);
    if plan.moved > 0 {
        let label = format!("Synchronize ({})", plan.method.name());
        s.edit(&label, |p, st| materialize_synchronize(p, st, plan))?;
    }
    Ok(result)
}
fn synchronize_result(plan: &Synchronize) -> Value {
    json!({"moved":plan.moved,"reference":plan.reference,"clips":plan.report})
}
fn materialize_synchronize(p: &mut Project, st: &mut crate::EditorState, plan: Synchronize) -> Result<()> {
    let budget = plan.placements.len().checked_mul(4).and_then(|v| v.checked_add(1)).ok_or_else(|| bad("clip.synchronize", "overwrite budget overflow"))?;
    p.next_id.checked_add(budget as u64).ok_or_else(|| bad("clip.synchronize", "overwrite allocator exhausted"))?;
    for (_, c) in &plan.placements {
        checked_add(c.start, c.duration)?;
        checked_add(c.source_in, finite_tick((c.duration.0 as f64 * c.speed.abs()).round())?)?;
    }
    let mut next = p.next_id;
    let seq = p.sequence_mut(plan.seq_id).ok_or(EngineError::NoSequence)?;
    let queried = std::cell::Cell::new(false);
    let duration = |_: ItemId| {
        queried.set(true);
        None
    };
    let start = |_: ItemId| {
        queried.set(true);
        Tick::ZERO
    };
    let mut ctx = EditCtx { next_id: &mut next, media_duration: &duration, media_start: &start, min_duration: seq.settings.frame_rate.frame_duration() };
    edit::delete_items(seq, &plan.remove);
    let ids = edit::overwrite(seq, plan.placements, &mut ctx)?;
    if queried.get() {
        return Err(bad("clip.synchronize", "native overwrite unexpectedly requested source observations"));
    }
    seq.check().map_err(EngineError::Other)?;
    p.next_id = next;
    st.selection = ids;
    Ok(())
}

fn prepare_merge(s: &Session, p: &Value) -> Result<Merge> {
    prepare_merge_observed(s, p, None)
}
fn prepare_merge_observed(s: &Session, p: &Value, observed: Option<&[history::Row]>) -> Result<Merge> {
    let cmd = "clip.mergeClips";
    let method = method_p(p, cmd)?;
    let clips = sync_items(s, &items_p(s, p), cmd)?;
    let has_v = |c: &SyncClip| s.project.item(c.item).is_some_and(|i| i.has_video());
    let has_a = |c: &SyncClip| s.project.item(c.item).is_some_and(|i| i.has_audio());
    let videos: Vec<usize> = (0..clips.len()).filter(|&i| has_v(&clips[i])).collect();
    if videos.len() > 1 {
        return Err(bad(cmd, "Merge Clips takes one video clip plus audio clips"));
    }
    let audio_n = clips.iter().filter(|c| !has_v(c)).count();
    if audio_n == 0 || audio_n > 16 {
        return Err(bad(cmd, "Merge Clips takes 1–16 audio clips"));
    }
    // the video clip (or the first audio clip) is the reference
    let reference = videos.first().copied().unwrap_or(0);
    let mut order: Vec<usize> = vec![reference];
    order.extend((0..clips.len()).filter(|&i| i != reference));
    let clips: Vec<SyncClip> = order.iter().map(|&i| clips[i].clone()).collect();
    let settings = settings_for(s, clips[0].item);
    let rate = settings.frame_rate;
    let anchors = computed_anchors(s, &clips, 0, &method, offset_p(p, rate), observed, rate)?;
    let starts = sync::placements_checked(&clips, &anchors, rate, false)?;
    let remove_video_audio = bool_p(p, "removeVideoAudio").unwrap_or(false);
    let name = str_p(p, "name").map(str::to_string).unwrap_or_else(|| format!("{} - Merged", clips[0].name));
    let video = videos.first().map(|_| clips[0].item);
    let vid_audio = video.is_some() && has_a(&clips[0]) && !remove_video_audio;
    let audio_items: Vec<usize> = (1..clips.len()).collect();
    let n_audio = audio_items.len() + vid_audio as usize;
    let rep = sync::report(&clips, &anchors, &starts, rate);
    let bin = s.project.root.parent_of(clips[0].item).filter(|b| *b != s.project.root.id);
    Ok(Merge { clips, settings, starts, name, video, vid_audio, audio_items, n_audio, report: rep, bin, method })
}

fn apply_merge(s: &mut Session, plan: Merge) -> Result<Value> {
    let rep = plan.report.clone();
    let id = s.edit("Merge Clips", |p, st| materialize_merge(p, st, plan))?;
    Ok(json!({"item": id.0, "clips": rep}))
}
fn materialize_merge(pr: &mut Project, st: &mut crate::EditorState, plan: Merge) -> Result<ItemId> {
    let cmd = "clip.mergeClips";
    let Merge { clips, settings, starts, name, video, vid_audio, audio_items, n_audio, report: _, bin, method } = plan;

    let sid = pr.new_sequence(&name, settings.clone(), video.is_some() as usize, n_audio, bin);
    let mut v_items = Vec::new();
    let mut a_items = Vec::new();
    let link = pr.alloc_id();
    if video.is_some() {
        let mut v = placed_item(pr, clips[0].item, TrackKind::Video, starts[0], clips[0].range, &settings).ok_or_else(|| bad(cmd, "bad video clip"))?;
        v.link = Some(link);
        v_items.push(v);
        if vid_audio {
            let mut a = placed_item(pr, clips[0].item, TrackKind::Audio, starts[0], clips[0].range, &settings).ok_or_else(|| bad(cmd, "bad clip"))?;
            a.link = Some(link);
            a_items.push(a);
        }
    }
    for &i in &audio_items {
        let mut a = placed_item(pr, clips[i].item, TrackKind::Audio, starts[i], clips[i].range, &settings).ok_or_else(|| bad(cmd, "bad audio clip"))?;
        a.link = Some(link);
        a_items.push(a);
    }
    let q = pr.sequence_mut(sid).ok_or(EngineError::NoSequence)?;
    for (t, it) in q.video_tracks.iter_mut().zip(v_items) {
        t.items.push(it);
    }
    for (t, it) in q.audio_tracks.iter_mut().zip(a_items) {
        t.items.push(it);
    }
    q.merged = Some(MergedClip { video, audio: audio_items.iter().map(|&i| clips[i].item).collect(), sync: method.name().into() });
    q.check().map_err(EngineError::Other)?;
    if let Some(it) = pr.item_mut(sid) {
        it.label = Label::Iris;
    }
    st.project_selection = vec![sid];
    Ok(sid)
}

fn prepare_multicam(s: &Session, p: &Value) -> Result<CreateMulticam> {
    prepare_multicam_observed(s, p, None)
}
fn prepare_multicam_observed(s: &Session, p: &Value, observed: Option<&[history::Row]>) -> Result<CreateMulticam> {
    let cmd = "clip.createMulticam";
    let method = method_p(p, cmd)?;
    let mut clips = sync_items(s, &items_p(s, p), cmd)?;
    let has_v = |s: &Session, c: &SyncClip| s.project.item(c.item).is_some_and(|i| i.has_video());
    let has_a = |s: &Session, c: &SyncClip| s.project.item(c.item).is_some_and(|i| i.has_audio());
    // cameras with video first (selection order), then audio-only sources
    clips.sort_by_key(|c| !has_v(s, c));
    if !clips.iter().any(|c| has_v(s, c)) {
        return Err(bad(cmd, "select at least one clip with video"));
    }
    let names_mode = str_p(p, "cameraNames").unwrap_or("clip").to_string();
    if names_mode == "metadata" {
        // cameras ordered by their Camera Angle metadata when present
        let key = |c: &SyncClip| s.project.item(c.item).and_then(|i| i.metadata.get("Camera Angle").cloned());
        let mut v: Vec<(bool, Option<String>, SyncClip)> = clips.iter().map(|c| (!has_v(s, c), key(c), c.clone())).collect();
        v.sort_by_key(|a| (a.0, a.1.is_none(), a.1.clone()));
        clips = v.into_iter().map(|x| x.2).collect();
    }
    let reference = match item_p(p, "reference")? {
        Some(r) => clips.iter().position(|c| c.item == r).ok_or_else(|| bad(cmd, "`reference` is not among the items"))?,
        None => 0,
    };
    let audio = match str_p(p, "audio") {
        Some(a) => MulticamAudio::from_name(a).ok_or_else(|| bad(cmd, format!("unknown audio mode `{a}` (camera1, all, switch)")))?,
        None => MulticamAudio::Camera1,
    };
    let settings = settings_for(s, clips[reference].item);
    let rate = settings.frame_rate;
    let anchors = computed_anchors(s, &clips, reference, &method, offset_p(p, rate), observed, rate)?;
    let starts = sync::placements_checked(&clips, &anchors, rate, false)?;
    let rep = sync::report(&clips, &anchors, &starts, rate);
    let name = str_p(p, "name").map(str::to_string).unwrap_or_else(|| format!("{} Multicam", clips[reference].name));
    let move_processed = bool_p(p, "processedBin").unwrap_or(false);
    let flags: Vec<(bool, bool)> = clips.iter().map(|c| (has_v(s, c), has_a(s, c))).collect();
    let names: Vec<String> = clips.iter().enumerate().map(|(i, c)| camera_name(&s.project, c.item, &names_mode, i + 1)).collect();
    let nv = flags.iter().filter(|f| f.0).count();
    let na = flags.iter().filter(|f| f.1).count();
    Ok(CreateMulticam { clips, settings, starts, method, audio, name, move_processed, flags, names, nv, na, report: rep })
}

fn apply_multicam(s: &mut Session, plan: CreateMulticam) -> Result<Value> {
    let rep = plan.report.clone();
    let names = plan.names.clone();
    let id = s.edit("Create Multi-Camera Source Sequence", |p, st| materialize_multicam(p, st, plan))?;
    Ok(json!({"sequence": id.0, "cameras": names, "clips": rep}))
}
fn materialize_multicam(pr: &mut Project, st: &mut crate::EditorState, plan: CreateMulticam) -> Result<ItemId> {
    let cmd = "clip.createMulticam";
    let CreateMulticam { clips, settings, starts, method, audio, name, move_processed, flags, names, nv, na, report: _ } = plan;

    let sid = pr.new_sequence(&name, settings.clone(), nv, na.max(1), None);
    let mut cameras = Vec::new();
    let (mut vi, mut ai) = (0usize, 0usize);
    let mut placed: Vec<(TrackKind, usize, TrackItem)> = Vec::new();
    let (vtracks, atracks): (Vec<TrackId>, Vec<TrackId>) = {
        let q = pr.sequence(sid).ok_or(EngineError::NoSequence)?;
        (q.video_tracks.iter().map(|t| t.id).collect(), q.audio_tracks.iter().map(|t| t.id).collect())
    };
    for (k, c) in clips.iter().enumerate() {
        let (hv, ha) = flags[k];
        let link = (hv && ha).then(|| pr.alloc_id());
        let mut cam = Camera { name: names[k].clone(), video_track: None, audio_tracks: Vec::new(), enabled: true, source: Some(c.item) };
        if hv {
            let mut v = placed_item(pr, c.item, TrackKind::Video, starts[k], c.range, &settings).ok_or_else(|| bad(cmd, "bad clip"))?;
            v.link = link;
            placed.push((TrackKind::Video, vi, v));
            cam.video_track = Some(vtracks[vi]);
            vi += 1;
        }
        if ha {
            let mut a = placed_item(pr, c.item, TrackKind::Audio, starts[k], c.range, &settings).ok_or_else(|| bad(cmd, "bad clip"))?;
            a.link = link;
            placed.push((TrackKind::Audio, ai, a));
            cam.audio_tracks.push(atracks[ai]);
            ai += 1;
        }
        cameras.push(cam);
    }
    {
        let q = pr.sequence_mut(sid).ok_or(EngineError::NoSequence)?;
        for (kind, idx, it) in placed {
            q.tracks_mut(kind)[idx].items.push(it);
        }
        for cam in &cameras {
            if let Some(vt) = cam.video_track.and_then(|id| q.video_tracks.iter_mut().find(|x| x.id == id)) {
                vt.name = cam.name.clone();
            }
            for at in &cam.audio_tracks {
                if let Some(x) = q.audio_tracks.iter_mut().find(|x| x.id == *at) {
                    x.name = cam.name.clone();
                }
            }
        }
        q.multicam = Some(MulticamSource { cameras, audio, sync: method.name().into() });
        q.check().map_err(EngineError::Other)?;
    }
    if let Some(it) = pr.item_mut(sid) {
        it.label = Label::Mango;
    }
    if move_processed {
        let bin = processed_bin(pr);
        for c in &clips {
            pr.root.remove_item(c.item);
            if let Some(b) = pr.root.find_bin_mut(bin) {
                b.children.push(BinEntry::Item(c.item));
            }
        }
    }
    st.project_selection = vec![sid];
    Ok(sid)
}

#[cfg(test)]
mod tests;
