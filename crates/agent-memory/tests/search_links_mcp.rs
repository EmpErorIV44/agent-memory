use anyhow::Result;
use memory_sqlite::Store;
use rmcp::{
    model::{CallToolRequestParams, CallToolResult},
    transport::TokioChildProcess,
    Peer, RoleClient, ServiceExt,
};
use serde_json::{json, Value};
use std::{fs, path::Path};

async fn call(client: &Peer<RoleClient>, name: &str, args: Value) -> Result<CallToolResult> {
    Ok(client
        .call_tool(
            CallToolRequestParams::new(name.to_owned())
                .with_arguments(args.as_object().unwrap().clone()),
        )
        .await?)
}

fn launch(db: &Path, write: bool) -> Result<TokioChildProcess> {
    let mut command = tokio::process::Command::new(env!("CARGO_BIN_EXE_agent-memory"));
    command
        .arg("--database")
        .arg(db)
        .args(["mcp", "--project", "demo", "--project", "denied"]);
    if write {
        command.arg("--allow-write");
    }
    Ok(TokioChildProcess::new(command)?)
}

#[tokio::test]
async fn russian_filters_diagnostics_and_guarded_rename_over_stdio() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let root = temp.path().join("notes");
    fs::create_dir(&root)?;
    let db = temp.path().join("memory.sqlite");
    let source = {
        let store = Store::open(&db)?;
        store.create_project("demo")?;
        store.create_project("denied")?;
        store.register_source("demo", &root, true)?
    };
    fs::write(root.join("decision.md"), "---\nid: decision-doc\nmemory:\n  id: decision\n  project: demo\n  kind: decision\n  status: accepted\n  summary: Хранение заметок\n---\n# Заметки\nСвязи между заметками.\n")?;
    fs::write(
        root.join("origin.md"),
        "# Origin\n[decision](decision.md#heading)\n[missing](absent.md)\n`[code](decision.md)`\n",
    )?;
    let client = ().serve(launch(&db, true)?).await?;
    let found = call(
        &client,
        "memory_search",
        json!({"project":"demo","query":"заметка","kind":"decision","status":"accepted"}),
    )
    .await?;
    assert_ne!(found.is_error, Some(true), "{found:?}");
    assert_eq!(
        found.structured_content.unwrap()["hits"][0]["path"],
        "decision.md"
    );
    let excluded = call(
        &client,
        "memory_search",
        json!({"project":"demo","query":"заметка","status":"superseded"}),
    )
    .await?;
    assert!(excluded.structured_content.unwrap()["hits"]
        .as_array()
        .unwrap()
        .is_empty());
    let invalid = call(
        &client,
        "memory_search",
        json!({"project":"demo","query":"заметка","kind":"invalid"}),
    )
    .await?;
    assert_eq!(invalid.is_error, Some(true));
    let diagnostics = call(
        &client,
        "memory_link_diagnostics",
        json!({"project":"demo","source_id":source}),
    )
    .await?;
    let issues = diagnostics.structured_content.unwrap();
    assert!(issues
        .as_array()
        .unwrap()
        .iter()
        .any(|issue| issue["target"] == "absent.md" && issue["status"] == "missing"));
    let preview = call(
        &client,
        "memory_rename",
        json!({"project":"demo","source_id":source,"from":"decision.md","to":"renamed.md"}),
    )
    .await?;
    assert_ne!(preview.is_error, Some(true), "{preview:?}");
    let hash = preview.structured_content.unwrap()["plan_hash"].clone();
    let denied = call(
        &client,
        "memory_rename",
        json!({"project":"denied","source_id":source,"from":"decision.md","to":"renamed.md"}),
    )
    .await?;
    assert_eq!(denied.is_error, Some(true));
    let readonly = ().serve(launch(&db, false)?).await?;
    let refused = call(&readonly, "memory_rename", json!({"project":"demo","source_id":source,"from":"decision.md","to":"renamed.md","expected_plan_hash":hash})).await?;
    assert_eq!(refused.is_error, Some(true));
    readonly.cancel().await?;
    let renamed = call(&client, "memory_rename", json!({"project":"demo","source_id":source,"from":"decision.md","to":"renamed.md","expected_plan_hash":hash})).await?;
    assert_ne!(renamed.is_error, Some(true), "{renamed:?}");
    assert_eq!(renamed.structured_content.unwrap()["complete"], true);
    assert!(!root.join("decision.md").exists());
    assert!(root.join("renamed.md").exists());
    let text = fs::read_to_string(root.join("origin.md"))?;
    assert!(text.contains("(renamed.md#heading)"));
    assert!(text.contains("`[code](decision.md)`"));
    client.cancel().await?;
    Ok(())
}

#[test]
fn cli_search_supports_russian_forms_and_validated_filters() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let root = temp.path().join("notes");
    fs::create_dir(&root)?;
    fs::write(root.join("note.md"), "---\nmemory:\n  id: choice\n  project: demo\n  kind: decision\n  status: accepted\n  summary: Заметки\n---\n# Хранение заметок\n")?;
    let db = temp.path().join("memory.sqlite");
    let mut store = Store::open(&db)?;
    store.create_project("demo")?;
    store.add_source("demo", &root)?;
    store.index("demo")?;
    drop(store);
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_agent-memory"))
        .arg("--database")
        .arg(&db)
        .args([
            "search",
            "--project",
            "demo",
            "--kind",
            "decision",
            "--status",
            "accepted",
            "заметка",
        ])
        .output()?;
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("note.md"));
    Ok(())
}
