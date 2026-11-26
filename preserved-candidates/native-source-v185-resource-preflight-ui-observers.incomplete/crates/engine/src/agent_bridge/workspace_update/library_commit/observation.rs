//! Observational closure of the actual candidate's retained library owner.
//! This value cannot reconstruct a Commit, StagedWrite, worker or startup grant.
use super::*;
use std::sync::Mutex;
pub(in crate::agent_bridge::workspace_update) struct Observation {
    owner: Option<Arc<Mutex<Commit>>>,
    pub(in crate::agent_bridge::workspace_update) record: Value,
}
impl Observation {
    fn inspect_owner(s: &Session, owner: &Arc<Mutex<Commit>>, check: &impl Fn() -> io::Result<()>) -> Result<Value> {
        let commit = owner.lock().unwrap_or_else(|e| e.into_inner());
        let h = s.agent_bridge.workspace_update.as_ref().ok_or_else(|| fail("actual paused library owner required"))?;
        if h.token != commit.operation
            || h.persistence_directory.as_ref().is_none_or(|r| !Arc::ptr_eq(r, &commit.root))
            || h.persistence_start.as_ref().is_none_or(|cfg| cfg.data_dir != commit.root.record().path)
        {
            return Err(fail("actual original library directory and activation binding changed"));
        }
        commit.observe(s, check)
    }
    pub(in crate::agent_bridge::workspace_update) fn capture(s: &Session, check: &impl Fn() -> io::Result<()>) -> Result<Self> {
        let owner = s.agent_bridge.workspace_update.as_ref().and_then(|h| h.library_commit.clone());
        let record = if let Some(owner) = &owner { Self::inspect_owner(s, owner, check)? } else { Value::Null };
        let value = Self { owner, record };
        value.current(s, check)?;
        Ok(value)
    }
    pub(in crate::agent_bridge::workspace_update) fn current(&self, s: &Session, check: &impl Fn() -> io::Result<()>) -> Result<()> {
        let current = s.agent_bridge.workspace_update.as_ref().and_then(|h| h.library_commit.as_ref());
        match (&self.owner, current) {
            (None, None) => check().map_err(fail),
            (Some(original), Some(current)) if Arc::ptr_eq(original, current) => {
                let seen = Self::inspect_owner(s, original, check)?;
                if seen != self.record {
                    return Err(fail("original library publication effects changed after recovery image capture"));
                }
                Ok(())
            }
            _ => Err(fail("actual original library publication owner changed; metadata cannot reconstruct it")),
        }
    }
}
impl Commit {
    /// Cold handoff adds original control generations to the same verified
    /// observation. Ordinary warm/current-image records keep their wire shape.
    pub(super) fn observe_with_controls(&self, s: &Session, check: &impl Fn() -> io::Result<()>) -> Result<Value> {
        let mut value = self.observe(s, check)?;
        let rows = value["entries"].as_array_mut().ok_or_else(|| fail("typed original canonical rows required"))?;
        if rows.len() != self.entries.len() {
            return Err(fail("original canonical observation row count changed"));
        }
        for (row, entry) in rows.iter_mut().zip(&self.entries) {
            if let Effect::Published { publication, .. } = &entry.effect {
                row["originalControls"] = serde_json::to_value(publication.control_observations()).map_err(fail)?;
            }
        }
        Ok(value)
    }
    pub(super) fn observe(&self, s: &Session, check: &impl Fn() -> io::Result<()>) -> Result<Value> {
        self.current_memory(s, check)?;
        self.shortcut.current(check).map_err(fail)?;
        let mut entries = Vec::new();
        for e in &self.entries {
            self.current_memory(s, check)?;
            let target = frame(&self.root.record().path.join(e.relative), check)?;
            let operation = format!("{:x}", Sha256::digest(format!("{}:native-library:{}", self.operation, e.relative).as_bytes()));
            let directory = target.path().parent().ok_or_else(|| fail("library parent missing"))?.join(format!(".poema-save-{operation}.incomplete"));
            let mut row = json!({"relative":e.relative,"payloadBytes":e.bytes.len(),"payloadSha256":format!("{:x}",Sha256::digest(&e.bytes)),
                "source":e.source.as_ref().map(|f|json!({"path":f.path(),"state":f.state()})),
                "target":{"path":target.path(),"state":target.state()},"operationDirectory":directory});
            match &e.effect {
                Effect::Before(before) | Effect::Unchanged(before) => {
                    row["phase"] = json!(if matches!(&e.effect, Effect::Before(_)) { "before" } else { "unchanged" });
                    row["originalTarget"] = json!({"path":before.path(),"state":before.state()});
                }
                Effect::Staging => {
                    // Staging may have failed before an immutable intent existed.
                    // A matching directory never manufactures a StagedWrite.
                    row["phase"] = json!("staging_unknown");
                    row["retainedDispatchOwner"] = json!(false);
                }
                Effect::Staged(staged) => {
                    if staged.directory() != directory {
                        return Err(fail("actual staged library directory differs"));
                    }
                    row["phase"] = json!("staged");
                    row["observation"] = serde_json::to_value(
                        retained::inspect_expected_after_directory_seals(staged.directory(), staged.intent(), &self.directory_seals, check).map_err(fail)?,
                    )
                    .map_err(fail)?;
                }
                Effect::Published { publication, intent } => {
                    if publication.directory != directory {
                        return Err(fail("original published library directory differs"));
                    }
                    row["phase"] = json!("published");
                    row["originalReceipt"] = serde_json::to_value(&publication.receipt).map_err(fail)?;
                    row["originalReceiptDurable"] = json!(publication.receipt_durable);
                    row["observation"] = serde_json::to_value(
                        retained::inspect_publication_after_directory_seals(publication, intent, &self.directory_seals, check).map_err(fail)?,
                    )
                    .map_err(fail)?;
                }
            }
            target.current(check).map_err(fail)?;
            target.fence(check).map_err(fail)?;
            entries.push(row);
        }
        self.current_memory(s, check)?;
        Ok(json!({"kind":"native-original-library-publication-effects","version":1,"activationToken":self.operation,
            "directory":self.root.record(),"shortcutDirectory":self.shortcut.record(),"directoryAttempted":self.directory_attempted,
            "stopped":self.stopped,"entries":entries,"writeAuthority":false,"startupAuthority":false,"retirementAuthority":false,
            "workersMayResume":false,"replayAllowed":false}))
    }
}
