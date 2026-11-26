//! Complete passive editor libraries and session choices shared by native
//! snapshots. This is data, not a worker, SDK key, file grant or command handler.
use super::*;
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
    sync::{Arc, Mutex, atomic::Ordering},
};
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Snapshot {
    version: u8,
    shortcuts: crate::shortcuts::Recovery,
    effects: crate::presets::Recovery,
    exports: crate::export_tools::PresetRecovery,
    queue: crate::export_tools::QueueRecovery,
    browser: crate::media_browser::BrowserState,
    offline: crate::relink::OfflineState,
    origins: BTreeMap<String, Option<PathBuf>>,
    completed_jobs: Vec<CompletedJob>,
    export_offline: BTreeMap<u64, Vec<OfflineItem>>,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Report {
    path: String,
    frames: u64,
    seconds_bits: u64,
    bytes: u64,
    render_fps_bits: u64,
    extra_files: Vec<String>,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Loudness {
    measured_lufs_bits: u64,
    gain_db_bits: u64,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CompletedJob {
    id: u64,
    label: String,
    result: std::result::Result<Report, String>,
    done: u64,
    total: u64,
    cancel: bool,
    finished: bool,
    status: String,
    error: Option<String>,
    loudness: Option<Loudness>,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct OfflineItem {
    item: u64,
    name: String,
    path: String,
    status: String,
    error: String,
}
fn error(message: impl Into<String>) -> crate::EngineError {
    bad("native.snapshotAuxiliary", message)
}
impl CompletedJob {
    fn capture(j: &crate::Job) -> Result<Self> {
        let result = j.result.lock().unwrap_or_else(|e| e.into_inner()).clone().ok_or_else(|| error("a live job cannot become completed recovery data"))?;
        let result = result.map(|r| Report {
            path: r.path,
            frames: r.frames,
            seconds_bits: r.seconds.to_bits(),
            bytes: r.bytes,
            render_fps_bits: r.render_fps.to_bits(),
            extra_files: r.extra_files,
        });
        let p = &j.progress;
        Ok(Self {
            id: j.id,
            label: j.label.clone(),
            result,
            done: p.done.load(Ordering::Relaxed),
            total: p.total.load(Ordering::Relaxed),
            cancel: p.cancel.load(Ordering::Relaxed),
            finished: p.finished.load(Ordering::Relaxed),
            status: p.status.lock().unwrap_or_else(|e| e.into_inner()).clone(),
            error: p.error.lock().unwrap_or_else(|e| e.into_inner()).clone(),
            loudness: p
                .loudness
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .map(|v| Loudness { measured_lufs_bits: v.measured_lufs.to_bits(), gain_db_bits: v.gain_db.to_bits() }),
        })
    }
    fn restore(&self) -> crate::Job {
        // No thread, task, callback or original operation owner is constructed.
        // A present completed result keeps historical jobs inert.
        let result = self.result.clone().map(|r| filmcraft_export::Report {
            path: r.path,
            frames: r.frames,
            seconds: f64::from_bits(r.seconds_bits),
            bytes: r.bytes,
            render_fps: f64::from_bits(r.render_fps_bits),
            extra_files: r.extra_files,
        });
        let progress = Arc::new(filmcraft_export::Progress::default());
        progress.done.store(self.done, Ordering::Relaxed);
        progress.total.store(self.total, Ordering::Relaxed);
        progress.cancel.store(self.cancel, Ordering::Relaxed);
        progress.finished.store(self.finished, Ordering::Relaxed);
        *progress.status.lock().unwrap_or_else(|e| e.into_inner()) = self.status.clone();
        *progress.error.lock().unwrap_or_else(|e| e.into_inner()) = self.error.clone();
        if let Some(v) = &self.loudness {
            let mut target = progress.loudness.lock().unwrap_or_else(|e| e.into_inner());
            let value = target.get_or_insert_with(Default::default);
            value.measured_lufs = f64::from_bits(v.measured_lufs_bits);
            value.gain_db = f64::from_bits(v.gain_db_bits);
        }
        crate::Job { id: self.id, label: self.label.clone(), progress, result: Arc::new(Mutex::new(Some(result))) }
    }
}
impl OfflineItem {
    fn capture(v: &crate::media_pool::OfflineMedia) -> Self {
        Self { item: v.item, name: v.name.clone(), path: v.path.clone(), status: v.status.into(), error: v.error.clone() }
    }
    fn restore(&self) -> Result<crate::media_pool::OfflineMedia> {
        let status = match self.status.as_str() {
            "missing" => "missing",
            "offline" => "offline",
            "unreadable" => "unreadable",
            _ => return Err(error("unknown historical offline media status")),
        };
        Ok(crate::media_pool::OfflineMedia { item: self.item, name: self.name.clone(), path: self.path.clone(), status, error: self.error.clone() })
    }
}
impl Snapshot {
    pub(super) fn capture(s: &Session) -> Result<Self> {
        let snapshot = Self {
            version: 1,
            shortcuts: s.shortcuts.recovery(),
            effects: s.presets.recovery(),
            exports: s.export_presets.recovery(),
            queue: s.export_queue.recovery(),
            browser: s.browser.clone(),
            offline: s.offline.clone(),
            origins: super::history_checkpoint::application_archive::origins(s)?,
            completed_jobs: s.jobs.iter().map(CompletedJob::capture).collect::<Result<_>>()?,
            export_offline: s
                .export_offline
                .iter()
                .map(|(id, rows)| (*id, rows.lock().unwrap_or_else(|e| e.into_inner()).iter().map(OfflineItem::capture).collect()))
                .collect(),
        };
        snapshot.validate()?;
        Ok(snapshot)
    }
    pub(super) fn validate(&self) -> Result<()> {
        if self.version != 1 || self.origins.len() > 65_536 || self.completed_jobs.len() > 65_536 {
            return Err(error("incompatible complete auxiliary recovery"));
        }
        self.queue.validate()?;
        let mut effects = crate::presets::PresetLibrary::default();
        effects.restore_recovery(&self.effects)?;
        let mut exports = crate::export_tools::ExportPresetLibrary::default();
        exports.restore_recovery(&self.exports)?;
        let mut ids = BTreeSet::new();
        for j in &self.completed_jobs {
            if j.id == 0 || j.id > 9_007_199_254_740_991 || !ids.insert(j.id) {
                return Err(error("invalid original completed job identities"));
            }
        }
        for rows in self.export_offline.values() {
            for row in rows {
                row.restore()?;
            }
        }
        // Same canonical native project decoder used for complete history.
        for project in self.queue.projects() {
            let raw = history_checkpoint::project_bytes(project, 32 * 1024 * 1024)?;
            let text = std::str::from_utf8(&raw).map_err(|e| error(e.to_string()))?;
            history_checkpoint::project(text)?;
        }
        Ok(())
    }
    pub(super) fn install(&self, s: &mut Session) -> Result<()> {
        self.validate()?;
        s.shortcuts.restore_recovery(&self.shortcuts);
        s.presets.restore_recovery(&self.effects)?;
        s.export_presets.restore_recovery(&self.exports)?;
        s.export_queue.restore_recovery(&self.queue)?;
        s.browser = self.browser.clone();
        s.offline = self.offline.clone();
        s.jobs = self.completed_jobs.iter().map(CompletedJob::restore).collect();
        s.export_offline = self
            .export_offline
            .iter()
            .map(|(id, rows)| Ok((*id, Arc::new(Mutex::new(rows.iter().map(OfflineItem::restore).collect::<Result<Vec<_>>>()?)))))
            .collect::<Result<_>>()?;
        self.restore_origins(s)?;
        s.native_auxiliary_restored = true;
        Ok(())
    }
    pub(super) fn restore_origins(&self, s: &Session) -> Result<()> {
        s.media.restore_recovery_origins(&self.origins)
    }
    pub(super) fn matches(&self, s: &Session) -> Result<bool> {
        let current = Self::capture(s)?;
        Ok(history_checkpoint::bytes(self, 80 * 1024 * 1024)? == history_checkpoint::bytes(&current, 80 * 1024 * 1024)?)
    }
}
