use memory_core::{current_date_utc, parse_metadata, prepare_note_metadata, validate_note_date};

#[test]
fn calendar_dates_tags_and_legacy_notes_are_strictly_validated() {
    for day in ["0001-01-01", "2000-02-29", "2024-02-29", "9999-12-31"] {
        validate_note_date(day).unwrap();
    }
    for day in [
        "0000-01-01",
        "1900-02-29",
        "2026-02-29",
        "2026-04-31",
        "2026-00-01",
        "2026-01-00",
        "2026-13-01",
        "2026-1-01",
        " 2026-01-01",
        "2026-01-01T00:00:00Z",
        "٢٠٢٦-01-01",
    ] {
        assert!(validate_note_date(day).is_err(), "accepted {day}");
    }
    let metadata = parse_metadata("---\ncreated: 2024-02-29\nupdated: '2026-11-02'\ntags: [rust, 'русский тег']\ncustom: {nested: true}\n---\n# Note\n").unwrap();
    assert_eq!(metadata.created.as_deref(), Some("2024-02-29"));
    assert_eq!(metadata.updated.as_deref(), Some("2026-11-02"));
    assert_eq!(metadata.tags, ["rust", "русский тег"]);
    let legacy = parse_metadata("# Untimed\n").unwrap();
    assert!(legacy.created.is_none() && legacy.updated.is_none() && legacy.tags.is_empty());
    for fields in [
        "created: 2026-11-03\nupdated: 2026-11-02",
        "created: null",
        "updated: 20261102",
        "tags: [rust, rust]",
        "tags: [rust, 123]",
        "tags: [true]",
        "tags: [null]",
        "tags: null",
        "tags: rust",
        "tags: ['']",
        "tags: [' padded']",
        "tags: [\"bad\\nvalue\"]",
    ] {
        assert!(
            parse_metadata(&format!("---\n{fields}\n---\n# Body\n")).is_err(),
            "accepted {fields}"
        );
    }
    assert!(parse_metadata(&format!("---\ntags: ['{}']\n---\n", "x".repeat(129))).is_err());
    let many = (0..65)
        .map(|i| format!("tag{i}"))
        .collect::<Vec<_>>()
        .join(", ");
    assert!(parse_metadata(&format!("---\ntags: [{many}]\n---\n")).is_err());
    validate_note_date(&current_date_utc().unwrap()).unwrap();
}

#[test]
fn explicit_stamping_preserves_author_dates_unknown_values_and_markdown_body() {
    let plain = "# New\nBody `unchanged`.\n";
    let stamped = prepare_note_metadata(plain, None, "2026-11-02").unwrap();
    let metadata = parse_metadata(&stamped).unwrap();
    assert_eq!(metadata.created.as_deref(), Some("2026-11-02"));
    assert_eq!(metadata.updated.as_deref(), Some("2026-11-02"));
    assert!(stamped.ends_with(plain));

    let authored = "\u{feff}---\r\ncreated: '2020-01-01'\r\nupdated: 2024-02-29\r\ntags: [rust, memory]\r\ncustom: {nested: [one, two]}\r\n...\r\n# Body\r\n\r\n    code\r\n";
    let result = prepare_note_metadata(authored, None, "2026-11-02").unwrap();
    assert!(result.starts_with("\u{feff}---\r\n"));
    assert!(result.ends_with("# Body\r\n\r\n    code\r\n"));
    let metadata = parse_metadata(&result).unwrap();
    assert_eq!(metadata.created.as_deref(), Some("2020-01-01"));
    assert_eq!(metadata.updated.as_deref(), Some("2024-02-29"));
    assert_eq!(metadata.tags, ["rust", "memory"]);
    assert!(result.contains("custom:") && result.contains("nested:") && result.contains("- two"));

    let imported =
        prepare_note_metadata("---\nupdated: 2020-01-01\n---\n# Old\n", None, "2026-11-02")
            .unwrap();
    assert_eq!(
        parse_metadata(&imported).unwrap().created.as_deref(),
        Some("2020-01-01")
    );
    assert!(prepare_note_metadata(
        "---\ncreated: 2027-01-01\n---\n# Future\n",
        None,
        "2026-11-02"
    )
    .is_err());
}

#[test]
fn updates_preserve_creation_and_noops_do_not_rewrite_bytes_or_infer_old_dates() {
    let old = "---\ncreated: '2020-01-01' # authored\nupdated: '2020-02-01'\ncustom: {a: 1}\n---\n# Original\n";
    assert_eq!(
        prepare_note_metadata(old, Some(old), "2026-11-02").unwrap(),
        old
    );
    let only_updated = old.replace("2020-02-01", "2026-11-02");
    assert_eq!(
        prepare_note_metadata(&only_updated, Some(old), "2026-11-02").unwrap(),
        old
    );
    let next = "---\ncustom: {a: 1}\n---\n# Changed\n";
    let stamped = prepare_note_metadata(next, Some(old), "2026-11-02").unwrap();
    let metadata = parse_metadata(&stamped).unwrap();
    assert_eq!(metadata.created.as_deref(), Some("2020-01-01"));
    assert_eq!(metadata.updated.as_deref(), Some("2026-11-02"));
    let changed_creation = next.replace("custom:", "created: 2021-01-01\ncustom:");
    assert!(prepare_note_metadata(&changed_creation, Some(old), "2026-11-02").is_err());

    let legacy = "# Legacy\n";
    assert_eq!(
        prepare_note_metadata(legacy, Some(legacy), "2026-11-02").unwrap(),
        legacy
    );
    let result = prepare_note_metadata("# Updated legacy\n", Some(legacy), "2026-11-02").unwrap();
    let metadata = parse_metadata(&result).unwrap();
    assert!(metadata.created.is_none());
    assert_eq!(metadata.updated.as_deref(), Some("2026-11-02"));
}

#[cfg(unix)]
#[test]
fn scans_propagate_authored_metadata_without_changing_source_bytes() {
    let dir = tempfile::tempdir().unwrap();
    let note = "---\ntitle: Authored title\ncreated: 2026-01-01\nupdated: 2026-11-02\ntags: [rust]\n---\n# Body\n";
    std::fs::write(dir.path().join("note.md"), note).unwrap();
    let documents = memory_core::read_source(dir.path()).unwrap();
    let doc = &documents[0];
    assert_eq!(doc.title.as_deref(), Some("Authored title"));
    assert_eq!(doc.created.as_deref(), Some("2026-01-01"));
    assert_eq!(doc.updated.as_deref(), Some("2026-11-02"));
    assert_eq!(doc.tags, ["rust"]);
    assert_eq!(
        std::fs::read_to_string(dir.path().join("note.md")).unwrap(),
        note
    );
}
