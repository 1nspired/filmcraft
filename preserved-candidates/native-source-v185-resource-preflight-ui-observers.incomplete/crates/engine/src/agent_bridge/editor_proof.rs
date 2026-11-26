//! Exact opaque editor testimony shared by native non-history services.
//! A decoded proof is descriptive: it cannot restore an editor, create a read
//! ticket, grant media access, or replace the caller's typed operation checks.
use super::parse;
use crate::{EditorState, Result, commands::bad};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

#[derive(Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Proof {
    version: u8,
    before_editor: Value,
    after_editor: Value,
    clipboard_sha256: String,
}
#[derive(Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub(super) struct OpaqueProof {
    kind: String,
    version: u8,
    sha256: String,
    bytes: usize,
    data: String,
}
pub(super) struct Decoded {
    pub(super) before: EditorState,
    pub(super) after_editor: Value,
    pub(super) clipboard_sha256: String,
}
fn valid_hash(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
pub(super) fn encode(before: &EditorState, after: &EditorState, clipboard: &str, kind: &str, limit: usize, command: &str) -> Result<OpaqueProof> {
    if !valid_hash(clipboard) {
        return Err(bad(command, "invalid historical native editor/clipboard proof"));
    }
    let inner = Proof {
        version: 1,
        before_editor: serde_json::to_value(before).map_err(|e| bad(command, e.to_string()))?,
        after_editor: serde_json::to_value(after).map_err(|e| bad(command, e.to_string()))?,
        clipboard_sha256: clipboard.into(),
    };
    // Preserve the original private byte format and native i64 clocks. The
    // caller separately bounds its complete receipt/envelope on the wire.
    let data = serde_json::to_string(&inner).map_err(|e| bad(command, e.to_string()))?;
    if data.len() > limit {
        return Err(bad(command, "view proof exceeds its private byte bound"));
    }
    Ok(OpaqueProof { kind: kind.into(), version: 1, sha256: format!("{:x}", Sha256::digest(data.as_bytes())), bytes: data.len(), data })
}
pub(super) fn exact_editor(value: &Value, command: &str) -> Result<EditorState> {
    let editor: EditorState = serde_json::from_value(value.clone()).map_err(|e| bad(command, e.to_string()))?;
    if serde_json::to_value(&editor).map_err(|e| bad(command, e.to_string()))? != *value {
        return Err(bad(command, "historical editor must roundtrip exactly without unknown or dropped fields"));
    }
    Ok(editor)
}
pub(super) fn decode(value: &Value, kind: &str, limit: usize, command: &str) -> Result<Decoded> {
    let opaque: OpaqueProof = parse(command, value)?;
    if opaque.kind != kind
        || opaque.version != 1
        || opaque.bytes == 0
        || opaque.bytes > limit
        || opaque.bytes != opaque.data.len()
        || !valid_hash(&opaque.sha256)
        || format!("{:x}", Sha256::digest(opaque.data.as_bytes())) != opaque.sha256
    {
        return Err(bad(command, "changed or oversized opaque native view proof"));
    }
    let inner: Proof = serde_json::from_str(&opaque.data).map_err(|e| bad(command, e.to_string()))?;
    if inner.version != 1 || !valid_hash(&inner.clipboard_sha256) {
        return Err(bad(command, "invalid historical native editor/clipboard proof"));
    }
    let before = exact_editor(&inner.before_editor, command)?;
    exact_editor(&inner.after_editor, command)?;
    Ok(Decoded { before, after_editor: inner.after_editor, clipboard_sha256: inner.clipboard_sha256 })
}
pub(super) fn require_after(proof_after: &Value, expected: &EditorState, command: &str) -> Result<()> {
    if *proof_after != serde_json::to_value(expected).map_err(|e| bad(command, e.to_string()))? {
        return Err(bad(command, "historical editor differs from the exact original human state change"));
    }
    Ok(())
}

#[cfg(test)]
mod tests;
