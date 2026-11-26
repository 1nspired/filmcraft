//! Actual native authored resources: original admitted frames, original copy
//! and directory publishers, exact namespace and one-shot permission successors.
//! No historical intent or host descriptor becomes a publication owner.
use super::*;
use filmcraft_format::retained::{
    directory_effect::{self, OwnedDirectory, SealedDirectory},
    relocation::copying,
};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
const FILE_BYTES: u64 = 64 * 1024 * 1024;
const TOTAL_BYTES: u64 = 256 * 1024 * 1024;
struct Entry {
    relative: PathBuf,
    source: NodeFrame,
    mode: u32,
    directory: bool,
}
pub(super) struct Plan {
    entries: Vec<Entry>,
    copies: Vec<copying::Publication>,
    seals: Arc<Vec<SealedDirectory>>,
    attempted: bool,
    complete: bool,
}
fn fail(e: impl std::fmt::Display) -> crate::EngineError {
    bad(ACTIVATE, e.to_string())
}
fn same(a: &NodeState, b: &NodeState) -> bool {
    match (a, b) {
        (NodeState::File(a), NodeState::File(b)) => a.bytes == b.bytes && a.sha256 == b.sha256 && a.identity.mode == b.identity.mode,
        (NodeState::Directory(a), NodeState::Directory(b)) => a.identity.mode == b.identity.mode,
        _ => false,
    }
}
fn operation(token: &str, relative: &Path) -> Result<String> {
    let relative = relative.to_str().ok_or_else(|| fail("literal authored relative name required"))?;
    Ok(format!("{:x}", Sha256::digest(format!("{token}:native-authored:{relative}").as_bytes())))
}
fn frame(path: &Path, check: &impl Fn() -> io::Result<()>) -> Result<NodeFrame> {
    DirectoryFrame::capture(path.parent().ok_or_else(|| fail("authored parent missing"))?, check)
        .map_err(fail)?
        .node(path.file_name().and_then(|v| v.to_str()).ok_or_else(|| fail("literal authored filename required"))?, check)
        .map_err(fail)
}
fn valid_relative(path: &Path, directory: bool) -> bool {
    let components = path.components().count();
    // The original library walker starts its family directory at depth zero.
    // A file inside a depth-16 directory therefore has 18 relative components.
    components > 0 && components <= if directory { 17 } else { 18 } && path.components().all(|c| matches!(c, std::path::Component::Normal(_)))
}
impl Plan {
    pub(super) fn prepare(input: &restore::Input, root: &DirectoryFrame, check: &media_guard::Check) -> Result<Self> {
        input.current(check)?;
        let io = || check.check().map_err(|e| io::Error::other(e.to_string()));
        let mut selected = BTreeMap::<PathBuf, (PathBuf, NodeState)>::new();
        // Roots are exactly those selected by the original resource enumerator
        // plus the actual paused producer directory, not arbitrary historical paths.
        // Conflicting authored roles refuse before effects; no silent precedence.
        for (relative, source, state) in input.authored_members()? {
            check.check()?;
            if !valid_relative(&relative, matches!(state, NodeState::Directory(_))) {
                return Err(fail("authored depth or relative path exceeds its original bound"));
            }
            if let Some((_, prior)) = selected.get(&relative) {
                if !same(prior, &state) {
                    return Err(fail(format!("conflicting original authored resource: {}", relative.display())));
                }
            } else {
                selected.insert(relative, (source, state));
            }
        }
        let mut entries = Vec::new();
        let mut bytes = 0u64;
        for (relative, (source, state)) in selected {
            let source = frame(&source, &io)?;
            if source.state() != &state || !input.matches_source(&source) {
                return Err(fail("authored source differs from its actual admitted generation"));
            }
            let (mode, directory) = match &state {
                NodeState::File(v) if v.bytes > 0 && v.bytes <= FILE_BYTES => {
                    bytes = bytes.checked_add(v.bytes).filter(|v| *v <= TOTAL_BYTES).ok_or_else(|| fail("authored aggregate byte bound exceeded"))?;
                    (v.identity.mode & 0o7777, false)
                }
                NodeState::Directory(v) => (v.identity.mode & 0o7777, true),
                _ => return Err(fail("authored resource must be a bounded complete regular file or directory")),
            };
            if relative.components().count() == 1 && !directory {
                return Err(fail("authored directory family has a non-directory root"));
            }
            if !matches!(frame(&root.record().path.join(&relative), &io)?.state(), NodeState::Absent) {
                return Err(fail("fresh authored target is not absent"));
            }
            entries.push(Entry { relative, source, mode, directory });
        }
        entries.sort_by(|a, b| a.relative.components().count().cmp(&b.relative.components().count()).then(a.relative.cmp(&b.relative)));
        for entry in &entries {
            if let Some(parent) = entry.relative.parent().filter(|p| !p.as_os_str().is_empty()) {
                if !entries.iter().any(|e| e.directory && e.relative == parent) {
                    return Err(fail("authored resource lacks its admitted directory parent"));
                }
            }
        }
        input.current(check)?;
        Ok(Self { entries, copies: Vec::new(), seals: Arc::new(Vec::new()), attempted: false, complete: false })
    }
    pub(super) fn namespace(&self, token: &str) -> Result<BTreeMap<PathBuf, BTreeSet<String>>> {
        let mut result = BTreeMap::<PathBuf, BTreeSet<String>>::new();
        result.entry(PathBuf::new()).or_default();
        for entry in &self.entries {
            let name = entry.relative.file_name().and_then(|v| v.to_str()).ok_or_else(|| fail("literal authored member required"))?.to_owned();
            let parent = entry.relative.parent().ok_or_else(|| fail("authored parent required"))?.to_path_buf();
            result.entry(parent.clone()).or_default().insert(name);
            if entry.directory {
                result.entry(entry.relative.clone()).or_default();
            } else {
                result.entry(parent).or_default().insert(format!(".poema-copy-{}.incomplete", operation(token, &entry.relative)?));
            }
        }
        for relative in ["Keyboard Shortcuts/active.json", "effect-presets.json", "export-presets.json", "preferences.json"] {
            let path = Path::new(relative);
            let parent = path.parent().ok_or_else(|| fail("canonical parent required"))?.to_path_buf();
            result
                .entry(parent.clone())
                .or_default()
                .insert(path.file_name().and_then(|v| v.to_str()).ok_or_else(|| fail("canonical filename required"))?.to_owned());
            let operation = format!("{:x}", Sha256::digest(format!("{token}:native-library:{relative}").as_bytes()));
            result.entry(parent).or_default().insert(format!(".poema-save-{operation}.incomplete"));
        }
        result.entry(PathBuf::new()).or_default().insert("Keyboard Shortcuts".into());
        Ok(result)
    }
    fn sources_current(&self, check: &impl Fn() -> io::Result<()>) -> Result<()> {
        for e in &self.entries {
            e.source.current(check).map_err(fail)?;
            e.source.fence(check).map_err(fail)?;
        }
        Ok(())
    }
    pub(super) fn publish(
        &mut self,
        key: &activation::FreshDirectory,
        shortcut: Option<OwnedDirectory>,
        token: &str,
        check: &media_guard::Check,
    ) -> Result<Arc<Vec<SealedDirectory>>> {
        if self.attempted {
            return Err(fail("authored publication already attempted; retain effects without replay"));
        }
        self.attempted = true;
        key.io_current(check)?;
        let io = || key.resource_io_current(check).map_err(|e| io::Error::other(e.to_string()));
        self.sources_current(&io)?;
        let root = key.root().record().path;
        let mut owners = Vec::new();
        let mut shortcut = shortcut;
        for entry in self.entries.iter().filter(|e| e.directory) {
            key.io_current(check)?;
            self.sources_current(&io)?;
            let target = root.join(&entry.relative);
            let owner = if entry.relative == Path::new("Keyboard Shortcuts") {
                let owner = shortcut.take().ok_or_else(|| fail("actual original shortcut-directory publisher required"))?;
                if owner.pin().path != target {
                    return Err(fail("shortcut-directory owner crossed its configured root"));
                }
                owner
            } else {
                directory_effect::stage(DirectoryFrame::capture(&target, &io).map_err(fail)?, &io).map_err(fail)?.publish_owned(&io).map_err(fail)?
            };
            owner.current(&io).map_err(fail)?;
            owners.push((entry.mode, owner));
            key.io_current(check)?;
        }
        // A source without a shortcut directory still receives the canonical
        // native directory, with its original private mode.
        if let Some(owner) = shortcut {
            owners.push((0o700, owner));
        }
        for entry in self.entries.iter().filter(|e| !e.directory) {
            key.io_current(check)?;
            self.sources_current(&io)?;
            for (_, owner) in &owners {
                owner.current(&io).map_err(fail)?;
            }
            let staged = copying::stage(&entry.source.path(), &root.join(&entry.relative), &operation(token, &entry.relative)?, &io).map_err(fail)?;
            key.io_current(check)?;
            let publication = staged.publish(&io);
            // Preserve a real uncertain receipt before returning; never replay.
            self.copies.push(publication);
            self.copies.last().ok_or_else(|| fail("actual authored copy owner missing"))?.verify_current(&io).map_err(fail)?;
            #[cfg(test)]
            key.after_authored_effect(1)?;
            key.io_current(check)?;
        }
        self.sources_current(&io)?;
        for copy in &self.copies {
            copy.verify_current(&io).map_err(fail)?;
        }
        let mut seals = Vec::new();
        while let Some((mode, owner)) = owners.pop() {
            key.io_current(check)?;
            seals.push(owner.seal(mode, &io).map_err(fail)?);
            #[cfg(test)]
            key.after_authored_effect(2)?;
            key.io_current(check)?;
        }
        self.seals = Arc::new(seals);
        self.complete = true;
        self.verify(key, check)?;
        Ok(self.seals.clone())
    }
    pub(super) fn verify(&self, key: &activation::FreshDirectory, check: &media_guard::Check) -> Result<()> {
        if !self.complete {
            return Err(fail("complete actual authored publication required"));
        }
        key.io_current(check)?;
        let io = || key.resource_io_current(check).map_err(|e| io::Error::other(e.to_string()));
        self.sources_current(&io)?;
        for seal in self.seals.iter() {
            seal.current(&io).map_err(fail)?;
        }
        for copy in &self.copies {
            copy.verify_current_after_directory_seals(&self.seals, &io).map_err(fail)?;
        }
        key.io_current(check)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn original_authored_depth_bound_includes_file_at_depth_sixteen() {
        let mut dir = PathBuf::from("Graphics Templates");
        for n in 0..16 {
            dir.push(format!("level-{n}"));
        }
        assert!(valid_relative(&dir, true));
        assert!(valid_relative(&dir.join("current.json"), false));
        assert!(!valid_relative(&dir.join("extra"), true));
        assert!(!valid_relative(&dir.join("extra/current.json"), false));
        assert!(!valid_relative(Path::new("../escape"), false));
        assert!(!valid_relative(Path::new("/escape"), true));
        assert!(!valid_relative(Path::new(""), true));
    }
}
