//! Durable observations of Git synchronization, independent of the search index.
use anyhow::{ensure, Context, Result};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    io::Write,
    path::Path,
    time::{SystemTime, UNIX_EPOCH},
};

const FILE_NAME: &str = "agent-memory-sync-status.json";

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SyncStatus {
    /// The last observed attempt, not proof that a scheduler is currently alive.
    pub status: String,
    /// Unix milliseconds when the last attempt started.
    pub last_attempt_at: u64,
    /// Unix milliseconds when the last successful push completed.
    pub last_success_at: Option<u64>,
    /// Fixed diagnostic text; never stores raw Git output or remote credentials.
    pub last_error: Option<String>,
}

fn now() -> Result<u64> {
    Ok(u64::try_from(
        SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis(),
    )?)
}

fn read_directory(git_dir: &Path) -> Result<Option<SyncStatus>> {
    let path = git_dir.join(FILE_NAME);
    let metadata = match fs::symlink_metadata(&path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error).context("inspect Git sync status"),
    };
    ensure!(
        metadata.is_file() && !metadata.file_type().is_symlink(),
        "Git sync status must be a regular file, not a symlink"
    );
    ensure!(metadata.len() <= 64 * 1024, "Git sync status is too large");
    let status: SyncStatus =
        serde_json::from_slice(&fs::read(&path)?).context("invalid Git sync status JSON")?;
    ensure!(
        matches!(status.status.as_str(), "running" | "ok" | "error"),
        "invalid Git sync status state"
    );
    Ok(Some(status))
}

/// Read existing metadata only. A plain Markdown source or never-synchronized
/// repository returns None; a nested source is not silently treated as its parent.
pub fn read(root: &Path) -> Result<Option<SyncStatus>> {
    let Some(git_dir) = crate::git_sync::status_directory(root)? else {
        return Ok(None);
    };
    read_directory(&git_dir)
}

fn write(git_dir: &Path, status: &SyncStatus) -> Result<()> {
    let path = git_dir.join(FILE_NAME);
    if let Ok(metadata) = fs::symlink_metadata(&path) {
        ensure!(
            metadata.is_file() && !metadata.file_type().is_symlink(),
            "Git sync status must be a regular file, not a symlink"
        );
    }
    let mut pending = tempfile::NamedTempFile::new_in(git_dir)?;
    serde_json::to_writer_pretty(&mut pending, status)?;
    pending.write_all(b"\n")?;
    pending.as_file().sync_all()?;
    pending
        .persist(path)
        .map_err(|error| error.error)
        .context("publish Git sync status")?;
    Ok(())
}

/// Called only while holding the repository's synchronization lock.
pub(crate) fn start(git_dir: &Path) -> Result<SyncStatus> {
    let previous = read_directory(git_dir)?;
    let status = SyncStatus {
        status: "running".to_owned(),
        last_attempt_at: now()?,
        last_success_at: previous.and_then(|status| status.last_success_at),
        last_error: None,
    };
    write(git_dir, &status)?;
    Ok(status)
}

/// Called under the same repository lock as start and the Git operation itself.
pub(crate) fn finish(git_dir: &Path, mut status: SyncStatus, result: &Result<()>) -> Result<()> {
    match result {
        Ok(()) => {
            status.status = "ok".to_owned();
            status.last_success_at = Some(now()?);
            status.last_error = None;
        }
        Err(error) => {
            status.status = "error".to_owned();
            status.last_error = Some(safe_error(error));
        }
    }
    write(git_dir, &status)
}

fn safe_error(error: &anyhow::Error) -> String {
    let message = format!("{error:#}");
    let diagnostic = if message.contains("Git push failed") {
        "Git push failed; local commits are retained"
    } else if message.contains("existing staged changes") {
        "Git sync refused existing staged changes"
    } else if message.contains("upstream branch") {
        "Git sync requires an existing upstream branch"
    } else if message.contains("attached branch") {
        "Git sync requires an attached branch"
    } else if message.contains("conflict") || message.contains("in-progress") {
        "Git sync refused a conflict or in-progress repository operation"
    } else if message.contains("timed out") {
        "Git command timed out"
    } else {
        "Git synchronization failed; inspect local MCP diagnostics"
    };
    diagnostic.to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn successful_push_survives_a_later_failed_attempt() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let status = start(temp.path())?;
        assert_eq!(read_directory(temp.path())?.unwrap().status, "running");
        finish(temp.path(), status, &Ok(()))?;
        let successful = read_directory(temp.path())?.unwrap();
        assert_eq!(successful.status, "ok");
        let status = start(temp.path())?;
        finish(
            temp.path(),
            status,
            &Err(anyhow::anyhow!(
                "Git push failed https://user:secret@example.invalid/repo"
            )),
        )?;
        let failed = read_directory(temp.path())?.unwrap();
        assert_eq!(failed.status, "error");
        assert_eq!(failed.last_success_at, successful.last_success_at);
        assert!(!failed.last_error.unwrap().contains("secret"));
        Ok(())
    }

    #[test]
    fn status_read_does_not_create_files_and_rejects_invalid_or_symlink_files() -> Result<()> {
        let temp = tempfile::tempdir()?;
        assert!(read_directory(temp.path())?.is_none());
        assert_eq!(fs::read_dir(temp.path())?.count(), 0);
        let path = temp.path().join(FILE_NAME);
        fs::write(&path, "bad json")?;
        assert!(read_directory(temp.path()).is_err());
        assert!(start(temp.path()).is_err());
        #[cfg(unix)]
        {
            let outside = temp.path().join("outside.json");
            fs::write(&outside, "outside")?;
            fs::remove_file(&path)?;
            std::os::unix::fs::symlink(&outside, &path)?;
            assert!(read_directory(temp.path()).is_err());
            assert!(start(temp.path()).is_err());
            assert_eq!(fs::read_to_string(outside)?, "outside");
        }
        Ok(())
    }
}
