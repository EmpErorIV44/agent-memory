use memory_core::{
    chunk_markdown, parse_metadata, read_source, write_markdown, EvidenceKind, MemoryKind,
    MemoryStatus, RelationKind, WriteMode,
};

const STRUCTURED: &str = r#"---
id: document-auth-v2
title: Authentication decision
tags: [architecture, authentication]
memory:
  id: authentication-policy
  project: banking
  kind: decision
  status: accepted
  summary: OAuth2 is used for authentication
  evidence:
    - kind: document
      reference: decisions/auth.md
    - kind: commit
      reference: 0123456789abcdef0123456789abcdef01234567
      description: Implementation reviewed
  relations:
    - kind: supersedes
      target: authentication-policy-v1
---
# Authentication
OAuth2 implemented.
"#;

#[test]
fn parses_document_and_memory_identifiers_separately_and_preserves_chunks() {
    let metadata = parse_metadata(STRUCTURED).unwrap();
    assert_eq!(metadata.stable_id.as_deref(), Some("document-auth-v2"));
    assert_eq!(metadata.title.as_deref(), Some("Authentication decision"));
    let memory = metadata.memory.unwrap();
    assert_eq!(memory.id, "authentication-policy");
    assert_eq!(memory.project, "banking");
    assert_eq!(memory.kind, MemoryKind::Decision);
    assert_eq!(memory.status, MemoryStatus::Accepted);
    assert_eq!(memory.evidence.len(), 2);
    assert_eq!(memory.evidence[0].kind, EvidenceKind::Document);
    assert_eq!(memory.evidence[0].description, None);
    assert_eq!(memory.relations[0].kind, RelationKind::Supersedes);
    let encoded = serde_yaml_ng::to_string(&memory).unwrap();
    assert_eq!(
        serde_yaml_ng::from_str::<memory_core::Memory>(&encoded).unwrap(),
        memory
    );
    let chunks = chunk_markdown(STRUCTURED);
    assert_eq!(chunks.len(), 1);
    assert_eq!(chunks[0].heading, "Authentication");
    assert_eq!(chunks[0].body, "# Authentication\nOAuth2 implemented.");
    assert_eq!(chunks[0].start_line, STRUCTURED.lines().count() - 1);
}

#[test]
fn legacy_markdown_unknown_adr_fields_empty_and_bom_headers_are_supported() {
    assert_eq!(
        parse_metadata("# Legacy\nNo metadata\n").unwrap().stable_id,
        None
    );
    assert_eq!(parse_metadata("---\n---\n# Empty\n").unwrap().memory, None);
    let adr = "\u{feff}---\r\nid: ADR-001\r\nstatus: accepted\r\ndate: 2026-10-29\r\nunknown: {nested: true}\r\n...\r\n# Решение\r\nТекст\r\n";
    assert_eq!(
        parse_metadata(adr).unwrap().stable_id.as_deref(),
        Some("ADR-001")
    );
    let chunks = chunk_markdown(adr);
    assert_eq!(chunks.len(), 1);
    assert_eq!(chunks[0].start_line, 7);
    assert_eq!(chunks[0].end_line, 8);
    assert!(!chunks[0].body.contains("id:"));
}

#[test]
fn invalid_frontmatter_is_rejected_before_writing_or_scanning() {
    let directory = tempfile::tempdir().unwrap();
    for metadata in [
        "---\nid: missing-close\n# Heading\n",
        "---\nid: [a, b]\n---\n# Heading\n",
        "---\nid: 123\n---\n# Heading\n",
        "---\nid: null\n---\n# Heading\n",
        "---\nid: ''\n---\n# Heading\n",
        "---\nid: ' padded '\n---\n# Heading\n",
        "---\nid: first\nid: second\n---\n# Heading\n",
        "---\n- list\n---\n# Heading\n",
        "---\nmemory: null\n---\n# Heading\n",
    ] {
        assert!(parse_metadata(metadata).is_err(), "accepted {metadata:?}");
        assert!(
            write_markdown(directory.path(), "invalid.md", metadata, WriteMode::Create).is_err()
        );
        assert!(!directory.path().join("invalid.md").exists());
        std::fs::write(directory.path().join("external.md"), metadata).unwrap();
        assert!(read_source(directory.path()).is_err());
        std::fs::remove_file(directory.path().join("external.md")).unwrap();
    }
}

#[test]
fn structured_memory_requires_valid_explicit_fields_and_relations() {
    for invalid in [
        STRUCTURED.replace("project: banking", "project: ''"),
        STRUCTURED.replace("kind: decision", "kind: invented"),
        STRUCTURED.replace("status: accepted", "status: perhaps"),
        STRUCTURED.replace("summary: OAuth2 is used for authentication", "summary: ''"),
        STRUCTURED.replace("reference: decisions/auth.md", "reference: ''"),
        STRUCTURED.replace(
            "target: authentication-policy-v1",
            "target: authentication-policy",
        ),
        STRUCTURED.replace("  summary:", "  misspelled_summary:"),
        STRUCTURED.replace("  project: banking\n", ""),
    ] {
        assert!(parse_metadata(&invalid).is_err(), "accepted {invalid}");
    }
    let duplicate = STRUCTURED.replace(
        "      target: authentication-policy-v1\n",
        "      target: authentication-policy-v1\n    - kind: supersedes\n      target: authentication-policy-v1\n",
    );
    assert!(parse_metadata(&duplicate).is_err());
}

#[test]
fn source_documents_retain_metadata_and_limit_large_frontmatter() {
    let directory = tempfile::tempdir().unwrap();
    std::fs::write(directory.path().join("decision.md"), STRUCTURED).unwrap();
    let documents = read_source(directory.path()).unwrap();
    assert_eq!(documents[0].stable_id.as_deref(), Some("document-auth-v2"));
    assert_eq!(
        documents[0].memory.as_ref().unwrap().id,
        "authentication-policy"
    );
    assert!(parse_metadata(&format!(
        "---\nignored: {}\n---\n# Body\n",
        "a".repeat(64 * 1024)
    ))
    .is_err());
}

#[test]
fn scalar_aliases_work_and_recursive_or_nested_identifier_values_are_rejected() {
    let alias = "---\nanchor: &doc_id document-id\nid: *doc_id\n---\n# Body\n";
    assert_eq!(
        parse_metadata(alias).unwrap().stable_id.as_deref(),
        Some("document-id")
    );
    let recursive = "---\nid: &recursive [*recursive]\n---\n# Body\n";
    assert!(parse_metadata(recursive).is_err());
    let nested = format!(
        "---\nid: {}value{}\n---\n# Body\n",
        "[".repeat(200),
        "]".repeat(200)
    );
    assert!(parse_metadata(&nested).is_err());
}
