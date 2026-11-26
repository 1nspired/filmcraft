//! Descriptive projection of the existing complete provider, not a new writer.
use super::*;
use crate::{Result, project_save::operation::observation::Budget};
use serde_json::{Value, json};
impl Report {
    pub(in crate::project_save) fn observation(&self, budget: &mut Budget<'_>) -> Result<Value> {
        budget.current()?;
        let original = budget;
        let mut branch = original.branch();
        let budget = &mut branch;
        let planned = budget.value(&self._plan.description())?;
        let admission = self.document_admission.as_ref().map(|v| budget.value(&v.description())).transpose()?.unwrap_or(Value::Null);
        let directories = budget.value(&self.directories)?;
        let boundary_errors = budget.value(&self.boundary_errors)?;
        let remaining = budget.value(&self.remaining)?;
        let initial = budget.value(&self.initial_demand)?;
        let mut files = Vec::new();
        for row in &self.files {
            budget.current()?;
            let rename = if let Some(p) = &row.rename {
                let source_check = budget.checker();
                let check = || source_check().map_err(|e| io::Error::other(e.to_string()));
                let inspection = budget.inspection(|| relocation::inspect(&p.directory, &check))?;
                budget.local(&json!({"directory":p.directory,"receipt":p.receipt,"receiptDurable":p.receipt_durable,"inspection":inspection}))?
            } else {
                Value::Null
            };
            let copy = if let Some(p) = &row.copy {
                let source_check = budget.checker();
                let check = || source_check().map_err(|e| io::Error::other(e.to_string()));
                let inspection = budget.inspection(|| copying::inspect(&p.directory, &check))?;
                budget.local(&json!({"directory":p.directory,"receipt":p.receipt,"receiptDurable":p.receipt_durable,"inspection":inspection}))?
            } else {
                Value::Null
            };
            let retirement = if let Some(p) = &row.retirement {
                let source_check = budget.checker();
                let check = || source_check().map_err(|e| io::Error::other(e.to_string()));
                let inspection = budget.inspection(|| copying::retirement::inspect(&p.directory, &check))?;
                budget.local(&json!({"directory":p.directory,"receipt":p.receipt,"receiptDurable":p.receipt_durable,"inspection":inspection}))?
            } else {
                Value::Null
            };
            files.push(
                budget.value(
                    &json!({"name":row.name,"phase":format!("{:?}",row.phase),"rename":rename,"copy":copy,"retirement":retirement,"errors":row.errors}),
                )?,
            );
        }
        original.value(&json!({"kind":"native_original_Save_preview_effect_observation","version":1,"planned":planned,"documentAdmission":admission,"directories":directories,
            "boundaryErrors":boundary_errors,"remaining":remaining,"initialDemand":initial,"files":files,
            "current":false,"allEffectsVerified":false,"writeAuthority":false,"nativeComplete":false,"startupAuthority":false,"workersMayResume":false}))
    }
}
