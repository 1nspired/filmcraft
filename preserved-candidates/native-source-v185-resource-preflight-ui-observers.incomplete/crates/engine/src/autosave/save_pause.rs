//! A Save owns its own worker pause. Workspace-update state is never released.
use super::{Msg, Persistence};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
    mpsc::{Receiver, RecvTimeoutError, Sender, channel},
};
use std::time::Duration;

pub(super) enum Control {
    Wake,
    Stop,
}
pub(crate) struct SavePause {
    active: Option<Arc<AtomicBool>>,
    owner: Arc<AtomicBool>,
    tx: Sender<Msg>,
    control: Sender<Control>,
    resume_ack: Option<Receiver<()>>,
    resume_confirmed: bool,
}
/// Observation only; cannot resume, dispatch or acquire ownership.
pub(crate) struct Probe {
    active: std::sync::Weak<AtomicBool>,
    control: Sender<Control>,
}
impl Probe {
    pub(crate) fn current(&self) -> Result<(), String> {
        if self.active.upgrade().is_none_or(|v| !v.load(Ordering::Acquire)) {
            return Err("original Save pause lifetime ended".into());
        }
        self.control.send(Control::Wake).map_err(|_| "original paused autosave worker stopped".into())
    }
}
impl SavePause {
    pub(crate) fn probe(&self) -> Result<Probe, String> {
        let active = self.active.as_ref().ok_or_else(|| "Save resume already began; no paused-worker observation minted".to_string())?;
        let probe = Probe { active: Arc::downgrade(active), control: self.control.clone() };
        probe.current()?;
        Ok(probe)
    }
    fn claim(p: &Persistence, check: &mut impl FnMut() -> Result<(), String>) -> Result<Self, String> {
        check()?;
        p.save_owner.compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire).map_err(|_| "another Save owns this persistence pause".to_string())?;
        let active = Arc::new(AtomicBool::new(true));
        let guard = Self {
            active: Some(active.clone()),
            owner: p.save_owner.clone(),
            tx: p.tx.clone(),
            control: p.save_control.clone(),
            resume_ack: None,
            resume_confirmed: false,
        };
        Ok(guard)
    }
    pub(super) fn acquire(p: &Persistence, mut check: impl FnMut() -> Result<(), String>) -> Result<Self, String> {
        let guard = Self::claim(p, &mut check)?;
        let active = guard.active.as_ref().ok_or_else(|| "original Save pause has no active lifetime".to_string())?.clone();
        let (tx, rx) = channel();
        p.tx.send(Msg::ProjectSavePause(active, tx)).map_err(|_| "auto-save worker stopped before Save pause".to_string())?;
        wait(rx, &mut check)?;
        guard.current(p)?;
        Ok(guard)
    }
    pub(super) fn acquire_resource_preflight(p: &Persistence, mut check: impl FnMut() -> Result<(), String>) -> Result<Self, String> {
        check()?;
        if p.startup_pending() || p.thread.as_ref().is_none_or(|t| t.is_finished()) {
            return Err("resource preflight requires the live original released worker, not a cold/startup owner".into());
        }
        let guard = Self::claim(p, &mut check)?;
        let active = guard.active.as_ref().ok_or_else(|| "original preflight pause has no active lifetime".to_string())?.clone();
        let (tx, rx) = channel();
        p.tx.send(Msg::ResourcePreflightPause(active, tx)).map_err(|_| "original worker stopped before resource preflight pause".to_string())?;
        loop {
            check()?;
            match rx.recv_timeout(Duration::from_millis(10)) {
                Ok(result) => {
                    if let Err(writer) = result {
                        // Preserve the actual producer refusal as primary even
                        // if the owner's final observation also refuses.
                        return Err(match check() {
                            Ok(()) => writer,
                            Err(fence) => format!("{writer}; original resource preflight owner also refused: {fence}"),
                        });
                    }
                    check()?;
                    guard.current(p)?;
                    return Ok(guard);
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => return Err("original resource preflight pause acknowledgement lost; no held namespace was observed".into()),
            }
        }
    }
    pub(crate) fn current(&self, p: &Persistence) -> Result<(), String> {
        if !Arc::ptr_eq(&self.owner, &p.save_owner) || self.active.as_ref().is_none_or(|v| !v.load(Ordering::Acquire)) {
            return Err("Save pause no longer belongs to this persistence owner".into());
        }
        self.control.send(Control::Wake).map_err(|_| "paused auto-save worker stopped".into())
    }
    pub(crate) fn same_owner(&self, p: &Persistence) -> bool {
        Arc::ptr_eq(&self.owner, &p.save_owner)
    }
    /// A long-lived native owner keeps this acknowledgement even if Stop or
    /// observation fails. Only the first call queues the original Resume marker.
    /// Existing scoped `finish` semantics remain unchanged.
    pub(crate) fn finish_owned(&mut self, mut check: impl FnMut() -> Result<(), String>) -> Result<(), String> {
        check()?;
        if !self.resume_confirmed {
            if self.resume_ack.is_none() {
                let (tx, rx) = channel();
                self.resume_ack = Some(rx);
                self.release(Some(tx))?;
            }
            let rx = self.resume_ack.as_ref().ok_or_else(|| "Save resume acknowledgement unavailable".to_string())?;
            loop {
                check()?;
                match rx.recv_timeout(Duration::from_millis(10)) {
                    Ok(()) => {
                        self.resume_confirmed = true;
                        break;
                    }
                    Err(RecvTimeoutError::Timeout) => {}
                    Err(RecvTimeoutError::Disconnected) => return Err("Save resume acknowledgement lost; do not queue another marker".into()),
                }
            }
        }
        check()
    }
    fn release(&mut self, ack: Option<Sender<()>>) -> Result<(), String> {
        let Some(active) = self.active.take() else { return Ok(()) };
        // Queue the marker AFTER the owner's final Clean/Dirty notification,
        // BEFORE waking the worker. No stale timer can overtake that state.
        let queued = self.tx.send(Msg::ProjectSaveResume(active.clone(), ack));
        active.store(false, Ordering::Release);
        let wake = self.control.send(Control::Wake);
        queued.map_err(|_| "auto-save worker stopped before Save resume".to_string())?;
        wake.map_err(|_| "auto-save worker stopped during Save resume".into())
    }
    pub(crate) fn finish(mut self, mut check: impl FnMut() -> Result<(), String>) -> Result<(), String> {
        let (tx, rx) = channel();
        self.release(Some(tx))?;
        wait(rx, &mut check)
    }
}
fn wait(rx: Receiver<()>, check: &mut impl FnMut() -> Result<(), String>) -> Result<(), String> {
    loop {
        check()?;
        match rx.recv_timeout(Duration::from_millis(10)) {
            Ok(()) => return check(),
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return Err("Save worker acknowledgement was lost".into()),
        }
    }
}
impl Drop for SavePause {
    fn drop(&mut self) {
        let _ = self.release(None);
        self.owner.store(false, Ordering::Release);
    }
}

#[cfg(test)]
#[path = "save_pause_tests.rs"]
mod tests;
