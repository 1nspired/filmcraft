//! File receiver for the ONE original Save body, not a native RPC/controller.
//! The caller must retain actual native/file/worker ownership across unresolved
//! effects. This receiver never releases workers, retries or adopts from JSON.
pub(crate) use super::effects::FileRole as Role;
use super::{
    Prepared,
    effects::{DirectoryRole, DocumentRole, Event, FileEffects},
};
use crate::{Services, saved_file};
use filmcraft_format::retained::{self, CurrentState, Outcome, Publication};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeSet,
    io,
    path::{Path, PathBuf},
};
pub(crate) struct DocumentReceipt {
    pub(crate) role: Role,
    pub(crate) literal: String,
    pub(crate) physical: PathBuf,
    pub(crate) publication: Option<Publication>,
    pub(crate) qualified: bool,
    pub(crate) error: Option<String>,
}
pub(super) struct Expected {
    pub role: Role,
    pub literal: String,
    pub sha256: Option<String>,
    pub bytes: Option<usize>,
}
pub(crate) struct RetainedDocuments<'a> {
    operation: String,
    cwd: PathBuf,
    current_baseline: Option<(String, String)>,
    expected: Vec<Expected>,
    attempted: BTreeSet<Role>,
    documents: Vec<DocumentReceipt>,
    delegate: &'a mut dyn FileEffects,
    check: &'a dyn Fn() -> io::Result<()>,
}
fn error(message: impl Into<String>) -> io::Error {
    io::Error::other(message.into())
}
/// One original role catalog, shared by admission and the actual receiver.
pub(super) fn expected(prepared: &Prepared, cwd: &Path) -> io::Result<Vec<Expected>> {
    let mut expected = vec![Expected {
        role: Role::Project,
        literal: prepared.path.clone(),
        sha256: Some(saved_file::sha256(&prepared.bytes)),
        bytes: Some(prepared.bytes.len()),
    }];
    if prepared.adopt && prepared.input.path.as_deref() == Some(prepared.path.as_str()) && prepared.input.loaded_schema < filmcraft_format::SCHEMA_VERSION {
        expected.push(Expected {
            role: Role::SchemaBackup,
            literal: super::schema_backup_path(&prepared.path, prepared.input.loaded_schema),
            sha256: prepared.input.saved_file.as_ref().map(|b| b.sha256.clone()),
            bytes: None,
        });
    }
    if prepared.adopt && prepared.input.prefs.general.recent_projects.first().map(String::as_str) != Some(prepared.path.as_str()) {
        if let Some(path) = &prepared.input.prefs_path {
            let mut prefs = prepared.input.prefs.clone();
            prefs.note_recent(&prepared.path);
            let bytes = serde_json::to_vec_pretty(&prefs).unwrap_or_default();
            let literal = path.to_str().ok_or_else(|| error("native preferences path cannot be represented"))?.to_string();
            expected.push(Expected { role: Role::Preferences, literal, sha256: Some(saved_file::sha256(&bytes)), bytes: Some(bytes.len()) });
        }
    }
    // Different native roles may not silently destroy one another's files.
    let mut paths = BTreeSet::new();
    for e in &expected {
        let p = if Path::new(&e.literal).is_absolute() { PathBuf::from(&e.literal) } else { cwd.join(&e.literal) };
        if !paths.insert(p) {
            return Err(error("native Save document roles collide at one destination"));
        }
    }
    Ok(expected)
}
impl<'a> RetainedDocuments<'a> {
    pub(crate) fn new(prepared: &Prepared, operation: &str, delegate: &'a mut dyn FileEffects, check: &'a dyn Fn() -> io::Result<()>) -> io::Result<Self> {
        check()?;
        if !(32..=128).contains(&operation.len()) || !operation.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'-') {
            return Err(error("exact native Save operation id required"));
        }
        let cwd = std::env::current_dir()?;
        if std::fs::canonicalize(&cwd)? != cwd {
            return Err(error("native current directory is redirected"));
        }
        let expected = expected(prepared, &cwd)?;
        let current_baseline = prepared.input.saved_file.as_ref().map(|b| (b.path.clone(), b.sha256.clone()));
        Ok(Self { operation: operation.into(), cwd, current_baseline, expected, attempted: BTreeSet::new(), documents: Vec::new(), delegate, check })
    }
    pub(crate) fn into_documents(self) -> Vec<DocumentReceipt> {
        self.documents
    }
    pub(crate) fn documents(&self) -> &[DocumentReceipt] {
        &self.documents
    }
    pub(crate) fn has_unconfirmed_documents(&self) -> bool {
        self.documents.iter().any(|d| {
            d.publication
                .as_ref()
                .is_some_and(|p| matches!(p.receipt.outcome, Outcome::Published | Outcome::ConflictRetained | Outcome::Uncertain) && !d.qualified)
        })
    }
    /// Recheck completed document receipts after later auxiliary work. This
    /// does not settle an originally uncertain native/file operation or retry.
    pub(crate) fn verify_documents(&mut self) -> io::Result<()> {
        self.guard()?;
        for index in 0..self.documents.len() {
            let result = match self.documents[index].publication.as_ref() {
                Some(p) if p.receipt_durable && p.receipt.outcome == Outcome::Published => retained::inspect(&p.directory, &|| self.guard())
                    .map(|r| r.current == CurrentState::Published && r.recorded_receipt.as_ref() == Some(&p.receipt)),
                Some(_) => Ok(false),
                None => continue,
            };
            if !result.unwrap_or(false) {
                self.documents[index].qualified = false;
                return Err(error("native Save document receipt changed or remains unconfirmed; no publication repeated"));
            }
        }
        self.guard()
    }
    fn guard(&self) -> io::Result<()> {
        (self.check)()?;
        self.delegate.current_save_files()?;
        if std::env::current_dir()? != self.cwd || std::fs::canonicalize(&self.cwd)? != self.cwd {
            return Err(error("native Save current-directory admission expired"));
        }
        Ok(())
    }
    fn write(&mut self, role: Role, literal: &str, bytes: &[u8]) -> io::Result<()> {
        self.guard()?;
        let expected = self.expected.iter().find(|e| e.role == role).ok_or_else(|| error("native Save did not plan this file role"))?;
        if expected.literal != literal
            || expected.bytes.is_some_and(|n| n != bytes.len())
            || expected.sha256.as_ref().is_some_and(|h| *h != saved_file::sha256(bytes))
        {
            return Err(error("native Save role path or original complete payload changed"));
        }
        if !self.attempted.insert(role) {
            return Err(error("native Save file role was already attempted; reconcile its exact receipt before any continuation"));
        }
        let physical = if Path::new(literal).is_absolute() { PathBuf::from(literal) } else { self.cwd.join(literal) };
        let tag = format!("{:x}", Sha256::digest(format!("{}:{role:?}", self.operation).as_bytes()));
        // The delegate receives the actual native role and stages it once.
        // Its complete provider guards all credits while streaming. Keep this
        // closure independent of the mutable delegate borrow.
        let check = self.check;
        let cwd = self.cwd.clone();
        let current = || {
            check()?;
            if std::env::current_dir()? != cwd || std::fs::canonicalize(&cwd)? != cwd {
                return Err(error("native Save directory changed before document stage"));
            }
            Ok(())
        };
        let staged = match self.delegate.stage_document(role, &physical, &tag, bytes, &current) {
            Ok(v) => v,
            Err(e) => {
                self.documents.push(DocumentReceipt {
                    role,
                    literal: literal.into(),
                    physical,
                    publication: None,
                    qualified: false,
                    error: Some(e.to_string()),
                });
                return Err(e);
            }
        };
        if role == Role::Project
            && self
                .current_baseline
                .as_ref()
                .is_some_and(|(path, hash)| path == literal && staged.intent().target_before.as_ref().is_some_and(|actual| &actual.sha256 != hash))
        {
            let message = "current native saved bytes changed; historical metadata cannot authorize overwrite";
            self.documents.push(DocumentReceipt { role, literal: literal.into(), physical, publication: None, qualified: false, error: Some(message.into()) });
            return Err(error(message));
        }
        let publication = staged.publish(&|| self.guard());
        let qualified = if publication.receipt.outcome == Outcome::Published && publication.receipt_durable {
            retained::inspect(&publication.directory, &|| self.guard())
                .is_ok_and(|r| r.current == CurrentState::Published && r.recorded_receipt.as_ref() == Some(&publication.receipt))
        } else {
            false
        };
        let failure = (!qualified).then(|| {
            publication.receipt.error.clone().unwrap_or_else(|| "native Save file publication is unconfirmed or retained a conflict; do not repeat it".into())
        });
        self.documents.push(DocumentReceipt { role, literal: literal.into(), physical, publication: Some(publication), qualified, error: failure.clone() });
        match failure {
            Some(message) => Err(error(message)),
            None => Ok(()),
        }
    }
    fn refused(&mut self, phase: &'static str, path: &Path, e: &io::Error) {
        self.delegate.event(Event::BoundaryRefused { phase, path: path.into(), error: e.to_string() });
    }
}
impl FileEffects for RetainedDocuments<'_> {
    fn document(&mut self, _services: &dyn Services, role: DocumentRole, path: &str, bytes: &[u8]) -> io::Result<()> {
        self.write(
            match role {
                DocumentRole::SchemaBackup => Role::SchemaBackup,
                DocumentRole::Project => Role::Project,
            },
            path,
            bytes,
        )
    }
    fn preferences(&mut self, path: &Path, bytes: &[u8]) -> io::Result<()> {
        self.write(Role::Preferences, path.to_str().ok_or_else(|| error("native preferences path cannot be represented"))?, bytes)
    }
    fn directories(&mut self, role: DirectoryRole, path: &Path) -> io::Result<()> {
        self.guard()?;
        self.delegate.directories(role, path)
    }
    fn preview_exists(&mut self, path: &Path) -> bool {
        if let Err(e) = self.guard() {
            self.refused("preview_exists", path, &e);
            return true;
        }
        self.delegate.preview_exists(path)
    }
    fn preview_rename(&mut self, from: &Path, to: &Path) -> io::Result<()> {
        self.guard()?;
        self.delegate.preview_rename(from, to)
    }
    fn preview_copy(&mut self, from: &Path, to: &Path) -> io::Result<u64> {
        self.guard()?;
        self.delegate.preview_copy(from, to)
    }
    fn preview_remove(&mut self, path: &Path) -> io::Result<()> {
        self.guard()?;
        self.delegate.preview_remove(path)
    }
    fn event(&mut self, event: Event) {
        self.delegate.event(event)
    }
}
