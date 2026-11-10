use anyhow::Result;
use memory_sqlite::{SearchMode, SearchOptions, Store};
use std::fs;

fn options() -> SearchOptions {
    SearchOptions {
        mode: SearchMode::Natural,
        ..SearchOptions::default()
    }
}

#[test]
fn minimum_coverage_filters_documents_before_limit_and_explains_empty_results() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let notes = temp.path().join("notes");
    fs::create_dir(&notes)?;
    fs::write(notes.join("weak.md"), "# General\nwebhook\n")?;
    fs::write(
        notes.join("strong.md"),
        "# First\nwebhook\n## Second\nretry\n",
    )?;
    fs::write(notes.join("ru.md"), "# General\nзаметками доставка\n")?;
    let mut store = Store::open(&temp.path().join("index.sqlite"))?;
    store.create_project("demo")?;
    store.add_source("demo", &notes)?;
    store.index("demo")?;
    let mut opts = options();
    let original = store.search_filtered("demo", "webhook retry", &opts)?;
    assert_eq!(original.len(), 2);
    opts.min_coverage = Some(0.0);
    assert_eq!(
        store.search_filtered("demo", "webhook retry", &opts)?.len(),
        2
    );
    opts.min_coverage = Some(1.0);
    opts.limit = 1;
    let hits = store.search_filtered("demo", "webhook retry", &opts)?;
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].path, "strong.md");
    opts.explain = true;
    let report = store.search_report("demo", "webhook retry", &opts)?;
    assert_eq!(report["coverage_filter"]["rejected"][0]["path"], "weak.md");
    assert_eq!(report["coverage_filter"]["rejected"][0]["coverage"], 0.5);
    let empty = store.search_report("demo", "webhook absent", &opts)?;
    assert!(empty["hits"].as_array().unwrap().is_empty());
    assert_eq!(
        empty["coverage_filter"]["rejected"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    // Morphological variants count as one normalized query word.
    assert_eq!(
        store
            .search_filtered("demo", "заметки заметками доставка", &opts)?
            .len(),
        1
    );
    assert!(store.search_filtered("demo", "???", &opts)?.is_empty());
    for value in [-0.1, 1.1, f64::NAN, f64::INFINITY] {
        opts.min_coverage = Some(value);
        assert!(store.search_filtered("demo", "webhook", &opts).is_err());
    }
    opts.min_coverage = Some(0.5);
    opts.mode = SearchMode::Fts;
    assert!(store.search_filtered("demo", "webhook", &opts).is_err());
    Ok(())
}

#[test]
fn coverage_precedes_exactness_and_limit_is_applied_after_ranking() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let notes = temp.path().join("notes");
    fs::create_dir(&notes)?;
    let mut store = Store::open(&temp.path().join("index.sqlite"))?;
    store.create_project("demo")?;
    store.add_source("demo", &notes)?;
    for i in 0..60 {
        fs::write(notes.join(format!("single-{i}.md")), "# General\nзаметки\n")?;
    }
    fs::write(notes.join("coverage.md"), "# General\nзаметками доставка\n")?;
    fs::write(notes.join("exact.md"), "# General\nзаметки доставка\n")?;
    store.index("demo")?;
    let mut opts = options();
    let hits = store.search_filtered("demo", "заметки доставка", &opts)?;
    assert_eq!(hits.len(), 50);
    assert_eq!(hits[0].path, "exact.md");
    assert_eq!(hits[1].path, "coverage.md");
    let ids: Vec<_> = hits.iter().map(|h| h.chunk_id).collect();
    for limit in [1, 2, 5] {
        opts.limit = limit;
        let limited = store.search_filtered("demo", "заметки доставка", &opts)?;
        assert_eq!(
            limited.iter().map(|h| h.chunk_id).collect::<Vec<_>>(),
            ids[..limit]
        );
    }
    assert_eq!(
        ids,
        store
            .search_filtered("demo", "ДОСТАВКА заметки заметки", &options())?
            .iter()
            .map(|h| h.chunk_id)
            .collect::<Vec<_>>()
    );
    Ok(())
}

#[test]
fn ordinary_words_do_not_execute_fts_syntax_and_legacy_fts_is_unchanged() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let notes = temp.path().join("notes");
    fs::create_dir(&notes)?;
    let mut store = Store::open(&temp.path().join("index.sqlite"))?;
    store.create_project("demo")?;
    store.add_source("demo", &notes)?;
    for (name, body) in [
        ("alpha.md", "alpha"),
        ("beta.md", "beta"),
        ("operator.md", "AND"),
        ("prefix.md", "alphabet"),
    ] {
        fs::write(notes.join(name), format!("# General\n{body}\n"))?;
    }
    store.index("demo")?;
    for query in [
        "alpha beta",
        "alpha AND beta",
        "alpha\"beta",
        "unknown:alpha beta",
        "alpha* beta",
        "(alpha / beta)",
    ] {
        let hits = store.search_filtered("demo", query, &options())?;
        assert!(hits.iter().any(|h| h.path == "alpha.md"), "{query}");
        assert!(hits.iter().any(|h| h.path == "beta.md"), "{query}");
        assert!(!hits.iter().any(|h| h.path == "prefix.md"), "{query}");
    }
    assert!(store.search("demo", "alpha beta")?.is_empty());
    assert_eq!(store.search("demo", "alpha OR beta")?.len(), 2);
    assert!(store.search("demo", "alpha AND").is_err());
    assert_eq!(
        serde_json::from_str::<SearchOptions>("{}")?.mode,
        SearchMode::Fts
    );
    Ok(())
}

#[test]
fn natural_input_bounds_and_conflicts_are_explicit() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let store = Store::open(&temp.path().join("index.sqlite"))?;
    store.create_project("demo")?;
    for query in ["", "   ", "---/():*^", "\"\""] {
        assert!(store.search_filtered("demo", query, &options())?.is_empty());
    }
    let mut opts = options();
    opts.literal = true;
    assert!(store
        .search_filtered("demo", "alpha", &opts)
        .unwrap_err()
        .to_string()
        .contains("literal"));
    for limit in [0, 51] {
        opts = options();
        opts.limit = limit;
        assert!(store.search_filtered("demo", "alpha", &opts).is_err());
    }
    assert!(store
        .search_filtered("demo", &"я".repeat(4097), &options())
        .unwrap_err()
        .to_string()
        .contains("8192"));
    let too_many = (0..129)
        .map(|i| format!("term{i}"))
        .collect::<Vec<_>>()
        .join(" ");
    assert!(store
        .search_filtered("demo", &too_many, &options())
        .unwrap_err()
        .to_string()
        .contains("128"));
    let allowed = (0..128)
        .map(|i| format!("term{i}"))
        .collect::<Vec<_>>()
        .join(" ");
    assert!(store
        .search_filtered("demo", &allowed, &options())?
        .is_empty());
    Ok(())
}

#[test]
fn filters_and_project_access_apply_before_returning_natural_hits() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let notes = temp.path().join("notes");
    let foreign = temp.path().join("foreign");
    fs::create_dir(&notes)?;
    fs::create_dir(&foreign)?;
    let mut store = Store::open(&temp.path().join("index.sqlite"))?;
    store.create_project("demo")?;
    store.create_project("other")?;
    store.add_source("demo", &notes)?;
    store.add_source("other", &foreign)?;
    fs::write(foreign.join("secret.md"), "# General\nalpha beta secret\n")?;
    for (name, kind, status) in [
        ("wanted", "fact", "accepted"),
        ("wrong-kind", "decision", "accepted"),
        ("wrong-status", "fact", "deprecated"),
    ] {
        fs::write(notes.join(format!("{name}.md")), format!("---\nmemory:\n  id: {name}\n  project: demo\n  kind: {kind}\n  status: {status}\n  summary: alpha beta\n---\n# General\nordinary\n"))?;
    }
    store.index("demo")?;
    store.index("other")?;
    let mut opts = SearchOptions::from_filters(Some("fact"), Some("accepted"))?;
    opts.mode = SearchMode::Natural;
    let hits = store.search_filtered("demo", "alpha beta secret", &opts)?;
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].path, "wanted.md");
    opts.min_coverage = Some(1.0);
    opts.explain = true;
    let report = store.search_report("demo", "alpha beta secret", &opts)?;
    assert!(report["hits"].as_array().unwrap().is_empty());
    let rejected = report["coverage_filter"]["rejected"].as_array().unwrap();
    assert_eq!(rejected.len(), 1);
    assert_eq!(rejected[0]["path"], "wanted.md");
    assert!(store
        .search_filtered("demo", "secret", &options())?
        .is_empty());
    assert_eq!(
        store.search_filtered("other", "secret", &options())?.len(),
        1
    );
    Ok(())
}

#[test]
fn document_coverage_unions_sections_and_returns_one_best_chunk() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let notes = temp.path().join("notes");
    fs::create_dir(&notes)?;
    let mut store = Store::open(&temp.path().join("index.sqlite"))?;
    store.create_project("demo")?;
    store.add_source("demo", &notes)?;
    fs::write(
        notes.join("complete.md"),
        "# First\nalpha beta\n\n# Second\ngamma\n",
    )?;
    fs::write(notes.join("partial.md"), "# General\nalpha beta\n")?;
    store.index("demo")?;
    let hits = store.search_filtered("demo", "alpha beta gamma", &options())?;
    assert_eq!(hits.len(), 2);
    assert_eq!(hits[0].path, "complete.md");
    assert!(hits[0].body.contains("alpha beta"));
    assert_eq!(hits[1].path, "partial.md");
    Ok(())
}
