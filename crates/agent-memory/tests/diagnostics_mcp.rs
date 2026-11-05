use anyhow::Result;
use memory_sqlite::Store;
use rmcp::{
    model::{CallToolRequestParams, CallToolResult},
    transport::TokioChildProcess,
    Peer, RoleClient, ServiceExt,
};
use serde_json::{json, Value};
use std::{fs, path::Path, process::Command};

async fn call(client: &Peer<RoleClient>, tool: &str, args: Value) -> Result<CallToolResult> {
    Ok(client
        .call_tool(
            CallToolRequestParams::new(tool.to_owned())
                .with_arguments(args.as_object().unwrap().clone()),
        )
        .await?)
}
async fn start(db: &Path, extra: &[&str]) -> Result<rmcp::service::RunningService<RoleClient, ()>> {
    let mut command = tokio::process::Command::new(env!("CARGO_BIN_EXE_agent-memory"));
    command
        .arg("--database")
        .arg(db)
        .args(["mcp", "--project", "demo"])
        .args(extra);
    Ok(().serve(TokioChildProcess::new(command)?).await?)
}
fn cli(db: &Path, args: &[&str]) -> Result<Value> {
    let o = Command::new(env!("CARGO_BIN_EXE_agent-memory"))
        .arg("--database")
        .arg(db)
        .args(args)
        .output()?;
    anyhow::ensure!(o.status.success(), "{o:?}");
    Ok(serde_json::from_slice(&o.stdout)?)
}
fn note(id: &str) -> String {
    format!("---\nid: {id}\nmemory:\n  id: {id}\n  project: demo\n  kind: fact\n  status: accepted\n  summary: Claim {id}\n---\n# Claim\n")
}

#[tokio::test]
async fn runtime_is_connection_specific_scoped_and_matches_enabled_tools() -> Result<()> {
    let tmp = tempfile::tempdir()?;
    let notes = tmp.path().join("notes");
    fs::create_dir(&notes)?;
    let db = tmp.path().join("db");
    let store = Store::open(&db)?;
    store.create_project("demo")?;
    store.create_project("private")?;
    let source = store.register_source("demo", &notes, true)?;
    drop(store);
    let read = start(&db, &[]).await?;
    let peer_info = read.peer_info().unwrap();
    assert_eq!(peer_info.server_info.as_ref().unwrap().name, "agent-memory");
    assert_eq!(
        peer_info.server_info.as_ref().unwrap().version,
        env!("CARGO_PKG_VERSION")
    );
    let info = call(&read, "memory_runtime_info", json!({"project":"demo"}))
        .await?
        .structured_content
        .unwrap();
    let mut tools: Vec<_> = read
        .list_all_tools()
        .await?
        .iter()
        .map(|t| t.name.to_string())
        .collect();
    tools.sort();
    assert_eq!(info["tools"], json!(tools));
    assert_eq!(info["version"], env!("CARGO_PKG_VERSION"));
    assert_eq!(info["allow_write"], false);
    assert_eq!(info["git_sync"]["enabled"], false);
    assert_eq!(
        info["sources"],
        json!([{"source_id":source,"can_write":true}])
    );
    assert!(!info.to_string().contains(notes.to_str().unwrap()));
    assert_eq!(
        info,
        call(&read, "memory_runtime_info", json!({"project":"demo"}))
            .await?
            .structured_content
            .unwrap()
    );
    for tool in ["memory_health", "memory_runtime_info"] {
        assert_eq!(
            call(&read, tool, json!({"project":"private"}))
                .await?
                .is_error,
            Some(true)
        );
        assert_eq!(
            call(&read, tool, json!({"project":"demo","extra":true}))
                .await?
                .is_error,
            Some(true)
        );
    }
    let cli_info = cli(&db, &["runtime-info", "--project", "demo"])?;
    assert_eq!(cli_info["mode"], "cli");
    assert!(cli_info["mcp_runtime"].is_null());
    let write = start(
        &db,
        &[
            "--allow-write",
            "--git-sync-source",
            &source.to_string(),
            "--git-sync-after-write",
            "--git-sync-interval-secs",
            "86400",
        ],
    )
    .await?;
    let write_info = call(&write, "memory_runtime_info", json!({"project":"demo"}))
        .await?
        .structured_content
        .unwrap();
    assert_ne!(info["pid"], write_info["pid"]);
    assert_eq!(
        write_info["git_sync"],
        json!({"enabled":true,"source_ids":[source],"after_write":true,"interval_secs":86400,"remote_checked":false})
    );
    write.cancel().await?;
    read.cancel().await?;
    Ok(())
}

#[tokio::test]
async fn health_refreshes_matches_cli_and_reports_unknown_evidence_without_mutation() -> Result<()>
{
    let tmp = tempfile::tempdir()?;
    let notes = tmp.path().join("notes");
    fs::create_dir(&notes)?;
    let db = tmp.path().join("db");
    let store = Store::open(&db)?;
    store.create_project("demo")?;
    store.register_source("demo", &notes, false)?;
    drop(store);
    let client = start(&db, &[]).await?;
    assert_eq!(
        call(&client, "memory_health", json!({"project":"demo"}))
            .await?
            .structured_content
            .unwrap()["status"],
        "clear"
    );
    let a = note("a").replace("  summary: Claim a", "  relations:\n    - kind: contradicts\n      target: b\n  evidence:\n    - kind: file\n      reference: git:unconfigured@HEAD:file.rs\n  summary: Claim a") + "[Missing](missing.md)\n";
    fs::write(notes.join("a.md"), &a)?;
    fs::write(notes.join("b.md"), note("b"))?;
    let health = call(&client, "memory_health", json!({"project":"demo"}))
        .await?
        .structured_content
        .unwrap();
    assert_eq!(health["status"], "issues");
    assert_eq!(health["issue_count"], 2);
    assert_eq!(health["sections"]["evidence"]["status"], "partial");
    assert_eq!(health["sections"]["evidence"]["unknown_count"], 1);
    assert_eq!(
        health["sections"]["links"]["data"][0]["source"]["path"],
        "a.md"
    );
    assert_eq!(health, cli(&db, &["health", "--project", "demo"])?);
    assert_eq!(fs::read_to_string(notes.join("a.md"))?, a);
    assert!(!notes.join(".agent-memory-operations").exists());
    // A failed refresh must not silently report stale indexed findings as current.
    fs::write(notes.join("b.md"), "---\nmemory: invalid\n---\n")?;
    let failed = call(&client, "memory_health", json!({"project":"demo"}))
        .await?
        .structured_content
        .unwrap();
    assert_eq!(failed["status"], "incomplete");
    assert_eq!(failed["sections"]["index"]["status"], "error");
    assert_eq!(failed["sections"]["relations"]["status"], "skipped");
    assert_eq!(failed["sections"]["operations"]["status"], "ok");
    // Runtime information does not depend on a healthy Markdown index.
    assert_ne!(
        call(&client, "memory_runtime_info", json!({"project":"demo"}))
            .await?
            .is_error,
        Some(true)
    );
    client.cancel().await?;
    Ok(())
}

#[tokio::test]
async fn health_discovers_pending_batch_even_when_index_is_unusable() -> Result<()> {
    let tmp = tempfile::tempdir()?;
    let notes = tmp.path().join("notes");
    fs::create_dir(&notes)?;
    fs::write(notes.join("a.md"), note("a"))?;
    fs::write(notes.join("b.md"), note("b"))?;
    let db = tmp.path().join("db");
    let mut store = Store::open(&db)?;
    store.create_project("demo")?;
    let source = store.register_source("demo", &notes, true)?;
    store.index("demo")?;
    let edits = vec![
        memory_sqlite::BatchEdit {
            path: "a.md".into(),
            content: note("b"),
            expected_hash: Some(memory_core::content_hash(note("a").as_bytes())),
        },
        memory_sqlite::BatchEdit {
            path: "b.md".into(),
            content: note("a"),
            expected_hash: Some(memory_core::content_hash(note("b").as_bytes())),
        },
    ];
    let plan = store.batch_preview("demo", source, edits)?;
    let id = memory_core::save_operation(
        &notes,
        &serde_json::to_vec(
            &json!({"operation_kind":"batch_write","version":1,"nonce":"health-test","plan":plan}),
        )?,
    )?;
    fs::write(notes.join("a.md"), note("b"))?;
    drop(store);
    let client = start(&db, &[]).await?;
    let h = call(&client, "memory_health", json!({"project":"demo"}))
        .await?
        .structured_content
        .unwrap();
    assert_eq!(h["status"], "issues");
    assert_eq!(h["issue_count"], 1);
    assert_eq!(
        h["sections"]["operations"]["data"][0]["batch"]["data"][0]["operation_id"],
        id
    );
    assert_eq!(h["sections"]["index"]["status"], "error");
    assert_eq!(h, cli(&db, &["health", "--project", "demo"])?);
    assert_eq!(fs::read_to_string(notes.join("a.md"))?, note("b"));
    client.cancel().await?;
    Ok(())
}

#[tokio::test]
async fn health_isolates_corrupt_journals_and_ignores_unreadable_sources() -> Result<()> {
    let tmp = tempfile::tempdir()?;
    let notes = tmp.path().join("notes");
    let private = tmp.path().join("private");
    fs::create_dir(&notes)?;
    fs::create_dir(&private)?;
    fs::write(notes.join("a.md"), note("a"))?;
    fs::write(private.join("secret.md"), "[secret](absent.md)\n")?;
    let db = tmp.path().join("db");
    let mut store = Store::open(&db)?;
    store.create_project("demo")?;
    store.create_project("private")?;
    let source = store.register_source("demo", &notes, true)?;
    store.register_source("private", &private, false)?;
    let plan = store.rename_preview("demo", source, "a.md", "renamed.md")?;
    let id = memory_core::save_operation(
        &notes,
        &serde_json::to_vec(&json!({"version":1,"nonce":"health-rename","plan":plan}))?,
    )?;
    drop(store);
    let client = start(&db, &[]).await?;
    let h = call(&client, "memory_health", json!({"project":"demo"}))
        .await?
        .structured_content
        .unwrap();
    assert_eq!(h["issue_count"], 1);
    assert_eq!(
        h["sections"]["operations"]["data"][0]["rename"]["data"][0]["operation_id"],
        id
    );
    assert_eq!(h["sections"]["links"]["issue_count"], 0);
    assert_eq!(
        h["sections"]["operations"]["data"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert!(!h.to_string().contains("secret.md"));
    memory_core::save_operation(&notes, b"broken JSON")?;
    let corrupt = call(&client, "memory_health", json!({"project":"demo"}))
        .await?
        .structured_content
        .unwrap();
    assert_eq!(corrupt["sections"]["operations"]["status"], "partial");
    assert_eq!(corrupt["sections"]["links"]["status"], "ok");
    assert!(corrupt["incomplete_checks"].as_u64().unwrap() > 0);
    assert_eq!(fs::read_to_string(notes.join("a.md"))?, note("a"));
    client.cancel().await?;
    Ok(())
}
