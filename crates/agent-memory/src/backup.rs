//! Owner-only, portable backups of authored memory and its durable journals.
use anyhow::{bail, ensure, Context, Result};
use memory_sqlite::config::Config;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

const MAGIC: &[u8] = b"AGENT-MEMORY-BACKUP\n1\n";
const MAX_MANIFEST: u64 = 16 * 1024 * 1024;
const MAX_FILE: u64 = 64 * 1024 * 1024;
const MAX_TOTAL: u64 = 8 * 1024 * 1024 * 1024;
const MAX_FILES: usize = 100_000;
const MAX_SOURCES: usize = 1024;
const CONFIG_FILE: &str = "agent-memory.config.json";

#[derive(Debug, Serialize)]
pub struct Report {
    pub format_version: u32,
    pub sources: usize,
    pub files: usize,
    pub bytes: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub destination: Option<PathBuf>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub config_path: Option<PathBuf>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    version: u32,
    sources: usize,
    config: Config,
    entries: Vec<Entry>,
}

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Entry {
    source: usize,
    path: String,
    size: u64,
    sha256: String,
}

fn source_path(index: usize) -> PathBuf {
    PathBuf::from(format!("sources/{index}"))
}

fn path_parts(path: &str) -> Result<Vec<&str>> {
    ensure!(
        !path.is_empty() && path.len() <= 4096,
        "invalid backup path"
    );
    let parts: Vec<_> = path.split('/').collect();
    ensure!(parts.len() <= 128, "backup path is too deeply nested");
    for part in &parts {
        ensure!(
            !part.is_empty()
                && *part != "."
                && *part != ".."
                && !part.ends_with(['.', ' '])
                && !part
                    .chars()
                    .any(|c| c.is_control() || "\\:<>\"|?*".contains(c)),
            "backup paths must be portable relative paths without traversal"
        );
        let stem = part.split('.').next().unwrap().to_ascii_uppercase();
        ensure!(
            !["CON", "PRN", "AUX", "NUL"].contains(&stem.as_str())
                && !(stem.len() == 4
                    && (stem.starts_with("COM") || stem.starts_with("LPT"))
                    && matches!(stem.as_bytes()[3], b'1'..=b'9')),
            "reserved backup filename"
        );
        ensure!(
            !part.eq_ignore_ascii_case(".git") && *part != ".agent-memory.lock",
            "Git metadata and live locks are excluded from backups"
        );
    }
    Ok(parts)
}

fn selected(path: &str) -> bool {
    let first = path.split('/').next().unwrap_or("");
    matches!(first, ".agent-memory-history" | ".agent-memory-operations")
        || path.to_ascii_lowercase().ends_with(".md")
}

fn validate(manifest: &Manifest) -> Result<u64> {
    use std::collections::{HashMap, HashSet};
    ensure!(
        manifest.version == 1 && manifest.config.version == 1,
        "unsupported backup version"
    );
    ensure!(
        manifest.sources <= MAX_SOURCES && manifest.entries.len() <= MAX_FILES,
        "backup has too many entries"
    );
    let mut projects = HashSet::new();
    let mut referenced = HashSet::new();
    for project in &manifest.config.projects {
        memory_core::validate_document_id(&project.name)?;
        ensure!(projects.insert(&project.name), "duplicate backup project");
        let mut roots = HashSet::new();
        for source in &project.sources {
            ensure!(
                source.can_read || !source.can_write,
                "write permission requires read permission"
            );
            let index: usize = source
                .path
                .to_str()
                .and_then(|path| path.strip_prefix("sources/"))
                .context("backup configuration refers to an unknown source")?
                .parse()
                .context("invalid backup source identifier")?;
            ensure!(
                index < manifest.sources && source.path == source_path(index),
                "backup configuration refers to an unknown source"
            );
            ensure!(roots.insert(index), "duplicate project source");
            referenced.insert(index);
        }
    }
    ensure!(
        referenced.len() == manifest.sources,
        "unreferenced backup source"
    );
    let mut files = HashSet::new();
    let mut directories: HashMap<String, String> = HashMap::new();
    let mut total = 0_u64;
    for entry in &manifest.entries {
        ensure!(
            entry.source < manifest.sources && selected(&entry.path),
            "invalid backup file scope"
        );
        let parts = path_parts(&entry.path)?;
        ensure!(
            !matches!(
                parts[0],
                ".agent-memory-history" | ".agent-memory-operations"
            ) || parts.len() > 1,
            "backup journal root must be a directory"
        );
        ensure!(entry.size <= MAX_FILE, "backup file exceeds 64 MiB limit");
        ensure!(
            entry.sha256.len() == 64
                && entry
                    .sha256
                    .bytes()
                    .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c)),
            "invalid backup checksum"
        );
        let key = format!("{}/{}", entry.source, entry.path.to_lowercase());
        ensure!(
            files.insert(key.clone()),
            "duplicate or case-colliding backup file"
        );
        ensure!(
            !directories.contains_key(&key),
            "backup file conflicts with a directory"
        );
        for end in 1..parts.len() {
            let exact = format!("{}/{}", entry.source, parts[..end].join("/"));
            let lower = exact.to_lowercase();
            ensure!(
                !files.contains(&lower),
                "backup directory conflicts with a file"
            );
            if let Some(previous) = directories.insert(lower, exact.clone()) {
                ensure!(previous == exact, "case-colliding backup directory");
            }
        }
        total = total
            .checked_add(entry.size)
            .context("backup size overflow")?;
        ensure!(total <= MAX_TOTAL, "backup exceeds 8 GiB limit");
    }
    Ok(total)
}

fn report(manifest: &Manifest) -> Report {
    Report {
        format_version: 1,
        sources: manifest.sources,
        files: manifest.entries.len(),
        bytes: manifest.entries.iter().map(|entry| entry.size).sum(),
        destination: None,
        config_path: None,
    }
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
mod platform {
    use super::*;
    use cap_primitives::fs::OpenOptionsExt;
    use cap_std::{
        ambient_authority,
        fs::{Dir, DirBuilderExt, OpenOptions},
    };
    use std::{
        collections::BTreeMap,
        ffi::OsString,
        fs::File,
        io::{ErrorKind, Read, Seek, SeekFrom, Write},
        os::unix::fs::MetadataExt,
        path::Component,
        sync::atomic::{AtomicU64, Ordering},
    };

    static NEXT: AtomicU64 = AtomicU64::new(0);

    fn open_root(path: &Path) -> Result<Dir> {
        ensure!(path.is_absolute(), "backup directory must be absolute");
        let mut directory = Dir::open_ambient_dir("/", ambient_authority())?.into_std_file();
        for component in path.components() {
            match component {
                Component::RootDir => {}
                Component::Normal(name) => {
                    directory = cap_primitives::fs::open_dir_nofollow(&directory, Path::new(name))?
                }
                _ => bail!("backup directory must be a canonical absolute path"),
            }
        }
        Ok(Dir::from_std_file(directory))
    }

    fn parent(path: &Path) -> Result<(PathBuf, Dir, OsString)> {
        let filename = path
            .file_name()
            .context("backup path needs a file or directory name")?
            .to_owned();
        ensure!(
            path.components()
                .all(|c| !matches!(c, Component::ParentDir)),
            "backup destination refuses parent traversal"
        );
        let parent = path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        let parent = parent
            .canonicalize()
            .context("resolve existing backup parent")?;
        let directory = open_root(&parent)?;
        Ok((parent, directory, filename))
    }

    fn read_file(dir: &Dir, name: &Path) -> Result<Vec<u8>> {
        let anchor = dir.try_clone()?.into_std_file();
        let mut options = cap_primitives::fs::OpenOptions::new();
        options
            .read(true)
            ._cap_fs_ext_nonblock(true)
            ._cap_fs_ext_follow(cap_primitives::fs::FollowSymlinks::No);
        let mut file = cap_primitives::fs::open(&anchor, name, &options)?;
        let before = file.metadata()?;
        ensure!(
            before.is_file() && before.len() <= MAX_FILE,
            "backup input must be a regular file of at most 64 MiB"
        );
        let mut bytes = Vec::new();
        Read::by_ref(&mut file)
            .take(MAX_FILE + 1)
            .read_to_end(&mut bytes)?;
        let after = file.metadata()?;
        ensure!(
            bytes.len() as u64 <= MAX_FILE
                && before.len() == after.len()
                && bytes.len() as u64 == after.len()
                && before.modified()? == after.modified()?
                && (before.dev(), before.ino(), before.nlink())
                    == (after.dev(), after.ino(), after.nlink()),
            "backup input changed while reading"
        );
        Ok(bytes)
    }

    fn walk(
        dir: &Dir,
        prefix: &str,
        source: usize,
        entries: &mut Vec<Entry>,
        used: &mut u64,
        mut payload: Option<&mut File>,
    ) -> Result<()> {
        ensure!(
            prefix.split('/').count() <= 128,
            "backup source nesting exceeds limit"
        );
        let mut children = dir
            .entries()?
            .take(MAX_FILES + 1)
            .collect::<std::io::Result<Vec<_>>>()?;
        ensure!(
            children.len() <= MAX_FILES,
            "backup directory has too many children"
        );
        children.sort_by_key(|entry| entry.file_name());
        for child in children {
            let name = child.file_name();
            if name
                .to_str()
                .is_some_and(|n| n.eq_ignore_ascii_case(".git"))
                || name == ".agent-memory.lock"
            {
                continue;
            }
            let name = name.to_str().context("backup filenames must be UTF-8")?;
            let path = if prefix.is_empty() {
                name.to_owned()
            } else {
                format!("{prefix}/{name}")
            };
            let kind = child.file_type()?;
            if kind.is_symlink() {
                ensure!(
                    !selected(&path)
                        && !matches!(name, ".agent-memory-history" | ".agent-memory-operations"),
                    "backup refuses symbolic links in selected data"
                );
                continue;
            }
            if kind.is_dir() {
                let anchor = dir.try_clone()?.into_std_file();
                let child = Dir::from_std_file(cap_primitives::fs::open_dir_nofollow(
                    &anchor,
                    Path::new(name),
                )?);
                walk(&child, &path, source, entries, used, payload.as_deref_mut())?;
            } else if selected(&path) {
                ensure!(kind.is_file(), "backup selected data must be regular files");
                path_parts(&path)?;
                let bytes = read_file(dir, Path::new(name))?;
                ensure!(entries.len() < MAX_FILES, "backup has too many files");
                *used = used
                    .checked_add(bytes.len() as u64)
                    .context("backup size overflow")?;
                ensure!(*used <= MAX_TOTAL, "backup exceeds 8 GiB limit");
                entries.push(Entry {
                    source,
                    path,
                    size: bytes.len() as u64,
                    sha256: memory_core::content_hash(&bytes),
                });
                if let Some(output) = payload.as_deref_mut() {
                    output.write_all(&bytes)?;
                }
            }
        }
        Ok(())
    }

    struct Temporary {
        directory: Dir,
        name: OsString,
        is_dir: bool,
        published: bool,
    }
    impl Drop for Temporary {
        fn drop(&mut self) {
            if !self.published {
                if self.is_dir {
                    let _ = self.directory.remove_dir_all(&self.name);
                } else {
                    let _ = self.directory.remove_file(&self.name);
                }
            }
        }
    }

    fn temporary(parent: &Dir, directory: bool) -> Result<(Temporary, File)> {
        loop {
            let name = OsString::from(format!(
                ".agent-memory-backup-{}-{}.tmp",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            if directory {
                let mut builder = cap_std::fs::DirBuilder::new();
                builder.mode(0o700);
                match parent.create_dir_with(&name, &builder) {
                    Err(e) if e.kind() == ErrorKind::AlreadyExists => continue,
                    Err(e) => return Err(e.into()),
                    Ok(()) => {}
                }
                return Ok((
                    Temporary {
                        directory: parent.try_clone()?,
                        name,
                        is_dir: true,
                        published: false,
                    },
                    parent.open(".")?.into_std(),
                ));
            }
            let mut options = OpenOptions::new();
            options.write(true).read(true).create_new(true).mode(0o600);
            match parent.open_with(&name, &options) {
                Err(e) if e.kind() == ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(e.into()),
                Ok(file) => {
                    return Ok((
                        Temporary {
                            directory: parent.try_clone()?,
                            name,
                            is_dir: false,
                            published: false,
                        },
                        file.into_std(),
                    ))
                }
            }
        }
    }

    fn open_archive(path: &Path) -> Result<File> {
        let (_, directory, name) = parent(path)?;
        let anchor = directory.into_std_file();
        let mut options = cap_primitives::fs::OpenOptions::new();
        options
            .read(true)
            ._cap_fs_ext_nonblock(true)
            ._cap_fs_ext_follow(cap_primitives::fs::FollowSymlinks::No);
        let file = cap_primitives::fs::open(&anchor, Path::new(&name), &options)?;
        ensure!(
            file.metadata()?.is_file(),
            "backup archive must be a regular file"
        );
        Ok(file)
    }

    fn read_manifest(file: &mut File) -> Result<Manifest> {
        let mut magic = vec![0; MAGIC.len()];
        file.read_exact(&mut magic)?;
        ensure!(magic == MAGIC, "unsupported backup format");
        let mut length = [0; 8];
        file.read_exact(&mut length)?;
        let length = u64::from_le_bytes(length);
        ensure!(length <= MAX_MANIFEST, "backup manifest exceeds size limit");
        let mut checksum = [0; 64];
        file.read_exact(&mut checksum)?;
        let mut bytes = vec![0; length as usize];
        file.read_exact(&mut bytes)?;
        ensure!(
            memory_core::content_hash(&bytes).as_bytes() == checksum,
            "backup manifest checksum mismatch"
        );
        let manifest: Manifest = serde_json::from_slice(&bytes)?;
        let total = validate(&manifest)?;
        ensure!(
            file.metadata()?.len() == MAGIC.len() as u64 + 8 + 64 + length + total,
            "backup is truncated or has trailing data"
        );
        Ok(manifest)
    }

    fn read_payload(file: &mut File, entry: &Entry) -> Result<Vec<u8>> {
        let mut bytes = vec![0; entry.size as usize];
        file.read_exact(&mut bytes)?;
        ensure!(
            memory_core::content_hash(&bytes) == entry.sha256,
            "backup file checksum mismatch: {}",
            entry.path
        );
        Ok(bytes)
    }

    fn stable_archive(file: &File, before: &std::fs::Metadata) -> Result<()> {
        let after = file.metadata()?;
        ensure!(
            before.len() == after.len()
                && before.modified()? == after.modified()?
                && (
                    before.dev(),
                    before.ino(),
                    before.ctime(),
                    before.ctime_nsec()
                ) == (after.dev(), after.ino(), after.ctime(), after.ctime_nsec()),
            "backup archive changed while reading"
        );
        Ok(())
    }

    fn ensure_eof(file: &mut File) -> Result<()> {
        let mut trailing = [0];
        ensure!(file.read(&mut trailing)? == 0, "backup has trailing data");
        Ok(())
    }

    pub fn create(database: &Path, output: &Path) -> Result<Report> {
        let (parent_path, parent, filename) = parent(output)?;
        match parent.symlink_metadata(&filename) {
            Ok(_) => bail!("backup destination already exists"),
            Err(e) if e.kind() == ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }
        let output_path = parent_path.join(&filename);
        let store = memory_sqlite::Store::open_read_only(database)?;
        let mut config = store.export_config()?;
        let mut roots = BTreeMap::new();
        for project in &config.projects {
            for source in &project.sources {
                ensure!(
                    source.path.is_absolute(),
                    "registered source root must be absolute"
                );
                roots.insert(source.path.clone(), 0_usize);
            }
        }
        let paths: Vec<_> = roots.keys().cloned().collect();
        ensure!(paths.len() <= MAX_SOURCES, "backup has too many sources");
        for pair in paths.windows(2) {
            ensure!(
                !pair[1].starts_with(&pair[0]),
                "backup refuses overlapping source roots"
            );
        }
        for (i, path) in paths.iter().enumerate() {
            ensure!(
                !output_path.starts_with(path),
                "backup output must be outside registered source roots"
            );
            roots.insert(path.clone(), i);
        }
        for project in &mut config.projects {
            for source in &mut project.sources {
                source.path = source_path(roots[&source.path]);
            }
        }
        let mut directories = Vec::new();
        let mut locks = Vec::new();
        for path in &paths {
            let directory = open_root(path)?;
            match directory.symlink_metadata(".agent-memory.lock") {
                Ok(metadata) => {
                    ensure!(
                        metadata.is_file() && !metadata.file_type().is_symlink(),
                        "backup source lock must be a regular file"
                    );
                    let anchor = directory.try_clone()?.into_std_file();
                    let mut options = cap_primitives::fs::OpenOptions::new();
                    options
                        .read(true)
                        ._cap_fs_ext_nonblock(true)
                        ._cap_fs_ext_follow(cap_primitives::fs::FollowSymlinks::No);
                    let lock = cap_primitives::fs::open(
                        &anchor,
                        Path::new(".agent-memory.lock"),
                        &options,
                    )?;
                    ensure!(
                        lock.metadata()?.is_file() && lock.metadata()?.nlink() == 1,
                        "backup source lock must be regular with one hard link"
                    );
                    lock.try_lock_shared()
                        .context("another memory writer holds the source lock; retry later")?;
                    locks.push(lock);
                }
                Err(e) if e.kind() == ErrorKind::NotFound => {}
                Err(e) => return Err(e.into()),
            }
            directories.push(directory);
        }
        let mut payload = tempfile::tempfile()?;
        let mut manifest = Manifest {
            version: 1,
            sources: paths.len(),
            config,
            entries: Vec::new(),
        };
        let mut used = 0;
        for (source, directory) in directories.iter().enumerate() {
            walk(
                directory,
                "",
                source,
                &mut manifest.entries,
                &mut used,
                Some(&mut payload),
            )?;
        }
        validate(&manifest)?;
        let mut checked = Vec::new();
        let mut checked_bytes = 0;
        for (source, directory) in directories.iter().enumerate() {
            walk(
                directory,
                "",
                source,
                &mut checked,
                &mut checked_bytes,
                None,
            )?;
        }
        ensure!(
            checked == manifest.entries,
            "backup source changed during capture; retry with writers stopped"
        );
        for (path, directory) in paths.iter().zip(&directories) {
            let current = open_root(path)?.into_std_file().metadata()?;
            let pinned = directory.try_clone()?.into_std_file().metadata()?;
            ensure!(
                (current.dev(), current.ino()) == (pinned.dev(), pinned.ino()),
                "backup source directory was replaced"
            );
        }
        let bytes = serde_json::to_vec(&manifest)?;
        ensure!(
            bytes.len() as u64 <= MAX_MANIFEST,
            "backup manifest exceeds size limit"
        );
        let (temporary, mut file) = temporary(&parent, false)?;
        file.write_all(MAGIC)?;
        file.write_all(&(bytes.len() as u64).to_le_bytes())?;
        file.write_all(memory_core::content_hash(&bytes).as_bytes())?;
        file.write_all(&bytes)?;
        payload.seek(SeekFrom::Start(0))?;
        std::io::copy(&mut payload, &mut file)?;
        file.sync_all()?;
        parent
            .hard_link(&temporary.name, &parent, &filename)
            .context("backup destination already exists or cannot be published")?;
        parent
            .open(".")?
            .sync_all()
            .context("backup published, but syncing parent directory failed")?;
        drop(temporary);
        drop(locks);
        Ok(report(&manifest))
    }

    pub fn verify(archive: &Path) -> Result<Report> {
        let mut file = open_archive(archive)?;
        let archive_before = file.metadata()?;
        let manifest = read_manifest(&mut file)?;
        for entry in &manifest.entries {
            read_payload(&mut file, entry)?;
        }
        ensure_eof(&mut file)?;
        stable_archive(&file, &archive_before)?;
        Ok(report(&manifest))
    }

    fn child(parent: &Dir, name: &str) -> Result<Dir> {
        let mut builder = cap_std::fs::DirBuilder::new();
        builder.mode(0o700);
        match parent.create_dir_with(name, &builder) {
            Ok(()) => {
                parent.open(".")?.sync_all()?;
            }
            Err(e) if e.kind() == ErrorKind::AlreadyExists => {}
            Err(e) => return Err(e.into()),
        }
        let anchor = parent.try_clone()?.into_std_file();
        Ok(Dir::from_std_file(cap_primitives::fs::open_dir_nofollow(
            &anchor,
            Path::new(name),
        )?))
    }

    pub fn restore(archive: &Path, destination: &Path) -> Result<Report> {
        let mut file = open_archive(archive)?;
        let archive_before = file.metadata()?;
        let manifest = read_manifest(&mut file)?;
        let (parent_path, parent, name) = parent(destination)?;
        match parent.symlink_metadata(&name) {
            Ok(_) => bail!("restore destination already exists"),
            Err(e) if e.kind() == ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }
        let (mut stage, _) = temporary(&parent, true)?;
        let anchor = parent.try_clone()?.into_std_file();
        let directory = Dir::from_std_file(cap_primitives::fs::open_dir_nofollow(
            &anchor,
            Path::new(&stage.name),
        )?);
        let sources = child(&directory, "sources")?;
        for source in 0..manifest.sources {
            child(&sources, &source.to_string())?;
        }
        for entry in &manifest.entries {
            let bytes = read_payload(&mut file, entry)?;
            let mut directory = child(&sources, &entry.source.to_string())?;
            let parts = path_parts(&entry.path)?;
            for part in &parts[..parts.len() - 1] {
                directory = child(&directory, part)?;
            }
            let mut options = OpenOptions::new();
            options.write(true).create_new(true).mode(0o600);
            let mut file = directory.open_with(parts[parts.len() - 1], &options)?;
            file.write_all(&bytes)?;
            file.sync_all()?;
            directory.open(".")?.sync_all()?;
        }
        ensure_eof(&mut file)?;
        stable_archive(&file, &archive_before)?;
        let mut options = OpenOptions::new();
        options.write(true).create_new(true).mode(0o600);
        let mut config = directory.open_with(CONFIG_FILE, &options)?;
        serde_json::to_writer_pretty(&mut config, &manifest.config)?;
        config.write_all(b"\n")?;
        config.sync_all()?;
        directory.open(".")?.sync_all()?;
        rustix::fs::renameat_with(
            &anchor,
            &stage.name,
            &anchor,
            &name,
            rustix::fs::RenameFlags::NOREPLACE,
        )
        .map_err(std::io::Error::from)
        .context("publish restored directory without overwriting")?;
        stage.published = true;
        parent
            .open(".")?
            .sync_all()
            .context("restore published, but syncing parent directory failed")?;
        let destination = parent_path.join(name);
        let mut result = report(&manifest);
        result.config_path = Some(destination.join(CONFIG_FILE));
        result.destination = Some(destination);
        Ok(result)
    }
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
pub use platform::{create, restore, verify};

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
pub fn create(_: &Path, _: &Path) -> Result<Report> {
    bail!("backup requires Linux, macOS, or WSL")
}
#[cfg(not(any(target_os = "linux", target_os = "macos")))]
pub fn verify(_: &Path) -> Result<Report> {
    bail!("backup requires Linux, macOS, or WSL")
}
#[cfg(not(any(target_os = "linux", target_os = "macos")))]
pub fn restore(_: &Path, _: &Path) -> Result<Report> {
    bail!("restore requires Linux, macOS, or WSL")
}

#[cfg(all(test, any(target_os = "linux", target_os = "macos")))]
mod tests {
    use super::*;
    use memory_sqlite::config::{ProjectConfig, SourceConfig};
    use std::{fs, io::Write};

    fn manifest(path: &str, content: &[u8]) -> Manifest {
        Manifest {
            version: 1,
            sources: 1,
            config: Config {
                version: 1,
                projects: vec![ProjectConfig {
                    name: "demo".into(),
                    sources: vec![SourceConfig {
                        path: source_path(0),
                        can_read: true,
                        can_write: true,
                    }],
                }],
            },
            entries: vec![Entry {
                source: 0,
                path: path.into(),
                size: content.len() as u64,
                sha256: memory_core::content_hash(content),
            }],
        }
    }

    fn archive(path: &Path, manifest: &Manifest, payload: &[u8]) -> Result<()> {
        let bytes = serde_json::to_vec(manifest)?;
        let mut file = fs::File::create(path)?;
        file.write_all(MAGIC)?;
        file.write_all(&(bytes.len() as u64).to_le_bytes())?;
        file.write_all(memory_core::content_hash(&bytes).as_bytes())?;
        file.write_all(&bytes)?;
        file.write_all(payload)?;
        Ok(())
    }

    #[test]
    fn invalid_paths_permissions_and_source_aliases_are_rejected() -> Result<()> {
        for path in [
            "../outside.md",
            "/outside.md",
            "a\\outside.md",
            "a/../b.md",
            "AUX.md",
            ".git/README.md",
            ".GIT/note.md",
            ".agent-memory-history",
        ] {
            assert!(validate(&manifest(path, b"x")).is_err(), "{path}");
        }
        let mut value = manifest("note.md", b"x");
        value.config.projects[0].sources[0].can_read = false;
        assert!(validate(&value).is_err());
        value.config.projects[0].sources[0].can_read = true;
        value.config.projects[0].sources[0].path = PathBuf::from("sources/00");
        assert!(validate(&value).is_err());
        Ok(())
    }

    #[test]
    fn duplicate_case_and_file_directory_conflicts_are_rejected() {
        for other in ["note.md", "NOTE.md", "note.md/child.md"] {
            let mut value = manifest("note.md", b"x");
            value.entries.push(Entry {
                source: 0,
                path: other.into(),
                size: 1,
                sha256: memory_core::content_hash(b"x"),
            });
            assert!(validate(&value).is_err(), "{other}");
        }
        let mut value = manifest("Dir/a.md", b"x");
        value.entries.push(Entry {
            source: 0,
            path: "dir/b.md".into(),
            size: 1,
            sha256: memory_core::content_hash(b"x"),
        });
        assert!(validate(&value).is_err());
    }

    #[test]
    fn corrupted_manifest_and_payload_never_publish_restore() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("test.ambackup");
        let target = directory.path().join("restored");
        let value = manifest("note.md", b"safe");
        archive(&path, &value, b"evil")?;
        assert!(verify(&path).is_err());
        assert!(restore(&path, &target).is_err());
        assert!(!target.exists());
        assert_eq!(fs::read_dir(directory.path())?.count(), 1);
        archive(&path, &value, b"safe")?;
        let mut bytes = fs::read(&path)?;
        bytes[MAGIC.len() + 8] ^= 1;
        fs::write(&path, bytes)?;
        assert!(restore(&path, &target).is_err());
        assert!(!target.exists());
        archive(&path, &value, b"safeextra")?;
        assert!(verify(&path).is_err());
        archive(&path, &value, b"saf")?;
        assert!(verify(&path).is_err());
        Ok(())
    }

    #[test]
    fn shared_permissions_and_raw_journal_bytes_survive_restore() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("test.ambackup");
        let target = directory.path().join("restored");
        let bytes = b"{\"journal\":\"exact bytes\\n\"}\n";
        let mut value = manifest(".agent-memory-operations/record.json", bytes);
        value.config.projects.push(ProjectConfig {
            name: "other".into(),
            sources: vec![SourceConfig {
                path: source_path(0),
                can_read: true,
                can_write: false,
            }],
        });
        archive(&path, &value, bytes)?;
        assert_eq!(verify(&path)?.sources, 1);
        let report = restore(&path, &target)?;
        assert_eq!(
            fs::read(target.join("sources/0/.agent-memory-operations/record.json"))?,
            bytes
        );
        let config: Config = serde_json::from_slice(&fs::read(report.config_path.unwrap())?)?;
        assert_eq!(config, value.config);
        assert!(restore(&path, &target).is_err());
        Ok(())
    }

    fn registered_source() -> Result<(tempfile::TempDir, PathBuf, PathBuf)> {
        let directory = tempfile::tempdir()?;
        let root = directory.path().join("notes");
        fs::create_dir(&root)?;
        fs::write(root.join("note.md"), b"# Safe\n")?;
        let database = directory.path().join("memory.sqlite");
        let store = memory_sqlite::Store::open(&database)?;
        store.create_project("demo")?;
        store.register_source("demo", &root, true)?;
        Ok((directory, database, root))
    }

    #[test]
    fn interrupted_journal_hardlinks_restore_as_independent_regular_files() -> Result<()> {
        use std::os::unix::fs::MetadataExt;
        let (directory, database, root) = registered_source()?;
        let content = b"{\"recovery\":true}\n";
        let id = memory_core::save_operation(&root, content)?;
        let final_file = root
            .join(".agent-memory-operations")
            .join(format!("{id}.json"));
        let leftover = root.join(".agent-memory-operations/leftover.tmp");
        fs::hard_link(&final_file, &leftover)?;
        assert_eq!(fs::metadata(&final_file)?.nlink(), 2);
        let archive = directory.path().join("saved.ambackup");
        create(&database, &archive)?;
        verify(&archive)?;
        let restored = directory.path().join("restored");
        restore(&archive, &restored)?;
        let source = restored.join("sources/0");
        assert_eq!(memory_core::read_operation(&source, &id)?, content);
        for name in [format!("{id}.json"), "leftover.tmp".into()] {
            let file = source.join(".agent-memory-operations").join(name);
            assert_eq!(fs::read(&file)?, content);
            assert_eq!(fs::metadata(&file)?.nlink(), 1);
        }
        Ok(())
    }

    #[test]
    fn active_writers_block_backup_without_creating_an_archive() -> Result<()> {
        let (directory, database, root) = registered_source()?;
        let lock = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(root.join(".agent-memory.lock"))?;
        lock.try_lock()?;
        let archive = directory.path().join("saved.ambackup");
        assert!(create(&database, &archive).is_err());
        assert!(!archive.exists());
        lock.unlock()?;
        create(&database, &archive)?;
        Ok(())
    }

    #[test]
    fn nested_roots_and_substituted_source_links_are_refused() -> Result<()> {
        use std::os::unix::fs::symlink;
        let (directory, database, root) = registered_source()?;
        let nested = root.join("sub");
        fs::create_dir(&nested)?;
        let store = memory_sqlite::Store::open(&database)?;
        store.register_source("demo", &nested, true)?;
        drop(store);
        let archive = directory.path().join("saved.ambackup");
        assert!(create(&database, &archive).is_err());
        assert!(!archive.exists());
        let (directory, database, root) = registered_source()?;
        let saved = directory.path().join("original");
        fs::rename(&root, &saved)?;
        symlink(&saved, &root)?;
        let archive = directory.path().join("saved.ambackup");
        assert!(create(&database, &archive).is_err());
        assert!(!archive.exists());
        Ok(())
    }

    #[test]
    fn restore_and_verify_refuse_symlink_targets() -> Result<()> {
        use std::os::unix::fs::symlink;
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("test.ambackup");
        archive(&path, &manifest("note.md", b"safe"), b"safe")?;
        let link = directory.path().join("link");
        symlink(&path, &link)?;
        assert!(verify(&link).is_err());
        let outside = directory.path().join("outside");
        fs::create_dir(&outside)?;
        let target = directory.path().join("restored");
        symlink(&outside, &target)?;
        assert!(restore(&path, &target).is_err());
        assert_eq!(fs::read_dir(&outside)?.count(), 0);
        Ok(())
    }
}
