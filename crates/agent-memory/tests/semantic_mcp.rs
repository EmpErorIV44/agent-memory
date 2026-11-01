use anyhow::{Context, Result};
use memory_sqlite::Store;
use rmcp::{
    model::{CallToolRequestParams, CallToolResult},
    transport::TokioChildProcess,
    Peer, RoleClient, ServiceExt,
};
use serde_json::{json, Value};
use std::fs;

async fn call(client: &Peer<RoleClient>, args: Value) -> Result<CallToolResult> {
    Ok(client
        .call_tool(
            CallToolRequestParams::new("memory_search")
                .with_arguments(args.as_object().context("object arguments")?.clone()),
        )
        .await?)
}

#[tokio::test]
async fn default_search_retains_fts_constraints_without_any_embedding_provider() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let notes = temp.path().join("notes");
    let private = temp.path().join("private");
    fs::create_dir(&notes)?;
    fs::create_dir(&private)?;
    fs::write(notes.join("allowed.md"),"---\nmemory:\n  id: allowed\n  project: demo\n  kind: decision\n  status: accepted\n  summary: Authentication rules\n---\n# OAuth2\nUse access token credentials. Заметками управляют агенты.\n")?;
    fs::write(
        notes.join("excluded.md"),
        "# Other\nUse access extra token credentials forbidden.\n",
    )?;
    fs::write(notes.join("shared.md"),"---\nmemory:\n  id: foreign\n  project: other\n  kind: fact\n  status: accepted\n  summary: foreignsummarysentinel\n---\n# Shared\nA readable shared document.\n")?;
    fs::write(private.join("secret.md"), "# Secret\nprivatesourcemarker\n")?;
    let db = temp.path().join("memory.sqlite");
    let store = Store::open(&db)?;
    store.create_project("demo")?;
    store.create_project("other")?;
    store.register_source("demo", &notes, false)?;
    store.register_source("other", &notes, false)?;
    store.register_source("other", &private, false)?;
    let mut command = tokio::process::Command::new(env!("CARGO_BIN_EXE_agent-memory"));
    command
        .arg("--database")
        .arg(&db)
        .args(["mcp", "--project", "demo"]);
    let client = ().serve(TokioChildProcess::new(command)?).await?;
    for query in [
        "credentials NOT forbidden",
        "\"access token\"",
        "heading:OAuth2",
        "заметка",
    ] {
        let result = call(&client, json!({"project":"demo","query":query})).await?;
        assert_ne!(result.is_error, Some(true), "{query}: {result:?}");
        let content = result.structured_content.context("search results")?;
        let hits = content["hits"].as_array().context("hits")?;
        assert_eq!(hits.len(), 1, "{query}: {content}");
        assert_eq!(hits[0]["path"], "allowed.md");
        assert!(hits[0]["content_hash"]
            .as_str()
            .is_some_and(|h| h.len() == 64));
    }
    for query in ["foreignsummarysentinel", "privatesourcemarker"] {
        let result = call(&client, json!({"project":"demo","query":query})).await?;
        assert!(
            result.structured_content.context("isolated search")?["hits"]
                .as_array()
                .unwrap()
                .is_empty()
        );
    }
    let filtered = call(
        &client,
        json!({"project":"demo","query":"credentials","kind":"decision","status":"accepted"}),
    )
    .await?;
    assert_eq!(
        filtered.structured_content.unwrap()["hits"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    let excluded = call(
        &client,
        json!({"project":"demo","query":"credentials","status":"deprecated"}),
    )
    .await?;
    assert!(excluded.structured_content.unwrap()["hits"]
        .as_array()
        .unwrap()
        .is_empty());
    let malformed = call(&client, json!({"project":"demo","query":"\""})).await?;
    assert_eq!(malformed.is_error, Some(true));
    let denied = call(
        &client,
        json!({"project":"other","query":"privatesourcemarker"}),
    )
    .await?;
    assert_eq!(denied.is_error, Some(true));
    assert!(!notes.join(".agent-memory-history").exists());
    client.cancel().await?;
    Ok(())
}
