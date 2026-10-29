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
    assert_eq!(
        names,
        [
            "memory_get",
            "memory_get_record",
            "memory_history",
            "memory_list",
            "memory_revision_get",
            "memory_search",
            "memory_sources"
        ]
    );

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

async fn structured_scenario() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let database = temp.path().join("structured.sqlite");
    let source = temp.path().join("shared");
    std::fs::create_dir(&source)?;
    let source_id = {
        let store = Store::open(&database)?;
        store.create_project("banking")?;
        store.create_project("storefront")?;
        let id = store.register_source("banking", &source, true)?;
        store.register_source("storefront", &source, true)?;
        id
    };
    let mut command = tokio::process::Command::new(env!("CARGO_BIN_EXE_agent-memory"));
    command
        .arg("--database")
        .arg(&database)
        .args(["mcp", "--project", "banking", "--allow-write"]);
    let client = ().serve(TokioChildProcess::new(command)?).await?;

    let original = "---\nid: doc-original\nmemory:\n  id: original\n  project: banking\n  kind: implementation\n  status: implemented\n  summary: Original implementation\n---\n# Original\nstructuredmarker\n";
    let replacement = "---\nid: doc-replacement\nmemory:\n  id: replacement\n  project: banking\n  kind: decision\n  status: accepted\n  summary: Replacement decision\n  evidence:\n    - kind: document\n      reference: original.md\n  relations:\n    - kind: supersedes\n      target: original\n    - kind: relates_to\n      target: inaccessible\n---\n# Replacement\nstructuredmarker\n";
    for (path, content) in [("original.md", original), ("replacement.md", replacement)] {
        let result = call(
            &client,
            "memory_write",
            json!({"project":"banking", "source_id":source_id, "path":path, "content":content}),
        )
        .await?;
        assert_ne!(result.is_error, Some(true), "{result:?}");
        assert_eq!(result.structured_content.unwrap()["indexed"], true);
    }
    let result = call(&client, "memory_list", json!({"project":"banking"})).await?;
    assert_eq!(
        result.structured_content.unwrap()["memories"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    let result = call(
        &client,
        "memory_get_record",
        json!({"project":"banking", "memory_id":"replacement"}),
    )
    .await?;
    assert_ne!(result.is_error, Some(true));
    let record = result.structured_content.unwrap()["record"].clone();
    assert_eq!(record["memory"]["id"], "replacement");
    assert_eq!(record["memory"]["status"], "accepted");
    assert_eq!(record["memory"]["evidence"][0]["reference"], "original.md");
    assert_eq!(record["memory"]["relations"].as_array().unwrap().len(), 1);
    assert_eq!(record["memory"]["relations"][0]["target"], "original");
    assert_eq!(record["path"], "replacement.md");

    // Stable document identity survives a filesystem rename and is returned by search.
    let result = call(
        &client,
        "memory_search",
        json!({"project":"banking", "query":"structuredmarker"}),
    )
    .await?;
    let hits = result.structured_content.unwrap()["hits"]
        .as_array()
        .unwrap()
        .clone();
    let before = hits
        .iter()
        .find(|hit| hit["path"] == "replacement.md")
        .unwrap();
    std::fs::rename(source.join("replacement.md"), source.join("renamed.md"))?;
    let result = call(
        &client,
        "memory_get_record",
        json!({"project":"banking", "memory_id":"replacement"}),
    )
    .await?;
    let after = result.structured_content.unwrap()["record"].clone();
    assert_eq!(after["path"], "renamed.md");
    assert_eq!(after["document_id"], before["document_id"]);
    let result = call(
        &client,
        "memory_search",
        json!({"project":"banking", "query":"structuredmarker"}),
    )
    .await?;
    assert!(result.structured_content.unwrap()["hits"]
        .as_array()
        .unwrap()
        .iter()
        .any(|hit| hit["stable_id"] == "doc-replacement" && hit["path"] == "renamed.md"));

    for arguments in [
        json!({"project":"storefront", "memory_id":"replacement"}),
        json!({"project":"banking", "memory_id":"missing"}),
    ] {
        assert_eq!(
            call(&client, "memory_get_record", arguments)
                .await?
                .is_error,
            Some(true)
        );
    }
    assert_eq!(
        call(&client, "memory_list", json!({"project":"storefront"}))
            .await?
            .is_error,
        Some(true)
    );
    // Even a source bound to both projects cannot be used to write a foreign Memory.
    let foreign = original.replace("project: banking", "project: storefront");
    let result = call(
        &client,
        "memory_write",
        json!({"project":"banking", "source_id":source_id, "path":"foreign.md", "content":foreign}),
    )
    .await?;
    assert_eq!(result.is_error, Some(true));
    assert!(!source.join("foreign.md").exists());
    client.cancel().await?;
    Ok(())
}

#[tokio::test]
async fn structured_memories_stable_ids_and_authorization_over_stdio() -> Result<()> {
    tokio::time::timeout(Duration::from_secs(30), structured_scenario()).await?
}

async fn revisions_scenario() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let database = temp.path().join("revisions.sqlite");
    let source = temp.path().join("notes");
    std::fs::create_dir(&source)?;
    let source_id = {
        let store = Store::open(&database)?;
        store.create_project("demo")?;
        store.create_project("other")?;
        store.register_source("demo", &source, true)?
    };
    let launch = |allow_write: bool| {
        let mut command = tokio::process::Command::new(env!("CARGO_BIN_EXE_agent-memory"));
        command.arg("--database").arg(&database).args(["mcp", "--project", "demo", "--project", "other"]);
        if allow_write { command.arg("--allow-write"); }
        TokioChildProcess::new(command)
    };
    let client = ().serve(launch(true)?).await?;
    let first = "---\nid: durable-note\n---\n# Note\nfirstrevisionmarker\n";
    let second = "---\nid: durable-note\n---\n# Note\nsecondrevisionmarker\n";
    let result = call(&client, "memory_write", json!({"project":"demo", "source_id":source_id,"path":"note.md","content":first})).await?;
    assert_ne!(result.is_error, Some(true));
    let first_hash = result.structured_content.unwrap()["content_hash"].clone();
    let result = call(&client, "memory_write", json!({"project":"demo", "source_id":source_id,"path":"note.md","content":second,"expected_hash":first_hash})).await?;
    assert_ne!(result.is_error, Some(true));
    let second_hash = result.structured_content.unwrap()["content_hash"].clone();
    let history = call(&client, "memory_history", json!({"project":"demo","source_id":source_id,"path":"note.md"})).await?;
    let revisions = history.structured_content.unwrap()["revisions"].as_array().unwrap().clone();
    assert_eq!(revisions.len(), 2);
    assert_eq!(revisions[0]["content_hash"], first_hash);
    assert_eq!(revisions[1]["previous_hash"], first_hash);
    let id = revisions[0]["id"].clone();
    let revision = call(&client, "memory_revision_get", json!({"project":"demo","source_id":source_id,"path":"note.md","revision_id":id})).await?;
    assert_eq!(revision.structured_content.unwrap()["revision"]["content"], first);
    for (name, arguments) in [
        ("memory_history", json!({"project":"other","source_id":source_id,"path":"note.md"})),
        ("memory_revision_get", json!({"project":"other","source_id":source_id,"path":"note.md","revision_id":id})),
        ("memory_revision_get", json!({"project":"demo","source_id":source_id,"path":"../escape.md","revision_id":id})),
        ("memory_restore", json!({"project":"demo","source_id":source_id,"path":"note.md","revision_id":id,"expected_hash":first_hash})),
    ] {
        assert_eq!(call(&client, name, arguments).await?.is_error, Some(true));
    }
    assert_eq!(std::fs::read_to_string(source.join("note.md"))?, second);
    let restored = call(&client, "memory_restore", json!({"project":"demo","source_id":source_id,"path":"note.md","revision_id":id,"expected_hash":second_hash})).await?;
    assert_ne!(restored.is_error, Some(true));
    assert_eq!(restored.structured_content.unwrap()["indexed"], true);
    assert_eq!(std::fs::read_to_string(source.join("note.md"))?, first);
    let history = call(&client, "memory_history", json!({"project":"demo","source_id":source_id,"path":"note.md"})).await?;
    let revisions = history.structured_content.unwrap()["revisions"].as_array().unwrap().clone();
    assert_eq!(revisions.len(), 3);
    assert_eq!(revisions[2]["content_hash"], first_hash);
    assert_eq!(revisions[2]["previous_hash"], second_hash);
    assert_ne!(revisions[2]["id"], id);
    client.cancel().await?;

    let client = ().serve(launch(false)?).await?;
    let tools = client.list_all_tools().await?;
    assert!(tools.iter().any(|tool| tool.name == "memory_history"));
    assert!(tools.iter().any(|tool| tool.name == "memory_revision_get"));
    assert!(!tools.iter().any(|tool| tool.name == "memory_restore"));
    let history = call(&client, "memory_history", json!({"project":"demo","source_id":source_id,"path":"note.md"})).await?;
    assert_eq!(history.structured_content.unwrap()["revisions"].as_array().unwrap().len(), 3);
    let denied = call(&client, "memory_restore", json!({"project":"demo","source_id":source_id,"path":"note.md","revision_id":id,"expected_hash":first_hash})).await;
    assert!(denied.is_err() || denied.unwrap().is_error == Some(true));
    client.cancel().await?;
    Ok(())
}

#[tokio::test]
async fn durable_history_read_restore_and_permissions_over_stdio() -> Result<()> {
    tokio::time::timeout(Duration::from_secs(30), revisions_scenario()).await?
}
