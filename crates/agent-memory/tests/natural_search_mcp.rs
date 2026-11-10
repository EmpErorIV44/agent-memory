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
async fn natural_mode_is_explicit_in_cli_and_mcp_and_preserves_fts_defaults() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let notes = temp.path().join("notes");
    fs::create_dir(&notes)?;
    fs::write(notes.join("one.md"), "# General\nalpha\n")?;
    fs::write(notes.join("two.md"), "# General\nbeta\n")?;
    let db = temp.path().join("index.sqlite");
    let mut store = Store::open(&db)?;
    store.create_project("demo")?;
    store.add_source("demo", &notes)?;
    store.index("demo")?;
    drop(store);
    let ordinary = cli(&db, &["--json", "--mode", "natural", "alpha beta"])?;
    assert!(ordinary.status.success(), "{ordinary:?}");
    let expected: Value = serde_json::from_slice(&ordinary.stdout)?;
    assert_eq!(expected["hits"].as_array().unwrap().len(), 2);
    let fts = cli(&db, &["--json", "alpha beta"])?;
    assert!(fts.status.success());
    assert_eq!(
        serde_json::from_slice::<Value>(&fts.stdout)?,
        json!({"hits":[]})
    );
    assert!(!cli(&db, &["--mode", "natural", "--literal", "alpha"])?
        .status
        .success());
    assert!(!cli(&db, &["--mode", "unknown", "alpha"])?.status.success());
    assert!(cli(&db, &["--mode", "fts", "--literal", "alpha"])?
        .status
        .success());
    let mut command = tokio::process::Command::new(env!("CARGO_BIN_EXE_agent-memory"));
    command
        .arg("--database")
        .arg(&db)
        .args(["mcp", "--project", "demo"]);
    let client = ().serve(TokioChildProcess::new(command)?).await?;
    let natural = call(
        &client,
        json!({"project":"demo","query":"alpha beta","mode":"natural"}),
    )
    .await?;
    assert_ne!(natural.is_error, Some(true), "{natural:?}");
    assert_eq!(natural.structured_content.unwrap(), expected);
    let fts = call(&client, json!({"project":"demo","query":"alpha beta"})).await?;
    assert_eq!(fts.structured_content.unwrap(), json!({"hits":[]}));
    for args in [
        json!({"project":"demo","query":"alpha","mode":"natural","literal":true}),
        json!({"project":"demo","query":"alpha","mode":"unknown"}),
    ] {
        let result = call(&client, args).await?;
        assert_eq!(result.is_error, Some(true), "{result:?}");
    }
    client.cancel().await?;
    Ok(())
}

#[test]
fn paths_only_preserves_ranked_paths_and_deduplicates_after_the_hit_limit() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let notes = temp.path().join("notes");
    fs::create_dir_all(notes.join("nested"))?;
    fs::write(
        notes.join("nested/multi word.md"),
        "---\nmemory:\n  id: multi\n  project: demo\n  kind: fact\n  status: accepted\n  summary: ordinary\n---\n# First\nalpha beta\n\n# Second\nalpha beta\n",
    )?;
    fs::write(notes.join("second.md"), "# General\nalpha beta gamma\n")?;
    fs::write(notes.join("third.md"), "# General\nbeta\n")?;
    let db = temp.path().join("index.sqlite");
    let mut store = Store::open(&db)?;
    store.create_project("demo")?;
    store.add_source("demo", &notes)?;
    store.index("demo")?;
    drop(store);

    let full = cli(&db, &["--json", "alpha"])?;
    assert!(full.status.success());
    let result: Value = serde_json::from_slice(&full.stdout)?;
    let hits = result["hits"].as_array().unwrap();
    assert!(
        hits.iter()
            .filter(|h| h["path"] == "nested/multi word.md")
            .count()
            > 1
    );

    for args in [
        vec!["alpha"],
        vec!["--limit", "1", "alpha"],
        vec!["--limit", "2", "alpha"],
        vec!["--mode", "natural", "alpha beta"],
        vec!["--literal", "alpha beta"],
        vec!["--kind", "fact", "--status", "accepted", "alpha"],
        vec!["unmatchedmarker"],
    ] {
        let mut json_args = vec!["--json"];
        json_args.extend(&args);
        let full = cli(&db, &json_args)?;
        assert!(full.status.success(), "{args:?}: {full:?}");
        let result: Value = serde_json::from_slice(&full.stdout)?;
        let mut expected = String::new();
        let mut seen = std::collections::HashSet::new();
        for hit in result["hits"].as_array().unwrap() {
            let path = hit["path"].as_str().unwrap();
            if seen.insert(path) {
                expected.push_str(path);
                expected.push('\n');
            }
        }
        let mut path_args = vec!["--paths-only"];
        path_args.extend(&args);
        let actual = cli(&db, &path_args)?;
        assert!(actual.status.success(), "{args:?}: {actual:?}");
        assert!(actual.stderr.is_empty(), "{args:?}: {actual:?}");
        assert_eq!(String::from_utf8(actual.stdout)?, expected, "{args:?}");
    }
    let conflicting = cli(&db, &["--paths-only", "--json", "alpha"])?;
    assert!(!conflicting.status.success());
    assert!(conflicting.stdout.is_empty());
    assert!(String::from_utf8(conflicting.stderr)?.contains("cannot be used"));
    Ok(())
}
