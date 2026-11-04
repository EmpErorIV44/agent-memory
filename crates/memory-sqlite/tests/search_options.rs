use anyhow::Result;
use memory_sqlite::{SearchOptions, Store};
use std::fs;

#[test]
fn literal_phrase_escapes_punctuation_quotes_operators_and_preserves_stemming() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let root = temp.path().join("notes");
    fs::create_dir(&root)?;
    let mut store = Store::open(&temp.path().join("index.sqlite"))?;
    for project in ["demo", "other"] {
        store.create_project(project)?;
    }
    store.add_source("demo", &root)?;
    for (name, body) in [
        ("hyphen.md", "Co-Authored-By"),
        ("path.md", "apps/foo.md"),
        ("quotes.md", "alpha\"beta"),
        ("operator.md", "alpha OR beta"),
        ("separated.md", "alpha something beta"),
        ("russian.md", "Заметками"),
        ("longer.md", "заметкамина"),
    ] {
        fs::write(root.join(name), format!("# General\n{body}\n"))?;
    }
    store.index("demo")?;
    let options = SearchOptions {
        literal: true,
        ..SearchOptions::default()
    };
    for (query, expected) in [
        ("Co-Authored-By", "hyphen.md"),
        ("apps/foo.md", "path.md"),
        ("alpha\"beta", "quotes.md"),
        ("alpha OR beta", "operator.md"),
        ("заметки*", "russian.md"),
    ] {
        let hits = store.search_filtered("demo", query, &options)?;
        assert_eq!(hits.len(), 1, "{query}: {hits:?}");
        assert_eq!(hits[0].path, expected);
        assert!(store.search_filtered("other", query, &options)?.is_empty());
    }
    // Punctuation is still subject to FTS tokenization, not byte matching.
    assert_eq!(
        store.search_filtered("demo", "Co Authored By", &options)?[0].path,
        "hyphen.md"
    );
    for empty in ["", "   ", "---/():*^", "\"\""] {
        assert!(store.search_filtered("demo", empty, &options)?.is_empty());
    }
    let all = store.search("demo", "alpha OR beta")?;
    assert_eq!(all.len(), 3);
    for invalid in [
        "Co-Authored-By",
        "alpha AND",
        "\"alpha",
        "unknown:alpha",
        "alpha OR",
        "(",
        "NEAR(alpha, nope)",
    ] {
        let error = store.search("demo", invalid).unwrap_err();
        assert!(
            error.to_string().contains("Invalid FTS5 search expression"),
            "{error:#}"
        );
        assert!(error.to_string().contains("--literal"));
        assert!(error.chain().any(|cause| cause.is::<rusqlite::Error>()));
    }
    Ok(())
}

#[test]
fn limits_apply_after_ranking_and_filters_and_old_options_keep_defaults() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let root = temp.path().join("notes");
    fs::create_dir(&root)?;
    let mut store = Store::open(&temp.path().join("index.sqlite"))?;
    store.create_project("demo")?;
    store.add_source("demo", &root)?;
    for i in 0..55 {
        let header = if i % 2 == 0 {
            format!("---\nmemory:\n  id: record-{i}\n  project: demo\n  kind: fact\n  status: accepted\n  summary: Ordinary\n---\n")
        } else {
            String::new()
        };
        fs::write(
            root.join(format!("{i:02}.md")),
            format!("{header}# General\nmarker\n"),
        )?;
    }
    store.index("demo")?;
    let default = SearchOptions::default();
    let all = store.search_filtered("demo", "marker", &default)?;
    assert_eq!(all.len(), 50);
    for limit in [1, 5, 50] {
        let options = SearchOptions {
            limit,
            ..default.clone()
        };
        let hits = store.search_filtered("demo", "marker", &options)?;
        assert_eq!(hits.len(), limit);
        assert_eq!(
            hits.iter().map(|h| h.chunk_id).collect::<Vec<_>>(),
            all[..limit].iter().map(|h| h.chunk_id).collect::<Vec<_>>()
        );
    }
    for limit in [0, 51, usize::MAX] {
        let options = SearchOptions {
            limit,
            ..default.clone()
        };
        assert!(store
            .search_filtered("demo", "marker", &options)
            .unwrap_err()
            .to_string()
            .contains("between 1 and 50"));
    }
    let mut filtered = SearchOptions::from_filters(Some("fact"), Some("accepted"))?;
    filtered.limit = 3;
    filtered.literal = true;
    let hits = store.search_filtered("demo", "marker", &filtered)?;
    assert_eq!(hits.len(), 3);
    assert!(hits
        .iter()
        .all(|h| h.path.trim_end_matches(".md").parse::<usize>().unwrap() % 2 == 0));
    let old: SearchOptions = serde_json::from_str(r#"{"kind":"fact","status":null}"#)?;
    assert_eq!(old.limit, 50);
    assert!(!old.literal);
    Ok(())
}

#[test]
fn database_schema_errors_are_not_reported_as_bad_user_queries() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let db = temp.path().join("index.sqlite");
    let store = Store::open(&db)?;
    store.create_project("demo")?;
    let connection = rusqlite::Connection::open(&db)?;
    connection.execute_batch(
        "DROP TABLE search_exact_fts; CREATE VIRTUAL TABLE search_exact_fts USING fts5(heading);",
    )?;
    let error = store.search("demo", "ordinary").unwrap_err();
    assert!(!error.to_string().contains("Invalid FTS5 search expression"));
    assert!(error.chain().any(|cause| cause.is::<rusqlite::Error>()));
    Ok(())
}
