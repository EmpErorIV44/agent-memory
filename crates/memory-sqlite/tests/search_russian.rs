use anyhow::Result;
use memory_sqlite::{SearchOptions, Store};
use std::fs;

fn structured(kind: &str, status: &str, summary: &str, body: &str) -> String {
    format!("---\nmemory:\n  id: record\n  project: demo\n  kind: {kind}\n  status: {status}\n  summary: {summary}\n---\n# General\n{body}\n")
}

#[test]
fn russian_forms_exact_ranking_phrases_boolean_and_prefixes() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let root = temp.path().join("notes");
    fs::create_dir(&root)?;
    let mut store = Store::open(&temp.path().join("index.sqlite"))?;
    store.create_project("demo")?;
    store.add_source("demo", &root)?;
    for (path, body) in [
        (
            "a-stem.md",
            "# Русскими заметками\nconfigurations alpha beta\n",
        ),
        (
            "z-exact.md",
            "# General\nРусские заметки configuration alpha gamma\n",
        ),
        (
            "forbidden.md",
            "# General\nРусские заметки кошки configuration\n",
        ),
        // Phrase exclusion needs supported Snowball forms, not an irregular stem:
        // Russian "заметок" retains the fleeting vowel and stems differently.
        ("separated.md", "# General\nрусскими прочими заметками\n"),
        ("compound.md", "# General\nРусскими_заметками\n"),
    ] {
        fs::write(root.join(path), body)?;
    }
    store.index("demo")?;
    let hits = store.search("demo", "русские заметки")?;
    assert_eq!(
        hits.len(),
        5,
        "matched paths: {:?}",
        hits.iter().map(|h| &h.path).collect::<Vec<_>>()
    );
    assert!(
        hits.iter().position(|h| h.path == "z-exact.md").unwrap()
            < hits.iter().position(|h| h.path == "a-stem.md").unwrap()
    );
    let phrase = store.search("demo", "\"русские заметки\"")?;
    assert_eq!(phrase.len(), 4);
    assert!(!phrase.iter().any(|h| h.path == "separated.md"));
    let not = store.search("demo", "заметки NOT кошка")?;
    assert_eq!(not.len(), 4);
    assert!(!not.iter().any(|h| h.path == "forbidden.md"));
    assert_eq!(store.search("demo", "заметки AND config*")?.len(), 3);
    assert_eq!(store.search("demo", "alpha AND (beta OR gamma)")?.len(), 2);
    assert_eq!(store.search("demo", "configuration")?.len(), 2);
    assert_eq!(store.search("demo", "NEAR(русские заметки, 0)")?.len(), 4);
    assert_eq!(store.search("demo", "заметко*")?.len(), 0);
    assert_eq!(store.search("demo", "заметк*")?.len(), 5);
    assert_eq!(
        store.search("demo", "body:заметки NOT body:кошка")?.len(),
        4
    );
    for invalid in ["заметки AND", "\"заметки", "absentcolumn:заметки", "("] {
        assert!(store.search("demo", invalid).is_err(), "{invalid}");
    }
    Ok(())
}

#[test]
fn title_and_summary_weights_and_validated_filters_respect_ownership() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let root = temp.path().join("notes");
    fs::create_dir(&root)?;
    let db = temp.path().join("index.sqlite");
    let mut store = Store::open(&db)?;
    for p in ["demo", "other"] {
        store.create_project(p)?;
        store.add_source(p, &root)?;
    }
    fs::write(root.join("z-title.md"), "# prioritymarker\nordinary\n")?;
    fs::write(root.join("a-body.md"), "# General\nprioritymarker\n")?;
    fs::write(
        root.join("knowledge.md"),
        structured(
            "constraint",
            "accepted",
            "Правила памяти secretmetadata",
            "ordinary",
        ),
    )?;
    fs::write(root.join("public.md"), "# General\nправилом памяти\n")?;
    store.index("demo")?;
    store.index("other")?;
    assert_eq!(
        store.search("demo", "prioritymarker")?[0].path,
        "z-title.md"
    );
    let options = SearchOptions::from_filters(Some("constraint"), Some("accepted"))?;
    assert_eq!(store.search_filtered("demo", "правило", &options)?.len(), 1);
    assert!(store
        .search_filtered("other", "правило", &options)?
        .is_empty());
    assert!(store.search("other", "secretmetadata")?.is_empty());
    assert_eq!(store.search("demo", "summary:правила")?.len(), 1);
    assert!(store.search("other", "summary:правила")?.is_empty());
    for (kind, status) in [
        (Some("unknown"), None),
        (None, Some("current")),
        (Some("Constraint"), None),
    ] {
        assert!(SearchOptions::from_filters(kind, status).is_err());
    }
    let wrong = SearchOptions::from_filters(None, Some("deprecated"))?;
    assert!(store.search_filtered("demo", "правило", &wrong)?.is_empty());
    fs::write(
        root.join("knowledge.md"),
        structured("constraint", "deprecated", "Новое знание", "ordinary"),
    )?;
    store.index("demo")?;
    assert!(store.search("demo", "secretmetadata")?.is_empty());
    assert_eq!(store.search_filtered("demo", "знания", &wrong)?.len(), 1);
    let conn = rusqlite::Connection::open(&db)?;
    conn.execute("UPDATE project_sources SET can_read=0 WHERE project_id=(SELECT id FROM projects WHERE name='demo')",[])?;
    assert!(store.search("demo", "prioritymarker OR знания")?.is_empty());
    Ok(())
}
