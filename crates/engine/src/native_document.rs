//! Bounded, read-only project plane. Numeric JSON leaves and arrays are tagged
//! so JavaScript cannot round native ticks/ids or confuse numbers with strings.
use crate::{Result, Session, commands::bad};
use filmcraft_project::{BinEntry, ItemKind};
use serde::Serialize;
use serde_json::{Value, json};
use std::io::{self, Write};

const LIMIT: usize = 2 * 1024 * 1024;
const NODES: usize = 200_000;

#[derive(Default)]
struct Bounded(Vec<u8>);
impl Write for Bounded {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if self.0.len().checked_add(bytes.len()).is_none_or(|n| n > LIMIT) {
            return Err(io::Error::other("native document exceeds 2 MiB; paginated reads are required"));
        }
        self.0.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
fn bounded_json(value: &impl Serialize) -> Result<Vec<u8>> {
    let mut out = Bounded::default();
    serde_json::to_writer(&mut out, value).map_err(|e| bad("agent.document", e.to_string()))?;
    Ok(out.0)
}
fn encode(value: Value, depth: usize, budget: &mut usize) -> Result<Value> {
    if depth > 128 || *budget == 0 {
        return Err(bad("agent.document", "native document nesting/node limit"));
    }
    *budget -= 1;
    Ok(match value {
        Value::Number(n) => json!(["number", n.to_string()]),
        Value::Array(items) => json!(["array", items.into_iter().map(|v| encode(v, depth + 1, budget)).collect::<Result<Vec<_>>>()?]),
        Value::Object(items) => Value::Object(items.into_iter().map(|(k, v)| Ok((k, encode(v, depth + 1, budget)?))).collect::<Result<_>>()?),
        scalar => scalar,
    })
}
pub(crate) fn read(s: &mut Session, _: &Value) -> Result<Value> {
    // Bound the only recursive project container before serde traverses it.
    let mut stack = vec![(&s.project.root, 0usize)];
    let mut remaining = NODES;
    while let Some((bin, depth)) = stack.pop() {
        if depth > 64 || remaining == 0 {
            return Err(bad("agent.document", "native bin nesting/node limit"));
        }
        let spent = bin.children.len().checked_add(1).ok_or_else(|| bad("agent.document", "native bin size limit"))?;
        remaining = remaining.checked_sub(spent).ok_or_else(|| bad("agent.document", "native bin node limit"))?;
        for child in &bin.children {
            if let BinEntry::Bin(child) = child {
                stack.push((child, depth + 1));
            }
        }
        if stack.len() > NODES {
            return Err(bad("agent.document", "native bin node limit"));
        }
    }
    let bytes = bounded_json(&s.project)?;
    let project: Value = serde_json::from_slice(&bytes).map_err(|e| bad("agent.document", e.to_string()))?;
    let mut budget = NODES;
    let project = encode(project, 0, &mut budget)?;
    // Native methods compute source-out; the host does not reimplement edit math.
    let mut geometry = Vec::new();
    for (id, item) in &s.project.items {
        if let ItemKind::Sequence(q) = &item.kind {
            q.check_bounds().map_err(|e| bad("agent.document", e))?;
            for track in q.video_tracks.iter().chain(&q.audio_tracks) {
                for c in &track.items {
                    geometry.push(
                        json!({"sequence":id.0.to_string(),"track":track.id.0.to_string(),"clip":c.id.0.to_string(),"sourceOut":c.source_out().0.to_string()}),
                    );
                }
            }
        }
    }
    let sequence = s.state.active_sequence;
    let token = crate::agent_bridge::read_token(s, sequence)?;
    let reply = json!({"protocol":1,"encoding":"filmcraft-project-decimal-v1","token":token,"revision":s.revision.to_string(),"activeSequence":sequence.map(|s|s.0.to_string()),"project":project,"geometry":geometry});
    bounded_json(&reply)?;
    Ok(reply)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn integer_float_string_and_array_encodings_are_unambiguous() {
        let mut budget = NODES;
        let out = encode(json!({"tick":i64::MAX,"id":u64::MAX,"speed":1.0,"label":"1","original":["number","123"]}), 0, &mut budget).unwrap();
        assert_eq!(out["tick"], json!(["number", "9223372036854775807"]));
        assert_eq!(out["id"], json!(["number", "18446744073709551615"]));
        assert_eq!(out["speed"], json!(["number", "1.0"]));
        assert_eq!(out["label"], "1");
        assert_eq!(out["original"], json!(["array", ["number", "123"]]));
    }
    #[test]
    fn size_and_depth_limits_return_errors() {
        assert!(bounded_json(&"x".repeat(LIMIT)).is_err());
        let mut budget = NODES;
        assert!(encode(Value::Null, 129, &mut budget).is_err());
        let mut empty = 0;
        assert!(encode(Value::Null, 0, &mut empty).is_err());
    }
    #[test]
    fn full_project_reads_preserve_an_exhausted_unsigned_id_counter() {
        let mut s = Session::default();
        std::sync::Arc::make_mut(&mut s.project).next_id = u64::MAX;
        let doc = s.execute("agent.document", json!({})).unwrap();
        assert_eq!(doc["project"]["next_id"], json!(["number", "18446744073709551615"]));
        assert_eq!(s.project.next_id, u64::MAX);
    }
    #[test]
    fn a_document_read_preserves_revision_history_journal_and_pending_ticket() {
        let mut s = Session::default();
        let (p, q) = crate::demo::demo_project(&s.media);
        s.project = std::sync::Arc::new(p);
        s.state.active_sequence = Some(q);
        let before = s.project.clone();
        let snapshot = s.execute("agent.snapshot", json!({})).unwrap();
        let revision = s.revision;
        let depth = s.history.undo.len();
        let (_, events, _subscription) = s.command_journal.subscribe().unwrap();
        let full = s.execute("agent.document", json!({})).unwrap();
        assert_eq!(full["token"], snapshot["token"]);
        assert_eq!(full["activeSequence"], q.0.to_string());
        assert!(std::sync::Arc::ptr_eq(&before, &s.project));
        assert_eq!(s.revision, revision);
        assert_eq!(s.history.undo.len(), depth);
        assert!(events.try_recv().is_err());
        let c = s.active_sequence().unwrap().video_tracks[0].items[0].id.0;
        s.execute(
            "agent.apply",
            json!({"token":snapshot["token"],"operations":[{"command":"timeline.trim","params":{"clip":c,"edge":"out","deltaFrames":-1}}]}),
        )
        .unwrap();
    }
}
