//! Physical Save effects are replaceable; original human policy stays in its
//! one shared service. Events describe call returns, NOT file/owner authority.
use crate::{Services, autosave::Preferences};
use serde::Serialize;
use std::{
    io,
    path::{Path, PathBuf},
};
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub(crate) enum DocumentRole {
    SchemaBackup,
    Project,
}
/// Shared exact original document roles for receiver and complete admission.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub(crate) enum FileRole {
    SchemaBackup,
    Project,
    Preferences,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub(crate) enum DirectoryRole {
    Preferences,
    Previews,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "status", content = "detail", rename_all = "snake_case")]
pub(crate) enum CallResult {
    ReturnedOk { bytes: Option<u64> },
    ReturnedError(String),
}
impl CallResult {
    pub(crate) fn unit(result: &io::Result<()>) -> Self {
        match result {
            Ok(()) => Self::ReturnedOk { bytes: None },
            Err(error) => Self::ReturnedError(error.to_string()),
        }
    }
    pub(crate) fn bytes(result: &io::Result<u64>) -> Self {
        match result {
            Ok(bytes) => Self::ReturnedOk { bytes: Some(*bytes) },
            Err(error) => Self::ReturnedError(error.to_string()),
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(crate) enum Event {
    BoundaryRefused { phase: &'static str, path: PathBuf, error: String },
    Document { role: DocumentRole, path: String, bytes: usize, result: CallResult },
    Directories { role: DirectoryRole, path: PathBuf, result: CallResult },
    Preferences { path: PathBuf, bytes: usize, result: CallResult },
    RecentProject { path: Option<PathBuf>, result: CallResult },
    PreviewRename { from: PathBuf, to: PathBuf, result: CallResult },
    PreviewCopy { from: PathBuf, to: PathBuf, result: CallResult },
    PreviewRemove { path: PathBuf, result: CallResult },
}
/// Trusted native composition only. No serialized callback, raw command,
/// second Save implementation, new model hand or implicit write grant.
pub(crate) trait FileEffects {
    /// Supplemental actual live capacity/owner check, never a serialized grant.
    fn current_save_files(&self) -> io::Result<()> {
        Ok(())
    }
    #[cfg(any(target_vendor = "apple", target_os = "linux"))]
    fn stage_document(
        &mut self,
        _role: FileRole,
        path: &Path,
        operation: &str,
        bytes: &[u8],
        check: &dyn Fn() -> io::Result<()>,
    ) -> io::Result<filmcraft_format::retained::StagedWrite> {
        filmcraft_format::retained::stage(path, operation, bytes, &|| check())
    }
    fn document(&mut self, services: &dyn Services, _role: DocumentRole, path: &str, bytes: &[u8]) -> io::Result<()> {
        services.write_file(path, bytes)
    }
    fn directories(&mut self, _role: DirectoryRole, path: &Path) -> io::Result<()> {
        std::fs::create_dir_all(path)
    }
    fn preferences(&mut self, path: &Path, bytes: &[u8]) -> io::Result<()> {
        filmcraft_format::atomic_write(path, bytes)
    }
    fn preview_exists(&mut self, path: &Path) -> bool {
        path.exists()
    }
    fn preview_rename(&mut self, from: &Path, to: &Path) -> io::Result<()> {
        std::fs::rename(from, to)
    }
    fn preview_copy(&mut self, from: &Path, to: &Path) -> io::Result<u64> {
        std::fs::copy(from, to)
    }
    fn preview_remove(&mut self, path: &Path) -> io::Result<()> {
        std::fs::remove_file(path)
    }
    fn event(&mut self, _event: Event) {}
}
pub(crate) struct Original;
impl FileEffects for Original {}
pub(crate) fn document(files: &mut dyn FileEffects, services: &dyn Services, role: DocumentRole, path: &str, bytes: &[u8]) -> io::Result<()> {
    let result = files.document(services, role, path, bytes);
    files.event(Event::Document { role, path: path.into(), bytes: bytes.len(), result: CallResult::unit(&result) });
    result
}
pub(crate) fn directories(files: &mut dyn FileEffects, role: DirectoryRole, path: &Path) -> io::Result<()> {
    let result = files.directories(role, path);
    files.event(Event::Directories { role, path: path.into(), result: CallResult::unit(&result) });
    result
}
pub(crate) fn preference_bytes(prefs: &Preferences) -> Vec<u8> {
    serde_json::to_vec_pretty(prefs).unwrap_or_default()
}
pub(crate) fn preferences(files: &mut dyn FileEffects, prefs: &Preferences, path: &Path) -> io::Result<()> {
    // Original pretty serializer and original error fallback, shared by every
    // normal preference command and Save's recent-project update.
    if let Some(parent) = path.parent() {
        directories(files, DirectoryRole::Preferences, parent)?;
    }
    let bytes = preference_bytes(prefs);
    let result = files.preferences(path, &bytes);
    files.event(Event::Preferences { path: path.into(), bytes: bytes.len(), result: CallResult::unit(&result) });
    result
}
