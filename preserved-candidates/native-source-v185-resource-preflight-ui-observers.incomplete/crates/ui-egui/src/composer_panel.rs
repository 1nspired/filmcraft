//! Layout and public identity of the shared Poema web composer. The desktop
//! host owns the webview; this crate never starts a server or an assistant.
use egui::Rect;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ClosePhase {
    #[default]
    Open,
    Requested,
    Joining,
    Ready,
    Cancelling,
    Failed,
}

#[derive(Debug, Default, Serialize)]
pub struct CloseGate {
    pub phase: ClosePhase,
    pub id: Option<String>,
    #[serde(skip)]
    dispatch: bool,
}
impl CloseGate {
    pub fn intercept(&mut self) -> bool {
        if self.phase == ClosePhase::Open || self.phase == ClosePhase::Failed && self.id.is_none() {
            self.phase = ClosePhase::Requested;
        }
        self.phase != ClosePhase::Ready
    }
    pub fn begin(&mut self, id: String) -> Result<(), String> {
        if self.phase != ClosePhase::Requested || id.len() != 32 || !id.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)) {
            return Err("invalid native close identity or phase".into());
        }
        self.id = Some(id);
        self.phase = ClosePhase::Joining;
        self.dispatch = true;
        Ok(())
    }
    pub fn cancel(&mut self) {
        if self.id.is_none() {
            self.phase = ClosePhase::Open;
            self.dispatch = false;
        } else {
            self.phase = ClosePhase::Cancelling;
            self.dispatch = true;
        }
    }
    pub fn fail(&mut self) {
        self.phase = ClosePhase::Failed;
        self.dispatch = false;
    }
    pub fn reply(&mut self, id: &str, status: &str) -> bool {
        if self.id.as_deref() != Some(id) {
            return false;
        }
        match status {
            "ready" if self.phase == ClosePhase::Joining => {
                self.phase = ClosePhase::Ready;
                self.dispatch = false;
                return true;
            }
            "cancelled" => {
                self.phase = ClosePhase::Open;
                self.id = None;
                self.dispatch = false;
            }
            "failed" if matches!(self.phase, ClosePhase::Joining | ClosePhase::Cancelling) => self.fail(),
            _ => {}
        }
        false
    }
    pub fn resend(&mut self) {
        if self.id.is_some() && matches!(self.phase, ClosePhase::Joining | ClosePhase::Cancelling | ClosePhase::Failed) {
            self.dispatch = true;
        }
    }
    pub fn take_action(&mut self) -> Option<(String, &'static str)> {
        if !std::mem::take(&mut self.dispatch) {
            return None;
        }
        Some((self.id.clone()?, if self.phase == ClosePhase::Cancelling { "cancel" } else { "prepare" }))
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Binding {
    pub version: u8,
    pub http_port: u16,
    pub workspace_id: String,
    pub project_id: String,
    pub project_path: String,
    pub application_release: String,
}

impl Binding {
    pub fn parse(text: &str) -> Result<Self, String> {
        if text.len() > 20_480 {
            return Err("Poema composer binding exceeds its limit".into());
        }
        let binding: Self = serde_json::from_str(text).map_err(|e| format!("invalid Poema composer binding: {e}"))?;
        binding.validate()?;
        Ok(binding)
    }

    pub fn validate(&self) -> Result<(), String> {
        let hex = |s: &str| s.bytes().all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c));
        let uuid = self.workspace_id.len() == 36
            && self
                .workspace_id
                .bytes()
                .enumerate()
                .all(|(i, c)| if [8, 13, 18, 23].contains(&i) { c == b'-' } else { c.is_ascii_digit() || (b'a'..=b'f').contains(&c) });
        if ![1, 2].contains(&self.version)
            || self.http_port == 0
            || self.http_port == 4519
            || !uuid
            || self.project_id.is_empty()
            || self.project_id.len() > 256
            || !self.project_id.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'-')
            || self.project_path.len() > 16_384
            || !std::path::Path::new(&self.project_path).is_absolute()
            || !self.project_path.ends_with(".fcproj")
            || self.project_path.chars().any(char::is_control)
            || self.application_release.len() != 64
            || !hex(&self.application_release)
        {
            return Err("Poema composer requires one explicit workspace, native project and sealed application release".into());
        }
        Ok(())
    }

    pub fn matches_session(&self, session: &filmcraft_engine::Session) -> Result<bool, String> {
        self.validate()?;
        if session.path.as_deref() != Some(self.project_path.as_str()) {
            return Ok(false);
        }
        let found = session.matches_project_workspace(&self.project_id, &self.workspace_id).map_err(|e| e.to_string())?;
        // Existing unassociated legacy projects keep their old path binding.
        // A present independent identity is always checked; version2requires it.
        Ok(found.unwrap_or(self.version == 1))
    }

    pub fn origin(&self) -> String {
        format!("http://127.0.0.1:{}", self.http_port)
    }

    pub fn url(&self) -> String {
        // All query values have bounded ASCII alphabets validated above.
        format!("{}/?film={}&poema_workspace={}&poema_release={}", self.origin(), self.project_id, self.workspace_id, self.application_release)
    }
}

struct DocumentCache {
    project: std::sync::Weak<filmcraft_project::Project>,
    path: Option<String>,
    binding: Binding,
    result: Result<bool, String>,
}

pub struct Panel {
    document: Option<DocumentCache>,
    pub close: CloseGate,
    pub update: CloseGate,
    pub new_project: CloseGate,
    pub binding: Binding,
    /// Set only when this panel's content was actually drawn in this UI pass.
    pub rect: Option<Rect>,
    pub layer: Option<egui::LayerId>,
    pub visible: bool,
    pub host_state: &'static str,
    pub status: String,
    pub connected: bool,
    pub focused: bool,
}

impl Panel {
    /// Weak COW identity avoids rescanning a large library every paint. The
    /// weak reference is invalidated by ordinary Arc::make_mut edits as well
    /// as document replacement, without retaining a second project owner.
    pub fn document_matches(&mut self, session: &filmcraft_engine::Session) -> Result<bool, String> {
        let project = std::sync::Arc::downgrade(&session.project);
        if let Some(old) = &self.document
            && old.project.ptr_eq(&project)
            && old.path == session.path
            && old.binding == self.binding
        {
            return old.result.clone();
        }
        let result = self.binding.matches_session(session);
        self.document = Some(DocumentCache { project, path: session.path.clone(), binding: self.binding.clone(), result: result.clone() });
        result
    }

    /// Reuse an uncertain launch identity only on an explicit menu retry.
    pub fn request_new_project(&mut self) -> Result<(), String> {
        if !self.connected || self.close.phase != ClosePhase::Open || self.update.phase != ClosePhase::Open {
            return Err("finish the current composer connection or window action before New Project".into());
        }
        match self.new_project.phase {
            ClosePhase::Open => {
                self.new_project.intercept();
            }
            ClosePhase::Failed if self.new_project.id.is_some() => {
                self.new_project.phase = ClosePhase::Joining;
                self.new_project.resend();
            }
            ClosePhase::Failed => {
                self.new_project.intercept();
            }
            _ => return Err("another New Project request is still opening".into()),
        }
        Ok(())
    }
    pub fn new_project_reply(&mut self, id: &str, status: &str) {
        if self.new_project.id.as_deref() != Some(id) || self.new_project.phase != ClosePhase::Joining {
            return;
        }
        match status {
            "ready" | "cancelled" => self.new_project = CloseGate::default(),
            "failed" => self.new_project.fail(),
            _ => {}
        }
    }
    pub fn request_update(&mut self, id: &str, action: &str) -> Result<(), String> {
        if id.len() != 32 || !id.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)) {
            return Err("invalid native update preparation identity".into());
        }
        match action {
            "prepare" => {
                if self.close.phase != ClosePhase::Open || matches!(self.new_project.phase, ClosePhase::Requested | ClosePhase::Joining) {
                    return Err("native close is already pending".into());
                }
                if self.update.id.as_deref() == Some(id) {
                    return Ok(());
                }
                if self.update.phase != ClosePhase::Open {
                    return Err("another native update preparation is pending".into());
                }
                self.update.intercept();
                self.update.begin(id.to_string())
            }
            "cancel" => {
                if self.update.id.is_some() && self.update.id.as_deref() != Some(id) {
                    return Err("another native update preparation is pending".into());
                }
                self.update.cancel();
                Ok(())
            }
            _ => Err("prepare or cancel one native update".into()),
        }
    }
    pub fn new(binding: Binding) -> Self {
        Self {
            document: None,
            close: CloseGate::default(),
            update: CloseGate::default(),
            new_project: CloseGate::default(),
            binding,
            rect: None,
            layer: None,
            visible: false,
            host_state: "waiting_for_layout",
            status: "Connecting Poema…".into(),
            connected: false,
            focused: false,
        }
    }
}

/// Install one rail after the initial saved workspace has been restored.
pub fn dock(app: &mut crate::FilmcraftApp) -> Result<(), String> {
    let panel = crate::dock::PanelKind::PoemaComposer;
    if !app.ui.dock.contains(panel) && !app.ui.floating.iter().any(|g| g.panels.contains(&panel)) {
        // The shared move operation requires an existing panel. Open it with
        // the ordinary human service before moving it to the rail edge.
        app.show_panel(panel);
        app.dock_to_edge(panel, craft_ui::docking::Zone::Left)?;
        if let crate::dock::DockNode::Split { size, .. } = &mut app.ui.dock {
            *size = crate::dock::SplitSize::FixedA(420.0);
        }
    }
    Ok(())
}

pub(crate) fn show(app: &mut crate::FilmcraftApp, ui: &mut egui::Ui, rect: Rect) {
    let Some(panel) = app.composer_panel.as_mut() else {
        ui.label("Open a managed Poema workspace to connect its assistant.");
        return;
    };
    match panel.document_matches(&app.session) {
        Ok(true) => {}
        Ok(false) => {
            ui.label("This native project belongs to another Poema workspace. Open the connected project to use its conversation.");
            return;
        }
        Err(error) => {
            ui.label(format!("Poema project identity could not be checked: {error}"));
            return;
        }
    }
    if !panel.connected {
        ui.label(&panel.status);
        if panel.close.phase != ClosePhase::Open {
            let r = ui.button("Cancel close");
            app.auto.add_response("poema.composer.cancelClose", &r, "Cancel close");
            if r.clicked() {
                panel.close.cancel();
                ui.ctx().request_repaint();
            }
        }
    }
    let content = rect.intersect(ui.clip_rect());
    if content.is_finite() && content.width() >= 120.0 && content.height() >= 100.0 {
        panel.rect = Some(content);
        panel.layer = Some(ui.layer_id());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn close_gate_requires_matching_readiness_and_cancellation_never_consumes_a_late_ready() {
        let mut gate = CloseGate::default();
        assert!(gate.intercept());
        assert_eq!(gate.phase, ClosePhase::Requested);
        assert!(gate.begin("bad".into()).is_err());
        gate.begin("a".repeat(32)).unwrap();
        assert_eq!(gate.take_action(), Some(("a".repeat(32), "prepare")));
        assert!(!gate.reply(&"b".repeat(32), "ready"));
        assert!(gate.intercept());
        gate.cancel();
        assert_eq!(gate.take_action(), Some(("a".repeat(32), "cancel")));
        assert!(!gate.reply(&"a".repeat(32), "ready"));
        assert!(gate.intercept());
        gate.reply(&"a".repeat(32), "cancelled");
        assert_eq!(gate.phase, ClosePhase::Open);
        assert!(gate.intercept());
        gate.begin("b".repeat(32)).unwrap();
        assert!(!gate.reply(&"a".repeat(32), "ready"));
        assert!(gate.reply(&"b".repeat(32), "ready"));
        assert!(!gate.intercept());
    }

    #[test]
    fn page_reload_resends_the_same_close_identity_without_reopening_a_failed_attempt() {
        let mut gate = CloseGate::default();
        gate.intercept();
        gate.begin("a".repeat(32)).unwrap();
        gate.take_action();
        gate.resend();
        assert_eq!(gate.take_action(), Some(("a".repeat(32), "prepare")));
        gate.reply(&"a".repeat(32), "failed");
        assert!(gate.intercept());
        gate.cancel();
        gate.resend();
        assert_eq!(gate.take_action(), Some(("a".repeat(32), "cancel")));
    }

    fn binding() -> Binding {
        Binding {
            version: 1,
            http_port: 55117,
            workspace_id: "b2113e8d-8fe4-42e2-9561-a8f39d427d61".into(),
            project_id: "p_test".into(),
            project_path: "/generated/Film.fcproj".into(),
            application_release: "a".repeat(64),
        }
    }

    #[test]
    fn managed_new_project_uses_one_scoped_launch_without_replacing_the_human_edit() {
        use crate::control::{ControlRequest, Outcome};
        let b = binding();
        let mut session = filmcraft_engine::Session::default();
        session.path = Some(b.project_path.clone());
        session.execute("file.newBin", serde_json::json!({"name":"Human bin kept"})).unwrap();
        let before = session.project.clone();
        let revision = session.revision;
        let mut app = crate::FilmcraftApp::new(session);
        let ctx = egui::Context::default();
        app.composer_panel = Some(Panel::new(b.clone()));
        assert!(crate::menus::invoke(&mut app, &ctx, "file.newProject", serde_json::json!({})).is_err());
        app.composer_panel.as_mut().unwrap().connected = true;
        assert!(crate::menus::invoke(&mut app, &ctx, "file.newProject", serde_json::json!({"name":"Cannot replace current edit"})).is_err());
        let (req, _) = ControlRequest::new("engine.execute", serde_json::json!({"command":"file.newProject","params":{}}));
        let Outcome::Done(reply) = crate::control::handle(&mut app, &ctx, &req) else { panic!("bounded menu reply") };
        assert_eq!(reply["ok"], true);
        assert!(std::sync::Arc::ptr_eq(&before, &app.session.project));
        assert_eq!(app.session.revision, revision);
        assert_eq!(app.session.path.as_deref(), Some(b.project_path.as_str()));
        let panel = app.composer_panel.as_mut().unwrap();
        assert_eq!(panel.new_project.phase, ClosePhase::Requested);
        let id = "a".repeat(32);
        panel.new_project.begin(id.clone()).unwrap();
        assert_eq!(panel.new_project.take_action(), Some((id.clone(), "prepare")));
        assert!(panel.request_new_project().is_err());
        panel.new_project_reply(&"b".repeat(32), "ready");
        assert_eq!(panel.new_project.phase, ClosePhase::Joining);
        panel.new_project_reply(&id, "failed");
        panel.new_project_reply(&id, "ready");
        assert_eq!(panel.new_project.phase, ClosePhase::Failed);
        panel.request_new_project().unwrap();
        assert_eq!(panel.new_project.take_action(), Some((id.clone(), "prepare")));
        panel.new_project_reply(&id, "ready");
        assert_eq!(panel.new_project.phase, ClosePhase::Open);
        assert_eq!(panel.close.phase, ClosePhase::Open);
        assert!(std::sync::Arc::ptr_eq(&before, &app.session.project));
        app.session.path = Some("/generated/Other.fcproj".into());
        assert!(crate::menus::invoke(&mut app, &ctx, "file.newProject", serde_json::json!({})).is_err());
        assert!(std::sync::Arc::ptr_eq(&before, &app.session.project));
    }

    #[test]
    fn update_preparation_has_independent_readiness_and_cancel_cannot_accept_late_ready() {
        let mut panel = Panel::new(binding());
        assert!(panel.request_update("bad", "prepare").is_err());
        assert_eq!(panel.update.phase, ClosePhase::Open);
        let id = "a".repeat(32);
        panel.request_update(&id, "prepare").unwrap();
        assert_eq!(panel.close.phase, ClosePhase::Open);
        assert_eq!(panel.update.take_action(), Some((id.clone(), "prepare")));
        panel.request_update(&id, "prepare").unwrap();
        assert!(panel.update.take_action().is_none());
        assert!(panel.request_update(&"b".repeat(32), "prepare").is_err());
        panel.request_update(&id, "cancel").unwrap();
        assert!(!panel.update.reply(&id, "ready"));
        panel.update.reply(&id, "cancelled");
        assert_eq!(panel.update.phase, ClosePhase::Open);
        assert_eq!(panel.close.phase, ClosePhase::Open);
    }

    #[test]
    fn update_control_refuses_foreign_binding_and_never_changes_the_native_edit() {
        use crate::control::{ControlRequest, Outcome};
        let b = binding();
        let mut session = filmcraft_engine::Session::default();
        session.path = Some(b.project_path.clone());
        let before = session.project.clone();
        let revision = session.revision;
        let mut app = crate::FilmcraftApp::new(session);
        app.composer_panel = Some(Panel::new(b.clone()));
        let ctx = egui::Context::default();
        let id = "a".repeat(32);
        for mut params in [
            serde_json::json!({"action":"send","id":id,"binding":b}),
            serde_json::json!({"action":"prepare","id":"bad","binding":b}),
            serde_json::json!({"action":"prepare","id":id,"binding":b,"command":"edit.undo"}),
        ] {
            let (req, _) = ControlRequest::new("ui.poemaComposer.update", params.take());
            let Outcome::Done(result) = crate::control::handle(&mut app, &ctx, &req) else { panic!("must return a bounded reply") };
            assert_eq!(result["ok"], false);
        }
        let mut foreign = serde_json::json!({"action":"prepare","id":id,"binding":b});
        foreign["binding"]["projectId"] = "p_other".into();
        let (req, _) = ControlRequest::new("ui.poemaComposer.update", foreign);
        let Outcome::Done(result) = crate::control::handle(&mut app, &ctx, &req) else { panic!("must reply") };
        assert_eq!(result["ok"], false);
        assert!(std::sync::Arc::ptr_eq(&before, &app.session.project));
        assert_eq!(revision, app.session.revision);
        assert_eq!(app.composer_panel.as_ref().unwrap().update.phase, ClosePhase::Open);
        let (req, _) = ControlRequest::new("ui.poemaComposer.update", serde_json::json!({"action":"prepare","id":id,"binding":b}));
        let Outcome::Done(result) = crate::control::handle(&mut app, &ctx, &req) else { panic!("must reply") };
        assert_eq!(result["ok"], true);
        assert!(std::sync::Arc::ptr_eq(&before, &app.session.project));
        assert_eq!(revision, app.session.revision);
    }

    #[test]
    fn binding_is_exact_loopback_identity_without_paths_in_navigation() {
        let b = binding();
        let text = serde_json::to_string(&b).unwrap();
        assert_eq!(Binding::parse(&text).unwrap(), b);
        assert!(b.url().starts_with("http://127.0.0.1:55117/?film=p_test&poema_workspace="));
        assert!(!b.url().contains("generated"));
        for key in ["version", "httpPort", "workspaceId", "projectId", "projectPath", "applicationRelease"] {
            let mut value = serde_json::to_value(&b).unwrap();
            value.as_object_mut().unwrap().remove(key);
            assert!(Binding::parse(&value.to_string()).is_err(), "{key}");
        }
        let mut value = serde_json::to_value(&b).unwrap();
        value["arbitraryUrl"] = "https://example.test".into();
        assert!(Binding::parse(&value.to_string()).is_err());
    }

    #[test]
    fn malformed_binding_never_opens_a_fallback_film() {
        for text in ["null", "[]", "{", &"x".repeat(20_481)] {
            assert!(Binding::parse(text).is_err());
        }
        for change in [0, 4519] {
            let mut b = binding();
            b.http_port = change;
            assert!(b.validate().is_err());
        }
        for id in ["other&film=p_other", "", "é"] {
            let mut b = binding();
            b.project_id = id.into();
            assert!(b.validate().is_err());
        }
        for path in ["relative.fcproj", "/wrong.txt", "/control\n.fcproj"] {
            let mut b = binding();
            b.project_path = path.into();
            assert!(b.validate().is_err());
        }
        let mut b = binding();
        b.application_release = "development-local".into();
        assert!(b.validate().is_err());
    }
    #[test]
    fn new_composer_panel_opens_before_it_moves_and_preserves_the_other_panels() {
        let mut app = crate::FilmcraftApp::new(filmcraft_engine::Session::default());
        let panel = crate::dock::PanelKind::PoemaComposer;
        let existing: Vec<_> = crate::dock::PanelKind::ALL.into_iter().filter(|p| app.ui.dock.contains(*p)).collect();
        assert!(!app.ui.dock.contains(panel));
        dock(&mut app).unwrap();
        assert!(app.ui.dock.contains(panel));
        for p in existing {
            assert!(app.ui.dock.contains(p), "{p:?} disappeared");
        }
        assert!(matches!(app.ui.dock, crate::dock::DockNode::Split { size: crate::dock::SplitSize::FixedA(size), .. } if size == 420.0));
        let once = serde_json::to_value(&app.ui.dock).unwrap();
        dock(&mut app).unwrap();
        assert_eq!(serde_json::to_value(&app.ui.dock).unwrap(), once);
    }
    #[test]
    fn independent_composer_binding_requires_actual_root_identity_and_keeps_real_absence_after_undo() {
        let mut b = binding();
        b.version = 2;
        let mut s = filmcraft_engine::Session::default();
        s.execute("file.newProject", serde_json::json!({"name":"Independent","workspace":{"projectId":b.project_id,"workspaceId":b.workspace_id}})).unwrap();
        s.path = Some(b.project_path.clone());
        let mut panel = Panel::new(b.clone());
        assert!(panel.document_matches(&s).unwrap());
        assert!(s.state.active_sequence.is_none());
        let anchor = s.execute("file.newSequence", serde_json::json!({"name":"First"})).unwrap()["sequence"].as_u64().unwrap();
        assert!(panel.document_matches(&s).unwrap());
        s.execute("sequence.close", serde_json::json!({"item":anchor})).unwrap();
        assert!(panel.document_matches(&s).unwrap());
        s.undo().unwrap();
        assert!(s.project.sequences().next().is_none());
        assert!(panel.document_matches(&s).unwrap());
        let mut legacy = filmcraft_engine::Session::default();
        legacy.path = Some(b.project_path.clone());
        assert!(!panel.document_matches(&legacy).unwrap());
        panel.binding.version = 1;
        assert!(panel.document_matches(&legacy).unwrap());
    }
    #[test]
    fn independent_composer_cache_invalidates_cow_path_binding_and_foreign_project_changes_without_borrowing_legacy_identity() {
        let mut b = binding();
        b.version = 2;
        let mut s = filmcraft_engine::Session::default();
        s.execute("file.newProject", serde_json::json!({"name":"Independent","workspace":{"projectId":b.project_id,"workspaceId":b.workspace_id}})).unwrap();
        s.path = Some(b.project_path.clone());
        let mut panel = Panel::new(b.clone());
        assert!(panel.document_matches(&s).unwrap());
        let old = std::sync::Arc::downgrade(&s.project);
        assert!(panel.document_matches(&s).unwrap());
        assert!(panel.document.as_ref().unwrap().project.ptr_eq(&old));
        s.execute("file.newProject", serde_json::json!({"name":"Foreign","workspace":{"projectId":"p_other","workspaceId":b.workspace_id}})).unwrap();
        s.path = Some(b.project_path.clone());
        assert!(!panel.document_matches(&s).unwrap());
        panel.binding.version = 1;
        assert!(!panel.document_matches(&s).unwrap());
        panel.binding.project_id = "p_other".into();
        assert!(panel.document_matches(&s).unwrap());
        s.path = Some("/other.fcproj".into());
        assert!(!panel.document_matches(&s).unwrap());
        s.path = Some(b.project_path);
        assert!(panel.document_matches(&s).unwrap());
        std::sync::Arc::make_mut(&mut s.project).metadata.insert("poema.projectWorkspace.v1".into(), "{".into());
        assert!(panel.document_matches(&s).is_err());
    }
    #[test]
    fn independent_composer_status_and_update_gate_require_the_actual_document_without_an_active_sequence() {
        use crate::control::{ControlRequest, Outcome};
        let mut b = binding();
        b.version = 2;
        let mut session = filmcraft_engine::Session::default();
        session
            .execute("file.newProject", serde_json::json!({"name":"Independent","workspace":{"projectId":b.project_id,"workspaceId":b.workspace_id}}))
            .unwrap();
        session.path = Some(b.project_path.clone());
        let mut app = crate::FilmcraftApp::new(session);
        app.composer_panel = Some(Panel::new(b.clone()));
        let ctx = egui::Context::default();
        let (request, _) = ControlRequest::new("ui.poemaComposer", serde_json::json!({}));
        let Outcome::Done(reply) = crate::control::handle(&mut app, &ctx, &request) else { panic!("bounded reply required") };
        assert_eq!(reply["result"]["documentMatched"], true);
        assert_eq!(reply["result"]["lifecycle"]["independent_project_binding"], true);
        assert!(app.session.state.active_sequence.is_none());
        app.session.execute("file.newProject", serde_json::json!({"name":"Foreign","workspace":{"projectId":"p_other","workspaceId":b.workspace_id}})).unwrap();
        app.session.path = Some(b.project_path.clone());
        let before = app.session.project.clone();
        let (request, _) = ControlRequest::new("ui.poemaComposer", serde_json::json!({}));
        let Outcome::Done(reply) = crate::control::handle(&mut app, &ctx, &request) else { panic!("bounded reply required") };
        assert_eq!(reply["result"]["documentMatched"], false);
        let (request, _) = ControlRequest::new("ui.poemaComposer.update", serde_json::json!({"binding":b,"action":"prepare","id":"a".repeat(32)}));
        let Outcome::Done(reply) = crate::control::handle(&mut app, &ctx, &request) else { panic!("bounded reply required") };
        assert_eq!(reply["ok"], false);
        assert!(std::sync::Arc::ptr_eq(&before, &app.session.project));
        assert_eq!(app.composer_panel.as_ref().unwrap().update.phase, ClosePhase::Open);
    }
}
