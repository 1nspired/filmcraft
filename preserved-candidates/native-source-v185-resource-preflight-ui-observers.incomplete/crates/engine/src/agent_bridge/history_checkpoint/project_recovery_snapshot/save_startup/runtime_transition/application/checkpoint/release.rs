//! Actual original current COW retained before the existing worker exchange.
use super::*;
impl Prepared {
    pub(in super::super) fn prepare_current_release(&mut self, s: &Session, source: &impl Fn() -> Result<()>, check: &media_guard::Check) -> Result<Value> {
        check.check()?;
        self.current(s)?;
        if self.current_release_attempted {
            return Err(error("the original current-release preparation was already attempted; no replay"));
        }
        let original_worker = s.persistence.as_ref().ok_or_else(|| error("the SAME actual original current-release worker is required"))?;
        self.archive.as_ref().ok_or_else(|| error("the SAME actual acknowledged archive is required"))?.require_current_release(original_worker)?;
        source()?;
        check.check()?;
        self.current_release_attempted = true;
        let native = crate::agent_bridge::history_checkpoint::journal::capture_retained(s)?;
        self.current_native = Some(native);
        let worker = s.persistence.as_ref().ok_or_else(|| error("the SAME actual original current-release worker is required"))?;
        self.current_native.as_ref().ok_or_else(|| error("actual complete COW input missing"))?.assert_current_checkpoint_input(
            &s.project,
            s.revision,
            s.path.as_deref(),
            &worker.data_dir,
            &worker.session_dir,
        )?;
        self.base_current(s)?;
        source()?;
        let archive = self.archive.as_mut().ok_or_else(|| error("the SAME actual acknowledged checkpoint archive is required"))?;
        let publication = archive.prepare_current_release(worker, &|| check.check().map_err(|e| io::Error::other(e.to_string())))?;
        let frame = archive.release_preparation()?.original_journal_frame();
        frame.assert_worker(worker).map_err(error)?;
        frame.inspect_source(&|| check.check().map_err(|e| io::Error::other(e.to_string()))).map_err(error)?;
        self.base_current(s)?;
        source()?;
        self.archive.as_ref().ok_or_else(|| error("actual archive missing after preparation"))?.current(worker)?;
        check.check()?;
        let result = json!({"kind":"native_cold_Save_current_release_preparation","version":1,
            "nativePid":std::process::id(),"nativeSession":self.description["nativeSession"],
            "projectPath":s.path,"revision":s.revision,"savedRevision":s.saved_revision,
            "checkpoint":self.description,"archive":publication,"completeCOWRetained":true,
            "sameOriginalConstructorRetained":true,"editingPaused":true,"startupPending":true,
            "workerReleased":false,"fifoEnqueued":false,"writeAuthority":false,"startupAuthority":false,
            "workersMayResume":false,"fullApplicationClosure":false});
        bytes(&result, INPUT)?;
        self.current_release_ready = true;
        Ok(result)
    }
}
