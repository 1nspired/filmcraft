//! Exact owning-project caption renderer observations, never media or write authority.
use super::{graphics, parse, project_audit, project_plane};
use crate::{Result, Session, commands::bad, project_workspace::ProjectScope};
use filmcraft_project::{ClipId, ItemId, TrackId};
use serde::Deserialize;
use serde_json::{Value, json};
pub(super) const READ: &str = "agent.readCaptionFontResources";
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Target {
    sequence: u64,
    track: u64,
    caption: u64,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Query {
    token: String,
    scope: ProjectScope,
    target: Target,
}
pub(super) fn read(s: &mut Session, p: &Value) -> Result<Value> {
    let q: Query = parse(READ, p)?;
    project_plane::validate_ticket(s, &q.token, &q.scope)?;
    if [q.target.sequence, q.target.track, q.target.caption].iter().any(|v| !(1..=9_007_199_254_740_991).contains(v)) {
        return Err(bad(READ, "exact bounded caption sequence/track/cue required"));
    }
    let seq = s.project.sequence(ItemId(q.target.sequence)).ok_or_else(|| bad(READ, "caption sequence missing"))?;
    seq.settings.validate().map_err(|e| bad(READ, e))?;
    let track = seq.caption_tracks.iter().find(|t| t.id == TrackId(q.target.track)).ok_or_else(|| bad(READ, "caption track missing"))?;
    let caption = track.captions.iter().find(|c| c.id == ClipId(q.target.caption)).ok_or_else(|| bad(READ, "caption cue missing"))?;
    let observed = graphics::observe_caption_fonts(caption, &track.style, seq.settings.width as usize, seq.settings.height as usize)?;
    let result = json!({"status":"read","kind":"native_caption_font_resources","version":1,"token":q.token,"scope":q.scope,"revision":s.revision,"sequence":s.state.active_sequence.map(|id|id.0),"projectSha256":project_plane::basis(s)?,"target":{"sequence":q.target.sequence,"track":q.target.track,"caption":q.target.caption},"trackEnabled":track.enabled,"trackLocked":track.locked,"fontState":observed,"referenceScope":"one_exact_saved_caption_and_canvas","sourceValidation":"not-performed","fontFileState":"not-inspected","embeddingRights":"not-established","writeAuthority":false});
    if project_audit::value_bytes(&result)?.len() > 64 * 1024 {
        return Err(bad(READ, "complete caption resource reply exceeds bound"));
    }
    project_plane::validate_ticket(s, &q.token, &q.scope)?;
    Ok(result)
}
