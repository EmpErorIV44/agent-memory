use anyhow::{Context, Result};
use memory_sqlite::Store;
use rmcp::{
    model::{CallToolRequestParams, CallToolResult},
    transport::TokioChildProcess,
    Peer, RoleClient, ServiceExt,
};
use serde_json::{json, Value};
use std::fs;

async fn call(client: &Peer<RoleClient>, name: &str, args: Value) -> Result<CallToolResult> {
    Ok(client
        .call_tool(
            CallToolRequestParams::new(name.to_owned())
                .with_arguments(args.as_object().context("object arguments")?.clone()),
        )
        .await?)
}

#[tokio::test]
async fn date_stamping_is_explicit_hash_guarded_and_preserves_legacy_bytes() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let notes = temp.path().join("notes");
    fs::create_dir(&notes)?;
    let db = temp.path().join("memory.sqlite");
    let store = Store::open(&db)?;
    store.create_project("demo")?;
    store.create_project("other")?;
    let source = store.register_source("demo", &notes, true)?;
    let mut command = tokio::process::Command::new(env!("CARGO_BIN_EXE_agent-memory"));
    command.arg("--database").arg(&db).args([
        "mcp",
        "--project",
        "demo",
        "--project",
        "other",
        "--allow-write",
    ]);
    let client = ().serve(TokioChildProcess::new(command)?).await?;
    let legacy = "---\nid: legacy\ncustom: preserve-me\n---\n# Legacy\n";
    let saved = call(
        &client,
        "memory_write",
        json!({"project":"demo","source_id":source,"path":"legacy.md","content":legacy}),
    )
    .await?;
    assert_ne!(saved.is_error, Some(true));
    assert_eq!(fs::read_to_string(notes.join("legacy.md"))?, legacy);
    let old_hash = saved.structured_content.context("legacy write")?["content_hash"].clone();
    let auto = call(&client,"memory_write",json!({"project":"demo","source_id":source,"path":"auto.md","content":"# New note\n","stamp_metadata":true})).await?;
    assert_ne!(auto.is_error, Some(true));
    let auto = serde_json::to_value(memory_core::parse_metadata(&fs::read_to_string(
        notes.join("auto.md"),
    )?)?)?;
    assert!(auto["created"]
        .as_str()
        .is_some_and(|date| date.len() == 10));
    assert_eq!(auto["created"], auto["updated"]);
    let full = call(
        &client,
        "memory_get",
        json!({"project":"demo","source_id":source,"path":"legacy.md","content_hash":old_hash}),
    )
    .await?;
    assert_eq!(
        full.structured_content.context("legacy full")?["document"]["content"],
        legacy
    );
    // Opt-in creates dates without discarding other metadata or content.
    let authored = "---\nid: stamped\ncreated: '2000-02-29'\ntags: [rust, память]\ncustom: preserve-me\n---\n# Stamped\n";
    let stamped = call(&client,"memory_write",json!({"project":"demo","source_id":source,"path":"stamped.md","content":authored,"stamp_metadata":true})).await?;
    assert_ne!(stamped.is_error, Some(true), "{stamped:?}");
    let first_hash = stamped.structured_content.context("stamped write")?["content_hash"].clone();
    let current = fs::read_to_string(notes.join("stamped.md"))?;
    let metadata = serde_json::to_value(memory_core::parse_metadata(&current)?)?;
    assert_eq!(metadata["created"], "2000-02-29");
    assert_eq!(metadata["tags"], json!(["rust", "память"]));
    assert!(metadata["updated"]
        .as_str()
        .is_some_and(|date| date.len() == 10));
    assert!(current.contains("preserve-me") && current.contains("# Stamped\n"));
    let noop = call(&client,"memory_write",json!({"project":"demo","source_id":source,"path":"stamped.md","content":current,"stamp_metadata":true,"expected_hash":first_hash})).await?;
    assert_ne!(noop.is_error, Some(true));
    assert_eq!(
        noop.structured_content.context("noop")?["content_hash"],
        first_hash
    );
    assert_eq!(fs::read_to_string(notes.join("stamped.md"))?, current);
    let edited = current.replace("# Stamped\n", "# Stamped\nChanged body.\n");
    let update = call(&client,"memory_write",json!({"project":"demo","source_id":source,"path":"stamped.md","content":edited,"stamp_metadata":true,"expected_hash":first_hash})).await?;
    assert_ne!(update.is_error, Some(true));
    let after = fs::read_to_string(notes.join("stamped.md"))?;
    assert_eq!(
        serde_json::to_value(memory_core::parse_metadata(&after)?)?["created"],
        "2000-02-29"
    );
    // The old hash cannot be reused, even with opt-in stamping.
    let stale = call(&client,"memory_write",json!({"project":"demo","source_id":source,"path":"stamped.md","content":current,"stamp_metadata":true,"expected_hash":first_hash})).await?;
    assert_eq!(stale.is_error, Some(true));
    let foreign = call(&client,"memory_write",json!({"project":"other","source_id":source,"path":"foreign.md","content":"# Foreign\n","stamp_metadata":true})).await?;
    assert_eq!(foreign.is_error, Some(true));
    assert!(!notes.join("foreign.md").exists());
    assert_eq!(fs::read_to_string(notes.join("stamped.md"))?, after);
    let changed_created = after.replace("2000-02-29", "2000-02-28");
    let forbidden_created = call(&client,"memory_write",json!({"project":"demo","source_id":source,"path":"stamped.md","content":changed_created,"stamp_metadata":true,"expected_hash":memory_core::content_hash(after.as_bytes())})).await?;
    assert_eq!(forbidden_created.is_error, Some(true));
    assert_eq!(fs::read_to_string(notes.join("stamped.md"))?, after);
    assert_eq!(fs::read_to_string(notes.join("legacy.md"))?, legacy);
    for (path, content) in [
        (
            "invalid-date.md",
            "---\ncreated: '1900-02-29'\n---\n# Invalid\n",
        ),
        (
            "invalid-order.md",
            "---\ncreated: '2024-03-01'\nupdated: '2024-02-29'\n---\n# Invalid\n",
        ),
        (
            "invalid-tags.md",
            "---\ntags: [rust, rust]\n---\n# Invalid\n",
        ),
    ] {
        let invalid = call(
            &client,
            "memory_write",
            json!({"project":"demo","source_id":source,"path":path,"content":content}),
        )
        .await?;
        assert_eq!(invalid.is_error, Some(true), "{path}");
        assert!(!notes.join(path).exists());
    }
    client.cancel().await?;
    Ok(())
}
