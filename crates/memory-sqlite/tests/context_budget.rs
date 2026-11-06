use anyhow::Result;
use memory_sqlite::Store;
use serde_json::{json, Value};
use std::fs;

fn note(id: &str, project: &str, kind: &str, status: &str, summary: &str, body: &str) -> String {
    let metadata = json!({
        "id": format!("document-{id}"),
        "title": summary,
        "memory": {"id": id, "project": project, "kind": kind, "status": status, "summary": summary}
    });
    format!("---\n{metadata}\n---\n# Knowledge\n{body}\n")
}

fn paths(report: &Value) -> Vec<&str> {
    report["entries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| entry["path"].as_str().unwrap())
        .collect()
}

fn assert_budget(report: &Value, budget: usize) -> Result<()> {
    // Serialize independently, including JSON escaping and UTF-8 bytes.
    let bytes = serde_json::to_vec(report)?;
    assert!(bytes.len() <= budget, "{} > {budget}", bytes.len());
    assert_eq!(report["estimated_tokens"], bytes.len());
    serde_json::from_slice::<Value>(&bytes)?;
    Ok(())
}

#[test]
fn long_constraint_excerpts_leave_room_for_task_knowledge_when_entries_fit() -> Result<()> {
    let temporary = tempfile::tempdir()?;
    let root = temporary.path().join("notes");
    fs::create_dir(&root)?;
    let summaries = [
        "Keep tenant boundaries when reading customer records across background jobs",
        "Keep monetary arithmetic exact when computing settlement and refund totals",
        "Preserve compatibility during deployments with older application processes",
    ];
    for (index, summary) in summaries.iter().enumerate() {
        fs::write(
            root.join(format!("rule-{index}.md")),
            note(
                &format!("rule-{index}"),
                "budget",
                "constraint",
                "accepted",
                summary,
                &"This durable project rule applies to every change. ".repeat(100),
            ),
        )?;
    }
    fs::write(
        root.join("task.md"),
        note(
            "task",
            "budget",
            "implementation",
            "implemented",
            "Recover interrupted reconciliation using a persisted checkpoint",
            &"Reconciliation restores the checkpoint before resuming account settlement. "
                .repeat(50),
        ),
    )?;
    let mut store = Store::open(&temporary.path().join("index.sqlite"))?;
    store.create_project("budget")?;
    store.add_source("budget", &root)?;
    store.index("budget")?;

    let report = store.task_context("budget", "reconciliation checkpoint", 4000)?;
    assert_budget(&report, 4000)?;
    let selected = paths(&report);
    for expected in ["rule-0.md", "rule-1.md", "rule-2.md", "task.md"] {
        assert!(selected.contains(&expected), "{expected} omitted: {report}");
    }
    assert_eq!(report["omitted_count"], 0);
    assert_eq!(report["omitted_accepted_constraints"], 0);
    assert!(
        report["entries"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|entry| entry["reason"] == "accepted_constraint")
            .count()
            == 3
    );
    assert_eq!(
        report,
        store.task_context("budget", "reconciliation checkpoint", 4000)?
    );
    Ok(())
}

#[test]
fn unicode_and_escaped_content_stay_within_every_budget_deterministically() -> Result<()> {
    let temporary = tempfile::tempdir()?;
    let root = temporary.path().join("notes");
    fs::create_dir(&root)?;
    for (id, kind, summary, body) in [
        (
            "rule",
            "constraint",
            "Секреты «клиента» не попадают в журналы 🦀",
            "Сохранять приватность. \"кавычки\" \\ путь\tтабуляция\n",
        ),
        (
            "task",
            "implementation",
            "Восстановление контрольной точки",
            "Контрольная точка восстанавливает обработку. \"журнал\" \\ путь\t🦀\n",
        ),
    ] {
        fs::write(
            root.join(format!("{id}.md")),
            note(id, "budget", kind, "accepted", summary, &body.repeat(200)),
        )?;
    }
    let mut store = Store::open(&temporary.path().join("index.sqlite"))?;
    store.create_project("budget")?;
    store.add_source("budget", &root)?;
    store.index("budget")?;
    for budget in [512, 700, 1024, 1600, 4000, 16000, 32768] {
        let report = store.task_context("budget", "контрольная точка", budget)?;
        assert_budget(&report, budget)?;
        assert_eq!(
            report,
            store.task_context("budget", "контрольная точка", budget)?
        );
        let selected = paths(&report);
        assert!(selected.len() <= 2);
        if budget >= 4000 {
            assert!(selected.contains(&"rule.md"));
            assert!(selected.contains(&"task.md"));
        }
    }
    Ok(())
}

#[test]
fn absent_task_matches_keep_constraints_and_exclude_foreign_and_retired_records() -> Result<()> {
    let temporary = tempfile::tempdir()?;
    let root = temporary.path().join("notes");
    fs::create_dir(&root)?;
    for (id, project, kind, status, body) in [
        (
            "rule",
            "budget",
            "constraint",
            "accepted",
            "Keep project records private",
        ),
        (
            "foreign",
            "other",
            "constraint",
            "accepted",
            "reconciliation foreign secret",
        ),
        (
            "retired",
            "budget",
            "constraint",
            "superseded",
            "reconciliation retired rule",
        ),
        (
            "deprecated",
            "budget",
            "implementation",
            "deprecated",
            "reconciliation deprecated logic",
        ),
    ] {
        fs::write(
            root.join(format!("{id}.md")),
            note(id, project, kind, status, id, body),
        )?;
    }
    let mut store = Store::open(&temporary.path().join("index.sqlite"))?;
    store.create_project("budget")?;
    store.create_project("other")?;
    store.add_source("budget", &root)?;
    store.add_source("other", &root)?;
    store.index("budget")?;
    store.index("other")?;
    for task in ["unmatchedtopic", "reconciliation", "\" \\ 🦀"] {
        let report = store.task_context("budget", task, 4000)?;
        assert_budget(&report, 4000)?;
        assert_eq!(paths(&report), vec!["rule.md"]);
        assert_eq!(report["entries"][0]["reason"], "accepted_constraint");
    }
    Ok(())
}

#[test]
fn oversized_metadata_is_omitted_without_blocking_other_useful_entries() -> Result<()> {
    let temporary = tempfile::tempdir()?;
    let root = temporary.path().join("notes");
    fs::create_dir(&root)?;
    for (id, kind, summary, body) in [
        (
            "a-oversized",
            "constraint",
            "A verbose author-supplied metadata claim. ".repeat(150),
            "Preserve project records",
        ),
        (
            "b-rule",
            "constraint",
            "Keep customer secrets private".to_owned(),
            "Keep customer secrets private",
        ),
        (
            "task",
            "implementation",
            "Reconciliation checkpoints".to_owned(),
            "Reconciliation resumes from persisted checkpoints",
        ),
    ] {
        fs::write(
            root.join(format!("{id}.md")),
            note(id, "budget", kind, "accepted", &summary, body),
        )?;
    }
    let mut store = Store::open(&temporary.path().join("index.sqlite"))?;
    store.create_project("budget")?;
    store.add_source("budget", &root)?;
    store.index("budget")?;
    let report = store.task_context("budget", "reconciliation checkpoints", 4000)?;
    assert_budget(&report, 4000)?;
    assert_eq!(paths(&report), vec!["b-rule.md", "task.md"]);
    assert_eq!(report["omitted_count"], 1);
    assert_eq!(report["omitted_accepted_constraints"], 1);
    assert!(report["entries"]
        .as_array()
        .unwrap()
        .iter()
        .all(|entry| !entry["excerpt"].as_str().unwrap().is_empty()));
    Ok(())
}

#[test]
fn distinct_normalized_task_coverage_beats_repetition_and_ties_are_stable() -> Result<()> {
    let temporary = tempfile::tempdir()?;
    let root = temporary.path().join("notes");
    fs::create_dir(&root)?;
    for (id, body) in [
        ("repeated", "накладные ".repeat(100)),
        (
            "broad-a",
            "Проверка накладными перевозчика перед отправкой.".to_owned(),
        ),
        (
            "broad-b",
            "Проверка накладными перевозчика перед отправкой.".to_owned(),
        ),
    ] {
        fs::write(
            root.join(format!("{id}.md")),
            note(id, "budget", "implementation", "implemented", id, &body),
        )?;
    }
    let mut store = Store::open(&temporary.path().join("index.sqlite"))?;
    store.create_project("budget")?;
    store.add_source("budget", &root)?;
    store.index("budget")?;
    let task = "накладные перевозчик";
    let report = store.task_context("budget", task, 16000)?;
    assert_budget(&report, 16000)?;
    let selected = paths(&report);
    assert_eq!(selected.len(), 3);
    assert!(selected[..2].contains(&"broad-a.md"));
    assert!(selected[..2].contains(&"broad-b.md"));
    assert_eq!(selected[2], "repeated.md");
    assert_eq!(report, store.task_context("budget", task, 16000)?);
    Ok(())
}
