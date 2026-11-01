use anyhow::Result;
use memory_sqlite::{SearchOptions, Store};
use std::fs;

#[test]
fn embedding_inputs_and_cached_vectors_obey_project_scope_and_live_permissions() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let notes = temp.path().join("shared");
    let private = temp.path().join("private");
    fs::create_dir(&notes)?;
    fs::create_dir(&private)?;
    fs::write(notes.join("shared.md"),"---\ntitle: Shared title\nmemory:\n  id: foreign\n  project: other\n  kind: fact\n  status: accepted\n  summary: foreignsummarysecret\n  evidence:\n    - kind: manual\n      reference: privateevidencesecret\n---\n# Shared body\nAllowed body text.\n")?;
    fs::write(
        private.join("private.md"),
        "# Private\nprivatesourcesecret\n",
    )?;
    let database = temp.path().join("memory.sqlite");
    let mut store = Store::open(&database)?;
    store.create_project("demo")?;
    store.create_project("other")?;
    let source = store.register_source("demo", &notes, false)?;
    store.register_source("other", &notes, false)?;
    store.register_source("other", &private, false)?;
    store.index("other")?;
    store.index("demo")?;
    let candidates = store.semantic_candidates("demo", &SearchOptions::default())?;
    assert_eq!(candidates.len(), 1);
    let candidate = &candidates[0];
    assert!(candidate.input.contains("Allowed body text"));
    for forbidden in [
        "foreignsummarysecret",
        "privateevidencesecret",
        "privatesourcesecret",
    ] {
        assert!(!candidate.input.contains(forbidden));
    }
    assert!(store
        .semantic_candidates("other", &SearchOptions::default())?
        .iter()
        .any(|c| c.input.contains("foreignsummarysecret")));
    let filtered = SearchOptions::from_filters(Some("fact"), Some("accepted"))?;
    assert!(store.semantic_candidates("demo", &filtered)?.is_empty());
    assert!(store.semantic_cache("demo", "model-v1", candidate, &[3.0, 4.0])?);
    let cached = store
        .semantic_cached("demo", "model-v1", candidate)?
        .unwrap();
    assert!((cached[0] - 0.6).abs() < 1e-6 && (cached[1] - 0.8).abs() < 1e-6);
    assert!(store
        .semantic_cached("demo", "model-v2", candidate)?
        .is_none());
    assert!(store
        .semantic_cached("other", "model-v1", candidate)?
        .is_none());
    assert!(store
        .semantic_search("other", "model-v1", &[1.0, 0.0], &SearchOptions::default())?
        .is_empty());
    let connection = rusqlite::Connection::open(&database)?;
    connection.execute("UPDATE project_sources SET can_read=0 WHERE project_id=(SELECT id FROM projects WHERE name='demo') AND source_id=?1",[source])?;
    assert!(store
        .semantic_candidates("demo", &SearchOptions::default())?
        .is_empty());
    assert!(store
        .semantic_cached("demo", "model-v1", candidate)?
        .is_none());
    assert!(!store.semantic_cache("demo", "model-v1", candidate, &[1.0, 0.0])?);
    assert!(store
        .semantic_search("demo", "model-v1", &[1.0, 0.0], &SearchOptions::default())?
        .is_empty());
    Ok(())
}

#[test]
fn stale_or_forged_inputs_cannot_publish_and_vectors_do_not_survive_rebuild() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let root = temp.path().join("notes");
    fs::create_dir(&root)?;
    fs::write(root.join("note.md"), "# First\noriginal\n")?;
    let mut store = Store::open(&temp.path().join("index.sqlite"))?;
    store.create_project("demo")?;
    store.register_source("demo", &root, false)?;
    store.index("demo")?;
    let candidate = store
        .semantic_candidates("demo", &SearchOptions::default())?
        .remove(0);
    let mut forged = candidate.clone();
    forged.input.push_str(" forged");
    forged.input_hash = memory_core::content_hash(forged.input.as_bytes());
    assert!(!store.semantic_cache("demo", "model", &forged, &[1.0, 0.0])?);
    assert!(store.semantic_cache("demo", "model", &candidate, &[1.0, 0.0])?);
    fs::write(root.join("note.md"), "# Second\nnew content\n")?;
    store.index("demo")?;
    assert!(store
        .semantic_cached("demo", "model", &candidate)?
        .is_none());
    assert!(!store.semantic_cache("demo", "model", &candidate, &[1.0, 0.0])?);
    assert!(store
        .semantic_search("demo", "model", &[1.0, 0.0], &SearchOptions::default())?
        .is_empty());
    let current = store
        .semantic_candidates("demo", &SearchOptions::default())?
        .remove(0);
    assert!(store.semantic_cache("demo", "model", &current, &[1.0, 0.0])?);
    let config = store.export_config()?;
    let mut rebuilt = Store::open(&temp.path().join("rebuilt.sqlite"))?;
    rebuilt.rebuild_config(&config)?;
    assert_eq!(
        rebuilt
            .semantic_candidates("demo", &SearchOptions::default())?
            .len(),
        1
    );
    assert!(rebuilt
        .semantic_search("demo", "model", &[1.0, 0.0], &SearchOptions::default())?
        .is_empty());
    Ok(())
}

#[test]
fn cosine_ranking_filters_and_model_dimensions_are_validated() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let root = temp.path().join("notes");
    fs::create_dir(&root)?;
    fs::write(root.join("accepted.md"),"---\nmemory:\n  id: accepted\n  project: demo\n  kind: decision\n  status: accepted\n  summary: Accepted\n---\n# Accepted\n")?;
    fs::write(root.join("deprecated.md"),"---\nmemory:\n  id: deprecated\n  project: demo\n  kind: fact\n  status: deprecated\n  summary: Deprecated\n---\n# Deprecated\n")?;
    let mut store = Store::open(&temp.path().join("index.sqlite"))?;
    store.create_project("demo")?;
    store.register_source("demo", &root, false)?;
    store.index("demo")?;
    let candidates = store.semantic_candidates("demo", &SearchOptions::default())?;
    for candidate in &candidates {
        let vector = if candidate.hit.path == "accepted.md" {
            [1.0, 0.0]
        } else {
            [0.0, 1.0]
        };
        assert!(store.semantic_cache("demo", "model", candidate, &vector)?);
    }
    let found = store.semantic_search("demo", "model", &[1.0, 0.0], &SearchOptions::default())?;
    assert_eq!(found[0].hit.path, "accepted.md");
    assert!((found[0].similarity - 1.0).abs() < 1e-6);
    assert!(found[1].similarity.abs() < 1e-6);
    let accepted = SearchOptions::from_filters(Some("decision"), Some("accepted"))?;
    assert_eq!(
        store
            .semantic_search("demo", "model", &[1.0, 0.0], &accepted)?
            .len(),
        1
    );
    for vector in [
        vec![],
        vec![0.0, 0.0],
        vec![f32::NAN, 1.0],
        vec![f32::INFINITY, 0.0],
        vec![1.0; 8193],
    ] {
        assert!(store
            .semantic_cache("demo", "bad-model", &candidates[0], &vector)
            .is_err());
        assert!(store
            .semantic_search("demo", "bad-model", &vector, &SearchOptions::default())
            .is_err());
    }
    assert!(store
        .semantic_cache("demo", "model", &candidates[0], &[1.0, 0.0, 0.0])
        .is_err());
    assert!(store
        .semantic_search("demo", "model", &[1.0], &SearchOptions::default())
        .is_err());
    assert!(store
        .semantic_cache("demo", "", &candidates[0], &[1.0, 0.0])
        .is_err());
    assert!(store
        .semantic_cache("demo", "model\nkey", &candidates[0], &[1.0, 0.0])
        .is_err());
    Ok(())
}
