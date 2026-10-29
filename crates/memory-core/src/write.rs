use anyhow::{bail, ensure, Context, Result};
use cap_std::{
    ambient_authority,
    fs::{Dir, OpenOptions},
};
use sha2::{Digest, Sha256};
use std::{
    io::{ErrorKind, Write},
    path::{Component, Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

const MAX_CONTENT_BYTES: usize = 1024 * 1024;
static TEMP_ID: AtomicU64 = AtomicU64::new(0);

pub enum WriteMode<'a> {
    Create,
    Update { expected_hash: &'a str },
}

pub(crate) fn validate_path(path: &str) -> Result<()> {
    ensure!(
        !path.is_empty() && path.len() <= 4096,
        "invalid Markdown path"
    );
    for component in path.split('/') {
        ensure!(
            !component.is_empty()
                && !component.starts_with('.')
                && !component.ends_with(['.', ' '])
                && !component
                    .chars()
                    .any(|c| c.is_control() || "\\:<>\"|?*".contains(c)),
            "path must be a relative Markdown path without hidden components or traversal"
        );
        let stem = component.split('.').next().unwrap().to_ascii_uppercase();
        ensure!(
            !["CON", "PRN", "AUX", "NUL"].contains(&stem.as_str())
                && !(stem.len() == 4
                    && (stem.starts_with("COM") || stem.starts_with("LPT"))
                    && matches!(stem.as_bytes()[3], b'1'..=b'9')),
            "reserved file name"
        );
    }
    ensure!(
        path.to_ascii_lowercase().ends_with(".md"),
        "only .md files may be written"
    );
    Ok(())
}

pub(crate) fn open_root(root: &Path) -> Result<Dir> {
    ensure!(
        root.is_absolute(),
        "registered source root must be absolute"
    );
    let mut anchor = PathBuf::new();
    let mut names = Vec::new();
    for component in root.components() {
        match component {
            Component::Prefix(_) | Component::RootDir if names.is_empty() => {
                anchor.push(component.as_os_str());
            }
            Component::Normal(name) => names.push(name),
            _ => bail!("registered source root must be a canonical absolute path"),
        }
    }
    // Ambient authority is used only for the filesystem root / Windows volume.
    // Every untrusted component is then opened relative to a pinned descriptor,
    // atomically refusing symlinks. A canonicalize-then-open check would race.
    let mut directory = Dir::open_ambient_dir(&anchor, ambient_authority())?.into_std_file();
    for name in names {
        directory = cap_primitives::fs::open_dir_nofollow(&directory, Path::new(name))
            .context("open source directory without following symbolic links")?;
    }
    Ok(Dir::from_std_file(directory))
}

fn reject_symlinks(directory: &Dir, path: &str) -> Result<()> {
    let mut prefix = std::path::PathBuf::new();
    for part in path.split('/') {
        prefix.push(part);
        match directory.symlink_metadata(&prefix) {
            Ok(metadata) => ensure!(
                !metadata.file_type().is_symlink(),
                "symbolic links are not allowed"
            ),
            Err(error) if error.kind() == ErrorKind::NotFound => break,
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
}

/// Read a complete UTF-8 document for a subsequent compare-and-update operation.
pub fn read_markdown(root: &Path, path: &str) -> Result<String> {
    validate_path(path)?;
    let directory = open_root(root)?;
    reject_symlinks(&directory, path)?;
    directory
        .read_to_string(path)
        .context("read full Markdown document")
}

struct TemporaryFile<'a> {
    directory: &'a Dir,
    name: String,
}

impl Drop for TemporaryFile<'_> {
    fn drop(&mut self) {
        let _ = self.directory.remove_file(&self.name);
    }
}

/// Atomically publish a Markdown file within the registered directory capability.
/// A source lock serializes cooperating Agent Memory writers, including other DBs.
pub fn write_markdown(
    root: &Path,
    path: &str,
    content: &str,
    mode: WriteMode<'_>,
) -> Result<String> {
    write_inner(root, path, content, mode, || Ok(()), || Ok(()))
}

// Private stage hooks let unit tests interrupt the real protocol without public
// environment variables or weakened production durability.
fn write_inner(
    root: &Path,
    path: &str,
    content: &str,
    mode: WriteMode<'_>,
    before_publish: impl FnOnce() -> Result<()>,
    after_publish: impl FnOnce() -> Result<()>,
) -> Result<String> {
    validate_path(path)?;
    ensure!(
        !content.trim().is_empty() && content.len() <= MAX_CONTENT_BYTES,
        "Markdown content must be nonempty and at most 1 MiB"
    );
    crate::parse_metadata(content).context("validate Markdown metadata before saving")?;
    if let WriteMode::Update { expected_hash } = mode {
        ensure!(
            expected_hash.len() == 64
                && expected_hash
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)),
            "expected_hash must be a lowercase SHA-256"
        );
    }
    let directory = open_root(root)?;
    reject_symlinks(&directory, ".agent-memory.lock")?;
    let lock = directory
        .open_with(
            ".agent-memory.lock",
            OpenOptions::new().read(true).write(true).create(true),
        )?
        .into_std();
    lock.try_lock()
        .context("another memory writer holds the source lock; retry later")?;
    reject_symlinks(&directory, path)?;
    let target = Path::new(path);
    let parent_path = target.parent().unwrap_or(Path::new(""));
    // Pin and synchronize every newly created ancestor, not only the leaf directory.
    let mut parent = directory.try_clone()?;
    for component in parent_path.components() {
        let name = component
            .as_os_str()
            .to_str()
            .context("invalid parent component")?;
        parent = crate::history::child_directory(&parent, name, true)?
            .context("parent directory was not created")?;
    }
    let name = target.file_name().context("missing file name")?;
    let (permissions, previous_bytes) = match &mode {
        WriteMode::Create => {
            match parent.symlink_metadata(name) {
                Ok(_) => bail!("write conflict: document already exists; read it before updating"),
                Err(error) if error.kind() == ErrorKind::NotFound => {}
                Err(error) => return Err(error.into()),
            }
            (None, None)
        }
        WriteMode::Update { expected_hash } => {
            let metadata = parent
                .symlink_metadata(name)
                .context("write conflict: document is missing")?;
            ensure!(
                metadata.is_file() && !metadata.file_type().is_symlink(),
                "target must be a regular Markdown file"
            );
            let bytes = crate::history::read_file(
                &parent,
                name.to_str().context("invalid filename")?,
                MAX_CONTENT_BYTES as u64,
            )?;
            std::str::from_utf8(&bytes).context("existing Markdown is not UTF-8")?;
            ensure!(
                format!("{:x}", Sha256::digest(&bytes)) == *expected_hash,
                "write conflict: document changed; read the full document again before updating"
            );
            (Some(metadata.permissions()), Some(bytes))
        }
    };
    let history = crate::history::History::open(&directory, path, true)?
        .context("history directory was not created")?;
    history.recover(&directory)?;
    if let Some(bytes) = &previous_bytes {
        history.baseline(bytes)?;
        // A no-op has no publication gap and must not create an ambiguous pending
        // record whose target was already present before any write.
        if bytes == content.as_bytes() {
            return Ok(crate::content_hash(bytes));
        }
    }
    let (temporary, mut file) = loop {
        let temp_name = format!(
            ".agent-memory-{}-{}.tmp",
            std::process::id(),
            TEMP_ID.fetch_add(1, Ordering::Relaxed)
        );
        match parent.open_with(&temp_name, OpenOptions::new().write(true).create_new(true)) {
            Ok(file) => {
                break (
                    TemporaryFile {
                        directory: &parent,
                        name: temp_name,
                    },
                    file,
                )
            }
            Err(error) if error.kind() == ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error.into()),
        }
    };
    if let Some(permissions) = permissions {
        file.set_permissions(permissions)?;
    }
    file.write_all(content.as_bytes())?;
    file.sync_all()?;
    drop(file);
    let revision = history.prepare(content.as_bytes())?;
    before_publish()?;
    // Recheck immediately before publication; external editors do not take our advisory lock.
    if let WriteMode::Update { expected_hash } = mode {
        let bytes = crate::history::read_file(
            &parent,
            name.to_str().context("invalid filename")?,
            MAX_CONTENT_BYTES as u64,
        )
        .context("write conflict: document disappeared")?;
        ensure!(
            format!("{:x}", Sha256::digest(bytes)) == expected_hash,
            "write conflict: document changed during write"
        );
        parent.rename(&temporary.name, &parent, name)?;
    } else {
        // Unlike rename, hard_link never replaces an existing destination, even on a race.
        parent
            .hard_link(&temporary.name, &parent, name)
            .context("publish new Markdown without overwriting an existing file")?;
    }
    let hash = crate::content_hash(content.as_bytes());
    let finalized = (|| -> Result<()> {
        after_publish()?;
        crate::history::sync_directory(&parent)?;
        history.finalize(&revision)?;
        Ok(())
    })();
    if let Err(error) = finalized {
        return Err(crate::history::PublishedWriteError {
            content_hash: hash,
            message: format!("{error:#}"),
        }
        .into());
    }
    Ok(hash)
}

#[cfg(test)]
mod revision_interruption_tests {
    use super::*;
    use crate::{content_hash, get_revision, list_revisions, PublishedWriteError};

    fn pending(root: &Path) -> PathBuf {
        root.join(".agent-memory-history")
            .join(content_hash(b"note.md"))
            .join("pending.json")
    }

    #[test]
    fn subprocess_writer() -> Result<()> {
        let Some(root) = std::env::var_os("AGENT_MEMORY_UNIT_HISTORY_ROOT") else {
            return Ok(());
        };
        let stage = std::env::var("AGENT_MEMORY_UNIT_HISTORY_STAGE")?;
        let recreate = stage == "recreate";
        let hash = content_hash(b"# One\n");
        let mode = if recreate {
            WriteMode::Create
        } else {
            WriteMode::Update {
                expected_hash: &hash,
            }
        };
        let content = if recreate { "# One\n" } else { "# Child\n" };
        write_inner(
            Path::new(&root),
            "note.md",
            content,
            mode,
            || {
                if stage == "before" {
                    std::process::exit(73);
                }
                Ok(())
            },
            || {
                std::process::exit(73);
            },
        )?;
        unreachable!("test child always exits abruptly")
    }

    #[test]
    fn abrupt_process_exit_recovers_before_after_and_same_content_recreation() -> Result<()> {
        for stage in ["before", "after", "recreate"] {
            let source = tempfile::tempdir()?;
            if stage == "before" {
                // No prior journal: interruption must still preserve the old external text.
                std::fs::write(source.path().join("note.md"), "# One\n")?;
            } else {
                write_markdown(source.path(), "note.md", "# One\n", WriteMode::Create)?;
            }
            if stage == "recreate" {
                std::fs::remove_file(source.path().join("note.md"))?;
            }
            let child = std::process::Command::new(std::env::current_exe()?)
                .args([
                    "--exact",
                    "write::revision_interruption_tests::subprocess_writer",
                    "--nocapture",
                ])
                .env("AGENT_MEMORY_UNIT_HISTORY_ROOT", source.path())
                .env("AGENT_MEMORY_UNIT_HISTORY_STAGE", stage)
                .output()?;
            ensure!(
                child.status.code() == Some(73),
                "unexpected child status: {:?}; stderr={}",
                child.status,
                String::from_utf8_lossy(&child.stderr)
            );
            let expected = if stage == "after" {
                "# Child\n"
            } else {
                "# One\n"
            };
            assert_eq!(
                std::fs::read_to_string(source.path().join("note.md"))?,
                expected
            );
            let history = list_revisions(source.path(), "note.md")?;
            assert_eq!(history.len(), if stage == "before" { 1 } else { 2 });
            assert_eq!(
                get_revision(source.path(), "note.md", &history[0].id)?
                    .unwrap()
                    .content,
                "# One\n"
            );
            let current = content_hash(expected.as_bytes());
            write_markdown(
                source.path(),
                "note.md",
                expected,
                WriteMode::Update {
                    expected_hash: &current,
                },
            )?;
            assert_eq!(
                list_revisions(source.path(), "note.md")?.len(),
                history.len()
            );
            assert!(!pending(source.path()).exists());
        }
        Ok(())
    }

    #[test]
    fn interruption_before_publication_never_exposes_the_prepared_target() -> Result<()> {
        let source = tempfile::tempdir()?;
        let first = write_markdown(source.path(), "note.md", "# One\n", WriteMode::Create)?;
        let error = write_inner(
            source.path(),
            "note.md",
            "# Abandoned\n",
            WriteMode::Update {
                expected_hash: &first,
            },
            || anyhow::bail!("simulated interruption before publication"),
            || Ok(()),
        )
        .unwrap_err();
        assert!(error.downcast_ref::<PublishedWriteError>().is_none());
        assert!(pending(source.path()).exists());
        let history = list_revisions(source.path(), "note.md")?;
        assert_eq!(history.len(), 1);
        assert_eq!(
            std::fs::read_to_string(source.path().join("note.md"))?,
            "# One\n"
        );
        write_markdown(
            source.path(),
            "note.md",
            "# Three\n",
            WriteMode::Update {
                expected_hash: &first,
            },
        )?;
        let history = list_revisions(source.path(), "note.md")?;
        assert_eq!(history.len(), 2);
        assert_eq!(
            get_revision(source.path(), "note.md", &history[1].id)?
                .unwrap()
                .content,
            "# Three\n"
        );
        assert!(!pending(source.path()).exists());
        Ok(())
    }

    #[test]
    fn external_edit_after_interruption_preserves_unconfirmed_snapshot_for_review() -> Result<()> {
        let source = tempfile::tempdir()?;
        let first = write_markdown(source.path(), "note.md", "# One\n", WriteMode::Create)?;
        let error = write_inner(
            source.path(),
            "note.md",
            "# Published two\n",
            WriteMode::Update {
                expected_hash: &first,
            },
            || Ok(()),
            || anyhow::bail!("interrupted after publication"),
        )
        .unwrap_err();
        assert!(error.downcast_ref::<PublishedWriteError>().is_some());
        let prepared: serde_json::Value =
            serde_json::from_slice(&std::fs::read(pending(source.path()))?)?;
        std::fs::write(source.path().join("note.md"), "# External three\n")?;
        // The historical publication cannot now be proven from live bytes.
        assert_eq!(list_revisions(source.path(), "note.md")?.len(), 1);
        write_markdown(
            source.path(),
            "note.md",
            "# Four\n",
            WriteMode::Update {
                expected_hash: &content_hash(b"# External three\n"),
            },
        )?;
        let history = list_revisions(source.path(), "note.md")?;
        assert_eq!(history.len(), 3);
        let directory = pending(source.path()).parent().unwrap().to_owned();
        let candidate = directory.join("unconfirmed").join(format!(
            "{}.json",
            prepared["revision"]["id"].as_str().unwrap()
        ));
        assert!(candidate.is_file());
        let candidate: serde_json::Value = serde_json::from_slice(&std::fs::read(candidate)?)?;
        let hash = candidate["revision"]["content_hash"].as_str().unwrap();
        assert_eq!(
            std::fs::read_to_string(directory.join(format!("{hash}.blob")))?,
            "# Published two\n"
        );
        assert!(!history.iter().any(|revision| revision.content_hash == hash));
        Ok(())
    }

    #[test]
    fn interruption_after_publication_is_readable_without_mutating_pending_state() -> Result<()> {
        let source = tempfile::tempdir()?;
        let first = write_markdown(source.path(), "note.md", "# One\n", WriteMode::Create)?;
        let error = write_inner(
            source.path(),
            "note.md",
            "# Two\n",
            WriteMode::Update {
                expected_hash: &first,
            },
            || Ok(()),
            || anyhow::bail!("simulated interruption after publication"),
        )
        .unwrap_err();
        let saved = error.downcast_ref::<PublishedWriteError>().unwrap();
        assert_eq!(saved.content_hash, content_hash(b"# Two\n"));
        let original_pending = std::fs::read(pending(source.path()))?;
        let history = list_revisions(source.path(), "note.md")?;
        assert_eq!(history.len(), 2);
        assert!(!history[0].pending);
        assert!(history[1].pending);
        assert!(
            get_revision(source.path(), "note.md", &history[1].id)?
                .unwrap()
                .revision
                .pending
        );
        assert_eq!(
            get_revision(source.path(), "note.md", &history[1].id)?
                .unwrap()
                .content,
            "# Two\n"
        );
        assert_eq!(std::fs::read(pending(source.path()))?, original_pending);
        // Next cooperating write physically finalizes the pending record. A no-op
        // must retain exactly the two real content versions.
        write_markdown(
            source.path(),
            "note.md",
            "# Two\n",
            WriteMode::Update {
                expected_hash: &saved.content_hash,
            },
        )?;
        let finalized = list_revisions(source.path(), "note.md")?;
        assert_eq!(finalized.len(), 2);
        assert!(finalized.iter().all(|revision| !revision.pending));
        assert!(!pending(source.path()).exists());
        Ok(())
    }
}
