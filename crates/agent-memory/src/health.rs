//! Derived local checks. Errors are data so recovery remains discoverable.
use anyhow::Result;
use memory_sqlite::Store;
use serde_json::{json, Value};

fn section(result: Result<(Value, usize, usize)>) -> Value {
    match result {
        Ok((data, issues, unknown)) => json!({
            "status":if unknown > 0 {"partial"} else {"ok"},
            "issue_count":issues,"unknown_count":unknown,"data":data
        }),
        Err(error) => {
            json!({"status":"error","issue_count":0,"unknown_count":0,"error":format!("{error:#}")})
        }
    }
}

pub fn report(
    store: &mut Store,
    repositories: &crate::git_evidence::Repositories,
    project: &str,
) -> Result<Value> {
    // Authorize before any index, journal or evidence access.
    let sources = store.sources(project)?;
    let refresh = store.index(project);
    let mut sections = serde_json::Map::new();
    let indexed = refresh.is_ok();
    sections.insert(
        "index".into(),
        section(refresh.map(|_| (json!({"refreshed":true}), 0, 0))),
    );
    if indexed {
        sections.insert(
            "links".into(),
            section(
                store
                    .link_diagnostics(project, None, None)
                    .and_then(|links| {
                        let count = links.len();
                        Ok((serde_json::to_value(links)?, count, 0))
                    }),
            ),
        );
        sections.insert(
            "relations".into(),
            section(store.contradiction_diagnostics(project).map(|v| {
                let count = v["issue_count"].as_u64().unwrap_or(0) as usize;
                (v, count, 0)
            })),
        );
        sections.insert(
            "evidence".into(),
            section(
                crate::review::queue(store, repositories, project, true).map(|v| {
                    let count = v["entries"]
                        .as_array()
                        .map(|entries| {
                            entries
                                .iter()
                                .filter(|e| {
                                    e["reasons"].as_array().is_some_and(|r| {
                                        r.iter().any(|s| s == "changed" || s == "missing")
                                    })
                                })
                                .count()
                        })
                        .unwrap_or(0);
                    let unknown = v["unknown_count"].as_u64().unwrap_or(0) as usize;
                    (v, count, unknown)
                }),
            ),
        );
    } else {
        for name in ["links", "relations", "evidence"] {
            sections.insert(name.into(), json!({"status":"skipped","reason":"index_refresh_failed","issue_count":0,"unknown_count":0}));
        }
    }
    let mut journals = Vec::new();
    let mut pending = 0;
    let mut failed = 0;
    for source in sources {
        let rename = section(
            store
                .rename_operations(project, source.source_id)
                .and_then(|ops| {
                    let count = ops.iter().filter(|o| !o.complete).count();
                    let incomplete: Vec<_> = ops.into_iter().filter(|o| !o.complete).collect();
                    Ok((serde_json::to_value(incomplete)?, count, 0))
                }),
        );
        let batch = section(store.batch_operations(project, source.source_id).map(|v| {
            let incomplete: Vec<_> = v["operations"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|o| o["complete"] == false)
                .cloned()
                .collect();
            let count = incomplete.len();
            (json!(incomplete), count, 0)
        }));
        for s in [&rename, &batch] {
            pending += s["issue_count"].as_u64().unwrap_or(0);
            failed += u64::from(s["status"] == "error");
        }
        journals.push(json!({"source_id":source.source_id,"rename":rename,"batch":batch}));
    }
    sections.insert(
        "operations".into(),
        json!({"status":if failed > 0 {"partial"} else {"ok"},
        "issue_count":pending,"unknown_count":failed,"data":journals}),
    );
    let issue_count: u64 = sections
        .values()
        .filter_map(|v| v["issue_count"].as_u64())
        .sum();
    let incomplete_checks = sections.values().filter(|v| v["status"] != "ok").count();
    Ok(
        json!({"project":project,"local_only":true,"remote_checked":false,
        "status":if issue_count > 0 {"issues"} else if incomplete_checks > 0 {"incomplete"} else {"clear"},
        "issue_count":issue_count,"incomplete_checks":incomplete_checks,"sections":sections,
        "warnings":["Checks observe local state sequentially, not an atomic snapshot.",
            "Only supported git: evidence on active records is verified; other references and claim truth are not checked.",
            "Clear means no detected issues in these checks, not semantic consistency or remote freshness."]}),
    )
}
