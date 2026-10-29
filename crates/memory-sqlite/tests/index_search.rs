use memory_sqlite::Store;
use std::path::Path;

fn verify(store: &mut Store) -> anyhow::Result<()> {
    let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures");
    for project in ["banking", "storefront"] {
        store.create_project(project)?;
        store.add_source(project, &fixtures.join(project))?;
        assert_eq!(store.index(project)?, 3);
        assert_eq!(store.index(project)?, 3);
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
