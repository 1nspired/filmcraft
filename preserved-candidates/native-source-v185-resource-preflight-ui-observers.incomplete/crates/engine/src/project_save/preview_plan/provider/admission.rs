//! Live original file frames and credit ledger, retained without callbacks.
//! Descriptions cannot construct this owner or re-observe a newer target.
use super::*;
use crate::{
    Result, Session,
    project_save::{
        Prepared,
        operation::{Key, observation::Budget},
    },
};
use serde_json::{Value, json};
pub(crate) struct Admission {
    plan: Plan,
    operation: String,
    source: Option<Arc<DirectoryFrame>>,
    target: Option<Arc<DirectoryFrame>>,
    rows: Vec<FileReport>,
    lookup: BTreeMap<String, usize>,
    ledger: Ledger,
    initial_demand: BTreeMap<String, u64>,
    documents: Option<documents::Documents>,
    directories: Option<directory_effect::Publication>,
    directory_called: bool,
    boundary_errors: Vec<String>,
}
// This is the original complete_current fence shared by borrowed publication
// and retained admission. No second filesystem or native Save policy.
pub(super) fn current(
    plan: &Plan,
    ledger: &Ledger,
    documents: Option<&documents::Documents>,
    s: &Session,
    key: &Key,
    check: &dyn Fn() -> io::Result<()>,
) -> Result<()> {
    let now = || guard(plan, ledger, check);
    plan.current(s, key, &now)?;
    if let Some(documents) = documents {
        documents.current(&now).map_err(super::super::io_error)?;
    }
    plan.owner.current(s, key)?;
    for row in &plan.entries {
        row.source.fence(&now).map_err(super::super::io_error)?;
        row.target.fence(&now).map_err(super::super::io_error)?;
    }
    if let Some(documents) = documents {
        documents.fence(&now).map_err(super::super::io_error)?;
    }
    guard(plan, ledger, check).map_err(super::super::io_error)
}
impl Admission {
    pub(in crate::project_save) fn capture(
        s: &Session,
        key: &Key,
        specs: &[super::super::VolumeSpec],
        operation: &str,
        prepared: &Prepared,
        check: &dyn Fn() -> io::Result<()>,
    ) -> Result<Self> {
        let plan = Plan::capture(s, key, specs, &|| check())?;
        let mut original = crate::project_save::effects::Original;
        let files = Provider::new_complete(plan, prepared, operation, &mut original, check).map_err(super::super::io_error)?;
        files.complete_current(s, key)?;
        Ok(Self {
            plan: files.plan,
            operation: files.operation,
            source: files.source,
            target: files.target,
            rows: files.rows,
            lookup: files.lookup,
            ledger: files.ledger,
            initial_demand: files.initial_demand,
            documents: files.documents,
            directories: files.directories,
            directory_called: files.directory_called,
            boundary_errors: files.boundary_errors,
        })
    }
    pub(in crate::project_save) fn current(&self, s: &Session, key: &Key, check: &dyn Fn() -> io::Result<()>) -> Result<()> {
        current(&self.plan, &self.ledger, self.documents.as_ref(), s, key, check)
    }
    pub(in crate::project_save) fn bind<'a>(
        self,
        s: &Session,
        key: &Key,
        operation: &str,
        delegate: &'a mut dyn FileEffects,
        check: &'a dyn Fn() -> io::Result<()>,
    ) -> Result<Provider<'a>> {
        if self.operation != operation {
            return Err(crate::EngineError::Other("retained native Save admission belongs to another original operation".into()));
        }
        self.current(s, key, check)?;
        Ok(Provider {
            plan: self.plan,
            operation: self.operation,
            source: self.source,
            target: self.target,
            rows: self.rows,
            lookup: self.lookup,
            ledger: self.ledger,
            initial_demand: self.initial_demand,
            documents: self.documents,
            directories: self.directories,
            directory_called: self.directory_called,
            boundary_errors: self.boundary_errors,
            delegate,
            check,
        })
    }
    pub(in crate::project_save) fn description(&self, check: &impl Fn() -> Result<()>) -> Result<Value> {
        let mut budget = Budget::new(check);
        let plan = budget.value(&self.plan.description())?;
        let documents = self.documents.as_ref().map(|d| budget.value(&d.description())).transpose()?.unwrap_or(Value::Null);
        let remaining = budget.value(&self.ledger.remaining)?;
        let initial = budget.value(&self.initial_demand)?;
        let mut entries = Vec::new();
        for e in &self.plan.entries {
            budget.current()?;
            entries.push(budget.value(
                &json!({"name":e.name,"source":{"path":e.source.path(),"before":e.source.state()},"target":{"path":e.target.path(),"before":e.target.state()}}),
            )?);
        }
        let source = self.plan.source.as_ref().map(|d| budget.value(&d.record())).transpose()?.unwrap_or(Value::Null);
        let target = self.plan.target.as_ref().map(|d| budget.value(&d.record())).transpose()?.unwrap_or(Value::Null);
        budget.finish(json!({"kind":"native_original_complete_Save_file_admission","version":1,"operation":self.operation,
            "planned":plan,"documents":documents,"sourceDirectory":source,"targetDirectory":target,"entries":entries,"remaining":remaining,"initialDemand":initial,
            "current":false,"fileCustody":false,"overwriteAuthority":false,"spaceReserved":false,"writeAuthority":false,"nativeComplete":false,"startupAuthority":false,"workersMayResume":false}))
    }
}
