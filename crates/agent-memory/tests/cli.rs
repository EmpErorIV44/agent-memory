use std::{path::Path, process::Command};

fn run(db: &Path, args: &[&str]) -> String {
    let output = Command::new(env!("CARGO_BIN_EXE_agent-memory"))
        .arg("--database")
        .arg(db)
        .args(args)
        .output()
        .expect("launch CLI");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

#[test]
fn markdown_to_cli_search_is_scoped() {
    let temp = tempfile::tempdir().unwrap();
    let db = temp.path().join("index.sqlite");
    let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures");
    run(&db, &["init"]);
    for project in ["banking", "storefront"] {
        run(&db, &["project", "create", project]);
        run(
            &db,
            &[
                "source",
                "add",
                "--project",
                project,
                fixtures.join(project).to_str().unwrap(),
            ],
        );
        assert!(run(&db, &["index", "--project", project])
            .contains("added=3 updated=0 unchanged=0 deleted=0"));
    }
    let result = run(&db, &["search", "--project", "banking", "OAuth2"]);
    assert!(result.contains("authentication.md:"));
    assert!(result.contains("bankauthmarker"));
    assert!(!result.contains("shopauthmarker"));
    assert!(run(&db, &["search", "--project", "banking", "shopauthmarker"]).is_empty());
    let result = run(&db, &["search", "--project", "storefront", "OAuth2"]);
    assert!(result.contains("shopauthmarker"));
    assert!(!result.contains("bankauthmarker"));
}

#[test]
fn writable_source_and_cli_create_update() {
    let temp = tempfile::tempdir().unwrap();
    let db = temp.path().join("index.sqlite");
    let notes = temp.path().join("notes");
    std::fs::create_dir(&notes).unwrap();
    let input = temp.path().join("input.md");
    std::fs::write(&input, "# First\ncliwritemarker\n").unwrap();
    run(&db, &["project", "create", "demo"]);
    run(
        &db,
        &[
            "source",
            "add",
            "--project",
            "demo",
            "--write",
            notes.to_str().unwrap(),
        ],
    );
    let sources: serde_json::Value =
        serde_json::from_str(&run(&db, &["source", "list", "--project", "demo"])).unwrap();
    assert_eq!(sources[0]["can_write"], true);
    let source_id = sources[0]["source_id"].to_string();
    let args = [
        "write",
        "--project",
        "demo",
        "--source-id",
        &source_id,
        "--path",
        "note.md",
        "--content-file",
        input.to_str().unwrap(),
    ];
    let saved: serde_json::Value = serde_json::from_str(&run(&db, &args)).unwrap();
    assert_eq!(saved["indexed"], true);
    assert!(run(&db, &["search", "--project", "demo", "cliwritemarker"]).contains("note.md:"));
    std::fs::write(&input, "# Updated\ncliupdatedmarker\n").unwrap();
    let mut args = args.to_vec();
    args.extend(["--expected-hash", saved["content_hash"].as_str().unwrap()]);
    run(&db, &args);
    assert_eq!(
        std::fs::read_to_string(notes.join("note.md")).unwrap(),
        "# Updated\ncliupdatedmarker\n"
    );
}
