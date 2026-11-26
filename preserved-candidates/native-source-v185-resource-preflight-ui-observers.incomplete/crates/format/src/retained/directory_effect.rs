//! Descriptor-relative mkdir effects for an exact previously absent chain.
//! No replacement, cleanup, rollback or claim that call returns prove custody.
use super::{Directory, DirectoryPin, error, preview_context::DirectoryFrame};
use rustix::fs::{Mode, OFlags, fchmod, mkdirat, openat};
use serde::Serialize;
use std::{io, path::PathBuf, sync::Arc};
pub struct StagedDirectories {
    frame: Arc<DirectoryFrame>,
}
#[derive(Clone, Debug, Serialize)]
pub struct Publication {
    pub path: PathBuf,
    pub successful_calls: Vec<PathBuf>,
    pub observed: Vec<DirectoryPin>,
    pub complete: bool,
    pub error: Option<String>,
    pub write_authority: bool,
    pub custody: bool,
}
pub fn stage(frame: Arc<DirectoryFrame>, check: &impl Fn() -> io::Result<()>) -> io::Result<StagedDirectories> {
    frame.current(check)?;
    Ok(StagedDirectories { frame })
}
impl StagedDirectories {
    /// Retain only the directory actually created by this exact absent-leaf call.
    /// Historical Publication values cannot reconstruct this owner.
    pub fn publish_owned(self, check: &impl Fn() -> io::Result<()>) -> io::Result<OwnedDirectory> {
        if self.frame.record().missing.len() != 1 {
            return Err(error("owned directory publication requires exactly one absent leaf"));
        }
        let mut owned = Vec::new();
        let publication = self.publish_collect(check, &mut |_| Ok(()), &mut owned, Mode::RWXU);
        if !publication.complete || owned.len() != 1 {
            return Err(error(publication.error.unwrap_or_else(|| "directory publication is unresolved; retain effects without replay".into())));
        }
        let directory = owned.pop().ok_or_else(|| error("actual created directory owner missing"))?;
        directory.current()?;
        Ok(OwnedDirectory { directory })
    }
    pub fn publish(self, check: &impl Fn() -> io::Result<()>) -> Publication {
        self.publish_with(check, &mut |_| Ok(()))
    }
    fn publish_with(self, check: &impl Fn() -> io::Result<()>, after_mkdir: &mut impl FnMut(&PathBuf) -> io::Result<()>) -> Publication {
        self.publish_collect(check, after_mkdir, &mut Vec::new(), Mode::RWXU | Mode::RWXG | Mode::RWXO)
    }
    fn publish_collect(
        self,
        check: &impl Fn() -> io::Result<()>,
        after_mkdir: &mut impl FnMut(&PathBuf) -> io::Result<()>,
        owned: &mut Vec<Directory>,
        creation_mode: Mode,
    ) -> Publication {
        let record = self.frame.record();
        let mut calls = Vec::new();
        let mut observed = Vec::new();
        let result = (|| {
            self.frame.current(check)?;
            let mut current = Directory::open(&record.anchor.path, false)?;
            if current.pin != record.anchor {
                return Err(error("directory creation ancestor identity changed"));
            }
            for path in &record.missing {
                check()?;
                current.current()?;
                self.frame.anchor().current()?;
                let name = path.file_name().ok_or_else(|| error("missing directory leaf unavailable"))?;
                mkdirat(&current.file, name, creation_mode)?;
                calls.push(path.clone());
                current.file.sync_all()?;
                after_mkdir(path)?;
                current.current()?;
                let child = Directory::open(path, false)?;
                if child.pin.identity.device != record.anchor.identity.device {
                    return Err(error("directory effect left its admitted volume"));
                }
                owned.push(Directory { pin: child.pin.clone(), file: child.file.try_clone()? });
                observed.push(child.pin.clone());
                current = child;
            }
            check()?;
            self.frame.anchor().current()?;
            current.current()?;
            if current.pin.path != record.path {
                return Err(error("directory effect did not reach its exact selected path"));
            }
            Ok(())
        })();
        Publication {
            path: record.path,
            successful_calls: calls,
            observed,
            complete: result.is_ok(),
            error: result.err().map(|e: io::Error| e.to_string()),
            write_authority: false,
            custody: false,
        }
    }
}
#[cfg(test)]
#[path = "directory_effect_tests.rs"]
mod tests;

/// One-shot owner of a directory actually created here. No Clone or Deserialize.
pub struct OwnedDirectory {
    directory: Directory,
}
/// Read-only successor to the exact descriptor-relative permission effect.
/// No write authority; arbitrary pins and historical records cannot mint one.
pub struct SealedDirectory {
    before: DirectoryPin,
    directory: Directory,
}
impl OwnedDirectory {
    pub fn pin(&self) -> &DirectoryPin {
        &self.directory.pin
    }
    pub fn current(&self, check: &impl Fn() -> io::Result<()>) -> io::Result<()> {
        check()?;
        self.directory.current()?;
        check()
    }
    /// Create only a new empty private file through the actual created directory
    /// descriptor. Both the opened file and its pinned named observation survive.
    pub fn create_empty_private_file(&self, name: &str, check: &impl Fn() -> io::Result<()>) -> io::Result<(std::fs::File, super::preview_context::NodeFrame)> {
        use super::preview_context::NodeState;
        self.current(check)?;
        let frame = DirectoryFrame::capture(&self.directory.pin.path, check)?;
        if frame.record().anchor != self.directory.pin || !matches!(frame.node_bounded(name, 0, check)?.state(), NodeState::Absent) {
            return Err(error("new private file requires its actual owned directory and absent literal name"));
        }
        let file = std::fs::File::from(openat(
            &self.directory.file,
            name,
            OFlags::RDWR | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::RUSR | Mode::WUSR,
        )?);
        fchmod(&file, Mode::RUSR | Mode::WUSR)?;
        file.sync_all()?;
        self.directory.file.sync_all()?;
        let observed = frame.node_bounded(name, 0, check)?;
        let NodeState::File(state) = observed.state() else {
            return Err(error("created private file disappeared"));
        };
        if state.bytes != 0 || state != &super::stamp(&file.metadata()?, state.sha256.clone()) {
            return Err(error("new empty private file differs from its actual opened descriptor"));
        }
        self.current(check)?;
        observed.current(check)?;
        observed.fence(check)?;
        Ok((file, observed))
    }
    pub fn seal(mut self, mode: u32, check: &impl Fn() -> io::Result<()>) -> io::Result<SealedDirectory> {
        self.current(check)?;
        if mode > 0o7777 {
            return Err(error("directory mode exceeds its permission bits"));
        }
        let before = self.directory.pin.clone();
        let native = Mode::from_bits_truncate(mode.try_into().map_err(|_| error("directory mode exceeds native width"))?);
        fchmod(&self.directory.file, native)?;
        self.directory.file.sync_all()?;
        let mut expected = before.clone();
        expected.identity.mode = (before.identity.mode & !0o7777) | mode;
        self.directory.pin = expected;
        self.directory.current()?;
        check()?;
        Ok(SealedDirectory { before, directory: self.directory })
    }
}
impl SealedDirectory {
    pub fn current(&self, check: &impl Fn() -> io::Result<()>) -> io::Result<()> {
        check()?;
        self.directory.current()?;
        check()
    }
    pub fn pin(&self) -> &DirectoryPin {
        &self.directory.pin
    }
    pub fn continues(&self, before: &DirectoryPin) -> io::Result<bool> {
        self.directory.current()?;
        Ok(before == &self.before)
    }
    pub(crate) fn permits(&self, before: &DirectoryPin, actual: &DirectoryPin) -> io::Result<bool> {
        self.directory.current()?;
        Ok(before == &self.before && actual == &self.directory.pin)
    }
}
/// Retained readers accept an intentional parent-mode transition only through
/// an actual live seal owner; their immutable original intents remain unchanged.
pub(crate) fn matches_sealed(before: &DirectoryPin, actual: &DirectoryPin, seals: &[SealedDirectory]) -> io::Result<bool> {
    for seal in seals {
        seal.directory.current()?;
    }
    if before == actual {
        return Ok(true);
    }
    for seal in seals {
        if seal.permits(before, actual)? {
            return Ok(true);
        }
    }
    Ok(false)
}
