use std::{path::Path, process::Command};

fn git(root: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}

fn cli(db: &Path, args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_agent-memory"))
        .arg("--database")
        .arg(db)
        .args(args)
        .output()
        .unwrap()
}

#[test]
fn explicit_cli_sync_pushes_only_markdown_and_requires_writable_binding() {
    let temp = tempfile::tempdir().unwrap();
    let remote = temp.path().join("remote.git");
    let notes = temp.path().join("notes");
    std::fs::create_dir(&notes).unwrap();
    git(temp.path(), &["init", "--bare", remote.to_str().unwrap()]);
    git(&notes, &["init", "-b", "main"]);
    git(&notes, &["config", "user.name", "Memory Test"]);
    git(&notes, &["config", "user.email", "memory@example.invalid"]);
    std::fs::write(notes.join("initial.md"), "# Initial\n").unwrap();
    git(&notes, &["add", "initial.md"]);
    git(&notes, &["commit", "-m", "Initial"]);
    git(
        &notes,
        &["remote", "add", "origin", remote.to_str().unwrap()],
    );
    git(&notes, &["push", "-u", "origin", "main"]);
    let db = temp.path().join("index.sqlite");
    let source_id = {
        let mut store = memory_sqlite::Store::open(&db).unwrap();
        store.create_project("demo").unwrap();
        let id = store.register_source("demo", &notes, true).unwrap();
        store.index("demo").unwrap();
        id.to_string()
    };
    std::fs::write(notes.join("new.md"), "# New\n").unwrap();
    std::fs::write(notes.join("index.sqlite"), "not markdown").unwrap();
    let result = cli(
        &db,
        &["git-sync", "--project", "demo", "--source-id", &source_id],
    );
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert_eq!(
        git(&notes, &["rev-parse", "HEAD"]),
        git(&remote, &["rev-parse", "refs/heads/main"])
    );
    assert!(git(&notes, &["ls-tree", "--name-only", "HEAD"]).contains("new.md"));
    assert!(!git(&notes, &["ls-tree", "--name-only", "HEAD"]).contains("index.sqlite"));
    let store = memory_sqlite::Store::open(&db).unwrap();
    store.register_source("demo", &notes, false).unwrap();
    std::fs::write(notes.join("denied.md"), "# Denied\n").unwrap();
    assert!(!cli(
        &db,
        &["git-sync", "--project", "demo", "--source-id", &source_id]
    )
    .status
    .success());
    assert!(!git(&notes, &["ls-tree", "--name-only", "HEAD"]).contains("denied.md"));
}

#[test]
fn mcp_sync_configuration_rejects_missing_write_authority_and_invalid_interval() {
    let temp = tempfile::tempdir().unwrap();
    let db = temp.path().join("index.sqlite");
    let store = memory_sqlite::Store::open(&db).unwrap();
    store.create_project("demo").unwrap();
    for args in [
        vec!["mcp", "--project", "demo", "--git-sync-source", "1"],
        vec![
            "mcp",
            "--project",
            "demo",
            "--allow-write",
            "--git-sync-after-write",
        ],
        vec![
            "mcp",
            "--project",
            "demo",
            "--allow-write",
            "--git-sync-source",
            "1",
            "--git-sync-interval-secs",
            "0",
        ],
    ] {
        assert!(!cli(&db, &args).status.success());
    }
}

#[tokio::test]
async fn after_write_pushes_and_reports_push_failure_without_losing_markdown() -> anyhow::Result<()>
{
    use rmcp::{model::CallToolRequestParams, transport::TokioChildProcess, ServiceExt};
    use serde_json::json;
    let temp = tempfile::tempdir()?;
    let remote = temp.path().join("remote.git");
    let notes = temp.path().join("notes");
    std::fs::create_dir(&notes)?;
    git(temp.path(), &["init", "--bare", remote.to_str().unwrap()]);
    git(&notes, &["init", "-b", "main"]);
    git(&notes, &["config", "user.name", "Memory Test"]);
    git(&notes, &["config", "user.email", "memory@example.invalid"]);
    std::fs::write(notes.join("initial.md"), "# Initial\n")?;
    git(&notes, &["add", "initial.md"]);
    git(&notes, &["commit", "-m", "Initial"]);
    git(
        &notes,
        &["remote", "add", "origin", remote.to_str().unwrap()],
    );
    git(&notes, &["push", "-u", "origin", "main"]);
    let db = temp.path().join("index.sqlite");
    let id = {
        let store = memory_sqlite::Store::open(&db)?;
        store.create_project("demo")?;
        store.register_source("demo", &notes, true)?
    };
    let mut command = tokio::process::Command::new(env!("CARGO_BIN_EXE_agent-memory"));
    command.arg("--database").arg(&db).args([
        "mcp",
        "--project",
        "demo",
        "--allow-write",
        "--git-sync-source",
        &id.to_string(),
        "--git-sync-after-write",
        "--git-sync-interval-secs",
        "3600",
    ]);
    let client = ().serve(TokioChildProcess::new(command)?).await?;
    for (path, status) in [("first.md", "ok"), ("second.md", "error")] {
        if status == "error" {
            git(
                &notes,
                &[
                    "remote",
                    "set-url",
                    "origin",
                    temp.path().join("missing.git").to_str().unwrap(),
                ],
            );
        }
        let result = client
            .call_tool(
                CallToolRequestParams::new("memory_write").with_arguments(
                    json!({"project":"demo","source_id":id,"path":path,"content":"# Saved\n"})
                        .as_object()
                        .unwrap()
                        .clone(),
                ),
            )
            .await?;
        assert_ne!(result.is_error, Some(true));
        let value = result.structured_content.unwrap();
        assert_eq!(value["indexed"], true);
        assert_eq!(value["git_sync"]["status"], status);
        assert_eq!(std::fs::read_to_string(notes.join(path))?, "# Saved\n");
    }
    assert_eq!(
        git(&remote, &["show", "refs/heads/main:first.md"]),
        "# Saved"
    );
    assert_eq!(git(&notes, &["show", "HEAD:second.md"]), "# Saved");
    client.cancel().await?;
    Ok(())
}

#[tokio::test]
async fn timer_uses_environment_interval_and_preserves_protocol() -> anyhow::Result<()> {
    use rmcp::{transport::TokioChildProcess, ServiceExt};
    let temp = tempfile::tempdir()?;
    let remote = temp.path().join("remote.git");
    let notes = temp.path().join("notes");
    std::fs::create_dir(&notes)?;
    git(temp.path(), &["init", "--bare", remote.to_str().unwrap()]);
    git(&notes, &["init", "-b", "main"]);
    git(&notes, &["config", "user.name", "Memory Test"]);
    git(&notes, &["config", "user.email", "memory@example.invalid"]);
    std::fs::write(notes.join("initial.md"), "# Initial\n")?;
    git(&notes, &["add", "initial.md"]);
    git(&notes, &["commit", "-m", "Initial"]);
    git(
        &notes,
        &["remote", "add", "origin", remote.to_str().unwrap()],
    );
    git(&notes, &["push", "-u", "origin", "main"]);
    let db = temp.path().join("index.sqlite");
    let id = {
        let store = memory_sqlite::Store::open(&db)?;
        store.create_project("demo")?;
        store.register_source("demo", &notes, true)?
    };
    let mut command = tokio::process::Command::new(env!("CARGO_BIN_EXE_agent-memory"));
    command
        .arg("--database")
        .arg(&db)
        .args([
            "mcp",
            "--project",
            "demo",
            "--allow-write",
            "--git-sync-source",
            &id.to_string(),
        ])
        .env("AGENT_MEMORY_GIT_SYNC_INTERVAL_SECS", "1");
    let client = ().serve(TokioChildProcess::new(command)?).await?;
    std::fs::write(notes.join("timer.md"), "# Timer\n")?;
    tokio::time::timeout(std::time::Duration::from_secs(15), async {
        loop {
            let output = Command::new("git")
                .arg("-C")
                .arg(&remote)
                .args(["cat-file", "-e", "refs/heads/main:timer.md"])
                .output()
                .unwrap();
            if output.status.success() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
    })
    .await?;
    assert!(!client.list_all_tools().await?.is_empty());
    let store = memory_sqlite::Store::open(&db)?;
    store.register_source("demo", &notes, false)?;
    std::fs::write(notes.join("revoked.md"), "# Revoked\n")?;
    tokio::time::sleep(std::time::Duration::from_millis(2200)).await;
    let output = Command::new("git")
        .arg("-C")
        .arg(&remote)
        .args(["cat-file", "-e", "refs/heads/main:revoked.md"])
        .output()?;
    assert!(
        !output.status.success(),
        "revoked source must not be pushed"
    );
    client.cancel().await?;
    Ok(())
}
