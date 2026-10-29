use anyhow::Result;
use memory_sqlite::{
    config::{Config, ProjectConfig, SourceConfig},
    Store,
};
use std::fs;

#[test]
fn rebuild_preserves_live_identity_and_revocation_cannot_reuse_stale_ids() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let root = temp.path().join("notes");
    let replacement_root = temp.path().join("replacement");
    fs::create_dir(&root)?;
    fs::create_dir(&replacement_root)?;
    let content = "---\nid: note-id\n---\n# Note\nregistrymarker\n";
    fs::write(root.join("note.md"), content)?;
    fs::write(replacement_root.join("replacement.md"), content)?;
    let mut store = Store::open(&temp.path().join("index.sqlite"))?;
    store.create_project("demo")?;
    let source_id = store.register_source("demo", &root, true)?;
    store.index("demo")?;
    let initial = store.search("demo", "registrymarker")?.remove(0);
    let mut configuration = store.export_config()?;
    assert_eq!(store.rebuild_config(&configuration)?.unchanged, 1);
    let preserved = store.search("demo", "registrymarker")?.remove(0);
    assert_eq!(preserved.document_id, initial.document_id);
    assert_eq!(preserved.chunk_id, initial.chunk_id);
    assert_eq!(store.sources("demo")?[0].source_id, source_id);

    configuration.projects[0].sources[0].can_read = false;
    configuration.projects[0].sources[0].can_write = false;
    assert_eq!(store.rebuild_config(&configuration)?.deleted, 1);
    assert!(store.search("demo", "registrymarker")?.is_empty());
    assert!(store
        .get("demo", initial.chunk_id, &initial.content_hash)?
        .is_none());
    assert!(store
        .write("demo", source_id, "denied.md", "# Denied", None)
        .is_err());
    configuration.projects[0].sources[0].can_read = true;
    store.rebuild_config(&configuration)?;
    let restored = store.search("demo", "registrymarker")?.remove(0);
    assert_ne!(restored.document_id, initial.document_id);
    assert_ne!(restored.chunk_id, initial.chunk_id);

    configuration.projects[0].sources.clear();
    store.rebuild_config(&configuration)?;
    let replacement = Config {
        version: 1,
        projects: vec![ProjectConfig {
            name: "demo".to_owned(),
            sources: vec![SourceConfig {
                path: replacement_root,
                can_read: true,
                can_write: true,
            }],
        }],
    };
    store.rebuild_config(&replacement)?;
    let new_source = store.sources("demo")?[0].source_id;
    assert_ne!(new_source, source_id);
    assert!(store
        .write("demo", source_id, "stale.md", "# Stale", None)
        .is_err());
    assert!(store
        .get("demo", initial.chunk_id, &initial.content_hash)?
        .is_none());
    Ok(())
}

#[test]
fn failed_rebuild_rolls_back_registration_permissions_and_every_projection() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let original = temp.path().join("original");
    let broken = temp.path().join("broken");
    fs::create_dir(&original)?;
    fs::create_dir(&broken)?;
    fs::write(original.join("original.md"), "# Original\nrollbackmarker\n")?;
    fs::write(
        broken.join("bad.md"),
        "---\nid: [invalid-id]\n---\n# Broken",
    )?;
    let mut store = Store::open(&temp.path().join("index.sqlite"))?;
    store.create_project("demo")?;
    let original_id = store.register_source("demo", &original, true)?;
    store.index("demo")?;
    let before = store.export_config()?;
    let failing = Config {
        version: 1,
        projects: vec![ProjectConfig {
            name: "replacement".to_owned(),
            sources: vec![SourceConfig {
                path: broken,
                can_read: true,
                can_write: false,
            }],
        }],
    };
    assert!(store.rebuild_config(&failing).is_err());
    assert_eq!(
        serde_json::to_value(store.export_config()?)?,
        serde_json::to_value(before)?
    );
    assert!(store.list_memories("replacement").is_err());
    assert_eq!(store.sources("demo")?[0].source_id, original_id);
    assert!(store.sources("demo")?[0].can_write);
    assert_eq!(store.search("demo", "rollbackmarker")?.len(), 1);
    Ok(())
}
