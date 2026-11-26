//! Explicit authored-project publication. No file evidence or source admission.
use super::{Batch, Ticket, bins, captions, effect_stack, finish, graphics, graphics_layout, markers, parameter, parse, project_audit, searches, tracks};
use crate::{Result, commands::bad};
use filmcraft_project::Project;
use serde::Deserialize;
use serde_json::{Value, json};

#[derive(Clone, Copy, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(super) enum Authority {
    #[default]
    MediaInputs,
    DocumentState,
}
fn error(message: impl Into<String>) -> crate::EngineError {
    bad("agent.apply", message)
}
pub(super) fn validate(batch: &Batch) -> Result<()> {
    if batch.authority == Authority::DocumentState
        && (!batch.exact
            || batch.operations.len() != 1
            || batch.operations.iter().any(|op| {
                ![
                    tracks::COMMAND,
                    bins::COMMAND,
                    super::project_clear::COMMAND,
                    super::project_plane::CREATE,
                    searches::COMMAND,
                    markers::COMMAND,
                    captions::COMMAND,
                    parameter::COMMAND,
                    finish::COMMAND,
                    effect_stack::COMMAND,
                    graphics::COMMAND,
                    graphics_layout::COMMAND,
                ]
                .contains(&op.command.as_str())
            }))
    {
        return Err(error(
            "document authority requires one exact authored bin, track, marker, caption, parameter, finishing, effect-stack or graphics/layout gesture; no source or mixed operation",
        ));
    }
    Ok(())
}
// Media declarations remain identical even when source marker notes change.
// This is a project-only invariant; it does not stat, resolve or open a file.
pub(super) fn transition(before: &Project, after: &Project) -> Result<()> {
    if before.items.len() > 100_000 || after.items.len() > 100_000 {
        return Err(error("document source-declaration inventory exceeds its bound"));
    }
    let ids = before.items.keys().chain(after.items.keys()).collect::<std::collections::BTreeSet<_>>();
    for id in ids {
        let a = before.item(*id);
        let b = after.item(*id);
        let binding = |item: Option<&filmcraft_project::ProjectItem>| item.and_then(|i| i.metadata.get(super::BINDING_KEY)).cloned();
        let admission = |item: Option<&filmcraft_project::ProjectItem>| item.and_then(|i| i.metadata.get("poema.admission.v1")).cloned();
        let same_media = match (a.and_then(|i| i.as_media()), b.and_then(|i| i.as_media())) {
            (Some(a), Some(b)) => std::ptr::eq(a, b),
            (None, None) => true,
            _ => false,
        };
        if same_media && binding(a) == binding(b) && admission(a) == admission(b) {
            continue;
        }
        let declarations = |item: Option<&filmcraft_project::ProjectItem>| -> Result<Vec<u8>> {
            project_audit::value_bytes(&(
                item.and_then(|i| i.as_media()).map(|media| {
                    // New source/decode fields must be accounted for explicitly.
                    let filmcraft_project::MediaClip { media, info, interpret, mark_in, mark_out, markers: _, offline, proxy, identity } = media;
                    (media, info, interpret, mark_in, mark_out, offline, proxy, identity)
                }),
                item.and_then(|i| i.metadata.get(super::BINDING_KEY)),
                item.and_then(|i| i.metadata.get("poema.admission.v1")),
            ))
        };
        if declarations(a)? != declarations(b)? {
            return Err(error("document publication cannot change source declarations, bindings or original admission proofs"));
        }
    }
    Ok(())
}
pub(super) fn committed(ticket: &Ticket) -> Result<Authority> {
    let (request, result) = ticket.committed.as_ref().ok_or_else(|| error("missing original owned publication"))?;
    let batch: Batch = parse("agent.apply", request)?;
    validate(&batch)?;
    receipt(&batch, result)?;
    Ok(batch.authority)
}
pub(super) fn receipt(batch: &Batch, result: &Value) -> Result<()> {
    let expected = if batch.authority == Authority::DocumentState { Some(json!("document_state")) } else { None };
    if result.get("authority") != expected.as_ref() {
        return Err(error("original publication authority differs from its exact native receipt"));
    }
    Ok(())
}
pub(super) fn mark(result: &mut Value, authority: Authority) -> Result<()> {
    if authority == Authority::DocumentState {
        result.as_object_mut().ok_or_else(|| error("document authority needs an object receipt"))?.insert("authority".into(), json!("document_state"));
    }
    Ok(())
}

#[cfg(test)]
#[path = "document_authority_tests.rs"]
mod tests;
