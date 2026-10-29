use std::{
    path::Path,
    process::{Command, Output},
};

use serde_json::Value;

fn invoke(database: &Path, arguments: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_agent-memory"))
        .arg("--database")
        .arg(database)
        .args(arguments)
        .output()
        .expect("launch history CLI")
}

fn run(database: &Path, arguments: &[&str]) -> String {
    let output = invoke(database, arguments);
    assert!(
        output.status.success(),
        "arguments={arguments:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

fn json(database: &Path, arguments: &[&str]) -> Value {
    serde_json::from_str(&run(database, arguments)).unwrap()
}

fn source_id(database: &Path, project: &str) -> String {
    json(database, &["source", "list", "--project", project])[0]["source_id"].to_string()
}

fn history(database: &Path, source: &str) -> Value {
    json(
        database,
        &[
            "history",
            "list",
            "--project",
            "writer",
            "--source-id",
            source,
            "--path",
            "note.md",
        ],
    )
}

#[test]
fn revisions_restore_as_new_writes_and_survive_index_reconstruction() {
    let temp = tempfile::tempdir().unwrap();
    let notes = temp.path().join("notes");
    std::fs::create_dir(&notes).unwrap();
    let database = temp.path().join("index.sqlite");
    let input = temp.path().join("content.md");
    let first = "---\nid: historical-document\n---\n# First\nfirstrevisionmarker\n";
    let second = "---\nid: historical-document\n---\n# Second\nsecondrevisionmarker\n";
    let third = "---\nid: historical-document\n---\n# Third\nthirdrevisionmarker\n";
    run(&database, &["project", "create", "writer"]);
    run(
        &database,
        &[
            "source",
            "add",
            "--project",
            "writer",
            "--write",
            notes.to_str().unwrap(),
        ],
    );
    let source = source_id(&database, "writer");
    std::fs::write(&input, first).unwrap();
    let write_arguments = [
        "write",
        "--project",
        "writer",
        "--source-id",
        &source,
        "--path",
        "note.md",
        "--content-file",
        input.to_str().unwrap(),
    ];
    let initial = json(&database, &write_arguments);
    assert_eq!(initial["indexed"], true);
    let first_hash = initial["content_hash"].as_str().unwrap();
    std::fs::write(&input, second).unwrap();
    let mut update_arguments = write_arguments.to_vec();
    update_arguments.extend(["--expected-hash", first_hash]);
    let updated = json(&database, &update_arguments);
    assert_eq!(updated["indexed"], true);
    let second_hash = updated["content_hash"].as_str().unwrap();

    let before_restore = history(&database, &source);
    let revisions = before_restore.as_array().unwrap();
    assert_eq!(revisions.len(), 2);
    assert_eq!(revisions[0]["content_hash"], first_hash);
    assert_eq!(revisions[1]["content_hash"], second_hash);
    let revision = revisions[0]["id"].as_str().unwrap();
    let get_arguments = [
        "history",
        "get",
        "--project",
        "writer",
        "--source-id",
        &source,
        "--path",
        "note.md",
        "--revision-id",
        revision,
    ];
    let read = json(&database, &get_arguments);
    assert_eq!(read["content"], first);
    assert_eq!(read["revision"]["content_hash"], first_hash);
    assert_eq!(before_restore, history(&database, &source));

    let mut restore_arguments = get_arguments;
    restore_arguments[1] = "restore";
    let mut stale_restore = restore_arguments.to_vec();
    stale_restore.extend(["--expected-hash", first_hash]);
    assert!(!invoke(&database, &stale_restore).status.success());
    assert_eq!(
        std::fs::read_to_string(notes.join("note.md")).unwrap(),
        second
    );
    assert_eq!(before_restore, history(&database, &source));
    assert!(!invoke(&database, &restore_arguments).status.success());

    let mut restore = restore_arguments.to_vec();
    restore.extend(["--expected-hash", second_hash]);
    let restored = json(&database, &restore);
    assert_eq!(restored["indexed"], true);
    assert_eq!(restored["content_hash"], first_hash);
    assert_eq!(
        std::fs::read_to_string(notes.join("note.md")).unwrap(),
        first
    );
    let restored_history = history(&database, &source);
    let restored_revisions = restored_history.as_array().unwrap();
    assert_eq!(restored_revisions.len(), 3);
    assert_eq!(restored_revisions[2]["content_hash"], first_hash);
    assert_eq!(restored_revisions[2]["previous_hash"], second_hash);
    assert_ne!(restored_revisions[2]["id"], restored_revisions[0]["id"]);

    std::fs::write(&input, third).unwrap();
    json(&database, &update_arguments);
    let complete_history = history(&database, &source);
    assert_eq!(complete_history.as_array().unwrap().len(), 4);
    assert_eq!(
        &complete_history.as_array().unwrap()[..3],
        restored_revisions
    );
    let manifest = temp.path().join("owner.json");
    run(
        &database,
        &["config", "export", "--output", manifest.to_str().unwrap()],
    );
    std::fs::remove_file(&database).unwrap();
    run(
        &database,
        &["rebuild", "--config", manifest.to_str().unwrap()],
    );
    let new_source = source_id(&database, "writer");
    assert_eq!(complete_history, history(&database, &new_source));
    let mut read_after_rebuild = get_arguments;
    read_after_rebuild[5] = &new_source;
    assert_eq!(json(&database, &read_after_rebuild)["content"], first);
    assert_eq!(
        std::fs::read_to_string(notes.join("note.md")).unwrap(),
        third
    );
}

#[test]
fn history_commands_enforce_source_access_and_keep_read_only_readers_read_only() {
    let temp = tempfile::tempdir().unwrap();
    let notes = temp.path().join("notes");
    std::fs::create_dir(&notes).unwrap();
    let database = temp.path().join("index.sqlite");
    for project in ["writer", "reader", "outsider"] {
        run(&database, &["project", "create", project]);
    }
    run(
        &database,
        &[
            "source",
            "add",
            "--project",
            "writer",
            "--write",
            notes.to_str().unwrap(),
        ],
    );
    run(
        &database,
        &[
            "source",
            "add",
            "--project",
            "reader",
            notes.to_str().unwrap(),
        ],
    );
    let source = source_id(&database, "writer");
    let input = temp.path().join("content.md");
    let content = "# Scoped history\nprivatehistorymarker\n";
    std::fs::write(&input, content).unwrap();
    let saved = json(
        &database,
        &[
            "write",
            "--project",
            "writer",
            "--source-id",
            &source,
            "--path",
            "note.md",
            "--content-file",
            input.to_str().unwrap(),
        ],
    );
    let hash = saved["content_hash"].as_str().unwrap();
    let listed = history(&database, &source);
    let revision = listed[0]["id"].as_str().unwrap();
    let reader_get = [
        "history",
        "get",
        "--project",
        "reader",
        "--source-id",
        &source,
        "--path",
        "note.md",
        "--revision-id",
        revision,
    ];
    assert_eq!(json(&database, &reader_get)["content"], content);
    let mut outsider_get = reader_get;
    outsider_get[3] = "outsider";
    let denied = invoke(&database, &outsider_get);
    assert!(!denied.status.success());
    assert!(!String::from_utf8_lossy(&denied.stderr).contains("privatehistorymarker"));
    let mut wrong_path = reader_get;
    wrong_path[7] = "another.md";
    assert!(!invoke(&database, &wrong_path).status.success());
    let mut reader_restore = reader_get.to_vec();
    reader_restore[1] = "restore";
    reader_restore.extend(["--expected-hash", hash]);
    assert!(!invoke(&database, &reader_restore).status.success());
    assert_eq!(
        std::fs::read_to_string(notes.join("note.md")).unwrap(),
        content
    );
    assert_eq!(listed, history(&database, &source));
}
