//! Shared human Speed/Duration implementation and actual sequential group timing.
use filmcraft_edit as edit;
use filmcraft_project::ClipId;
use serde_json::Value;

use crate::commands::{bad, clips_p, with_links};
use crate::{Result, Session};

pub(crate) fn run(s: &mut Session, p: &Value) -> Result<Vec<edit::SpeedRipple>> {
    if !p.is_object() {
        return Err(bad("clip.speedDuration", "options must be an object"));
    }
    // Only absent options receive human defaults. Wrong types fail before
    // publishing an edit or consuming the current Redo branch.
    let speed =
        p.get("speed").map(|v| v.as_f64().ok_or_else(|| bad("clip.speedDuration", "speed must be a positive numeric percent"))).transpose()?.unwrap_or(100.0)
            / 100.0;
    if !speed.is_finite() || speed <= 0.0 {
        return Err(bad("clip.speedDuration", "speed must be a finite positive native ratio"));
    }
    let flag = |key: &str| -> Result<bool> {
        p.get(key).map(|v| v.as_bool().ok_or_else(|| bad("clip.speedDuration", format!("{key} must be a boolean")))).transpose().map(|v| v.unwrap_or(false))
    };
    let reverse = flag("reverse")?;
    let ripple = flag("ripple")?;
    let interp = p
        .get("interpolation")
        .map(|v| {
            v.as_str()
                .and_then(filmcraft_project::TimeInterpolation::from_name)
                .ok_or_else(|| bad("clip.speedDuration", "interpolation must name frameSampling, frameBlending or opticalFlow"))
        })
        .transpose()?;
    let sel = with_links(s, &clips_p(s, p)?);
    let mut effects = Vec::new();
    s.edit_sequence("Speed/Duration", |q, ctx, _| {
        // A clip and its linked partners on other tracks are one group, so
        // later material moves once. Preserve the human selection/group order.
        let place = |q: &filmcraft_project::Sequence, c: ClipId| q.find_item(c).map(|(t, i)| (t, i.link));
        let mut groups: Vec<Vec<ClipId>> = Vec::new();
        for c in &sel {
            let Some((track, link)) = place(q, *c) else {
                groups.push(vec![*c]);
                continue;
            };
            let joins = |g: &Vec<ClipId>| link.is_some() && g.iter().all(|o| place(q, *o).is_some_and(|(t, l)| l == link && t != track));
            match groups.iter_mut().find(|g| joins(g)) {
                Some(g) => g.push(*c),
                None => groups.push(vec![*c]),
            }
        }
        for group in &groups {
            let unlocked: Vec<ClipId> =
                group.iter().copied().filter(|c| q.find_item(*c).is_some_and(|(t, _)| q.track(t).is_some_and(|tr| !tr.locked))).collect();
            if unlocked.is_empty() {
                return Err(edit::EditError::Locked.into());
            }
            if let Some(effect) = edit::set_speed_group_with_ripple(q, &unlocked, speed, reverse, ripple, ctx)? {
                effects.push(effect);
            }
        }
        for c in &sel {
            if let (Some(m), Some((_, it))) = (interp, q.find_item_mut(*c)) {
                it.time_interpolation = m;
            }
        }
        Ok(())
    })?;
    Ok(effects)
}
