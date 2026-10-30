use anyhow::Result;
use rmcp::{model::CallToolRequestParams, transport::TokioChildProcess, ServiceExt};
use serde_json::json;

#[tokio::test]
async fn status_needs_read_permission_but_not_write_or_sync_enabled() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let root = temp.path().join("notes");
    std::fs::create_dir(&root)?;
    std::fs::write(root.join("note.md"), "# Note\n")?;
    let db = temp.path().join("memory.sqlite");
    let source_id = {
        let store = memory_sqlite::Store::open(&db)?;
        store.create_project("demo")?;
        store.create_project("other")?;
        store.register_source("demo", &root, false)?
    };
    let mut command = tokio::process::Command::new(env!("CARGO_BIN_EXE_agent-memory"));
    command
        .arg("--database")
        .arg(&db)
        .args(["mcp", "--project", "demo", "--project", "other"]);
    let client = ().serve(TokioChildProcess::new(command)?).await?;
    for (project, error) in [("demo", false), ("other", true)] {
        let result = client
            .call_tool(
                CallToolRequestParams::new("memory_sync_status").with_arguments(
                    json!({"project":project,"source_id":source_id})
                        .as_object()
                        .unwrap()
                        .clone(),
                ),
            )
            .await?;
        assert_eq!(result.is_error == Some(true), error);
        if !error {
            let value = result.structured_content.unwrap();
            assert_eq!(value["source_id"], source_id);
            assert_eq!(value["sync"], serde_json::Value::Null);
        }
    }
    let connection = rusqlite::Connection::open(&db)?;
    connection.execute("UPDATE project_sources SET can_read=0", [])?;
    let result = client
        .call_tool(
            CallToolRequestParams::new("memory_sync_status").with_arguments(
                json!({"project":"demo","source_id":source_id})
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
