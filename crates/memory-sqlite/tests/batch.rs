use anyhow::Result;
use memory_sqlite::{BatchEdit, Store};
use std::fs;

fn edit(path: &str, content: &str, previous: Option<&str>) -> BatchEdit {
    BatchEdit {
        path: path.into(),
        content: content.into(),
        expected_hash: previous.map(|s| memory_core::content_hash(s.as_bytes())),
    }
}
fn note(id: &str, status: &str) -> String {
    format!("---\nid: doc-{id}\nmemory:\n  id: {id}\n  project: demo\n  kind: decision\n  status: {status}\n  summary: Decision {id}\n---\n# Decision\n{id}\n")
}

#[test]
fn preview_apply_history_id_validation_and_no_overwrite() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let root = temp.path().join("notes");
    fs::create_dir(&root)?;
    let old = note("old", "accepted");
    fs::write(root.join("old.md"), &old)?;
    let mut store = Store::open(&temp.path().join("db"))?;
    store.create_project("demo")?;
    let source = store.register_source("demo", &root, true)?;
    store.index("demo")?;
    let edits = vec![
        edit("old.md", &note("old", "superseded"), Some(&old)),
        edit("new.md", &note("new", "accepted"), None),
    ];
    let plan = store.batch_preview("demo", source, edits.clone())?;
    assert!(!root.join(".agent-memory-operations").exists());
    assert!(!root.join("new.md").exists());
    assert!(store
        .batch_apply("demo", source, edits.clone(), "wrong")
        .is_err());
    let result = store.batch_apply("demo", source, edits.clone(), &plan.plan_hash)?;
    assert!(
        result.complete && result.indexed && !result.partial,
        "{result:?}"
    );
    assert_eq!(result.applied, vec!["new.md", "old.md"]);
    assert_eq!(
        store.get_memory("demo", "old")?.unwrap().memory.status,
        memory_core::MemoryStatus::Superseded
    );
    assert!(memory_core::list_revisions(&root, "old.md")?.len() >= 2);
    assert_eq!(
        store.batch_operations("demo", source)?["operations"][0]["complete"],
        true
    );
    assert!(store.rename_operations("demo", source)?.is_empty());
    // Completed receipts never overwrite later edits.
    fs::write(root.join("new.md"), "# Later edit\n")?;
    assert!(
        store
            .batch_resume("demo", source, &result.operation_id)?
            .complete
    );
    assert_eq!(fs::read_to_string(root.join("new.md"))?, "# Later edit\n");
    assert!(store
        .batch_apply("demo", source, edits, &plan.plan_hash)
        .is_err());
    Ok(())
}

#[test]
fn all_known_conflicts_and_permissions_fail_before_publication() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let root = temp.path().join("notes");
    fs::create_dir(&root)?;
    fs::write(root.join("last.md"), "# External\n")?;
    let db = temp.path().join("db");
    let mut store = Store::open(&db)?;
    store.create_project("demo")?;
    store.create_project("foreign")?;
    let source = store.register_source("demo", &root, true)?;
    let invalid = vec![
        vec![],
        vec![
            edit("first.md", "# First", None),
            edit("last.md", "# Last", Some("# Stale")),
        ],
        vec![
            edit("first.md", "# First", None),
            edit("../bad.md", "# Bad", None),
        ],
        vec![edit("dup.md", "# One", None), edit("DUP.md", "# Two", None)],
        vec![
            edit("folder.md", "# Parent", None),
            edit("folder.md/note.md", "# Child", None),
        ],
        vec![
            edit("a.md", &note("same", "accepted"), None),
            edit("b.md", &note("same", "accepted"), None),
        ],
        vec![edit(
            "foreign.md",
            &note("x", "accepted").replace("project: demo", "project: foreign"),
            None,
        )],
        vec![edit("broken.md", "---\nmemory: [wrong]\n---", None)],
    ];
    for edits in invalid {
        assert!(store.batch_preview("demo", source, edits).is_err());
    }
    // A writable shared source does not authorize replacing foreign structured
    // knowledge with ordinary Markdown (new content alone cannot reveal this).
    let foreign = note("foreign", "accepted").replace("project: demo", "project: foreign");
    fs::write(root.join("foreign-existing.md"), &foreign)?;
    assert!(store
        .batch_preview(
            "demo",
            source,
            vec![edit("foreign-existing.md", "# Replacement", Some(&foreign))]
        )
        .is_err());
    assert!(!root.join("first.md").exists());
    assert!(!root.join(".agent-memory-operations").exists());
    assert!(store
        .batch_preview("foreign", source, vec![edit("new.md", "# New", None)])
        .is_err());
    let edits = vec![edit("new.md", "# New", None)];
    let preview = store.batch_preview("demo", source, edits.clone())?;
    store.register_source("demo", &root, false)?;
    assert!(store
        .batch_apply("demo", source, edits, &preview.plan_hash)
        .is_err());
    assert!(!root.join("new.md").exists());
    Ok(())
}

#[test]
fn source_lock_serializes_old_and_new_writers_and_refuses_symlinks() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let root = temp.path().join("notes");
    fs::create_dir(&root)?;
    let mut store = Store::open(&temp.path().join("db"))?;
    store.create_project("demo")?;
    let source = store.register_source("demo", &root, true)?;
    let edits = vec![edit("a.md", "# A", None), edit("b.md", "# B", None)];
    let plan = store.batch_preview("demo", source, edits.clone())?;
    let writer = memory_core::SourceWriter::open(&root)?;
    assert!(store
        .batch_apply("demo", source, edits.clone(), &plan.plan_hash)
        .is_err());
    assert!(memory_core::write_markdown(
        &root,
        "single.md",
        "# Single",
        memory_core::WriteMode::Create
    )
    .is_err());
    drop(writer);
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(temp.path(), root.join("link"))?;
        assert!(store
            .batch_preview("demo", source, vec![edit("link/out.md", "# Escape", None)])
            .is_err());
    }
    assert!(
        store
            .batch_apply("demo", source, edits, &plan.plan_hash)?
            .complete
    );
    Ok(())
}
