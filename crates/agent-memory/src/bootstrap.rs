//! Session orchestration. Never pulls, writes Markdown, or treats notes as authority.
use anyhow::{ensure, Result};
use memory_core::{MemoryKind, MemoryStatus};
use memory_sqlite::Store;
use serde_json::{json, Value};
use std::path::Path;

pub const DEFAULT_MAX_BYTES: usize = 65536;
pub const DEFAULT_PREFERENCE_LIMIT: usize = 3;

fn size(value: &Value) -> Result<usize> {
    Ok(serde_json::to_vec(value)?.len())
}

fn prefix(text: &str, bytes: usize) -> &str {
    let mut end = text.len().min(bytes);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

/// The byte budget covers the complete compact JSON payload, excluding transport.
pub fn run(
    store: &mut Store,
    project: &str,
    task: Option<&str>,
    max_bytes: usize,
    fetch: bool,
    preference_limit: usize,
) -> Result<Value> {
    ensure!(
        (4096..=1_048_576).contains(&max_bytes),
        "max_bytes must be 4096..1048576"
    );
    ensure!(preference_limit <= 20, "preference_limit must be 0..20");
    if let Some(task) = task {
        ensure!(
            !task.trim().is_empty() && task.len() <= 8192,
            "task must contain 1..8192 UTF-8 bytes"
        );
    }
    let mut freshness = Vec::new();
    for source in store.sources(project)? {
        let entry = match crate::git_sync::freshness(Path::new(&source.root_path), fetch) {
            Ok(value) => json!({"source_id":source.source_id,"status":"ok","freshness":value}),
            // Git diagnostics can contain owner paths or credentials. Keep failures scoped
            // and explicit without copying stderr into an agent's knowledge context.
            Err(_) => {
                json!({"source_id":source.source_id,"status":"error","freshness":null,"error":"freshness_unavailable"})
            }
        };
        freshness.push(entry);
    }
    // Refresh and capture BEFORE reading. Changes observed during/after reads are not
    // swallowed by a cursor taken at the end. This is still not an atomic FS snapshot.
    let changes = store.changes(project, None)?;
    let plan = store.recall_plan(project, task, preference_limit)?;
    let records = plan.required;
    let preferences = plan.preferences;
    let baseline_keys = plan.managed_keys;
    let mut baseline = Vec::new();
    for record in &records {
        baseline.push(json!({"memory_id":record.memory.id,"source_id":record.source_id,"path":record.path,
            "content_hash":record.content_hash,"reason":if record.memory.kind == MemoryKind::Constraint {"accepted_constraint"} else {"accepted_preference"},
            "status":"omitted_budget"}));
    }
    let mut candidate_omitted_count = 0;
    let mut candidates = Vec::new();
    let mut context_status = "not_requested";
    if let Some(task) = task {
        match store.task_context_excluding(project, task, 32768, &baseline_keys) {
            Ok(context) => {
                candidate_omitted_count = context["omitted_count"].as_u64().unwrap_or(0);
                candidates = context["entries"]
                    .as_array()
                    .expect("context entries")
                    .iter()
                    .filter(|entry| {
                        !baseline_keys.contains(&(
                            entry["source_id"].as_i64().unwrap_or(0),
                            entry["path"].as_str().unwrap_or("").to_owned(),
                        ))
                    })
                    .cloned()
                    .collect();
                context_status = "ok";
            }
            Err(_) => context_status = "unavailable",
        }
    }
    let remote_checked = !freshness.is_empty()
        && freshness
            .iter()
            .all(|entry| entry["freshness"]["checked_remote"] == true);
    let mut warnings = vec![
        "untrusted_source_content",
        "evidence_not_verified",
        "not_atomic_snapshot",
    ];
    if !remote_checked {
        warnings.push("remote_freshness_not_checked_for_all_sources");
    }
    let mut report = json!({"project":project,"max_bytes":max_bytes,"used_bytes":max_bytes,"budget_method":"utf8_bytes",
        "baseline_complete":false,"baseline":baseline,
        "context":{"status":if context_status == "ok" {"omitted_budget"} else {context_status},"entries":[],
            "omitted_entries":candidates.iter().map(|e| json!({"source_id":e["source_id"],"path":e["path"],"content_hash":e["content_hash"],"reason":e["reason"]})).collect::<Vec<_>>(),
            "omitted_count":candidates.len(),"candidate_omitted_count":candidate_omitted_count},
        "preferences":{"status":if task.is_some() {"ok"} else {"not_requested"},"limit":preference_limit,
            "matched_count":plan.matched_count,"deferred_count":plan.deferred_count,"complete":false,
            "entries":preferences.iter().map(|r|json!({"memory_id":r.memory.id,"source_id":r.source_id,"path":r.path,"content_hash":r.content_hash,"reason":"task_preference","status":"omitted_budget"})).collect::<Vec<_>>()},
        "freshness":freshness,"cursor":changes.cursor,"revision":changes.revision,"atomic_snapshot":false,"warnings":warnings});
    ensure!(size(&report)? <= max_bytes, "bootstrap manifest exceeds max_bytes; increase budget or use paginated memory_list and memory_get_many");
    // Full baseline documents have priority. A too-large document must not block
    // later small documents. Never truncate a document and mark it as fully read.
    for (index, record) in records.iter().enumerate() {
        // Read one full note at a time, after validating that the manifest fits.
        // The response budget should not require retaining the entire corpus.
        let document = store
            .get_document_by_path(
                project,
                record.source_id,
                &record.path,
                &record.content_hash,
            )
            .ok()
            .flatten()
            .filter(|document| {
                memory_core::parse_metadata(&document.content)
                    .ok()
                    .and_then(|m| m.memory)
                    .is_some_and(|m| {
                        m.project == project
                            && m.id == record.memory.id
                            && m.status == MemoryStatus::Accepted
                    })
            });
        let Some(document) = document else {
            report["baseline"][index]["status"] = json!("unavailable");
            continue;
        };
        let previous = report["baseline"][index].clone();
        report["baseline"][index]["status"] = json!("ok");
        report["baseline"][index]["document"] = serde_json::to_value(document)?;
        if size(&report)? > max_bytes {
            report["baseline"][index] = previous;
        }
    }
    report["baseline_complete"] = json!(report["baseline"]
        .as_array()
        .expect("baseline")
        .iter()
        .all(|entry| entry["status"] == "ok"));
    // Only selected task preferences are read in full. Deferred preferences stay
    // in the index and contribute counters, never a startup list of all notes.
    for (index, record) in preferences.iter().enumerate() {
        let document = store
            .get_document_by_path(
                project,
                record.source_id,
                &record.path,
                &record.content_hash,
            )
            .ok()
            .flatten();
        let Some(document) = document else {
            report["preferences"]["entries"][index]["status"] = json!("unavailable");
            continue;
        };
        let old = report["preferences"]["entries"][index].clone();
        report["preferences"]["entries"][index]["status"] = json!("ok");
        report["preferences"]["entries"][index]["document"] = serde_json::to_value(document)?;
        if size(&report)? > max_bytes {
            report["preferences"]["entries"][index] = old;
        }
    }
    report["preferences"]["complete"] = json!(report["preferences"]["entries"]
        .as_array()
        .expect("preferences")
        .iter()
        .all(|e| e["status"] == "ok"));
    // Admit small excerpts first, then expand fairly with the remaining budget.
    let mut admitted = Vec::new();
    for (index, entry) in candidates.iter().enumerate() {
        let mut short = entry.clone();
        let text = entry["excerpt"].as_str().unwrap_or("");
        short["excerpt"] = json!(prefix(text, 160));
        short["excerpt_truncated"] = json!(text.len() > 160 || entry["excerpt_truncated"] == true);
        let previous_omitted = report["context"]["omitted_entries"].clone();
        report["context"]["omitted_entries"]
            .as_array_mut()
            .expect("omissions")
            .retain(|e| e["source_id"] != entry["source_id"] || e["path"] != entry["path"]);
        report["context"]["entries"]
            .as_array_mut()
            .expect("entries")
            .push(short);
        if size(&report)? <= max_bytes {
            admitted.push(index);
        } else {
            report["context"]["entries"]
                .as_array_mut()
                .expect("entries")
                .pop();
            report["context"]["omitted_entries"] = previous_omitted;
        }
    }
    loop {
        let mut grew = false;
        for (position, &index) in admitted.iter().enumerate() {
            let text = candidates[index]["excerpt"].as_str().unwrap_or("");
            let current = report["context"]["entries"][position]["excerpt"]
                .as_str()
                .unwrap_or("")
                .len();
            let next = prefix(text, current + 128);
            if next.len() <= current {
                continue;
            }
            let old = report["context"]["entries"][position].clone();
            report["context"]["entries"][position]["excerpt"] = json!(next);
            report["context"]["entries"][position]["excerpt_truncated"] =
                json!(next.len() < text.len() || candidates[index]["excerpt_truncated"] == true);
            if size(&report)? <= max_bytes {
                grew = true;
            } else {
                report["context"]["entries"][position] = old;
            }
        }
        if !grew {
            break;
        }
    }
    let omitted = candidates.len() - admitted.len();
    report["context"]["omitted_count"] = json!(omitted);
    if context_status == "ok" {
        report["context"]["status"] = json!(if omitted > 0 && admitted.is_empty() {
            "omitted_budget"
        } else {
            "ok"
        });
    }
    loop {
        let bytes = size(&report)?;
        if report["used_bytes"] == bytes {
            break;
        }
        report["used_bytes"] = json!(bytes);
    }
    ensure!(
        size(&report)? <= max_bytes,
        "bootstrap response exceeds max_bytes"
    );
    Ok(report)
}
