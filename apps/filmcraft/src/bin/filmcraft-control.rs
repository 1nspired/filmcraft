//! Headless host for the same native Session and control transport used by the desktop app.
//! No mock project model, ffmpeg renderer or independent timeline writer.
#![cfg_attr(not(test), deny(clippy::unwrap_used, clippy::expect_used, clippy::panic))]
#[path = "../control_server.rs"]
mod control_server;
use filmcraft_engine::Session;
use serde_json::json;
use std::io::Write;
fn run() -> Result<(), String> {
    let args: Vec<String> = std::env::args().collect();
    let port = match args.as_slice() {
        [_, flag, port] if flag == "--port" => port.parse::<u16>().map_err(|e| e.to_string())?,
        _ => return Err("usage: filmcraft-control --port <port or 0>".into()),
    };
    let mut session = Session::default();
    let ctx = egui::Context::default();
    let (rx, port) = control_server::start_bound(port, ctx, session.command_journal.clone())?;
    writeln!(std::io::stdout(), "{}", json!({"ready":true,"port":port,"nativeRevision":env!("FILMCRAFT_SLICE_REV")})).map_err(|e| e.to_string())?;
    for req in rx {
        let reply = match req.method.as_str() {
            "engine.execute" => match req.params["command"].as_str() {
                Some(id) => match session.execute(id, req.params.get("params").cloned().unwrap_or(json!({}))) {
                    Ok(v) => json!({"ok":true,"result":v}),
                    Err(e) => json!({"ok":false,"error":e.to_string()}),
                },
                None => json!({"ok":false,"error":"missing command"}),
            },
            _ => json!({"ok":false,"error":"headless control supports engine commands and journal subscriptions"}),
        };
        let _ = req.reply.send(reply);
    }
    Ok(())
}
fn main() {
    if let Err(e) = run() {
        let _ = writeln!(std::io::stderr(), "{e}");
        std::process::exit(1);
    }
}
