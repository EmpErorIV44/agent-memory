use anyhow::Result;
use memory_sqlite::Store;
use rusqlite::Connection;
use std::fs;

const NOTE: &str = "---\nid: doc-note\ntitle: Authored title\ncreated: 2024-02-29\nupdated: 2026-11-02\ntags: [architecture, rust]\nmemory:\n  id: claim\n  project: demo\n  kind: fact\n  status: accepted\n  summary: Metadata example\n---\n# Note\nmetadatamarker\n";

#[test]
fn authored_metadata_survives_index_rebuild_and_preserves_read_scope() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let root = temp.path().join("notes");
    fs::create_dir(&root)?;
    fs::write(root.join("note.md"), NOTE)?;
    let db = temp.path().join("index.sqlite");
    let mut store = Store::open(&db)?;
    store.create_project("demo")?;
    store.create_project("other")?;
    let source = store.register_source("demo", &root, false)?;
    store.index("demo")?;
    assert_eq!(store.search("demo", "title:Authored")?.len(), 1);
    assert!(store.search("other", "title:Authored")?.is_empty());
    let first = store.get_memory("demo", "claim")?.unwrap();
    assert_eq!(first.created.as_deref(), Some("2024-02-29"));
    assert_eq!(first.updated.as_deref(), Some("2026-11-02"));
    assert_eq!(first.tags, ["architecture", "rust"]);
    let document = store
        .get_document_by_path("demo", source, "note.md", &first.content_hash)?
        .unwrap();
    assert_eq!(document.created, first.created);
    assert_eq!(document.updated, first.updated);
    assert_eq!(document.tags, first.tags);
    assert_eq!(document.title.as_deref(), Some("Authored title"));
    assert_eq!(document.content, NOTE);
    assert!(store.get_memory("other", "claim")?.is_none());
    assert!(store
        .get_document_by_path("other", source, "note.md", &first.content_hash)?
        .is_none());
    let config = store.export_config()?;
    store.rebuild_config(&config)?;
    let rebuilt = store.get_memory("demo", "claim")?.unwrap();
    assert_eq!(rebuilt.document_id, first.document_id);
    assert_eq!(rebuilt.created, first.created);
    assert_eq!(rebuilt.updated, first.updated);
    assert_eq!(rebuilt.tags, first.tags);
    fs::rename(root.join("note.md"), root.join("moved.md"))?;
    store.index("demo")?;
    let moved = store.get_memory("demo", "claim")?.unwrap();
    assert_eq!(moved.document_id, first.document_id);
    assert_eq!(moved.created, first.created);
    assert_eq!(moved.tags, first.tags);
    // Structured list/get remain index-only and do not reread changed source text.
    fs::write(root.join("moved.md"), "# External edit\n")?;
    assert_eq!(store.list_memories("demo")?[0].updated, first.updated);
    assert!(store
        .get_document_by_path("demo", source, "moved.md", &first.content_hash)
        .is_err());
    let connection = Connection::open(&db)?;
    connection.execute(
        "UPDATE project_sources SET can_read=0 WHERE source_id=?1",
        [source],
    )?;
    assert!(store.list_memories("demo")?.is_empty());
    assert!(store.get_memory("demo", "claim")?.is_none());
    Ok(())
}

#[test]
fn schema_seven_backfills_unchanged_files_on_next_scan_without_rewriting_markdown() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let root = temp.path().join("notes");
    fs::create_dir(&root)?;
    fs::write(root.join("note.md"), NOTE)?;
    let db = temp.path().join("index.sqlite");
    let mut store = Store::open(&db)?;
    store.create_project("demo")?;
    store.add_source("demo", &root)?;
    store.index("demo")?;
    let initial = store.get_memory("demo", "claim")?.unwrap();
    drop(store);
    // Model a pre-v7 index containing an already indexed, unchanged source file.
    let connection = Connection::open(&db)?;
    connection.execute_batch("ALTER TABLE documents DROP COLUMN authored_created; ALTER TABLE documents DROP COLUMN authored_updated; ALTER TABLE documents DROP COLUMN tags_json; PRAGMA user_version=6;")?;
    drop(connection);
    let mut store = Store::open(&db)?;
    let migrated = store.get_memory("demo", "claim")?.unwrap();
    assert_eq!(migrated.created, None);
    assert!(migrated.tags.is_empty());
    assert_eq!(store.index("demo")?.unchanged, 1);
    let refreshed = store.get_memory("demo", "claim")?.unwrap();
    assert_eq!(refreshed.created, initial.created);
    assert_eq!(refreshed.updated, initial.updated);
    assert_eq!(refreshed.tags, initial.tags);
    assert_eq!(refreshed.document_id, initial.document_id);
    assert_eq!(refreshed.content_hash, initial.content_hash);
    assert_eq!(fs::read_to_string(root.join("note.md"))?, NOTE);
    Ok(())
}
