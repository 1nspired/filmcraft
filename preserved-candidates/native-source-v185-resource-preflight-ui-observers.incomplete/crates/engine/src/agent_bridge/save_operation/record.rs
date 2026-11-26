//! Original retained-key physical intent reads and single-attempt publication.
use super::{RECORD_PREPARE, RECORD_PUBLISH, RECORD_READ, Request, error};
use crate::{Result, Session};
use serde::Deserialize;
use serde_json::{Value, json};
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Prepare {
    job: String,
    operation: String,
    capture: String,
    reserve_bytes: u64,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Read {
    job: String,
    operation: String,
    capture: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Publish {
    job: String,
    operation: String,
    capture: String,
    intent: Value,
}
pub(super) fn supports(command: &str) -> bool {
    matches!(command, RECORD_PREPARE | RECORD_READ | RECORD_PUBLISH)
}
pub(super) fn parameters(command: &str, p: &Value) -> Result<(Request, String, Option<u64>, Option<Value>)> {
    let (request, capture, reserve, intent) = match command {
        RECORD_PREPARE => {
            let v: Prepare = super::super::parse(command, p)?;
            if v.reserve_bytes > 9_007_199_254_740_991 {
                return Err(error("exact bounded original recovery reserve required"));
            }
            (Request { job: v.job, operation: v.operation }, v.capture, Some(v.reserve_bytes), None)
        }
        RECORD_READ => {
            let v: Read = super::super::parse(command, p)?;
            (Request { job: v.job, operation: v.operation }, v.capture, None, None)
        }
        RECORD_PUBLISH => {
            let v: Publish = super::super::parse(command, p)?;
            if !v.intent.is_object() || serde_json::to_vec(&v.intent).map_err(|e| error(e.to_string()))?.len() > 64 * 1024 {
                return Err(error("complete bounded original physical intent required"));
            }
            (Request { job: v.job, operation: v.operation }, v.capture, None, Some(v.intent))
        }
        _ => return Err(error("not an original recovery-record lifecycle call")),
    };
    super::request(&json!({"job":request.job,"operation":request.operation}))?;
    if !crate::editorial_workspace::uuid(&capture) {
        return Err(error("exact original retained native frame capture required"));
    }
    Ok((request, capture, reserve, intent))
}
pub(super) fn prepare(s: &mut Session, p: &Value) -> Result<Value> {
    run(s, RECORD_PREPARE, p)
}
pub(super) fn publish(s: &mut Session, p: &Value) -> Result<Value> {
    run(s, RECORD_PUBLISH, p)
}
pub(super) fn read(s: &mut Session, p: &Value) -> Result<Value> {
    run(s, RECORD_READ, p)
}
fn run(s: &mut Session, command: &str, p: &Value) -> Result<Value> {
    let (r, capture, reserve, intent) = parameters(command, p)?;
    let task = s.save_lifecycle_control.clone();
    let fallback = super::Check::new(web_time::Instant::now());
    let check = task.as_ref().map_or(&fallback, |t| t.read_check());
    let current = || {
        if let Some(t) = &task {
            t.current(command, &r)?;
        }
        check.check()
    };
    current()?;
    let status = super::status_for(s, &r, &current)?;
    if status["status"] != "held" {
        return Err(error("actual original pending Save required for physical intent inspection"));
    }
    let mut owner = s.agent_bridge.save_operation.take().ok_or_else(|| error("original native Save owner unavailable"))?;
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        #[cfg(any(target_vendor = "apple", target_os = "linux"))]
        {
            let frame = owner
                .recovery
                .iter_mut()
                .chain(owner.recovery_after.iter_mut())
                .find(|f| f.describe(&r.job, &r.operation)["capture"] == capture)
                .ok_or_else(|| error("unknown capture; no raw frame or file owner was reconstructed"))?;
            if let Some(reserve) = reserve {
                frame.prepare_private_record(s, &owner.key, reserve, check)?;
            }
            current()?;
            if let Some(intent) = &intent {
                // Uses the original live Key/Record and planning lifetime.
                // JSON can only match that retained intent, never create it.
                frame.publish_private_record(s, &owner.key, intent, check)?;
            }
            current()?;
            let record = frame.inspect_private_record(check)?;
            let frame = frame.describe(&r.job, &r.operation);
            let result = json!({"kind":"native_Save_recovery_record","version":1,"job":r.job,"operation":r.operation,
                "scope":frame["scope"],"nativeSession":frame["nativeSession"],"nativePid":frame["nativePid"],
                "capture":frame["capture"],"phase":frame["phase"],"frameSha256":frame["sha256"],"record":record,
                "current":false,"writeAuthority":false,"nativeComplete":false,"startupAuthority":false,"workersMayResume":false});
            current()?;
            if serde_json::to_vec(&result).map_err(|e| error(e.to_string()))?.len() > 64 * 1024 {
                return Err(error("complete native physical-intent metadata exceeds original resource bound; owner retained"));
            }
            Ok(result)
        }
        #[cfg(not(any(target_vendor = "apple", target_os = "linux")))]
        {
            let _ = (&mut owner, &capture, reserve, &intent, check);
            Err(error("original private record preparation is unsupported on this platform"))
        }
    }))
    .unwrap_or_else(|_| Err(error("native physical-intent read failed; same actual pending key retained")));
    s.agent_bridge.save_operation = Some(owner);
    result
}
