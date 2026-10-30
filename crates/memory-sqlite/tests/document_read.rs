use memory_sqlite::Store;

#[test]
fn direct_document_read_checks_disk_hash_and_revoked_permissions() -> anyhow::Result<()> {
    let temp = tempfile::tempdir()?;
    let source = temp.path().join("notes");
    std::fs::create_dir(&source)?;
    std::fs::write(source.join("note.md"), "# Note\noriginal\n")?;
    let database = temp.path().join("index.sqlite");
    let mut store = Store::open(&database)?;
    store.create_project("demo")?;
    let source_id = store.register_source("demo", &source, false)?;
    store.index("demo")?;
    let hit = store.search("demo", "original")?.remove(0);
    assert!(store
        .get_document_by_path("demo", source_id, "note.md", &hit.content_hash)?
        .is_some());
    std::fs::write(source.join("note.md"), "# Note\nchanged\n")?;
    assert!(store
        .get_document_by_path("demo", source_id, "note.md", &hit.content_hash)
        .is_err());
    std::fs::write(source.join("note.md"), "# Note\noriginal\n")?;
    #[cfg(unix)]
    {
        let outside = temp.path().join("outside.md");
        std::fs::write(&outside, "# Note\noriginal\n")?;
        std::fs::remove_file(source.join("note.md"))?;
        std::os::unix::fs::symlink(outside, source.join("note.md"))?;
        assert!(store
            .get_document_by_path("demo", source_id, "note.md", &hit.content_hash)
            .is_err());
    }
    let connection = rusqlite::Connection::open(&database)?;
    connection.execute("UPDATE project_sources SET can_read=0", [])?;
    assert!(store
        .get_document_by_path("demo", source_id, "note.md", &hit.content_hash)?
        .is_none());
    assert!(store
        .get_document_by_path("demo", source_id, "../outside.md", &hit.content_hash)?
        .is_none());
    Ok(())
}
