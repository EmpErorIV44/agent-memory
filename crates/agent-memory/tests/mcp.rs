use std::{path::Path, time::Duration};

use anyhow::{Context, Result};
use memory_sqlite::Store;
use rmcp::{
    model::{CallToolRequestParams, CallToolResult},
    transport::TokioChildProcess,
    Peer, RoleClient, ServiceExt,
};
use serde_json::{json, Value};

async fn call(client: &Peer<RoleClient>, name: &str, arguments: Value) -> Result<CallToolResult> {
    Ok(client
        .call_tool(
            CallToolRequestParams::new(name.to_owned())
                .with_arguments(arguments.as_object().unwrap().clone()),
        )
        .await?)
}

async fn scenario() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let db = temp.path().join("mcp.sqlite");
    let banking = temp.path().join("banking");
    let storefront = temp.path().join("storefront");
    std::fs::create_dir(&banking)?;
    std::fs::create_dir(&storefront)?;
    let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures");
    for (project, root) in [("banking", &banking), ("storefront", &storefront)] {
        for entry in std::fs::read_dir(fixtures.join(project))? {
            let entry = entry?;
            std::fs::copy(entry.path(), root.join(entry.file_name()))?;
        }
    }
    let foreign = {
        let mut store = Store::open(&db)?;
        for (project, root) in [("banking", &banking), ("storefront", &storefront)] {
            store.create_project(project)?;
            store.add_source(project, root)?;
            store.index(project)?;
        }
        store.search("storefront", "OAuth2")?.remove(0)
    };
    let mut command = tokio::process::Command::new(env!("CARGO_BIN_EXE_agent-memory"));
    command
        .arg("--database")
        .arg(&db)
        .args(["mcp", "--project", "banking"]);
    let client = ().serve(TokioChildProcess::new(command)?).await?;
    let tools = client.list_all_tools().await?;
    let mut names: Vec<_> = tools.iter().map(|t| t.name.as_ref()).collect();
    names.sort();
    assert_eq!(names, ["memory_get", "memory_search", "memory_sources"]);

    let result = call(
        &client,
        "memory_search",
        json!({"project": "banking", "query": "OAuth2"}),
    )
    .await?;
    assert_ne!(result.is_error, Some(true));
    let content = result
        .structured_content
        .context("structured search results")?;
    let hit = &content["hits"][0];
    assert_eq!(hit["path"], "authentication.md");
    assert!(hit["body"].as_str().unwrap().contains("bankauthmarker"));
    assert!(!content.to_string().contains("shopauthmarker"));
    assert!(hit["start_line"].as_u64().unwrap() >= 1);

    let get_args = json!({"project": "banking", "chunk_id": hit["chunk_id"], "content_hash": hit["content_hash"]});
    let result = call(&client, "memory_get", get_args.clone()).await?;
    assert_ne!(result.is_error, Some(true));
    assert_eq!(result.structured_content.unwrap()["chunk"], *hit);

    // An allowed project cannot retrieve a foreign chunk by its known ID/hash.
    let result = call(&client, "memory_get", json!({"project":"banking", "chunk_id":foreign.chunk_id, "content_hash":foreign.content_hash})).await?;
    assert_eq!(result.is_error, Some(true));
    assert!(!serde_json::to_string(&result)?.contains("shopauthmarker"));
    let result = call(
        &client,
        "memory_search",
        json!({"project":"storefront", "query":"OAuth2"}),
    )
    .await?;
    assert_eq!(result.is_error, Some(true));
    let result = call(&client, "memory_get", json!({"project":"storefront", "chunk_id":foreign.chunk_id, "content_hash":foreign.content_hash})).await?;
    assert_eq!(result.is_error, Some(true));
    let result = call(
        &client,
        "memory_search",
        json!({"project":"banking", "query":"\""}),
    )
    .await?;
    assert_eq!(result.is_error, Some(true));
    let result = call(
        &client,
        "memory_search",
        json!({"project":"banking", "query":"OAuth2", "scope":"global"}),
    )
    .await;
    assert!(result.is_err() || result.as_ref().unwrap().is_error == Some(true));

    // The running server refreshes the index, and stale get references fail.
    std::fs::write(
        banking.join("authentication.md"),
        "# Auth\nOAuth2 bankfreshmarker\n",
    )?;
    assert_eq!(
        call(&client, "memory_get", get_args).await?.is_error,
        Some(true)
    );
    let result = call(
        &client,
        "memory_search",
        json!({"project":"banking", "query":"bankfreshmarker"}),
    )
    .await?;
    assert_ne!(result.is_error, Some(true));
    assert_eq!(
        result.structured_content.unwrap()["hits"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    std::fs::remove_file(banking.join("authentication.md"))?;
    let result = call(
        &client,
        "memory_search",
        json!({"project":"banking", "query":"bankfreshmarker"}),
    )
    .await?;
    assert!(result.structured_content.unwrap()["hits"]
        .as_array()
        .unwrap()
        .is_empty());
    client.cancel().await?;
    Ok(())
}

#[tokio::test]
async fn stdio_search_get_scope_and_refresh() -> Result<()> {
    tokio::time::timeout(Duration::from_secs(30), scenario()).await?
}

async fn write_scenario() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let db = temp.path().join("mcp.sqlite");
    let writable = temp.path().join("writable");
    let readonly = temp.path().join("readonly");
    std::fs::create_dir(&writable)?;
    std::fs::create_dir(&readonly)?;
    let (source_id, readonly_id) = {
        let store = Store::open(&db)?;
        store.create_project("banking")?;
        store.create_project("storefront")?;
        (
            store.register_source("banking", &writable, true)?,
            store.register_source("banking", &readonly, false)?,
        )
    };
    let launch = |write: bool| {
        let mut command = tokio::process::Command::new(env!("CARGO_BIN_EXE_agent-memory"));
        command
            .arg("--database")
            .arg(&db)
            .args(["mcp", "--project", "banking"]);
        if write {
            command.args(["--allow-write", "--write-policy", "verified"]);
        }
        TokioChildProcess::new(command)
    };
    let client = ().serve(launch(false)?).await?;
    assert!(!client
        .list_all_tools()
        .await?
        .iter()
        .any(|t| t.name == "memory_write"));
    let result = call(&client, "memory_sources", json!({"project":"banking"})).await?;
    assert!(result.structured_content.unwrap()["sources"]
        .as_array()
        .unwrap()
        .iter()
        .all(|s| s["can_write"] == false));
    let args = json!({"project":"banking", "source_id":source_id, "path":"note.md", "content":"# Note\nwritefrommcp\n"});
    let result = call(&client, "memory_write", args.clone()).await;
    assert!(result.is_err() || result.unwrap().is_error == Some(true));
    assert!(!writable.join("note.md").exists());
    client.cancel().await?;

    let client = ().serve(launch(true)?).await?;
    assert!(client
        .list_all_tools()
        .await?
        .iter()
        .any(|t| t.name == "memory_write"));
    let result = call(&client, "memory_sources", json!({"project":"banking"})).await?;
    let sources = result.structured_content.unwrap();
    assert!(sources["sources"]
        .as_array()
        .unwrap()
        .iter()
        .any(|s| s["source_id"] == source_id && s["can_write"] == true));
    for invalid in [
        json!({"project":"storefront", "source_id":source_id, "path":"note.md", "content":"# Denied"}),
        json!({"project":"banking", "source_id":readonly_id, "path":"note.md", "content":"# Denied"}),
        json!({"project":"banking", "source_id":source_id, "path":"../escape.md", "content":"# Denied"}),
    ] {
        assert_eq!(
            call(&client, "memory_write", invalid).await?.is_error,
            Some(true)
        );
    }
    let result = call(&client, "memory_write", args.clone()).await?;
    assert_ne!(result.is_error, Some(true));
    assert_eq!(result.structured_content.unwrap()["indexed"], true);
    assert_eq!(
        call(&client, "memory_write", args).await?.is_error,
        Some(true)
    );

    let result = call(
        &client,
        "memory_search",
        json!({"project":"banking", "query":"writefrommcp"}),
    )
    .await?;
    let hit = result.structured_content.unwrap()["hits"][0].clone();
    let result = call(&client, "memory_get", json!({"project":"banking", "chunk_id":hit["chunk_id"], "content_hash":hit["content_hash"], "include_document":true})).await?;
    let document = result.structured_content.unwrap()["document"].clone();
    assert_eq!(document["content"], "# Note\nwritefrommcp\n");
    let args = json!({"project":"banking", "source_id":source_id, "path":"note.md", "content":"# Note\nupdatedfrommcp\n", "expected_hash":document["content_hash"]});
    let result = call(&client, "memory_write", args.clone()).await?;
    assert_ne!(result.is_error, Some(true));
    assert_eq!(
        call(&client, "memory_write", args).await?.is_error,
        Some(true)
    );
    let result = call(
        &client,
        "memory_search",
        json!({"project":"banking", "query":"updatedfrommcp"}),
    )
    .await?;
    assert_eq!(
        result.structured_content.unwrap()["hits"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        std::fs::read_to_string(writable.join("note.md"))?,
        "# Note\nupdatedfrommcp\n"
    );
    assert!(!readonly.join("note.md").exists());
    assert!(!temp.path().join("escape.md").exists());
    client.cancel().await?;
    Ok(())
}

#[tokio::test]
async fn stdio_write_permissions_create_update_and_conflicts() -> Result<()> {
    tokio::time::timeout(Duration::from_secs(30), write_scenario()).await?
}
