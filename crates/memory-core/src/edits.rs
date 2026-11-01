//! Guarded note relocation. History stays path-scoped; the destination starts its own history.
use anyhow::{ensure, Context, Result};
use cap_std::fs::OpenOptions;
use std::path::Path;

/// Publish the destination without overwriting, then guardedly remove the original.
/// This is deliberately not atomic: on failure both copies may remain, never retry blindly.
pub fn move_markdown(
    root: &Path,
    from: &str,
    to: &str,
    content: &str,
    expected_hash: &str,
) -> Result<()> {
    crate::write::validate_path(from)?;
    crate::write::validate_path(to)?;
    ensure!(from != to, "rename paths must differ");
    let old = crate::read_markdown(root, from)?;
    ensure!(
        crate::content_hash(old.as_bytes()) == expected_hash,
        "rename conflict: source changed"
    );
    crate::write_markdown(root, to, content, crate::WriteMode::Create)?;
    finish_move_markdown(root, from, to, content, expected_hash)
}

/// Resume source removal only after validating both live copies under the source lock.
pub fn finish_move_markdown(
    root: &Path,
    from: &str,
    to: &str,
    content: &str,
    expected_hash: &str,
) -> Result<()> {
    crate::write::validate_path(from)?;
    crate::write::validate_path(to)?;
    ensure!(from != to, "rename paths must differ");
    let directory = crate::write::open_root(root)?;
    if let Ok(metadata) = directory.symlink_metadata(".agent-memory.lock") {
        ensure!(
            !metadata.file_type().is_symlink(),
            "source lock must not be a symlink"
        );
    }
    let lock = directory
        .open_with(
            ".agent-memory.lock",
            OpenOptions::new().read(true).write(true).create(true),
        )?
        .into_std();
    lock.try_lock()
        .context("another memory writer holds the source lock")?;
    let mut parent = directory.try_clone()?;
    let path = Path::new(from);
    for component in path.parent().unwrap_or(Path::new("")).components() {
        parent = crate::history::child_directory(
            &parent,
            component.as_os_str().to_str().context("invalid parent")?,
            false,
        )?
        .context("rename parent disappeared")?;
    }
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .context("invalid source filename")?;
    let metadata = match parent.symlink_metadata(name) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            ensure!(
                crate::history::read_live(&directory, to)?.as_deref() == Some(content.as_bytes()),
                "rename destination changed"
            );
            return Ok(());
        }
        Err(error) => return Err(error.into()),
    };
    ensure!(
        metadata.is_file() && !metadata.file_type().is_symlink(),
        "rename source must be a regular file"
    );
    let bytes = crate::history::read_file(&parent, name, 1024 * 1024)?;
    ensure!(
        crate::content_hash(&bytes) == expected_hash,
        "rename conflict: source changed after destination publication; both copies retained"
    );
    ensure!(
        crate::history::read_live(&directory, to)?.as_deref() == Some(content.as_bytes()),
        "rename destination changed; original retained"
    );
    let history =
        crate::history::History::open(&directory, from, true)?.context("history unavailable")?;
    history.recover(&directory)?;
    history.baseline(&bytes)?;
    parent.remove_file(name)?;
    crate::history::sync_directory(&parent)?;
    Ok(())
}
