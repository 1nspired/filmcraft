//! Distinct bounded multicam testimony. This request never creates a Ticket or a recorder owner.
use super::{parse, project_audit, project_clear, project_plane, token_ok};
use crate::{Result, Session, commands::bad, project_workspace::ProjectScope};
use filmcraft_time::Tick;
use serde::Deserialize;
use serde_json::{Value, json};
pub(super) const READ: &str = "agent.readMulticam";
const ROWS: usize = 100_000;
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Read {
    version: u8,
    request_id: String,
    scope: ProjectScope,
    project_path: String,
    time_ticks: Option<String>,
    camera_offset: u32,
    cut_offset: u32,
    limit: u8,
}
fn error(message: impl Into<String>) -> crate::EngineError {
    bad(READ, message)
}
pub(super) fn read(s: &mut Session, p: &Value) -> Result<Value> {
    let r: Read = parse(READ, p)?;
    if r.version != 1
        || !token_ok(&r.request_id)
        || r.limit == 0
        || r.limit > 32
        || r.camera_offset > 100_000
        || r.cut_offset > 100_000
        || r.project_path.len() > 4096
        || !r.project_path.starts_with('/')
        || r.project_path.chars().any(char::is_control)
        || s.path.as_ref() != Some(&r.project_path)
    {
        return Err(error("exact owning Root, saved path, request identity and bounded pages required"));
    }
    project_plane::require_scope(s, &r.scope)?;
    let time = match r.time_ticks {
        Some(ref raw) => {
            let value = raw.parse::<i64>().map_err(|_| error("canonical exact i64 ticks required"))?;
            if value.to_string() != *raw {
                return Err(error("canonical exact i64 ticks required"));
            }
            Tick(value)
        }
        None => s.playhead(),
    };
    // These are existing bounded authored hashes, not a media census or saved authority.
    let project_sha = project_plane::basis(s)?;
    let editor_sha = project_clear::editor_hash(&s.state)?;
    let target = crate::multicam::multicam_clip_at(s, time);
    if s.mcrec.cuts.len() > ROWS {
        return Err(error("multicam recording exceeds the bounded metadata census"));
    }
    if let Some((track, clip)) = &target {
        if [track.0, clip.id.0, clip.item.0].iter().any(|id| *id == 0 || *id > 9_007_199_254_740_991) {
            return Err(error("selected multicam identities exceed exact host integers"));
        }
        if let Some(source) = s.project.sequence(clip.item) {
            if source.camera_count() > ROWS {
                return Err(error("multicam camera census exceeds its explicit bound"));
            }
            for angle in 0..source.camera_count() {
                if let Some(camera) = source.camera_at(angle) {
                    if camera.name.len() > 4096
                        || camera.audio_tracks.len() > 256
                        || camera.video_track.is_some_and(|id| id.0 == 0 || id.0 > 9_007_199_254_740_991)
                        || camera.audio_tracks.iter().any(|id| id.0 == 0 || id.0 > 9_007_199_254_740_991)
                        || camera.source.is_some_and(|id| id.0 == 0 || id.0 > 9_007_199_254_740_991)
                    {
                        return Err(error("camera identity or metadata exceeds the closed reply bound"));
                    }
                }
            }
        }
    }
    let epoch = super::history_checkpoint::epoch(s)?;
    let rate = s.active_sequence().map(|q| json!({"num":q.settings.frame_rate.num.to_string(),"den":q.settings.frame_rate.den.to_string()}));
    let result = json!({"status":"read","kind":"native_multicam_read","version":1,"requestId":r.request_id,
        "scope":r.scope,"project":{"path":r.project_path,"dirty":s.revision != s.saved_revision},
        "nativeSession":epoch,"revision":s.revision,"sequence":s.state.active_sequence.map(|i|i.0),
        "projectStateSha256":project_sha,"editorStateSha256":editor_sha,"timeTicks":time.0.to_string(),"frameRate":rate,
        "inspection":crate::multicam::inspect_page_at(s,time,r.camera_offset as usize,r.cut_offset as usize,r.limit as usize),
        "newReadTicket":false,"sourceValidation":"not-performed","mediaEvidence":false,"writeAuthority":false,"recordingOwnership":false});
    if project_audit::value_bytes(&result)?.len() > 256 * 1024 {
        return Err(error("multicam metadata exceeds the bounded256KiB reply"));
    }
    Ok(result)
}
#[cfg(test)]
mod tests;
