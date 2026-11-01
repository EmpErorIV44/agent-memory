use anyhow::Result;
use memory_sqlite::Store;
use std::fs;

#[test]
fn preview_and_guarded_rename_fix_incoming_outgoing_and_reference_links() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let root = temp.path().join("notes");
    fs::create_dir_all(root.join("old"))?;
    fs::write(root.join("old/target.md"), "---\nid: target-id\n---\n# Target\n[Peer](../peer.md#heading) [[peer|Alias]] [[id:target-id]]\n")?;
    fs::write(root.join("peer.md"), "# Peer\n[Target](old/target.md#section) [[target|Name]] [[id:target-id]]\n[Again][ref]\n\n[ref]: old/target.md#reference\n\n`[Code](old/target.md)`\n")?;
    let mut store = Store::open(&temp.path().join("index.sqlite"))?;
    store.create_project("demo")?;
    let source = store.register_source("demo", &root, true)?;
    let preview = store.rename_preview("demo", source, "old/target.md", "new/deep/moved.md")?;
    assert_eq!(preview.edits.len(), 1);
    assert!(preview
        .moved_content
        .contains("[Peer](../../peer.md#heading)"));
    assert!(root.join("old/target.md").exists());
    assert!(!root.join("new/deep/moved.md").exists());
    let result = store.rename_apply(
        "demo",
        source,
        "old/target.md",
        "new/deep/moved.md",
        &preview.plan_hash,
    )?;
    assert!(result.complete, "{:?}", result);
    assert!(!result.partial);
    assert!(result.indexed);
    assert!(result.destination_present);
    assert!(!result.source_present);
    assert!(!root.join("old/target.md").exists());
    let peer = fs::read_to_string(root.join("peer.md"))?;
    assert!(peer.contains("[Target](new/deep/moved.md#section)"));
    assert!(peer.contains("[[new/deep/moved|Name]]"));
    assert!(peer.contains("[[id:target-id]]"));
    assert!(peer.contains("[ref]: new/deep/moved.md#reference"));
    assert!(peer.contains("`[Code](old/target.md)`"));
    let moved = fs::read_to_string(root.join("new/deep/moved.md"))?;
    assert!(moved.contains("id: target-id"));
    assert!(store.link_diagnostics("demo", None, None)?.is_empty());
    assert!(!store
        .list_revisions("demo", source, "old/target.md")?
        .is_empty());
    assert!(!store
        .list_revisions("demo", source, "new/deep/moved.md")?
        .is_empty());
    Ok(())
}

#[test]
fn rename_rejects_stale_plan_added_files_read_only_and_existing_destination() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let root = temp.path().join("notes");
    fs::create_dir(&root)?;
    fs::write(root.join("from.md"), "# From\n")?;
    let mut store = Store::open(&temp.path().join("index.sqlite"))?;
    store.create_project("demo")?;
    let source = store.register_source("demo", &root, true)?;
    let plan = store.rename_preview("demo", source, "from.md", "to.md")?;
    fs::write(root.join("new.md"), "[From](from.md)\n")?;
    assert!(store
        .rename_apply("demo", source, "from.md", "to.md", &plan.plan_hash)
        .is_err());
    let plan = store.rename_preview("demo", source, "from.md", "to.md")?;
    store.register_source("demo", &root, false)?;
    assert!(store
        .rename_apply("demo", source, "from.md", "to.md", &plan.plan_hash)
        .is_err());
    assert!(store
        .rename_preview("demo", source, "from.md", "../escape.md")
        .is_err());
    assert!(store
        .rename_preview("demo", source, "from.md", "new.md")
        .is_err());
    assert!(root.join("from.md").exists());
    assert!(!root.join("to.md").exists());
    Ok(())
}

#[test]
fn diagnostics_report_missing_ambiguous_and_retained_stale_paths() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let root = temp.path().join("notes");
    fs::create_dir_all(root.join("one"))?;
    fs::create_dir_all(root.join("two"))?;
    fs::write(
        root.join("origin.md"),
        "[[missing]] [[same]] [Target](target.md)\n",
    )?;
    fs::write(root.join("target.md"), "---\nid: stable\n---\n# Target\n")?;
    fs::write(root.join("one/same.md"), "# One\n")?;
    fs::write(root.join("two/same.md"), "# Two\n")?;
    let mut store = Store::open(&temp.path().join("index.sqlite"))?;
    store.create_project("demo")?;
    let source = store.register_source("demo", &root, true)?;
    store.index("demo")?;
    fs::rename(root.join("target.md"), root.join("renamed.md"))?;
    let problems = store.link_diagnostics("demo", Some(source), Some("origin.md"))?;
    assert_eq!(problems.len(), 3);
    for status in ["missing", "ambiguous", "stale_path"] {
        assert!(problems.iter().any(|link| link.status == status));
    }
    assert!(store
        .link_diagnostics("demo", Some(source), Some("one/same.md"))?
        .is_empty());
    Ok(())
}

#[test]
fn rename_preview_never_guesses_an_ambiguous_wiki_target() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let root = temp.path().join("notes");
    fs::create_dir_all(root.join("one"))?;
    fs::create_dir_all(root.join("two"))?;
    fs::write(root.join("origin.md"), "[[same|Ambiguous]]\n")?;
    fs::write(root.join("one/same.md"), "# One\n")?;
    fs::write(root.join("two/same.md"), "# Two\n")?;
    let mut store = Store::open(&temp.path().join("index.sqlite"))?;
    store.create_project("demo")?;
    let source = store.register_source("demo", &root, true)?;
    let plan = store.rename_preview("demo", source, "one/same.md", "one/renamed.md")?;
    assert!(plan.edits.is_empty());
    assert_eq!(
        fs::read_to_string(root.join("origin.md"))?,
        "[[same|Ambiguous]]\n"
    );
    Ok(())
}

#[cfg(unix)]
#[test]
fn rename_refuses_symlink_destination_parent() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let root = temp.path().join("notes");
    let outside = temp.path().join("outside");
    fs::create_dir(&root)?;
    fs::create_dir(&outside)?;
    fs::write(root.join("from.md"), "# From\n")?;
    std::os::unix::fs::symlink(&outside, root.join("escape"))?;
    let mut store = Store::open(&temp.path().join("index.sqlite"))?;
    store.create_project("demo")?;
    let source = store.register_source("demo", &root, true)?;
    assert!(store
        .rename_preview("demo", source, "from.md", "escape/to.md")
        .is_err());
    assert!(!outside.join("to.md").exists());
    Ok(())
}

#[cfg(unix)]
#[test]
fn repair_failure_reports_partial_move_without_repeating_publication() -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let temp = tempfile::tempdir()?;
    let root = temp.path().join("notes");
    fs::create_dir_all(root.join("locked"))?;
    fs::write(root.join("from.md"), "---\nid: stable\n---\n# From\n")?;
    fs::write(root.join("locked/ref.md"), "[From](../from.md)\n")?;
    let mut store = Store::open(&temp.path().join("index.sqlite"))?;
    store.create_project("demo")?;
    let source = store.register_source("demo", &root, true)?;
    let plan = store.rename_preview("demo", source, "from.md", "moved/to.md")?;
    fs::set_permissions(root.join("locked"), fs::Permissions::from_mode(0o555))?;
    let attempted = store.rename_apply("demo", source, "from.md", "moved/to.md", &plan.plan_hash);
    fs::set_permissions(root.join("locked"), fs::Permissions::from_mode(0o755))?;
    let result = attempted?;
    assert!(!result.complete);
    assert!(result.partial);
    assert!(result.moved);
    assert!(result.destination_present);
    assert!(!result.source_present);
    assert_eq!(result.applied, vec!["moved/to.md"]);
    assert!(result.error.is_some());
    assert_eq!(
        fs::read_to_string(root.join("locked/ref.md"))?,
        "[From](../from.md)\n"
    );
    assert!(store
        .link_diagnostics("demo", Some(source), Some("locked/ref.md"))?
        .iter()
        .any(|link| link.status == "stale_path"));
    assert!(store
        .rename_apply("demo", source, "from.md", "moved/to.md", &plan.plan_hash)
        .is_err());
    Ok(())
}
