use anyhow::Result;
use memory_sqlite::Store;
use rmcp::{model::CallToolRequestParams, transport::TokioChildProcess, ServiceExt};
use serde_json::json;

#[tokio::test]
async fn links_backlinks_refresh_directions_and_project_permissions() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let notes = temp.path().join("notes");
    std::fs::create_dir(&notes)?;
    std::fs::write(notes.join("a.md"), "# A\n[Target](b.md) and [[b|Alias]]\n")?;
    std::fs::write(notes.join("b.md"), "---\nid: target-id\n---\n# B\n")?;
    let db = temp.path().join("index.sqlite");
    let source_id = {
        let store = Store::open(&db)?;
        store.create_project("demo")?;
        store.create_project("other")?;
        store.register_source("demo", &notes, false)?
    };
    let mut command = tokio::process::Command::new(env!("CARGO_BIN_EXE_agent-memory"));
    command
        .arg("--database")
        .arg(&db)
        .args(["mcp", "--project", "demo", "--project", "other"]);
    let client = ().serve(TokioChildProcess::new(command)?).await?;
    assert!(client
        .list_all_tools()
        .await?
        .iter()
        .any(|tool| tool.name == "memory_links"));
    for (path, direction, incoming, outgoing) in [
        ("a.md", "both", 0, 2),
        ("b.md", "incoming", 2, 0),
        ("b.md", "outgoing", 0, 0),
    ] {
        let result = client.call_tool(CallToolRequestParams::new("memory_links").with_arguments(
            json!({"project":"demo","source_id":source_id,"path":path,"direction":direction}).as_object().unwrap().clone()
        )).await?;
        assert_ne!(result.is_error, Some(true));
        let value = result.structured_content.unwrap();
        assert_eq!(value["incoming"].as_array().unwrap().len(), incoming);
        assert_eq!(value["outgoing"].as_array().unwrap().len(), outgoing);
    }
    std::fs::rename(notes.join("b.md"), notes.join("renamed.md"))?;
    let result = client
        .call_tool(
            CallToolRequestParams::new("memory_links").with_arguments(
                json!({"project":"demo","source_id":source_id,"path":"a.md"})
                    .as_object()
                    .unwrap()
                    .clone(),
            ),
        )
        .await?;
    assert_ne!(result.is_error, Some(true));
    let value = result.structured_content.unwrap();
    assert_eq!(
        value["outgoing"][0]["target_document"]["path"],
        "renamed.md"
    );
    assert_eq!(
        value["outgoing"][1]["target_document"]["path"],
        "renamed.md"
    );
    for arguments in [
        json!({"project":"other","source_id":source_id,"path":"a.md"}),
        json!({"project":"demo","source_id":source_id,"path":"../a.md"}),
        json!({"project":"demo","source_id":source_id,"path":"missing.md"}),
    ] {
        let result = client
            .call_tool(
                CallToolRequestParams::new("memory_links")
                    .with_arguments(arguments.as_object().unwrap().clone()),
            )
            .await?;
        assert_eq!(result.is_error, Some(true));
    }
    std::fs::write(notes.join("a.md"), "# A\nNo links now\n")?;
    let result = client
        .call_tool(
            CallToolRequestParams::new("memory_links").with_arguments(
                json!({"project":"demo","source_id":source_id,"path":"renamed.md"})
                    .as_object()
                    .unwrap()
                    .clone(),
            ),
        )
        .await?;
    assert_eq!(result.structured_content.unwrap()["incoming"], json!([]));
    let connection = rusqlite::Connection::open(&db)?;
    connection.execute("UPDATE project_sources SET can_read=0", [])?;
    let result = client
        .call_tool(
            CallToolRequestParams::new("memory_links").with_arguments(
                json!({"project":"demo","source_id":source_id,"path":"a.md"})
                    .as_object()
                    .unwrap()
                    .clone(),
            ),
        )
        .await?;
    assert_eq!(result.is_error, Some(true));
    client.cancel().await?;
    Ok(())
}
