//! Durable, path-scoped history. SQLite is never the archive.
use anyhow::{ensure, Context, Result};
use cap_std::fs::{Dir, OpenOptions};
#[cfg(unix)]
use cap_std::fs::{DirBuilderExt, OpenOptionsExt};
use serde::{Deserialize, Serialize};
use std::{
    fmt,
    io::{ErrorKind, Read, Write},
    path::Path,
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

pub(crate) const HISTORY_DIRECTORY: &str = ".agent-memory-history";
const PENDING: &str = "pending.json";
static TEMP_ID: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Revision {
    pub id: String,
    pub path: String,
    pub content_hash: String,
    pub previous_hash: Option<String>,
    /// Unix milliseconds when this snapshot was recorded, not the external edit time.
    pub created_at: u64,
    /// True only for a prepared snapshot inferred from the current live bytes.
    /// Such a candidate has not yet been physically finalized by a writer.
    #[serde(default, skip_serializing_if = "not_pending")]
    pub pending: bool,
}

fn not_pending(pending: &bool) -> bool {
    !pending
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RevisionDocument {
    pub revision: Revision,
    pub content: String,
}

/// An error after publication must not be mistaken for an unsaved Markdown write.
#[derive(Debug)]
pub struct PublishedWriteError {
    pub content_hash: String,
    pub message: String,
}

impl fmt::Display for PublishedWriteError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "Markdown saved, but revision finalization needs recovery: {}",
            self.message
        )
    }
}
impl std::error::Error for PublishedWriteError {}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Envelope {
    version: u32,
    revision: Revision,
    checksum: String,
}

pub(crate) struct History {
    directory: Dir,
    path: String,
}

pub(crate) fn sync_directory(directory: &Dir) -> Result<()> {
    // Capability directory handles may use O_PATH, which cannot be fsynced.
    // Re-open the pinned directory itself with ordinary read access.
    directory.open(".")?.sync_all().context("sync directory")
}

pub(crate) fn child_directory(parent: &Dir, name: &str, create: bool) -> Result<Option<Dir>> {
    match parent.symlink_metadata(name) {
        Ok(metadata) => ensure!(
            metadata.is_dir() && !metadata.file_type().is_symlink(),
            "history component must be a real directory"
        ),
        Err(error) if error.kind() == ErrorKind::NotFound && !create => return Ok(None),
        Err(error) if error.kind() == ErrorKind::NotFound => {
            let mut builder = cap_std::fs::DirBuilder::new();
            #[cfg(unix)]
            builder.mode(0o700);
            match parent.create_dir_with(name, &builder) {
                Ok(()) => sync_directory(parent)?,
                Err(error) if error.kind() == ErrorKind::AlreadyExists => {}
                Err(error) => return Err(error.into()),
            }
        }
        Err(error) => return Err(error.into()),
    }
    let anchor = parent.try_clone()?.into_std_file();
    Ok(Some(Dir::from_std_file(
        cap_primitives::fs::open_dir_nofollow(&anchor, Path::new(name))?,
    )))
}

fn private_directory(directory: &Dir) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        directory
            .open(".")?
            .into_std()
            .set_permissions(std::fs::Permissions::from_mode(0o700))?;
        sync_directory(directory)?;
    }
    #[cfg(not(unix))]
    let _ = directory;
    Ok(())
}

/// Pin the final file without following a substituted symlink.
pub(crate) fn read_file(directory: &Dir, name: &str, max: u64) -> Result<Vec<u8>> {
    let anchor = directory.try_clone()?.into_std_file();
    let mut options = cap_primitives::fs::OpenOptions::new();
    options
        .read(true)
        ._cap_fs_ext_follow(cap_primitives::fs::FollowSymlinks::No);
    let file = cap_primitives::fs::open(&anchor, Path::new(name), &options)?;
    let metadata = file.metadata()?;
    ensure!(
        metadata.is_file() && metadata.len() <= max,
        "history/source file is not regular or exceeds its size limit"
    );
    let mut bytes = Vec::new();
    file.take(max + 1).read_to_end(&mut bytes)?;
    ensure!(
        bytes.len() as u64 <= max,
        "history/source file exceeded its size limit while reading"
    );
    Ok(bytes)
}

pub(crate) fn read_live(directory: &Dir, path: &str) -> Result<Option<Vec<u8>>> {
    crate::write::validate_path(path)?;
    let mut parent = directory.try_clone()?;
    let mut parts = path.split('/').peekable();
    while let Some(part) = parts.next() {
        if parts.peek().is_none() {
            return match read_file(&parent, part, 1024 * 1024) {
                Ok(bytes) => Ok(Some(bytes)),
                Err(error)
                    if error
                        .downcast_ref::<std::io::Error>()
                        .is_some_and(|e| e.kind() == ErrorKind::NotFound) =>
                {
                    Ok(None)
                }
                Err(error) => Err(error),
            };
        }
        let Some(child) = child_directory(&parent, part, false)? else {
            return Ok(None);
        };
        parent = child;
    }
    unreachable!("validated nonempty path")
}

fn valid_hash(hash: &str) -> bool {
    hash.len() == 64
        && hash
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

fn sequence(id: &str) -> Result<u64> {
    ensure!(
        id.len() == 37
            && id.as_bytes()[20] == b'-'
            && id[..20].bytes().all(|b| b.is_ascii_digit())
            && id[21..]
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)),
        "invalid revision ID"
    );
    let sequence: u64 = id[..20].parse()?;
    ensure!(sequence > 0, "invalid revision sequence");
    Ok(sequence)
}

fn identity(revision: &Revision, number: u64) -> Result<String> {
    let payload = serde_json::to_vec(&(
        &revision.path,
        &revision.content_hash,
        &revision.previous_hash,
        revision.created_at,
        number,
    ))?;
    Ok(format!(
        "{number:020}-{}",
        &crate::content_hash(&payload)[..16]
    ))
}

impl History {
    pub(crate) fn open(source: &Dir, path: &str, create: bool) -> Result<Option<Self>> {
        crate::write::validate_path(path)?;
        let Some(root) = child_directory(source, HISTORY_DIRECTORY, create)? else {
            return Ok(None);
        };
        if create {
            private_directory(&root)?;
        }
        let key = crate::content_hash(path.as_bytes());
        let Some(directory) = child_directory(&root, &key, create)? else {
            return Ok(None);
        };
        if create {
            private_directory(&directory)?;
        }
        Ok(Some(Self {
            directory,
            path: path.to_owned(),
        }))
    }

    fn blob(&self, hash: &str) -> Result<Vec<u8>> {
        ensure!(valid_hash(hash), "invalid revision content hash");
        let bytes = read_file(&self.directory, &format!("{hash}.blob"), 1024 * 1024)?;
        ensure!(
            crate::content_hash(&bytes) == hash,
            "revision snapshot checksum mismatch"
        );
        std::str::from_utf8(&bytes).context("revision snapshot is not UTF-8")?;
        Ok(bytes)
    }

    fn record(&self, name: &str) -> Result<Revision> {
        let bytes = read_file(&self.directory, name, 16 * 1024)?;
        let envelope: Envelope =
            serde_json::from_slice(&bytes).context("invalid revision metadata")?;
        let revision = envelope.revision;
        ensure!(
            !revision.pending,
            "stored revisions cannot carry an inferred pending marker"
        );
        ensure!(
            envelope.version == 1 && revision.path == self.path,
            "revision metadata scope/version mismatch"
        );
        let number = sequence(&revision.id)?;
        ensure!(
            identity(&revision, number)? == revision.id,
            "revision identity checksum mismatch"
        );
        ensure!(
            name == PENDING || name == format!("{}.json", revision.id),
            "revision filename mismatch"
        );
        ensure!(
            valid_hash(&revision.content_hash)
                && revision.previous_hash.as_deref().is_none_or(valid_hash),
            "invalid revision hashes"
        );
        ensure!(
            crate::content_hash(&serde_json::to_vec(&revision)?) == envelope.checksum,
            "revision metadata checksum mismatch"
        );
        self.blob(&revision.content_hash)?;
        Ok(revision)
    }

    fn committed(&self) -> Result<Vec<Revision>> {
        let mut names = Vec::new();
        for entry in self.directory.entries()? {
            let entry = entry?;
            let name = entry.file_name();
            let name = name.to_str().context("history name is not UTF-8")?;
            if name.ends_with(".json") && name != PENDING {
                names.push(name.to_owned());
            }
        }
        names.sort();
        let mut result: Vec<Revision> = Vec::new();
        for name in names {
            let revision = self.record(&name)?;
            ensure!(
                sequence(&revision.id)? == result.len() as u64 + 1,
                "revision history sequence is incomplete"
            );
            ensure!(
                revision.previous_hash.as_deref() == result.last().map(|r| r.content_hash.as_str()),
                "revision history hash chain is broken"
            );
            result.push(revision);
        }
        Ok(result)
    }

    fn pending(&self) -> Result<Option<Revision>> {
        match self.directory.symlink_metadata(PENDING) {
            Ok(_) => self.record(PENDING).map(Some),
            Err(error) if error.kind() == ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error.into()),
        }
    }

    fn published(&self, source: &Dir, revision: &Revision) -> Result<bool> {
        Ok(read_live(source, &self.path)?
            .is_some_and(|bytes| crate::content_hash(&bytes) == revision.content_hash))
    }

    pub(crate) fn recover(&self, source: &Dir) -> Result<()> {
        if let Some(pending) = self.pending()? {
            let committed = self.committed()?;
            let already = committed.last().is_some_and(|r| r.id == pending.id);
            ensure!(
                already || sequence(&pending.id)? == committed.len() as u64 + 1,
                "pending revision sequence mismatch"
            );
            ensure!(
                already
                    || pending.previous_hash.as_deref()
                        == committed.last().map(|r| r.content_hash.as_str()),
                "pending revision chain mismatch"
            );
            if already || self.published(source, &pending)? {
                self.finalize(&pending)?;
            } else {
                // An uncooperative editor can replace a published file before
                // recovery. Hash mismatch cannot distinguish that from an
                // abandoned pre-publication attempt: preserve the candidate,
                // but never claim it was a committed revision.
                let unconfirmed = child_directory(&self.directory, "unconfirmed", true)?
                    .context("unconfirmed history directory was not created")?;
                private_directory(&unconfirmed)?;
                let name = format!("{}.json", pending.id);
                self.directory
                    .hard_link(PENDING, &unconfirmed, &name)
                    .or_else(|error| {
                        if error.kind() == ErrorKind::AlreadyExists {
                            Ok(())
                        } else {
                            Err(error)
                        }
                    })?;
                ensure!(
                    read_file(&self.directory, PENDING, 16 * 1024)?
                        == read_file(&unconfirmed, &name, 16 * 1024)?,
                    "unconfirmed revision collision"
                );
                sync_directory(&unconfirmed)?;
                self.directory.remove_file(PENDING)?;
                sync_directory(&self.directory)?;
            }
        }
        Ok(())
    }

    fn write_new(&self, name: &str, bytes: &[u8]) -> Result<()> {
        match self.directory.symlink_metadata(name) {
            Ok(_) => {
                ensure!(
                    read_file(&self.directory, name, 1024 * 1024)? == bytes,
                    "immutable revision file already exists with different content"
                );
                return Ok(());
            }
            Err(error) if error.kind() == ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
        let (temporary, mut file) = loop {
            let temporary = format!(
                ".history-{}-{}.tmp",
                std::process::id(),
                TEMP_ID.fetch_add(1, Ordering::Relaxed)
            );
            let mut options = OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            options.mode(0o600);
            match self.directory.open_with(&temporary, &options) {
                Ok(file) => break (temporary, file),
                Err(error) if error.kind() == ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(error.into()),
            }
        };
        let result = (|| -> Result<()> {
            file.write_all(bytes)?;
            file.sync_all()?;
            drop(file);
            self.directory
                .hard_link(&temporary, &self.directory, name)?;
            sync_directory(&self.directory)?;
            Ok(())
        })();
        let _ = self.directory.remove_file(&temporary);
        result
    }

    fn new_revision(
        &self,
        bytes: &[u8],
        previous: Option<&Revision>,
        number: u64,
    ) -> Result<Revision> {
        let created_at = SystemTime::now()
            .duration_since(UNIX_EPOCH)?
            .as_millis()
            .try_into()
            .context("revision timestamp overflow")?;
        let mut revision = Revision {
            id: String::new(),
            path: self.path.clone(),
            content_hash: crate::content_hash(bytes),
            previous_hash: previous.map(|r| r.content_hash.clone()),
            created_at,
            pending: false,
        };
        revision.id = identity(&revision, number)?;
        self.write_new(&format!("{}.blob", revision.content_hash), bytes)?;
        Ok(revision)
    }

    fn save_record(&self, name: &str, revision: &Revision) -> Result<()> {
        let record = Envelope {
            version: 1,
            revision: revision.clone(),
            checksum: crate::content_hash(&serde_json::to_vec(revision)?),
        };
        self.write_new(name, &serde_json::to_vec(&record)?)
    }

    /// Capture existing external text before preparing any replacement.
    pub(crate) fn baseline(&self, bytes: &[u8]) -> Result<()> {
        let previous = self.committed()?;
        let hash = crate::content_hash(bytes);
        if previous.last().is_none_or(|r| r.content_hash != hash) {
            let revision = self.new_revision(bytes, previous.last(), previous.len() as u64 + 1)?;
            self.save_record(&format!("{}.json", revision.id), &revision)?;
        }
        Ok(())
    }

    pub(crate) fn prepare(&self, bytes: &[u8]) -> Result<Revision> {
        let previous = self.committed()?;
        let revision = self.new_revision(bytes, previous.last(), previous.len() as u64 + 1)?;
        self.save_record(PENDING, &revision)?;
        Ok(revision)
    }

    pub(crate) fn finalize(&self, revision: &Revision) -> Result<()> {
        let name = format!("{}.json", revision.id);
        self.directory
            .hard_link(PENDING, &self.directory, &name)
            .or_else(|error| {
                if error.kind() == ErrorKind::AlreadyExists {
                    Ok(())
                } else {
                    Err(error)
                }
            })?;
        ensure!(
            self.record(&name)? == *revision,
            "existing committed revision differs from pending record"
        );
        sync_directory(&self.directory)?;
        self.directory.remove_file(PENDING)?;
        sync_directory(&self.directory)?;
        Ok(())
    }

    fn view(&self, source: &Dir) -> Result<Vec<Revision>> {
        let mut revisions = self.committed()?;
        if let Some(mut pending) = self.pending()? {
            let already = revisions.last().is_some_and(|r| r.id == pending.id);
            if !already {
                ensure!(
                    sequence(&pending.id)? == revisions.len() as u64 + 1
                        && pending.previous_hash.as_deref()
                            == revisions.last().map(|r| r.content_hash.as_str()),
                    "pending revision chain mismatch"
                );
                if self.published(source, &pending)? {
                    pending.pending = true;
                    revisions.push(pending);
                }
            }
        }
        Ok(revisions)
    }
}

fn read_history(root: &Path, path: &str) -> Result<Option<(Dir, History, Option<std::fs::File>)>> {
    crate::write::validate_path(path)?;
    let source = crate::write::open_root(root)?;
    // A read never creates lock/history files, including for cloned read-only sources.
    let lock = match source.symlink_metadata(".agent-memory.lock") {
        Ok(metadata) => {
            ensure!(
                metadata.is_file() && !metadata.file_type().is_symlink(),
                "source lock must not be a symbolic link"
            );
            let anchor = source.try_clone()?.into_std_file();
            let mut options = cap_primitives::fs::OpenOptions::new();
            options
                .read(true)
                ._cap_fs_ext_follow(cap_primitives::fs::FollowSymlinks::No);
            let file =
                cap_primitives::fs::open(&anchor, Path::new(".agent-memory.lock"), &options)?;
            file.try_lock_shared()
                .context("another memory writer holds the source lock; retry later")?;
            Some(file)
        }
        Err(error) if error.kind() == ErrorKind::NotFound => None,
        Err(error) => return Err(error.into()),
    };
    Ok(History::open(&source, path, false)?.map(|history| (source, history, lock)))
}

pub fn list_revisions(root: &Path, path: &str) -> Result<Vec<Revision>> {
    let Some((source, history, _lock)) = read_history(root, path)? else {
        return Ok(Vec::new());
    };
    history.view(&source)
}

pub fn get_revision(
    root: &Path,
    path: &str,
    revision_id: &str,
) -> Result<Option<RevisionDocument>> {
    sequence(revision_id)?;
    let Some((source, history, _lock)) = read_history(root, path)? else {
        return Ok(None);
    };
    let Some(revision) = history
        .view(&source)?
        .into_iter()
        .find(|revision| revision.id == revision_id)
    else {
        return Ok(None);
    };
    let content = String::from_utf8(history.blob(&revision.content_hash)?)?;
    Ok(Some(RevisionDocument { revision, content }))
}
