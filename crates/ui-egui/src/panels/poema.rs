//! Experimental editor-owned notes/protection. Drafts are ephemeral and pinned to one project
//! snapshot/sequence/instance; Cancel or selection changes never write a different clip.
use crate::FilmcraftApp;
use filmcraft_project::{ClipId, ItemId, Project};
use serde_json::{Value, json};
use std::sync::{Arc, Weak};

pub struct Draft {
    clip: ClipId,
    sequence: ItemId,
    project: Weak<Project>,
    revision: u64,
    identity: Option<String>,
    name: String,
    annotations: std::collections::BTreeMap<String, String>,
    protection: Option<String>,
    pub note: String,
    pub reason: String,
    message: String,
}

fn capture(app: &FilmcraftApp, clip: ClipId) -> Result<Draft, String> {
    let sequence = app.session.state.active_sequence.ok_or("no active sequence")?;
    let (_, c) = app.session.project.sequence(sequence).and_then(|q| q.find_item(clip)).ok_or("clip is not in the active sequence")?;
    let annotations = c.identity.as_ref().map(|i| i.annotations.clone()).unwrap_or_default();
    let protection = c.identity.as_ref().and_then(|i| i.protection.clone());
    Ok(Draft {
        clip,
        sequence,
        project: Arc::downgrade(&app.session.project),
        revision: app.session.revision,
        identity: c.identity.as_ref().map(|i| i.id.clone()),
        name: c.name.clone(),
        note: annotations.get("editor_note").cloned().unwrap_or_default(),
        reason: protection.clone().unwrap_or_default(),
        annotations,
        protection,
        message: String::new(),
    })
}

pub fn open(app: &mut FilmcraftApp, params: &Value) -> Result<Value, String> {
    let p = params.as_object().ok_or("parameters must be an object")?;
    if p.keys().any(|key| key != "clip") {
        return Err("only an optional explicit clip id is accepted".into());
    }
    let q = app.session.active_sequence().ok_or("open a timeline first")?;
    let selection = &app.session.state.selection;
    let clip = if let Some(id) = p.get("clip") {
        ClipId(id.as_u64().ok_or("invalid clip id")?)
    } else if let [id] = selection.as_slice() {
        *id
    } else {
        // Linked selection includes picture AND sound; treat that as one choice.
        let picture: Vec<_> = q.video_tracks.iter().flat_map(|t| &t.items).filter(|c| selection.contains(&c.id)).collect();
        let first = picture.first().filter(|_| picture.len() == 1).ok_or("select one picture clip (with its linked sound) or one audio clip")?;
        let link = first.link.ok_or("select one clip")?;
        if selection.iter().any(|id| q.find_item(*id).is_none_or(|(_, c)| c.link != Some(link))) {
            return Err("select one linked clip group".into());
        }
        first.id
    };
    app.poema = Some(capture(app, clip)?);
    Ok(json!({"clip": clip.0}))
}

fn valid(app: &FilmcraftApp, d: &Draft) -> bool {
    app.session.revision == d.revision
        && app.session.state.active_sequence == Some(d.sequence)
        && d.project.ptr_eq(&Arc::downgrade(&app.session.project))
        && app.session.active_sequence().and_then(|q| q.find_item(d.clip)).is_some_and(|(_, c)| c.identity.as_ref().map(|i| &i.id) == d.identity.as_ref())
}

pub fn inspect(app: &FilmcraftApp) -> Value {
    match &app.poema {
        Some(d) => json!({"clip":d.clip.0, "sequence":d.sequence.0, "identity":d.identity, "revision":d.revision,
            "note":d.note, "reason":d.reason, "protection":d.protection, "stale":!valid(app,d), "message":d.message}),
        None => Value::Null,
    }
}

/// Automation changes text only, never the pinned target, revision or protection authority.
pub fn patch(app: &mut FilmcraftApp, patch: &Value) -> Result<(), String> {
    let p = patch.as_object().ok_or("poema draft patch must be an object")?;
    for (key, v) in p {
        if !matches!(key.as_str(), "note" | "reason") || v.as_str().is_none_or(|v| v.len() > 1024) {
            return Err("poema accepts only note/reason strings up to 1024 UTF-8 bytes".into());
        }
    }
    let d = app.poema.as_mut().ok_or("no Poema clip dialog is open")?;
    if let Some(v) = p.get("note").and_then(Value::as_str) {
        d.note = v.into();
    }
    if let Some(v) = p.get("reason").and_then(Value::as_str) {
        d.reason = v.into();
    }
    Ok(())
}

fn action(app: &mut FilmcraftApp, d: &mut Draft, command: &str) -> Result<(), String> {
    if !valid(app, d) {
        return Err("The timeline changed. Reload the selected clip before saving; the draft was not applied.".into());
    }
    let params = match command {
        "poema.trackSequence" => json!({}),
        "poema.annotateClips" => {
            let mut notes = d.annotations.clone();
            if d.note.is_empty() {
                notes.remove("editor_note");
            } else {
                notes.insert("editor_note".into(), d.note.clone());
            }
            json!({"clips":[d.clip.0], "annotations":notes})
        }
        "poema.protectClips" => json!({"clips":[d.clip.0], "reason":d.reason}),
        "unlock" => json!({"clips":[d.clip.0], "reason":null}),
        _ => return Err("unknown Poema dialog action".into()),
    };
    app.session.execute(if command == "unlock" { "poema.protectClips" } else { command }, params).map_err(|e| e.to_string())?;
    // Keep unsaved text in the OTHER field; each button saves only its named action.
    let (note, reason) = (d.note.clone(), d.reason.clone());
    *d = capture(app, d.clip)?;
    (d.note, d.reason) = (note, reason);
    d.message = match command {
        "poema.trackSequence" => "Sequence tracking enabled. Notes and protection are ready.",
        "poema.annotateClips" => "Note saved. Existing annotation references were retained.",
        "unlock" => "Protection removed from this clip and its linked picture/sound.",
        _ => "Protection saved for this clip and its linked picture/sound. Notes save separately.",
    }
    .into();
    Ok(())
}

pub fn show(app: &mut FilmcraftApp, ctx: &egui::Context) {
    let Some(mut d) = app.poema.take() else { return };
    let mut open = true;
    let mut command = None;
    let mut reload = false;
    let mut close = false;
    let stale = !valid(app, &d);
    egui::Window::new("Poema notes & protection")
        .id(egui::Id::new("poema-clip-notes"))
        .open(&mut open)
        .collapsible(false)
        .default_width(460.0)
        .min_width(260.0)
        .default_pos(crate::widgets::dialog_center(ctx))
        .pivot(egui::Align2::CENTER_CENTER)
        .show(ctx, |ui| {
            egui::ScrollArea::vertical().max_height((ctx.content_rect().height() - 140.0).clamp(100.0, 600.0)).show(ui, |ui| {
                ui.label(&d.name);
                ui.label(format!("Clip {} · sequence {}", d.clip.0, d.sequence.0));
                if let Some(id) = &d.identity {
                    ui.monospace(id);
                }
                if stale {
                    ui.colored_label(app.tokens.accent, "The timeline changed. This draft cannot be applied.");
                    let r = ui.button("Reload selected clip (discard draft)");
                    app.auto.add_response("poema.reload", &r, "Reload selected clip (discard draft)");
                    reload = r.clicked();
                } else if d.identity.is_none() {
                    ui.label("Enable tracking for this sequence before adding notes or protection. This is undoable.");
                    let r = ui.button("Enable sequence tracking");
                    app.auto.add_response("poema.enable", &r, "Enable sequence tracking");
                    if r.clicked() {
                        command = Some("poema.trackSequence");
                    }
                }
                ui.separator();
                ui.label("Editor note — this clip only");
                let r = ui.add(egui::TextEdit::multiline(&mut d.note).id_salt("poema-note").desired_rows(3).desired_width(f32::INFINITY).char_limit(1024));
                app.auto.add_response("poema.note", &r, "Editor note");
                let ready = !stale && d.identity.is_some();
                let r = ui.add_enabled(ready && d.note.len() <= 1024, egui::Button::new("Apply note"));
                app.auto.add_response("poema.saveNote", &r, "Apply note");
                if r.clicked() {
                    command = Some("poema.annotateClips");
                }
                ui.separator();
                ui.label(if d.protection.is_some() { "Protected from agent edits" } else { "Not protected from agent edits" });
                if let Some(reason) = &d.protection {
                    ui.label(reason);
                }
                ui.label("Protection reason — also covers linked picture/sound");
                let r = ui.add(egui::TextEdit::multiline(&mut d.reason).id_salt("poema-reason").desired_rows(2).desired_width(f32::INFINITY).char_limit(1024));
                app.auto.add_response("poema.reason", &r, "Protection reason");
                ui.horizontal_wrapped(|ui| {
                    let r = ui.add_enabled(ready && !d.reason.trim().is_empty() && d.reason.len() <= 1024, egui::Button::new("Protect linked clips"));
                    app.auto.add_response("poema.protect", &r, "Protect linked clips");
                    if r.clicked() {
                        command = Some("poema.protectClips");
                    }
                    let r = ui.add_enabled(ready && d.protection.is_some(), egui::Button::new("Remove protection"));
                    app.auto.add_response("poema.unlock", &r, "Remove protection");
                    if r.clicked() {
                        command = Some("unlock");
                    }
                });
                ui.small("You can still edit protected clips manually. Each saved action is one Undo step. Text limit: 1024 UTF-8 bytes.");
                if !d.message.is_empty() {
                    ui.label(&d.message);
                }
            });
            ui.separator();
            let r = ui.button("Close");
            app.auto.add_response("poema.close", &r, "Close");
            close = r.clicked();
        });
    if let Some(command) = command
        && let Err(e) = action(app, &mut d, command)
    {
        d.message = e;
    }
    if reload {
        app.poema = None;
        if let Err(e) = self::open(app, &json!({})) {
            d.message = e;
            app.poema = Some(d);
        }
        return;
    }
    if open && !close && !crate::widgets::escape_closes(ctx) {
        app.poema = Some(d);
    }
}

#[cfg(test)]
mod tests;
