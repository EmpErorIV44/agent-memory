use anyhow::Result;
use memory_sqlite::{SearchMode, SearchOptions, Store};
use serde_json::{json, Value};
use std::fs;

fn strip_explanations(hits: Vec<memory_sqlite::SearchHit>) -> Result<Value> {
    let mut value = serde_json::to_value(hits)?;
    for hit in value.as_array_mut().unwrap() {
        hit.as_object_mut().unwrap().remove("explanation");
    }
    Ok(value)
}

#[test]
fn ranking_explanations_preserve_results_and_measure_real_index_matches() -> Result<()> {
    let tmp = tempfile::tempdir()?;
    let root = tmp.path().join("notes");
    fs::create_dir(&root)?;
    fs::write(root.join("exact.md"), "# General\nзаметки доставка\n")?;
    fs::write(root.join("stem.md"), "# General\nзаметками доставка\n")?;
    fs::write(
        root.join("sections.md"),
        "# General\nзаметки\n## Another\nдоставка\n",
    )?;
    fs::write(
        root.join("metadata.md"),
        "---\ntitle: заметки доставка\n---\n# General\nunrelated body\n",
    )?;
    let hidden = tmp.path().join("private");
    fs::create_dir(&hidden)?;
    fs::write(
        hidden.join("secret.md"),
        "# Secret\nзаметки доставка secret_marker\n",
    )?;
    let mut store = Store::open(&tmp.path().join("index"))?;
    store.create_project("demo")?;
    store.create_project("other")?;
    store.add_source("demo", &root)?;
    store.add_source("other", &hidden)?;
    store.index("demo")?;
    store.index("other")?;
    for (mode, literal, query) in [
        (SearchMode::Fts, false, "заметки OR доставка"),
        (SearchMode::Fts, false, "заметки NOT отсутствует"),
        (SearchMode::Fts, true, "заметки доставка"),
        (SearchMode::Fts, false, "замет*"),
        (SearchMode::Natural, false, "заметки доставка"),
    ] {
        for limit in [1, 2, 50] {
            let mut opts = SearchOptions {
                mode,
                literal,
                limit,
                ..SearchOptions::default()
            };
            let plain = serde_json::to_value(store.search_filtered("demo", query, &opts)?)?;
            opts.explain = true;
            let explained = store.search_filtered("demo", query, &opts)?;
            assert_eq!(strip_explanations(explained.clone())?, plain);
            assert!(!serde_json::to_string(&explained)?.contains("secret_marker"));
            for (rank, hit) in explained.iter().enumerate() {
                let e = hit.explanation.as_ref().unwrap();
                assert_eq!(e["returned_rank"], rank + 1);
                assert!(e["bm25"].as_f64().unwrap().is_finite());
            }
        }
    }
    let opts = SearchOptions {
        mode: SearchMode::Natural,
        explain: true,
        ..SearchOptions::default()
    };
    let hits = store.search_filtered("demo", "заметки доставка", &opts)?;
    let exact = &hits
        .iter()
        .find(|h| h.path == "exact.md")
        .unwrap()
        .explanation
        .as_ref()
        .unwrap();
    assert_eq!(exact["chunk_coverage"], json!([2, 2]));
    let stem = &hits
        .iter()
        .find(|h| h.path == "stem.md")
        .unwrap()
        .explanation
        .as_ref()
        .unwrap();
    assert_eq!(stem["chunk_coverage"], json!([2, 1]));
    let sections = &hits
        .iter()
        .find(|h| h.path == "sections.md")
        .unwrap()
        .explanation
        .as_ref()
        .unwrap();
    assert_eq!(sections["document_coverage"], json!([2, 2]));
    assert_eq!(sections["chunk_coverage"], json!([1, 1]));
    let metadata = &hits.iter().find(|h| h.path == "metadata.md").unwrap();
    assert!(!metadata.body.contains("доставка"));
    assert_eq!(
        metadata.explanation.as_ref().unwrap()["chunk_coverage"],
        json!([2, 2])
    );
    assert!(store.search_filtered("demo", "!!!!", &opts)?.is_empty());
    assert!(store
        .search_filtered("demo", "noneunknown", &opts)?
        .is_empty());
    assert!(store.search_filtered("missing", "query", &opts).is_err());
    let invalid = SearchOptions {
        literal: true,
        ..opts
    };
    assert!(store.search_filtered("demo", "alpha", &invalid).is_err());
    Ok(())
}

#[test]
fn context_diagnostics_reconcile_budget_and_explain_inclusions_and_omissions() -> Result<()> {
    let tmp = tempfile::tempdir()?;
    let root = tmp.path().join("notes");
    fs::create_dir(&root)?;
    let note = |id: &str, kind: &str, status: &str, relations: &str, body: &str| {
        format!("---\nmemory:\n  id: {id}\n  project: demo\n  kind: {kind}\n  status: {status}\n  summary: {id}\n  relations: {relations}\n---\n# {id}\n{body}\n")
    };
    fs::write(
        root.join("core.md"),
        note(
            "core",
            "constraint",
            "accepted",
            "[]",
            &"Обязательные правила 🐈 \"quoted\". ".repeat(200),
        ),
    )?;
    fs::write(
        root.join("match.md"),
        note(
            "match",
            "decision",
            "accepted",
            "[{kind: relates_to, target: related}]",
            "alpha shipping\n## Later\nalpha",
        ),
    )?;
    fs::write(
        root.join("related.md"),
        note("related", "fact", "accepted", "[]", "other material"),
    )?;
    fs::write(
        root.join("retired.md"),
        note(
            "retired",
            "decision",
            "deprecated",
            "[]",
            "alpha retired_marker",
        ),
    )?;
    let private = tmp.path().join("hidden");
    fs::create_dir(&private)?;
    fs::write(private.join("secret.md"), "# alpha\nsecret_marker\n")?;
    let mut store = Store::open(&tmp.path().join("index"))?;
    store.create_project("demo")?;
    store.create_project("other")?;
    store.add_source("demo", &root)?;
    store.add_source("other", &private)?;
    store.index("demo")?;
    store.index("other")?;
    let mut saw_omission = false;
    for budget in [512, 1100, 1800, 4000, 32768] {
        let plain = store.task_context("demo", "alpha shipping", budget)?;
        let explained = store.task_context_explained("demo", "alpha shipping", budget)?;
        assert_eq!(explained["context"], plain);
        let text = serde_json::to_string(&explained)?;
        assert!(!text.contains("secret_marker"));
        assert!(!text.contains("retired.md"));
        let e = &explained["explanation"];
        let b = &e["budget"];
        let actual = serde_json::to_vec(&plain)?.len();
        assert_eq!(b["used_bytes"], actual);
        assert_eq!(plain["estimated_tokens"], actual);
        assert!(actual <= budget);
        assert_eq!(b["remaining_bytes"], budget - actual);
        let group_bytes: u64 = b["entry_bytes_by_reason"]
            .as_object()
            .unwrap()
            .values()
            .map(|v| v.as_u64().unwrap())
            .sum();
        assert_eq!(
            group_bytes + b["metadata_and_container_bytes"].as_u64().unwrap(),
            actual as u64
        );
        assert_eq!(
            b["excerpt_json_bytes"].as_u64().unwrap() + b["entry_metadata_bytes"].as_u64().unwrap(),
            group_bytes
        );
        let candidates = e["candidates"].as_array().unwrap();
        let omissions = candidates
            .iter()
            .filter(|c| c["outcome"] == "omitted_budget")
            .count();
        assert_eq!(plain["omitted_count"], omissions);
        saw_omission |= omissions > 0;
        let related = candidates
            .iter()
            .find(|c| c["path"] == "related.md")
            .unwrap();
        assert_eq!(related["reason"], "related_memory");
        assert_eq!(
            related["relation_witnesses"][0],
            json!({"from":"match", "to":"related", "kind":"relates_to"})
        );
        let matched = candidates.iter().find(|c| c["path"] == "match.md").unwrap();
        assert_eq!(
            matched["task_match"]["matched_normalized_task_terms"],
            json!(["alpha", "shipping"])
        );
        for entry in plain["entries"].as_array().unwrap() {
            let c = candidates
                .iter()
                .find(|c| c["path"] == entry["path"])
                .unwrap();
            assert_eq!(c["outcome"], "included");
            assert_eq!(c["entry_bytes"], serde_json::to_vec(entry)?.len());
        }
    }
    assert!(saw_omission);
    assert!(store.task_context_explained("demo", "alpha", 511).is_err());
    assert!(store.task_context_explained("demo", " ", 4000).is_err());
    Ok(())
}
