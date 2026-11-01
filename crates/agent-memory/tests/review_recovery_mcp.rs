use anyhow::{Context, Result};
use memory_sqlite::Store;
use rmcp::{
    model::{CallToolRequestParams, CallToolResult},
    transport::TokioChildProcess,
    Peer, RoleClient, ServiceExt,
};
use serde_json::{json, Value};
use std::fs;

async fn call(client: &Peer<RoleClient>, name: &str, args: Value) -> Result<CallToolResult> {
    Ok(client
        .call_tool(
            CallToolRequestParams::new(name.to_owned())
                .with_arguments(args.as_object().context("object arguments")?.clone()),
        )
        .await?)
}

#[tokio::test]
async fn review_queue_is_derived_unknown_is_not_false_and_access_is_current() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let notes = temp.path().join("notes");
    fs::create_dir(&notes)?;
    let document = "---\nmemory:\n  id: claim\n  project: demo\n  kind: fact\n  status: accepted\n  summary: Claim requiring review\n  evidence:\n    - kind: file\n      reference: git:unconfigured@0123456789abcdef0123456789abcdef01234567:code.rs\n---\n# Claim\n";
    fs::write(notes.join("claim.md"), document)?;
    let db = temp.path().join("memory.sqlite");
    let store = Store::open(&db)?;
    store.create_project("demo")?;
    store.create_project("other")?;
    let source = store.register_source("demo", &notes, true)?;
    store.register_source("other", &notes, true)?;
    let mut command = tokio::process::Command::new(env!("CARGO_BIN_EXE_agent-memory"));
    command
        .arg("--database")
        .arg(&db)
        .args(["mcp", "--project", "demo", "--project", "other"]);
    let client = ().serve(TokioChildProcess::new(command)?).await?;
    let tools = client.list_all_tools().await?;
    assert!(tools.iter().any(|t| t.name == "memory_review_queue"));
    assert!(tools.iter().any(|t| t.name == "memory_rename_operations"));
    assert!(!tools.iter().any(|t| t.name == "memory_rename_resume"));
    let default = call(&client, "memory_review_queue", json!({"project":"demo"})).await?;
    assert_ne!(default.is_error, Some(true));
    let default = default.structured_content.context("queue")?;
    assert_eq!(default["unknown_count"], 1);
    assert!(default["entries"].as_array().unwrap().is_empty());
    let all = call(
        &client,
        "memory_review_queue",
        json!({"project":"demo","include_unknown":true}),
    )
    .await?;
    let all = all.structured_content.context("unknown queue")?;
    assert_eq!(all["entries"][0]["reasons"], json!(["unknown"]));
    assert_eq!(all["entries"][0]["memory"]["memory"]["status"], "accepted");
    let hash = all["entries"][0]["memory"]["content_hash"].clone();
    for project in ["other", "forbidden"] {
        let result = call(
            &client,
            "memory_review_queue",
            json!({"project":project,"include_unknown":true}),
        )
        .await?;
        if project == "forbidden" {
            assert_eq!(result.is_error, Some(true));
        } else {
            assert!(result.structured_content.unwrap()["entries"]
                .as_array()
                .unwrap()
                .is_empty());
        }
    }
    assert_eq!(fs::read_to_string(notes.join("claim.md"))?, document);
    assert!(!notes.join(".agent-memory-history").exists());
    // A refreshed queue observes external status changes without acknowledging a stale hash.
    let updated = document.replace("status: accepted", "status: deprecated");
    fs::write(notes.join("claim.md"), &updated)?;
    let refreshed = call(
        &client,
        "memory_review_queue",
        json!({"project":"demo","include_unknown":true}),
    )
    .await?;
    assert!(refreshed.structured_content.unwrap()["entries"]
        .as_array()
        .unwrap()
        .is_empty());
    let stale = call(
        &client,
        "memory_get",
        json!({"project":"demo","source_id":source,"path":"claim.md","content_hash":hash}),
    )
    .await?;
    assert_eq!(stale.is_error, Some(true));
    let connection = rusqlite::Connection::open(&db)?;
    connection.execute(
        "UPDATE project_sources SET can_read=0 WHERE source_id=?1",
        [source],
    )?;
    let revoked = call(
        &client,
        "memory_review_queue",
        json!({"project":"demo","include_unknown":true}),
    )
    .await?;
    assert!(revoked.structured_content.unwrap()["entries"]
        .as_array()
        .unwrap()
        .is_empty());
    let denied = call(
        &client,
        "memory_rename_operations",
        json!({"project":"demo","source_id":source}),
    )
    .await?;
    assert_eq!(denied.is_error, Some(true));
    assert_eq!(fs::read_to_string(notes.join("claim.md"))?, updated);
    client.cancel().await?;
    Ok(())
}

#[tokio::test]
async fn rename_recovery_is_scoped_idempotent_and_rechecks_write_permission() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let notes = temp.path().join("notes");
    fs::create_dir(&notes)?;
    fs::write(notes.join("from.md"), "---\nid: stable\n---\n# Original\n")?;
    fs::write(notes.join("ref.md"), "[Original](from.md)\n")?;
    let db = temp.path().join("memory.sqlite");
    let store = Store::open(&db)?;
    store.create_project("demo")?;
    store.create_project("other")?;
    let source = store.register_source("demo", &notes, true)?;
    store.register_source("other", &notes, true)?;
    let mut command = tokio::process::Command::new(env!("CARGO_BIN_EXE_agent-memory"));
    command.arg("--database").arg(&db).args([
        "mcp",
        "--project",
        "demo",
        "--project",
        "other",
        "--allow-write",
    ]);
    let client = ().serve(TokioChildProcess::new(command)?).await?;
    assert!(client
        .list_all_tools()
        .await?
        .iter()
        .any(|t| t.name == "memory_rename_resume"));
    let preview = call(
        &client,
        "memory_rename",
        json!({"project":"demo","source_id":source,"from":"from.md","to":"to.md"}),
    )
    .await?;
    let hash = preview.structured_content.context("preview")?["plan_hash"].clone();
    let applied = call(&client,"memory_rename",json!({"project":"demo","source_id":source,"from":"from.md","to":"to.md","expected_plan_hash":hash})).await?;
    assert_ne!(applied.is_error, Some(true));
    let applied = applied.structured_content.context("applied")?;
    assert_eq!(applied["complete"], true, "{applied}");
    let id = applied["operation_id"].clone();
    let history = store.list_revisions("demo", source, "to.md")?.len();
    let operations = call(
        &client,
        "memory_rename_operations",
        json!({"project":"demo","source_id":source}),
    )
    .await?;
    let operations = operations.structured_content.context("operations")?;
    assert_eq!(operations[0]["operation_id"], id);
    assert_eq!(operations[0]["complete"], true);
    let foreign = call(
        &client,
        "memory_rename_operations",
        json!({"project":"other","source_id":source}),
    )
    .await?;
    assert!(foreign
        .structured_content
        .unwrap()
        .as_array()
        .unwrap()
        .is_empty());
    let foreign_resume = call(
        &client,
        "memory_rename_resume",
        json!({"project":"other","source_id":source,"operation_id":id}),
    )
    .await?;
    assert_eq!(foreign_resume.is_error, Some(true));
    let repeated = call(
        &client,
        "memory_rename_resume",
        json!({"project":"demo","source_id":source,"operation_id":id}),
    )
    .await?;
    assert_eq!(repeated.structured_content.unwrap()["complete"], true);
    assert_eq!(
        store.list_revisions("demo", source, "to.md")?.len(),
        history
    );
    // Completed operations are historical: retry must never restore over a later edit.
    fs::write(notes.join("to.md"), "# Later owner edit\n")?;
    call(
        &client,
        "memory_rename_resume",
        json!({"project":"demo","source_id":source,"operation_id":id}),
    )
    .await?;
    assert_eq!(
        fs::read_to_string(notes.join("to.md"))?,
        "# Later owner edit\n"
    );
    store.register_source("demo", &notes, false)?;
    let revoked = call(
        &client,
        "memory_rename_resume",
        json!({"project":"demo","source_id":source,"operation_id":id}),
    )
    .await?;
    assert_eq!(revoked.is_error, Some(true));
    // A corrupt journal is rejected rather than treated as a replayable plan.
    let operation_path = notes
        .join(".agent-memory-operations")
        .join(format!("{}.json", id.as_str().unwrap()));
    fs::write(operation_path, "{}")?;
    let corrupt = call(
        &client,
        "memory_rename_operations",
        json!({"project":"demo","source_id":source}),
    )
    .await?;
    assert_eq!(corrupt.is_error, Some(true));
    assert_eq!(
        fs::read_to_string(notes.join("to.md"))?,
        "# Later owner edit\n"
    );
    client.cancel().await?;
    Ok(())
}
