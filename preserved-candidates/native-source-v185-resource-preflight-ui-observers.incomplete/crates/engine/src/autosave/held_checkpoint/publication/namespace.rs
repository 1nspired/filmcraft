//! Bounded metadata only; a matching partial namespace never grants custody.
use super::*;
use std::{collections::BTreeSet, fs, os::unix::fs::MetadataExt};
fn error(message: &str) -> io::Error {
    io::Error::other(message)
}
fn directory(path: &Path, data: &retained::Identity, check: &impl Fn() -> io::Result<()>) -> io::Result<bool> {
    check()?;
    let meta = match fs::symlink_metadata(path) {
        Ok(v) => v,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(e) => return Err(e),
    };
    if !meta.is_dir() || meta.mode() & 0o7777 != 0o700 || meta.uid() != data.uid || meta.dev().to_string() != data.device || fs::canonicalize(path)? != path {
        return Err(error("checkpoint namespace directory changed type, mode, owner or volume"));
    }
    check()?;
    Ok(true)
}
fn names(path: &Path, allowed: &BTreeSet<String>, check: &impl Fn() -> io::Result<()>) -> io::Result<Vec<String>> {
    check()?;
    let mut result = Vec::new();
    for entry in fs::read_dir(path)? {
        check()?;
        if result.len() >= allowed.len() {
            return Err(error("surplus checkpoint archive entry"));
        }
        let name = entry?.file_name().into_string().map_err(|_| error("literal checkpoint entry required"))?;
        if !allowed.contains(&name) {
            return Err(error("unknown checkpoint archive entry"));
        }
        result.push(name);
    }
    check()?;
    Ok(result)
}
fn file(path: &Path, max: u64, data: &retained::Identity, check: &impl Fn() -> io::Result<()>) -> io::Result<()> {
    check()?;
    let meta = match fs::symlink_metadata(path) {
        Ok(v) => v,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e),
    };
    if !meta.is_file()
        || meta.mode() & 0o7777 != 0o600
        || meta.nlink() != 1
        || meta.len() > max
        || meta.uid() != data.uid
        || meta.dev().to_string() != data.device
    {
        return Err(error("checkpoint namespace file exceeds its exact private metadata bound"));
    }
    check()
}
pub(super) fn current(data: &Arc<DirectoryFrame>, root: &Path, epoch: &str, lengths: &[u64; 3], check: &impl Fn() -> io::Result<()>) -> io::Result<()> {
    data.current(check)?;
    let identity = data.record().anchor.identity;
    if !directory(root, &identity, check)? {
        return Ok(());
    }
    let children = names(root, &BTreeSet::from(["files".into(), "writes".into()]), check)?;
    if children.iter().any(|n| n == "files") {
        let parent = root.join("files");
        if !directory(&parent, &identity, check)? {
            return Err(error("checkpoint files directory disappeared"));
        }
        let allowed = BTreeSet::from([super::super::super::SNAPSHOT.into(), super::super::super::META.into(), crate::agent_bridge::NATIVE_JOURNAL_FILE.into()]);
        for name in names(&parent, &allowed, check)? {
            let max = match name.as_str() {
                super::super::super::SNAPSHOT => lengths.first(),
                super::super::super::META => lengths.get(1),
                _ => lengths.get(2),
            }
            .copied()
            .ok_or_else(|| error("checkpoint length missing"))?;
            file(&parent.join(name), max, &identity, check)?;
        }
    }
    if children.iter().any(|n| n == "writes") {
        let parent = root.join("writes");
        if !directory(&parent, &identity, check)? {
            return Err(error("checkpoint writes directory disappeared"));
        }
        let stages = ["snapshot", "metadata", "native"].map(|s| format!(".poema-save-{epoch}_{s}.incomplete"));
        let allowed = stages.iter().cloned().collect();
        for name in names(&parent, &allowed, check)? {
            let stage = parent.join(&name);
            if !directory(&stage, &identity, check)? {
                return Err(error("checkpoint stage disappeared"));
            }
            let index = stages.iter().position(|v| v == &name).ok_or_else(|| error("original checkpoint stage missing"))?;
            let size = *lengths.get(index).ok_or_else(|| error("checkpoint length missing"))?;
            for child in names(
                &stage,
                &BTreeSet::from(["payload.fcproj".into(), "exchange-slot.fcproj".into(), "intent.json".into(), "dispatch.json".into(), "receipt.json".into()]),
                check,
            )? {
                let max = if child == "payload.fcproj" || child == "exchange-slot.fcproj" { size } else { 65536 };
                file(&stage.join(child), max, &identity, check)?;
            }
        }
    }
    data.current(check)?;
    check()
}
