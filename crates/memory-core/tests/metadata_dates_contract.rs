use anyhow::Result;

#[test]
fn authored_dates_use_gregorian_rules_and_tags_require_clean_unique_strings() -> Result<()> {
    for (created, updated) in [
        ("2000-02-29", "2000-03-01"),
        ("2024-02-29", "2024-02-29"),
        ("0001-01-01", "9999-12-31"),
    ] {
        let content = format!(
            "---\ncreated: '{created}'\nupdated: '{updated}'\ntags: [rust, память]\n---\n# Note\n"
        );
        let value = serde_json::to_value(memory_core::parse_metadata(&content)?)?;
        assert_eq!(value["created"], created);
        assert_eq!(value["updated"], updated);
        assert_eq!(value["tags"], serde_json::json!(["rust", "память"]));
    }
    for date in [
        "1900-02-29",
        "2100-02-29",
        "2023-02-29",
        "2024-04-31",
        "0000-01-01",
        "2024-00-01",
        "2024-13-01",
        "2024-01-00",
        "2024-1-01",
        "2024-01-1",
        " 2024-01-01",
        "2024-01-01T00:00:00Z",
    ] {
        assert!(
            memory_core::parse_metadata(&format!("---\ncreated: '{date}'\n---\n# Note\n")).is_err(),
            "accepted invalid date {date}"
        );
    }
    assert!(memory_core::parse_metadata(
        "---\ncreated: 2024-02-29\nupdated: 2024-02-28\n---\n# Note\n"
    )
    .is_err());
    for tags in [
        "[rust, rust]",
        "['']",
        "[' padded']",
        "['padded ']",
        "[12]",
        "rust",
    ] {
        assert!(
            memory_core::parse_metadata(&format!("---\ntags: {tags}\n---\n# Note\n")).is_err(),
            "accepted invalid tags {tags}"
        );
    }
    assert!(memory_core::parse_metadata("---\ntags: [\"bad\\nvalue\"]\n---\n# Note\n").is_err());
    Ok(())
}

#[test]
fn legacy_notes_do_not_acquire_dates_and_unknown_frontmatter_remains_supported() -> Result<()> {
    for content in [
        "# Legacy note\n",
        "---\nid: old\nadr_status: accepted\n---\n# Existing ADR\n",
    ] {
        let value = serde_json::to_value(memory_core::parse_metadata(content)?)?;
        assert!(value.get("created").is_none_or(serde_json::Value::is_null));
        assert!(value.get("updated").is_none_or(serde_json::Value::is_null));
        assert!(value
            .get("tags")
            .is_none_or(|v| v.as_array().is_some_and(Vec::is_empty)));
    }
    Ok(())
}
