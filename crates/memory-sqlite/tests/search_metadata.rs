use anyhow::Result;
use memory_sqlite::Store;
use std::fs;

fn note(id: &str, summary: &str) -> String {
    format!("---\nid: stableidentifier\nmemory:\n  id: {id}\n  project: demo\n  kind: constraint\n  status: accepted\n  summary: {summary}\n---\n# General\nПринятое правило. ordinarybody\n")
}

#[test]
fn metadata_search_tracks_rename_update_delete_and_rebuild() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let root = temp.path().join("notes");
    fs::create_dir_all(root.join("preferences"))?;
    fs::write(
        root.join("preferences/initial.md"),
        note("memoryidentifier", "summarymarker"),
    )?;
    let db = temp.path().join("index.sqlite");
    let mut store = Store::open(&db)?;
    store.create_project("demo")?;
    store.add_source("demo", &root)?;
    store.index("demo")?;
    let original = store.search("demo", "ordinarybody")?.remove(0);
    for query in [
        "preferences",
        "stableidentifier",
        "memoryidentifier",
        "summarymarker",
        "правил*",
    ] {
        let hits = store.search("demo", query)?;
        assert_eq!(hits.len(), 1, "{query}");
        assert_eq!(hits[0].chunk_id, original.chunk_id);
        assert_eq!(hits[0].body, original.body);
    }
    // Matching the same chunk through both indexes must not duplicate it.
    assert_eq!(
        store.search("demo", "ordinarybody OR preferences")?.len(),
        1
    );
    assert_eq!(store.search("demo", "heading:General")?.len(), 1);
    assert_eq!(store.search("demo", "body:ordinarybody")?.len(), 1);
    fs::rename(
        root.join("preferences/initial.md"),
        root.join("renamedmarker.md"),
    )?;
    store.index("demo")?;
    assert!(store.search("demo", "preferences")?.is_empty());
    assert_eq!(
        store.search("demo", "renamedmarker")?[0].chunk_id,
        original.chunk_id
    );
    fs::write(
        root.join("renamedmarker.md"),
        note("replacementmemory", "newsummary"),
    )?;
    store.index("demo")?;
    assert!(store
        .search("demo", "summarymarker OR memoryidentifier")?
        .is_empty());
    assert_eq!(store.search("demo", "newsummary")?.len(), 1);
    let config = store.export_config()?;
    store.rebuild_config(&config)?;
    assert_eq!(store.search("demo", "newsummary")?.len(), 1);
    fs::remove_file(root.join("renamedmarker.md"))?;
    store.index("demo")?;
    assert!(store
        .search("demo", "newsummary OR renamedmarker")?
        .is_empty());
    Ok(())
}

#[test]
fn shared_source_metadata_respects_ownership_and_read_permissions() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let root = temp.path().join("shared");
    fs::create_dir(&root)?;
    fs::write(
        root.join("commonpath.md"),
        note("privatememorymarker", "privatesummarymarker"),
    )?;
    let db = temp.path().join("index.sqlite");
    let mut store = Store::open(&db)?;
    for project in ["demo", "other"] {
        store.create_project(project)?;
        store.add_source(project, &root)?;
    }
    store.index("demo")?;
    store.index("other")?;
    for query in ["privatememorymarker", "privatesummarymarker"] {
        assert_eq!(store.search("demo", query)?.len(), 1);
        assert!(store.search("other", query)?.is_empty());
    }
    for query in ["commonpath", "stableidentifier", "ordinarybody"] {
        assert_eq!(store.search("other", query)?.len(), 1);
    }
    let connection = rusqlite::Connection::open(&db)?;
    connection.execute("UPDATE project_sources SET can_read=0 WHERE project_id=(SELECT id FROM projects WHERE name='demo')", [])?;
    assert!(store
        .search("demo", "privatesummarymarker OR commonpath")?
        .is_empty());
    assert_eq!(store.search("other", "commonpath")?.len(), 1);
    Ok(())
}

#[test]
fn v2_migration_backfills_existing_metadata_without_reindexing() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let db = temp.path().join("legacy.sqlite");
    let connection = rusqlite::Connection::open(&db)?;
    connection.execute_batch(include_str!("../../../migrations/001_initial.sql"))?;
    connection.execute_batch(include_str!(
        "../../../migrations/002_memory_projection.sql"
    ))?;
    connection.execute_batch("INSERT INTO projects(id,name) VALUES(1,'demo');
        INSERT INTO sources(id,root_path) VALUES(1,'/unused');
        INSERT INTO project_sources(project_id,source_id) VALUES(1,1);
        INSERT INTO documents(id,source_id,relative_path,stable_id,content_hash,file_mtime_ns,byte_size)
        VALUES(1,1,'pathmarker.md','stablemarker','aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa',0,1);
        INSERT INTO document_chunks(id,document_id,ordinal,heading,body,start_line,end_line)
        VALUES(1,1,0,'Heading','Body',1,1);
        INSERT INTO memories(project_id,memory_id,document_id,kind,status,summary)
        VALUES(1,'memorymarker',1,'constraint','accepted','summarymarker');")?;
    drop(connection);
    let store = Store::open(&db)?;
    for query in [
        "pathmarker",
        "stablemarker",
        "memorymarker",
        "summarymarker",
        "Body",
    ] {
        let hits = store.search("demo", query)?;
        assert_eq!(hits.len(), 1, "{query}");
        assert_eq!(hits[0].chunk_id, 1);
    }
    Ok(())
}
