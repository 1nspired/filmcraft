//! Finite descriptive retention of the SAME original file provider.
//! There is deliberately no native dispatch/overwrite/release entry here.
use super::{ADMISSION_PREPARE, ADMISSION_READ, Request, error};
use crate::{Result, Session, project_save::operation};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
const MAX_ENVELOPE: usize = 512 * 1024;
#[derive(Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct Volume {
    root: String,
    device: String,
    minimum_free: u64,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Prepare {
    job: String,
    operation: String,
    capture: String,
    volumes: Vec<Volume>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Read {
    job: String,
    operation: String,
    capture: String,
}
pub(super) struct Slot {
    pub(super) capture: String,
    pub(super) volumes: Vec<Volume>,
    #[cfg(any(target_vendor = "apple", target_os = "linux"))]
    pub(super) original: Option<operation::FileAdmission>,
    // Preserve the complete original descriptive bytes across future consumption.
    pub(super) description: Option<Value>,
}
pub(super) fn supports(command: &str) -> bool {
    matches!(command, ADMISSION_PREPARE | ADMISSION_READ)
}
pub(super) fn parameters(command: &str, p: &Value) -> Result<(Request, String, Option<Vec<Volume>>)> {
    let (r, capture, volumes) = match command {
        ADMISSION_PREPARE => {
            let v: Prepare = super::super::parse(command, p)?;
            (Request { job: v.job, operation: v.operation }, v.capture, Some(v.volumes))
        }
        ADMISSION_READ => {
            let v: Read = super::super::parse(command, p)?;
            (Request { job: v.job, operation: v.operation }, v.capture, None)
        }
        _ => return Err(error("not an original file admission lifecycle call")),
    };
    super::request(&json!({"job":r.job,"operation":r.operation}))?;
    if !crate::editorial_workspace::uuid(&capture) {
        return Err(error("exact original before-frame capture required for file admission"));
    }
    if let Some(volumes) = &volumes {
        if volumes.is_empty() || volumes.len() > 16 {
            return Err(error("one to sixteen explicit native working roots required"));
        }
        for (i, v) in volumes.iter().enumerate() {
            let path = std::path::Path::new(&v.root);
            if v.root.len() > 16384
                || v.root.chars().any(|c| c.is_control())
                || !path.is_absolute()
                || path.components().any(|c| matches!(c, std::path::Component::ParentDir | std::path::Component::CurDir))
                || v.minimum_free > 9_007_199_254_740_991
                || v.device.parse::<u64>().ok().is_none_or(|d| d.to_string() != v.device)
                || volumes.iter().take(i).any(|old| old.root == v.root)
            {
                return Err(error("exact bounded original root/device/reserve policy required"));
            }
        }
    }
    Ok((r, capture, volumes))
}
pub(super) fn prepare(s: &mut Session, p: &Value) -> Result<Value> {
    run(s, ADMISSION_PREPARE, p)
}
pub(super) fn read(s: &mut Session, p: &Value) -> Result<Value> {
    run(s, ADMISSION_READ, p)
}
fn run(s: &mut Session, command: &str, p: &Value) -> Result<Value> {
    let (r, capture, volumes) = parameters(command, p)?;
    let task = s.save_lifecycle_control.clone();
    let fallback = super::Check::new(web_time::Instant::now());
    let check = task.as_ref().map_or(&fallback, |t| t.read_check());
    let current = || {
        task.as_ref().map_or(Ok(()), |t| t.current(command, &r))?;
        check.check()
    };
    current()?;
    if super::status_for(s, &r, &current)?["status"] != "held" {
        return Err(error("same actual pending native Save required for file admission"));
    }
    let mut owner = s.agent_bridge.save_operation.take().ok_or_else(|| error("original native Save owner unavailable"))?;
    let result=std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        #[cfg(any(target_vendor="apple",target_os="linux"))]
        {
            if command==ADMISSION_PREPARE {
                let before=owner.recovery.as_ref().ok_or_else(||error("prepare and physically publish the original before frame first"))?;
                if before.describe(&r.job,&r.operation)["capture"]!=capture||before.describe(&r.job,&r.operation)["phase"]!="Prepared" {return Err(error("file admission must bind the same original before frame"));}
                before.require_published_private_record(s,&owner.key,check)?;
                let volumes=volumes.as_ref().ok_or_else(||error("original explicit volume policy missing"))?;
                if let Some(slot)=&owner.admission {
                    if slot.capture!=capture||slot.volumes!=*volumes {return Err(error("retained file admission cannot adopt another capture or volume policy"));}
                } else {
                    // Install BEFORE capture: an error, Stop or caught panic
                    // can never silently retry with a newer target generation.
                    owner.admission=Some(Slot{capture:capture.clone(),volumes:volumes.clone(),original:None,description:None});
                    let specs=volumes.iter().map(|v|crate::project_save::preview_plan::VolumeSpec{root:v.root.clone().into(),device:v.device.clone(),minimum_free:v.minimum_free}).collect::<Vec<_>>();
                    let original=operation::prepare_dispatch_files(s,&owner.key,&specs,&||current().map_err(|e|std::io::Error::other(e.to_string())))?;
                    owner.admission.as_mut().ok_or_else(||error("original admission slot disappeared"))?.original=Some(original);
                }
                let original=owner.admission.as_ref().and_then(|slot|slot.original.as_ref()).ok_or_else(||error("original file admission attempt failed or was consumed; never recapture"))?;
                operation::validate_file_admission(s,&owner.key,original,&||current().map_err(|e|std::io::Error::other(e.to_string())))?;
            }
            let slot=owner.admission.as_mut().ok_or_else(||error("read requires the same retained original file admission; no capture was started"))?;
            if slot.capture!=capture {return Err(error("foreign original file-admission capture"));}
            if slot.description.is_none() {
                let original=slot.original.as_ref().ok_or_else(||error("original file admission attempt failed before completion; owner remains held"))?;
                slot.description=Some(operation::describe_file_admission(original,&current)?);
            }
            let admission=slot.description.as_ref().ok_or_else(||error("complete original file admission description unavailable"))?;
            let scope=crate::project_workspace::identity(&s.project)?.ok_or_else(||error("same saved original Root identity required"))?.scope;
            super::super::project_plane::require_scope(s,&scope)?;
            let epoch=super::super::history_checkpoint::epoch(s)?;
            current()?;
            let header=json!({"kind":"native_Save_file_admission_read","version":1,"job":r.job,"operation":r.operation,"capture":slot.capture,"scope":scope,"nativeSession":epoch,"nativePid":std::process::id(),"volumes":slot.volumes,"phase":operation::status(s,&owner.key,&current)?["phase"],"current":false,"fileCustody":false,"overwriteAuthority":false,"spaceReserved":false,"writeAuthority":false,"nativeComplete":false,"startupAuthority":false,"workersMayResume":false});
            if serde_json::to_vec(&header).map_err(|e|error(e.to_string()))?.len()>MAX_ENVELOPE {return Err(error("complete original admission envelope exceeds its finite bound"));}
            let mut result=header;result["admission"]=admission.clone();
            if serde_json::to_vec(&result).map_err(|e|error(e.to_string()))?.len()>operation::observation::MAX_BYTES+MAX_ENVELOPE {return Err(error("complete original file admission exceeds its finite reply bound"));}
            current()?;Ok(result)
        }
        #[cfg(not(any(target_vendor="apple",target_os="linux")))]
        {let _=(&mut owner,&capture,&volumes);Err(error("original retained file admission unavailable on this target"))}
    })).unwrap_or_else(|_|Err(error("native file admission failed; same original owner and attempt retained")));
    s.agent_bridge.save_operation = Some(owner);
    result
}
