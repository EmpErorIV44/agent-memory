use anyhow::Result;
use memory_sqlite::Store;
use std::fs;

#[test]
fn writes_require_source_permission_and_refresh_search() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let root = temp.path().join("notes");
    fs::create_dir(&root)?;
    let mut store = Store::open(&temp.path().join("index.sqlite"))?;
    store.create_project("banking")?;
    store.create_project("other")?;
    let source_id = store.register_source("banking", &root, false)?;
    assert!(store
        .write("banking", source_id, "note.md", "# No permission", None)
        .is_err());
    assert!(!root.join("note.md").exists());
    assert!(!root.join(".agent-memory.lock").exists());
    assert_eq!(store.register_source("banking", &root, true)?, source_id);
    assert!(store
        .write("other", source_id, "note.md", "# Wrong project", None)
        .is_err());
    let content =
        "---\nid: note\n---\n# First\nbankwritemarker\n\n## Keep\nPreserve this section.\n";
    let result = store.write("banking", source_id, "note.md", content, None)?;
    assert!(result.indexed);
    assert!(result.index_error.is_none());
    let hit = store.search("banking", "bankwritemarker")?.remove(0);
    assert!(store.search("other", "bankwritemarker")?.is_empty());
    let document = store
        .get_document("banking", hit.chunk_id, &hit.content_hash)?
        .unwrap();
    assert_eq!(document.content, content);
    assert!(store
        .get_document("other", hit.chunk_id, &hit.content_hash)?
        .is_none());
    let changed = document
        .content
        .replace("bankwritemarker", "bankupdatedmarker");
    assert!(
        store
            .write(
                "banking",
                source_id,
                "note.md",
                &changed,
                Some(&document.content_hash)
            )?
            .indexed
    );
    assert!(store.search("banking", "bankwritemarker")?.is_empty());
    assert_eq!(store.search("banking", "bankupdatedmarker")?.len(), 1);
    assert!(store
        .write(
            "banking",
            source_id,
            "note.md",
            "lost update",
            Some(&document.content_hash)
        )
        .is_err());
    assert_eq!(fs::read_to_string(root.join("note.md"))?, changed);

    // Human edits are detected from Markdown bytes even if SQLite hasn't been refreshed.
    let hit = store.search("banking", "bankupdatedmarker")?.remove(0);
    fs::write(root.join("note.md"), "# Human edit\nmanualmarker\n")?;
    assert!(store
        .write(
            "banking",
            source_id,
            "note.md",
            "overwrite manual edit",
            Some(&hit.content_hash)
        )
        .is_err());
    assert!(store
        .get_document("banking", hit.chunk_id, &hit.content_hash)
        .is_err());
    store.register_source("banking", &root, false)?;
    assert!(!store.sources("banking")?[0].can_write);
    assert!(store
        .write("banking", source_id, "new.md", "# Revoked", None)
        .is_err());
    Ok(())
}

#[test]
fn saved_markdown_is_reported_even_when_indexing_fails() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let root = temp.path().join("notes");
    fs::create_dir(&root)?;
    let db = temp.path().join("index.sqlite");
    let mut store = Store::open(&db)?;
    store.create_project("demo")?;
    let source_id = store.register_source("demo", &root, true)?;
    let connection = rusqlite::Connection::open(&db)?;
    connection.execute_batch("CREATE TRIGGER fail_insert BEFORE INSERT ON documents BEGIN SELECT RAISE(ABORT, 'injected index failure'); END;")?;
    let result = store.write(
        "demo",
        source_id,
        "saved.md",
        "# Saved\nsavedmarker\n",
        None,
    )?;
    assert!(!result.indexed);
    assert!(result
        .index_error
        .unwrap()
        .contains("injected index failure"));
    assert_eq!(
        fs::read_to_string(root.join("saved.md"))?,
        "# Saved\nsavedmarker\n"
    );
    assert!(store.search("demo", "savedmarker")?.is_empty());
    connection.execute_batch("DROP TRIGGER fail_insert;")?;
    assert_eq!(store.index("demo")?.added, 1);
    assert_eq!(store.search("demo", "savedmarker")?.len(), 1);
    Ok(())
}
