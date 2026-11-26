//! The original idle Clip Mixer calculation, shared with exact callers.
//! Human parsing/defaults and drag history stay in mixer.rs. This is not a
//! native ticket, identity grant, live recorder or an alternate history owner.
use crate::{Result, commands::bad};
use filmcraft_project::mixer::{FADER_MAX_DB, FADER_MIN_DB};
use filmcraft_project::{ClipId, ParamValue, Sequence};
use filmcraft_time::Tick;

pub(crate) struct Change {
    clip: ClipId,
    effect: String,
    param: String,
    value: f64,
    time: Tick,
    keyframe: bool,
}
fn target(q: &Sequence, clip: ClipId, effect: &str, param: &str, time: Tick) -> Result<(Tick, bool)> {
    let (_, it) = q.find_item(clip).ok_or_else(|| bad("clipMixer.set", "no such clip"))?;
    let end = it.end();
    if end < it.start || !it.speed.is_finite() {
        return Err(bad("clipMixer.set", "invalid clip mixer source clock"));
    }
    // This is deliberately source_time_at, including frame holds independent
    // of Hold Filters. The effect-control entry uses a different native clock.
    let mt = it.source_time_at(time.clamp(it.start, end));
    let e = it.effect(effect).ok_or_else(|| bad("clipMixer.set", format!("the clip has no `{effect}` effect")))?;
    let p = e.param(param).ok_or_else(|| bad("clipMixer.set", format!("no parameter `{param}`")))?;
    Ok((mt, p.is_animated()))
}
pub(crate) fn prepare(q: &Sequence, clip: ClipId, effect: &str, param: &str, value: f64, time: Tick, keyframe: bool) -> Result<Change> {
    if !value.is_finite() {
        return Err(bad("clipMixer.set", "finite clip mixer value required"));
    }
    target(q, clip, effect, param, time)?;
    let value = if param == "level" { value.clamp(FADER_MIN_DB, FADER_MAX_DB) } else { value.clamp(-100.0, 100.0) };
    Ok(Change { clip, effect: effect.to_string(), param: param.to_string(), value, time, keyframe })
}
impl Change {
    pub(crate) fn apply(self, q: &mut Sequence) -> Result<()> {
        // Re-resolve on the caller's candidate. No cached clip/source time or
        // animation decision can silently cross into a different current cut.
        let (mt, animated) = target(q, self.clip, &self.effect, &self.param, self.time)?;
        let (_, it) = q.find_item_mut(self.clip).ok_or_else(|| bad("clipMixer.set", "no such clip"))?;
        let e = it.effect_mut(&self.effect).ok_or_else(|| bad("clipMixer.set", format!("the clip has no `{}` effect", self.effect)))?;
        let p = e.param_mut(&self.param).ok_or_else(|| bad("clipMixer.set", format!("no parameter `{}`", self.param)))?;
        if self.keyframe || animated {
            p.put_keyframe(mt, ParamValue::Float(self.value));
        } else {
            p.value = ParamValue::Float(self.value);
        }
        Ok(())
    }
}
#[cfg(test)]
mod tests;
