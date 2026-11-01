use anyhow::Result;
use std::fs;

#[test]
fn immutable_operation_checksums_and_completion_markers_survive_reopen() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let root = temp.path();
    assert!(memory_core::list_operations(root)?.is_empty());
    assert!(!root.join(".agent-memory-operations").exists());
    let bytes = br#"{"operation":"owner-reviewed"}"#;
    let id = memory_core::save_operation(root, bytes)?;
    assert_eq!(memory_core::save_operation(root, bytes)?, id);
    assert_eq!(memory_core::list_operations(root)?, vec![id.clone()]);
    assert_eq!(memory_core::read_operation(root, &id)?, bytes);
    assert!(!memory_core::operation_complete(root, &id)?);
    memory_core::finish_operation(root, &id)?;
    memory_core::finish_operation(root, &id)?;
    assert!(memory_core::operation_complete(root, &id)?);
    let dir = root.join(".agent-memory-operations");
    fs::write(dir.join(format!("{id}.done")), b"wrong")?;
    assert!(memory_core::operation_complete(root, &id).is_err());
    fs::write(dir.join(format!("{id}.json")), b"tampered")?;
    assert!(memory_core::read_operation(root, &id).is_err());
    assert!(memory_core::finish_operation(root, &id).is_err());
    assert!(memory_core::read_operation(root, "../../escape").is_err());
    Ok(())
}

#[cfg(unix)]
#[test]
fn operation_storage_refuses_symlink_directory_record_and_marker() -> Result<()> {
    use std::os::unix::fs::symlink;
    let temp = tempfile::tempdir()?;
    let root = temp.path().join("notes");
    let outside = temp.path().join("outside");
    fs::create_dir(&root)?;
    fs::create_dir(&outside)?;
    let dir = root.join(".agent-memory-operations");
    symlink(&outside, &dir)?;
    assert!(memory_core::save_operation(&root, b"reviewed").is_err());
    assert!(memory_core::list_operations(&root).is_err());
    assert!(fs::read_dir(&outside)?.next().is_none());
    fs::remove_file(&dir)?;
    let id = memory_core::save_operation(&root, b"reviewed")?;
    let record = dir.join(format!("{id}.json"));
    let external = outside.join("record");
    fs::write(&external, b"reviewed")?;
    fs::remove_file(&record)?;
    symlink(&external, &record)?;
    assert!(memory_core::read_operation(&root, &id).is_err());
    assert!(memory_core::list_operations(&root).is_err());
    fs::remove_file(&record)?;
    memory_core::save_operation(&root, b"reviewed")?;
    symlink(&external, dir.join(format!("{id}.done")))?;
    assert!(memory_core::operation_complete(&root, &id).is_err());
    assert!(memory_core::finish_operation(&root, &id).is_err());
    assert_eq!(fs::read(&external)?, b"reviewed");
    Ok(())
}
