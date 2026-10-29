use std::{
    path::Path,
    process::{Command, Output},
};

use serde_json::Value;

fn invoke(db: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_agent-memory"))
        .arg("--database")
        .arg(db)
        .args(args)
        .output()
        .expect("launch CLI")
}

fn run(db: &Path, args: &[&str]) -> String {
    let result = invoke(db, args);
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    String::from_utf8(result.stdout).unwrap()
}

#[test]
fn structured_cli_refreshes_and_rebuilds_without_cross_project_disclosure() {
    let temp = tempfile::tempdir().unwrap();
    let notes = temp.path().join("notes");
    std::fs::create_dir(&notes).unwrap();
    let db = temp.path().join("index.sqlite");
    let old = "---\nid: old-document\nmemory:\n  id: old-decision\n  project: demo\n  kind: decision\n  status: superseded\n  summary: Previous choice\n---\n# Previous\nolddecisionmarker\n";
    let current = "---\nid: new-document\nmemory:\n  id: new-decision\n  project: demo\n  kind: decision\n  status: planned\n  summary: Current choice\n  evidence:\n    - kind: document\n      reference: old-document\n  relations:\n    - kind: supersedes\n      target: old-decision\n---\n# Current\nnewdecisionmarker\n";
    std::fs::write(notes.join("old.md"), old).unwrap();
    std::fs::write(notes.join("current.md"), current).unwrap();
    for project in ["demo", "other"] {
        run(&db, &["project", "create", project]);
        run(
            &db,
            &[
                "source",
                "add",
                "--project",
                project,
                notes.to_str().unwrap(),
            ],
        );
    }
    let list: Value =
        serde_json::from_str(&run(&db, &["memory", "list", "--project", "demo"])).unwrap();
    assert_eq!(list.as_array().unwrap().len(), 2);
    assert_eq!(
        run(&db, &["memory", "list", "--project", "other"]).trim(),
        "[]"
    );
    let get: Value = serde_json::from_str(&run(
        &db,
        &["memory", "get", "--project", "demo", "new-decision"],
    ))
    .unwrap();
    assert_eq!(get["stable_id"], "new-document");
    assert_eq!(get["memory"]["id"], "new-decision");
    assert_eq!(get["memory"]["status"], "planned");
    assert_eq!(get["memory"]["evidence"][0]["reference"], "old-document");
    assert_eq!(get["memory"]["relations"][0]["kind"], "supersedes");
    let denied = invoke(
        &db,
        &["memory", "get", "--project", "other", "new-decision"],
    );
    let missing = invoke(&db, &["memory", "get", "--project", "other", "unknown"]);
    assert!(!denied.status.success());
    assert!(!missing.status.success());
    assert_eq!(denied.stderr, missing.stderr);
    assert!(!String::from_utf8_lossy(&denied.stderr).contains("Current choice"));

    std::fs::write(
        notes.join("current.md"),
        current.replace("status: planned", "status: implemented"),
    )
    .unwrap();
    let updated: Value = serde_json::from_str(&run(
        &db,
        &["memory", "get", "--project", "demo", "new-decision"],
    ))
    .unwrap();
    assert_eq!(updated["memory"]["status"], "implemented");
    let config = temp.path().join("owner.json");
    run(
        &db,
        &["config", "export", "--output", config.to_str().unwrap()],
    );
    std::fs::remove_file(&db).unwrap();
    run(&db, &["rebuild", "--config", config.to_str().unwrap()]);
    let restored: Value = serde_json::from_str(&run(
        &db,
        &["memory", "get", "--project", "demo", "new-decision"],
    ))
    .unwrap();
    assert_eq!(updated["memory"], restored["memory"]);
    assert_eq!(updated["stable_id"], restored["stable_id"]);
}
