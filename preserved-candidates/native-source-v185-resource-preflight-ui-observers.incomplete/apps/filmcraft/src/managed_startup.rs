//! Trusted startup preparation before a UI, logging root or worker exists.
//! Native capsules stay opaque. No project-file fallback, activation or model.
use crate::args::Launch;
use filmcraft_engine::{Session, autosave::AutosaveConfig};
pub fn prepare(launch: &Launch, local_offset: fn(i64) -> i32) -> Result<Option<Session>, String> {
    if launch.cold_save_recovery.is_some() {
        return Err("cold Save must retain its exclusive pre-UI pipe entry; no ordinary managed startup fallback".into());
    }
    let (raw, kind) = match (&launch.workspace_update, &launch.identity_recovery, &launch.project_recovery, &launch.project_create) {
        (Some(raw), None, None, None) => (raw, 0),
        (None, Some(raw), None, None) => (raw, 1),
        (None, None, Some(raw), None) => (raw, 2),
        (None, None, None, Some(raw)) => (raw, 3),
        (None, None, None, None) => return Ok(None),
        _ => return Err("choose one exact managed native startup".into()),
    };
    if raw.is_empty()
        || raw.len() > 16384
        || launch.control_port.is_none()
        || launch.control_input_only
        || !launch.files.is_empty()
        || launch.demo
        || !launch.startup_flag
        || launch.recover != Some(false)
    {
        return Err("managed native startup requires exclusive bounded descriptor/control mode without fallback".into());
    }
    let directory = launch.data_dir.as_ref().filter(|p| p.is_absolute()).ok_or("fresh absolute native data directory required")?;
    let descriptor: serde_json::Value = serde_json::from_str(raw).map_err(|e| e.to_string())?;
    if kind == 3 {
        if directory.parent().is_none_or(|p| std::fs::canonicalize(p).ok().as_deref() != Some(p)) {
            return Err("new project data root needs a canonical existing parent".into());
        }
        match std::fs::symlink_metadata(directory) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            _ => return Err("new project data directory must not already exist".into()),
        }
        if let Some(binding) = &launch.poema_composer {
            binding.validate()?;
            if binding.version != 2
                || descriptor["projectPath"] != binding.project_path
                || descriptor["workspace"] != binding.workspace_id
                || descriptor["scope"]["workspaceId"] != binding.workspace_id
                || descriptor["scope"]["projectId"] != binding.project_id
            {
                return Err("new project requires its exact independent composer binding".into());
            }
        }
    }
    let mut session = Session::default();
    match kind {
        0 => session.open_workspace_update(&descriptor),
        1 => session.open_identity_publication(&descriptor),
        2 => session.open_project_recovery(&descriptor),
        _ => session.create_project_workspace(&descriptor),
    }
    .map_err(|e| e.to_string())?;
    if let Some(binding) = &launch.poema_composer {
        binding.validate()?;
        if session.path.as_deref() != Some(binding.project_path.as_str())
            || descriptor["workspace"] != binding.workspace_id
            || descriptor["scope"]["projectId"] != binding.project_id
            || binding.version == 2 && !binding.matches_session(&session)?
        {
            return Err("embedded composer differs from the exact managed native workspace".into());
        }
    }
    let mut cfg = AutosaveConfig::new(directory);
    cfg.local_offset = local_offset;
    session.configure_workspace_update_persistence(cfg).map_err(|e| e.to_string())?;
    if let Some(root) = &launch.poema_save_root {
        #[cfg(any(target_vendor = "apple", target_os = "linux"))]
        session.retain_managed_save_startup_root(root).map_err(|e| e.to_string())?;
        #[cfg(not(any(target_vendor = "apple", target_os = "linux")))]
        return Err("managed native Save is unavailable on this platform".into());
    }
    Ok(Some(session))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};
    use sha2::{Digest, Sha256};
    use std::{
        fs,
        path::PathBuf,
        sync::atomic::{AtomicUsize, Ordering},
    };
    const W: &str = "11111111-1111-4111-8111-111111111111";
    const M: &str = "22222222-2222-4222-8222-222222222222";
    struct Scratch(PathBuf);
    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    fn fixture() -> (Scratch, Launch, Vec<u8>) {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!("poema-managed-startup-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
        fs::create_dir(&dir).unwrap();
        let dir = Scratch(fs::canonicalize(dir).unwrap());
        let mut s = Session::default();
        s.execute("file.newProject", json!({"name":"Generated legacy"})).unwrap();
        let q = s.execute("file.newSequence", json!({"name":"Real anchor"})).unwrap()["sequence"].as_u64().unwrap();
        let scope = json!({"projectId":"p_startup","anchorSequence":q});
        s.execute("project.associateEditorialOwner", json!({"scope":scope,"workspaceId":W})).unwrap();
        s.path = Some(dir.0.join("generated.fcproj").to_str().unwrap().into());
        s.execute("file.save", json!({})).unwrap();
        s.history = Default::default();
        s.history.limit = 200;
        let worker = filmcraft_engine::session_worker::SessionWorker::new().unwrap();
        worker.install(&mut s).unwrap();
        s.agent_operation_control().enable_transport();
        let token = format!("{:064x}", 1);
        s.execute("agent.readHistoryCheckpointBasis", json!({"token":token,"scope":scope})).unwrap();
        let inspection = s.execute("agent.inspectIdentityMigration", json!({"token":token,"scope":scope,"workspace":W})).unwrap();
        let token = format!("{:064x}", 2);
        s.execute("agent.readHistoryCheckpointBasis", json!({"token":token,"scope":scope})).unwrap();
        let bytes = fs::read(s.path.as_ref().unwrap()).unwrap();
        let expected = json!({"sourceGraphSha256":inspection["sourceGraph"]["sha256"],"candidateGraphSha256":inspection["candidateGraph"]["sha256"],"savedFileSha256":format!("{:x}",Sha256::digest(&bytes))});
        let published =
            s.execute("agent.publishIdentityMigration", json!({"token":token,"scope":scope,"workspace":W,"migration":M,"expected":expected})).unwrap();
        let descriptor=json!({"projectPath":s.path,"scope":scope,"workspace":W,"migration":M,"expected":expected,"sha256":published["publication"]["sha256"],"activationToken":format!("{:064x}",3)}).to_string();
        let data = dir.0.join("candidate-state").to_str().unwrap().to_string();
        let binding = filmcraft_ui_egui::composer_panel::Binding {
            version: 2,
            http_port: 55001,
            workspace_id: W.into(),
            project_id: "p_startup".into(),
            project_path: s.path.clone().unwrap(),
            application_release: "a".repeat(64),
        };
        let binding = serde_json::to_string(&binding).unwrap();
        let crate::args::Cli::Run(launch) = crate::args::parse(
            None,
            ["--control", "55002", "--data-dir", &data, "--identity-recovery", &descriptor, "--poema-composer", &binding].into_iter().map(Into::into),
        )
        .unwrap() else {
            panic!("run expected")
        };
        (dir, *launch, bytes)
    }
    #[test]
    fn native_identity_startup_restores_the_exact_root_candidate_paused_without_saved_file_or_preferences_writes_then_activates_explicitly() {
        let (dir, launch, bytes) = fixture();
        let mut s = prepare(&launch, |_| 0).unwrap().unwrap();
        let binding = launch.poema_composer.as_ref().unwrap();
        assert!(binding.matches_session(&s).unwrap());
        assert!(s.is_display_only());
        assert_eq!(s.workspace_update_status().unwrap()["status"], "candidate_ready");
        assert_eq!(fs::read(&binding.project_path).unwrap(), bytes);
        assert!(!launch.data_dir.as_ref().unwrap().join("preferences.json").exists());
        assert!(s.execute("file.save", json!({})).is_err());
        let worker = filmcraft_engine::session_worker::SessionWorker::new().unwrap();
        worker.install(&mut s).unwrap();
        s.agent_operation_control().enable_transport();
        let fence = s.workspace_update_status().unwrap();
        s.execute("agent.activateWorkspaceUpdate",json!({"token":fence["token"],"scope":fence["scope"],"workspace":fence["workspace"],"update":fence["update"],"nativeSession":fence["nativeSession"]})).unwrap();
        assert!(!s.is_display_only());
        assert!(launch.data_dir.as_ref().unwrap().join("preferences.json").exists());
        assert_eq!(fs::read(&binding.project_path).unwrap(), bytes);
        s.shutdown();
        assert!(dir.0.exists());
    }
    #[test]
    fn prepared_desktop_transfer_keeps_original_paused_identity_and_deferred_worker_configuration() {
        let (dir, launch, bytes) = fixture();
        let session = prepare(&launch, |_| 0).unwrap().unwrap();
        assert!(session.is_display_only());
        assert!(session.ensure_desktop_startup_transfer().is_ok());
        let graph = std::sync::Arc::clone(&session.project);
        let status = session.workspace_update_status().unwrap();
        let owner = std::sync::Arc::new(std::sync::Mutex::new(crate::startup_session::StartupSession::new(session)));
        let mut hostile = launch.clone();
        hostile.data_dir = Some(dir.0.join("foreign-desktop-runtime"));
        hostile.files = vec![launch.poema_composer.as_ref().unwrap().project_path.clone()];
        hostile.demo = true;
        hostile.recover = Some(true);
        let mut desktop = crate::application_session::prepare(Some(owner.clone()), &hostile, |_| 123).unwrap();
        desktop.initialize_project(&hostile);
        let mut actual = desktop.into_session();
        assert!(owner.lock().unwrap().take().is_err());
        assert!(std::sync::Arc::ptr_eq(&graph, &actual.project));
        assert_eq!(actual.workspace_update_status().unwrap(), status);
        assert!(actual.is_display_only());
        assert!(!hostile.data_dir.as_ref().unwrap().exists());
        assert!(!launch.data_dir.as_ref().unwrap().join("preferences.json").exists());
        let worker = filmcraft_engine::session_worker::SessionWorker::new().unwrap();
        worker.install(&mut actual).unwrap();
        actual.agent_operation_control().enable_transport();
        actual.execute("agent.activateWorkspaceUpdate", json!({"token":status["token"],"scope":status["scope"],"workspace":status["workspace"],"update":status["update"],"nativeSession":status["nativeSession"]})).unwrap();
        assert!(!actual.is_display_only());
        assert!(launch.data_dir.as_ref().unwrap().join("preferences.json").exists());
        assert!(actual.persistence.as_ref().unwrap().session_dir.starts_with(launch.data_dir.as_ref().unwrap()));
        assert!(!hostile.data_dir.as_ref().unwrap().exists());
        assert_eq!(fs::read(&launch.poema_composer.as_ref().unwrap().project_path).unwrap(), bytes);
        actual.shutdown();
    }
    #[test]
    fn native_identity_startup_refuses_foreign_binding_corrupt_decisions_and_existing_data_without_saved_file_fallback() {
        for mode in ["binding", "decision", "existing_data", "saved_file"] {
            let (_dir, mut launch, bytes) = fixture();
            let file = launch.poema_composer.as_ref().unwrap().project_path.clone();
            match mode {
                "binding" => launch.poema_composer.as_mut().unwrap().project_id = "p_other".into(),
                "decision" => {
                    let mut d: Value = serde_json::from_str(launch.identity_recovery.as_ref().unwrap()).unwrap();
                    d["sha256"] = json!("0".repeat(64));
                    launch.identity_recovery = Some(d.to_string());
                }
                "existing_data" => {
                    let root = launch.data_dir.as_ref().unwrap();
                    fs::create_dir(root).unwrap();
                    fs::write(root.join("existing.txt"), b"keep").unwrap();
                }
                "saved_file" => {
                    fs::write(&file, b"changed human saved baseline").unwrap();
                }
                _ => unreachable!(),
            }
            assert!(prepare(&launch, |_| 0).is_err(), "{mode}");
            if mode != "saved_file" {
                assert_eq!(fs::read(&file).unwrap(), bytes);
            } else {
                assert_eq!(fs::read(&file).unwrap(), b"changed human saved baseline");
            }
            assert!(!launch.data_dir.as_ref().unwrap().join("preferences.json").exists());
        }
    }
    fn root_fixture() -> (Scratch, Launch, Vec<u8>) {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let root = std::env::temp_dir().join(format!("poema-root-app-startup-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
        fs::create_dir(&root).unwrap();
        let dir = Scratch(fs::canonicalize(root).unwrap());
        let scope = json!({"projectId":"p_root_startup","workspaceId":W});
        let token = |n| format!("{n:064x}");
        let checkpoint = "33333333-3333-4333-8333-333333333333";
        let mut source = Session::default();
        source.execute("file.newProject", json!({"name":"Root","workspace":scope})).unwrap();
        source.start_autosave(AutosaveConfig::new(dir.0.join("source-state"))).unwrap();
        source.execute("file.saveAs", json!({"path":dir.0.join("root.fcproj").to_str().unwrap()})).unwrap();
        source.execute("file.newBin", json!({"name":"Human bin before any sequence"})).unwrap();
        let worker = filmcraft_engine::session_worker::SessionWorker::new().unwrap();
        worker.install(&mut source).unwrap();
        source.agent_operation_control().enable_transport();
        source.execute("agent.readHistoryCheckpointBasis", json!({"token":token(60),"scope":scope})).unwrap();
        let saved = source
            .execute("agent.saveHistoryCheckpoint", json!({"token":token(60),"scope":scope,"workspace":W,"checkpoint":checkpoint,"metadataBasis":true}))
            .unwrap();
        source.execute("agent.readHistoryCheckpointBasis", json!({"token":token(61),"scope":scope})).unwrap();
        source.execute("agent.beginWorkspaceUpdate", json!({"token":token(61),"scope":scope,"workspace":W,"update":M})).unwrap();
        source.execute("agent.readHistoryCheckpointBasis", json!({"token":token(62),"scope":scope})).unwrap();
        let staged=source.execute("agent.stageProjectRecovery",json!({"token":token(62),"scope":scope,"workspace":W,"update":M,"pauseToken":token(61),"checkpoint":checkpoint,"checkpointSha256":saved["checkpoint"]["sha256"]})).unwrap();
        source.execute("agent.readHistoryCheckpointBasis", json!({"token":token(63),"scope":scope})).unwrap();
        source
            .execute(
                "agent.retireWorkspaceUpdate",
                json!({"token":token(63),"scope":scope,"workspace":W,"update":M,"pauseToken":token(61),"stageSha256":staged["sha256"]}),
            )
            .unwrap();
        source.shutdown();
        let file = source.path.as_ref().unwrap();
        let original = fs::read(file).unwrap();
        let descriptor = json!({"projectPath":file,"scope":scope,"workspace":W,"update":M,"sha256":staged["sha256"],"activationToken":token(64)}).to_string();
        let data = dir.0.join("candidate-state").to_str().unwrap().to_owned();
        let binding = filmcraft_ui_egui::composer_panel::Binding {
            version: 2,
            http_port: 55001,
            workspace_id: W.into(),
            project_id: "p_root_startup".into(),
            project_path: file.clone(),
            application_release: "a".repeat(64),
        };
        let binding = serde_json::to_string(&binding).unwrap();
        let crate::args::Cli::Run(launch) = crate::args::parse(
            None,
            ["--control", "55002", "--data-dir", &data, "--project-recovery", &descriptor, "--poema-composer", &binding].into_iter().map(Into::into),
        )
        .unwrap() else {
            panic!("run required")
        };
        (dir, *launch, original)
    }
    #[test]
    fn native_complete_root_app_startup_uses_actual_cli_and_shared_preparation_before_a_first_sequence_or_any_automatic_activation() {
        let (dir, launch, original) = root_fixture();
        let mut session = prepare(&launch, |_| 0).unwrap().unwrap();
        assert!(session.is_display_only());
        assert!(session.state.active_sequence.is_none());
        assert!(session.project.sequences().next().is_none());
        assert_eq!(session.project.root.children.len(), 1);
        assert!(launch.poema_composer.as_ref().unwrap().matches_session(&session).unwrap());
        assert!(!dir.0.join("candidate-state/preferences.json").exists());
        let worker = filmcraft_engine::session_worker::SessionWorker::new().unwrap();
        worker.install(&mut session).unwrap();
        session.agent_operation_control().enable_transport();
        let h = session.workspace_update_status().unwrap();
        session
            .execute(
                "agent.activateWorkspaceUpdate",
                json!({"token":h["token"],"scope":h["scope"],"workspace":h["workspace"],"update":h["update"],"nativeSession":h["nativeSession"]}),
            )
            .unwrap();
        assert!(!session.is_display_only());
        assert!(dir.0.join("candidate-state/preferences.json").exists());
        assert_eq!(fs::read(launch.poema_composer.as_ref().unwrap().project_path.clone()).unwrap(), original);
        session.undo().unwrap();
        assert!(session.project.root.children.is_empty());
        session.shutdown();
    }
    #[test]
    fn native_complete_root_app_startup_refuses_later_work_foreign_binding_and_existing_target_data_before_opening_a_fallback() {
        for mode in ["saved", "preferences", "binding", "existing_data"] {
            let (dir, mut launch, original) = root_fixture();
            let file = launch.poema_composer.as_ref().unwrap().project_path.clone();
            match mode {
                "saved" => fs::write(&file, b"later human saved state").unwrap(),
                "preferences" => fs::write(dir.0.join("source-state/preferences.json"), b"later human preferences").unwrap(),
                "binding" => launch.poema_composer.as_mut().unwrap().project_id = "p_foreign".into(),
                "existing_data" => {
                    fs::create_dir(launch.data_dir.as_ref().unwrap()).unwrap();
                    fs::write(launch.data_dir.as_ref().unwrap().join("keep.txt"), b"existing target data").unwrap();
                }
                _ => unreachable!(),
            }
            assert!(prepare(&launch, |_| 0).is_err(), "{mode}");
            assert!(!dir.0.join("candidate-state/preferences.json").exists());
            if mode != "saved" {
                assert_eq!(fs::read(&file).unwrap(), original);
            }
            if mode == "existing_data" {
                assert_eq!(fs::read(dir.0.join("candidate-state/keep.txt")).unwrap(), b"existing target data");
            }
        }
    }
}

#[cfg(test)]
mod project_creation_tests {
    use super::*;
    use serde_json::json;
    use std::{
        fs,
        path::PathBuf,
        sync::atomic::{AtomicUsize, Ordering},
    };
    struct Scratch(PathBuf);
    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    fn fixture() -> (Scratch, Launch) {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let root = std::env::temp_dir().join(format!("poema-app-creation-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
        fs::create_dir(&root).unwrap();
        let root = fs::canonicalize(root).unwrap();
        let scope = json!({"projectId":"p_app_creation","workspaceId":"11111111-1111-4111-8111-111111111111"});
        let project = root.join("New.fcproj");
        let raw=json!({"kind":"poema-native-project-creation","version":1,"scope":scope,"workspace":scope["workspaceId"],"update":"22222222-2222-4222-8222-222222222222","name":"New","projectPath":project,"activationToken":format!("{:064x}",1)}).to_string();
        let binding=json!({"version":2,"httpPort":55001,"workspaceId":scope["workspaceId"],"projectId":scope["projectId"],"projectPath":project,"applicationRelease":"a".repeat(64)}).to_string();
        let data = root.join("native-state");
        let crate::args::Cli::Run(launch) = crate::args::parse(
            None,
            ["--control", "55002", "--data-dir", data.to_str().unwrap(), "--project-create", &raw, "--poema-composer", &binding].into_iter().map(Into::into),
        )
        .unwrap() else {
            panic!("run expected")
        };
        (Scratch(root), *launch)
    }
    #[test]
    fn actual_managed_prepare_creates_one_empty_identity_bound_paused_session_before_logs_or_workers() {
        let (dir, launch) = fixture();
        let mut session = prepare(&launch, |_| 0).unwrap().unwrap();
        assert!(session.is_display_only());
        assert!(session.state.active_sequence.is_none());
        assert_eq!(session.matches_project_workspace("p_app_creation", "11111111-1111-4111-8111-111111111111").unwrap(), Some(true));
        let status = session.workspace_update_status().unwrap();
        assert_eq!(status["projectCreation"]["history"], "fresh");
        assert!(!launch.data_dir.as_ref().unwrap().join("preferences.json").exists());
        assert!(dir.0.join("New.fcproj").is_file());
        let worker = filmcraft_engine::session_worker::SessionWorker::new().unwrap();
        worker.install(&mut session).unwrap();
        session.agent_operation_control().enable_transport();
        session.execute("agent.activateWorkspaceUpdate",json!({"token":status["token"],"scope":status["scope"],"workspace":status["workspace"],"update":status["update"],"nativeSession":status["nativeSession"]})).unwrap();
        assert!(!session.is_display_only());
        session.execute("file.newBin", json!({"name":"New human work"})).unwrap();
    }
    #[test]
    fn a_crossed_composer_or_existing_data_root_is_refused_before_creating_any_project() {
        for case in 0..3 {
            let (dir, mut launch) = fixture();
            if case == 0 {
                launch.poema_composer.as_mut().unwrap().project_id = "p_foreign".into();
            } else if case == 1 {
                launch.poema_composer.as_mut().unwrap().version = 1;
            } else {
                fs::create_dir(launch.data_dir.as_ref().unwrap()).unwrap();
            }
            assert!(prepare(&launch, |_| 0).is_err());
            assert!(!dir.0.join("New.fcproj").exists());
        }
    }
    #[test]
    fn existing_project_bytes_and_creation_attempt_are_not_replaced_on_repeated_prepare() {
        let (dir, launch) = fixture();
        let first = prepare(&launch, |_| 0).unwrap().unwrap();
        let before = fs::read(dir.0.join("New.fcproj")).unwrap();
        assert!(prepare(&launch, |_| 0).is_err());
        assert_eq!(fs::read(dir.0.join("New.fcproj")).unwrap(), before);
        assert!(first.is_display_only());
    }
}
