//! The original human Save/Save As/Save Copy service, with immutable document
//! preparation shared by the future typed persistence hand. A prepared value is
//! NOT a file grant: destination custody, collision checks, durable receipts and
//! manager path adoption must be provided by that separate lifecycle.
use crate::{EngineError, History, Result, Services, Session, autosave, previews, saved_file};
use filmcraft_project::Project;
use serde_json::{Value, json};
use std::{
    path::PathBuf,
    sync::{Arc, atomic::Ordering},
};
pub(crate) mod effects;
pub(crate) mod intent;
#[cfg(not(target_arch = "wasm32"))]
pub(crate) mod operation;
#[cfg(any(target_vendor = "apple", target_os = "linux"))]
pub(crate) mod preview_plan;
#[cfg(not(target_arch = "wasm32"))]
pub(crate) mod quiescence;
#[cfg(any(target_vendor = "apple", target_os = "linux"))]
pub(crate) mod retained_effects;

struct Basis {
    project: Arc<Project>,
    services: Arc<dyn Services>,
    path: Option<String>,
    revision: u64,
    saved_revision: u64,
    saved_file: Option<saved_file::Baseline>,
    loaded_schema: u32,
    editor: Value,
    history: History,
    prefs: autosave::Preferences,
    prefs_path: Option<PathBuf>,
    previews: Arc<previews::PreviewStore>,
    preview_dir: Option<PathBuf>,
    preview_temp: Option<PathBuf>,
    preview_generation: u64,
    media: Arc<crate::MediaPool>,
    media_proxies: bool,
}
fn editor(s: &Session) -> Result<Value> {
    serde_json::to_value(&s.state).map_err(|e| EngineError::Other(format!("project save editor basis: {e}")))
}
fn branch_same(a: &[(String, Arc<Project>)], b: &[(String, Arc<Project>)]) -> bool {
    a.len() == b.len() && a.iter().zip(b).all(|(a, b)| a.0 == b.0 && Arc::ptr_eq(&a.1, &b.1))
}
impl Basis {
    fn capture(s: &Session) -> Result<Self> {
        Ok(Self {
            project: s.project.clone(),
            services: s.services.clone(),
            path: s.path.clone(),
            revision: s.revision,
            saved_revision: s.saved_revision,
            saved_file: s.saved_file.clone(),
            loaded_schema: s.loaded_schema,
            editor: editor(s)?,
            history: s.history.clone(),
            prefs: s.prefs.clone(),
            prefs_path: s.prefs_path.clone(),
            previews: s.previews.clone(),
            preview_dir: s.previews.dir(),
            preview_temp: s.previews.temp_root(),
            preview_generation: s.previews.generation.load(Ordering::Acquire),
            media: s.media.clone(),
            media_proxies: s.media.use_proxies(),
        })
    }
    fn current(&self, s: &Session) -> Result<()> {
        self.current_with_preference_path(s, self.prefs_path == s.prefs_path)
    }
    #[cfg(all(not(target_arch = "wasm32"), any(target_vendor = "apple", target_os = "linux")))]
    fn current_cold_binding(&self, s: &Session, bindings: Option<&crate::agent_bridge::ColdServiceBindings>) -> Result<()> {
        let preference_current = if let Some(bindings) = bindings {
            bindings.preference_current(s, &self.prefs_path)?;
            true
        } else {
            self.prefs_path == s.prefs_path
        };
        self.current_with_preference_path(s, preference_current)
    }
    fn current_with_preference_path(&self, s: &Session, preference_current: bool) -> Result<()> {
        if !Arc::ptr_eq(&self.project, &s.project)
            || !Arc::ptr_eq(&self.services, &s.services)
            || self.path != s.path
            || self.revision != s.revision
            || self.saved_revision != s.saved_revision
            || self.saved_file != s.saved_file
            || self.loaded_schema != s.loaded_schema
            || self.editor != editor(s)?
            || !branch_same(&self.history.undo, &s.history.undo)
            || !branch_same(&self.history.redo, &s.history.redo)
            || self.history.limit != s.history.limit
            || self.history.merge_key != s.history.merge_key
            || self.prefs != s.prefs
            || !preference_current
            || !Arc::ptr_eq(&self.previews, &s.previews)
            || self.preview_dir != s.previews.dir()
            || self.preview_temp != s.previews.temp_root()
            || self.preview_generation != s.previews.generation.load(Ordering::Acquire)
            || !Arc::ptr_eq(&self.media, &s.media)
            || self.media_proxies != s.media.use_proxies()
        {
            return Err(EngineError::Other("Prepared project save is stale; no file was written by this publication.".into()));
        }
        Ok(())
    }
}

pub(crate) struct Prepared {
    basis: Basis,
    project: Arc<Project>,
    path: String,
    adopt: bool,
    bytes: Vec<u8>,
    began: web_time::Instant,
    revision_after: u64,
    input: Input,
}
pub(crate) struct Guard(Basis);
pub(crate) fn guard(s: &Session) -> Result<Guard> {
    Ok(Guard(Basis::capture(s)?))
}
impl Guard {
    #[cfg(all(test, not(target_arch = "wasm32"), any(target_vendor = "apple", target_os = "linux")))]
    pub(crate) fn current_cold_binding(&self, s: &Session, bindings: &crate::agent_bridge::ColdServiceBindings) -> Result<()> {
        self.0.current_cold_binding(s, Some(bindings))
    }
    pub(crate) fn current(&self, s: &Session) -> Result<()> {
        self.0.current(s)
    }
    /// Move genuine original encoded bytes; neither JSON nor a digest can bind.
    pub(crate) fn bind_encoded(&self, s: &Session, encoded: Encoded, path: &str, adopt: bool) -> Result<Prepared> {
        self.current(s)?;
        if encoded.path != path || encoded.adopt != adopt || encoded.input.target != path || encoded.input.adopt != adopt {
            return Err(EngineError::Other("Encoded original Save belongs to a different native intent/destination.".into()));
        }
        encoded.input.current(s)?;
        let basis = Basis::capture(s)?;
        let Encoded { project, path, adopt, bytes, began, revision_after, input } = encoded;
        Ok(Prepared { basis, project, path, adopt, bytes, began, revision_after, input })
    }
}
#[derive(Clone)]
pub(crate) struct Input {
    project: Arc<Project>,
    view: filmcraft_project::ProjectView,
    target: String,
    adopt: bool,
    began: web_time::Instant,
    path: Option<String>,
    revision: u64,
    saved_revision: u64,
    saved_file: Option<saved_file::Baseline>,
    loaded_schema: u32,
    prefs: autosave::Preferences,
    prefs_path: Option<PathBuf>,
    preview_dir: Option<PathBuf>,
    preview_temp: Option<PathBuf>,
    preview_generation: u64,
    media_proxies: bool,
}
pub(crate) struct Encoded {
    project: Arc<Project>,
    path: String,
    adopt: bool,
    bytes: Vec<u8>,
    began: web_time::Instant,
    revision_after: u64,
    input: Input,
}
pub(crate) fn input(s: &Session, path: &str, adopt: bool) -> Input {
    Input {
        project: s.project.clone(),
        view: s.project_view(),
        target: path.into(),
        adopt,
        began: web_time::Instant::now(),
        path: s.path.clone(),
        revision: s.revision,
        saved_revision: s.saved_revision,
        saved_file: s.saved_file.clone(),
        loaded_schema: s.loaded_schema,
        prefs: s.prefs.clone(),
        prefs_path: s.prefs_path.clone(),
        preview_dir: s.previews.dir(),
        preview_temp: s.previews.temp_root(),
        preview_generation: s.previews.generation.load(Ordering::Acquire),
        media_proxies: s.media.use_proxies(),
    }
}
impl Input {
    fn current(&self, s: &Session) -> Result<()> {
        if !Arc::ptr_eq(&self.project, &s.project)
            || self.view != s.project_view()
            || self.path != s.path
            || self.revision != s.revision
            || self.saved_revision != s.saved_revision
            || self.saved_file != s.saved_file
            || self.loaded_schema != s.loaded_schema
            || self.prefs != s.prefs
            || self.prefs_path != s.prefs_path
            || self.preview_dir != s.previews.dir()
            || self.preview_temp != s.previews.temp_root()
            || self.preview_generation != s.previews.generation.load(Ordering::Acquire)
            || self.media_proxies != s.media.use_proxies()
        {
            return Err(EngineError::Other("Original encoded Save input is stale; no re-encoding or publication.".into()));
        }
        Ok(())
    }
    pub(crate) fn compute(&self, check: &impl Fn() -> Result<()>) -> Result<Encoded> {
        check()?;
        let proxy_change = self.adopt
            && self.prefs.general.recent_projects.first().map(String::as_str) != Some(self.target.as_str())
            && self.media_proxies != self.prefs.media.enable_proxies;
        let revision_after = if proxy_change {
            self.revision
                .checked_add(1)
                .ok_or_else(|| EngineError::Other("Native revision space exhausted during Save preference synchronization; nothing written.".into()))?
        } else {
            self.revision
        };
        let mut project = self.project.clone();
        let preserve_title = crate::project_workspace::preserves_display_title(&project)?;
        if self.adopt
            && !preserve_title
            && let Some(name) = crate::commands::project_name_for(&self.target)
            && project.name != name
        {
            let p = Arc::make_mut(&mut project);
            p.root.name.clone_from(&name);
            p.name = name;
        }
        check()?;
        let bytes = filmcraft_format::encode_with_view(&project, Some(&self.view), false);
        check()?;
        Ok(Encoded { project, path: self.target.clone(), adopt: self.adopt, bytes, began: self.began, revision_after, input: self.clone() })
    }
}
impl Encoded {
    #[cfg(test)]
    pub(crate) fn payload_identity(&self) -> (usize, usize) {
        (self.bytes.as_ptr() as usize, self.bytes.len())
    }
    pub(crate) fn description(&self) -> Value {
        describe(&self.input, &self.project, &self.path, self.adopt, &self.bytes, self.revision_after)
    }
}
/// Pure document preparation through the same immutable encoder used by reads.
pub(crate) fn prepare(s: &Session, path: &str, adopt: bool) -> Result<Prepared> {
    let basis = Basis::capture(s)?;
    let Encoded { project, path, adopt, bytes, began, revision_after, input } = input(s, path, adopt).compute(&|| Ok(()))?;
    Ok(Prepared { basis, project, path, adopt, bytes, began, revision_after, input })
}
impl Prepared {
    #[cfg(test)]
    pub(crate) fn payload_identity(&self) -> (usize, usize) {
        (self.bytes.as_ptr() as usize, self.bytes.len())
    }
    pub(crate) fn description(&self) -> Value {
        describe(&self.input, &self.project, &self.path, self.adopt, &self.bytes, self.revision_after)
    }
    pub(crate) fn still_current(&self, s: &Session) -> Result<()> {
        self.basis.current(s)
    }
    /// One-shot ownership of the prepared bytes. This retains the exact human
    /// service behavior; it does NOT supply conditional filesystem publication,
    /// a cancellation receipt, manager rebind or rights to overwrite a target.
    pub(crate) fn publish(self, s: &mut Session) -> Result<Value> {
        self.publish_with(s, &mut effects::Original)
    }
    /// The same human Save body with an explicit physical-effects receiver.
    /// The receiver must own native/file/worker custody before it writes;
    /// call-return events alone do not establish publication or completion.
    pub(crate) fn publish_with(self, s: &mut Session, files: &mut dyn effects::FileEffects) -> Result<Value> {
        self.basis.current(s)?;
        let Self { project, path, adopt, bytes, began, .. } = self;
        let mut backup = None;
        if adopt && s.path.as_deref() == Some(path.as_str()) && s.loaded_schema < filmcraft_format::SCHEMA_VERSION {
            let b = schema_backup_path(&path, s.loaded_schema);
            if s.services.read_file(&b).is_err()
                && let Ok(old) = s.services.read_file(&path)
            {
                effects::document(files, &*s.services, effects::DocumentRole::SchemaBackup, &b, &old).map_err(|e| EngineError::Other(format!("{b}: {e}")))?;
                backup = Some(b);
            }
        }
        effects::document(files, &*s.services, effects::DocumentRole::Project, &path, &bytes).map_err(|e| EngineError::Other(format!("{path}: {e}")))?;
        if adopt {
            s.project = project;
            s.path = Some(path.to_string());
            s.saved_file = Some(crate::saved_file::Baseline::from_bytes(&path, &bytes));
            s.previews_follow_path_with(files);
            s.note_recent_project_with(files);
            s.saved_revision = s.revision;
            s.loaded_schema = filmcraft_format::SCHEMA_VERSION;
        } else if s.path.as_deref() == Some(path.as_str()) {
            // Preserve the human same-path Copy policy: the overwritten bytes
            // cannot leave an obsolete recovery baseline on the current owner.
            s.saved_file = None;
        }
        Ok(json!({"path":path,"bytes":bytes.len(),"ms":began.elapsed().as_secs_f64()*1000.0,"backup":backup}))
    }
}
fn describe(input: &Input, project: &Project, path: &str, adopt: bool, bytes: &[u8], revision_after: u64) -> Value {
    let prefs_change = adopt && input.prefs.general.recent_projects.first().map(String::as_str) != Some(path);
    let preview_destination = adopt.then(|| crate::project_tools::previews_dir_for(&project, &path, &input.prefs.auto_save.scratch_folders));
    let preview_from = input.preview_dir.as_ref();
    let move_temporary = preview_destination.as_ref().is_some_and(|to| {
        preview_from
            .is_some_and(|old| old != to && (old.starts_with(crate::temp_dir()) || input.preview_temp.as_ref().is_some_and(|root| old.starts_with(root))))
    });
    let backup = (adopt && input.path.as_deref() == Some(path) && input.loaded_schema < filmcraft_format::SCHEMA_VERSION)
        .then(|| schema_backup_path(&path, input.loaded_schema));
    json!({
        "output":{"path":path,"bytes":bytes.len(),"sha256":saved_file::sha256(bytes),"targetObserved":false},
        "document":{"pathAfter":if adopt {Some(path)}else{input.path.as_deref()},"titleAfter":project.name,"rootTitleAfter":project.root.name,
            "revisionAfter":revision_after,"savedRevisionAfter":if adopt {revision_after}else{input.saved_revision},"dirtyAfter":!adopt&&input.revision!=input.saved_revision,
            "savedBaselineAfter":if adopt {"replace_with_written_bytes"}else if input.path.as_deref()==Some(path) {"clear"}else{"retain"},
            "pathChanges":adopt&&input.path.as_deref()!=Some(path),"historicalSavedBaseline":input.saved_file},
        "effects":{"schemaBackupCandidate":backup,"backupCondition":"only_if_current_old_schema_backup_unreadable_and_original_readable",
            "preferences":{"updatesMemory":prefs_change,"fileCandidate":if prefs_change {input.prefs_path.as_ref()}else{None},"errorPolicy":"original_best_effort_auxiliary"},
            "previews":{"from":preview_from,"to":preview_destination,"mayMoveTemporaryFiles":move_temporary,"generation":input.preview_generation.to_string(),"fileCatalogObserved":false,"quiescenceRequiredBeforePublication":adopt},
            "destinationChecksRequired":true,"auxiliaryChecksRequired":adopt,"nativeUndo":false},
        "fileCustody":false,"overwriteAuthority":false,"startupAuthority":false,"allEffectsVerified":false
    })
}

fn schema_backup_path(path: &str, schema: u32) -> String {
    let stem = path.strip_suffix(".fcproj").unwrap_or(path);
    format!("{stem} (schema v{schema} backup).fcproj")
}
#[cfg(test)]
mod tests;

#[cfg(test)]
mod effect_tests;
