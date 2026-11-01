use anyhow::{ensure, Result};
use rmcp::{model::CallToolRequestParams, transport::TokioChildProcess, ServiceExt};
use serde_json::{json, Value};
use std::{fs, path::Path, process::Command};

fn git(root: &Path, args: &[&str]) -> Result<()> {
    let output = Command::new("git").current_dir(root).args(args).output()?;
    ensure!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(())
}

#[tokio::test]
async fn remote_pull_refreshes_knowledge_and_checks_permissions() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let remote = temp.path().join("remote.git");
    let root = temp.path().join("notes");
    let other = temp.path().join("other");
    git(temp.path(), &["init", "--bare", remote.to_str().unwrap()])?;
    git(
        temp.path(),
        &["clone", remote.to_str().unwrap(), root.to_str().unwrap()],
    )?;
    git(&root, &["config", "user.name", "Test"])?;
    git(&root, &["config", "user.email", "test@example.test"])?;
    fs::write(root.join("seed.md"), "# Seed\n")?;
    git(&root, &["add", "."])?;
    git(&root, &["commit", "-m", "Seed"])?;
    git(&root, &["push", "-u", "origin", "HEAD"])?;
    git(
        temp.path(),
        &["clone", remote.to_str().unwrap(), other.to_str().unwrap()],
    )?;
    git(&other, &["config", "user.name", "Test"])?;
    git(&other, &["config", "user.email", "test@example.test"])?;
    let db = temp.path().join("index.sqlite");
    let source = {
        let store = memory_sqlite::Store::open(&db)?;
        store.create_project("demo")?;
        store.create_project("denied")?;
        store.register_source("demo", &root, true)?
    };
    let mut command = tokio::process::Command::new(env!("CARGO_BIN_EXE_agent-memory"));
    command.arg("--database").arg(&db).args([
        "mcp",
        "--project",
        "demo",
        "--project",
        "denied",
        "--allow-write",
    ]);
    let client = ().serve(TokioChildProcess::new(command)?).await?;
    let args = |name: &str, value: Value| {
        CallToolRequestParams::new(name.to_owned())
            .with_arguments(value.as_object().unwrap().clone())
    };
    let baseline = client
        .call_tool(args("memory_changes", json!({"project":"demo"})))
        .await?;
    let cursor = baseline.structured_content.unwrap()["cursor"].clone();
    fs::write(
        other.join("new.md"),
        "# Remote knowledge\nremotepullmarker\n",
    )?;
    git(&other, &["add", "."])?;
    git(&other, &["commit", "-m", "Remote note"])?;
    git(&other, &["push"])?;
    let cached = client
        .call_tool(args(
            "memory_freshness",
            json!({"project":"demo","source_id":source}),
        ))
        .await?;
    assert_eq!(
        cached.structured_content.unwrap()["freshness"]["checked_remote"],
        false
    );
    let checked = client
        .call_tool(args(
            "memory_freshness",
            json!({"project":"demo","source_id":source,"fetch":true}),
        ))
        .await?;
    assert_ne!(checked.is_error, Some(true));
    assert_eq!(
        checked.structured_content.unwrap()["freshness"]["status"],
        "behind"
    );
    let denied = client
        .call_tool(args(
            "memory_pull",
            json!({"project":"denied","source_id":source}),
        ))
        .await?;
    assert_eq!(denied.is_error, Some(true));
    let pulled = client
        .call_tool(args(
            "memory_pull",
            json!({"project":"demo","source_id":source}),
        ))
        .await?;
    assert_ne!(pulled.is_error, Some(true), "{pulled:?}");
    assert_eq!(pulled.structured_content.unwrap()["indexed"], true);
    let changes = client
        .call_tool(args(
            "memory_changes",
            json!({"project":"demo","since":cursor}),
        ))
        .await?;
    assert_eq!(
        changes.structured_content.unwrap()["changes"][0]["path"],
        "new.md"
    );
    let found = client
        .call_tool(args(
            "memory_search",
            json!({"project":"demo","query":"remotepullmarker"}),
        ))
        .await?;
    assert_eq!(
        found.structured_content.unwrap()["hits"][0]["path"],
        "new.md"
    );
    let connection = rusqlite::Connection::open(&db)?;
    connection.execute("UPDATE project_sources SET can_write=0", [])?;
    let denied = client
        .call_tool(args(
            "memory_pull",
            json!({"project":"demo","source_id":source}),
        ))
        .await?;
    assert_eq!(denied.is_error, Some(true));
    client.cancel().await?;
    Ok(())
}
