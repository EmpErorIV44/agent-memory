use anyhow::Result;
use memory_sqlite::{BatchEdit, Store};
use rmcp::{
    model::{CallToolRequestParams, CallToolResult},
    transport::TokioChildProcess,
    Peer, RoleClient, ServiceExt,
};
use serde_json::{json, Value};
use std::{fs, path::Path, process::Command};

async fn call(client: &Peer<RoleClient>, tool: &str, args: Value) -> Result<CallToolResult> {
    Ok(client
        .call_tool(
            CallToolRequestParams::new(tool.to_owned())
                .with_arguments(args.as_object().unwrap().clone()),
        )
        .await?)
}
async fn start(db: &Path, write: bool) -> Result<rmcp::service::RunningService<RoleClient, ()>> {
    let mut command = tokio::process::Command::new(env!("CARGO_BIN_EXE_agent-memory"));
    command
        .arg("--database")
        .arg(db)
        .args(["mcp", "--project", "demo"]);
    if write {
        command.arg("--allow-write");
    }
    Ok(().serve(TokioChildProcess::new(command)?).await?)
}
fn cli(db: &Path, args: &[&str]) -> Result<Value> {
    let output = Command::new(env!("CARGO_BIN_EXE_agent-memory"))
        .arg("--database")
        .arg(db)
        .args(args)
        .output()?;
    anyhow::ensure!(output.status.success(), "{output:?}");
    Ok(serde_json::from_slice(&output.stdout)?)
}

#[tokio::test]
async fn previews_match_cli_and_write_permission_gates_apply_and_resume() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let notes = temp.path().join("notes");
    fs::create_dir(&notes)?;
    fs::write(notes.join("old.md"), "# Old\n")?;
    let db = temp.path().join("db");
    let store = Store::open(&db)?;
    store.create_project("demo")?;
    let source = store.register_source("demo", &notes, true)?;
    drop(store);
    let edits = json!([{"path":"old.md","content":"# Retired\n","expected_hash":memory_core::content_hash(b"# Old\n")},{"path":"new.md","content":"# New\n"}]);
    let file = temp.path().join("edits.json");
    fs::write(&file, serde_json::to_vec(&edits)?)?;
    let source_string = source.to_string();
    let plan = cli(
        &db,
        &[
            "batch-write",
            "--project",
            "demo",
            "--source-id",
            &source_string,
            "--edits-file",
            file.to_str().unwrap(),
        ],
    )?;
    let read = start(&db, false).await?;
    let preview = call(
        &read,
        "memory_batch_write",
        json!({"project":"demo","source_id":source,"edits":edits}),
    )
    .await?;
    assert_eq!(preview.structured_content.as_ref().unwrap(), &plan);
    assert!(!notes.join("new.md").exists());
    let denied=call(&read,"memory_batch_write",json!({"project":"demo","source_id":source,"edits":edits,"expected_plan_hash":plan["plan_hash"]})).await?;
    assert_eq!(denied.is_error, Some(true));
    let tools = read.list_all_tools().await?;
    assert!(!tools.iter().any(|t| t.name == "memory_batch_write_resume"));
    let unknown = call(
        &read,
        "memory_batch_write",
        json!({"project":"foreign","source_id":source,"edits":edits}),
    )
    .await?;
    assert_eq!(unknown.is_error, Some(true));
    read.cancel().await?;
    let write = start(&db, true).await?;
    let result=call(&write,"memory_batch_write",json!({"project":"demo","source_id":source,"edits":edits,"expected_plan_hash":plan["plan_hash"]})).await?;
    assert_ne!(result.is_error, Some(true), "{result:?}");
    let result = result.structured_content.unwrap();
    assert_eq!(result["complete"], true);
    assert_eq!(fs::read_to_string(notes.join("new.md"))?, "# New\n");
    let operations = cli(
        &db,
        &[
            "batch-write-operations",
            "--project",
            "demo",
            "--source-id",
            &source_string,
        ],
    )?;
    assert_eq!(
        operations["operations"][0]["operation_id"],
        result["operation_id"]
    );
    let resume = cli(
        &db,
        &[
            "batch-write-resume",
            "--project",
            "demo",
            "--source-id",
            &source_string,
            "--operation-id",
            result["operation_id"].as_str().unwrap(),
        ],
    )?;
    assert_eq!(resume["complete"], true);
    write.cancel().await?;
    Ok(())
}

fn note(id: &str) -> String {
    format!("---\nid: {id}\nmemory:\n  id: {id}\n  project: demo\n  kind: decision\n  status: accepted\n  summary: {id}\n---\n# {id}\n")
}

#[tokio::test]
async fn new_mcp_process_can_recover_an_unindexable_interrupted_id_swap() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let notes = temp.path().join("notes");
    fs::create_dir(&notes)?;
    fs::write(notes.join("a.md"), note("a"))?;
    fs::write(notes.join("b.md"), note("b"))?;
    let db = temp.path().join("db");
    let mut store = Store::open(&db)?;
    store.create_project("demo")?;
    let source = store.register_source("demo", &notes, true)?;
    store.index("demo")?;
    let edits = vec![
        BatchEdit {
            path: "a.md".into(),
            content: note("b"),
            expected_hash: Some(memory_core::content_hash(note("a").as_bytes())),
        },
        BatchEdit {
            path: "b.md".into(),
            content: note("a"),
            expected_hash: Some(memory_core::content_hash(note("b").as_bytes())),
        },
    ];
    let plan = store.batch_preview("demo", source, edits.clone())?;
    let journal =
        json!({"operation_kind":"batch_write","version":1,"nonce":"interrupted-test","plan":plan});
    let id = memory_core::save_operation(&notes, &serde_json::to_vec(&journal)?)?;
    // Materialize the exact state a crashed process leaves after its first publication.
    memory_core::write_markdown(
        &notes,
        "a.md",
        &edits[0].content,
        memory_core::WriteMode::Update {
            expected_hash: edits[0].expected_hash.as_deref().unwrap(),
        },
    )?;
    assert!(store.index("demo").is_err());
    drop(store);
    let client = start(&db, true).await?;
    let operations = call(
        &client,
        "memory_batch_write_operations",
        json!({"project":"demo","source_id":source}),
    )
    .await?;
    assert_ne!(operations.is_error, Some(true), "{operations:?}");
    assert_eq!(
        operations.structured_content.unwrap()["operations"][0]["operation_id"],
        id
    );
    let result = call(
        &client,
        "memory_batch_write_resume",
        json!({"project":"demo","source_id":source,"operation_id":id}),
    )
    .await?;
    assert_ne!(result.is_error, Some(true), "{result:?}");
    assert_eq!(result.structured_content.unwrap()["complete"], true);
    assert_eq!(fs::read_to_string(notes.join("b.md"))?, note("a"));
    let list = call(&client, "memory_list", json!({"project":"demo"})).await?;
    assert_ne!(list.is_error, Some(true));
    client.cancel().await?;
    Ok(())
}
