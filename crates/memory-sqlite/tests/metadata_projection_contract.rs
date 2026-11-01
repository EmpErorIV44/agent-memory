use anyhow::Result;
use memory_sqlite::Store;
use std::fs;

#[test]
fn authored_metadata_is_indexed_rebuildable_and_does_not_grant_access() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let root = temp.path().join("notes");
    fs::create_dir(&root)?;
    let content = "---\nid: document\ncreated: '2000-02-29'\nupdated: '2024-02-29'\ntags: [rust, память]\nmemory:\n  id: claim\n  project: demo\n  kind: fact\n  status: accepted\n  summary: Authored metadata\n---\n# Claim\n";
    fs::write(root.join("claim.md"), content)?;
    fs::write(root.join("legacy.md"), "# Legacy\n")?;
    let mut store = Store::open(&temp.path().join("index.sqlite"))?;
    store.create_project("demo")?;
    store.create_project("other")?;
    let source = store.register_source("demo", &root, false)?;
    store.register_source("other", &root, false)?;
    store.index("demo")?;
    let record = serde_json::to_value(store.get_memory("demo", "claim")?.unwrap())?;
    assert_eq!(record["created"], "2000-02-29");
    assert_eq!(record["updated"], "2024-02-29");
    assert_eq!(record["tags"], serde_json::json!(["rust", "память"]));
    assert!(store.get_memory("other", "claim")?.is_none());
    let full = store
        .get_document_by_path(
            "demo",
            source,
            "claim.md",
            record["content_hash"].as_str().unwrap(),
        )?
        .unwrap();
    assert_eq!(serde_json::to_value(full)?["created"], "2000-02-29");
    // MemoryRecord reads remain index-only; filesystem edits require a refresh.
    let updated = content.replace("2024-02-29", "2024-03-01");
    fs::write(root.join("claim.md"), &updated)?;
    assert_eq!(
        serde_json::to_value(store.get_memory("demo", "claim")?.unwrap())?["updated"],
        "2024-02-29"
    );
    assert!(store
        .get_document_by_path(
            "demo",
            source,
            "claim.md",
            record["content_hash"].as_str().unwrap()
        )
        .is_err());
    store.index("demo")?;
    let configuration = store.export_config()?;
    let mut rebuilt = Store::open(&temp.path().join("rebuilt.sqlite"))?;
    rebuilt.rebuild_config(&configuration)?;
    let recovered = serde_json::to_value(rebuilt.get_memory("demo", "claim")?.unwrap())?;
    assert_eq!(recovered["created"], "2000-02-29");
    assert_eq!(recovered["updated"], "2024-03-01");
    assert_eq!(recovered["tags"], record["tags"]);
    let source = rebuilt.sources("demo")?[0].source_id;
    let legacy = rebuilt.search("demo", "Legacy")?.remove(0);
    let legacy = serde_json::to_value(
        rebuilt
            .get_document_by_path("demo", source, "legacy.md", &legacy.content_hash)?
            .unwrap(),
    )?;
    assert!(legacy["created"].is_null());
    assert!(legacy["updated"].is_null());
    assert_eq!(legacy["tags"], serde_json::json!([]));
    let mut revoked = configuration;
    for project in &mut revoked.projects {
        for source in &mut project.sources {
            source.can_read = false;
            source.can_write = false;
        }
    }
    rebuilt.rebuild_config(&revoked)?;
    assert!(rebuilt.get_memory("demo", "claim")?.is_none());
    assert!(rebuilt
        .get_document_by_path(
            "demo",
            source,
            "claim.md",
            recovered["content_hash"].as_str().unwrap()
        )?
        .is_none());
    Ok(())
}
