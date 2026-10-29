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

fn validate_path(path: &str) -> Result<()> {
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
    validate_path(path)?;
    ensure!(
        !content.trim().is_empty() && content.len() <= MAX_CONTENT_BYTES,
        "Markdown content must be nonempty and at most 1 MiB"
    );
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
    if !parent_path.as_os_str().is_empty() {
        directory.create_dir_all(parent_path)?;
    }
    // Keep a handle to the parent so a replaced path component cannot redirect publication.
    let parent = if parent_path.as_os_str().is_empty() {
        directory.try_clone()?
    } else {
        directory.open_dir(parent_path)?
    };
    let name = target.file_name().context("missing file name")?;
    let permissions = match &mode {
        WriteMode::Create => {
            match parent.symlink_metadata(name) {
                Ok(_) => bail!("write conflict: document already exists; read it before updating"),
                Err(error) if error.kind() == ErrorKind::NotFound => {}
                Err(error) => return Err(error.into()),
            }
            None
        }
        WriteMode::Update { expected_hash } => {
            let metadata = parent
                .symlink_metadata(name)
                .context("write conflict: document is missing")?;
            ensure!(
                metadata.is_file() && !metadata.file_type().is_symlink(),
                "target must be a regular Markdown file"
            );
            let bytes = parent.read(name)?;
            ensure!(
                format!("{:x}", Sha256::digest(bytes)) == *expected_hash,
                "write conflict: document changed; read the full document again before updating"
            );
            Some(metadata.permissions())
        }
    };
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
    // Recheck immediately before publication; external editors do not take our advisory lock.
    if let WriteMode::Update { expected_hash } = mode {
        let bytes = parent
            .read(name)
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
    Ok(format!("{:x}", Sha256::digest(content.as_bytes())))
}
