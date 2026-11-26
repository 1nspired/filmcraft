//! One worker call's wait/Stop handle. Never the native Save operation key.
use super::super::media_guard::Check;
use super::{Request, request};
use crate::{Result, commands::bad};
use serde_json::{Value, json};
use std::sync::{
    Arc,
    atomic::{AtomicU8, Ordering},
};
const QUEUED: u8 = 0;
const RUNNING: u8 = 1;
const RETURNED: u8 = 2;
#[derive(Clone)]
pub(crate) struct Control {
    command: String,
    request: Request,
    check: Arc<Check>,
    phase: Arc<AtomicU8>,
}
pub(crate) fn supports(command: &str) -> bool {
    super::managed_dispatch_supports(command)
        || matches!(
            command,
            super::BEGIN
                | super::STATUS
                | super::EFFECTS
                | super::CANCEL
                | super::RECOVERY
                | super::RECORD_PREPARE
                | super::RECORD_READ
                | super::RECORD_PUBLISH
                | super::ADMISSION_PREPARE
                | super::ADMISSION_READ
        )
}
impl Control {
    pub(crate) fn new(command: &str, p: &Value) -> Result<Self> {
        if !supports(command) {
            return Err(bad(command, "not a finite native Save lifecycle call"));
        }
        Ok(Self {
            command: command.into(),
            request: super::task_request(command, p)?,
            check: Arc::new(Check::new(web_time::Instant::now())),
            phase: Arc::new(AtomicU8::new(QUEUED)),
        })
    }
    pub(crate) fn enter(&self) {
        self.phase.store(RUNNING, Ordering::Release);
    }
    pub(crate) fn finish(&self) {
        self.phase.store(RETURNED, Ordering::Release);
    }
    pub(crate) fn same(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.phase, &other.phase)
    }
    pub(crate) fn matches(&self, p: &Value) -> bool {
        request(p).is_ok_and(|r| r == self.request)
    }
    pub(crate) fn params(&self) -> Value {
        json!({"job":self.request.job,"operation":self.request.operation})
    }
    pub(super) fn read_check(&self) -> &Check {
        &self.check
    }
    pub(super) fn current(&self, command: &str, r: &Request) -> Result<()> {
        if self.command != command || self.request != *r || self.phase.load(Ordering::Acquire) == RETURNED {
            return Err(bad(command, "original Save worker wait changed or already returned"));
        }
        self.check.check()
    }
    pub(crate) fn progress(&self) -> Value {
        let returned = self.phase.load(Ordering::Acquire) == RETURNED;
        json!({"kind":"native_Save_lifecycle_wait","phase":if returned{"returned"}else if self.check.stop.load(Ordering::Acquire){"stopping"}else if self.phase.load(Ordering::Acquire)==QUEUED{"queued"}else{"running"},
          "job":self.request.job,"operation":self.request.operation,"cancellable":!returned,"cancel_requested":self.check.stop.load(Ordering::Acquire),
          "nativeOperationCancelled":false,"writeAuthority":false,"nativeComplete":false})
    }
    pub(crate) fn stop(&self) -> Value {
        let accepted = self.phase.load(Ordering::Acquire) != RETURNED;
        if accepted {
            self.check.stop.store(true, Ordering::Release);
        }
        let mut value = self.progress();
        value["waitStopAccepted"] = json!(accepted);
        value
    }
}
