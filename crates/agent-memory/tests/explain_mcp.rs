use anyhow::Result;
use memory_sqlite::Store;
use rmcp::{model::CallToolRequestParams, transport::TokioChildProcess, ServiceExt};
use serde_json::{json, Value};
use std::{fs, path::Path, process::Command};

fn cli(db: &Path, args: &[&str]) -> Result<Value> {
    let output = Command::new(env!("CARGO_BIN_EXE_agent-memory"))
        .arg("--database")
        .arg(db)
        .args(args)
        .output()?;
    anyhow::ensure!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(serde_json::from_slice(&output.stdout)?)
}

#[tokio::test]
async fn explain_cli_mcp_parity_defaults_schema_and_validation() -> Result<()> {
    let tmp = tempfile::tempdir()?;
    let root = tmp.path().join("notes");
    fs::create_dir(&root)?;
    fs::write(
        root.join("one.md"),
        "# Shipping\nalpha beta\n## Other\nbeta\n",
    )?;
    let db = tmp.path().join("index");
    let mut store = Store::open(&db)?;
    store.create_project("demo")?;
    store.add_source("demo", &root)?;
    store.index("demo")?;
    drop(store);
    let mut command = tokio::process::Command::new(env!("CARGO_BIN_EXE_agent-memory"));
    command
        .arg("--database")
        .arg(&db)
        .args(["mcp", "--project", "demo"]);
    let client = ().serve(TokioChildProcess::new(command)?).await?;
    let tools = client.list_all_tools().await?;
    for name in ["memory_search", "memory_context"] {
        let tool = tools.iter().find(|t| t.name == name).unwrap();
        assert_eq!(
            tool.input_schema["properties"]["explain"]["type"],
            "boolean"
        );
    }
    for mode in ["fts", "natural"] {
        let expected = cli(
            &db,
            &[
                "search",
                "--project",
                "demo",
                "--explain",
                "--mode",
                mode,
                "alpha beta",
            ],
        )?;
        let result = client
            .call_tool(
                CallToolRequestParams::new("memory_search").with_arguments(
                    json!({"project":"demo","query":"alpha beta","mode":mode,"explain":true})
                        .as_object()
                        .unwrap()
                        .clone(),
                ),
            )
            .await?;
        assert_ne!(result.is_error, Some(true), "{result:?}");
        assert_eq!(result.structured_content.unwrap(), expected);
    }
    let plain = cli(&db, &["search", "--project", "demo", "--json", "alpha"])?;
    assert!(plain["hits"][0].get("explanation").is_none());
    for budget in ["512", "4000"] {
        let expected = cli(
            &db,
            &[
                "context",
                "--project",
                "demo",
                "--max-tokens",
                budget,
                "--explain",
                "alpha beta",
            ],
        )?;
        let result = client.call_tool(CallToolRequestParams::new("memory_context").with_arguments(
            json!({"project":"demo","task":"alpha beta","max_tokens":budget.parse::<usize>()?,"explain":true}).as_object().unwrap().clone())).await?;
        assert_ne!(result.is_error, Some(true), "{result:?}");
        assert_eq!(result.structured_content.unwrap(), expected);
        let plain = cli(
            &db,
            &[
                "context",
                "--project",
                "demo",
                "--max-tokens",
                budget,
                "alpha beta",
            ],
        )?;
        assert_eq!(expected["context"], plain);
    }
    for (tool, args) in [
        (
            "memory_search",
            json!({"project":"demo","query":"alpha","explain":"yes"}),
        ),
        (
            "memory_search",
            json!({"project":"demo","query":"alpha","mode":"natural","literal":true,"explain":true}),
        ),
        (
            "memory_context",
            json!({"project":"demo","task":"alpha","max_tokens":511,"explain":true}),
        ),
        (
            "memory_context",
            json!({"project":"unauthorized","task":"alpha","explain":true}),
        ),
    ] {
        let result = client
            .call_tool(
                CallToolRequestParams::new(tool).with_arguments(args.as_object().unwrap().clone()),
            )
            .await?;
        assert_eq!(result.is_error, Some(true), "{result:?}");
    }
    let conflict = Command::new(env!("CARGO_BIN_EXE_agent-memory"))
        .arg("--database")
        .arg(&db)
        .args([
            "search",
            "--project",
            "demo",
            "--explain",
            "--paths-only",
            "alpha",
        ])
        .output()?;
    assert!(!conflict.status.success());
    client.cancel().await?;
    Ok(())
}
