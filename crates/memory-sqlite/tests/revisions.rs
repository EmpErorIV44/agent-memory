use std::fs;

use anyhow::Result;
use memory_sqlite::Store;

const FIRST: &str = "---\nid: durable-document\ncustom: preserved\n---\n# First\nfirsthistorymarker\n\n## Keep\nPreserve the complete document.\n";
const SECOND: &str = "---\nid: durable-document\ncustom: preserved\n---\n# Second\nsecondhistorymarker\n\n## Keep\nPreserve the complete document.\n";

#[test]
fn revisions_survive_index_rebuild_and_restore_appends_without_losing_later_versions() -> Result<()>
{
    let temp = tempfile::tempdir()?;
    let source = temp.path().join("notes");
    fs::create_dir(&source)?;
    let database = temp.path().join("index.sqlite");
    let mut store = Store::open(&database)?;
    store.create_project("demo")?;
    let source_id = store.register_source("demo", &source, true)?;
    assert!(store
        .list_revisions("demo", source_id, "note.md")?
        .is_empty());
    assert_eq!(
        fs::read_dir(&source)?.count(),
        0,
        "reads must not initialize source state"
    );

    let first = store.write("demo", source_id, "note.md", FIRST, None)?;
    assert!(first.indexed);
    assert!(first.history_error.is_none());
    let second = store.write(
        "demo",
        source_id,
        "note.md",
        SECOND,
        Some(&first.content_hash),
    )?;
    let before = store.list_revisions("demo", source_id, "note.md")?;
    assert_eq!(before.len(), 2);
    assert_eq!(before[0].content_hash, first.content_hash);
    assert_eq!(
        before[1].previous_hash.as_deref(),
        Some(first.content_hash.as_str())
    );
    assert_eq!(
        store
            .get_revision("demo", source_id, "note.md", &before[0].id)?
            .unwrap()
            .content,
        FIRST
    );
    assert!(
        store.search("demo", "firsthistorymarker")?.is_empty(),
        "history must never become searchable document content"
    );

    let configuration = store.export_config()?;
    drop(store);
    fs::remove_file(&database)?;
    let mut store = Store::open(&database)?;
    store.rebuild_config(&configuration)?;
    let source_id = store.sources("demo")?[0].source_id;
    assert_eq!(
        serde_json::to_value(store.list_revisions("demo", source_id, "note.md")?)?,
        serde_json::to_value(&before)?
    );
    let restored = store.restore_revision(
        "demo",
        source_id,
        "note.md",
        &before[0].id,
        &second.content_hash,
    )?;
    assert!(restored.indexed);
    assert!(restored.history_error.is_none());
    assert_eq!(fs::read_to_string(source.join("note.md"))?, FIRST);
    let after = store.list_revisions("demo", source_id, "note.md")?;
    assert_eq!(after.len(), 3);
    assert_ne!(
        after[2].id, before[0].id,
        "restore is a new revision of old content"
    );
    assert_eq!(
        after[2].previous_hash.as_deref(),
        Some(second.content_hash.as_str())
    );
    assert_eq!(
        store
            .get_revision("demo", source_id, "note.md", &before[1].id)?
            .unwrap()
            .content,
        SECOND
    );
    assert_eq!(store.search("demo", "firsthistorymarker")?.len(), 1);
    assert!(store.search("demo", "secondhistorymarker")?.is_empty());

    store.write(
        "demo",
        source_id,
        "note.md",
        "# Later\nlaterhistorymarker\n",
        Some(&restored.content_hash),
    )?;
    assert_eq!(store.list_revisions("demo", source_id, "note.md")?.len(), 4);
    Ok(())
}

#[test]
fn history_access_checks_current_source_rights_before_reading_any_revision() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let source = temp.path().join("notes");
    let private = temp.path().join("private");
    fs::create_dir(&source)?;
    fs::create_dir(&private)?;
    let database = temp.path().join("index.sqlite");
    let mut store = Store::open(&database)?;
    for project in ["writer", "reader", "other"] {
        store.create_project(project)?;
    }
    let source_id = store.register_source("writer", &source, true)?;
    store.register_source("reader", &source, false)?;
    let private_id = store.register_source("other", &private, true)?;
    let first = store.write("writer", source_id, "note.md", FIRST, None)?;
    let revision = store
        .list_revisions("writer", source_id, "note.md")?
        .remove(0);
    assert_eq!(
        store
            .get_revision("reader", source_id, "note.md", &revision.id)?
            .unwrap()
            .content,
        FIRST
    );
    // A cloned source can retain its durable history while excluding runtime locks.
    fs::remove_file(source.join(".agent-memory.lock"))?;
    assert_eq!(
        store.list_revisions("reader", source_id, "note.md")?.len(),
        1
    );
    assert_eq!(
        store
            .get_revision("reader", source_id, "note.md", &revision.id)?
            .unwrap()
            .content,
        FIRST
    );
    assert!(
        !source.join(".agent-memory.lock").exists(),
        "history reads must not recreate a runtime lock"
    );
    for project in ["other", "missing"] {
        assert!(store.list_revisions(project, source_id, "note.md").is_err());
        assert!(store
            .get_revision(project, source_id, "note.md", &revision.id)
            .is_err());
        assert!(store
            .restore_revision(
                project,
                source_id,
                "note.md",
                &revision.id,
                &first.content_hash
            )
            .is_err());
    }
    assert!(store
        .get_revision("writer", private_id, "note.md", &revision.id)
        .is_err());
    assert!(store
        .restore_revision(
            "reader",
            source_id,
            "note.md",
            &revision.id,
            &first.content_hash
        )
        .is_err());
    assert!(store
        .list_revisions("reader", source_id, "../note.md")
        .is_err());
    assert!(store
        .get_revision("reader", source_id, "other.md", &revision.id)?
        .is_none());

    let connection = rusqlite::Connection::open(&database)?;
    connection.execute("UPDATE project_sources SET can_read=0 WHERE project_id=(SELECT id FROM projects WHERE name='reader')", [])?;
    assert!(store
        .list_revisions("reader", source_id, "note.md")
        .is_err());
    assert!(store
        .get_revision("reader", source_id, "note.md", &revision.id)
        .is_err());
    assert_eq!(fs::read_to_string(source.join("note.md"))?, FIRST);
    Ok(())
}

#[test]
fn stale_restore_and_invalid_historical_ownership_cannot_overwrite_current_text() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let source = temp.path().join("notes");
    fs::create_dir(&source)?;
    let mut store = Store::open(&temp.path().join("index.sqlite"))?;
    for project in ["demo", "other"] {
        store.create_project(project)?;
        store.register_source(project, &source, true)?;
    }
    let source_id = store.sources("demo")?[0].source_id;
    let first = store.write("demo", source_id, "note.md", FIRST, None)?;
    let old = store
        .list_revisions("demo", source_id, "note.md")?
        .remove(0);
    let second = store.write(
        "demo",
        source_id,
        "note.md",
        SECOND,
        Some(&first.content_hash),
    )?;
    assert!(store
        .restore_revision("demo", source_id, "note.md", &old.id, &first.content_hash)
        .is_err());
    assert_eq!(fs::read_to_string(source.join("note.md"))?, SECOND);
    assert_eq!(store.list_revisions("demo", source_id, "note.md")?.len(), 2);

    let foreign = "---\nid: durable-document\nmemory:\n  id: ownership\n  project: other\n  kind: decision\n  status: accepted\n  summary: Owned by other\n---\n# Foreign\n";
    let foreign_write = store.write(
        "other",
        source_id,
        "note.md",
        foreign,
        Some(&second.content_hash),
    )?;
    let foreign_revision = store
        .list_revisions("other", source_id, "note.md")?
        .pop()
        .unwrap();
    let current = store.write(
        "demo",
        source_id,
        "note.md",
        SECOND,
        Some(&foreign_write.content_hash),
    )?;
    let count = store.list_revisions("demo", source_id, "note.md")?.len();
    assert!(store
        .restore_revision(
            "demo",
            source_id,
            "note.md",
            &foreign_revision.id,
            &current.content_hash
        )
        .is_err());
    assert_eq!(fs::read_to_string(source.join("note.md"))?, SECOND);
    assert_eq!(
        store.list_revisions("demo", source_id, "note.md")?.len(),
        count
    );
    Ok(())
}

#[test]
fn revision_is_retained_when_restore_saves_markdown_but_index_refresh_fails() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let source = temp.path().join("notes");
    fs::create_dir(&source)?;
    let database = temp.path().join("index.sqlite");
    let mut store = Store::open(&database)?;
    store.create_project("demo")?;
    let source_id = store.register_source("demo", &source, true)?;
    let first = store.write("demo", source_id, "note.md", FIRST, None)?;
    let old = store
        .list_revisions("demo", source_id, "note.md")?
        .remove(0);
    let second = store.write(
        "demo",
        source_id,
        "note.md",
        SECOND,
        Some(&first.content_hash),
    )?;
    let connection = rusqlite::Connection::open(&database)?;
    connection.execute_batch("CREATE TRIGGER fail_restore_index BEFORE UPDATE ON documents BEGIN SELECT RAISE(ABORT, 'restore index unavailable'); END;")?;
    let result =
        store.restore_revision("demo", source_id, "note.md", &old.id, &second.content_hash)?;
    assert!(!result.indexed);
    assert!(result
        .index_error
        .unwrap()
        .contains("restore index unavailable"));
    assert!(result.history_error.is_none());
    assert_eq!(fs::read_to_string(source.join("note.md"))?, FIRST);
    assert_eq!(store.list_revisions("demo", source_id, "note.md")?.len(), 3);
    connection.execute_batch("DROP TRIGGER fail_restore_index;")?;
    store.index("demo")?;
    assert_eq!(store.search("demo", "firsthistorymarker")?.len(), 1);
    Ok(())
}

#[test]
fn independent_indexes_cannot_race_restores_over_the_same_current_hash() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let source = temp.path().join("notes");
    fs::create_dir(&source)?;
    let mut original = Store::open(&temp.path().join("original.sqlite"))?;
    original.create_project("demo")?;
    let source_id = original.register_source("demo", &source, true)?;
    let first = original.write("demo", source_id, "note.md", FIRST, None)?;
    let second = original.write(
        "demo",
        source_id,
        "note.md",
        SECOND,
        Some(&first.content_hash),
    )?;
    let third = original.write(
        "demo",
        source_id,
        "note.md",
        "# Third\ncurrenthistorymarker\n",
        Some(&second.content_hash),
    )?;
    let revisions = original.list_revisions("demo", source_id, "note.md")?;
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
    let mut workers = Vec::new();
    for (number, revision) in revisions.iter().take(2).enumerate() {
        let mut worker = Store::open(&temp.path().join(format!("worker-{number}.sqlite")))?;
        worker.create_project("demo")?;
        let worker_source = worker.register_source("demo", &source, true)?;
        worker.index("demo")?;
        let barrier = barrier.clone();
        let revision_id = revision.id.clone();
        let expected_hash = third.content_hash.clone();
        workers.push(std::thread::spawn(move || {
            barrier.wait();
            worker.restore_revision(
                "demo",
                worker_source,
                "note.md",
                &revision_id,
                &expected_hash,
            )
        }));
    }
    let results: Vec<_> = workers
        .into_iter()
        .map(|worker| worker.join().unwrap())
        .collect();
    assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
    let text = fs::read_to_string(source.join("note.md"))?;
    assert!(text == FIRST || text == SECOND);
    assert_eq!(
        original.list_revisions("demo", source_id, "note.md")?.len(),
        4
    );
    Ok(())
}

#[cfg(unix)]
#[test]
fn history_symlink_never_exposes_revisions_outside_registered_source() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let source = temp.path().join("notes");
    fs::create_dir(&source)?;
    let mut store = Store::open(&temp.path().join("index.sqlite"))?;
    store.create_project("demo")?;
    let source_id = store.register_source("demo", &source, true)?;
    store.write("demo", source_id, "note.md", FIRST, None)?;
    let revision = store
        .list_revisions("demo", source_id, "note.md")?
        .remove(0);
    let outside = temp.path().join("external-history");
    fs::rename(source.join(".agent-memory-history"), &outside)?;
    std::os::unix::fs::symlink(&outside, source.join(".agent-memory-history"))?;
    assert!(store.list_revisions("demo", source_id, "note.md").is_err());
    assert!(store
        .get_revision("demo", source_id, "note.md", &revision.id)
        .is_err());
    Ok(())
}
