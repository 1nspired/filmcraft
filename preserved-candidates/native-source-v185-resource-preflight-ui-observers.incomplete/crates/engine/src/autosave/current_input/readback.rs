//! Complete original current-image readback after the SAME worker's FIFO ACK.
//! Retain the first complete file generations; failed initial reads cannot be
//! retried into a replacement baseline. No replay, writer, catalog settlement
//! or editor admission is created by this read-only component.
use super::*;
use crate::autosave::{
    JournalMeta, META, OriginalJournalFlush, SNAPSHOT,
    current_startup::release::Frame,
    held_checkpoint::publication::{CheckpointSource, release::ReleasePreparation},
};
use filmcraft_format::retained::preview_context::{DirectoryFrame, NodeFrame};
use std::{fs, io, os::unix::fs::MetadataExt};
const METADATA_LIMIT: u64 = 64 * 1024;
const NATIVE: &str = crate::agent_bridge::NATIVE_JOURNAL_FILE;
fn error(e: impl std::fmt::Display) -> String {
    e.to_string()
}
fn io_error(e: impl std::fmt::Display) -> io::Error {
    io::Error::other(e.to_string())
}
fn source_read<T>(source: &dyn CheckpointSource, read: impl FnOnce() -> io::Result<T>) -> io::Result<T> {
    source.current()?;
    let observed = read();
    source.fence_after_read()?;
    observed
}
fn namespace(session: &std::path::Path, check: &impl Fn() -> io::Result<()>) -> io::Result<()> {
    check()?;
    let mut names = Vec::new();
    for entry in fs::read_dir(session)? {
        check()?;
        if names.len() >= 4 {
            return Err(io_error("surplus current-image journal member; retain unknown work"));
        }
        names.push(entry?.file_name().into_string().map_err(|_| io_error("literal original journal file name required"))?);
    }
    names.sort();
    let mut expected = vec![crate::autosave::LOCK.to_owned(), SNAPSHOT.to_owned(), META.to_owned(), NATIVE.to_owned()];
    expected.sort();
    if names != expected {
        return Err(io_error("complete original current-image journal namespace required; no partial or unknown members"));
    }
    check()
}
fn metadata(actual: &[u8], expected: &[u8]) -> io::Result<()> {
    let mut actual_value: JournalMeta = serde_json::from_slice(actual).map_err(io_error)?;
    let expected_value: JournalMeta = serde_json::from_slice(expected).map_err(io_error)?;
    if serde_json::to_vec_pretty(&actual_value).map_err(io_error)? != actual {
        return Err(io_error("current metadata differs from the exact original worker recipe"));
    }
    // Only the original worker's new write timestamp legitimately differs.
    // It grants no authority and is not used as a liveness or release proof.
    actual_value.saved_unix = expected_value.saved_unix;
    if actual_value != expected_value {
        return Err(io_error("current metadata crossed the original PID, start, image, revision, path or saved state"));
    }
    Ok(())
}

pub(crate) struct CompleteCurrentImage {
    owner: Arc<()>,
    project: Arc<Project>,
    revision: u64,
    path: Option<String>,
    frame: Arc<Frame>,
    parent: Arc<DirectoryFrame>,
    outputs: Vec<NodeFrame>,
}
impl Persistence {
    pub(crate) fn read_current_image(
        &self,
        release: &CurrentImageRelease,
        input: &OriginalCurrentImageInput,
        flush: &OriginalJournalFlush,
        prepared: &ReleasePreparation,
        source: &dyn CheckpointSource,
        check: &impl Fn() -> io::Result<()>,
    ) -> Result<CompleteCurrentImage, String> {
        receipts(self, release, input, flush)?;
        check().map_err(error)?;
        flush.begin_current_readback()?;
        let image = source_read(source, || {
            prepared.original_journal_frame().inspect_acknowledged(self, release, check)?;
            namespace(&self.session_dir, check)?;
            let expected = prepared.expected_files(self).map_err(io_error)?;
            if expected.map(|(name, _)| name) != [SNAPSHOT, META, NATIVE] {
                return Err(io_error("the same original three complete checkpoint roles are required"));
            }
            let parent = DirectoryFrame::capture(&self.session_dir, check)?;
            if !parent.exists() || !parent.record().missing.is_empty() {
                return Err(io_error("the same actual original journal parent is required"));
            }
            let mut outputs = Vec::new();
            for (name, bytes) in expected {
                let max = if name == META { METADATA_LIMIT } else { u64::try_from(bytes.len()).map_err(io_error)? };
                outputs.push(parent.node_bounded(name, max, check)?);
            }
            let image = CompleteCurrentImage {
                owner: self.startup_identity.clone(),
                project: input.project.clone(),
                revision: input.revision,
                path: input.path.clone(),
                frame: prepared.original_journal_frame().clone(),
                parent,
                outputs,
            };
            image.inspect(self, release, input, flush, prepared, source, check).map_err(io_error)?;
            Ok(image)
        })
        .map_err(error)?;
        image.final_fences(self, release, check).map_err(error)?;
        receipts(self, release, input, flush)?;
        Ok(image)
    }
}
fn receipts(worker: &Persistence, release: &CurrentImageRelease, input: &OriginalCurrentImageInput, flush: &OriginalJournalFlush) -> Result<(), String> {
    worker.assert_original_current_image_input(release, input, &input.project, input.revision, input.path.as_deref())?;
    worker.assert_original_journal_flush(release, flush)?;
    if flush.written_revision().is_some_and(|revision| revision != input.revision) {
        return Err("the actual original FIFO acknowledgement named another image revision".into());
    }
    Ok(())
}
impl CompleteCurrentImage {
    /// Descriptions of the SAME first verified output witnesses, never fresh
    /// captures or authority. The actual controller checks current() before use.
    pub(crate) fn file_observations(&self) -> Result<serde_json::Value, String> {
        let mut files = Vec::new();
        for node in &self.outputs {
            let filmcraft_format::retained::preview_context::NodeState::File(observation) = node.state() else {
                return Err("the SAME complete original image file witness is required".into());
            };
            files.push(serde_json::json!({"path":node.path(),"observation":observation}));
        }
        if files.len() != 3 {
            return Err("all three SAME actual current-image file witnesses are required".into());
        }
        Ok(serde_json::json!(files))
    }
    fn same_input(&self, worker: &Persistence, input: &OriginalCurrentImageInput, prepared: &ReleasePreparation) -> io::Result<()> {
        if !Arc::ptr_eq(&self.owner, &worker.startup_identity)
            || !Arc::ptr_eq(&self.project, &input.project)
            || self.revision != input.revision
            || self.path != input.path
            || !Arc::ptr_eq(&self.frame, prepared.original_journal_frame())
            || self.parent.record().path != worker.session_dir
            || self.outputs.len() != 3
        {
            return Err(io_error("the retained complete image crossed its original worker, COW input or constructor frame"));
        }
        Ok(())
    }
    fn final_fences(&self, worker: &Persistence, release: &CurrentImageRelease, check: &impl Fn() -> io::Result<()>) -> io::Result<()> {
        self.frame.inspect_acknowledged(worker, release, check)?;
        self.parent.current(check)?;
        namespace(&worker.session_dir, check)?;
        for node in &self.outputs {
            node.fence(check)?;
            if fs::symlink_metadata(node.path())?.nlink() != 1 {
                return Err(io_error("current-image file has another link"));
            }
        }
        self.frame.inspect_acknowledged(worker, release, check)?;
        check()
    }
    fn inspect(
        &self,
        worker: &Persistence,
        release: &CurrentImageRelease,
        input: &OriginalCurrentImageInput,
        flush: &OriginalJournalFlush,
        prepared: &ReleasePreparation,
        source: &dyn CheckpointSource,
        check: &impl Fn() -> io::Result<()>,
    ) -> Result<(), String> {
        receipts(worker, release, input, flush)?;
        self.same_input(worker, input, prepared).map_err(error)?;
        if !flush.current_readback_started() {
            return Err("the original checked current-image readback was not started".into());
        }
        prepared.inspect_acknowledged_release(worker, release, source, check)?;
        let expected = prepared.expected_files(worker)?;
        let mut complete = Vec::new();
        self.final_fences(worker, release, check).map_err(error)?;
        for (node, (name, bytes)) in self.outputs.iter().zip(expected) {
            check().map_err(error)?;
            node.current(check).map_err(error)?;
            let max = if name == META { METADATA_LIMIT } else { u64::try_from(bytes.len()).map_err(error)? };
            let raw = if name == NATIVE { node.read_private_bytes(max, check) } else { node.read_owned_bytes(max, check) }.map_err(error)?;
            if name == META {
                metadata(&raw, bytes).map_err(error)?;
            } else if raw != bytes {
                return Err("the ordinary FIFO image differs from the original complete checkpoint bytes".into());
            }
            node.fence(check).map_err(error)?;
            complete.push(raw);
        }
        let snapshot = complete.first().ok_or_else(|| "complete original snapshot missing".to_owned())?;
        let native = complete.get(2).ok_or_else(|| "complete original native image missing".to_owned())?;
        if filmcraft_format::encode(&input.project, false) != *snapshot {
            return Err("the complete file image crossed its actual enqueued COW project".into());
        }
        crate::agent_bridge::validate_held_checkpoint_bytes(native, snapshot).map_err(error)?;
        self.final_fences(worker, release, check).map_err(error)?;
        self.same_input(worker, input, prepared).map_err(error)?;
        receipts(worker, release, input, flush)
    }
    pub(crate) fn current(
        &self,
        worker: &Persistence,
        release: &CurrentImageRelease,
        input: &OriginalCurrentImageInput,
        flush: &OriginalJournalFlush,
        prepared: &ReleasePreparation,
        source: &dyn CheckpointSource,
        check: &impl Fn() -> io::Result<()>,
    ) -> Result<(), String> {
        check().map_err(error)?;
        let observed = source_read(source, || self.inspect(worker, release, input, flush, prepared, source, check).map_err(io_error));
        // Source callbacks can fail or observe changes too. Keep the SAME
        // retained output generations after the final original source fence.
        let final_files = self.final_fences(worker, release, check);
        observed.map_err(error)?;
        final_files.map_err(error)
    }
}

#[cfg(test)]
mod tests;
