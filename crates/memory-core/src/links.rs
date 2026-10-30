//! Author-written note links, independent of storage and link resolution.
use pulldown_cmark::{Event, Parser, Tag};

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct DocumentLink {
    /// Original target, including an optional heading fragment; wiki aliases are omitted.
    pub target: String,
    pub kind: String,
    /// One-based line number in the original complete Markdown document.
    pub start_line: usize,
}

fn local_note(target: &str, wiki: bool) -> bool {
    let path = target.split('#').next().unwrap_or("");
    if let Some(id) = path.strip_prefix("id:") {
        return !id.is_empty();
    }
    if path.is_empty() || path.starts_with(['/', '\\']) || path.contains(':') {
        return false;
    }
    let extension = std::path::Path::new(path).extension();
    extension.is_some_and(|extension| extension.eq_ignore_ascii_case("md"))
        || (wiki && extension.is_none())
}

fn line(text: &str, byte: usize) -> usize {
    text[..byte].bytes().filter(|byte| *byte == b'\n').count() + 1
}

fn escaped(bytes: &[u8], at: usize) -> bool {
    bytes[..at]
        .iter()
        .rev()
        .take_while(|byte| **byte == b'\\')
        .count()
        % 2
        == 1
}

/// Extract Markdown/reference links and Obsidian wiki links from prose only.
/// Resolution, root containment, ambiguity and permissions belong to storage.
pub fn extract_links(text: &str) -> Vec<DocumentLink> {
    let offset = crate::frontmatter_end(text);
    let mut excluded = vec![false; text.len()];
    excluded[..offset].fill(true);
    let mut links = Vec::new();
    for (event, range) in Parser::new(&text[offset..]).into_offset_iter() {
        let range = offset + range.start..offset + range.end;
        match event {
            Event::Start(Tag::Link { dest_url, .. }) => {
                if !excluded[range.start] && local_note(&dest_url, false) {
                    links.push((
                        range.start,
                        DocumentLink {
                            target: dest_url.into_string(),
                            kind: "markdown".into(),
                            start_line: line(text, range.start),
                        },
                    ));
                }
                // A wiki-looking link label or destination is not another note reference.
                excluded[range].fill(true);
            }
            Event::Start(Tag::CodeBlock(_) | Tag::Image { .. })
            | Event::Code(_)
            | Event::Html(_)
            | Event::InlineHtml(_) => excluded[range].fill(true),
            _ => {}
        }
    }
    let bytes = text.as_bytes();
    let mut at = offset;
    while at + 1 < bytes.len() {
        if bytes[at..].starts_with(b"[[")
            && !excluded[at]
            && !excluded[at + 1]
            && !escaped(bytes, at)
            && !(at > 0 && bytes[at - 1] == b'!' && !escaped(bytes, at - 1))
        {
            if let Some(end) = text[at + 2..].find("]]") {
                let end = at + 2 + end;
                let raw = &text[at + 2..end];
                if !raw.contains(['\n', '\r', '[', ']'])
                    && !excluded[at..end + 2].iter().any(|value| *value)
                {
                    let target = raw.split('|').next().unwrap_or("").trim();
                    if local_note(target, true) {
                        links.push((
                            at,
                            DocumentLink {
                                target: target.to_owned(),
                                kind: "wiki".into(),
                                start_line: line(text, at),
                            },
                        ));
                    }
                    at = end + 2;
                    continue;
                }
            }
        }
        at += 1;
        // All delimiter searches are ASCII; avoid slicing the string inside UTF-8 characters.
        while at < bytes.len() && !text.is_char_boundary(at) {
            at += 1;
        }
    }
    links.sort_by_key(|(offset, _)| *offset);
    links.into_iter().map(|(_, link)| link).collect()
}
