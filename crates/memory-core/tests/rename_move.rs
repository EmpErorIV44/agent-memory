use anyhow::Result;
use memory_core::{content_hash, move_markdown};
use std::fs;

#[test]
fn guarded_move_rejects_stale_hash_and_never_overwrites_destination() -> Result<()> {
    let temp = tempfile::tempdir()?;
    fs::write(temp.path().join("from.md"), "# Original\n")?;
    fs::write(temp.path().join("to.md"), "# Existing\n")?;
    let hash = content_hash(b"# Original\n");
    assert!(move_markdown(temp.path(), "from.md", "to.md", "# Moved\n", &hash).is_err());
    assert_eq!(
        fs::read_to_string(temp.path().join("from.md"))?,
        "# Original\n"
    );
    assert_eq!(
        fs::read_to_string(temp.path().join("to.md"))?,
        "# Existing\n"
    );
    assert!(move_markdown(
        temp.path(),
        "from.md",
        "new.md",
        "# Moved\n",
        &content_hash(b"stale")
    )
    .is_err());
    assert!(!temp.path().join("new.md").exists());
    Ok(())
}

#[test]
fn completed_move_preserves_old_history_and_starts_destination_history() -> Result<()> {
    let temp = tempfile::tempdir()?;
    fs::write(temp.path().join("from.md"), "# Original\n")?;
    move_markdown(
        temp.path(),
        "from.md",
        "deep/to.md",
        "# Moved\n",
        &content_hash(b"# Original\n"),
    )?;
    assert!(!temp.path().join("from.md").exists());
    assert_eq!(
        fs::read_to_string(temp.path().join("deep/to.md"))?,
        "# Moved\n"
    );
    let old = memory_core::list_revisions(temp.path(), "from.md")?;
    let new = memory_core::list_revisions(temp.path(), "deep/to.md")?;
    assert_eq!(old.len(), 1);
    assert_eq!(new.len(), 1);
    assert_eq!(old[0].content_hash, content_hash(b"# Original\n"));
    assert_eq!(new[0].content_hash, content_hash(b"# Moved\n"));
    Ok(())
}
