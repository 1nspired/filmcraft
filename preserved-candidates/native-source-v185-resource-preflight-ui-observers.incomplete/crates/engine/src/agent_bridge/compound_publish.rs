//! Sole native compound publication fence for the finite original human services.
//! Requests, complete previews and immutable receipts are validated by their typed family.
use super::editor_setting::{self, Setting};
use crate::{
    EngineError, Result, Session, commands,
    trim::{clock::SequenceViewCommand, command_settlement},
};
use filmcraft_project::ItemId;
use serde_json::{Value, json};

pub(super) enum Effect<'a> {
    EditorSetting(&'a Setting),
    SequenceView { command: SequenceViewCommand, item: ItemId, index: Option<u64> },
}
impl Effect<'_> {
    fn human(&self) -> (&'static str, Value) {
        match self {
            Self::EditorSetting(setting) => editor_setting::human(setting),
            Self::SequenceView { command, item, index } => (
                command.id(),
                match index {
                    Some(index) => json!({"item":item.0,"index":index}),
                    None => json!({"item":item.0}),
                },
            ),
        }
    }
    fn settles(&self) -> bool {
        !matches!(self, Self::EditorSetting(Setting::ClipMixerMode { .. }))
    }
    fn error(&self, message: impl Into<String>) -> EngineError {
        commands::bad(
            match self {
                Self::EditorSetting(_) => "agent.configureEditorControl",
                Self::SequenceView { .. } => "agent.navigateSequenceView",
            },
            message,
        )
    }
}
pub(super) fn validate(s: &mut Session, effect: &Effect<'_>) -> Result<()> {
    let (command, params) = effect.human();
    s.require_command_owner(command)?;
    s.enabled_for(commands::find(command).ok_or_else(|| effect.error("original human editor command unavailable"))?, &params)
        .map_err(|e| effect.error(e.to_string()))
}
pub(super) fn publish(s: &mut Session, token: &str, prepared: command_settlement::Prepared, effect: &Effect<'_>) -> Result<Value> {
    prepared.validate(s)?;
    s.agent_bridge.operations.enter_publication(token)?;
    if effect.settles() {
        prepared.apply(s)?;
    }
    let (command, params) = effect.human();
    s.execute(command, params)
}
