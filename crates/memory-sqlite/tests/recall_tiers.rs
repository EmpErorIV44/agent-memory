use anyhow::Result;
use memory_sqlite::Store;
use serde_json::json;
use std::{collections::HashSet, fs};

fn note(id: &str, kind: &str, status: &str, tags: &[&str], body: &str) -> String {
    format!(
        "---\n{}\n---\n# Details\n{body}\n",
        json!({"tags":tags,"memory":{"id":id,"project":"demo","kind":kind,"status":status,"summary":id}})
    )
}

#[test]
fn explicit_tiers_preserve_legacy_baselines_and_do_not_read_deferred_notes() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let root = temp.path().join("notes");
    fs::create_dir(&root)?;
    let mut store = Store::open(&temp.path().join("db"))?;
    store.create_project("demo")?;
    store.create_project("other")?;
    store.add_source("demo", &root)?;
    for (id, kind, status, tags) in [
        ("legacy", "constraint", "accepted", vec![]),
        ("legacy-pref", "decision", "accepted", vec!["preference"]),
        ("core", "fact", "accepted", vec!["recall/always"]),
        (
            "scoped",
            "constraint",
            "accepted",
            vec!["recall/task", "topic/git"],
        ),
        ("retired", "constraint", "deprecated", vec!["recall/always"]),
    ] {
        fs::write(
            root.join(format!("{id}.md")),
            note(id, kind, status, &tags, "Details"),
        )?;
    }
    store.index("demo")?;
    fs::remove_file(root.join("scoped.md"))?; // Selection uses the index, not full-note reads.
    let plan = store.recall_plan("demo", None, 3)?;
    assert_eq!(
        plan.required
            .iter()
            .map(|r| r.memory.id.as_str())
            .collect::<Vec<_>>(),
        vec!["legacy", "core", "legacy-pref"]
    );
    assert!(plan.preferences.is_empty());
    assert_eq!(plan.deferred_count, 1);
    assert_eq!(
        store.recall_plan("demo", Some("GIT"), 3)?.preferences[0]
            .memory
            .id,
        "scoped"
    );
    assert!(store
        .recall_plan("other", Some("git"), 3)?
        .required
        .is_empty());
    assert!(store.recall_plan("demo", None, 21).is_err());
    assert!(store.recall_plan("demo", Some("  "), 3).is_err());
    let connection = rusqlite::Connection::open(temp.path().join("db"))?;
    connection.execute("UPDATE project_sources SET can_read=0 WHERE project_id=(SELECT id FROM projects WHERE name='demo')", [])?;
    let revoked = store.recall_plan("demo", Some("git"), 3)?;
    assert!(revoked.required.is_empty());
    assert!(revoked.preferences.is_empty());
    assert_eq!(revoked.deferred_count, 0);
    Ok(())
}

#[test]
fn topics_override_noisy_body_matches_and_use_words_russian_stems_and_limits() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let root = temp.path().join("notes");
    fs::create_dir(&root)?;
    let mut store = Store::open(&temp.path().join("db"))?;
    store.create_project("demo")?;
    store.add_source("demo", &root)?;
    for (id, tags, body) in [
        ("git", vec!["recall/task", "topic/git"], "release release"),
        ("release", vec!["recall/task", "topic/release"], "release"),
        ("ru", vec!["recall/task", "topic/заметки"], "unrelated"),
        (
            "phrase",
            vec!["recall/task", "topic/database-migration"],
            "unrelated",
        ),
        ("fallback", vec!["recall/task"], "release"),
    ] {
        fs::write(
            root.join(format!("{id}.md")),
            note(id, "constraint", "accepted", &tags, body),
        )?;
    }
    store.index("demo")?;
    let plan = store.recall_plan("demo", Some("release"), 20)?;
    let ids: HashSet<_> = plan
        .preferences
        .iter()
        .map(|r| r.memory.id.as_str())
        .collect();
    assert_eq!(ids, HashSet::from(["release", "fallback"]));
    assert!(store
        .recall_plan("demo", Some("gitty"), 20)?
        .preferences
        .is_empty());
    assert_eq!(
        store
            .recall_plan("demo", Some("ЗАМЕТКАМИ"), 20)?
            .preferences[0]
            .memory
            .id,
        "ru"
    );
    assert!(store
        .recall_plan("demo", Some("database"), 20)?
        .preferences
        .is_empty());
    assert_eq!(
        store
            .recall_plan("demo", Some("migration database"), 20)?
            .preferences[0]
            .memory
            .id,
        "phrase"
    );
    let limited = store.recall_plan("demo", Some("release"), 1)?;
    assert_eq!(limited.matched_count, 2);
    assert_eq!(limited.preferences.len(), 1);
    assert_eq!(limited.deferred_count, 4);
    let none = store.recall_plan("demo", Some("release"), 0)?;
    assert_eq!(none.matched_count, 2);
    assert!(none.preferences.is_empty());
    Ok(())
}

#[test]
fn deferred_constraints_are_not_forced_and_exclusions_avoid_full_reads() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let root = temp.path().join("notes");
    fs::create_dir(&root)?;
    let mut store = Store::open(&temp.path().join("db"))?;
    store.create_project("demo")?;
    store.add_source("demo", &root)?;
    fs::write(
        root.join("core.md"),
        note("core", "constraint", "accepted", &["recall/always"], "core"),
    )?;
    fs::write(
        root.join("pref.md"),
        note(
            "pref",
            "constraint",
            "accepted",
            &["recall/task", "topic/git"],
            "git",
        ),
    )?;
    fs::write(root.join("task.md"), "# Feature\nwebhook delivery\n")?;
    store.index("demo")?;
    let result = store.task_context("demo", "webhook", 16000)?;
    assert!(!serde_json::to_string(&result)?.contains("pref.md"));
    let plan = store.recall_plan("demo", Some("webhook"), 3)?;
    fs::remove_file(root.join("core.md"))?;
    let result = store.task_context_excluding("demo", "webhook", 16000, &plan.managed_keys)?;
    assert_eq!(result["entries"].as_array().unwrap().len(), 1);
    assert_eq!(result["entries"][0]["path"], "task.md");
    Ok(())
}

#[test]
fn contradictory_scope_tags_are_rejected() {
    let result =
        memory_core::parse_metadata("---\ntags: ['recall/task', 'recall/always']\n---\n# Rule\n");
    assert!(result
        .unwrap_err()
        .to_string()
        .contains("cannot be combined"));
    assert!(
        memory_core::parse_metadata("---\ntags: ['recall/task','topic/---']\n---\n")
            .unwrap_err()
            .to_string()
            .contains("at least one word")
    );
    assert!(memory_core::parse_metadata("---\ntags: ['topic/']\n---\n").is_ok());
}

#[test]
fn legacy_tags_and_mixed_scope_validation_remain_supported() -> Result<()> {
    assert!(
        memory_core::parse_metadata("---\ntags: ['recall:always', 'recall/task']\n---\n").is_err()
    );
    let temp = tempfile::tempdir()?;
    let mut store = Store::open(&temp.path().join("db"))?;
    let root = temp.path().join("notes");
    fs::create_dir(&root)?;
    store.create_project("demo")?;
    store.add_source("demo", &root)?;
    fs::write(
        root.join("legacy.md"),
        note(
            "legacy",
            "constraint",
            "accepted",
            &["recall:task", "topic:заметки"],
            "unrelated",
        ),
    )?;
    store.index("demo")?;
    assert!(store.recall_plan("demo", None, 3)?.required.is_empty());
    assert_eq!(
        store.recall_plan("demo", Some("заметками"), 3)?.preferences[0]
            .memory
            .id,
        "legacy"
    );
    Ok(())
}
