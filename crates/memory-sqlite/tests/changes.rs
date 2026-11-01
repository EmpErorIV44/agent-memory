use anyhow::Result;
use memory_sqlite::Store;
use std::fs;

#[test]
fn changes_survive_clients_and_track_external_edits() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let root = temp.path().join("notes");
    fs::create_dir(&root)?;
    let db = temp.path().join("index.sqlite");
    let mut store = Store::open(&db)?;
    for project in ["one", "two"] {
        store.create_project(project)?;
        store.add_source(project, &root)?;
    }
    let base = store.changes("one", None)?;
    assert_eq!(base.revision, 0);
    assert!(base.changes.is_empty());
    fs::write(root.join("a.md"), "---\nid: stable\n---\n# First\n")?;
    let mut other = Store::open(&db)?;
    other.index("two")?;
    let added = store.changes("one", Some(&base.cursor))?;
    assert_eq!(added.changes.len(), 1);
    assert_eq!(added.changes[0].kind, "added");
    assert!(store.changes("two", Some(&added.cursor)).is_err());
    fs::rename(root.join("a.md"), root.join("b.md"))?;
    fs::write(root.join("b.md"), "---\nid: stable\n---\n# Changed\n")?;
    other.index("two")?;
    let renamed = store.changes("one", Some(&added.cursor))?;
    assert_eq!(renamed.changes[0].kind, "renamed");
    assert_eq!(renamed.changes[0].previous_path.as_deref(), Some("a.md"));
    assert_ne!(
        renamed.changes[0].content_hash,
        added.changes[0].content_hash
    );
    assert_eq!(renamed.changes[0].document_id, added.changes[0].document_id);
    fs::write(root.join("b.md"), "---\nid: stable\n---\n# Updated\n")?;
    let updated = store.changes("one", Some(&renamed.cursor))?;
    assert_eq!(updated.changes[0].kind, "updated");
    fs::remove_file(root.join("b.md"))?;
    let deleted = store.changes("one", Some(&updated.cursor))?;
    assert_eq!(deleted.changes[0].kind, "deleted");
    drop(store);
    let mut store = Store::open(&db)?;
    let unchanged = store.changes("one", Some(&deleted.cursor))?;
    assert_eq!(unchanged.cursor, deleted.cursor);
    assert!(unchanged.changes.is_empty());
    assert_eq!(store.changes("one", Some(&base.cursor))?.changes.len(), 4);
    assert!(store.changes("one", Some("bad")).is_err());
    let future = format!("{}:999", base.cursor.split(':').next().unwrap());
    assert!(store.changes("one", Some(&future)).is_err());
    let rebuilt_db = temp.path().join("rebuilt.sqlite");
    let mut rebuilt = Store::open(&rebuilt_db)?;
    rebuilt.rebuild_config(&store.export_config()?)?;
    assert!(rebuilt.changes("one", Some(&deleted.cursor)).is_err());
    Ok(())
}

#[test]
fn failed_scan_preserves_cursor_and_revoked_sources_do_not_leak() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let root = temp.path().join("notes");
    fs::create_dir(&root)?;
    let mut store = Store::open(&temp.path().join("index.sqlite"))?;
    store.create_project("demo")?;
    store.add_source("demo", &root)?;
    let baseline = store.changes("demo", None)?;
    fs::write(root.join("a.md"), "---\nid: duplicate\n---\n# Secret\n")?;
    fs::write(root.join("b.md"), "---\nid: duplicate\n---\n# Duplicate\n")?;
    assert!(store.changes("demo", Some(&baseline.cursor)).is_err());
    fs::remove_file(root.join("b.md"))?;
    let added = store.changes("demo", Some(&baseline.cursor))?;
    assert_eq!(added.revision, baseline.revision + 1);
    let mut config = store.export_config()?;
    config.projects[0].sources[0].can_read = false;
    store.rebuild_config(&config)?;
    assert!(store
        .changes("demo", Some(&baseline.cursor))?
        .changes
        .is_empty());
    Ok(())
}

#[test]
fn pagination_does_not_skip_events_in_one_revision() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let root = temp.path().join("notes");
    fs::create_dir(&root)?;
    let mut store = Store::open(&temp.path().join("index.sqlite"))?;
    store.create_project("demo")?;
    store.add_source("demo", &root)?;
    let baseline = store.changes("demo", None)?;
    for n in 0..205 {
        fs::write(root.join(format!("{n}.md")), "# New knowledge\n")?;
    }
    let first = store.changes("demo", Some(&baseline.cursor))?;
    assert_eq!(first.changes.len(), 100);
    assert!(first.has_more);
    let hit = &first.changes[0];
    assert!(store
        .get("demo", hit.chunk_id.unwrap(), &hit.content_hash)?
        .is_some());
    let second = store.changes("demo", Some(&first.cursor))?;
    assert_eq!(second.changes.len(), 100);
    assert!(second.has_more);
    let third = store.changes("demo", Some(&second.cursor))?;
    assert_eq!(third.changes.len(), 5);
    assert!(!third.has_more);
    assert_eq!(first.revision, third.revision);
    assert!(first.changes.last().unwrap().sequence < second.changes[0].sequence);
    assert!(second.changes.last().unwrap().sequence < third.changes[0].sequence);
    assert!(store
        .changes("demo", Some(&third.cursor))?
        .changes
        .is_empty());
    Ok(())
}

#[test]
fn vacuum_and_project_removal_preserve_cursors() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let root = temp.path().join("notes");
    fs::create_dir(&root)?;
    let db = temp.path().join("index.sqlite");
    let mut store = Store::open(&db)?;
    for p in ["one", "two"] {
        store.create_project(p)?;
        store.add_source(p, &root)?;
    }
    fs::write(root.join("note.md"), "# First\n")?;
    let base = store.changes("two", None)?;
    fs::write(root.join("note.md"), "# Second\n")?;
    store.index("one")?;
    let connection = rusqlite::Connection::open(&db)?;
    connection
        .execute_batch("PRAGMA foreign_keys=ON; DELETE FROM projects WHERE name='one'; VACUUM;")?;
    let page = store.changes("two", Some(&base.cursor))?;
    assert_eq!(page.changes.len(), 1);
    assert_eq!(page.changes[0].kind, "updated");
    assert!(store.changes("two", Some(&page.cursor))?.changes.is_empty());
    Ok(())
}

#[test]
fn version_four_upgrade_keeps_search_links_and_document_identity() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let root = temp.path().join("notes");
    fs::create_dir(&root)?;
    fs::write(
        root.join("origin.md"),
        "# Origin\n[target](id:stabletarget)\n",
    )?;
    fs::write(
        root.join("target.md"),
        "---\nid: stabletarget\n---\n# Target\n",
    )?;
    let db = temp.path().join("index.sqlite");
    let mut store = Store::open(&db)?;
    store.create_project("demo")?;
    let source = store.register_source("demo", &root, true)?;
    store.index("demo")?;
    let original = store.search("demo", "stabletarget")?.remove(0);
    drop(store);
    let connection = rusqlite::Connection::open(&db)?;
    connection.execute_batch("ALTER TABLE documents DROP COLUMN authored_created; ALTER TABLE documents DROP COLUMN authored_updated; ALTER TABLE documents DROP COLUMN tags_json; DROP TABLE search_exact_fts; DROP TABLE search_russian_fts; DROP TABLE project_changes; DROP TABLE project_snapshot; DROP TABLE project_clocks; PRAGMA user_version=4;")?;
    drop(connection);
    let mut store = Store::open(&db)?;
    let baseline = store.changes("demo", None)?;
    assert!(baseline.changes.is_empty());
    assert_eq!(
        store.search("demo", "stabletarget")?[0].chunk_id,
        original.chunk_id
    );
    fs::rename(root.join("target.md"), root.join("renamed.md"))?;
    let changes = store.changes("demo", Some(&baseline.cursor))?;
    assert_eq!(changes.changes.len(), 1);
    assert_eq!(changes.changes[0].kind, "renamed");
    assert_eq!(changes.changes[0].chunk_id, Some(original.chunk_id));
    assert!(store
        .search("demo", "target")?
        .iter()
        .all(|hit| hit.path != "target.md"));
    assert_eq!(
        store.search("demo", "renamed")?[0].chunk_id,
        original.chunk_id
    );
    let links = store.document_links("demo", source, "origin.md")?.unwrap();
    assert_eq!(
        links.outgoing[0].target_document.as_ref().unwrap().path,
        "renamed.md"
    );
    let connection = rusqlite::Connection::open(&db)?;
    assert_eq!(
        connection.query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))?,
        memory_sqlite::SCHEMA_VERSION
    );
    Ok(())
}
