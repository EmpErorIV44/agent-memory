use anyhow::Result;
use memory_sqlite::Store;
use std::fs;

#[test]
fn links_resolve_and_follow_stable_identity_rename_but_not_deletion() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let root = temp.path().join("notes");
    fs::create_dir_all(root.join("decisions"))?;
    fs::write(root.join("overview.md"), "# Overview\n[Decision](decisions/plan.md#section) and [[plan|Plan]].\n[[missing]]\n[[id:plan-id]]\n")?;
    fs::write(
        root.join("decisions/plan.md"),
        "---\nid: plan-id\n---\n# Plan\n",
    )?;
    let db = temp.path().join("index.sqlite");
    let mut store = Store::open(&db)?;
    store.create_project("demo")?;
    let source = store.register_source("demo", &root, true)?;
    store.index("demo")?;
    let links = store
        .document_links("demo", source, "overview.md")?
        .unwrap();
    assert_eq!(links.outgoing.len(), 4);
    assert_eq!(
        links
            .outgoing
            .iter()
            .filter(|link| link.status == "resolved")
            .count(),
        3
    );
    assert_eq!(
        store
            .document_links("demo", source, "decisions/plan.md")?
            .unwrap()
            .incoming
            .len(),
        3
    );
    fs::rename(
        root.join("decisions/plan.md"),
        root.join("decisions/renamed.md"),
    )?;
    store.index("demo")?;
    let links = store
        .document_links("demo", source, "overview.md")?
        .unwrap();
    assert_eq!(
        links
            .outgoing
            .iter()
            .filter(|link| link.status == "resolved")
            .count(),
        3
    );
    for link in links
        .outgoing
        .iter()
        .filter(|link| link.status == "resolved")
    {
        assert_eq!(
            link.target_document.as_ref().unwrap().path,
            "decisions/renamed.md"
        );
    }
    fs::remove_file(root.join("decisions/renamed.md"))?;
    store.index("demo")?;
    let links = store
        .document_links("demo", source, "overview.md")?
        .unwrap();
    assert!(links
        .outgoing
        .iter()
        .all(|link| link.status == "missing" && link.target_document.is_none()));
    Ok(())
}

#[test]
fn links_ambiguity_rebuild_permissions_and_source_isolation() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let root = temp.path().join("notes");
    fs::create_dir_all(root.join("a"))?;
    fs::create_dir_all(root.join("b"))?;
    fs::write(root.join("origin.md"), "# Origin\n[[duplicate]] [safe](a/duplicate.md) [escape](../private/secret.md) [[secret]]\n")?;
    fs::write(root.join("a/duplicate.md"), "# A\n")?;
    fs::write(root.join("b/duplicate.md"), "# B\n")?;
    let private = temp.path().join("private");
    fs::create_dir(&private)?;
    fs::write(private.join("secret.md"), "# Secret\n")?;
    let db = temp.path().join("index.sqlite");
    let mut store = Store::open(&db)?;
    store.create_project("demo")?;
    store.create_project("foreign")?;
    let source = store.register_source("demo", &root, true)?;
    let hidden = store.register_source("foreign", &private, false)?;
    store.index("foreign")?;
    store.index("demo")?;
    let links = store.document_links("demo", source, "origin.md")?.unwrap();
    assert_eq!(links.outgoing[0].status, "ambiguous");
    assert_eq!(links.outgoing[1].status, "resolved");
    assert!(links.outgoing[2..]
        .iter()
        .all(|link| link.status == "missing" && link.target_document.is_none()));
    assert!(store
        .document_links("foreign", source, "origin.md")?
        .is_none());
    assert!(store.document_links("demo", hidden, "secret.md")?.is_none());
    let config = store.export_config()?;
    let rebuilt = temp.path().join("rebuilt.sqlite");
    let mut fresh = Store::open(&rebuilt)?;
    fresh.rebuild_config(&config)?;
    let rebuilt_source = fresh.sources("demo")?[0].source_id;
    assert_eq!(
        fresh
            .document_links("demo", rebuilt_source, "a/duplicate.md")?
            .unwrap()
            .incoming
            .len(),
        1
    );
    let connection = rusqlite::Connection::open(&db)?;
    connection.execute(
        "UPDATE project_sources SET can_read=0 WHERE source_id=?1",
        [source],
    )?;
    assert!(store.document_links("demo", source, "origin.md")?.is_none());
    Ok(())
}

#[test]
fn version_three_links_backfill_on_unchanged_refresh_and_roll_back_failed_scan() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let root = temp.path().join("notes");
    fs::create_dir(&root)?;
    fs::write(root.join("origin.md"), "# Origin\n[target](target.md)\n")?;
    fs::write(root.join("target.md"), "# Target\n")?;
    let db = temp.path().join("index.sqlite");
    let mut store = Store::open(&db)?;
    store.create_project("demo")?;
    let source = store.register_source("demo", &root, true)?;
    store.index("demo")?;
    drop(store);
    let connection = rusqlite::Connection::open(&db)?;
    connection.execute_batch("DROP TABLE document_links; PRAGMA user_version=3;")?;
    drop(connection);
    let mut store = Store::open(&db)?;
    assert!(store
        .document_links("demo", source, "origin.md")?
        .unwrap()
        .outgoing
        .is_empty());
    assert_eq!(store.index("demo")?.unchanged, 2);
    assert_eq!(
        store
            .document_links("demo", source, "target.md")?
            .unwrap()
            .incoming
            .len(),
        1
    );
    fs::write(root.join("invalid.md"), b"\xff")?;
    fs::write(root.join("origin.md"), "# No link\n")?;
    assert!(store.index("demo").is_err());
    assert_eq!(
        store
            .document_links("demo", source, "target.md")?
            .unwrap()
            .incoming
            .len(),
        1
    );
    Ok(())
}

#[test]
fn percent_encoded_targets_and_self_links_have_bidirectional_coordinates() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let root = temp.path().join("notes");
    fs::create_dir(&root)?;
    fs::write(
        root.join("origin.md"),
        "# Origin\n[self](origin.md#heading) [spaced](space%20name.md#heading)\n",
    )?;
    fs::write(root.join("space name.md"), "# Heading\n")?;
    let mut store = Store::open(&temp.path().join("index.sqlite"))?;
    store.create_project("demo")?;
    let source = store.register_source("demo", &root, true)?;
    store.index("demo")?;
    let links = store.document_links("demo", source, "origin.md")?.unwrap();
    assert_eq!(links.outgoing.len(), 2);
    assert_eq!(links.incoming.len(), 1);
    assert_eq!(links.incoming[0].source.path, "origin.md");
    assert_eq!(
        links.outgoing[1].target_document.as_ref().unwrap().path,
        "space name.md"
    );
    assert_eq!(links.outgoing[1].start_line, 2);
    Ok(())
}
