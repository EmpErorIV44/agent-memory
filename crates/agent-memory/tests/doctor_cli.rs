use memory_sqlite::Store;
use serde_json::Value;
use std::{fs, path::Path, process::Command};

fn doctor(db: &Path, project: &str) -> (bool, Value) {
    let output = Command::new(env!("CARGO_BIN_EXE_agent-memory"))
        .arg("--database")
        .arg(db)
        .args(["doctor", "--project", project])
        .output()
        .unwrap();
    let report = serde_json::from_slice(&output.stdout).unwrap_or_else(|error| {
        panic!(
            "{error}: stdout={} stderr={}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )
    });
    (output.status.success(), report)
}

#[test]
fn doctor_never_creates_or_migrates_database() {
    let temp = tempfile::tempdir().unwrap();
    let missing = temp.path().join("missing.sqlite");
    let (ok, report) = doctor(&missing, "demo");
    assert!(!ok);
    assert_eq!(report["healthy"], false);
    assert!(!missing.exists());
    let old = temp.path().join("old.sqlite");
    let connection = rusqlite::Connection::open(&old).unwrap();
    connection.execute_batch("PRAGMA user_version=3;").unwrap();
    drop(connection);
    let before = fs::read(&old).unwrap();
    assert!(!doctor(&old, "demo").0);
    assert_eq!(fs::read(&old).unwrap(), before);
    let corrupt = temp.path().join("corrupt.sqlite");
    fs::write(&corrupt, b"not SQLite").unwrap();
    assert!(!doctor(&corrupt, "demo").0);
    let incomplete = temp.path().join("incomplete.sqlite");
    let connection = rusqlite::Connection::open(&incomplete).unwrap();
    connection.execute_batch("PRAGMA user_version=4; CREATE TABLE projects(id INTEGER PRIMARY KEY, name TEXT); INSERT INTO projects VALUES(1,'demo');").unwrap();
    drop(connection);
    let (ok, report) = doctor(&incomplete, "demo");
    assert!(!ok);
    assert!(report["checks"]
        .as_array()
        .unwrap()
        .iter()
        .any(|check| check["name"] == "sources" && check["status"] == "error"));
}

#[test]
fn doctor_checks_registered_sources_without_indexing_or_using_remote() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("notes");
    fs::create_dir(&root).unwrap();
    fs::write(root.join("note.md"), "# Note\ncontent\n").unwrap();
    let db = temp.path().join("index.sqlite");
    {
        let store = Store::open(&db).unwrap();
        store.create_project("demo").unwrap();
        store.register_source("demo", &root, true).unwrap();
    }
    let before = fs::read(&db).unwrap();
    let (ok, report) = doctor(&db, "demo");
    assert!(ok, "{report}");
    assert_eq!(report["healthy"], true);
    assert_eq!(fs::read(&db).unwrap(), before);
    let connection = rusqlite::Connection::open(&db).unwrap();
    assert_eq!(
        connection
            .query_row("SELECT count(*) FROM documents", [], |row| row
                .get::<_, i64>(0))
            .unwrap(),
        0
    );
    drop(connection);
    assert!(!doctor(&db, "absent").0);
    fs::write(root.join("first.md"), "---\nid: duplicate\n---\n# First\n").unwrap();
    fs::write(
        root.join("second.md"),
        "---\nid: duplicate\n---\n# Second\n",
    )
    .unwrap();
    let (ok, report) = doctor(&db, "demo");
    assert!(!ok);
    assert!(report["checks"]
        .as_array()
        .unwrap()
        .iter()
        .any(|check| check["name"] == "source_identifiers" && check["status"] == "error"));
    fs::remove_file(root.join("second.md")).unwrap();
    fs::write(root.join("invalid.md"), [0xff]).unwrap();
    assert!(!doctor(&db, "demo").0);
    fs::remove_dir_all(&root).unwrap();
    assert!(!doctor(&db, "demo").0);
}

#[test]
fn doctor_reports_git_configuration_warnings_without_network() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("notes");
    fs::create_dir(&root).unwrap();
    assert!(Command::new("git")
        .arg("-C")
        .arg(&root)
        .args(["init", "-q"])
        .status()
        .unwrap()
        .success());
    assert!(Command::new("git")
        .arg("-C")
        .arg(&root)
        .args([
            "remote",
            "add",
            "origin",
            "https://secret:password@example.invalid/never-contact.git"
        ])
        .status()
        .unwrap()
        .success());
    fs::write(root.join("note.md"), "# Note\n").unwrap();
    let db = temp.path().join("index.sqlite");
    let store = Store::open(&db).unwrap();
    store.create_project("demo").unwrap();
    store.add_source("demo", &root).unwrap();
    drop(store);
    let (ok, report) = doctor(&db, "demo");
    assert!(ok, "{report}");
    assert!(report["checks"]
        .as_array()
        .unwrap()
        .iter()
        .any(|check| check["name"] == "git_upstream" && check["status"] == "warning"));
    assert!(!report.to_string().contains("password"));
    assert!(!report.to_string().contains("example.invalid"));
}
