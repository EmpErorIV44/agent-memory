#![cfg(unix)]

use anyhow::{Context, Result};
use memory_sqlite::{config::Config, Store};
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    fs,
    os::unix::fs::symlink,
    path::{Path, PathBuf},
    process::{Command, Output},
};

fn invoke(database: &Path, args: &[&str], cwd: &Path) -> Result<Output> {
    Ok(Command::new(env!("CARGO_BIN_EXE_agent-memory"))
        .current_dir(cwd)
        .arg("--database")
        .arg(database)
        .args(args)
        .output()?)
}

fn run(database: &Path, args: &[&str], cwd: &Path) -> Result<Value> {
    let result = invoke(database, args, cwd)?;
    anyhow::ensure!(
        result.status.success(),
        "CLI failed: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    Ok(serde_json::from_slice(&result.stdout)?)
}

fn contents(root: &Path) -> Result<BTreeMap<PathBuf, Vec<u8>>> {
    fn visit(root: &Path, relative: &Path, files: &mut BTreeMap<PathBuf, Vec<u8>>) -> Result<()> {
        for entry in fs::read_dir(root.join(relative))? {
            let entry = entry?;
            let path = relative.join(entry.file_name());
            if entry.file_type()?.is_dir() {
                visit(root, &path, files)?;
            } else {
                files.insert(path.clone(), fs::read(root.join(path))?);
            }
        }
        Ok(())
    }
    let mut files = BTreeMap::new();
    visit(root, Path::new(""), &mut files)?;
    Ok(files)
}

fn create_fixture(root: &Path) -> Result<(PathBuf, PathBuf)> {
    let source = root.join("notes");
    fs::create_dir(&source)?;
    fs::write(source.join("note.md"), "# Note\nbackupfixturemarker\n")?;
    let database = root.join("index.sqlite");
    let store = Store::open(&database)?;
    store.create_project("demo")?;
    store.register_source("demo", &source, true)?;
    Ok((database, source))
}

#[test]
fn portable_backup_preserves_live_notes_permissions_history_and_pending_renames() -> Result<()> {
    let temporary = tempfile::tempdir()?;
    let root = temporary.path().canonicalize()?;
    let source = root.join("original-notes");
    fs::create_dir(&source)?;
    let database = root.join("original.sqlite");
    let mut store = Store::open(&database)?;
    for project in ["writer", "reader", "denied"] {
        store.create_project(project)?;
        store.register_source(project, &source, project == "writer")?;
    }
    let source_id = store.sources("writer")?[0].source_id;
    let first = "---\nid: durable-backup-note\n---\n# First\nfirstbackuprevision\n";
    let second = "---\nid: durable-backup-note\n---\n# Second\nsecondbackuprevision\n";
    let written = store.write("writer", source_id, "note.md", first, None)?;
    store.write(
        "writer",
        source_id,
        "note.md",
        second,
        Some(&written.content_hash),
    )?;
    let revisions = store.list_revisions("writer", source_id, "note.md")?;
    assert_eq!(revisions.len(), 2);
    fs::write(
        source.join("from.md"),
        "---\nid: movable-backup-note\n---\n# Move\n",
    )?;
    fs::write(source.join("ref.md"), "[Move](from.md)\n")?;
    let plan = store.rename_preview("writer", source_id, "from.md", "to.md")?;
    // Persist the real format produced before rename publication, as after a crash.
    let journal = serde_json::to_vec(&json!({
        "version": 1, "nonce": "backup-portability-test", "plan": plan
    }))?;
    let operation_id = memory_core::save_operation(&source, &journal)?;
    assert!(!memory_core::operation_complete(&source, &operation_id)?);
    drop(store);
    let connection = rusqlite::Connection::open(&database)?;
    connection.execute(
        "UPDATE project_sources SET can_read=0 WHERE project_id=(SELECT id FROM projects WHERE name='denied')",
        [],
    )?;
    drop(connection);

    // These edits were neither indexed nor committed before the backup.
    let live = "---\nid: durable-backup-note\n---\n# Current\nuncommittedbackupmarker\n";
    fs::write(source.join("note.md"), live)?;
    fs::write(
        source.join(".ordinary-hidden.md"),
        "# Hidden\nhiddenbackupmarker\n",
    )?;
    fs::create_dir(source.join(".git"))?;
    fs::write(
        source.join(".git/private.md"),
        "Git metadata is not memory\n",
    )?;
    fs::write(source.join("attachment.bin"), [0, 255, 42])?;
    fs::write(source.join("cache.sqlite"), b"disposable SQLite cache")?;
    let before = contents(&source)?;
    let archive = root.join("memory.ambackup");
    let created = run(
        &database,
        &[
            "backup",
            "create",
            "--output",
            archive.to_str().context("archive path")?,
        ],
        &root,
    )?;
    assert!(created["files"].as_u64().context("files count")? >= 5);
    assert_eq!(
        contents(&source)?,
        before,
        "backup must not alter source files"
    );
    let unused_database = root.join("unused.sqlite");
    let verified = run(
        &unused_database,
        &["backup", "verify", "--archive", archive.to_str().unwrap()],
        &root,
    )?;
    assert_eq!(created["files"], verified["files"]);
    assert!(!unused_database.exists());
    fs::remove_dir_all(&source)?;
    fs::remove_file(&database)?;

    let destination = root.join("restored");
    run(
        &unused_database,
        &[
            "restore",
            "--archive",
            archive.to_str().unwrap(),
            "--destination",
            destination.to_str().unwrap(),
        ],
        &root,
    )?;
    assert!(!unused_database.exists());
    let owner_config = destination.join("agent-memory.config.json");
    let config: Config = serde_json::from_slice(&fs::read(&owner_config)?)?;
    assert_eq!(config.projects.len(), 3);
    let binding = |name: &str| {
        &config
            .projects
            .iter()
            .find(|project| project.name == name)
            .unwrap()
            .sources[0]
    };
    assert!(binding("writer").path.is_relative());
    assert_eq!(binding("writer").path, binding("reader").path);
    assert_eq!(binding("writer").path, binding("denied").path);
    assert!(binding("writer").can_read && binding("writer").can_write);
    assert!(binding("reader").can_read && !binding("reader").can_write);
    assert!(!binding("denied").can_read && !binding("denied").can_write);
    let restored_source = destination.join(&binding("writer").path);
    assert_eq!(fs::read_to_string(restored_source.join("note.md"))?, live);
    assert!(restored_source.join(".ordinary-hidden.md").is_file());
    for excluded in [
        ".git",
        "attachment.bin",
        "cache.sqlite",
        ".agent-memory.lock",
    ] {
        assert!(
            !restored_source.join(excluded).exists(),
            "unexpected restored {excluded}"
        );
    }
    for (path, bytes) in &before {
        if path.starts_with(".agent-memory-history") || path.starts_with(".agent-memory-operations")
        {
            assert_eq!(fs::read(restored_source.join(path))?, *bytes);
        }
    }

    let rebuilt_database = destination.join("index.sqlite");
    let rebuilt = invoke(
        &rebuilt_database,
        &["rebuild", "--config", owner_config.to_str().unwrap()],
        &root,
    )?;
    anyhow::ensure!(
        rebuilt.status.success(),
        "{}",
        String::from_utf8_lossy(&rebuilt.stderr)
    );
    let mut restored = Store::open(&rebuilt_database)?;
    let restored_id = restored.sources("writer")?[0].source_id;
    assert_eq!(
        restored.search("writer", "uncommittedbackupmarker")?.len(),
        1
    );
    assert_eq!(restored.search("reader", "hiddenbackupmarker")?.len(), 1);
    assert!(restored
        .search("denied", "uncommittedbackupmarker")?
        .is_empty());
    assert_eq!(
        restored.search("writer", "uncommittedbackupmarker")?[0]
            .stable_id
            .as_deref(),
        Some("durable-backup-note")
    );
    assert_eq!(
        serde_json::to_value(restored.list_revisions("reader", restored_id, "note.md")?)?,
        serde_json::to_value(&revisions)?
    );
    assert_eq!(
        restored
            .get_revision("reader", restored_id, "note.md", &revisions[0].id)?
            .unwrap()
            .content,
        first
    );
    assert_eq!(
        memory_core::read_operation(&restored_source, &operation_id)?,
        journal
    );
    let resumed = restored.rename_resume("writer", restored_id, &operation_id)?;
    assert!(resumed.complete, "{resumed:?}");
    assert!(!restored_source.join("from.md").exists());
    assert!(restored_source.join("to.md").is_file());
    assert_eq!(
        fs::read_to_string(restored_source.join("ref.md"))?,
        "[Move](to.md)\n"
    );
    Ok(())
}

#[test]
fn backup_requires_existing_database_and_never_overwrites_outputs_or_restore_targets() -> Result<()>
{
    let temporary = tempfile::tempdir()?;
    let root = temporary.path().canonicalize()?;
    let missing = root.join("missing.sqlite");
    let archive = root.join("memory.ambackup");
    assert!(!invoke(
        &missing,
        &["backup", "create", "--output", archive.to_str().unwrap()],
        &root
    )?
    .status
    .success());
    assert!(!missing.exists());
    assert!(!archive.exists());
    let (database, _) = create_fixture(&root)?;
    run(
        &database,
        &["backup", "create", "--output", archive.to_str().unwrap()],
        &root,
    )?;
    let original_archive = fs::read(&archive)?;
    assert!(!invoke(
        &database,
        &["backup", "create", "--output", archive.to_str().unwrap()],
        &root
    )?
    .status
    .success());
    assert_eq!(fs::read(&archive)?, original_archive);
    let output_link = root.join("existing-output-link");
    symlink(&archive, &output_link)?;
    assert!(!invoke(
        &database,
        &[
            "backup",
            "create",
            "--output",
            output_link.to_str().unwrap()
        ],
        &root
    )?
    .status
    .success());
    assert_eq!(fs::read(&archive)?, original_archive);

    let existing_dir = root.join("existing-directory");
    fs::create_dir(&existing_dir)?;
    fs::write(existing_dir.join("keep.md"), "keep directory content")?;
    let existing_file = root.join("existing-file");
    fs::write(&existing_file, "keep file content")?;
    let existing_link = root.join("existing-link");
    symlink(&existing_dir, &existing_link)?;
    for target in [&existing_dir, &existing_file, &existing_link] {
        assert!(!invoke(
            &missing,
            &[
                "restore",
                "--archive",
                archive.to_str().unwrap(),
                "--destination",
                target.to_str().unwrap()
            ],
            &root
        )?
        .status
        .success());
        assert!(!missing.exists());
    }
    assert_eq!(
        fs::read_to_string(existing_dir.join("keep.md"))?,
        "keep directory content"
    );
    assert_eq!(fs::read_to_string(&existing_file)?, "keep file content");
    assert_eq!(fs::read_link(&existing_link)?, existing_dir);
    Ok(())
}

#[test]
fn corrupt_truncated_and_extended_archives_cannot_publish_restored_data() -> Result<()> {
    let temporary = tempfile::tempdir()?;
    let root = temporary.path().canonicalize()?;
    let (database, _) = create_fixture(&root)?;
    let archive = root.join("memory.ambackup");
    run(
        &database,
        &["backup", "create", "--output", archive.to_str().unwrap()],
        &root,
    )?;
    let bytes = fs::read(&archive)?;
    let mut changed_payload = bytes.clone();
    *changed_payload.last_mut().context("archive payload")? ^= 1;
    let mut changed_header = bytes.clone();
    changed_header[0] ^= 1;
    let mut trailing = bytes.clone();
    trailing.extend_from_slice(b"unexpected trailing bytes");
    let missing = root.join("must-not-create.sqlite");
    for (index, malformed) in [
        changed_payload,
        changed_header,
        bytes[..bytes.len() - 1].to_vec(),
        trailing,
    ]
    .into_iter()
    .enumerate()
    {
        let invalid = root.join(format!("invalid-{index}.ambackup"));
        fs::write(&invalid, malformed)?;
        let destination = root.join(format!("rejected-{index}"));
        assert!(!invoke(
            &missing,
            &["backup", "verify", "--archive", invalid.to_str().unwrap()],
            &root
        )?
        .status
        .success());
        assert!(!invoke(
            &missing,
            &[
                "restore",
                "--archive",
                invalid.to_str().unwrap(),
                "--destination",
                destination.to_str().unwrap()
            ],
            &root
        )?
        .status
        .success());
        assert!(
            !destination.exists(),
            "invalid archive published a restore directory"
        );
        assert!(!missing.exists());
    }
    Ok(())
}
