use anyhow::Result;
use memory_sqlite::Store;
use rusqlite::Connection;
use std::fs;

fn note(id: &str, marker: &str) -> String {
    format!("---\nid: {id}\n---\n# Document\n{marker}\n")
}

#[test]
fn stable_ids_survive_renames_swaps_and_reject_duplicates_atomically() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let root = temp.path().join("notes");
    fs::create_dir(&root)?;
    fs::write(root.join("a.md"), note("doc-a", "markera"))?;
    fs::write(root.join("b.md"), note("doc-b", "markerb"))?;
    fs::write(root.join("legacy.md"), "# Legacy\nlegacymarker\n")?;
    let mut store = Store::open(&temp.path().join("index.sqlite"))?;
    store.create_project("demo")?;
    store.add_source("demo", &root)?;
    assert_eq!(store.index("demo")?.added, 3);
    let original = store.search("demo", "markera")?.remove(0);
    assert_eq!(original.stable_id.as_deref(), Some("doc-a"));
    fs::rename(root.join("a.md"), root.join("moved.md"))?;
    assert_eq!(store.index("demo")?.updated, 1);
    let moved = store.search("demo", "markera")?.remove(0);
    assert_eq!(moved.path, "moved.md");
    assert_eq!(moved.document_id, original.document_id);
    assert_eq!(moved.chunk_id, original.chunk_id);
    assert_eq!(moved.content_hash, original.content_hash);

    // Stable IDs, not the old path's occupant, decide identity during a swap.
    fs::rename(root.join("moved.md"), root.join("swap.tmp"))?;
    fs::rename(root.join("b.md"), root.join("moved.md"))?;
    fs::rename(root.join("swap.tmp"), root.join("b.md"))?;
    assert_eq!(store.index("demo")?.updated, 2);
    let swapped = store.search("demo", "markera")?.remove(0);
    assert_eq!(swapped.document_id, original.document_id);
    assert_eq!(swapped.path, "b.md");

    fs::write(root.join("duplicate.md"), note("doc-a", "duplicatemarker"))?;
    fs::write(root.join("legacy.md"), "# Legacy\nnewlegacymarker\n")?;
    assert!(store.index("demo").is_err());
    assert_eq!(store.search("demo", "legacymarker")?.len(), 1);
    assert!(store.search("demo", "newlegacymarker")?.is_empty());
    assert!(store.search("demo", "duplicatemarker")?.is_empty());
    fs::remove_file(root.join("duplicate.md"))?;
    store.index("demo")?;

    // A replaced explicit ID is a new document; old client chunk references stop resolving.
    fs::write(
        root.join("b.md"),
        note("replacement-id", "replacementmarker"),
    )?;
    store.index("demo")?;
    let replacement = store.search("demo", "replacementmarker")?.remove(0);
    assert_ne!(replacement.document_id, original.document_id);
    assert!(store
        .get("demo", original.chunk_id, &original.content_hash)?
        .is_none());
    Ok(())
}

#[test]
fn version_one_migration_backfills_metadata_without_replacing_document_identity() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let root = temp.path().join("notes");
    fs::create_dir(&root)?;
    let content = note("migrated-id", "migrationmarker");
    fs::write(root.join("existing.md"), &content)?;
    let db = temp.path().join("legacy.sqlite");
    {
        let connection = Connection::open(&db)?;
        connection.execute_batch(include_str!("../../../migrations/001_initial.sql"))?;
        connection.execute("INSERT INTO projects(id,name) VALUES (12,'demo')", [])?;
        connection.execute(
            "INSERT INTO sources(id,root_path) VALUES (23,?1)",
            [root.to_str()],
        )?;
        connection.execute(
            "INSERT INTO project_sources(project_id,source_id) VALUES (12,23)",
            [],
        )?;
        connection.execute("INSERT INTO documents(id,source_id,relative_path,content_hash,file_mtime_ns,byte_size) VALUES (42,23,'existing.md',?1,0,?2)",
            rusqlite::params![memory_core::content_hash(content.as_bytes()), content.len() as i64])?;
        connection.execute("INSERT INTO document_chunks(id,document_id,ordinal,heading,body,start_line,end_line) VALUES (77,42,0,'Document','# Document\nmigrationmarker',4,5)", [])?;
    }
    let mut store = Store::open(&db)?;
    assert_eq!(store.index("demo")?.updated, 1);
    let hit = store.search("demo", "migrationmarker")?.remove(0);
    assert_eq!(hit.document_id, 42);
    assert_eq!(hit.chunk_id, 77);
    assert_eq!(hit.stable_id.as_deref(), Some("migrated-id"));
    let connection = Connection::open(&db)?;
    assert_eq!(
        connection.query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))?,
        4
    );
    connection.execute_batch("PRAGMA foreign_key_check; INSERT INTO document_chunks_fts(document_chunks_fts, rank) VALUES ('integrity-check', 1);")?;
    Ok(())
}

#[test]
fn deleting_and_readding_same_bytes_never_reuses_client_chunk_ids() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let root = temp.path().join("notes");
    fs::create_dir(&root)?;
    let content = note("stable", "identicalmarker");
    fs::write(root.join("note.md"), &content)?;
    let mut store = Store::open(&temp.path().join("index.sqlite"))?;
    store.create_project("demo")?;
    store.add_source("demo", &root)?;
    store.index("demo")?;
    let before = store.search("demo", "identicalmarker")?.remove(0);
    fs::remove_file(root.join("note.md"))?;
    store.index("demo")?;
    fs::write(root.join("another.md"), &content)?;
    store.index("demo")?;
    let after = store.search("demo", "identicalmarker")?.remove(0);
    assert_ne!(before.document_id, after.document_id);
    assert_ne!(before.chunk_id, after.chunk_id);
    assert!(store
        .get("demo", before.chunk_id, &before.content_hash)?
        .is_none());
    Ok(())
}

#[test]
fn concurrent_clients_upgrade_schema_once_under_the_migration_lock() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let db = temp.path().join("legacy.sqlite");
    {
        let connection = Connection::open(&db)?;
        connection.execute_batch("PRAGMA journal_mode=WAL;")?;
        connection.execute_batch(include_str!("../../../migrations/001_initial.sql"))?;
    }
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(4));
    let threads: Vec<_> = (0..4)
        .map(|_| {
            let db = db.clone();
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                barrier.wait();
                Store::open(&db).map(|_| ())
            })
        })
        .collect();
    for thread in threads {
        thread.join().expect("migration worker panicked")?;
    }
    let connection = Connection::open(&db)?;
    assert_eq!(
        connection.query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))?,
        4
    );
    Ok(())
}
