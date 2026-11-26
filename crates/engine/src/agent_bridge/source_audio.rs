//! Source-only metadata through the existing inspection worker and postflight.
//! Never uses an item/path-only decoder cache or changes project/history state.
use super::{BINDING_KEY, Binding, admission::Generation, media_guard};
use crate::{Result, Services, commands::bad, media_pool::MediaPool};
use filmcraft_project::{Interpretation, ItemId, MediaRef, Project};
use serde_json::{Value, json};
use std::{path::Path, sync::Arc};

fn error(message: impl Into<String>) -> crate::EngineError {
    bad("agent.inspect", message)
}

pub(super) fn validate(project: &Project, project_id: Option<&str>, item: u64, generation: &Generation) -> Result<()> {
    if !(1..=9_007_199_254_740_991).contains(&item) || !generation.valid() {
        return Err(error("exact source identity and complete byte generation required"));
    }
    let source = project.item(ItemId(item)).ok_or_else(|| error("source item is missing"))?;
    let media = source.as_media().ok_or_else(|| error("source layout requires imported file media"))?;
    if media.offline
        || media.proxy.is_some()
        || media.interpret != Interpretation::default()
        || !matches!(&media.media, MediaRef::File {path} if Path::new(path).is_absolute() && path.len()<=4096 && !path.chars().any(char::is_control))
    {
        return Err(error("source layout requires an online original without proxy or interpretation overrides"));
    }
    if let Some(raw) = source.metadata.get(BINDING_KEY) {
        let binding: Binding = serde_json::from_str(raw).map_err(|e| error(e.to_string()))?;
        if project_id.is_some_and(|p| binding.project_id != p) || binding.role != "original" {
            return Err(error("source layout belongs to another owner or a proxy"));
        }
    }
    Ok(())
}

pub(super) fn read(
    project: &Project,
    item: u64,
    generation: &Generation,
    pool: &MediaPool,
    services: &dyn Services,
    check: &media_guard::Check,
) -> Result<(Value, Arc<media_guard::AdmissionWitness>)> {
    validate(project, None, item, generation)?;
    let media = project.item(ItemId(item)).and_then(|i| i.as_media()).ok_or_else(|| error("source disappeared"))?;
    let MediaRef::File { path } = &media.media else { return Err(error("source is not a file")) };
    let witness = media_guard::admit(Path::new(path), generation.bytes, &generation.sha256, check)?;
    check.check()?;
    let fresh = pool.probe_file(path, services).map_err(|e| error(e.to_string()))?;
    check.check()?;
    if fresh.info() != &media.info {
        return Err(error("source metadata differs from the imported original; refresh/reimport before using its layout"));
    }
    let layout = fresh.audio_channel_layout().map_err(|e| error(e.to_string()))?;
    check.check()?;
    media_guard::validate_admission(&witness, check)?;
    Ok((json!({"kind":"source_audio_layout","item":item,"generation":generation,"layout":layout,"proxies":false,"readOnly":true}), witness))
}
