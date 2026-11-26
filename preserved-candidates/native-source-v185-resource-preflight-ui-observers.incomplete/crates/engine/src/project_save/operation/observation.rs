//! Complete descriptive reads of the SAME retained original Save effects.
//! Receipt observations do not authorize dispatch, repair, rollback or Resume.
use super::{Key, Phase, error, owned};
use crate::{Result, Session};
use serde::Serialize;
use serde_json::{Value, json};
use std::io::{self, Write};
pub(crate) const MAX_BYTES: usize = 32 * 1024 * 1024;
struct Writer<'a> {
    bytes: Vec<u8>,
    limit: usize,
    check: &'a dyn Fn() -> Result<()>,
}
impl Write for Writer<'_> {
    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        (self.check)().map_err(|e| io::Error::other(e.to_string()))?;
        if data.len() > self.limit.saturating_sub(self.bytes.len()) {
            return Err(io::Error::other("complete native Save effect observation exceeds its original bound; owner retained"));
        }
        self.bytes.extend_from_slice(data);
        Ok(data.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
fn bytes<T: Serialize + ?Sized>(value: &T, limit: usize, check: &dyn Fn() -> Result<()>) -> Result<Vec<u8>> {
    check()?;
    let mut writer = Writer { bytes: Vec::new(), limit, check };
    serde_json::to_writer(&mut writer, value).map_err(|e| error(e.to_string()))?;
    check()?;
    Ok(writer.bytes)
}
// A single shared budget for original events, documents and indexed previews.
// Serialization refuses before an oversized borrowed collection is cloned.
// No field/array is truncated and a fresh read cannot renew native write time.
pub(in crate::project_save) struct Budget<'a> {
    remaining: usize,
    check: &'a dyn Fn() -> Result<()>,
}
impl<'a> Budget<'a> {
    pub(in crate::project_save) fn new(check: &'a dyn Fn() -> Result<()>) -> Self {
        Self { remaining: MAX_BYTES, check }
    }
    pub(in crate::project_save) fn value<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<Value> {
        let raw = bytes(value, self.remaining, self.check)?;
        self.remaining = self.remaining.checked_sub(raw.len()).ok_or_else(|| error("native Save effect read budget underflow"))?;
        serde_json::from_slice(&raw).map_err(|e| error(e.to_string()))
    }
    pub(in crate::project_save) fn local<T: Serialize + ?Sized>(&self, value: &T) -> Result<Value> {
        let raw = bytes(value, self.remaining, self.check)?;
        serde_json::from_slice(&raw).map_err(|e| error(e.to_string()))
    }
    pub(in crate::project_save) fn branch(&self) -> Budget<'a> {
        Budget { remaining: self.remaining, check: self.check }
    }
    pub(in crate::project_save) fn checker(&self) -> &'a dyn Fn() -> Result<()> {
        self.check
    }
    pub(in crate::project_save) fn current(&self) -> Result<()> {
        (self.check)()
    }
    pub(in crate::project_save) fn inspection<T: Serialize>(&mut self, read: impl FnOnce() -> io::Result<T>) -> Result<Value> {
        self.current()?;
        let result = read();
        self.current()?;
        match result {
            Ok(value) => self.local(&json!({"observation":value,"error":null})),
            Err(e) => self.local(&json!({"observation":null,"error":e.to_string()})),
        }
    }
    pub(in crate::project_save) fn finish(&self, value: Value) -> Result<Value> {
        // Includes the complete envelope and separators, not just its members.
        bytes(&value, MAX_BYTES, self.check)?;
        Ok(value)
    }
}
// Fresh returned-receipt observation only. Full Root/namespace custody and
// durable manager adoption remain separate prerequisites for any Resume.
fn receipt_verification(s: &Session, key: &Key, check: &impl Fn() -> Result<()>) -> Result<Value> {
    check()?;
    let owner = owned(s, key)?;
    let mut status = "not_checked";
    let mut reason = None;
    let mut documents = 0;
    let mut previews = 0;
    if owner.phase == Phase::AwaitingReconciliation {
        let result = (|| {
            owner.workers(s)?;
            if !owner.body_returned
                || !owner.events_complete
                || !matches!(owner.returned.as_ref(), Some(Ok(_)))
                || !matches!(owner.postflight.as_ref(), Some(Ok(())))
            {
                return Err(error("original Save body or complete physical call returns remain unknown or failed"));
            }
            owner.after.as_ref().ok_or_else(|| error("original Save after-state is unavailable"))?.current(s)?;
            #[cfg(any(target_vendor = "apple", target_os = "linux"))]
            {
                let report = owner.preview_report.as_ref().ok_or_else(|| error("original complete Save receipt provider missing"))?;
                let counts = report
                    .verify_returned_receipts(&owner.documents, &|| check().map_err(|e| io::Error::other(e.to_string())))
                    .map_err(|e| error(e.to_string()))?;
                owner.after.as_ref().ok_or_else(|| error("original Save after-state disappeared"))?.current(s)?;
                owner.workers(s)?;
                check()?;
                Ok(counts)
            }
            #[cfg(not(any(target_vendor = "apple", target_os = "linux")))]
            {
                Err(error("original native physical receipt verification unavailable on this target"))
            }
        })();
        // Cancellation, lost original controls and the unchanged finite read
        // lifetime still refuse delivery, even if an effect read found an error.
        check()?;
        match result {
            Ok((d, p)) => {
                status = "verified";
                documents = d;
                previews = p;
            }
            Err(e) => {
                status = "unresolved";
                reason = Some(e.to_string().chars().take(4096).collect::<String>());
            }
        }
    }
    check()?;
    Ok(
        json!({"kind":"native_original_Save_returned_receipt_verification","version":1,"status":status,"documents":documents,"previews":previews,"reason":reason,
        "current":false,"allEffectsVerified":false,"writeAuthority":false,"nativeComplete":false,"startupAuthority":false,"workersMayResume":false}),
    )
}

/// Observe current physical receipts and all retained original call returns.
/// This cannot reconstruct a Key or finish an operation whose after-state is
/// unknown, whose physical evidence changed or whose original write time died.
pub(crate) fn read(s: &Session, key: &Key, check: &impl Fn() -> Result<()>) -> Result<Value> {
    check()?;
    let o = owned(s, key)?;
    o.workers(s)?;
    let mut budget = Budget::new(check);
    let receipts = receipt_verification(s, key, check)?;
    let receipts = budget.value(&receipts)?;
    let planned = budget.value(&o.planned)?;
    let events = budget.value(&o.events)?;
    let body = match &o.returned {
        None => Value::Null,
        Some(Ok(value)) => budget.value(&json!({"status":"ok","value":value}))?,
        Some(Err(message)) => budget.value(&json!({"status":"error","message":message}))?,
    };
    let postflight = match &o.postflight {
        None => Value::Null,
        Some(Ok(())) => json!({"status":"ok"}),
        Some(Err(message)) => budget.value(&json!({"status":"error","message":message}))?,
    };
    let mut documents: Vec<Value> = Vec::new();
    #[cfg(any(target_vendor = "apple", target_os = "linux"))]
    for document in &o.documents {
        budget.current()?;
        let publication = if let Some(p) = &document.publication {
            let inspection =
                budget.inspection(|| filmcraft_format::retained::inspect(&p.directory, &|| check().map_err(|e| io::Error::other(e.to_string()))))?;
            budget.local(&json!({"directory":p.directory,"receipt":p.receipt,"receiptDurable":p.receipt_durable,"inspection":inspection}))?
        } else {
            Value::Null
        };
        documents.push(budget.value(&json!({"role":document.role,"literal":document.literal,"physical":document.physical,"publication":publication,"qualifiedAtReturn":document.qualified,"error":document.error}))?);
    }
    #[cfg(any(target_vendor = "apple", target_os = "linux"))]
    let previews = o.preview_report.as_ref().map(|report| report.observation(&mut budget)).transpose()?.unwrap_or(Value::Null);
    #[cfg(not(any(target_vendor = "apple", target_os = "linux")))]
    let previews = Value::Null;
    o.workers(s)?;
    budget.current()?;
    budget.finish(json!({"kind":"native_original_Save_effect_observation","version":2,"returnedReceiptVerification":receipts,"operation":o.id,"phase":format!("{:?}",o.phase),
        "planned":planned,"bodyReturned":o.body_returned,"body":body,"postflight":postflight,
        "effectCallReturnsComplete":o.events_complete,"events":events,"documents":documents,"previews":previews,
        "nativeBeforeCurrent":o.before.current(s).is_ok(),"nativeAfterAvailable":o.after.is_some(),"nativeAfterCurrent":o.after.as_ref().is_some_and(|after|after.current(s).is_ok()),
        "effectBoundaryCrossed":o.phase==Phase::AwaitingReconciliation,"readComplete":true,
        "current":false,"allEffectsVerified":false,"writeAuthority":false,"nativeComplete":false,"startupAuthority":false,"workersMayResume":false}))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn complete_effect_serializer_accepts_full_original_bound_and_refuses_next_byte_without_truncation() {
        let check = || Ok(());
        let content = "x".repeat(MAX_BYTES - 2);
        let mut budget = Budget::new(&check);
        assert_eq!(budget.value(&content).unwrap(), json!(content));
        assert_eq!(budget.remaining, 0);
        assert!(Budget::new(&check).value(&(content + "x")).is_err());
    }
    #[test]
    fn stopped_effect_serialization_does_not_continue_to_another_member_or_renew_a_write_budget() {
        use std::cell::Cell;
        let calls = Cell::new(0);
        let check = || {
            calls.set(calls.get() + 1);
            if calls.get() > 3 { Err(error("generated stopped read")) } else { Ok(()) }
        };
        let mut budget = Budget::new(&check);
        assert!(budget.value(&vec!["complete"; 100]).is_err());
        assert!(calls.get() <= 4);
    }
}
