use anyhow::Result;
use memory_sqlite::Store;
use std::fs;

fn memory(project: &str, id: &str, status: &str, extra: &str) -> String {
    format!("---\nid: doc-{id}\nmemory:\n  id: {id}\n  project: {project}\n  kind: decision\n  status: {status}\n  summary: Decision {id}\n{extra}---\n# Decision\n{id}marker\n")
}

#[test]
fn structured_projection_is_rebuildable_and_project_scoped_including_edges() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let root = temp.path().join("shared");
    let private = temp.path().join("private");
    fs::create_dir(&root)?;
    fs::create_dir(&private)?;
    fs::write(
        root.join("old.md"),
        memory("banking", "old", "superseded", ""),
    )?;
    fs::write(root.join("new.md"), memory("banking", "new", "accepted",
        "  evidence:\n    - kind: document\n      reference: old.md\n    - kind: document\n      reference: private.md\n    - kind: manual\n      reference: User decision\n  relations:\n    - kind: supersedes\n      target: old\n    - kind: depends_on\n      target: private\n    - kind: relates_to\n      target: missing\n"))?;
    fs::write(
        private.join("private.md"),
        memory("banking", "private", "planned", ""),
    )?;
    fs::write(
        root.join("foreign.md"),
        memory("storefront", "foreign", "implemented", ""),
    )?;
    let db = temp.path().join("index.sqlite");
    let mut store = Store::open(&db)?;
    for project in ["banking", "storefront"] {
        store.create_project(project)?;
        store.add_source(project, &root)?;
    }
    let private_id = store.register_source("banking", &private, false)?;
    store.index("banking")?;
    store.index("storefront")?;
    assert_eq!(store.list_memories("banking")?.len(), 3);
    assert_eq!(store.list_memories("storefront")?.len(), 1);
    assert!(store.get_memory("storefront", "new")?.is_none());
    assert!(store.get_memory("banking", "foreign")?.is_none());
    assert_eq!(
        store
            .get_memory("banking", "new")?
            .unwrap()
            .memory
            .relations
            .len(),
        2
    );

    let connection = rusqlite::Connection::open(&db)?;
    connection.execute(
        "UPDATE project_sources SET can_read=0 WHERE source_id=?1",
        [private_id],
    )?;
    let new = store.get_memory("banking", "new")?.unwrap();
    assert_eq!(new.memory.relations.len(), 1);
    assert_eq!(new.memory.relations[0].target, "old");
    assert_eq!(new.memory.evidence.len(), 2);
    assert!(new
        .memory
        .evidence
        .iter()
        .all(|evidence| evidence.reference != "private.md"));
    assert!(store.get_memory("banking", "private")?.is_none());

    // Refreshing one project's shared source retains only explicitly bound ownership.
    store.index("storefront")?;
    assert!(store.get_memory("banking", "new")?.is_some());
    assert!(store.get_memory("storefront", "new")?.is_none());
    let configuration = store.export_config()?;
    drop(store);
    drop(connection);
    fs::remove_file(&db)?;
    let mut rebuilt = Store::open(&db)?;
    rebuilt.rebuild_config(&configuration)?;
    assert_eq!(rebuilt.list_memories("banking")?.len(), 2);
    assert_eq!(
        rebuilt.get_memory("banking", "new")?.unwrap().memory,
        new.memory
    );
    Ok(())
}

#[test]
fn refreshing_shared_source_never_reconciles_another_projects_private_projection() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let shared = temp.path().join("shared");
    let private = temp.path().join("private");
    fs::create_dir(&shared)?;
    fs::create_dir(&private)?;
    fs::write(
        private.join("moving.md"),
        memory("banking", "moving", "accepted", ""),
    )?;
    let mut store = Store::open(&temp.path().join("index.sqlite"))?;
    for project in ["banking", "other"] {
        store.create_project(project)?;
        store.add_source(project, &shared)?;
    }
    store.add_source("banking", &private)?;
    store.index("banking")?;
    fs::rename(private.join("moving.md"), shared.join("moving.md"))?;
    // The other project must neither scan the private source nor fail on its stale record.
    store.index("other")?;
    assert_eq!(store.search("other", "movingmarker")?.len(), 1);
    assert!(store.list_memories("other")?.is_empty());
    store.index("banking")?;
    let records = store.list_memories("banking")?;
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].memory.id, "moving");
    assert_eq!(records[0].path, "moving.md");
    // A shared document can explicitly change ownership; the previous metadata cache
    // must not block the new project or be returned with the changed document's hash.
    fs::write(
        shared.join("moving.md"),
        memory("other", "reassigned", "accepted", ""),
    )?;
    store.index("other")?;
    assert!(store.get_memory("banking", "moving")?.is_none());
    assert!(store.get_memory("other", "reassigned")?.is_some());
    store.index("banking")?;
    assert!(store.list_memories("banking")?.is_empty());
    Ok(())
}

#[test]
fn duplicate_memories_roll_back_and_shared_source_writes_cannot_assign_another_project(
) -> Result<()> {
    let temp = tempfile::tempdir()?;
    let root = temp.path().join("notes");
    fs::create_dir(&root)?;
    let original = memory("demo", "knowledge", "accepted", "");
    fs::write(root.join("original.md"), &original)?;
    let mut store = Store::open(&temp.path().join("index.sqlite"))?;
    store.create_project("demo")?;
    store.create_project("other")?;
    let source_id = store.register_source("demo", &root, true)?;
    store.register_source("other", &root, true)?;
    store.index("demo")?;
    assert!(store
        .write(
            "other",
            source_id,
            "injected.md",
            &memory("demo", "injected", "accepted", ""),
            None
        )
        .is_err());
    assert!(!root.join("injected.md").exists());
    assert!(store
        .write("demo", source_id, "duplicate.md", &original, None)
        .is_err());
    assert!(!root.join("duplicate.md").exists());

    let duplicate = original.replace("id: doc-knowledge", "id: another-document");
    fs::write(root.join("external.md"), duplicate)?;
    fs::write(
        root.join("original.md"),
        original.replace("accepted", "deprecated"),
    )?;
    assert!(store.index("demo").is_err());
    assert_eq!(
        store
            .get_memory("demo", "knowledge")?
            .unwrap()
            .memory
            .status,
        memory_core::MemoryStatus::Accepted
    );
    fs::remove_file(root.join("external.md"))?;
    store.index("demo")?;
    assert_eq!(
        store
            .get_memory("demo", "knowledge")?
            .unwrap()
            .memory
            .status,
        memory_core::MemoryStatus::Deprecated
    );
    Ok(())
}
