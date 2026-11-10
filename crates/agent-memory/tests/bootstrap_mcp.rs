use anyhow::{ensure, Result};
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

async fn call(client: &Peer<RoleClient>, name: &str, args: Value) -> Result<CallToolResult> {
    Ok(client
        .call_tool(
            CallToolRequestParams::new(name.to_owned())
                .with_arguments(args.as_object().unwrap().clone()),
        )
        .await?)
}
async fn bootstrap(client: &Peer<RoleClient>, args: Value) -> Result<Value> {
    let response = call(client, "memory_bootstrap", args).await?;
    ensure!(response.is_error != Some(true), "{response:?}");
    Ok(response.structured_content.unwrap())
}
async fn start(db: &Path) -> Result<rmcp::service::RunningService<RoleClient, ()>> {
    let mut command = tokio::process::Command::new(env!("CARGO_BIN_EXE_agent-memory"));
    command
        .arg("--database")
        .arg(db)
        .args(["mcp", "--project", "demo"]);
    Ok(().serve(TokioChildProcess::new(command)?).await?)
}
fn cli(db: &Path, args: &[&str]) -> Result<Output> {
    Ok(Command::new(env!("CARGO_BIN_EXE_agent-memory"))
        .arg("--database")
        .arg(db)
        .args(args)
        .output()?)
}
fn note(id: &str, kind: &str, status: &str, project: &str, tags: &str, body: &str) -> String {
    format!("---\ntags: [{tags}]\nmemory:\n  id: {id}\n  project: {project}\n  kind: {kind}\n  status: {status}\n  summary: Claim {id}\n---\n# Rule\n{body}\n")
}
fn budget(report: &Value, max: usize) -> Result<()> {
    let actual = serde_json::to_vec(report)?.len();
    assert_eq!(report["max_bytes"], max);
    assert_eq!(report["used_bytes"].as_u64().unwrap() as usize, actual);
    assert!(actual <= max, "{actual} > {max}");
    assert_eq!(report["budget_method"], "utf8_bytes");
    assert_eq!(report["atomic_snapshot"], false);
    Ok(())
}
fn git(root: &Path, args: &[&str]) -> Result<String> {
    let output = Command::new("git").current_dir(root).args(args).output()?;
    ensure!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(String::from_utf8(output.stdout)?.trim().to_owned())
}

#[tokio::test]
async fn bootstrap_full_baseline_filters_preferences_scope_and_matches_cli() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let notes = temp.path().join("notes");
    let private = temp.path().join("private");
    fs::create_dir(&notes)?;
    fs::create_dir(&private)?;
    let db = temp.path().join("db");
    let mut store = Store::open(&db)?;
    store.create_project("demo")?;
    store.create_project("other")?;
    let sid = store.register_source("demo", &notes, false)?;
    let secret_sid = store.register_source("demo", &private, false)?;
    fs::write(
        private.join("secret.md"),
        note(
            "private",
            "constraint",
            "accepted",
            "demo",
            "",
            "PRIVATE_BASELINE_SECRET",
        ),
    )?;
    store.index("demo")?;
    let connection = rusqlite::Connection::open(&db)?;
    connection.execute(
        "UPDATE project_sources SET can_read=0 WHERE source_id=?1",
        [secret_sid],
    )?;
    drop(connection);
    drop(store);
    let client = start(&db).await?;
    // Notes appear after initialization: bootstrap must refresh the index itself.
    let inputs = [
        (
            "constraint.md",
            note(
                "z-rule",
                "constraint",
                "accepted",
                "demo",
                "",
                "правило 🐈 \\\" Unicode",
            ),
        ),
        (
            "preference.md",
            note(
                "a-pref",
                "decision",
                "accepted",
                "demo",
                "preference",
                "first preference",
            ),
        ),
        (
            "preferences.md",
            note(
                "b-pref",
                "fact",
                "accepted",
                "demo",
                "preferences",
                "second preference",
            ),
        ),
        (
            "retired.md",
            note(
                "retired",
                "constraint",
                "deprecated",
                "demo",
                "preference",
                "RETIRED_SECRET",
            ),
        ),
        (
            "planned.md",
            note(
                "planned",
                "constraint",
                "planned",
                "demo",
                "preferences",
                "PLANNED_SECRET",
            ),
        ),
        (
            "foreign.md",
            note(
                "foreign",
                "constraint",
                "accepted",
                "other",
                "preference",
                "FOREIGN_SECRET",
            ),
        ),
        (
            "ordinary.md",
            note(
                "ordinary",
                "fact",
                "accepted",
                "demo",
                "other",
                "ordinary fact",
            ),
        ),
    ];
    for (path, text) in &inputs {
        fs::write(notes.join(path), text)?;
    }
    let report = bootstrap(&client, json!({"project":"demo"})).await?;
    budget(&report, 65536)?;
    assert_eq!(report["baseline_complete"], true);
    assert_eq!(report["context"]["status"], "not_requested");
    assert_eq!(report["baseline"].as_array().unwrap().len(), 3);
    assert_eq!(
        report["baseline"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| r["memory_id"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["z-rule", "a-pref", "b-pref"]
    );
    for (index, (path, text)) in inputs[..3].iter().enumerate() {
        let entry = &report["baseline"][index];
        assert_eq!(entry["status"], "ok");
        assert_eq!(entry["source_id"], sid);
        assert_eq!(entry["path"], *path);
        assert_eq!(
            entry["reason"],
            if index == 0 {
                "accepted_constraint"
            } else {
                "accepted_preference"
            }
        );
        assert_eq!(entry["document"]["content"], *text);
        assert_eq!(
            entry["content_hash"],
            memory_core::content_hash(text.as_bytes())
        );
        assert_eq!(entry["document"]["content_hash"], entry["content_hash"]);
    }
    let encoded = report.to_string();
    for secret in [
        "PRIVATE_BASELINE_SECRET",
        "RETIRED_SECRET",
        "PLANNED_SECRET",
        "FOREIGN_SECRET",
        "foreign.md",
        "secret.md",
    ] {
        assert!(!encoded.contains(secret), "{secret}");
    }
    assert!(report["freshness"]
        .as_array()
        .unwrap()
        .iter()
        .all(|r| r["source_id"] != secret_sid));
    let output = cli(&db, &["bootstrap", "--project", "demo"])?;
    assert!(output.status.success(), "{output:?}");
    assert_eq!(serde_json::from_slice::<Value>(&output.stdout)?, report);
    assert!(output.stderr.is_empty());
    // Read-only MCP registration and no --allow-write must be sufficient.
    for (path, text) in &inputs {
        assert_eq!(fs::read_to_string(notes.join(path))?, *text);
    }
    client.cancel().await?;
    Ok(())
}

#[tokio::test]
async fn small_budget_omits_whole_large_notes_without_blocking_small_ones_or_hiding_manifest(
) -> Result<()> {
    let temp = tempfile::tempdir()?;
    let notes = temp.path().join("notes");
    fs::create_dir(&notes)?;
    let db = temp.path().join("db");
    let store = Store::open(&db)?;
    store.create_project("demo")?;
    store.register_source("demo", &notes, false)?;
    drop(store);
    let large = note(
        "a-large",
        "constraint",
        "accepted",
        "demo",
        "",
        &"память 🐈 \\\"\n".repeat(1000),
    );
    let small = note(
        "b-small",
        "constraint",
        "accepted",
        "demo",
        "",
        "entire small note",
    );
    fs::write(notes.join("large.md"), &large)?;
    fs::write(notes.join("small.md"), &small)?;
    let client = start(&db).await?;
    let report = bootstrap(&client, json!({"project":"demo","max_bytes":4096})).await?;
    budget(&report, 4096)?;
    assert_eq!(report["baseline_complete"], false);
    assert_eq!(report["baseline"][0]["status"], "omitted_budget");
    assert!(report["baseline"][0].get("document").is_none());
    assert_eq!(report["baseline"][1]["status"], "ok");
    assert_eq!(report["baseline"][1]["document"]["content"], small);
    assert_eq!(
        report["baseline"][0]["content_hash"],
        memory_core::content_hash(large.as_bytes())
    );
    let full = bootstrap(&client, json!({"project":"demo","max_bytes":1048576})).await?;
    budget(&full, 1048576)?;
    assert_eq!(full["baseline_complete"], true);
    assert_eq!(full["baseline"][0]["document"]["content"], large);
    // Even an omitted baseline needs a complete manifest; error instead of silently dropping IDs.
    for index in 0..50 {
        fs::write(
            notes.join(format!("extra-{index}.md")),
            note(
                &format!("extra-{index}"),
                "constraint",
                "accepted",
                "demo",
                "",
                "x",
            ),
        )?;
    }
    let too_many = call(
        &client,
        "memory_bootstrap",
        json!({"project":"demo","max_bytes":4096}),
    )
    .await?;
    assert_eq!(too_many.is_error, Some(true), "{too_many:?}");
    assert!(too_many.structured_content.is_none());
    client.cancel().await?;
    Ok(())
}

#[tokio::test]
async fn optional_task_context_budget_validation_and_cursor_capture_changes() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let notes = temp.path().join("notes");
    fs::create_dir(&notes)?;
    let db = temp.path().join("db");
    let store = Store::open(&db)?;
    store.create_project("demo")?;
    store.register_source("demo", &notes, false)?;
    drop(store);
    fs::write(
        notes.join("baseline.md"),
        note(
            "rule",
            "constraint",
            "accepted",
            "demo",
            "",
            "authentication baseline",
        ),
    )?;
    fs::write(
        notes.join("task.md"),
        "# Authentication task\nUseful authentication implementation.\n",
    )?;
    let client = start(&db).await?;
    let report = bootstrap(&client, json!({"project":"demo","task":"authentication"})).await?;
    budget(&report, 65536)?;
    assert_eq!(report["context"]["status"], "ok");
    let context = report["context"]["entries"].as_array().unwrap();
    assert!(context.iter().any(|r| r["path"] == "task.md"));
    assert!(!context.iter().any(|r| r["path"] == "baseline.md"));
    assert!(report["context"]["candidate_omitted_count"].is_u64());
    assert!(report["revision"].is_u64());
    assert!(report["cursor"].as_str().is_some_and(|s| !s.is_empty()));
    let output = cli(
        &db,
        &[
            "bootstrap",
            "--project",
            "demo",
            "--max-bytes",
            "65536",
            "authentication",
        ],
    )?;
    assert!(output.status.success(), "{output:?}");
    assert_eq!(serde_json::from_slice::<Value>(&output.stdout)?, report);
    let small = bootstrap(
        &client,
        json!({"project":"demo","task":"authentication","max_bytes":4096}),
    )
    .await?;
    budget(&small, 4096)?;
    let old_cursor = report["cursor"].clone();
    fs::write(
        notes.join("task.md"),
        "# Authentication task\nUpdated implementation.\n",
    )?;
    let changes = call(
        &client,
        "memory_changes",
        json!({"project":"demo","since":old_cursor}),
    )
    .await?;
    assert_ne!(changes.is_error, Some(true));
    assert!(changes.structured_content.unwrap()["changes"]
        .as_array()
        .unwrap()
        .iter()
        .any(|r| r["path"] == "task.md"));
    for args in [
        json!({"project":"demo","max_bytes":4095}),
        json!({"project":"demo","max_bytes":1048577}),
        json!({"project":"demo","task":""}),
        json!({"project":"demo","task":" \n\t"}),
        json!({"project":"demo","task":"я".repeat(4097)}),
        json!({"project":"demo","unexpected":true}),
        json!({"project":"other"}),
    ] {
        assert_eq!(
            call(&client, "memory_bootstrap", args.clone())
                .await?
                .is_error,
            Some(true),
            "{args}"
        );
    }
    for args in [
        vec!["bootstrap", "--project", "demo", "--max-bytes", "4095"],
        vec!["bootstrap", "--project", "demo", "--max-bytes", "1048577"],
        vec!["bootstrap", "--project", "demo", " "],
    ] {
        assert!(!cli(&db, &args)?.status.success(), "{args:?}");
    }
    let schema = client
        .list_all_tools()
        .await?
        .into_iter()
        .find(|r| r.name == "memory_bootstrap")
        .unwrap()
        .input_schema;
    assert_eq!(schema["properties"]["max_bytes"]["minimum"], 4096);
    assert_eq!(schema["properties"]["max_bytes"]["maximum"], 1048576);
    client.cancel().await?;
    Ok(())
}

#[tokio::test]
async fn freshness_is_cached_by_default_fetch_is_opt_in_and_errors_do_not_leak_paths() -> Result<()>
{
    let temp = tempfile::tempdir()?;
    let remote = temp.path().join("remote.git");
    let notes = temp.path().join("notes");
    let other = temp.path().join("other");
    git(temp.path(), &["init", "--bare", remote.to_str().unwrap()])?;
    git(&remote, &["symbolic-ref", "HEAD", "refs/heads/main"])?;
    git(
        temp.path(),
        &["clone", remote.to_str().unwrap(), notes.to_str().unwrap()],
    )?;
    git(&notes, &["config", "user.name", "Test"])?;
    git(&notes, &["config", "user.email", "test@example.test"])?;
    let original = note(
        "rule",
        "constraint",
        "accepted",
        "demo",
        "",
        "Original rule",
    );
    fs::write(notes.join("rule.md"), &original)?;
    git(&notes, &["add", "."])?;
    git(&notes, &["commit", "-m", "Seed"])?;
    git(&notes, &["push", "-u", "origin", "HEAD"])?;
    git(
        temp.path(),
        &["clone", remote.to_str().unwrap(), other.to_str().unwrap()],
    )?;
    git(&other, &["config", "user.name", "Test"])?;
    git(&other, &["config", "user.email", "test@example.test"])?;
    fs::write(
        other.join("remote-only.md"),
        "# Unpulled remote knowledge\n",
    )?;
    git(&other, &["add", "."])?;
    git(&other, &["commit", "-m", "Remote update"])?;
    git(&other, &["push"])?;
    let before = git(&notes, &["rev-parse", "HEAD"])?;
    let db = temp.path().join("db");
    let store = Store::open(&db)?;
    store.create_project("demo")?;
    let sid = store.register_source("demo", &notes, false)?;
    drop(store);
    let client = start(&db).await?;
    let cached = bootstrap(&client, json!({"project":"demo"})).await?;
    let freshness = &cached["freshness"][0];
    assert_eq!(freshness["source_id"], sid);
    assert_eq!(freshness["status"], "ok");
    assert_eq!(freshness["freshness"]["checked_remote"], false);
    assert_eq!(freshness["freshness"]["status"], "up_to_date");
    let fetched = bootstrap(&client, json!({"project":"demo","fetch":true})).await?;
    budget(&fetched, 65536)?;
    assert_eq!(fetched["freshness"][0]["freshness"]["checked_remote"], true);
    assert_eq!(fetched["freshness"][0]["freshness"]["status"], "behind");
    assert_eq!(fetched["freshness"][0]["freshness"]["behind"], 1);
    let output = cli(&db, &["bootstrap", "--project", "demo", "--fetch"])?;
    assert!(output.status.success(), "{output:?}");
    assert_eq!(serde_json::from_slice::<Value>(&output.stdout)?, fetched);
    assert_eq!(git(&notes, &["rev-parse", "HEAD"])?, before);
    assert_eq!(git(&notes, &["status", "--porcelain"])?, "");
    assert_eq!(fs::read_to_string(notes.join("rule.md"))?, original);
    assert!(!notes.join("remote-only.md").exists());
    let unavailable_remote = temp.path().join("PRIVATE_REMOTE_TOKEN");
    git(
        &notes,
        &[
            "remote",
            "set-url",
            "origin",
            unavailable_remote.to_str().unwrap(),
        ],
    )?;
    // Cached operation still succeeds, demonstrating it does not attempt remote access.
    let cached_again = bootstrap(&client, json!({"project":"demo"})).await?;
    assert_eq!(cached_again["freshness"][0]["status"], "ok");
    let failed_fetch = bootstrap(&client, json!({"project":"demo","fetch":true})).await?;
    budget(&failed_fetch, 65536)?;
    assert_eq!(failed_fetch["baseline_complete"], true);
    assert_eq!(failed_fetch["freshness"][0]["status"], "error");
    assert!(failed_fetch["freshness"][0]["freshness"].is_null());
    assert!(!failed_fetch.to_string().contains("PRIVATE_REMOTE_TOKEN"));
    assert!(!failed_fetch.to_string().contains(notes.to_str().unwrap()));
    client.cancel().await?;
    Ok(())
}

#[tokio::test]
async fn omitted_context_candidates_keep_coordinates_without_truncating_full_baseline() -> Result<()>
{
    let temp = tempfile::tempdir()?;
    let notes = temp.path().join("notes");
    fs::create_dir(&notes)?;
    let db = temp.path().join("db");
    let store = Store::open(&db)?;
    store.create_project("demo")?;
    let sid = store.register_source("demo", &notes, false)?;
    drop(store);
    let baseline = note(
        "rule",
        "constraint",
        "accepted",
        "demo",
        "",
        "authentication baseline",
    );
    fs::write(notes.join("baseline.md"), &baseline)?;
    let mut expected = std::collections::BTreeMap::new();
    for index in 0..8 {
        let path = format!("task-{index}.md");
        let body = format!(
            "# Authentication {index}\n{}",
            "authentication Unicode 🐈 \\\"\n".repeat(100)
        );
        expected.insert(path.clone(), memory_core::content_hash(body.as_bytes()));
        fs::write(notes.join(path), body)?;
    }
    let client = start(&db).await?;
    let report = bootstrap(
        &client,
        json!({"project":"demo","task":"authentication","max_bytes":4096}),
    )
    .await?;
    budget(&report, 4096)?;
    assert_eq!(report["baseline_complete"], true);
    assert_eq!(report["baseline"][0]["document"]["content"], baseline);
    let omitted = report["context"]["omitted_entries"].as_array().unwrap();
    assert!(!omitted.is_empty());
    assert_eq!(report["context"]["omitted_count"], omitted.len());
    let mut returned = std::collections::BTreeMap::new();
    for entry in report["context"]["entries"]
        .as_array()
        .unwrap()
        .iter()
        .chain(omitted)
    {
        assert_eq!(entry["source_id"], sid);
        let path = entry["path"].as_str().unwrap();
        let hash = entry["content_hash"].as_str().unwrap();
        assert_eq!(Some(hash), expected.get(path).map(String::as_str));
        assert!(returned.insert(path.to_owned(), hash.to_owned()).is_none());
    }
    assert_eq!(returned, expected);
    assert!(omitted.iter().all(|entry| entry.get("excerpt").is_none()));
    client.cancel().await?;
    Ok(())
}

#[tokio::test]
async fn short_core_and_bounded_task_preferences_replace_startup_pref_flood() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let notes = temp.path().join("notes");
    fs::create_dir(&notes)?;
    let db = temp.path().join("db");
    let mut store = Store::open(&db)?;
    store.create_project("demo")?;
    store.add_source("demo", &notes)?;
    fs::write(
        notes.join("core.md"),
        note(
            "core",
            "constraint",
            "accepted",
            "demo",
            "'recall/always'",
            "Short mandatory rules.",
        ),
    )?;
    for i in 0..20 {
        let topic = if i < 6 { "release" } else { "git" };
        fs::write(
            notes.join(format!("pref-{i:02}.md")),
            note(
                &format!("pref-{i:02}"),
                "constraint",
                "accepted",
                "demo",
                &format!("'recall/task', 'topic/{topic}'"),
                &format!("release {}", "historical details ".repeat(200)),
            ),
        )?;
    }
    fs::write(notes.join("task.md"), "# Delivery\nrelease delivery\n")?;
    store.index("demo")?;
    drop(store);
    let client = start(&db).await?;
    let initial = bootstrap(&client, json!({"project":"demo"})).await?;
    assert_eq!(initial["baseline"].as_array().unwrap().len(), 1);
    assert_eq!(initial["baseline"][0]["memory_id"], "core");
    assert_eq!(initial["preferences"]["status"], "not_requested");
    assert_eq!(initial["preferences"]["deferred_count"], 20);
    assert!(initial["preferences"]["entries"]
        .as_array()
        .unwrap()
        .is_empty());
    assert!(serde_json::to_vec(&initial)?.len() < 4096);
    assert!(!serde_json::to_string(&initial)?.contains("historical details"));
    for limit in [0, 2, 3] {
        let selected = bootstrap(
            &client,
            json!({"project":"demo","task":"release","preference_limit":limit}),
        )
        .await?;
        budget(&selected, 65536)?;
        assert_eq!(selected["preferences"]["matched_count"], 6);
        assert_eq!(selected["preferences"]["deferred_count"], 20 - limit);
        assert_eq!(
            selected["preferences"]["entries"].as_array().unwrap().len(),
            limit as usize
        );
        assert_eq!(selected["preferences"]["complete"], true);
        for entry in selected["preferences"]["entries"].as_array().unwrap() {
            assert_eq!(entry["status"], "ok");
            assert!(entry["document"]["content"]
                .as_str()
                .unwrap()
                .contains("topic/release"));
        }
        for entry in selected["context"]["entries"].as_array().unwrap() {
            assert_eq!(entry["path"], "task.md"); // Deferred preferences cannot leak as excerpts.
        }
        let out = cli(
            &db,
            &[
                "bootstrap",
                "--project",
                "demo",
                "--preference-limit",
                &limit.to_string(),
                "release",
            ],
        )?;
        assert!(out.status.success());
        assert_eq!(serde_json::from_slice::<Value>(&out.stdout)?, selected);
    }
    let bad = call(
        &client,
        "memory_bootstrap",
        json!({"project":"demo","preference_limit":21}),
    )
    .await?;
    assert_eq!(bad.is_error, Some(true));
    client.cancel().await?;
    Ok(())
}

#[tokio::test]
async fn task_preference_full_read_omission_cannot_replace_or_displace_required_core() -> Result<()>
{
    let temp = tempfile::tempdir()?;
    let notes = temp.path().join("notes");
    fs::create_dir(&notes)?;
    let db = temp.path().join("db");
    let mut store = Store::open(&db)?;
    store.create_project("demo")?;
    store.add_source("demo", &notes)?;
    fs::write(
        notes.join("core.md"),
        note(
            "core",
            "constraint",
            "accepted",
            "demo",
            "'recall/always'",
            "Mandatory short rule.",
        ),
    )?;
    fs::write(
        notes.join("git.md"),
        note(
            "git",
            "constraint",
            "accepted",
            "demo",
            "'recall/task', 'topic/git'",
            &"Long git history. ".repeat(1000),
        ),
    )?;
    store.index("demo")?;
    drop(store);
    let client = start(&db).await?;
    let report = bootstrap(
        &client,
        json!({"project":"demo","task":"git","max_bytes":4096}),
    )
    .await?;
    budget(&report, 4096)?;
    assert_eq!(report["baseline_complete"], true);
    assert!(report["baseline"][0]["document"]["content"]
        .as_str()
        .unwrap()
        .contains("Mandatory short rule."));
    assert_eq!(report["preferences"]["complete"], false);
    assert_eq!(
        report["preferences"]["entries"][0]["status"],
        "omitted_budget"
    );
    assert!(report["preferences"]["entries"][0]
        .get("document")
        .is_none());
    assert!(report["context"]["entries"].as_array().unwrap().is_empty());
    client.cancel().await?;
    Ok(())
}
