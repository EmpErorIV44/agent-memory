use anyhow::Result;
use memory_sqlite::{DocumentSelector, Store};
use rmcp::{
    model::{CallToolRequestParams, CallToolResult},
    transport::TokioChildProcess,
    Peer, RoleClient, ServiceExt,
};
use serde_json::{json, Value};
use std::{fs, path::Path, process::Command};
async fn call(c: &Peer<RoleClient>, name: &str, args: Value) -> Result<CallToolResult> {
    Ok(c.call_tool(
        CallToolRequestParams::new(name.to_owned())
            .with_arguments(args.as_object().unwrap().clone()),
    )
    .await?)
}
async fn start(db: &Path) -> Result<rmcp::service::RunningService<RoleClient, ()>> {
    let mut c = tokio::process::Command::new(env!("CARGO_BIN_EXE_agent-memory"));
    c.arg("--database")
        .arg(db)
        .args(["mcp", "--project", "demo"]);
    Ok(().serve(TokioChildProcess::new(c)?).await?)
}
fn cli(db: &Path, args: &[&str]) -> Result<Value> {
    let o = Command::new(env!("CARGO_BIN_EXE_agent-memory"))
        .arg("--database")
        .arg(db)
        .args(args)
        .output()?;
    anyhow::ensure!(o.status.success(), "{o:?}");
    Ok(serde_json::from_slice(&o.stdout)?)
}
fn note(id: &str, kind: &str, status: &str, tag: &str) -> String {
    format!("---\ntags: [{tag}]\nmemory:\n  id: {id}\n  project: demo\n  kind: {kind}\n  status: {status}\n  summary: Claim {id}\n---\n# Note\n")
}
#[tokio::test]
async fn list_filters_paging_full_records_and_cli_obey_source_scope() -> Result<()> {
    let t = tempfile::tempdir()?;
    let db = t.path().join("db");
    let one = t.path().join("one");
    let two = t.path().join("two");
    let private = t.path().join("private");
    for dir in [&one, &two, &private] {
        fs::create_dir(dir)?;
    }
    fs::write(
        one.join("a.md"),
        note("a", "constraint", "accepted", "auth"),
    )?;
    fs::write(one.join("b.md"), note("b", "fact", "planned", "auth"))?;
    fs::write(
        two.join("z.md"),
        note("z", "constraint", "accepted", "other"),
    )?;
    fs::write(
        private.join("secret.md"),
        note("secret", "constraint", "accepted", "auth"),
    )?;
    let store = Store::open(&db)?;
    store.create_project("demo")?;
    store.create_project("private")?;
    let sid = store.register_source("demo", &one, false)?;
    store.register_source("demo", &two, false)?;
    store.register_source("private", &private, false)?;
    drop(store);
    let c = start(&db).await?;
    let all = call(&c, "memory_list", json!({"project":"demo"}))
        .await?
        .structured_content
        .unwrap();
    assert_eq!(all.as_object().unwrap().len(), 1);
    assert_eq!(all["memories"].as_array().unwrap().len(), 3);
    let filtered=call(&c,"memory_list",json!({"project":"demo","kind":"constraint","status":"accepted","tag":"auth","source_id":sid,"limit":1})).await?.structured_content.unwrap();
    assert_eq!(filtered["memories"][0]["memory"]["id"], "a");
    assert_eq!(filtered["has_more"], false);
    let mut ids = Vec::new();
    for offset in 0..3 {
        let p = call(
            &c,
            "memory_list",
            json!({"project":"demo","limit":1,"offset":offset}),
        )
        .await?
        .structured_content
        .unwrap();
        ids.push(
            p["memories"][0]["memory"]["id"]
                .as_str()
                .unwrap()
                .to_owned(),
        );
        assert_eq!(p["has_more"], offset < 2);
        if offset < 2 {
            assert_eq!(p["next_offset"], offset + 1);
        } else {
            assert!(p["next_offset"].is_null());
        }
    }
    assert_eq!(ids, ["a", "b", "z"]);
    let full = call(
        &c,
        "memory_list",
        json!({"project":"demo","kind":"constraint","include_details":true,"limit":1,"offset":1}),
    )
    .await?
    .structured_content
    .unwrap();
    assert_eq!(full["memories"][0]["memory"]["id"], "z");
    assert!(full["memories"][0]["memory"]["evidence"].is_array());
    assert_eq!(
        full,
        cli(
            &db,
            &[
                "memory",
                "list",
                "--project",
                "demo",
                "--kind",
                "constraint",
                "--full",
                "--limit",
                "1",
                "--offset",
                "1"
            ]
        )?
    );
    let empty = call(&c, "memory_list", json!({"project":"demo","offset":99}))
        .await?
        .structured_content
        .unwrap();
    assert_eq!(empty["limit"], 50);
    assert_eq!(empty["memories"], json!([]));
    for args in [
        json!({"project":"demo","limit":0}),
        json!({"project":"demo","limit":201}),
        json!({"project":"demo","kind":"invented"}),
        json!({"project":"demo","status":"invented"}),
        json!({"project":"demo","tag":" "}),
    ] {
        assert_eq!(call(&c, "memory_list", args).await?.is_error, Some(true));
    }
    let literal = call(
        &c,
        "memory_list",
        json!({"project":"demo","tag":"auth' OR 1=1 --"}),
    )
    .await?
    .structured_content
    .unwrap();
    assert_eq!(literal["memories"], json!([]));
    c.cancel().await?;
    Ok(())
}
#[tokio::test]
async fn get_many_full_roundtrip_budget_omissions_unavailable_and_cli_parity() -> Result<()> {
    let t = tempfile::tempdir()?;
    let db = t.path().join("db");
    let notes = t.path().join("notes");
    let private = t.path().join("private");
    fs::create_dir(&notes)?;
    fs::create_dir(&private)?;
    let store = Store::open(&db)?;
    store.create_project("demo")?;
    store.create_project("private")?;
    let sid = store.register_source("demo", &notes, false)?;
    let other = store.register_source("private", &private, false)?;
    drop(store);
    let c = start(&db).await?; // Read must refresh notes created after initialization.
    let huge = format!("# Unicode\n{}", "память 🐈 ".repeat(1000));
    let small = "# Full small note\nBody\n";
    fs::write(notes.join("huge.md"), &huge)?;
    fs::write(notes.join("small.md"), small)?;
    fs::write(private.join("secret.md"), "SECRET")?;
    let selectors = json!([
        {"source_id":sid,"path":"huge.md","content_hash":memory_core::content_hash(huge.as_bytes())},
        {"source_id":sid,"path":"small.md","content_hash":memory_core::content_hash(small.as_bytes())},
        {"source_id":other,"path":"secret.md","content_hash":memory_core::content_hash(b"SECRET")},
        {"source_id":sid,"path":"stale.md","content_hash":"0".repeat(64)}
    ]);
    let args = json!({"project":"demo","documents":selectors,"max_bytes":4096});
    let r = call(&c, "memory_get_many", args.clone())
        .await?
        .structured_content
        .unwrap();
    assert_eq!(r["results"][0]["status"], "omitted_budget");
    assert_eq!(r["results"][1]["document"]["content"], small);
    assert_eq!(r["results"][2], json!({"index":2,"status":"unavailable"}));
    assert_eq!(r["results"][3], json!({"index":3,"status":"unavailable"}));
    assert_eq!(r["omitted_count"], 1);
    assert_eq!(r["unavailable_count"], 2);
    assert_eq!(r["complete"], false);
    assert_eq!(
        r["used_bytes"].as_u64().unwrap() as usize,
        serde_json::to_vec(&r)?.len()
    );
    assert!(serde_json::to_vec(&r)?.len() <= 4096);
    assert!(!r.to_string().contains("SECRET"));
    let file = t.path().join("selectors.json");
    fs::write(&file, serde_json::to_vec(&selectors)?)?;
    assert_eq!(
        r,
        cli(
            &db,
            &[
                "memory",
                "get-many",
                "--project",
                "demo",
                "--documents-file",
                file.to_str().unwrap(),
                "--max-bytes",
                "4096"
            ]
        )?
    );
    let valid = json!([selectors[0].clone(), selectors[1].clone()]);
    let complete = call(
        &c,
        "memory_get_many",
        json!({"project":"demo","documents":valid}),
    )
    .await?
    .structured_content
    .unwrap();
    assert_eq!(complete["complete"], true);
    assert_eq!(complete["results"][0]["document"]["content"], huge);
    fs::write(notes.join("small.md"), "changed")?;
    let stale = call(
        &c,
        "memory_get_many",
        json!({"project":"demo","documents":[selectors[1].clone()]}),
    )
    .await?
    .structured_content
    .unwrap();
    assert_eq!(stale["results"][0]["status"], "unavailable");
    for bad in [
        json!({"project":"demo","documents":[]}),
        json!({"project":"demo","documents":[selectors[1].clone()],"max_bytes":4095}),
        json!({"project":"demo","documents":[selectors[1].clone()],"max_bytes":1048577}),
        json!({"project":"demo","documents":[selectors[1].clone(),selectors[1].clone()]}),
        json!({"project":"private","documents":[selectors[1].clone()]}),
    ] {
        assert_eq!(call(&c, "memory_get_many", bad).await?.is_error, Some(true));
    }
    c.cancel().await?;
    Ok(())
}
#[test]
fn bounded_library_read_handles_dirty_bytes_and_live_permission_revocation() -> Result<()> {
    let t = tempfile::tempdir()?;
    let db = t.path().join("db");
    let notes = t.path().join("notes");
    fs::create_dir(&notes)?;
    fs::write(notes.join("a.md"), "# First")?;
    let mut store = Store::open(&db)?;
    store.create_project("demo")?;
    let sid = store.register_source("demo", &notes, false)?;
    store.index("demo")?;
    let selector = DocumentSelector {
        source_id: sid,
        path: "a.md".into(),
        content_hash: memory_core::content_hash(b"# First"),
    };
    fs::write(notes.join("a.md"), "# Dirty")?;
    assert_eq!(
        store.get_many("demo", std::slice::from_ref(&selector), 4096)?["results"][0]["status"],
        "unavailable"
    );
    fs::write(notes.join("a.md"), "# First")?;
    let conn = rusqlite::Connection::open(&db)?;
    conn.execute(
        "UPDATE project_sources SET can_read=0 WHERE source_id=?1",
        [sid],
    )?;
    assert_eq!(
        store.get_many("demo", &[selector], 4096)?["results"][0]["status"],
        "unavailable"
    );
    Ok(())
}

#[test]
fn json_escaping_and_document_metadata_are_included_in_batch_budget() -> Result<()> {
    let t = tempfile::tempdir()?;
    let notes = t.path().join("notes");
    fs::create_dir(&notes)?;
    let body = format!(
        "---\n{}\n---\n# Tiny body\n",
        json!({"title":"\"\\".repeat(1500)})
    );
    fs::write(notes.join("metadata.md"), &body)?;
    let mut store = Store::open(&t.path().join("db"))?;
    store.create_project("demo")?;
    let sid = store.register_source("demo", &notes, false)?;
    store.index("demo")?;
    let selector = DocumentSelector {
        source_id: sid,
        path: "metadata.md".into(),
        content_hash: memory_core::content_hash(body.as_bytes()),
    };
    let omitted = store.get_many("demo", std::slice::from_ref(&selector), 4096)?;
    assert_eq!(omitted["results"][0]["status"], "omitted_budget");
    assert_eq!(
        omitted["used_bytes"].as_u64().unwrap() as usize,
        serde_json::to_vec(&omitted)?.len()
    );
    let full = store.get_many("demo", &[selector], 65536)?;
    assert_eq!(full["results"][0]["document"]["content"], body);
    assert_eq!(
        full["used_bytes"].as_u64().unwrap() as usize,
        serde_json::to_vec(&full)?.len()
    );
    Ok(())
}
