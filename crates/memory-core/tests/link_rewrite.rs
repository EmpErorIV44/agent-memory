use memory_core::rewrite_links;

#[test]
fn rewrites_destinations_preserving_prose_code_aliases_and_reference_labels() {
    let input = "---\nid: note\n---\n# Heading\n[Label](old.md#anchor \"title\") [[old#section|Alias]]\n[Ref][r]\n\n[r]: <old.md#ref> \"Reference title\"\n\n`[Code](old.md)`\n```md\n[[old]] [Code](old.md)\n```\n<!-- [[old]] -->\n";
    let rewritten = rewrite_links(input, |target, kind| {
        if kind == "wiki" {
            Some(target.replacen("old", "new", 1))
        } else {
            Some(target.replacen("old.md", "new.md", 1))
        }
    });
    assert!(rewritten.contains("[Label](new.md#anchor \"title\") [[new#section|Alias]]"));
    assert!(rewritten.contains("[Ref][r]"));
    assert!(rewritten.contains("[r]: <new.md#ref> \"Reference title\""));
    assert!(rewritten.contains("`[Code](old.md)`"));
    assert!(rewritten.contains("```md\n[[old]] [Code](old.md)\n```"));
    assert!(rewritten.contains("<!-- [[old]] -->"));
    assert!(rewritten.starts_with("---\nid: note\n---"));
}

#[test]
fn image_only_and_unused_reference_definitions_are_untouched() {
    let input = "![Image][image]\n\n[image]: target.md\n[unused]: target.md\n";
    assert_eq!(
        rewrite_links(input, |_, _| Some("renamed.md".into())),
        input
    );
    let shared = "![Image][shared]\n[Prose][shared]\n\n[shared]: target.md\n";
    let output = rewrite_links(shared, |_, _| Some("renamed.md".into()));
    assert_eq!(
        output,
        "![Image][shared]\n[Prose][shared]\n\n[shared]: renamed.md\n"
    );
}

#[test]
fn escaped_closing_bracket_in_reference_label_is_preserved() {
    let input = "[x][a\\]:b]\n\n[a\\]:b]: target.md \"Title\"\n";
    assert_eq!(memory_core::extract_links(input).len(), 1);
    let output = rewrite_links(input, |_, _| Some("renamed.md".into()));
    assert_eq!(output, "[x][a\\]:b]\n\n[a\\]:b]: renamed.md \"Title\"\n");
}

#[test]
fn nested_link_in_image_alt_is_untouched() {
    let input = "![a [link](target.md)](image.png)\n";
    assert_eq!(
        rewrite_links(input, |_, _| Some("renamed.md".into())),
        input
    );
}
