//! Archive descriptions come only from the already-captured native frames.
use super::*;
fn add(nodes: &mut BTreeMap<PathBuf, NodeState>, path: PathBuf, state: NodeState, check: &media_guard::Check) -> Result<()> {
    check.check()?;
    if let Some(prior) = nodes.get(&path) {
        if prior != &state {
            return Err(bad(COMMAND, "captured archive frames disagree for one native path"));
        }
        return Ok(());
    }
    if nodes.len() >= MAX_NODES + 1 {
        return Err(bad(COMMAND, "captured archive node bound exceeded"));
    }
    nodes.insert(path, state);
    Ok(())
}
pub(super) fn read(files: &Files, check: &media_guard::Check) -> Result<Value> {
    let mut nodes = BTreeMap::new();
    add(&mut nodes, files.saved.path(), files.saved.state().clone(), check)?;
    for node in files.nodes.iter().chain(files.source.archive_nodes()) {
        add(&mut nodes, node.path(), node.state().clone(), check)?;
    }
    for frame in files.directories.iter().map(|d| &d.frame).chain(files.source.archive_directories()) {
        let record = frame.record();
        let state = if frame.exists() { NodeState::Directory(record.anchor) } else { NodeState::Absent };
        add(&mut nodes, record.path, state, check)?;
    }
    let mut regular = Vec::new();
    let mut directories = Vec::new();
    let mut absent = Vec::new();
    for (path, state) in nodes {
        check.check()?;
        match state {
            NodeState::File(v) => regular.push(json!({"path":path,"sha256":v.sha256,"bytes":v.bytes.to_string(),"mode":v.identity.mode & 0o777})),
            NodeState::Directory(v) => directories.push(json!({"path":path,"mode":v.identity.mode & 0o777})),
            NodeState::Absent => absent.push(path),
        }
    }
    let value = json!({"kind":"native_paused_current_resource_inventory","version":1,"files":regular,"directories":directories,"absent":absent});
    history_checkpoint::bytes_checked(&value, MAX_META, Some(check))?;
    Ok(value)
}
