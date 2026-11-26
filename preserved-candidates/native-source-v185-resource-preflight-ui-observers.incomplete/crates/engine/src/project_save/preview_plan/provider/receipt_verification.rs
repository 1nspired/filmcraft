//! Fresh original receipt chains only. This never grants complete Root/namespace
//! custody, application adoption, publication, native completion or Resume.
use super::*;
use crate::project_save::retained_effects::DocumentReceipt;
use filmcraft_format::retained::{self, DirectoryPin, Observation, preview_context::NodeFrame};

const MAX_PINS: usize = 65536;
#[derive(Default)]
struct Fences {
    files: Vec<NodeFrame>,
    directories: Vec<Arc<DirectoryFrame>>,
}
impl Fences {
    fn room(&self) -> io::Result<()> {
        if self.files.len().saturating_add(self.directories.len()) >= MAX_PINS {
            return Err(error("original Save receipt inspection exceeds its bounded file fence set"));
        }
        Ok(())
    }
    fn node(&mut self, path: &Path, max: u64, check: &impl Fn() -> io::Result<()>) -> io::Result<&NodeState> {
        self.room()?;
        let parent = path.parent().ok_or_else(|| error("receipt input parent missing"))?;
        let name = path.file_name().and_then(|v| v.to_str()).ok_or_else(|| error("receipt input leaf unavailable"))?;
        let directory = DirectoryFrame::capture(parent, check)?;
        self.files.push(directory.node_bounded(name, max, check)?);
        self.files.last().map(NodeFrame::state).ok_or_else(|| error("receipt input fence unavailable"))
    }
    fn observed(&mut self, path: &Path, expected: Option<&Observation>, check: &impl Fn() -> io::Result<()>) -> io::Result<()> {
        let actual = self.node(path, expected.map_or(0, |v| v.bytes), check)?;
        if !matches!((actual, expected), (NodeState::Absent, None)) && !matches!((actual,expected),(NodeState::File(a),Some(e)) if a==e) {
            return Err(error("original Save receipt input changed after its complete inspection"));
        }
        Ok(())
    }
    fn unchanged(&mut self, before: &NodeFrame, check: &impl Fn() -> io::Result<()>) -> io::Result<()> {
        let max = match before.state() {
            NodeState::File(v) => v.bytes,
            _ => 0,
        };
        if self.node(&before.path(), max, check)? != before.state() {
            return Err(error("unmodified original Save file or absence changed"));
        }
        Ok(())
    }
    fn directory(&mut self, pin: &DirectoryPin, check: &impl Fn() -> io::Result<()>) -> io::Result<()> {
        self.room()?;
        let frame = DirectoryFrame::capture(&pin.path, check)?;
        if !frame.exists() || frame.record().anchor != *pin {
            return Err(error("original Save receipt directory generation changed"));
        }
        self.directories.push(frame);
        Ok(())
    }
    fn metadata(&mut self, directory: &Path, check: &impl Fn() -> io::Result<()>) -> io::Result<()> {
        for name in ["intent.json", "dispatch.json", "receipt.json"] {
            if !matches!(self.node(&directory.join(name), 65536, check)?, NodeState::File(_)) {
                return Err(error("original Save returned publication metadata is incomplete"));
            }
        }
        Ok(())
    }
    fn finish(&self, check: &impl Fn() -> io::Result<()>) -> io::Result<()> {
        check()?;
        for directory in &self.directories {
            directory.current(check)?;
        }
        for file in &self.files {
            file.fence(check)?;
        }
        check()
    }
}
fn directories(
    before: &Arc<DirectoryFrame>,
    publication: &directory_effect::Publication,
    fences: &mut Fences,
    check: &impl Fn() -> io::Result<()>,
) -> io::Result<()> {
    let old = before.record();
    if !publication.complete
        || publication.error.is_some()
        || publication.path != old.path
        || publication.successful_calls != old.missing
        || publication.observed.len() != old.missing.len()
    {
        return Err(error("original Save directory effects are incomplete or crossed"));
    }
    fences.directory(&old.anchor, check)?;
    for (path, pin) in old.missing.iter().zip(&publication.observed) {
        if &pin.path != path {
            return Err(error("original Save directory effect path changed"));
        }
        fences.directory(pin, check)?;
    }
    Ok(())
}
fn document(record: &DocumentReceipt, fences: &mut Fences, check: &impl Fn() -> io::Result<()>) -> io::Result<retained::Inspection> {
    let publication = record.publication.as_ref().ok_or_else(|| error("original Save document publication remains unknown"))?;
    if !record.qualified || record.error.is_some() || !publication.receipt_durable || publication.receipt.outcome != retained::Outcome::Published {
        return Err(error("original Save document receipt is not a confirmed publication"));
    }
    fences.metadata(&publication.directory, check)?;
    let seen = retained::inspect(&publication.directory, check)?;
    if seen.current != retained::CurrentState::Published
        || seen.recorded_receipt.as_ref() != Some(&publication.receipt)
        || seen.intent.destination != record.physical
    {
        return Err(error("original Save document publication no longer matches its exact receipt"));
    }
    fences.observed(&seen.intent.destination, seen.output.as_ref(), check)?;
    fences.observed(&publication.directory.join("payload.fcproj"), Some(&seen.intent.payload_archive), check)?;
    fences.observed(&publication.directory.join("exchange-slot.fcproj"), seen.slot.as_ref(), check)?;
    Ok(seen)
}
fn preview(entry: &super::super::Entry, row: &FileReport, fences: &mut Fences, check: &impl Fn() -> io::Result<()>) -> io::Result<()> {
    if row.name != entry.name {
        return Err(error("original indexed preview receipt order or name changed"));
    }
    if row.phase == Phase::Skipped {
        if row.rename.is_some() || row.copy.is_some() || row.retirement.is_some() || *entry.target.state() == NodeState::Absent {
            return Err(error("skipped original preview has unexpected physical effects"));
        }
        fences.unchanged(&entry.source, check)?;
        return fences.unchanged(&entry.target, check);
    }
    let source = entry.source.path();
    let target = entry.target.path();
    let NodeState::File(expected) = entry.source.state() else {
        return Err(error("original preview has no complete source for a successful receipt chain"));
    };
    if *entry.target.state() != NodeState::Absent {
        return Err(error("migrated original preview did not have its original absent target"));
    }
    let rename = if let Some(p) = &row.rename {
        if !p.receipt_durable {
            return Err(error("original preview rename receipt is not durable"));
        }
        fences.metadata(&p.directory, check)?;
        let seen = relocation::inspect(&p.directory, check)?;
        if seen.recorded_receipt.as_ref() != Some(&p.receipt)
            || seen.intent.source != source
            || seen.intent.destination != target
            || seen.intent.source_before != *expected
        {
            return Err(error("original preview rename receipt chain changed"));
        }
        fences.observed(&p.directory.join("source-archive.bin"), Some(&seen.intent.archive), check)?;
        fences.observed(&source, seen.source.as_ref(), check)?;
        fences.observed(&target, seen.destination.as_ref(), check)?;
        Some(seen)
    } else {
        None
    };
    match row.phase {
        Phase::Renamed => {
            let p = row.rename.as_ref().ok_or_else(|| error("original moved preview receipt missing"))?;
            if p.receipt.outcome != relocation::Outcome::Moved
                || rename.as_ref().is_none_or(|v| v.current != relocation::CurrentState::Moved)
                || row.copy.is_some()
                || row.retirement.is_some()
            {
                return Err(error("original moved preview remains uncertain or has crossed effects"));
            }
        }
        Phase::Retired => {
            // A failed rename may now read Changed: its original source was
            // subsequently retired by this exact copy. Do not weaken inspect.
            if row.rename.as_ref().is_some_and(|v| v.receipt.outcome != relocation::Outcome::NotMoved) {
                return Err(error("preview fallback has no original known-unmoved rename outcome"));
            }
            let copy = row.copy.as_ref().ok_or_else(|| error("original fallback copy missing"))?;
            let retirement = row.retirement.as_ref().ok_or_else(|| error("original fallback source retirement missing"))?;
            if !copy.receipt_durable
                || copy.receipt.outcome != copying::Outcome::Copied
                || !retirement.receipt_durable
                || retirement.receipt.outcome != copying::retirement::Outcome::Retired
            {
                return Err(error("original preview copy or retirement is not confirmed"));
            }
            fences.metadata(&copy.directory, check)?;
            fences.metadata(&retirement.directory, check)?;
            let seen = copying::retirement::inspect(&retirement.directory, check)?;
            if seen.current != copying::retirement::CurrentState::Retired
                || seen.recorded_receipt.as_ref() != Some(&retirement.receipt)
                || seen.intent.copy.directory.path != copy.directory
                || seen.intent.copy_receipt != copy.receipt
                || seen.intent.copy.source != source
                || seen.intent.copy.destination != target
                || seen.intent.copy.source_before != *expected
            {
                return Err(error("original preview fallback is not one exact retained copy/retirement chain"));
            }
            fences.observed(&copy.directory.join("source-archive.bin"), Some(&seen.intent.copy.archive), check)?;
            fences.observed(&copy.directory.join("copy-slot.bin"), None, check)?;
            fences.observed(&retirement.directory.join("retained-source.bin"), seen.retained.as_ref(), check)?;
            fences.observed(&source, seen.source.as_ref(), check)?;
            fences.observed(&target, seen.destination.as_ref(), check)?;
        }
        _ => return Err(error("original preview effect is partial, refused or unknown; workers remain owned")),
    }
    Ok(())
}
impl Report {
    /// Internal fresh read, not an imported result or a worker-release grant.
    pub(in crate::project_save) fn verify_returned_receipts(
        &self,
        documents: &[DocumentReceipt],
        check: &impl Fn() -> io::Result<()>,
    ) -> io::Result<(usize, usize)> {
        check()?;
        guard_remaining(&self._plan, &self.remaining, check)?;
        if !self.boundary_errors.is_empty() || self.files.len() != self._plan.entries.len() {
            return Err(error("original Save effect boundary remains unconfirmed"));
        }
        let mut fences = Fences::default();
        let docs = self.document_admission.as_ref().ok_or_else(|| error("complete original document admission missing"))?;
        if documents.len() != docs.rows.iter().filter(|r| r.assigned).count()
            || !docs.rows.iter().any(|r| r.role == crate::project_save::effects::FileRole::Project && r.assigned)
        {
            return Err(error("original Save document receipt set is incomplete"));
        }
        if let Some(p) = &docs.preferences {
            if !p.attempted {
                return Err(error("original preferences directory was never observed"));
            }
            directories(&p.before, p.publication.as_ref().ok_or_else(|| error("original preferences directory publication unknown"))?, &mut fences, check)?;
        }
        let mut inspected = Vec::new();
        for row in &docs.rows {
            check()?;
            if !row.errors.is_empty() {
                return Err(error("original document admission retained an error"));
            }
            if !row.assigned {
                if row.role != crate::project_save::effects::FileRole::SchemaBackup {
                    return Err(error("required original document role was not published"));
                }
                fences.unchanged(&row.frame, check)?;
                continue;
            }
            let found: Vec<_> = documents.iter().filter(|d| d.role == row.role).collect();
            let record = match found.as_slice() {
                [record] => *record,
                _ => return Err(error("missing or duplicate original document role receipt")),
            };
            if record.literal != row.literal || record.physical != row.frame.path() {
                return Err(error("original document receipt changed its role path"));
            }
            let seen = document(record, &mut fences, check)?;
            let expected_before = match row.frame.state() {
                NodeState::Absent => None,
                NodeState::File(v) => Some(v),
                _ => return Err(error("original document target was not a regular file or absence")),
            };
            if seen.intent.target_before.as_ref() != expected_before
                || row.payload.as_ref().is_none_or(|(bytes, hash)| seen.intent.payload_archive.bytes != *bytes || seen.intent.payload_archive.sha256 != *hash)
            {
                return Err(error("original document receipt changed its complete original input or encoded payload"));
            }
            inspected.push((record.physical.clone(), seen));
        }
        if let Some(source) = &docs.source {
            if let Some((_, seen)) = inspected.iter().find(|(p, _)| *p == source.path()) {
                let expected = match source.state() {
                    NodeState::Absent => None,
                    NodeState::File(v) => Some(v),
                    _ => return Err(error("original saved source is not a file or absence")),
                };
                if seen.intent.target_before.as_ref() != expected {
                    return Err(error("original saved source did not continue through its own publication"));
                }
            } else {
                fences.unchanged(source, check)?;
            }
        }
        if self._plan.may_move {
            directories(
                self._plan.target.as_ref().ok_or_else(|| error("original preview target directory missing"))?,
                self.directories.as_ref().ok_or_else(|| error("original preview directory publication missing"))?,
                &mut fences,
                check,
            )?;
        } else {
            if self.directories.is_some() {
                return Err(error("unplanned preview directory effects"));
            }
            if let Some(target) = &self._plan.target {
                target.current(check)?;
                fences.directories.push(target.clone());
            }
        }
        if let Some(source) = &self._plan.source {
            source.current(check)?;
            fences.directories.push(source.clone());
        }
        for (entry, row) in self._plan.entries.iter().zip(&self.files) {
            preview(entry, row, &mut fences, check)?;
        }
        guard_remaining(&self._plan, &self.remaining, check)?;
        fences.finish(check)?;
        Ok((documents.len(), self.files.len()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::project_save::{intent::Intent, operation};
    use serde_json::json;
    use std::{fs, os::unix::fs::MetadataExt};
    #[test]
    fn returned_preview_receipts_verify_actual_copy_retirement_succession_and_refuse_later_retained_source_replacement() {
        let (mut s, root, _) = super::super::complete_tests::fixture();
        let intent = Intent::parse(&json!({"kind":"save_as","path":root.join("Chosen.fcproj")})).unwrap();
        let id = format!("{:032x}", 7717);
        let key = operation::begin(&mut s, &id, &intent, &|| Ok(())).unwrap();
        let specs = [super::super::super::VolumeSpec { root: root.clone(), device: fs::metadata(&root).unwrap().dev().to_string(), minimum_free: 0 }];
        let plan = Plan::capture(&s, &key, &specs, &|| Ok(())).unwrap();
        let target = plan.target_literal.clone().unwrap();
        let mut original = crate::project_save::effects::Original;
        let mut provider = Provider::new(plan, &id, &mut original, &|| Ok(())).unwrap();
        provider.directories(DirectoryRole::Previews, &target).unwrap();
        // A source-preserving failure before a rename publication is one
        // original fallback entry. The copy and retirement below are real.
        provider.rows[0].phase = Phase::RenameFailed;
        provider.copy_effect(0).unwrap();
        provider.retire_effect(0).unwrap();
        let report = provider.into_report();
        let entry = &report._plan.entries[0];
        let row = &report.files[0];
        let mut fences = Fences::default();
        preview(entry, row, &mut fences, &|| Ok(())).unwrap();
        fences.finish(&|| Ok(())).unwrap();
        let retained = row.retirement.as_ref().unwrap().directory.join("retained-source.bin");
        let bytes = fs::read(&retained).unwrap();
        let permissions = fs::metadata(&retained).unwrap().permissions();
        fs::rename(&retained, root.join("old-retained-source")).unwrap();
        fs::write(&retained, bytes).unwrap();
        fs::set_permissions(&retained, permissions).unwrap();
        assert!(preview(entry, row, &mut Fences::default(), &|| Ok(())).is_err());
        assert!(s.previews.pause_writers(&|| Ok(())).is_err());
        s.shutdown();
    }
    #[test]
    fn returned_receipt_final_fences_refuse_an_earlier_same_byte_metadata_replacement_after_later_reads() {
        let (mut s, root, _) = super::super::complete_tests::fixture();
        let first = root.join("first-receipt.json");
        let later = root.join("later-receipt.json");
        fs::write(&first, b"original receipt").unwrap();
        fs::write(&later, b"later receipt").unwrap();
        let mut fences = Fences::default();
        fences.node(&first, 65536, &|| Ok(())).unwrap();
        fences.node(&later, 65536, &|| Ok(())).unwrap();
        fs::rename(&first, root.join("held-first-receipt.json")).unwrap();
        fs::write(&first, b"original receipt").unwrap();
        assert!(fences.finish(&|| Ok(())).is_err());
        s.shutdown();
    }
}
