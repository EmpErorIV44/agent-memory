use memory_sqlite::Store;
use std::path::Path;

fn verify(store: &mut Store) -> anyhow::Result<()> {
    let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures");
    for project in ["banking", "storefront"] {
        store.create_project(project)?;
        store.add_source(project, &fixtures.join(project))?;
        assert_eq!(store.index(project)?.added, 3);
        assert_eq!(store.index(project)?.unchanged, 3);
    }
    for (project, marker, foreign) in [
        ("banking", "bankauthmarker", "shopauthmarker"),
        ("storefront", "shopauthmarker", "bankauthmarker"),
    ] {
        let hits = store.search(project, "OAuth2")?;
        assert!(!hits.is_empty());
        assert!(hits.iter().any(|hit| hit.body.contains(marker)));
        assert!(store.search(project, foreign)?.is_empty());
        for hit in hits {
            assert_eq!(hit.path, "authentication.md");
            let contents = std::fs::read_to_string(fixtures.join(project).join(&hit.path))?;
            let lines: Vec<_> = contents.lines().collect();
            assert!(hit.start_line >= 1 && hit.start_line <= hit.end_line);
            assert!(hit.end_line <= lines.len());
            let excerpt = lines[hit.start_line - 1..hit.end_line].join("\n");
            assert!(excerpt.contains("OAuth2"));
            assert_eq!(excerpt, hit.body);
        }
    }
    assert!(store.search("unknown", "OAuth2").is_err());
    Ok(())
}

#[test]
fn index_search_isolation_and_rebuild() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let db = dir.path().join("index.sqlite");
    {
        let mut store = Store::open(&db)?;
        verify(&mut store)?;
    }
    std::fs::remove_file(&db)?;
    let mut rebuilt = Store::open(&db)?;
    verify(&mut rebuilt)?;
    Ok(())
}

#[test]
fn get_enforces_project_and_read_permission_in_sql() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let db = dir.path().join("index.sqlite");
    let mut store = Store::open(&db)?;
    verify(&mut store)?;
    let hit = store.search("banking", "OAuth2")?.remove(0);
    assert!(store
        .get("banking", hit.chunk_id, &hit.content_hash)?
        .is_some());
    assert!(store
        .get("storefront", hit.chunk_id, &hit.content_hash)?
        .is_none());
    assert!(store.get("banking", hit.chunk_id, "stale")?.is_none());
    let connection = rusqlite::Connection::open(db)?;
    connection.execute("UPDATE project_sources SET can_read = 0 WHERE project_id = (SELECT id FROM projects WHERE name = 'banking')", [])?;
    assert!(store.search("banking", "OAuth2")?.is_empty());
    assert!(store
        .get("banking", hit.chunk_id, &hit.content_hash)?
        .is_none());
    Ok(())
}

#[test]
fn incremental_changes_and_failed_write_preserve_fts() -> anyhow::Result<()> {
    use memory_sqlite::IndexStats;
    use rusqlite::Connection;
    use std::fs;

    let dir = tempfile::tempdir()?;
    let banking = dir.path().join("banking");
    let storefront = dir.path().join("storefront");
    fs::create_dir(&banking)?;
    fs::create_dir(&storefront)?;
    fs::write(banking.join("auth.md"), "# Auth\nOAuth2 bankoriginal\n")?;
    fs::write(banking.join("removed.md"), "# Old\nbankremoved\n")?;
    fs::write(storefront.join("auth.md"), "# Auth\nOAuth2 shoporiginal\n")?;
    let db = dir.path().join("index.sqlite");
    let mut store = Store::open(&db)?;
    for (project, root) in [("banking", &banking), ("storefront", &storefront)] {
        store.create_project(project)?;
        store.add_source(project, root)?;
        store.index(project)?;
    }
    let connection = Connection::open(&db)?;
    let snapshot = || -> rusqlite::Result<Vec<(i64, i64, String, i64)>> {
        connection.prepare("SELECT d.id, c.id, d.content_hash, d.indexed_at FROM documents d JOIN document_chunks c ON c.document_id = d.id ORDER BY d.id, c.id")?
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?
            .collect()
    };
    let original = snapshot()?;
    assert_eq!(
        store.index("banking")?,
        IndexStats {
            unchanged: 2,
            ..Default::default()
        }
    );
    assert_eq!(snapshot()?, original);

    fs::write(banking.join("auth.md"), "# Auth\nOAuth2 bankmodified\n")?;
    fs::remove_file(banking.join("removed.md"))?;
    fs::write(banking.join("new.md"), "# New\nbankadded\n")?;
    assert_eq!(
        store.index("banking")?,
        IndexStats {
            added: 1,
            updated: 1,
            deleted: 1,
            unchanged: 0
        }
    );
    assert!(store.search("banking", "bankoriginal")?.is_empty());
    assert!(store.search("banking", "bankremoved")?.is_empty());
    assert_eq!(store.search("banking", "bankmodified")?.len(), 1);
    assert_eq!(store.search("banking", "bankadded")?.len(), 1);
    assert!(store.search("storefront", "bankmodified")?.is_empty());
    assert_eq!(store.search("storefront", "shoporiginal")?.len(), 1);
    let committed = snapshot()?;
    assert_eq!(
        original[0].0, committed[0].0,
        "updated document retains its ID"
    );

    // Fail after UPDATE and chunk DELETE, during replacement chunk INSERT.
    connection.execute_batch("CREATE TRIGGER fail_chunk BEFORE INSERT ON document_chunks WHEN new.body LIKE '%bankfailure%' BEGIN SELECT RAISE(ABORT, 'injected failure'); END;")?;
    fs::write(banking.join("auth.md"), "# Auth\nOAuth2 bankfailure\n")?;
    assert!(store.index("banking").is_err());
    assert_eq!(snapshot()?, committed);
    assert_eq!(store.search("banking", "bankmodified")?.len(), 1);
    assert!(store.search("banking", "bankfailure")?.is_empty());
    connection.execute_batch("DROP TRIGGER fail_chunk;")?;
    assert_eq!(store.index("banking")?.updated, 1);
    assert_eq!(store.search("banking", "bankfailure")?.len(), 1);

    // A missing registered root is a failed scan, not an empty source.
    fs::remove_dir_all(&banking)?;
    assert!(store.index("banking").is_err());
    assert_eq!(store.search("banking", "bankfailure")?.len(), 1);
    connection.execute_batch(
        "INSERT INTO document_chunks_fts(document_chunks_fts, rank) VALUES ('integrity-check', 1);",
    )?;
    Ok(())
}
