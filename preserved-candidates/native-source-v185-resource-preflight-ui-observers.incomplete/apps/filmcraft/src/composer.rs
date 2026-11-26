//! The native host for Poema's existing, sealed web rail. No provider or edit loop.
use filmcraft_ui_egui::{FilmcraftApp, composer_panel::Binding};

pub fn wrap(
    app: FilmcraftApp,
    binding: Option<Binding>,
    focus: std::sync::Arc<std::sync::atomic::AtomicBool>,
    control_port: Option<u16>,
) -> Result<Box<dyn eframe::App>, String> {
    let Some(binding) = binding else { return Ok(Box::new(app)) };
    #[cfg(target_os = "macos")]
    {
        Ok(Box::new(desktop::ComposerApp::new(app, binding, focus, control_port.ok_or("embedded composer needs its native control port")?)?))
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (app, binding, focus, control_port);
        Err("embedded Poema composer is currently supported on macOS only".into())
    }
}

#[cfg(target_os = "macos")]
mod desktop {
    use super::*;
    use egui::{LayerId, Rect};
    use filmcraft_ui_egui::composer_panel::ClosePhase;
    use serde_json::Value;
    use std::{
        io::{Read, Write},
        net::{Ipv4Addr, SocketAddr, TcpStream},
        sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
            mpsc,
        },
        thread,
        time::{Duration, Instant},
    };
    use wry::{
        WebView, WebViewBuilder,
        dpi::{PhysicalPosition, PhysicalSize},
    };

    fn accepts_health(binding: &Binding, h: &Value, native_port: u16, native_pid: u32) -> bool {
        !(h["ok"] != true
            || h["kind"] != "native-copy-workspace"
            || h["workspaceId"] != binding.workspace_id
            || h["projectId"] != binding.project_id
            || h["application"]["mode"] != "frozen-release"
            || h["application"]["id"] != binding.application_release
            || h["nativePresentation"]["port"] != native_port
            || h["nativePresentation"]["pid"] != native_pid)
    }

    fn health(binding: &Binding, native_port: u16) -> Result<(), String> {
        let address = SocketAddr::from((Ipv4Addr::LOCALHOST, binding.http_port));
        let mut stream = TcpStream::connect_timeout(&address, Duration::from_millis(400)).map_err(|e| e.to_string())?;
        stream.set_read_timeout(Some(Duration::from_millis(400))).map_err(|e| e.to_string())?;
        stream.set_write_timeout(Some(Duration::from_millis(400))).map_err(|e| e.to_string())?;
        write!(stream, "GET /api/health HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nConnection: close\r\n\r\n", binding.http_port).map_err(|e| e.to_string())?;
        let deadline = Instant::now() + Duration::from_secs(2);
        let mut bytes = Vec::new();
        let mut chunk = [0_u8; 4096];
        loop {
            if Instant::now() >= deadline || bytes.len() >= 65_536 {
                return Err("Poema connection health exceeded its bound".into());
            }
            let n = stream.read(&mut chunk).map_err(|e| e.to_string())?;
            if n == 0 {
                break;
            }
            bytes.extend_from_slice(chunk.get(..n).ok_or("invalid health read length")?);
        }
        let response = std::str::from_utf8(&bytes).map_err(|e| e.to_string())?;
        let (headers, body) = response.split_once("\r\n\r\n").ok_or("invalid Poema health response")?;
        if !headers.starts_with("HTTP/1.1 200 ") || headers.to_ascii_lowercase().contains("transfer-encoding:") {
            return Err("Poema connection is not ready".into());
        }
        let h: Value = serde_json::from_str(body).map_err(|e| e.to_string())?;
        if !accepts_health(binding, &h, native_port, std::process::id()) {
            return Err("Poema workspace or sealed application differs; no page was opened".into());
        }
        Ok(())
    }

    #[derive(Debug, PartialEq)]
    enum PanelEvent {
        Ready,
        Focus(bool),
        Close { id: String, status: String },
        Update { id: String, status: String },
        NewProject { id: String, status: String },
    }
    fn event(binding: &Binding, text: &str) -> Option<PanelEvent> {
        if text.len() > 1024 {
            return None;
        }
        let value: Value = serde_json::from_str(text).ok()?;
        let o = value.as_object()?;
        let kind = value["kind"].as_str()?;
        let count = match kind {
            "ready" => 5,
            "focus" => 6,
            "close" | "update" | "new-project" => 7,
            _ => return None,
        };
        if o.len() != count
            || value["version"] != 1
            || value["workspaceId"] != binding.workspace_id
            || value["projectId"] != binding.project_id
            || value["applicationRelease"] != binding.application_release
        {
            return None;
        }
        match kind {
            "focus" => Some(PanelEvent::Focus(value["focused"].as_bool()?)),
            "ready" => Some(PanelEvent::Ready),
            "close" | "update" | "new-project" => {
                let id = value["id"].as_str()?;
                let status = value["status"].as_str()?;
                if id.len() != 32
                    || !id.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
                    || !["ready", "cancelled", "failed"].contains(&status)
                {
                    return None;
                }
                Some(if kind == "new-project" {
                    PanelEvent::NewProject { id: id.into(), status: status.into() }
                } else if kind == "update" {
                    PanelEvent::Update { id: id.into(), status: status.into() }
                } else {
                    PanelEvent::Close { id: id.into(), status: status.into() }
                })
            }
            _ => None,
        }
    }

    fn bounds(rect: Rect, ppp: f32) -> Option<wry::Rect> {
        let values = [rect.min.x, rect.min.y, rect.width(), rect.height()];
        if !ppp.is_finite()
            || !(0.1..=8.0).contains(&ppp)
            || !values.iter().all(|v| v.is_finite() && (0.0..=32_768.0).contains(v))
            || rect.width() < 120.0
            || rect.height() < 100.0
        {
            return None;
        }
        Some(wry::Rect {
            position: PhysicalPosition::new((rect.min.x * ppp).round() as i32, (rect.min.y * ppp).round() as i32).into(),
            size: PhysicalSize::new((rect.width() * ppp).round() as u32, (rect.height() * ppp).round() as u32).into(),
        })
    }

    fn covered(ctx: &egui::Context, rect: Rect, owner: LayerId) -> bool {
        if egui::Popup::is_any_open(ctx) || ctx.dragged_id().is_some() {
            return true;
        }
        ctx.memory(|m| {
            let order: Vec<_> = m.layer_ids().collect();
            let index = order.iter().position(|l| *l == owner);
            order
                .iter()
                .enumerate()
                .any(|(i, l)| *l != owner && index.is_none_or(|at| i > at) && m.areas().is_visible(l) && m.area_rect(l.id).is_some_and(|r| r.intersects(rect)))
        })
    }

    pub struct ComposerApp {
        editor: FilmcraftApp,
        binding: Binding,
        view: Option<WebView>,
        health: mpsc::Receiver<Result<(), String>>,
        stop: Arc<AtomicBool>,
        connect: Arc<AtomicBool>,
        worker: Option<thread::JoinHandle<()>>,
        focus: Arc<AtomicBool>,
        ready: Arc<AtomicBool>,
        accepted: bool,
        failed: bool,
        visible: bool,
        last_bounds: Option<wry::Rect>,
        created: Option<Instant>,
        installed_layout: bool,
        events: mpsc::Receiver<PanelEvent>,
        event_tx: mpsc::SyncSender<PanelEvent>,
    }
    impl ComposerApp {
        pub fn new(mut editor: FilmcraftApp, binding: Binding, focus: Arc<AtomicBool>, native_port: u16) -> Result<Self, String> {
            binding.validate()?;
            if binding.version == 2 && !binding.matches_session(&editor.session)? {
                return Err("independent composer differs from the native project identity".into());
            }
            editor.composer_panel = Some(filmcraft_ui_egui::composer_panel::Panel::new(binding.clone()));
            let (tx, rx) = mpsc::sync_channel(1);
            let (event_tx, events) = mpsc::sync_channel(8);
            let stop = Arc::new(AtomicBool::new(false));
            let stopped = stop.clone();
            let target = binding.clone();
            let connect = Arc::new(AtomicBool::new(editor.session.workspace_update_status().is_none()));
            let connecting = connect.clone();
            let worker = thread::Builder::new()
                .name("poema-composer-connect".into())
                .spawn(move || {
                    // A missing manager may return much later, including after
                    // a crash during activation. Keep this one scoped connector
                    // alive; bounded notifications cannot accumulate while hidden.
                    if std::panic::catch_unwind(|| {
                        let mut last_notice: Option<Instant> = None;
                        while !stopped.load(Ordering::Relaxed) {
                            if !connecting.load(Ordering::Relaxed) {
                                thread::sleep(Duration::from_millis(100));
                                continue;
                            }
                            match health(&target, native_port) {
                                Ok(()) => {
                                    if tx.try_send(Ok(())).is_ok() {
                                        return;
                                    }
                                }
                                Err(e) => {
                                    if last_notice.is_none_or(|t| t.elapsed() >= Duration::from_secs(5)) {
                                        let _ = tx.try_send(Err(format!("Waiting for Poema to reconnect: {e}. The native edit remains open.")));
                                        last_notice = Some(Instant::now());
                                    }
                                }
                            }
                            thread::sleep(Duration::from_millis(200));
                        }
                    })
                    .is_err()
                    {
                        let _ = tx.try_send(Err("Poema connection worker failed; the native edit is unchanged".into()));
                    }
                })
                .map_err(|e| e.to_string())?;
            Ok(Self {
                editor,
                binding,
                view: None,
                health: rx,
                stop,
                connect,
                worker: Some(worker),
                focus,
                ready: Arc::new(AtomicBool::new(false)),
                accepted: false,
                failed: false,
                visible: false,
                last_bounds: None,
                created: None,
                installed_layout: false,
                events,
                event_tx,
            })
        }
        fn create_view(&mut self, frame: &eframe::Frame, rect: wry::Rect, ctx: &egui::Context) -> Result<(), String> {
            let binding = self.binding.clone();
            let url = binding.url();
            let allowed = url.clone();
            let ipc_url = url.clone();
            let focus = self.focus.clone();
            let ready = self.ready.clone();
            let repaint = ctx.clone();
            let event_tx = self.event_tx.clone();
            let view = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                WebViewBuilder::new()
                    .with_url(url)
                    .with_bounds(rect)
                    .with_visible(false)
                    .with_focused(false)
                    .with_devtools(false)
                    .with_background_color((12, 12, 14, 255))
                    .with_navigation_handler(move |url| url == allowed)
                    .with_new_window_req_handler(|_, _| wry::NewWindowResponse::Deny)
                    .with_ipc_handler(move |request| {
                        if request.uri().to_string() != ipc_url {
                            return;
                        }
                        if let Some(value) = event(&binding, request.body()) {
                            match value {
                                PanelEvent::Ready => {
                                    ready.store(true, Ordering::Relaxed);
                                    let _ = event_tx.try_send(PanelEvent::Ready);
                                }
                                PanelEvent::Focus(value) => focus.store(value, Ordering::Relaxed),
                                close => {
                                    let _ = event_tx.try_send(close);
                                }
                            }
                            repaint.request_repaint();
                        }
                    })
                    .build_as_child(frame)
            }))
            .map_err(|_| "Poema webview initialization failed; native editing remains available".to_string())?
            .map_err(|e| format!("Poema webview could not open: {e}"))?;
            self.view = Some(view);
            self.last_bounds = Some(rect);
            self.created = Some(Instant::now());
            Ok(())
        }
        fn paint_view(&mut self, frame: &eframe::Frame, ctx: &egui::Context) -> Result<(), String> {
            let session = &self.editor.session;
            let target = self
                .editor
                .composer_panel
                .as_mut()
                .and_then(|p| if p.document_matches(session).is_ok_and(|matches| matches) { p.rect.zip(p.layer) } else { None })
                .filter(|_| session.workspace_update_status().is_none());
            if let Some(p) = self.editor.composer_panel.as_mut() {
                p.host_state = if target.is_none() {
                    "panel_not_drawn"
                } else if !self.accepted {
                    "waiting_for_manager"
                } else if target.is_some_and(|(r, l)| covered(ctx, r, l)) {
                    "native_overlay"
                } else if self.view.is_none() {
                    "creating_view"
                } else if self.ready.load(Ordering::Relaxed) {
                    "ready"
                } else {
                    "loading_shared_rail"
                };
            }
            let wanted = target.filter(|(r, layer)| !covered(ctx, *r, *layer)).and_then(|(r, _)| bounds(r, ctx.pixels_per_point()));
            if let Some(rect) = wanted {
                if self.accepted && !self.failed && self.view.is_none() {
                    self.create_view(frame, rect, ctx)?;
                }
                if self.last_bounds != Some(rect)
                    && let Some(view) = self.view.as_ref()
                {
                    view.set_bounds(rect).map_err(|e| e.to_string())?;
                    self.last_bounds = Some(rect);
                }
            }
            if self.created.is_some_and(|t| t.elapsed() > Duration::from_secs(30)) && !self.ready.load(Ordering::Relaxed) {
                return Err("Poema did not finish opening. The native edit is still available; inspect the workspace service before reopening.".into());
            }
            let visible = wanted.is_some() && self.ready.load(Ordering::Relaxed) && !self.failed;
            if visible != self.visible {
                if let Some(view) = self.view.as_ref() {
                    view.set_visible(visible).map_err(|e| e.to_string())?;
                }
                self.visible = visible;
                ctx.request_repaint();
                if !visible
                    && self.focus.swap(false, Ordering::Relaxed)
                    && let Some(view) = self.view.as_ref()
                {
                    view.focus_parent().map_err(|e| e.to_string())?;
                }
            }
            if let Some(p) = self.editor.composer_panel.as_mut() {
                p.visible = visible;
            }
            Ok(())
        }
    }
    impl eframe::App for ComposerApp {
        fn raw_input_hook(&mut self, ctx: &egui::Context, input: &mut egui::RawInput) {
            self.editor.raw_input_hook(ctx, input);
            if self.focus.load(Ordering::Relaxed) {
                input
                    .events
                    .retain(|e| !matches!(e, egui::Event::Key { .. } | egui::Event::Text(_) | egui::Event::Paste(_) | egui::Event::Copy | egui::Event::Cut));
            }
        }
        fn logic(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
            for event in self.events.try_iter().take(8) {
                if let Some(panel) = self.editor.composer_panel.as_mut() {
                    match event {
                        PanelEvent::Ready => {
                            panel.close.resend();
                            panel.update.resend();
                            if panel.new_project.phase == ClosePhase::Joining {
                                panel.new_project.resend();
                            }
                        }
                        PanelEvent::NewProject { id, status } => {
                            panel.new_project_reply(&id, &status);
                        }
                        PanelEvent::Update { id, status } => {
                            panel.update.reply(&id, &status);
                        }
                        PanelEvent::Close { id, status } => {
                            if panel.close.reply(&id, &status) {
                                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                            }
                        }
                        PanelEvent::Focus(_) => {}
                    }
                }
            }
            if let Ok(result) = self.health.try_recv() {
                match result {
                    Ok(()) => self.accepted = true,
                    Err(e) => {
                        if let Some(p) = self.editor.composer_panel.as_mut() {
                            p.status = e;
                        }
                    }
                }
            }
            if let Some(p) = self.editor.composer_panel.as_mut() {
                p.connected = self.ready.load(Ordering::Relaxed) && !self.failed;
                p.focused = self.focus.load(Ordering::Relaxed);
                if p.connected {
                    p.status = "Poema is connected to this edit".into();
                } else if self.accepted && !self.failed {
                    p.status = "Opening Poema…".into();
                }
            }
            self.editor.logic(ctx, frame);
            let session = &self.editor.session;
            let matches = self.editor.composer_panel.as_mut().is_some_and(|p| p.document_matches(session).is_ok_and(|v| v));
            self.connect.store(matches && session.workspace_update_status().is_none(), Ordering::Relaxed);
            if let Some(panel) = self.editor.composer_panel.as_mut() {
                for purpose in ["close", "new-project"] {
                    let gate = if purpose == "close" { &mut panel.close } else { &mut panel.new_project };
                    if gate.phase == ClosePhase::Requested {
                        let mut bytes = [0_u8; 16];
                        match getrandom::fill(&mut bytes)
                            .map_err(|e| e.to_string())
                            .and_then(|()| gate.begin(bytes.iter().map(|b| format!("{b:02x}")).collect()))
                        {
                            Ok(()) => {}
                            Err(error) => {
                                gate.fail();
                                panel.status = format!("Could not prepare native {purpose}: {error}");
                            }
                        }
                    }
                }
                for purpose in ["close", "update", "new-project"] {
                    let gate = match purpose {
                        "close" => &mut panel.close,
                        "update" => &mut panel.update,
                        _ => &mut panel.new_project,
                    };
                    if self.ready.load(Ordering::Relaxed)
                        && let Some(view) = self.view.as_ref()
                        && let Some((id, action)) = gate.take_action()
                    {
                        let request = serde_json::json!({"version":1,"action":action,"id":id,"workspaceId":self.binding.workspace_id,"projectId":self.binding.project_id,"applicationRelease":self.binding.application_release});
                        let script = format!("window.dispatchEvent(new CustomEvent('poema-native-{purpose}',{{detail:{request}}}));");
                        if let Err(error) = view.evaluate_script(&script) {
                            gate.fail();
                            self.failed = true;
                            panel.status = format!("Could not contact the composer before {purpose}: {error}");
                        }
                    }
                }
            }
            if !self.installed_layout {
                // The editor may restore its saved workspace during initial
                // logic. Install the rail after that restoration, exactly once.
                if let Err(e) = filmcraft_ui_egui::composer_panel::dock(&mut self.editor) {
                    self.failed = true;
                    if let Some(p) = self.editor.composer_panel.as_mut() {
                        p.status = e;
                    }
                }
                self.installed_layout = true;
                ctx.request_repaint();
            }
            if !self.ready.load(Ordering::Relaxed) && !self.failed {
                ctx.request_repaint_after(Duration::from_millis(100));
            }
            if self
                .editor
                .composer_panel
                .as_ref()
                .is_some_and(|p| p.close.phase != ClosePhase::Open || p.update.phase != ClosePhase::Open || p.new_project.phase != ClosePhase::Open)
            {
                ctx.request_repaint_after(Duration::from_millis(100));
            }
        }
        fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
            if let Some(p) = self.editor.composer_panel.as_mut() {
                p.rect = None;
                p.layer = None;
            }
            self.editor.ui(ui, frame);
            let painted = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| self.paint_view(frame, ui.ctx())))
                .unwrap_or_else(|_| Err("Poema view failed; the native edit remains open".into()));
            if let Err(e) = painted {
                self.failed = true;
                self.focus.store(false, Ordering::Relaxed);
                if let Some(p) = self.editor.composer_panel.as_mut() {
                    p.status = e;
                    p.connected = false;
                    p.visible = false;
                }
                if let Some(view) = self.view.as_ref() {
                    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| view.set_visible(false)));
                }
            }
        }
        fn on_exit(&mut self) {
            self.editor.on_exit();
            self.stop.store(true, Ordering::Relaxed);
        }
    }
    impl Drop for ComposerApp {
        fn drop(&mut self) {
            self.stop.store(true, Ordering::Relaxed);
            if let Some(worker) = self.worker.take() {
                let _ = worker.join();
            }
        }
    }
    #[cfg(test)]
    mod tests {
        use super::*;
        #[test]
        fn replacement_rail_waits_for_its_own_native_process_and_port_even_on_the_same_release() {
            let binding = Binding {
                version: 1,
                http_port: 55117,
                workspace_id: "b2113e8d-8fe4-42e2-9561-a8f39d427d61".into(),
                project_id: "p_test".into(),
                project_path: "/generated/a.fcproj".into(),
                application_release: "a".repeat(64),
            };
            let mut h = serde_json::json!({"ok":true,"kind":"native-copy-workspace","workspaceId":binding.workspace_id,"projectId":binding.project_id,"application":{"mode":"frozen-release","id":binding.application_release},"nativePresentation":{"port":55116,"pid":12345}});
            assert!(accepts_health(&binding, &h, 55116, 12345));
            assert!(!accepts_health(&binding, &h, 55118, 12345));
            assert!(!accepts_health(&binding, &h, 55116, 12346));
            h["nativePresentation"] = Value::Null;
            assert!(!accepts_health(&binding, &h, 55116, 12345));
        }
        #[test]
        fn hostile_geometry_never_reaches_native_view() {
            assert!(bounds(Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(420.0, 600.0)), 2.0).is_some());
            for ppp in [0.0, f32::NAN, f32::INFINITY, 9.0] {
                assert!(bounds(Rect::EVERYTHING, ppp).is_none());
            }
        }
        #[test]
        fn ipc_never_accepts_a_foreign_identity_or_edit_request() {
            let b = Binding {
                version: 1,
                http_port: 55117,
                workspace_id: "b2113e8d-8fe4-42e2-9561-a8f39d427d61".into(),
                project_id: "p_test".into(),
                project_path: "/generated/a.fcproj".into(),
                application_release: "a".repeat(64),
            };
            let mut v = serde_json::json!({"version":1,"kind":"ready","workspaceId":b.workspace_id,"projectId":b.project_id,"applicationRelease":b.application_release});
            assert_eq!(event(&b, &v.to_string()), Some(PanelEvent::Ready));
            v["projectId"] = "p_other".into();
            assert!(event(&b, &v.to_string()).is_none());
            v["projectId"] = "p_test".into();
            v["kind"] = "new-project".into();
            v["id"] = "a".repeat(32).into();
            v["status"] = "ready".into();
            assert_eq!(event(&b, &v.to_string()), Some(PanelEvent::NewProject { id: "a".repeat(32), status: "ready".into() }));
            v["command"] = "file.newProject".into();
            assert!(event(&b, &v.to_string()).is_none());
            v.as_object_mut().unwrap().remove("command");
            v["id"] = "bad".into();
            assert!(event(&b, &v.to_string()).is_none());
            v["kind"] = "execute".into();
            assert!(event(&b, &v.to_string()).is_none());
        }
    }
}
