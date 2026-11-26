//! Scoped worker coordination, not filesystem custody or a publication receipt.
use crate::{EngineError, Result, Session};
use std::sync::Arc;
fn error(message: impl Into<String>) -> EngineError {
    EngineError::Other(message.into())
}
pub(crate) fn with_quiescent<R>(s: &mut Session, check: &impl Fn() -> Result<()>, run: impl FnOnce(&mut Session) -> Result<R>) -> Result<R> {
    check()?;
    if s.is_display_only() {
        return Err(error("Save requires the original editable native owner"));
    }
    crate::agent_bridge::require_idle_native_writer_jobs(s)?;
    s.sync_persistence();
    let pause = s.persistence.as_ref().map(|p| p.pause_project_save(|| check().map_err(|e| e.to_string()))).transpose().map_err(error)?;
    s.apply_persistence_events();
    let store = s.previews.clone();
    let _previews = store.pause_writers(check)?;
    check()?;
    crate::agent_bridge::require_idle_native_writer_jobs(s)?;
    if let (Some(pause), Some(p)) = (&pause, &s.persistence) {
        pause.current(p).map_err(error)?;
    }
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run(s)))
        .unwrap_or_else(|_| Err(error("native Save body failed internally; reconcile its file outcome before any retry")));
    let owner = if !Arc::ptr_eq(&store, &s.previews) || pause.is_some() != s.persistence.is_some() {
        Err(error("native Save worker owner changed; later work was retained"))
    } else if let (Some(pause), Some(p)) = (&pause, &s.persistence) {
        pause.current(p).map_err(error)
    } else {
        Ok(())
    };
    // Queue the final original saved/dirty state BEFORE releasing the worker.
    s.sync_persistence();
    let resumed = pause.map(|p| p.finish(|| check().map_err(|e| e.to_string()))).transpose().map_err(error);
    result.and_then(|value| owner.and_then(|()| resumed.map(|_| value)))
}
#[cfg(test)]
#[path = "quiescence_tests.rs"]
mod tests;
