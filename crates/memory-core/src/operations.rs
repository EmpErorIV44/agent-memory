//! Immutable source-local operation journals, independent of the SQLite index.
use anyhow::{ensure, Context, Result};
#[cfg(unix)]
use cap_std::fs::OpenOptionsExt;
use cap_std::fs::{Dir, OpenOptions};
use std::{
    io::{ErrorKind, Write},
    path::Path,
    sync::atomic::{AtomicU64, Ordering},
};

const DIRECTORY: &str = ".agent-memory-operations";
const MAX_BYTES: u64 = 64 * 1024 * 1024;
static NEXT: AtomicU64 = AtomicU64::new(0);

fn valid_id(id: &str) -> Result<()> {
    ensure!(
        id.len() == 64
            && id
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)),
        "invalid operation ID"
    );
    Ok(())
}

fn directory(root: &Path, create: bool) -> Result<Option<Dir>> {
    let source = crate::write::open_root(root)?;
    let dir = crate::history::child_directory(&source, DIRECTORY, create)?;
    if create {
        if let Some(dir) = &dir {
            crate::history::private_directory(dir)?;
        }
    }
    Ok(dir)
}

fn publish(dir: &Dir, name: &str, bytes: &[u8]) -> Result<()> {
    if let Ok(existing) = crate::history::read_file(dir, name, MAX_BYTES) {
        ensure!(existing == bytes, "operation journal conflict");
        return Ok(());
    }
    let temporary = format!(
        ".operation-{}-{}.tmp",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    );
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    options.mode(0o600);
    let mut file = dir.open_with(&temporary, &options)?;
    let result = (|| -> Result<()> {
        file.write_all(bytes)?;
        file.sync_all()?;
        dir.hard_link(&temporary, dir, name)
            .context("publish immutable operation journal")?;
        crate::history::sync_directory(dir)?;
        Ok(())
    })();
    drop(file);
    let _ = dir.remove_file(&temporary);
    result
}

pub fn save_operation(root: &Path, bytes: &[u8]) -> Result<String> {
    ensure!(
        bytes.len() as u64 <= MAX_BYTES,
        "operation journal exceeds size limit"
    );
    let id = crate::content_hash(bytes);
    let dir = directory(root, true)?.context("operation directory unavailable")?;
    publish(&dir, &format!("{id}.json"), bytes)?;
    Ok(id)
}

pub fn read_operation(root: &Path, id: &str) -> Result<Vec<u8>> {
    valid_id(id)?;
    let dir = directory(root, false)?.context("operation journal not found")?;
    let bytes = crate::history::read_file(&dir, &format!("{id}.json"), MAX_BYTES)?;
    ensure!(
        crate::content_hash(&bytes) == id,
        "operation journal checksum mismatch"
    );
    Ok(bytes)
}

pub fn list_operations(root: &Path) -> Result<Vec<String>> {
    let Some(dir) = directory(root, false)? else {
        return Ok(Vec::new());
    };
    let mut ids = Vec::new();
    for entry in dir.entries()? {
        let entry = entry?;
        let name = entry.file_name();
        let Some(id) = name.to_str().and_then(|n| n.strip_suffix(".json")) else {
            continue;
        };
        valid_id(id)?;
        ensure!(
            entry.file_type()?.is_file(),
            "operation record must be a regular file"
        );
        ids.push(id.to_owned());
    }
    ids.sort();
    Ok(ids)
}

pub fn operation_complete(root: &Path, id: &str) -> Result<bool> {
    valid_id(id)?;
    let Some(dir) = directory(root, false)? else {
        return Ok(false);
    };
    match crate::history::read_file(&dir, &format!("{id}.done"), 64) {
        Ok(bytes) => {
            ensure!(
                bytes == id.as_bytes(),
                "invalid operation completion marker"
            );
            Ok(true)
        }
        Err(error)
            if error
                .downcast_ref::<std::io::Error>()
                .is_some_and(|e| e.kind() == ErrorKind::NotFound) =>
        {
            Ok(false)
        }
        Err(error) => Err(error),
    }
}

pub fn finish_operation(root: &Path, id: &str) -> Result<()> {
    read_operation(root, id)?;
    let dir = directory(root, false)?.context("operation directory unavailable")?;
    publish(&dir, &format!("{id}.done"), id.as_bytes())
}

/// Safely distinguish absence from unreadable or substituted paths.
pub fn markdown_hash(root: &Path, path: &str) -> Result<Option<String>> {
    let dir = crate::write::open_root(root)?;
    Ok(crate::history::read_live(&dir, path)?.map(|bytes| crate::content_hash(&bytes)))
}
