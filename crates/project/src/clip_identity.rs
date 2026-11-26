//! Poema clip provenance. ClipIdentity's instance fields are lifted from native v185;
//! timed handoff spans extend that carrier for the four-door slice.
use filmcraft_time::Tick;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ClipIdentity {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub annotations: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub protection: Option<String>,
    /// Stable handoffids.js tokens with their original media-time interval.
    /// Copy on split; intersect at readback, never mint a new editorial id.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub handoff_spans: Vec<HandoffSpan>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HandoffSpan {
    pub carrier: String,
    pub source_in: Tick,
    pub source_out: Tick,
}

impl ClipIdentity {
    pub fn validate(&self) -> Result<(), String> {
        if self.id.is_empty() || self.id.len() > 160 || self.handoff_spans.len() > 64 {
            return Err("invalid or excessive clip provenance".into());
        }
        for span in &self.handoff_spans {
            if span.source_in < Tick::ZERO || span.source_out <= span.source_in || span.carrier.len() > 4096 {
                return Err("invalid handoff source interval".into());
            }
            let tokens: Vec<_> = span.carrier.split_whitespace().collect();
            if tokens.is_empty()
                || tokens.len() > 2
                || tokens.iter().any(|s| !s.starts_with("poema:") || s.len() <= 6 || !s[6..].bytes().all(|b| b.is_ascii_alphanumeric() || b"_.:-".contains(&b)))
            {
                return Err("invalid Poema handoff token".into());
            }
        }
        Ok(())
    }
}
