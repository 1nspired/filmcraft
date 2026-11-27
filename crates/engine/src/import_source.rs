//! Native source boundary for a frame-conformed import. Record duration and
//! nominal speed stay independent; no source geometry is stored in annotations.
use crate::{Result, Session, commands::bad};
use filmcraft_project::ClipId;
use filmcraft_time::Tick;
use serde_json::{Value, json};

pub(crate) fn set(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "poema.setImportedSourceRange";
    let clip = p["clip"].as_u64().map(ClipId).ok_or_else(|| bad(CMD, "need clip"))?;
    let tick = |key: &str| -> Result<Tick> {
        let value = p[key].as_str().and_then(|v| v.parse::<i64>().ok()).ok_or_else(|| bad(CMD, "source bounds must be exact decimal tick strings"))?;
        if !(0..=Tick::MAX.0).contains(&value) {
            return Err(bad(CMD, "source bound outside native range"));
        }
        Ok(Tick(value))
    };
    let (source_in, source_out) = (tick("sourceIn")?, tick("sourceOut")?);
    if source_out <= source_in {
        return Err(bad(CMD, "source interval must be positive"));
    }
    let q = s.active_sequence().ok_or_else(|| bad(CMD, "no sequence"))?;
    let (_, item) = q.find_item(clip).ok_or_else(|| bad(CMD, "unknown clip"))?;
    if item.reverse || !item.speed.is_finite() || item.speed <= 0.0 || item.speed > 100.0 {
        return Err(bad(CMD, "source import requires a finite forward speed"));
    }
    if q.settings.frame_rate.snap_nearest(item.start) != item.start || q.settings.frame_rate.snap_nearest(item.end()) != item.end() {
        return Err(bad(CMD, "record edges must already be frame-conformed"));
    }
    let nominal = (item.duration.0 as f64 * item.speed).round() as i64;
    let offset = source_out.0.saturating_sub(source_in.0).saturating_sub(nominal);
    let limit = (q.settings.frame_rate.frame_duration().0 as f64 * item.speed).ceil() as i64;
    if offset.unsigned_abs() > limit.saturating_add(2) as u64 {
        return Err(bad(CMD, "source/record discrepancy exceeds one frame of edge conform"));
    }
    s.edit_sequence("Preserve imported source interval", |q, _, _| {
        let (_, item) = q.find_item_mut(clip).ok_or_else(|| bad(CMD, "unknown clip"))?;
        item.source_in = source_in;
        item.set_source_out(source_out);
        q.check().map_err(crate::EngineError::Other)?;
        Ok(())
    })?;
    Ok(json!({"sourceIn":source_in.0.to_string(),"sourceOut":source_out.0.to_string()}))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn demo() -> Session {
        let mut s = Session::default();
        let (p, q) = crate::demo::demo_project(&s.media);
        s.project = std::sync::Arc::new(p);
        s.state.active_sequence = Some(q);
        s
    }
    #[test]
    fn native_source_boundary_is_exact_undoable_and_hostile_ranges_refuse() {
        let mut s = demo();
        let c = s.active_sequence().unwrap().video_tracks[0].items[0].clone();
        let end = c.source_out() - Tick(2540160000);
        let before = s.project.clone();
        let p = json!({"clip":c.id.0,"sourceIn":c.source_in.0.to_string(),"sourceOut":end.0.to_string()});
        s.execute("poema.setImportedSourceRange", p).unwrap();
        let actual = s.active_sequence().unwrap().find_item(c.id).unwrap().1;
        assert_eq!(actual.source_out(), end);
        assert_eq!(actual.duration, c.duration);
        assert_eq!(actual.speed, c.speed);
        s.execute("edit.undo", json!({})).unwrap();
        assert!(std::sync::Arc::ptr_eq(&before, &s.project));
        for bounds in [json!(["-1", "3"]), json!(["0", "9223372036854775807"]), json!(["2", "1"]), json!([0, 3])] {
            assert!(s.execute("poema.setImportedSourceRange", json!({"clip":c.id.0,"sourceIn":bounds[0],"sourceOut":bounds[1]})).is_err());
            assert!(std::sync::Arc::ptr_eq(&before, &s.project));
        }
    }
}
