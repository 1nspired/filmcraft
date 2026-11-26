//! Full bounded project comparison, including authored automatic NaN points.
use crate::{Result, commands::bad};
use std::io::Write;
struct Bounded(Vec<u8>);
impl Write for Bounded {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
        if self.0.len().checked_add(b.len()).is_none_or(|n| n > 16 * 1024 * 1024) {
            return Err(std::io::Error::other("native project audit exceeds 16 MiB"));
        }
        self.0.extend_from_slice(b);
        Ok(b.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
pub(super) fn bytes(p: &filmcraft_project::Project) -> Result<Vec<u8>> {
    value_bytes(p)
}
pub(super) fn value_bytes(value: &impl serde::Serialize) -> Result<Vec<u8>> {
    let mut out = Bounded(Vec::new());
    serde_json::to_writer(&mut out, value).map_err(|e| bad("agent.apply", e.to_string()))?;
    Ok(out.0)
}
