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
