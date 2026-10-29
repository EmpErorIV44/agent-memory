#![cfg(unix)]

use anyhow::Result;
use memory_core::{read_markdown, read_source, write_markdown, WriteMode};
use std::{fs, os::unix::fs::symlink};

#[test]
fn replaced_registered_root_is_not_followed() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let source = temp.path().join("source");
    let parked = temp.path().join("parked");
    let outside = temp.path().join("outside");
    fs::create_dir(&source)?;
    fs::create_dir(&outside)?;
    fs::write(source.join("note.md"), "# Registered\ninside marker\n")?;
    fs::write(outside.join("note.md"), "# Unregistered\noutside marker\n")?;
    let registered = source.canonicalize()?;
    assert_eq!(read_source(&registered)?.len(), 1);

    fs::rename(&source, parked)?;
    symlink(&outside, &source)?;
    assert!(read_source(&registered).is_err());
    assert!(read_markdown(&registered, "note.md").is_err());
    assert!(write_markdown(&registered, "new.md", "# Rejected", WriteMode::Create).is_err());
    assert!(!outside.join("new.md").exists());
    assert!(!outside.join(".agent-memory.lock").exists());
    assert_eq!(
        fs::read_to_string(outside.join("note.md"))?,
        "# Unregistered\noutside marker\n"
    );
    Ok(())
}

#[test]
fn scanner_skips_symlink_files_and_directories() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let source = temp.path().join("source");
    let nested = source.join("nested");
    let outside = temp.path().join("outside");
    fs::create_dir_all(&nested)?;
    fs::create_dir(&outside)?;
    fs::write(nested.join("real.md"), "# Real\ninside marker\n")?;
    fs::write(outside.join("external.md"), "# External\noutside marker\n")?;
    symlink(&outside, source.join("linked-directory"))?;
    symlink(outside.join("external.md"), source.join("linked-file.md"))?;
    // Cycles must be skipped rather than recursed into.
    symlink(&source, nested.join("cycle"))?;
    let documents = read_source(&source.canonicalize()?)?;
    assert_eq!(documents.len(), 1);
    assert_eq!(documents[0].relative_path, "nested/real.md");
    assert!(documents[0]
        .chunks
        .iter()
        .all(|chunk| !chunk.body.contains("outside marker")));
    Ok(())
}
