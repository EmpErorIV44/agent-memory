use anyhow::Result;
use memory_sqlite::Store;
use rmcp::{
    model::{CallToolRequestParams, CallToolResult},
    transport::TokioChildProcess,
    Peer, RoleClient, ServiceExt,
};
use serde_json::{json, Value};
use std::{fs, path::Path, process::Command};

async fn call(client: &Peer<RoleClient>, name: &str, args: Value) -> Result<CallToolResult> {
    Ok(client
        .call_tool(
            CallToolRequestParams::new(name.to_owned())
                .with_arguments(args.as_object().unwrap().clone()),
        )
        .await?)
}
async fn start(
    db: &Path,
    format: Option<&str>,
) -> Result<rmcp::service::RunningService<RoleClient, ()>> {
    let mut c = tokio::process::Command::new(env!("CARGO_BIN_EXE_agent-memory"));
    c.arg("--database")
        .arg(db)
        .args(["mcp", "--project", "demo"]);
    if let Some(f) = format {
        c.args(["--response-format", f]);
    }
    Ok(().serve(TokioChildProcess::new(c)?).await?)
}
fn text_json(result: &CallToolResult) -> Value {
    let block = serde_json::to_value(&result.content[0]).unwrap();
    serde_json::from_str(block["text"].as_str().unwrap()).unwrap()
}

#[tokio::test]
async fn response_modes_preserve_payload_with_one_copy_by_default_and_text_errors() -> Result<()> {
    let t = tempfile::tempdir()?;
    let db = t.path().join("db");
    let store = Store::open(&db)?;
    store.create_project("demo")?;
    drop(store);
    let mut sizes = Vec::new();
    for format in [None, Some("text"), Some("both")] {
        let c = start(&db, format).await?;
        let r = call(&c, "memory_sources", json!({"project":"demo"})).await?;
        assert_ne!(r.is_error, Some(true));
        match format {
            None => {
                assert!(r.content.is_empty());
                assert_eq!(r.structured_content, Some(json!({"sources":[]})));
            }
            Some("text") => {
                assert!(r.structured_content.is_none());
                assert_eq!(text_json(&r), json!({"sources":[]}));
            }
            Some("both") => assert_eq!(text_json(&r), r.structured_content.clone().unwrap()),
            _ => unreachable!(),
        }
        sizes.push(serde_json::to_vec(&r)?.len());
        let denied = call(&c, "memory_sources", json!({"project":"private"})).await?;
        assert_eq!(denied.is_error, Some(true));
        assert!(!denied.content.is_empty());
        c.cancel().await?;
    }
    assert!(sizes[0] < sizes[2]);
    assert!(sizes[1] < sizes[2]);
    Ok(())
}

#[tokio::test]
async fn compact_default_full_opt_in_and_full_note_roundtrip_obey_scope() -> Result<()> {
    let t = tempfile::tempdir()?;
    let notes = t.path().join("notes");
    fs::create_dir(&notes)?;
    let db = t.path().join("db");
    let store = Store::open(&db)?;
    store.create_project("demo")?;
    store.create_project("other")?;
    let source = store.register_source("demo", &notes, false)?;
    store.register_source("other", &notes, false)?;
    drop(store);
    let note=format!("---\nid: doc-a\ntitle: Authored title\ntags: [one, two]\nmemory:\n  id: a\n  project: demo\n  kind: constraint\n  status: accepted\n  summary: Keep local\n  evidence:\n    - kind: manual\n      reference: {}\n  relations:\n    - kind: relates_to\n      target: b\n---\n# Heading\nBody\n","evidence ".repeat(300));
    fs::write(notes.join("a.md"), &note)?;
    fs::write(notes.join("b.md"),"---\nmemory:\n  id: b\n  project: demo\n  kind: fact\n  status: accepted\n  summary: Related\n---\n# B\n")?;
    fs::write(notes.join("foreign.md"),"---\nmemory:\n  id: secret\n  project: other\n  kind: fact\n  status: accepted\n  summary: Private claim\n---\n# Foreign\n")?;
    let c = start(&db, None).await?;
    let compact = call(&c, "memory_list", json!({"project":"demo"}))
        .await?
        .structured_content
        .unwrap();
    let full = call(
        &c,
        "memory_list",
        json!({"project":"demo","include_details":true}),
    )
    .await?
    .structured_content
    .unwrap();
    assert_eq!(compact["memories"].as_array().unwrap().len(), 2);
    let row = &compact["memories"][0];
    assert_eq!(row["title"], "Authored title");
    assert_eq!(
        row["memory"],
        json!({"id":"a","kind":"constraint","status":"accepted","summary":"Keep local"})
    );
    assert!(row.get("tags").is_none());
    assert!(row["memory"].get("evidence").is_none());
    assert!(row["memory"].get("relations").is_none());
    assert_eq!(full["memories"][0]["memory"]["relations"][0]["target"], "b");
    assert_eq!(
        full["memories"][0]["memory"]["evidence"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    let get=call(&c,"memory_get",json!({"project":"demo","source_id":row["source_id"],"path":row["path"],"content_hash":row["content_hash"]})).await?.structured_content.unwrap();
    assert_eq!(get["document"]["content"], note);
    let record = call(
        &c,
        "memory_get_record",
        json!({"project":"demo","memory_id":"a"}),
    )
    .await?
    .structured_content
    .unwrap();
    assert_eq!(record["record"], full["memories"][0]);
    assert_eq!(
        call(&c, "memory_list", json!({"project":"other"}))
            .await?
            .is_error,
        Some(true)
    );
    assert_eq!(
        call(
            &c,
            "memory_list",
            json!({"project":"demo","unexpected":true})
        )
        .await?
        .is_error,
        Some(true)
    );
    let compact_bytes = serde_json::to_vec(&compact)?.len();
    let full_bytes = serde_json::to_vec(&full)?.len();
    assert!(compact_bytes * 2 < full_bytes);
    println!("Fixture JSON bytes: compact={compact_bytes}, full={full_bytes}");
    let cli = Command::new(env!("CARGO_BIN_EXE_agent-memory"))
        .arg("--database")
        .arg(&db)
        .args(["memory", "list", "--project", "demo"])
        .output()?;
    assert!(cli.status.success());
    assert_eq!(
        serde_json::from_slice::<Value>(&cli.stdout)?,
        compact["memories"]
    );
    let cli_full = Command::new(env!("CARGO_BIN_EXE_agent-memory"))
        .arg("--database")
        .arg(&db)
        .args(["memory", "list", "--project", "demo", "--full"])
        .output()?;
    assert!(cli_full.status.success());
    assert_eq!(
        serde_json::from_slice::<Value>(&cli_full.stdout)?,
        full["memories"]
    );
    // Revoke read access live: both list formats must stop returning the source.
    let conn = rusqlite::Connection::open(&db)?;
    conn.execute("UPDATE project_sources SET can_read=0,can_write=0 WHERE source_id=?1 AND project_id=(SELECT id FROM projects WHERE name='demo')",[source])?;
    for details in [false, true] {
        assert_eq!(
            call(
                &c,
                "memory_list",
                json!({"project":"demo","include_details":details})
            )
            .await?
            .structured_content
            .unwrap()["memories"],
            json!([])
        );
    }
    c.cancel().await?;
    Ok(())
}

#[test]
fn old_protocol_gets_text_fallback_and_new_protocol_gets_one_structured_copy() -> Result<()> {
    use std::{io::Write, process::Stdio};
    let t = tempfile::tempdir()?;
    let db = t.path().join("db");
    let store = Store::open(&db)?;
    store.create_project("demo")?;
    drop(store);
    for version in ["2025-03-26", "2025-06-18"] {
        let mut child = Command::new(env!("CARGO_BIN_EXE_agent-memory"))
            .arg("--database")
            .arg(&db)
            .args(["mcp", "--project", "demo"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()?;
        {
            let mut input = child.stdin.take().unwrap();
            for msg in [
                json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":version,"capabilities":{},"clientInfo":{"name":"raw-test","version":"1"}}}),
                json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
                json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"memory_sources","arguments":{"project":"demo"}}}),
            ] {
                writeln!(input, "{msg}")?;
            }
        }
        let o = child.wait_with_output()?;
        assert!(o.status.success());
        let rows: Vec<Value> = String::from_utf8(o.stdout)?
            .lines()
            .map(serde_json::from_str)
            .collect::<Result<_, _>>()?;
        let response = &rows.iter().find(|r| r["id"] == 2).unwrap()["result"];
        if version == "2025-03-26" {
            assert!(response.get("structuredContent").is_none());
            assert_eq!(
                serde_json::from_str::<Value>(response["content"][0]["text"].as_str().unwrap())?,
                json!({"sources":[]})
            );
        } else {
            assert_eq!(response["structuredContent"], json!({"sources":[]}));
            assert_eq!(response["content"], json!([]));
        }
    }
    Ok(())
}
