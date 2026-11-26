//! Bounded passive native command metadata, never an executor or an edit ticket.
use super::*;
use serde::de::{self, SeqAccess, Visitor};
use std::fmt;

const MAX_ENTRIES: usize = 65_536;
const MAX_BYTES: usize = 80 * 1024 * 1024;
const MAX_NODES: usize = 1_000_000;
const MAX_DEPTH: usize = 64;

#[derive(Clone, Serialize)]
#[serde(transparent)]
pub(in crate::agent_bridge) struct CommandJournal(Vec<(String, Value)>);

struct Budget {
    nodes: usize,
    bytes: usize,
}
impl Budget {
    fn new() -> Self {
        Self { nodes: MAX_NODES, bytes: MAX_BYTES }
    }
    fn text(&mut self, text: &str) -> Result<()> {
        self.bytes = self.bytes.checked_sub(text.len()).ok_or_else(|| bad("native.commandJournal", "command journal text exceeds its byte bound"))?;
        Ok(())
    }
    fn value(&mut self, value: &Value, depth: usize) -> Result<()> {
        self.nodes = self.nodes.checked_sub(1).ok_or_else(|| bad("native.commandJournal", "command journal exceeds its node bound"))?;
        if depth > MAX_DEPTH {
            return Err(bad("native.commandJournal", "command journal exceeds its depth bound"));
        }
        match value {
            Value::String(text) => self.text(text)?,
            Value::Array(items) => {
                for item in items {
                    self.value(item, depth + 1)?;
                }
            }
            Value::Object(items) => {
                for (key, item) in items {
                    self.text(key)?;
                    self.value(item, depth + 1)?;
                }
            }
            _ => {}
        }
        Ok(())
    }
    fn entry(&mut self, entry: &(String, Value)) -> Result<()> {
        self.text(&entry.0)?;
        self.value(&entry.1, 0)
    }
}
impl CommandJournal {
    pub(in crate::agent_bridge) fn check_append(s: &Session, entry: &(String, Value)) -> Result<()> {
        Self::validate_entries(&s.journal)?;
        let mut budget = Budget::new();
        for row in &s.journal {
            budget.entry(row)?;
        }
        budget.entry(entry)?;
        if s.journal.len() >= MAX_ENTRIES {
            return Err(bad("native.commandJournal", "command journal append exceeds entry bound"));
        }
        let mut entries = s.journal.clone();
        entries.push(entry.clone());
        Self::validate_entries(&entries)
    }

    fn validate_entries(entries: &[(String, Value)]) -> Result<()> {
        if entries.len() > MAX_ENTRIES {
            return Err(bad("native.commandJournal", "command journal exceeds its entry bound"));
        }
        let mut budget = Budget::new();
        for entry in entries {
            budget.entry(entry)?;
        }
        history_checkpoint::bytes(&entries, MAX_BYTES)?;
        Ok(())
    }
    pub(in crate::agent_bridge) fn capture(s: &Session) -> Result<Self> {
        // Validate depth, allocation and encoded size before recursive cloning.
        Self::validate_entries(&s.journal)?;
        Ok(Self(s.journal.clone()))
    }
    pub(in crate::agent_bridge) fn validate(&self) -> Result<()> {
        Self::validate_entries(&self.0)
    }
    pub(in crate::agent_bridge) fn require_empty_target(&self, s: &Session) -> Result<()> {
        self.validate()?;
        if !s.journal.is_empty() {
            return Err(bad("native.commandJournal", "cold command journal target must be fresh"));
        }
        Ok(())
    }
    pub(in crate::agent_bridge) fn install_cold(&self, s: &mut Session) -> Result<()> {
        self.require_empty_target(s)?;
        s.journal = self.0.clone();
        Ok(())
    }
    pub(in crate::agent_bridge) fn matches(&self, s: &Session) -> Result<bool> {
        Self::validate_entries(&s.journal)?;
        Ok(self.0 == s.journal)
    }
}
impl<'de> Deserialize<'de> for CommandJournal {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        struct Entries;
        impl<'de> Visitor<'de> for Entries {
            type Value = CommandJournal;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a bounded passive native command journal")
            }
            fn visit_seq<A: SeqAccess<'de>>(self, mut input: A) -> std::result::Result<Self::Value, A::Error> {
                if input.size_hint().is_some_and(|n| n > MAX_ENTRIES) {
                    return Err(de::Error::custom("command journal exceeds its entry bound"));
                }
                let mut entries = Vec::new();
                let mut budget = Budget::new();
                while entries.len() < MAX_ENTRIES {
                    let Some(entry) = input.next_element::<(String, Value)>()? else {
                        let result = CommandJournal(entries);
                        result.validate().map_err(de::Error::custom)?;
                        return Ok(result);
                    };
                    budget.entry(&entry).map_err(de::Error::custom)?;
                    entries.push(entry);
                }
                if input.next_element::<de::IgnoredAny>()?.is_some() {
                    return Err(de::Error::custom("command journal exceeds its entry bound"));
                }
                let result = CommandJournal(entries);
                result.validate().map_err(de::Error::custom)?;
                Ok(result)
            }
        }
        deserializer.deserialize_seq(Entries)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn source() -> Session {
        let mut s = Session::default();
        s.journal = vec![
            ("file.newBin".into(), json!({"name":"historical only"})),
            ("unknown.historical.command".into(), json!({"integer":9_007_199_254_740_993u64,"negative":-9_007_199_254_740_993i64})),
            (
                "agent.replayBatch".into(),
                json!({"label":"old batch","operations":[{"command":"file.newBin","params":{"name":"must not execute"}}],"exact":true}),
            ),
        ];
        s
    }
    #[test]
    fn complete_payload_roundtrip_is_passive_and_preserves_exact_order_and_values() {
        let source = source();
        let snapshot = Snapshot::capture_complete(&source).unwrap();
        let raw = history_checkpoint::bytes(&snapshot, MAX_BYTES).unwrap();
        let decoded: Snapshot = serde_json::from_slice(&raw).unwrap();
        assert_eq!(history_checkpoint::bytes(&decoded, MAX_BYTES).unwrap(), raw);
        let mut fresh = Session::default();
        let graph = fresh.project.clone();
        let revision = fresh.revision;
        decoded.install_cold(&mut fresh).unwrap();
        assert_eq!(fresh.journal, source.journal);
        assert!(std::sync::Arc::ptr_eq(&fresh.project, &graph));
        assert_eq!(fresh.revision, revision);
        assert!(fresh.history.undo.is_empty());
        assert!(fresh.history.redo.is_empty());
        assert!(fresh.persistence.is_none());
        assert!(!fresh.worker_available);
    }
    #[test]
    fn ordinary_scopes_do_not_capture_compare_or_rewind_the_live_command_log() {
        let source = source();
        for snapshot in [Snapshot::capture(&source).unwrap(), Snapshot::capture_journal(&source).unwrap()] {
            assert!(snapshot.command_journal.is_none());
            assert!(serde_json::to_value(&snapshot).unwrap().get("command_journal").is_none());
            let mut live = Session::default();
            live.journal.push(("later.human".into(), json!({"keep":true})));
            let expected = live.journal.clone();
            assert!(snapshot.matches(&live).unwrap());
            snapshot.install(&mut live).unwrap();
            assert_eq!(live.journal, expected);
        }
    }
    #[test]
    fn complete_scope_compares_the_actual_current_log_and_refuses_a_nonempty_target() {
        let mut source = source();
        let snapshot = Snapshot::capture_complete(&source).unwrap();
        assert!(snapshot.matches(&source).unwrap());
        source.journal.reverse();
        assert!(!snapshot.matches(&source).unwrap());
        let before = serde_json::to_value(&source.state).unwrap();
        let journal = source.journal.clone();
        assert!(snapshot.install_cold(&mut source).is_err());
        assert_eq!(source.journal, journal);
        assert_eq!(serde_json::to_value(&source.state).unwrap(), before);
    }
    #[test]
    fn absent_legacy_and_captured_empty_logs_remain_distinct() {
        let source = Session::default();
        let ordinary = serde_json::to_value(Snapshot::capture(&source).unwrap()).unwrap();
        let legacy: Snapshot = serde_json::from_value(ordinary.clone()).unwrap();
        assert!(legacy.command_journal.is_none());
        assert_eq!(serde_json::to_value(legacy).unwrap(), ordinary);
        let complete = Snapshot::capture_complete(&source).unwrap();
        assert_eq!(serde_json::to_value(&complete).unwrap()["command_journal"], json!([]));
    }
    #[test]
    fn malformed_command_log_shapes_fail_without_panicking() {
        for raw in ["{}", "[{}]", "[[1,{}]]", "[[\"id\"]]", "[[\"id\",{},3]]"] {
            assert!(serde_json::from_str::<CommandJournal>(raw).is_err(), "{raw}");
        }
    }
    #[test]
    fn excessive_entry_counts_are_refused_without_truncation() {
        let allowed = vec![(String::new(), Value::Null); MAX_ENTRIES];
        let raw = serde_json::to_vec(&allowed).unwrap();
        let decoded: CommandJournal = serde_json::from_slice(&raw).unwrap();
        assert_eq!(decoded.0, allowed);
        let mut source = Session::default();
        source.journal = allowed;
        source.journal.push(("excess".into(), Value::Null));
        assert!(CommandJournal::capture(&source).is_err());
        assert!(serde_json::from_slice::<CommandJournal>(&serde_json::to_vec(&source.journal).unwrap()).is_err());
        assert_eq!(source.journal.len(), MAX_ENTRIES + 1);
    }
    #[test]
    fn excessive_depth_nodes_and_text_are_refused_before_capture_clones() {
        let mut source = Session::default();
        let mut deep = Value::Null;
        for _ in 0..=MAX_DEPTH {
            deep = Value::Array(vec![deep]);
        }
        source.journal.push(("deep".into(), deep));
        assert!(CommandJournal::capture(&source).is_err());
        source.journal = vec![("nodes".into(), Value::Array(vec![Value::Null; MAX_NODES]))];
        assert!(CommandJournal::capture(&source).is_err());
        source.journal = vec![("text".into(), Value::String("a".repeat(MAX_BYTES)))];
        assert!(CommandJournal::capture(&source).is_err());
        // Escaping can exceed encoded bytes while the plain text still fits.
        source.journal = vec![("escaped".into(), Value::String("\u{0}".repeat(MAX_BYTES / 6)))];
        assert!(CommandJournal::capture(&source).is_err());
    }
    #[test]
    fn busy_complete_autosave_keeps_passive_log_without_resurrecting_jobs() {
        let mut source = source();
        source.jobs.push(crate::Job {
            id: 1,
            label: "Live".into(),
            progress: std::sync::Arc::new(filmcraft_export::Progress::default()),
            result: std::sync::Arc::new(std::sync::Mutex::new(None)),
        });
        assert!(Snapshot::capture_complete(&source).is_err());
        let journal = Snapshot::capture_complete_journal(&source).unwrap();
        assert!(journal.auxiliary.is_none());
        assert!(journal.command_journal.is_some());
        let mut fresh = Session::default();
        journal.install_cold(&mut fresh).unwrap();
        assert_eq!(fresh.journal, source.journal);
        assert!(fresh.jobs.is_empty());
    }
}
