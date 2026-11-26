//! Disjoint closed project/sequence scopes for shared organization services.
use super::{project_plane, sequences};
use crate::{Result, Session, commands::bad, project_workspace};
use filmcraft_project::ItemId;
use serde::{Deserialize, Serialize};
#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
#[serde(untagged)]
pub(super) enum Scope {
    Sequence(sequences::Scope),
    Project(project_workspace::ProjectScope),
}
impl Scope {
    #[cfg(not(target_arch = "wasm32"))]
    pub(super) fn project_id(&self) -> &str {
        match self {
            Self::Sequence(scope) => &scope.project_id,
            Self::Project(scope) => &scope.project_id,
        }
    }
    pub(super) fn valid(&self) -> bool {
        match self {
            Self::Sequence(s) => crate::editorial_workspace::valid_scope(s),
            Self::Project(s) => project_workspace::valid_scope(s),
        }
    }
    pub(super) fn require(&self, s: &Session) -> Result<Option<ItemId>> {
        match self {
            Self::Sequence(scope) => {
                let id = s.state.active_sequence.ok_or(crate::EngineError::NoSequence)?;
                if !sequences::belongs(&s.project, id, scope)? {
                    return Err(bad("agent.scope", "current sequence is outside this workspace"));
                }
                Ok(Some(id))
            }
            Self::Project(scope) => {
                project_plane::require_scope(s, scope)?;
                Ok(s.state.active_sequence)
            }
        }
    }
    pub(super) fn read(&self, s: &mut Session, token: &str, projection: bool) -> Result<()> {
        match self {
            Self::Project(scope) => {
                if super::multicam_production::is_ticket(s, token) {
                    super::multicam_production::validate_read(s, token, scope, projection)
                } else if super::library_plane::is_ticket(s, token) {
                    super::library_plane::validate_ticket(s, token, scope, projection)
                } else {
                    project_plane::validate_ticket(s, token, scope)
                }
            }
            Self::Sequence(_) => {
                if projection {
                    super::validate_metadata_read(s, token, true, "agent.scope")?;
                } else {
                    super::validate_snapshot(s, &serde_json::json!({"token":token}))?;
                }
                Ok(())
            }
        }
    }
    pub(super) fn source_validation_owed(&self, projection: bool) -> bool {
        matches!(self, Self::Project(_)) || projection
    }
}
