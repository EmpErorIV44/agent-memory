use anyhow::Result;
use memory_sqlite::Store;
use rmcp::{
    model::{CallToolRequestParams, CallToolResult},
    transport::TokioChildProcess,
    Peer, RoleClient, ServiceExt,
};
use serde_json::{json, Value};
use std::{
    fs,
    path::Path,
    process::{Command, Output},
};

fn cli(db: &Path, args: &[&str]) -> Result<Output> {
    Ok(Command::new(env!("CARGO_BIN_EXE_agent-memory"))
        .arg("--database")
        .arg(db)
        .args(["search", "--project", "demo"])
        .args(args)
        .output()?)
}

async fn call(client: &Peer<RoleClient>, args: Value) -> Result<CallToolResult> {
    Ok(client
        .call_tool(
            CallToolRequestParams::new("memory_search")
                .with_arguments(args.as_object().unwrap().clone()),
        )
        .await?)
}

#[tokio::test]
async fn cli_and_stdio_search_options_preserve_defaults_and_literal_semantics() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let notes = temp.path().join("notes");
    fs::create_dir(&notes)?;
    let body = "# Notes\nCo-Authored-By\nalpha OR beta\nsay \"hello\"\napps/foo.md\n";
    for name in ["a.md", "b.md", "c.md"] {
        fs::write(notes.join(name), body)?;
    }
    // This matches a raw OR expression but must not match the literal phrase.
    fs::write(notes.join("other.md"), "# Other\nalpha alone\n")?;
    let db = temp.path().join("index.sqlite");
    let mut store = Store::open(&db)?;
    store.create_project("demo")?;
    store.add_source("demo", &notes)?;
    store.index("demo")?;
    drop(store);

    let default = cli(&db, &["alpha"])?;
    assert!(default.status.success());
    assert!(String::from_utf8(default.stdout)?.contains("a.md:"));
    let output = cli(&db, &["--json", "alpha"])?;
    assert!(output.status.success(), "{:?}", output);
    let all: Value = serde_json::from_slice(&output.stdout)?;
    assert_eq!(all["hits"].as_array().unwrap().len(), 4);
    let output = cli(&db, &["--json", "--limit", "2", "alpha"])?;
    assert!(output.status.success());
    let limited: Value = serde_json::from_slice(&output.stdout)?;
    assert_eq!(limited["hits"], json!(all["hits"].as_array().unwrap()[..2]));
    let first = &limited["hits"][0];
    assert_eq!(first["content_hash"].as_str().unwrap().len(), 64);
    assert!(first["start_line"].as_u64().unwrap() > 0);
    assert!(first["chunk_id"].is_i64());
    for limit in ["0", "51", "-1"] {
        assert!(!cli(&db, &["--limit", limit, "alpha"])?.status.success());
    }
    for query in [
        "Co-Authored-By",
        "alpha OR beta",
        "say \"hello\"",
        "apps/foo.md",
    ] {
        let output = cli(&db, &["--json", "--literal", query])?;
        assert!(output.status.success(), "{:?}", output);
        let result: Value = serde_json::from_slice(&output.stdout)?;
        assert_eq!(result["hits"].as_array().unwrap().len(), 3, "{query}");
    }
    let no_match = cli(&db, &["--json", "unmatchedmarker"])?;
    assert!(no_match.status.success());
    assert_eq!(
        serde_json::from_slice::<Value>(&no_match.stdout)?,
        json!({"hits": []})
    );
    let invalid = cli(&db, &["--json", "Co-Authored-By"])?;
    assert!(!invalid.status.success());
    assert!(invalid.stdout.is_empty());
    assert!(String::from_utf8(invalid.stderr)?.contains("literal"));

    let mut command = tokio::process::Command::new(env!("CARGO_BIN_EXE_agent-memory"));
    command
        .arg("--database")
        .arg(&db)
        .args(["mcp", "--project", "demo"]);
    let client = ().serve(TokioChildProcess::new(command)?).await?;
    let tools = client.list_tools(Default::default()).await?;
    let schema = &tools
        .tools
        .iter()
        .find(|tool| tool.name == "memory_search")
        .unwrap()
        .input_schema;
    assert_eq!(schema["properties"]["limit"]["minimum"], 1);
    assert_eq!(schema["properties"]["limit"]["maximum"], 50);
    let default = call(&client, json!({"project":"demo","query":"alpha"})).await?;
    assert_ne!(default.is_error, Some(true));
    assert_eq!(default.structured_content.unwrap(), all);
    let output = call(&client, json!({"project":"demo","query":"alpha","limit":2})).await?;
    assert_ne!(output.is_error, Some(true));
    assert_eq!(output.structured_content.unwrap(), limited);
    for query in [
        "Co-Authored-By",
        "alpha OR beta",
        "say \"hello\"",
        "apps/foo.md",
    ] {
        let output = call(
            &client,
            json!({"project":"demo","query":query,"literal":true}),
        )
        .await?;
        assert_ne!(output.is_error, Some(true), "{output:?}");
        assert_eq!(
            output.structured_content.unwrap()["hits"]
                .as_array()
                .unwrap()
                .len(),
            3,
            "{query}"
        );
    }
    for limit in [json!(0), json!(51), json!(-1)] {
        let output = call(
            &client,
            json!({"project":"demo","query":"alpha","limit":limit}),
        )
        .await?;
        assert_eq!(output.is_error, Some(true), "{output:?}");
    }
    let invalid = call(&client, json!({"project":"demo","query":"Co-Authored-By"})).await?;
    assert_eq!(invalid.is_error, Some(true));
    assert!(serde_json::to_string(&invalid)?.contains("literal"));
    client.cancel().await?;
    Ok(())
}
