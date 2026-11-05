use anyhow::Result;
use memory_sqlite::Store;
use serde_json::{json, Value};
use std::fs;

fn note(id: &str, project: &str, kind: &str, status: &str, relations: Value, body: &str) -> String {
    let metadata = json!({"memory":{"id":id,"project":project,"kind":kind,"status":status,"summary":format!("Claim {id}"),"relations":relations}});
    format!("---\n{metadata}\n---\n# Knowledge\n{body}\n")
}

#[test]
fn explicit_diagnostics_deduplicate_pairs_and_find_cycles_without_mutation() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let root = temp.path().join("notes");
    fs::create_dir(&root)?;
    let rows = [
        (
            "a",
            "accepted",
            json!([{"kind":"contradicts","target":"b"},{"kind":"supersedes","target":"b"},{"kind":"depends_on","target":"c"}]),
        ),
        (
            "b",
            "accepted",
            json!([{"kind":"contradicts","target":"a"},{"kind":"supersedes","target":"a"}]),
        ),
        (
            "c",
            "planned",
            json!([{"kind":"depends_on","target":"a"},{"kind":"contradicts","target":"retired"},{"kind":"contradicts","target":"hidden"}]),
        ),
        ("retired", "deprecated", json!([])),
    ];
    let mut store = Store::open(&temp.path().join("db"))?;
    store.create_project("demo")?;
    store.create_project("foreign")?;
    for (id, status, edges) in rows {
        fs::write(
            root.join(format!("{id}.md")),
            note(id, "demo", "decision", status, edges, "authentication"),
        )?;
    }
    fs::write(
        root.join("foreign.md"),
        note("hidden", "foreign", "fact", "accepted", json!([]), "secret"),
    )?;
    store.add_source("demo", &root)?;
    store.add_source("foreign", &root)?;
    store.index("demo")?;
    store.index("foreign")?;
    let before = fs::read(root.join("a.md"))?;
    let report = store.contradiction_diagnostics("demo")?;
    assert_eq!(report["issue_count"], 5);
    let issues = report["issues"].as_array().unwrap();
    assert!(issues.contains(&json!({"kind":"active_contradiction","memory_ids":["a","b"]})));
    assert!(issues.contains(&json!({"kind":"dependency_cycle","memory_ids":["a","c"]})));
    assert!(issues.contains(&json!({"kind":"supersession_cycle","memory_ids":["a","b"]})));
    assert!(!serde_json::to_string(&report)?.contains("hidden"));
    assert_eq!(fs::read(root.join("a.md"))?, before);
    assert_eq!(
        store.get_memory("demo", "a")?.unwrap().memory.status,
        memory_core::MemoryStatus::Accepted
    );
    assert!(store.contradiction_diagnostics("missing").is_err());
    Ok(())
}

#[test]
fn context_quotes_task_deduplicates_bounds_unicode_and_excludes_retired_records() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let root = temp.path().join("notes");
    fs::create_dir(&root)?;
    for (id, kind, status, edges, body) in [
        (
            "rule",
            "constraint",
            "accepted",
            json!([]),
            "Always preserve source files.".into(),
        ),
        (
            "auth",
            "decision",
            "accepted",
            json!([{"kind":"depends_on","target":"related"}]),
            "аутентификация Co-Authored-By apps/foo.md".into(),
        ),
        (
            "related",
            "implementation",
            "implemented",
            json!([]),
            "Auxiliary mechanism".into(),
        ),
        (
            "old",
            "decision",
            "superseded",
            json!([]),
            "аутентификация old".into(),
        ),
        (
            "large",
            "fact",
            "accepted",
            json!([]),
            "аутентификация Ж".repeat(2000),
        ),
    ] {
        fs::write(
            root.join(format!("{id}.md")),
            note(id, "demo", kind, status, edges, &body),
        )?;
    }
    let mut store = Store::open(&temp.path().join("db"))?;
    store.create_project("demo")?;
    store.add_source("demo", &root)?;
    store.index("demo")?;
    // Punctuation and operator words must be accepted as plain text.
    store.task_context("demo", "Co-Authored-By apps/foo.md OR \"", 16000)?;
    let report = store.task_context("demo", "аутентификация", 16000)?;
    let entries = report["entries"].as_array().unwrap();
    assert_eq!(entries[0]["reason"], "accepted_constraint");
    assert!(entries.iter().any(|r| r["path"] == "auth.md"));
    assert!(entries
        .iter()
        .any(|r| r["path"] == "related.md" && r["reason"] == "related_memory"));
    assert!(!entries.iter().any(|r| r["path"] == "old.md"));
    let paths: std::collections::HashSet<_> = entries
        .iter()
        .map(|r| r["path"].as_str().unwrap())
        .collect();
    assert_eq!(paths.len(), entries.len());
    for budget in [512, 1024, 4000, 16000, 32768] {
        let report = store.task_context("demo", "аутентификация", budget)?;
        let size = serde_json::to_vec(&report)?.len();
        assert!(size <= budget, "{size} > {budget}");
        assert_eq!(report["estimated_tokens"], size);
    }
    assert_eq!(
        store.task_context("demo", "unmatchedword", 16000)?["entries"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    for task in ["", "   "] {
        assert!(store.task_context("demo", task, 4000).is_err());
    }
    for budget in [0, 511, 32769] {
        assert!(store.task_context("demo", "task", budget).is_err());
    }
    assert!(store.task_context("demo", &"x".repeat(8193), 4000).is_err());
    assert!(store.task_context("unknown", "task", 4000).is_err());
    Ok(())
}

#[test]
fn context_rechecks_source_permissions_and_skips_foreign_shared_memories() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let shared = temp.path().join("shared");
    let private = temp.path().join("private");
    fs::create_dir(&shared)?;
    fs::create_dir(&private)?;
    fs::write(
        shared.join("foreign.md"),
        note(
            "foreign",
            "other",
            "constraint",
            "accepted",
            json!([]),
            "uniquetopic secret",
        ),
    )?;
    fs::write(shared.join("plain.md"), "# Plain\nuniquetopic public\n")?;
    fs::write(
        private.join("private.md"),
        note(
            "private",
            "demo",
            "constraint",
            "accepted",
            json!([]),
            &"uniquetopic Ж".repeat(4000),
        ),
    )?;
    let db = temp.path().join("db");
    let mut store = Store::open(&db)?;
    store.create_project("demo")?;
    store.create_project("other")?;
    store.add_source("demo", &shared)?;
    store.add_source("other", &shared)?;
    let private_id = store.register_source("demo", &private, false)?;
    store.index("demo")?;
    store.index("other")?;
    let report = store.task_context("demo", "uniquetopic", 1600)?;
    let entries = report["entries"].as_array().unwrap();
    assert!(entries
        .iter()
        .any(|r| r["path"] == "private.md" && r["excerpt_truncated"] == true));
    assert!(!entries.iter().any(|r| r["path"] == "foreign.md"));
    rusqlite::Connection::open(&db)?.execute(
        "UPDATE project_sources SET can_read=0 WHERE source_id=?1",
        [private_id],
    )?;
    let report = store.task_context("demo", "uniquetopic", 4000)?;
    let entries = report["entries"].as_array().unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0]["path"], "plain.md");
    assert_eq!(
        store.contradiction_diagnostics("demo")?["records"],
        json!([])
    );
    Ok(())
}

#[test]
fn supersession_conflicts_follow_historical_intermediates() -> Result<()> {
    let temp = tempfile::tempdir()?;
    for (id, status, edges) in [
        ("a", "accepted", json!([{"kind":"supersedes","target":"b"}])),
        (
            "b",
            "superseded",
            json!([{"kind":"supersedes","target":"c"}]),
        ),
        ("c", "accepted", json!([])),
    ] {
        fs::write(
            temp.path().join(format!("{id}.md")),
            note(id, "demo", "decision", status, edges, "chain"),
        )?;
    }
    let mut store = Store::open(&temp.path().join("db"))?;
    store.create_project("demo")?;
    store.add_source("demo", temp.path())?;
    store.index("demo")?;
    let report = store.contradiction_diagnostics("demo")?;
    assert_eq!(
        report["issues"],
        json!([{"kind":"active_supersession_chain","memory_ids":["a","c"],"relation_path":["a","b","c"]}])
    );
    Ok(())
}
