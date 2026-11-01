use anyhow::Result;
use memory_sqlite::Store;
use rmcp::{model::CallToolRequestParams, transport::TokioChildProcess, ServiceExt};
use serde_json::json;
use std::{fs, process::Command};

#[tokio::test]
async fn evidence_over_stdio_is_read_only_scoped_and_refreshed() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let code = temp.path().join("code");
    let notes = temp.path().join("notes");
    fs::create_dir(&code)?;
    fs::create_dir(&notes)?;
    for args in [
        vec!["init", "-q"],
        vec!["config", "user.name", "Test"],
        vec!["config", "user.email", "test@example.invalid"],
    ] {
        assert!(Command::new("git")
            .arg("-C")
            .arg(&code)
            .args(args)
            .status()?
            .success());
    }
    fs::write(code.join("code.rs"), "original\n")?;
    for args in [vec!["add", "."], vec!["commit", "-qm", "baseline"]] {
        assert!(Command::new("git")
            .arg("-C")
            .arg(&code)
            .args(args)
            .status()?
            .success());
    }
    let output = Command::new("git")
        .arg("-C")
        .arg(&code)
        .args(["rev-parse", "HEAD"])
        .output()?;
    let commit = String::from_utf8(output.stdout)?.trim().to_owned();
    let document = format!("---\nmemory:\n  id: claim\n  project: demo\n  kind: fact\n  status: accepted\n  summary: Code evidence\n  evidence:\n    - kind: file\n      reference: git:code@{commit}:code.rs\n---\n# Claim\n");
    fs::write(notes.join("claim.md"), &document)?;
    let database = temp.path().join("memory.sqlite");
    let mut store = Store::open(&database)?;
    store.create_project("demo")?;
    store.create_project("other")?;
    let source = store.register_source("demo", &notes, false)?;
    let mut child = tokio::process::Command::new(env!("CARGO_BIN_EXE_agent-memory"));
    child
        .arg("--database")
        .arg(&database)
        .args([
            "mcp",
            "--project",
            "demo",
            "--project",
            "other",
            "--evidence-repository",
        ])
        .arg(format!("demo:code={}", code.display()));
    let client = ().serve(TokioChildProcess::new(child)?).await?;
    assert!(client
        .list_all_tools()
        .await?
        .iter()
        .any(|t| t.name == "memory_verify_evidence"));
    for (project, status) in [
        ("demo", "unchanged"),
        ("other", "denied"),
        ("not-allowed", "denied"),
    ] {
        let result = client
            .call_tool(
                CallToolRequestParams::new("memory_verify_evidence").with_arguments(
                    json!({"project":project,"memory_id":"claim"})
                        .as_object()
                        .unwrap()
                        .clone(),
                ),
            )
            .await?;
        if status == "denied" {
            assert_eq!(result.is_error, Some(true));
        } else {
            assert_eq!(
                result.structured_content.unwrap()["evidence"][0]["status"],
                status
            );
        }
    }
    fs::write(code.join("code.rs"), "dirty\n")?;
    let result = client
        .call_tool(
            CallToolRequestParams::new("memory_verify_evidence").with_arguments(
                json!({"project":"demo","memory_id":"claim"})
                    .as_object()
                    .unwrap()
                    .clone(),
            ),
        )
        .await?;
    assert_eq!(
        result.structured_content.unwrap()["evidence"][0]["status"],
        "changed"
    );
    // Current SQL source rights still govern access even with a configured repository.
    let connection = rusqlite::Connection::open(&database)?;
    connection.execute(
        "UPDATE project_sources SET can_read=0 WHERE source_id=?1",
        [source],
    )?;
    let denied = client
        .call_tool(
            CallToolRequestParams::new("memory_verify_evidence").with_arguments(
                json!({"project":"demo","memory_id":"claim"})
                    .as_object()
                    .unwrap()
                    .clone(),
            ),
        )
        .await?;
    assert_eq!(denied.is_error, Some(true));
    assert_eq!(fs::read_to_string(notes.join("claim.md"))?, document);
    assert!(!notes.join(".agent-memory-history").exists());
    client.cancel().await?;
    // CLI uses the same checker; it refuses unavailable memory after revocation.
    store.index("other")?;
    Ok(())
}
