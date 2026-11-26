//! Exact saved bytes observed by the native open/save owner, never media proof.
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Baseline {
    pub(crate) path: String,
    pub(crate) sha256: String,
}
pub(crate) fn sha256(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
impl Baseline {
    pub(crate) fn from_bytes(path: &str, bytes: &[u8]) -> Self {
        Self { path: path.into(), sha256: sha256(bytes) }
    }
}
