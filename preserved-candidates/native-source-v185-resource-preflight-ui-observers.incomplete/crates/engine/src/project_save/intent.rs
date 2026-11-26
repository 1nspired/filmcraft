//! Finite Save intents. These data are not arbitrary engine commands, selected
//! file custody, overwrite permission, a live persistence lease or an Undo edge.
use crate::{Result, Session, commands::bad};
use serde::{Deserialize, Serialize};
use serde_json::Value;
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum Intent {
    Save {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        path: Option<String>,
    },
    SaveAs {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        path: Option<String>,
    },
    SaveCopy {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        path: Option<String>,
    },
    SaveAll,
}
pub(crate) enum Resolution {
    Destination { path: String, adopt: bool, human_command: &'static str, uses_current_path: bool },
    NeedsDestination { human_command: &'static str, native_dialog: bool },
}
fn error(message: impl Into<String>) -> crate::EngineError {
    bad("agent.planProjectSave", message)
}
fn valid_path(path: &str) -> bool {
    !path.is_empty() && path.len() <= 4096 && !path.chars().any(char::is_control)
}
impl Intent {
    // Serde internally tagged unit variants can ignore extra fields.
    // SaveAll therefore also requires its explicit one-field shape here.
    pub(crate) fn parse(value: &Value) -> Result<Self> {
        if !value.is_object()
            || value.as_object().is_none_or(|m| m.len() > 2)
            || (value.get("kind").and_then(Value::as_str) == Some("save_all") && value.as_object().is_none_or(|m| m.len() != 1))
            || value.get("path").is_some_and(|p| p.as_str().is_none_or(|s| !valid_path(s)))
        {
            return Err(error("Closed native Save intent and explicit nonempty path required; null cannot select a different destination."));
        }
        serde_json::from_value(value.clone()).map_err(|e| error(e.to_string()))
    }
    pub(crate) fn resolve(&self, s: &Session) -> Result<Resolution> {
        let (path, adopt, command, dialog, current) = match self {
            Self::Save { path } => (path.as_ref().or(s.path.as_ref()), true, "file.save", true, path.is_none()),
            Self::SaveAs { path } => (path.as_ref(), true, "file.saveAs", true, false),
            Self::SaveCopy { path } => (path.as_ref(), false, "file.saveCopy", true, false),
            Self::SaveAll => (s.path.as_ref(), true, "file.saveAll", false, true),
        };
        match path {
            Some(path) if valid_path(path) => Ok(Resolution::Destination { path: path.clone(), adopt, human_command: command, uses_current_path: current }),
            Some(_) => Err(error("Current or explicit native Save path is malformed.")),
            None => Ok(Resolution::NeedsDestination { human_command: command, native_dialog: dialog }),
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn closed_native_save_intents_preserve_all_four_human_variants_explicit_paths_and_native_destination_requests() {
        let mut s = Session::default();
        for kind in ["save", "save_as", "save_copy", "save_all"] {
            let i = Intent::parse(&json!({"kind":kind})).unwrap();
            match i.resolve(&s).unwrap() {
                Resolution::NeedsDestination { native_dialog, .. } => assert_eq!(native_dialog, kind != "save_all"),
                _ => panic!("unsaved requests cannot invent a destination"),
            }
        }
        s.path = Some("/generated/current.fcproj".into());
        for kind in ["save", "save_all"] {
            match Intent::parse(&json!({"kind":kind})).unwrap().resolve(&s).unwrap() {
                Resolution::Destination { path, adopt, uses_current_path, .. } => {
                    assert_eq!(path, s.path.clone().unwrap());
                    assert!(adopt && uses_current_path);
                }
                _ => panic!("saved current aliases resolve to current path"),
            }
        }
        for kind in ["save", "save_as", "save_copy"] {
            for path in ["/generated/chosen.fcproj", "relative chosen.fcproj"] {
                let i = Intent::parse(&json!({"kind":kind,"path":path})).unwrap();
                match i.resolve(&s).unwrap() {
                    Resolution::Destination { path: actual, adopt, uses_current_path, .. } => {
                        assert_eq!(actual, path);
                        assert_eq!(adopt, kind != "save_copy");
                        assert!(!uses_current_path);
                    }
                    _ => panic!("explicit path must remain explicit"),
                }
            }
        }
        for kind in ["save_as", "save_copy"] {
            assert!(
                matches!(Intent::parse(&json!({"kind":kind})).unwrap().resolve(&s).unwrap(), Resolution::NeedsDestination { .. }),
                "SaveAs/Copy cannot silently take current path"
            );
        }
    }
    #[test]
    fn closed_native_save_intents_refuse_null_wrong_types_unknown_raw_commands_hostile_paths_and_alias_widening() {
        for v in [
            Value::Null,
            json!([]),
            json!({}),
            json!({"kind":"execute","command":"file.save"}),
            json!({"kind":"save","path":null}),
            json!({"kind":"save","path":12}),
            json!({"kind":"save_as","path":""}),
            json!({"kind":"save_copy","path":"x\ny"}),
            json!({"kind":"save","path":"x\0y"}),
            json!({"kind":"save","path":"x".repeat(4097)}),
            json!({"kind":"save_all","path":"/generated/foreign.fcproj"}),
            json!({"kind":"save_copy","adopt":true}),
            json!({"kind":"save_as","path":"x","grant":true}),
        ] {
            assert!(Intent::parse(&v).is_err(), "{v}");
        }
    }
}
