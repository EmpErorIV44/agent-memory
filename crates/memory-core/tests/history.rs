use anyhow::Result;
use memory_core::{
    content_hash, get_revision, list_revisions, read_source, write_markdown, WriteMode,
};
use std::{fs, path::Path};

fn journal(root: &Path, path: &str) -> std::path::PathBuf {
    root.join(".agent-memory-history")
        .join(content_hash(path.as_bytes()))
}

#[test]
fn create_update_restore_and_noop_keep_ordered_immutable_snapshots() -> Result<()> {
    let source = tempfile::tempdir()?;
    let first = "# Первая\r\nПолный исходный текст.\r\n";
    let second = "# Second\nReplacement.\n";
    let initial = write_markdown(source.path(), "notes/one.md", first, WriteMode::Create)?;
    let updated = write_markdown(
        source.path(),
        "notes/one.md",
        second,
        WriteMode::Update {
            expected_hash: &initial,
        },
    )?;
    let before_restore = list_revisions(source.path(), "notes/one.md")?;
    assert_eq!(before_restore.len(), 2);
    assert_eq!(before_restore[0].previous_hash, None);
    assert_eq!(
        before_restore[1].previous_hash.as_deref(),
        Some(initial.as_str())
    );
    let previous = get_revision(source.path(), "notes/one.md", &before_restore[0].id)?.unwrap();
    assert_eq!(previous.content, first);
    write_markdown(
        source.path(),
        "notes/one.md",
        &previous.content,
        WriteMode::Update {
            expected_hash: &updated,
        },
    )?;
    let after_restore = list_revisions(source.path(), "notes/one.md")?;
    assert_eq!(after_restore.len(), 3);
    assert_eq!(after_restore[2].content_hash, initial);
    assert_eq!(
        after_restore[2].previous_hash.as_deref(),
        Some(updated.as_str())
    );
    assert!(after_restore.windows(2).all(|pair| pair[0].id < pair[1].id));
    assert_eq!(
        get_revision(source.path(), "notes/one.md", &after_restore[1].id)?
            .unwrap()
            .content,
        second
    );
    write_markdown(
        source.path(),
        "notes/one.md",
        first,
        WriteMode::Update {
            expected_hash: &initial,
        },
    )?;
    assert_eq!(
        list_revisions(source.path(), "notes/one.md")?,
        after_restore
    );
    assert!(get_revision(source.path(), "elsewhere.md", &after_restore[0].id)?.is_none());
    assert!(get_revision(source.path(), "notes/one.md", "../escape").is_err());
    Ok(())
}

#[test]
fn existing_and_external_edits_are_captured_before_overwrite() -> Result<()> {
    let source = tempfile::tempdir()?;
    let path = source.path().join("existing.md");
    fs::write(&path, "# External one\n")?;
    write_markdown(
        source.path(),
        "existing.md",
        "# API two\n",
        WriteMode::Update {
            expected_hash: &content_hash(b"# External one\n"),
        },
    )?;
    fs::write(&path, "# External three\n")?;
    write_markdown(
        source.path(),
        "existing.md",
        "# API four\n",
        WriteMode::Update {
            expected_hash: &content_hash(b"# External three\n"),
        },
    )?;
    let revisions = list_revisions(source.path(), "existing.md")?;
    let snapshots = revisions
        .iter()
        .map(|revision| {
            get_revision(source.path(), "existing.md", &revision.id).map(|r| r.unwrap().content)
        })
        .collect::<Result<Vec<_>>>()?;
    assert_eq!(
        snapshots,
        [
            "# External one\n",
            "# API two\n",
            "# External three\n",
            "# API four\n"
        ]
    );
    Ok(())
}

#[test]
fn invalid_external_bytes_cannot_be_silently_replaced() -> Result<()> {
    let source = tempfile::tempdir()?;
    let path = source.path().join("note.md");
    for bytes in [vec![0xff, 0xfe], vec![b'a'; 1024 * 1024 + 1]] {
        fs::write(&path, &bytes)?;
        assert!(write_markdown(
            source.path(),
            "note.md",
            "# New\n",
            WriteMode::Update {
                expected_hash: &content_hash(&bytes)
            }
        )
        .is_err());
        assert_eq!(fs::read(&path)?, bytes);
    }
    assert!(!source.path().join(".agent-memory-history").exists());
    Ok(())
}

#[test]
fn missing_history_reads_do_not_create_runtime_state() -> Result<()> {
    let source = tempfile::tempdir()?;
    assert!(list_revisions(source.path(), "missing.md")?.is_empty());
    assert!(get_revision(
        source.path(),
        "missing.md",
        "00000000000000000001-0000000000000000"
    )?
    .is_none());
    assert_eq!(fs::read_dir(source.path())?.count(), 0);
    Ok(())
}

#[test]
fn history_is_not_a_markdown_source_and_tampering_is_detected() -> Result<()> {
    let source = tempfile::tempdir()?;
    let hash = write_markdown(source.path(), "note.md", "# Live\n", WriteMode::Create)?;
    let revision = list_revisions(source.path(), "note.md")?.remove(0);
    fs::write(
        source.path().join(".agent-memory-history/rogue.md"),
        "# Historical secret\n",
    )?;
    assert_eq!(read_source(source.path())?.len(), 1);
    let blob = journal(source.path(), "note.md").join(format!("{hash}.blob"));
    fs::write(&blob, "# Corrupted\n")?;
    assert!(list_revisions(source.path(), "note.md").is_err());
    assert!(get_revision(source.path(), "note.md", &revision.id).is_err());
    assert!(write_markdown(
        source.path(),
        "note.md",
        "# Replacement\n",
        WriteMode::Update {
            expected_hash: &hash
        }
    )
    .is_err());
    assert_eq!(
        fs::read_to_string(source.path().join("note.md"))?,
        "# Live\n"
    );
    Ok(())
}

#[cfg(unix)]
#[test]
fn cloned_readonly_history_needs_no_lock_and_rejects_symlinks() -> Result<()> {
    use std::os::unix::fs::{symlink, PermissionsExt};
    let source = tempfile::tempdir()?;
    write_markdown(source.path(), "note.md", "# Preserved\n", WriteMode::Create)?;
    let first = list_revisions(source.path(), "note.md")?.remove(0);
    fs::remove_file(source.path().join(".agent-memory.lock"))?;
    let directory = journal(source.path(), "note.md");
    fs::set_permissions(&directory, fs::Permissions::from_mode(0o555))?;
    fs::set_permissions(source.path(), fs::Permissions::from_mode(0o555))?;
    let result = get_revision(source.path(), "note.md", &first.id);
    assert!(!source.path().join(".agent-memory.lock").exists());
    fs::set_permissions(source.path(), fs::Permissions::from_mode(0o755))?;
    fs::set_permissions(&directory, fs::Permissions::from_mode(0o755))?;
    assert_eq!(result?.unwrap().content, "# Preserved\n");
    let blob = directory.join(format!("{}.blob", first.content_hash));
    fs::remove_file(&blob)?;
    symlink(source.path().join("note.md"), &blob)?;
    assert!(get_revision(source.path(), "note.md", &first.id).is_err());

    let clean = tempfile::tempdir()?;
    symlink(source.path(), clean.path().join(".agent-memory-history"))?;
    assert!(write_markdown(clean.path(), "new.md", "# New\n", WriteMode::Create).is_err());
    assert!(!clean.path().join("new.md").exists());
    Ok(())
}

#[test]
fn existing_exclusive_lock_blocks_history_reads_without_mutation() -> Result<()> {
    let source = tempfile::tempdir()?;
    write_markdown(source.path(), "note.md", "# One\n", WriteMode::Create)?;
    let file = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(source.path().join(".agent-memory.lock"))?;
    file.try_lock()?;
    assert!(list_revisions(source.path(), "note.md").is_err());
    drop(file);
    assert_eq!(list_revisions(source.path(), "note.md")?.len(), 1);
    Ok(())
}

#[cfg(unix)]
#[test]
fn private_note_snapshots_never_inherit_world_readable_defaults() -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let source = tempfile::tempdir()?;
    let path = source.path().join("private.md");
    fs::write(&path, "# Private original\n")?;
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600))?;
    let hash = content_hash(b"# Private original\n");
    write_markdown(
        source.path(),
        "private.md",
        "# Private update\n",
        WriteMode::Update {
            expected_hash: &hash,
        },
    )?;
    let root = source.path().join(".agent-memory-history");
    let directory = journal(source.path(), "private.md");
    assert_eq!(fs::metadata(&root)?.permissions().mode() & 0o777, 0o700);
    assert_eq!(
        fs::metadata(&directory)?.permissions().mode() & 0o777,
        0o700
    );
    for entry in fs::read_dir(&directory)? {
        assert_eq!(entry?.metadata()?.permissions().mode() & 0o777, 0o600);
    }
    fs::set_permissions(&root, fs::Permissions::from_mode(0o755))?;
    write_markdown(
        source.path(),
        "private.md",
        "# Private third\n",
        WriteMode::Update {
            expected_hash: &content_hash(b"# Private update\n"),
        },
    )?;
    assert_eq!(fs::metadata(&root)?.permissions().mode() & 0o777, 0o700);
    Ok(())
}
