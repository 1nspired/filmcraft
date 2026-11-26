//! Tracks the complete original preview producer lifetime, including final I/O.
use crate::{EngineError, Result};
use std::sync::{Arc, Condvar, Mutex};
#[cfg(not(target_arch = "wasm32"))]
use std::time::Duration;
#[derive(Default)]
struct State {
    writers: usize,
    paused: bool,
}
#[derive(Default)]
pub(super) struct Activity {
    state: Mutex<State>,
    changed: Condvar,
}
pub(super) struct Writer(Arc<Activity>);
#[cfg(not(target_arch = "wasm32"))]
pub(crate) struct Pause {
    activity: Arc<Activity>,
    seal: Arc<()>,
}
#[cfg(not(target_arch = "wasm32"))]
pub(crate) struct PauseProbe {
    activity: std::sync::Weak<Activity>,
    seal: std::sync::Weak<()>,
}
fn error(message: &str) -> EngineError {
    EngineError::Other(message.into())
}
impl Activity {
    pub(super) fn writer(self: &Arc<Self>) -> Result<Writer> {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if state.paused {
            return Err(error("project Save has paused new preview writers"));
        }
        state.writers = state.writers.checked_add(1).ok_or_else(|| error("preview writer count exhausted"))?;
        Ok(Writer(self.clone()))
    }
    #[cfg(not(target_arch = "wasm32"))]
    pub(super) fn pause(self: &Arc<Self>, check: &impl Fn() -> Result<()>) -> Result<Pause> {
        check()?;
        {
            let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            if state.paused {
                return Err(error("another Save owns the preview pause"));
            }
            state.paused = true;
        }
        let pause = Pause { activity: self.clone(), seal: Arc::new(()) };
        loop {
            check()?;
            let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            if state.writers == 0 {
                drop(state);
                check()?;
                return Ok(pause);
            }
            drop(self.changed.wait_timeout(state, Duration::from_millis(10)).unwrap_or_else(|e| e.into_inner()));
        }
    }
}
impl Drop for Writer {
    fn drop(&mut self) {
        let mut state = self.0.state.lock().unwrap_or_else(|e| e.into_inner());
        if state.writers > 0 {
            state.writers -= 1;
        }
        self.0.changed.notify_all();
    }
}
#[cfg(not(target_arch = "wasm32"))]
impl Pause {
    pub(crate) fn probe(&self) -> PauseProbe {
        PauseProbe { activity: Arc::downgrade(&self.activity), seal: Arc::downgrade(&self.seal) }
    }
    pub(super) fn owns(&self, activity: &Arc<Activity>) -> bool {
        let state = self.activity.state.lock().unwrap_or_else(|e| e.into_inner());
        Arc::ptr_eq(&self.activity, activity) && state.paused && state.writers == 0
    }
}
#[cfg(not(target_arch = "wasm32"))]
impl PauseProbe {
    pub(crate) fn current(&self) -> Result<()> {
        if self.seal.upgrade().is_none() {
            return Err(error("original preview pause epoch ended"));
        }
        let activity = self.activity.upgrade().ok_or_else(|| error("original preview store ended"))?;
        let state = activity.state.lock().unwrap_or_else(|e| e.into_inner());
        if !state.paused || state.writers != 0 {
            return Err(error("original preview producers are no longer paused"));
        }
        Ok(())
    }
}
#[cfg(not(target_arch = "wasm32"))]
impl Drop for Pause {
    fn drop(&mut self) {
        let mut state = self.activity.state.lock().unwrap_or_else(|e| e.into_inner());
        state.paused = false;
        self.activity.changed.notify_all();
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
#[path = "activity_tests.rs"]
mod tests;
