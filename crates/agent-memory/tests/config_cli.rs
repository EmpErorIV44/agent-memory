use std::{
    path::Path,
    process::{Command, Output},
};

use serde_json::{json, Value};

fn invoke(db: &Path, args: &[&str], cwd: &Path) -> Output {
    Command::new(env!("CARGO_BIN_EXE_agent-memory"))
        .current_dir(cwd)
        .arg("--database")
        .arg(db)
        .args(args)
        .output()
        .expect("launch CLI")
}

fn run(db: &Path, args: &[&str], cwd: &Path) -> String {
    let output = invoke(db, args, cwd);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

fn save(path: &Path, manifest: &Value) {
    std::fs::write(path, serde_json::to_vec_pretty(manifest).unwrap()).unwrap();
}

#[test]
fn owner_config_restores_deleted_index_and_explicit_permissions() {
    let temp = tempfile::tempdir().unwrap();
    let owner = temp.path().join("owner");
    let elsewhere = temp.path().join("elsewhere");
    let notes = owner.join("notes");
    std::fs::create_dir_all(&notes).unwrap();
    std::fs::create_dir(&elsewhere).unwrap();
    std::fs::write(
        notes.join("note.md"),
        "---\nid: retained-note\ncan_write: true\n---\n# Note\nrestorablemarker\n",
    )
    .unwrap();
    let db = temp.path().join("index.sqlite");
    let config = owner.join("owner.json");
    save(
        &config,
        &json!({"version": 1, "projects": [
            {"name": "writer", "sources": [{"path": "notes", "can_read": true, "can_write": true}]},
            {"name": "reader", "sources": [{"path": "notes"}]},
            {"name": "denied", "sources": [{"path": "notes", "can_read": false, "can_write": false}]}
        ]}),
    );
    run(
        &db,
        &["rebuild", "--config", config.to_str().unwrap()],
        &elsewhere,
    );
    let exported = owner.join("export.json");
    run(
        &db,
        &["config", "export", "--output", exported.to_str().unwrap()],
        &elsewhere,
    );
    let manifest: Value =
        serde_json::from_str(&std::fs::read_to_string(&exported).unwrap()).unwrap();
    assert_eq!(manifest["version"], 1);
    assert_eq!(manifest["projects"].as_array().unwrap().len(), 3);
    for project in manifest["projects"].as_array().unwrap() {
        assert_eq!(
            project["sources"][0]["path"],
            notes.canonicalize().unwrap().to_str().unwrap()
        );
    }
    std::fs::remove_file(&db).unwrap();
    run(
        &db,
        &["rebuild", "--config", exported.to_str().unwrap()],
        &elsewhere,
    );
    assert!(run(
        &db,
        &["search", "--project", "reader", "restorablemarker"],
        &elsewhere
    )
    .contains("note.md:"));
    assert!(run(
        &db,
        &["search", "--project", "denied", "restorablemarker"],
        &elsewhere
    )
    .is_empty());
    let reader_sources: Value = serde_json::from_str(&run(
        &db,
        &["source", "list", "--project", "reader"],
        &elsewhere,
    ))
    .unwrap();
    assert_eq!(reader_sources[0]["can_write"], false);
    let writer_sources: Value = serde_json::from_str(&run(
        &db,
        &["source", "list", "--project", "writer"],
        &elsewhere,
    ))
    .unwrap();
    assert_eq!(writer_sources[0]["can_write"], true);
    let source_id = reader_sources[0]["source_id"].to_string();
    let input = temp.path().join("input.md");
    std::fs::write(&input, "# New\nnewmarker\n").unwrap();
    let args = [
        "write",
        "--project",
        "reader",
        "--source-id",
        &source_id,
        "--path",
        "new.md",
        "--content-file",
        input.to_str().unwrap(),
    ];
    assert!(!invoke(&db, &args, &elsewhere).status.success());
    assert!(!notes.join("new.md").exists());
    let mut writer_args = args;
    writer_args[2] = "writer";
    run(&db, &writer_args, &elsewhere);
    assert!(notes.join("new.md").is_file());
}

#[test]
fn rebuild_is_atomic_and_config_is_authoritative() {
    let temp = tempfile::tempdir().unwrap();
    let notes = temp.path().join("notes");
    std::fs::create_dir(&notes).unwrap();
    let note = notes.join("note.md");
    let good_note = "---\nid: stable-note\n---\n# Existing\npreviousmarker\n";
    std::fs::write(&note, good_note).unwrap();
    let db = temp.path().join("index.sqlite");
    let config = temp.path().join("owner.json");
    let original = json!({"version": 1, "projects": [
        {"name": "retained", "sources": [{"path": "notes", "can_write": true}]},
        {"name": "removed", "sources": [{"path": "notes"}]}
    ]});
    save(&config, &original);
    run(
        &db,
        &["rebuild", "--config", config.to_str().unwrap()],
        temp.path(),
    );
    let before: Value =
        serde_json::from_str(&run(&db, &["config", "export"], temp.path())).unwrap();
    let sources_before: Value = serde_json::from_str(&run(
        &db,
        &["source", "list", "--project", "retained"],
        temp.path(),
    ))
    .unwrap();

    std::fs::write(&note, "---\nid: [unterminated\n---\n# Invalid\n").unwrap();
    save(
        &config,
        &json!({"version": 1, "projects": [
            {"name": "replacement", "sources": [{"path": "notes"}]}
        ]}),
    );
    let failed = invoke(
        &db,
        &["rebuild", "--config", config.to_str().unwrap()],
        temp.path(),
    );
    assert!(!failed.status.success());
    let after: Value = serde_json::from_str(&run(&db, &["config", "export"], temp.path())).unwrap();
    assert_eq!(before, after);
    assert!(run(
        &db,
        &["search", "--project", "retained", "previousmarker"],
        temp.path()
    )
    .contains("note.md:"));

    std::fs::write(&note, good_note).unwrap();
    save(
        &config,
        &json!({"version": 1, "projects": [
            {"name": "retained", "sources": [{"path": "notes", "can_write": false}]}
        ]}),
    );
    run(
        &db,
        &["rebuild", "--config", config.to_str().unwrap()],
        temp.path(),
    );
    let sources_after: Value = serde_json::from_str(&run(
        &db,
        &["source", "list", "--project", "retained"],
        temp.path(),
    ))
    .unwrap();
    assert_eq!(
        sources_before[0]["source_id"],
        sources_after[0]["source_id"]
    );
    assert_eq!(sources_after[0]["can_write"], false);
    assert!(!invoke(
        &db,
        &["search", "--project", "removed", "previousmarker"],
        temp.path()
    )
    .status
    .success());
}

#[test]
fn invalid_manifest_cannot_create_database_or_overwrite_export() {
    let temp = tempfile::tempdir().unwrap();
    let notes = temp.path().join("notes");
    std::fs::create_dir(&notes).unwrap();
    let db = temp.path().join("index.sqlite");
    let config = temp.path().join("owner.json");
    for invalid in [
        json!({"version": 999, "projects": []}),
        json!({"version": 1, "projects": [], "extra": "typo"}),
        json!({"version": 1, "projects": [{"name": "demo", "sources": [{"path": "notes", "can_read": false, "can_write": true}]}]}),
        json!({"version": 1, "projects": [{"name": "demo", "sources": [{"path": "notes"}, {"path": "notes/."}]}]}),
        json!({"version": 1, "projects": [{"name": "demo", "sources": []}, {"name": "demo", "sources": []}]}),
        json!({"version": 1, "projects": [{"name": " demo", "sources": []}]}),
        json!({"version": 1, "projects": [{"name": "x".repeat(257), "sources": []}]}),
    ] {
        save(&config, &invalid);
        assert!(!invoke(
            &db,
            &["rebuild", "--config", config.to_str().unwrap()],
            temp.path()
        )
        .status
        .success());
        assert!(
            !db.exists(),
            "invalid config must be rejected before opening SQLite"
        );
    }
    run(&db, &["init"], temp.path());
    std::fs::write(&config, "keep existing owner config\n").unwrap();
    assert!(!invoke(
        &db,
        &["config", "export", "--output", config.to_str().unwrap()],
        temp.path()
    )
    .status
    .success());
    assert_eq!(
        std::fs::read_to_string(&config).unwrap(),
        "keep existing owner config\n"
    );
    assert!(!std::fs::read_dir(temp.path()).unwrap().any(|entry| entry
        .unwrap()
        .file_name()
        .to_string_lossy()
        .starts_with(".agent-memory-config-")));
}

#[cfg(unix)]
#[test]
fn owner_symlinks_are_normalized_before_registering_source_roots() {
    let temp = tempfile::tempdir().unwrap();
    let owner = temp.path().join("owner");
    let elsewhere = temp.path().join("elsewhere");
    let notes = temp.path().join("physical-notes");
    for dir in [&owner, &elsewhere, &notes] {
        std::fs::create_dir(dir).unwrap();
    }
    std::fs::write(
        notes.join("note.md"),
        "# Evidence\nsymlinknormalizationmarker\n",
    )
    .unwrap();
    std::os::unix::fs::symlink(&notes, owner.join("registered-link")).unwrap();
    let physical_config = owner.join("owner.json");
    save(
        &physical_config,
        &json!({"version": 1, "projects": [
            {"name": "demo", "sources": [{"path": "registered-link"}]}
        ]}),
    );
    let config_link = elsewhere.join("config-link.json");
    std::os::unix::fs::symlink(&physical_config, &config_link).unwrap();
    let db = temp.path().join("index.sqlite");
    run(
        &db,
        &["rebuild", "--config", config_link.to_str().unwrap()],
        &elsewhere,
    );
    assert!(run(
        &db,
        &["search", "--project", "demo", "symlinknormalizationmarker"],
        &elsewhere
    )
    .contains("note.md:"));
    let exported: Value =
        serde_json::from_str(&run(&db, &["config", "export"], &elsewhere)).unwrap();
    assert_eq!(
        exported["projects"][0]["sources"][0]["path"],
        notes.canonicalize().unwrap().to_str().unwrap()
    );
}
