#![cfg(unix)]

use anyhow::{Context, Result};
use memory_sqlite::Store;
use rmcp::{
    model::{CallToolRequestParams, CallToolResult},
    transport::TokioChildProcess,
    Peer, RoleClient, ServiceExt,
};
use serde_json::{json, Value};
use std::{
    fs,
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};

struct Daemon(Child);
impl Drop for Daemon {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

async fn call(client: &Peer<RoleClient>, name: &str, args: Value) -> Result<CallToolResult> {
    Ok(client
        .call_tool(
            CallToolRequestParams::new(name.to_owned())
                .with_arguments(args.as_object().context("object arguments")?.clone()),
        )
        .await?)
}

#[tokio::test]
async fn mcp_refresh_keeps_scope_and_current_hash_guards_with_daemon_and_fallback() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let database = temp.path().join("memory.sqlite");
    let demo = temp.path().join("demo");
    let other = temp.path().join("other");
    fs::create_dir(&demo)?;
    fs::create_dir(&other)?;
    fs::write(demo.join("note.md"), "# Demo\ndaemonfirstmarker\n")?;
    fs::write(other.join("note.md"), "# Other\nforeignfirstmarker\n")?;
    let store = Store::open(&database)?;
    store.create_project("demo")?;
    store.create_project("other")?;
    let source = store.register_source("demo", &demo, false)?;
    store.register_source("other", &other, false)?;
    drop(store);
    let mut daemon = Daemon(
        Command::new(env!("CARGO_BIN_EXE_agent-memory"))
            .arg("--database")
            .arg(&database)
            .args([
                "daemon",
                "run",
                "--project",
                "demo",
                "--poll-interval-ms",
                "500",
                "--reconcile-interval-secs",
                "30",
            ])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()?,
    );
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if Store::open(&database)?
            .search("demo", "daemonfirstmarker")?
            .len()
            == 1
        {
            break;
        }
        anyhow::ensure!(
            daemon.0.try_wait()?.is_none(),
            "daemon exited before startup"
        );
        anyhow::ensure!(Instant::now() < deadline, "daemon startup timed out");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let mut command = tokio::process::Command::new(env!("CARGO_BIN_EXE_agent-memory"));
    command.arg("--database").arg(&database).args([
        "mcp",
        "--project",
        "demo",
        "--project",
        "other",
    ]);
    let client = ().serve(TokioChildProcess::new(command)?).await?;
    let first = call(
        &client,
        "memory_search",
        json!({"project":"demo","query":"daemonfirstmarker"}),
    )
    .await?;
    assert_ne!(first.is_error, Some(true));
    let hash = first.structured_content.context("initial hits")?["hits"][0]["content_hash"].clone();
    fs::write(demo.join("note.md"), "# Demo\ndaemonfreshmarker\n")?;
    let fresh = call(
        &client,
        "memory_search",
        json!({"project":"demo","query":"daemonfreshmarker"}),
    )
    .await?;
    assert_ne!(fresh.is_error, Some(true));
    assert_eq!(
        fresh.structured_content.context("fresh hits")?["hits"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    let stale = call(
        &client,
        "memory_get",
        json!({"project":"demo","source_id":source,"path":"note.md","content_hash":hash}),
    )
    .await?;
    assert_eq!(stale.is_error, Some(true));
    // A daemon that does not serve another allowed MCP project must not block fallback.
    fs::write(other.join("note.md"), "# Other\nforeignfreshmarker\n")?;
    let foreign = call(
        &client,
        "memory_search",
        json!({"project":"other","query":"foreignfreshmarker"}),
    )
    .await?;
    assert_ne!(foreign.is_error, Some(true));
    assert_eq!(
        foreign.structured_content.context("other hits")?["hits"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    let isolated = call(
        &client,
        "memory_search",
        json!({"project":"demo","query":"foreignfreshmarker"}),
    )
    .await?;
    assert!(isolated.structured_content.unwrap()["hits"]
        .as_array()
        .unwrap()
        .is_empty());
    let forbidden = call(
        &client,
        "memory_search",
        json!({"project":"forbidden","query":"foreignfreshmarker"}),
    )
    .await?;
    assert_eq!(forbidden.is_error, Some(true));
    // A failed refresh must not serve last-good results as if they were current.
    fs::write(
        demo.join("broken.md"),
        "---\nupdated: invalid\n---\n# Broken\n",
    )?;
    let broken = call(
        &client,
        "memory_search",
        json!({"project":"demo","query":"daemonfreshmarker"}),
    )
    .await?;
    assert_eq!(broken.is_error, Some(true));
    fs::remove_file(demo.join("broken.md"))?;
    daemon.0.kill()?;
    daemon.0.wait()?;
    fs::write(demo.join("note.md"), "# Demo\nstandalonefallbackmarker\n")?;
    let fallback = call(
        &client,
        "memory_search",
        json!({"project":"demo","query":"standalonefallbackmarker"}),
    )
    .await?;
    assert_ne!(fallback.is_error, Some(true));
    assert_eq!(
        fallback.structured_content.context("fallback hits")?["hits"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    let connection = rusqlite::Connection::open(&database)?;
    connection.execute(
        "UPDATE project_sources SET can_read=0 WHERE source_id=?1",
        [source],
    )?;
    let revoked = call(
        &client,
        "memory_search",
        json!({"project":"demo","query":"standalonefallbackmarker"}),
    )
    .await?;
    assert!(revoked.structured_content.unwrap()["hits"]
        .as_array()
        .unwrap()
        .is_empty());
    assert!(!demo.join(".agent-memory.lock").exists());
    assert!(!demo.join(".agent-memory-history").exists());
    assert_eq!(
        fs::read_to_string(demo.join("note.md"))?,
        "# Demo\nstandalonefallbackmarker\n"
    );
    client.cancel().await?;
    Ok(())
}
