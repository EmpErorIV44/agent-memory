use memory_core::extract_links;

#[test]
fn extracts_relative_markdown_reference_and_wiki_links_with_original_lines() {
    let text = "---\nid: note\ntitle: '[[not-a-link]]'\n---\n# Связи\n[ADR](../decisions/adr.md#section)\n[Reference][ref]\n[[Setup|Настройка]] and [[docs/Guide.md#Heading]]\n[ID](id:stable-document)\n\n[ref]: notes/reference.md\n";
    let links = extract_links(text);
    let actual: Vec<_> = links
        .iter()
        .map(|link| (link.target.as_str(), link.kind.as_str(), link.start_line))
        .collect();
    assert_eq!(
        actual,
        vec![
            ("../decisions/adr.md#section", "markdown", 6),
            ("notes/reference.md", "markdown", 7),
            ("Setup", "wiki", 8),
            ("docs/Guide.md#Heading", "wiki", 8),
            ("id:stable-document", "markdown", 9),
        ]
    );
}

#[test]
fn excludes_code_images_external_and_escaped_links() {
    let text = r#"`[[inline]]` and `[inline](inline.md)`
```markdown
[[fenced]] [code](code.md)
```
    [[indented]]

![image](image.md) ![[embedded]] ![alt [[hidden]]](image.png)
\[[escaped]] and \\[[real]]
[Web](https://example.test/note.md) [Mail](mailto:test@example.test)
[Root](/root.md) [Heading](#section) [[#heading]]
[[https://example.test/no]] [[image.png]]
[Label [[nested]]](target.md)
<span data-note="[[attribute]]"></span>
[[Связь с пробелами]]
"#;
    let links = extract_links(text);
    let targets: Vec<_> = links.iter().map(|link| link.target.as_str()).collect();
    assert_eq!(targets, vec!["real", "target.md", "Связь с пробелами"]);
}

#[test]
fn preserves_occurrence_order_and_stable_id_wiki_targets() {
    let links = extract_links("[[first]] [Second](second.md) [[id:third#heading|Alias]]");
    let targets: Vec<_> = links.iter().map(|link| link.target.as_str()).collect();
    assert_eq!(targets, vec!["first", "second.md", "id:third#heading"]);
    assert!(links.iter().all(|link| link.start_line == 1));
}

#[test]
fn scanner_attaches_links_and_preserves_bom_and_crlf_line_numbers() {
    let temp = tempfile::tempdir().unwrap();
    std::fs::write(
        temp.path().join("note.md"),
        "\u{feff}---\r\nid: note\r\n---\r\n[[target]]\r\n",
    )
    .unwrap();
    let docs = memory_core::read_source(temp.path()).unwrap();
    assert_eq!(docs[0].links.len(), 1);
    assert_eq!(docs[0].links[0].start_line, 4);
}
