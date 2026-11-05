use anyhow::Result;
use memory_sqlite::Store;
use rmcp::{
    model::{CallToolRequestParams, CallToolResult},
    transport::TokioChildProcess,
    Peer, RoleClient, ServiceExt,
};
use serde_json::{json, Value};
use std::{fs, process::Command};

async fn call(client: &Peer<RoleClient>, tool: &str, args: Value) -> Result<CallToolResult> {
    Ok(client
        .call_tool(
            CallToolRequestParams::new(tool.to_owned())
                .with_arguments(args.as_object().unwrap().clone()),
        )
        .await?)
}

#[tokio::test]
async fn cli_and_mcp_refresh_match_and_enforce_scope_and_input_limits() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let notes = temp.path().join("notes");
    fs::create_dir(&notes)?;
    let db = temp.path().join("db");
    let store = Store::open(&db)?;
    store.create_project("demo")?;
    store.add_source("demo", &notes)?;
    drop(store);
    let mut command = tokio::process::Command::new(env!("CARGO_BIN_EXE_agent-memory"));
    command
        .arg("--database")
        .arg(&db)
        .args(["mcp", "--project", "demo"]);
    let client = ().serve(TokioChildProcess::new(command)?).await?;
    // Both new tools must refresh Markdown created after server initialization.
    fs::write(notes.join("a.md"),"---\nmemory:\n  id: a\n  project: demo\n  kind: constraint\n  status: accepted\n  summary: Keep authentication local\n  relations:\n    - kind: contradicts\n      target: b\n---\n# Rule\nauthentication\n")?;
    fs::write(notes.join("b.md"),"---\nmemory:\n  id: b\n  project: demo\n  kind: decision\n  status: accepted\n  summary: Authentication elsewhere\n---\n# Rule\nauthentication\n")?;
    for (tool, args, cli_args) in [
        (
            "memory_context",
            json!({"project":"demo","task":"authentication","max_tokens":4000}),
            vec![
                "context",
                "--project",
                "demo",
                "--max-tokens",
                "4000",
                "authentication",
            ],
        ),
        (
            "memory_contradictions",
            json!({"project":"demo"}),
            vec!["contradictions", "--project", "demo"],
        ),
    ] {
        let response = call(&client, tool, args).await?;
        assert_ne!(response.is_error, Some(true), "{response:?}");
        let report = response.structured_content.unwrap();
        let output = Command::new(env!("CARGO_BIN_EXE_agent-memory"))
            .arg("--database")
            .arg(&db)
            .args(cli_args)
            .output()?;
        assert!(output.status.success(), "{output:?}");
        assert_eq!(serde_json::from_slice::<Value>(&output.stdout)?, report);
        if tool == "memory_context" {
            assert!(!report["entries"].as_array().unwrap().is_empty());
            assert_eq!(report["diagnostic_issue_count"], 1);
        } else {
            assert_eq!(report["issues"][0]["kind"], "active_contradiction");
        }
        let denied_args = if tool == "memory_context" {
            json!({"project":"not_allowed","task":"authentication"})
        } else {
            json!({"project":"not_allowed"})
        };
        let denied = call(&client, tool, denied_args).await?;
        assert_eq!(denied.is_error, Some(true));
    }
    for args in [
        json!({"project":"demo","task":""}),
        json!({"project":"demo","task":"a","max_tokens":511}),
        json!({"project":"demo","task":"a","max_tokens":32769}),
        json!({"project":"demo","task":"a","unexpected":true}),
    ] {
        assert_eq!(
            call(&client, "memory_context", args).await?.is_error,
            Some(true)
        );
    }
    let schema = client
        .list_all_tools()
        .await?
        .into_iter()
        .find(|t| t.name == "memory_context")
        .unwrap()
        .input_schema;
    assert_eq!(schema["properties"]["max_tokens"]["minimum"], 512);
    let default = call(
        &client,
        "memory_context",
        json!({"project":"demo","task":"authentication"}),
    )
    .await?
    .structured_content
    .unwrap();
    assert_eq!(default["max_tokens"], 4000);
    client.cancel().await?;
    Ok(())
}
