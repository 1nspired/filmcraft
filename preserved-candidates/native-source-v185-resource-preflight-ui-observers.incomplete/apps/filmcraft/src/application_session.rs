//! Ordinary desktop initialization must never replace an already prepared Session.
//! A prepared Session retains its actual graph, history, preferences, bindings,
//! devices and worker. This module does not admit cold Save or release a gate.
use crate::{
    PreparedUpdate,
    args::{self, Launch},
};
use filmcraft_engine::{
    Session,
    autosave::{AutosaveConfig, default_data_dir},
    voiceover::AudioInput,
};
use serde_json::json;

pub(crate) struct ApplicationSession {
    pub(crate) session: Session,
    prepared: bool,
}

pub(crate) fn prepare(prepared: Option<PreparedUpdate>, launch: &Launch, local_offset: fn(i64) -> i32) -> Result<ApplicationSession, String> {
    if let Some(owner) = prepared {
        let session = owner.lock().unwrap_or_else(|error| error.into_inner()).take()?;
        return Ok(ApplicationSession { session, prepared: true });
    }
    let mut session = Session::default();
    if let Some(dir) = launch.data_dir.clone().or_else(default_data_dir) {
        let mut cfg = AutosaveConfig::new(dir);
        cfg.local_offset = local_offset;
        if let Err(error) = session.start_autosave(cfg) {
            log::warn!("auto-save and crash recovery unavailable: {error}");
        }
    }
    Ok(ApplicationSession { session, prepared: false })
}

impl ApplicationSession {
    pub(crate) fn initialize_voiceover_input(&mut self, create: impl FnOnce(&str) -> Box<dyn AudioInput>) {
        if self.session.voiceover.input.is_none() {
            self.session.voiceover.input = Some(create(&self.session.prefs.audio_hardware.device_class));
        }
    }

    pub(crate) fn initialize_project(&mut self, launch: &Launch) {
        if self.prepared {
            return;
        }
        let Launch { files, demo, startup_flag, recover, .. } = launch;
        let (demo, startup_flag, recover) = (*demo, *startup_flag, *recover);
        let session = &mut self.session;
        let (project, media) = args::project_and_media(files);
        if let Some(p) = project {
            if let Err(e) = session.execute("file.open", json!({"path": p})) {
                log::error!("{e}");
            }
        } else if !startup_flag && session.prefs.general.at_startup == "openMostRecent" {
            // Settings ▸ General ▸ At Startup ▸ Open Most Recent
            let recent = session.prefs.general.recent_projects.iter().find(|p| std::path::Path::new(p).exists()).cloned();
            match recent {
                Some(p) => {
                    if let Err(e) = session.execute("file.open", json!({"path": p})) {
                        log::error!("{e}");
                    }
                }
                None => {
                    let _ = session.execute("file.openDemoProject", json!({}));
                }
            }
        } else if !startup_flag && session.prefs.general.at_startup == "emptyProject" {
        } else if demo {
            let _ = session.execute("file.openDemoProject", json!({}));
        }
        if !media.is_empty() {
            let _ = session.execute("file.import", json!({"paths": media}));
        }
        if recover == Some(true)
            && let Some(id) = session.recovery_candidates().first().map(|candidate| candidate.id.clone())
        {
            match session.execute("file.recover", json!({"id": id})) {
                Ok(r) => log::info!("recovered {r}"),
                Err(e) => log::error!("recovery failed: {e}"),
            }
        }
    }

    pub(crate) fn into_session(self) -> Session {
        self.session
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::startup_session::StartupSession;
    use filmcraft_engine::{autosave, voiceover::SyntheticInput};
    use std::{
        fs,
        path::{Path, PathBuf},
        sync::{
            Arc, Mutex,
            atomic::{AtomicUsize, Ordering},
        },
        time::Duration,
    };

    struct Scratch(PathBuf);
    impl Scratch {
        fn new() -> Self {
            static NEXT: AtomicUsize = AtomicUsize::new(0);
            let path = std::env::temp_dir().join(format!("poema-application-session-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
            fs::create_dir(&path).unwrap();
            Self(fs::canonicalize(path).unwrap())
        }
    }
    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    fn launch(directory: &Path) -> Launch {
        let crate::args::Cli::Run(value) = crate::args::parse(None, ["--empty".into(), "--data-dir".into(), directory.as_os_str().to_owned()]).unwrap() else {
            panic!("generated launch")
        };
        *value
    }
    fn generated(name: &str) -> Session {
        let mut session = Session::default();
        session.execute("file.newProject", json!({"name": name})).unwrap();
        session
    }
    fn project_file(directory: &Path) -> PathBuf {
        let path = directory.join("Foreign.fcproj");
        let mut foreign = generated("Different project");
        foreign.execute("file.newSequence", json!({"name":"Foreign picture"})).unwrap();
        foreign.execute("file.saveAs", json!({"path":path})).unwrap();
        path
    }
    fn owner(session: Session) -> PreparedUpdate {
        Arc::new(Mutex::new(StartupSession::new(session)))
    }

    #[test]
    fn prepared_live_session_retains_original_worker_preferences_history_and_later_journal_writes() {
        let scratch = Scratch::new();
        let directory = scratch.0.join("original-runtime");
        let foreign = project_file(&scratch.0);
        let mut session = generated("Original prepared edit");
        let before_edit = Arc::clone(&session.project);
        session.execute("file.newSequence", json!({"name":"Human history step"})).unwrap();
        session.path = Some(scratch.0.join("original.fcproj").to_str().unwrap().into());
        let mut config = AutosaveConfig::new(&directory);
        config.journal_debounce = Duration::from_secs(3600);
        session.start_autosave(config).unwrap();
        session.prefs.auto_save.enabled = false;
        session.prefs.general.at_startup = "openMostRecent".into();
        session.prefs.general.recent_projects = vec![foreign.to_str().unwrap().into()];
        session.sync_persistence();
        session.persistence.as_ref().unwrap().flush();
        let worker_directory = session.persistence.as_ref().unwrap().session_dir.clone();
        let snapshot_before = fs::read(worker_directory.join("snapshot.fcproj")).unwrap();
        let graph = Arc::clone(&session.project);
        let media = Arc::clone(&session.media);
        let services = Arc::clone(&session.services);
        let editor = serde_json::to_value(&session.state).unwrap();
        let prefs = serde_json::to_value(&session.prefs).unwrap();
        let journal = session.journal.clone();
        let revisions = (session.revision, session.saved_revision);
        let original_path = session.path.clone();
        session.voiceover.input = Some(Box::new(SyntheticInput::clicks()));
        let prepared = owner(session);
        let mut hostile = launch(&scratch.0.join("must-not-create"));
        hostile.files = vec![foreign.to_str().unwrap().into()];
        hostile.demo = true;
        hostile.startup_flag = false;
        hostile.recover = Some(true);
        let mut startup = prepare(Some(prepared.clone()), &hostile, |_| 123).unwrap();
        startup.initialize_voiceover_input(|_| panic!("existing input must not be replaced"));
        startup.initialize_project(&hostile);
        let mut actual = startup.into_session();
        assert!(prepared.lock().unwrap().take().is_err());
        assert!(Arc::ptr_eq(&graph, &actual.project));
        assert!(Arc::ptr_eq(&media, &actual.media));
        assert!(Arc::ptr_eq(&services, &actual.services));
        assert_eq!(serde_json::to_value(&actual.state).unwrap(), editor);
        assert_eq!(serde_json::to_value(&actual.prefs).unwrap(), prefs);
        assert_eq!(actual.prefs_path.as_deref(), Some(directory.join("preferences.json").as_path()));
        assert_eq!(actual.path, original_path);
        assert_eq!(actual.journal, journal);
        assert_eq!((actual.revision, actual.saved_revision), revisions);
        assert_eq!(actual.persistence.as_ref().unwrap().session_dir, worker_directory);
        assert!(!hostile.data_dir.as_ref().unwrap().exists());
        actual.undo().unwrap();
        assert!(Arc::ptr_eq(&before_edit, &actual.project));
        actual.redo().unwrap();
        assert!(Arc::ptr_eq(&graph, &actual.project));
        actual.execute("file.newSequence", json!({"name":"Later native edit"})).unwrap();
        let expected = serde_json::to_value(actual.project.as_ref()).unwrap();
        actual.sync_persistence();
        actual.persistence.as_ref().unwrap().flush();
        assert_ne!(fs::read(worker_directory.join("snapshot.fcproj")).unwrap(), snapshot_before);
        assert!(autosave::scan_candidates(&directory, None).is_empty());
        drop(actual); // Join the same original worker, preserving its dirty journal.
        let candidates = autosave::scan_candidates(&directory, None);
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].dir, worker_directory);
        assert_eq!(serde_json::to_value(autosave::load_candidate(&candidates[0]).unwrap().project).unwrap(), expected);
        assert!(!hostile.data_dir.as_ref().unwrap().exists());
    }

    #[test]
    fn prepared_session_without_worker_does_not_bootstrap_normal_persistence_or_project_fallback() {
        let scratch = Scratch::new();
        let foreign = project_file(&scratch.0);
        let session = generated("Prepared with deferred persistence");
        let graph = Arc::clone(&session.project);
        let prefs = serde_json::to_value(&session.prefs).unwrap();
        let mut hostile = launch(&scratch.0.join("foreign-runtime"));
        hostile.files = vec![foreign.to_str().unwrap().into()];
        hostile.demo = true;
        hostile.recover = Some(true);
        let mut startup = prepare(Some(owner(session)), &hostile, |_| 0).unwrap();
        let mut calls = 0;
        startup.initialize_voiceover_input(|_| {
            calls += 1;
            Box::new(SyntheticInput::clicks())
        });
        startup.initialize_voiceover_input(|_| panic!("input must only be constructed once"));
        startup.initialize_project(&hostile);
        let actual = startup.into_session();
        assert_eq!(calls, 1);
        assert!(Arc::ptr_eq(&graph, &actual.project));
        assert_eq!(serde_json::to_value(&actual.prefs).unwrap(), prefs);
        assert!(actual.persistence.is_none());
        assert!(actual.prefs_path.is_none());
        assert!(!hostile.data_dir.as_ref().unwrap().exists());
    }

    #[test]
    fn ordinary_startup_still_opens_explicit_project_and_owns_original_data_directory() {
        let scratch = Scratch::new();
        let foreign = project_file(&scratch.0);
        let mut launch = launch(&scratch.0.join("ordinary-runtime"));
        launch.files = vec![foreign.to_str().unwrap().into()];
        let mut startup = prepare(None, &launch, |_| 0).unwrap();
        startup.initialize_project(&launch);
        let mut actual = startup.into_session();
        assert_eq!(actual.path.as_deref(), foreign.to_str());
        // Save As uses the chosen filename as the project name. Compare the
        // actual saved project through the same original human Open service.
        let mut reference = Session::default();
        reference.execute("file.open", json!({"path":foreign})).unwrap();
        assert_eq!(serde_json::to_value(actual.project.as_ref()).unwrap(), serde_json::to_value(reference.project.as_ref()).unwrap());
        assert!(actual.persistence.as_ref().unwrap().session_dir.starts_with(launch.data_dir.as_ref().unwrap()));
        assert_eq!(actual.prefs_path.as_deref(), Some(launch.data_dir.as_ref().unwrap().join("preferences.json").as_path()));
        actual.shutdown();
    }

    #[test]
    fn ordinary_startup_keeps_saved_recent_and_empty_project_preferences() {
        let scratch = Scratch::new();
        let foreign = project_file(&scratch.0);
        for setting in ["openMostRecent", "emptyProject"] {
            let directory = scratch.0.join(setting);
            fs::create_dir(&directory).unwrap();
            let mut prefs = autosave::Preferences::default();
            prefs.general.at_startup = setting.into();
            prefs.general.recent_projects = vec![foreign.to_str().unwrap().into()];
            prefs.save(&directory.join("preferences.json")).unwrap();
            let mut launch = launch(&directory);
            launch.startup_flag = false;
            launch.demo = true;
            let mut startup = prepare(None, &launch, |_| 0).unwrap();
            startup.initialize_project(&launch);
            let mut actual = startup.into_session();
            if setting == "openMostRecent" {
                assert_eq!(actual.path.as_deref(), foreign.to_str());
            } else {
                assert!(actual.path.is_none());
                assert!(actual.project.sequences().next().is_none());
            }
            actual.shutdown();
        }
    }

    #[test]
    fn ordinary_recovery_still_restores_original_unsaved_journal() {
        let scratch = Scratch::new();
        let directory = scratch.0.join("ordinary-recovery");
        let mut original = generated("Unsaved recovery project");
        original.start_autosave(AutosaveConfig::new(&directory)).unwrap();
        original.prefs.auto_save.enabled = false;
        original.execute("file.newSequence", json!({"name":"Unsaved picture"})).unwrap();
        let expected = serde_json::to_value(original.project.as_ref()).unwrap();
        original.sync_persistence();
        original.persistence.as_ref().unwrap().flush();
        drop(original);
        assert_eq!(autosave::scan_candidates(&directory, None).len(), 1);
        let mut launch = launch(&directory);
        launch.recover = Some(true);
        let mut startup = prepare(None, &launch, |_| 0).unwrap();
        startup.initialize_project(&launch);
        let mut actual = startup.into_session();
        assert_eq!(serde_json::to_value(actual.project.as_ref()).unwrap(), expected);
        actual.shutdown();
    }
}
