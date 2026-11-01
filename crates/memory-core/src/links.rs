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

/// Rewrite only prose note destinations, preserving labels, aliases, titles and code.
/// Reference uses are kept intact; their definition destination is rewritten once.
/// Image-only and unused definitions are untouched. A definition shared by prose
/// and an image follows the prose destination, since both uses share one definition.
pub fn rewrite_links(text: &str, mut replace: impl FnMut(&str, &str) -> Option<String>) -> String {
    use pulldown_cmark::LinkType;
    let offset = crate::frontmatter_end(text);
    let parser = Parser::new(&text[offset..]);
    let mut used_definitions = std::collections::HashSet::new();
    let mut image_depth = 0usize;
    for event in Parser::new(&text[offset..]) {
        match event {
            Event::Start(Tag::Image { .. }) => image_depth += 1,
            Event::End(pulldown_cmark::TagEnd::Image) => {
                image_depth = image_depth.saturating_sub(1)
            }
            Event::Start(Tag::Link { id, dest_url, .. })
                if image_depth == 0 && local_note(&dest_url, false) =>
            {
                if let Some(definition) = parser.reference_definitions().get(&id) {
                    used_definitions.insert(definition.span.start);
                }
            }
            _ => {}
        }
    }
    let mut edits: Vec<(std::ops::Range<usize>, String)> = Vec::new();
    let mut excluded = vec![false; text.len()];
    excluded[..offset].fill(true);
    for (_, definition) in parser.reference_definitions().iter() {
        let span = offset + definition.span.start..offset + definition.span.end;
        excluded[span.clone()].fill(true);
        if used_definitions.contains(&definition.span.start) && local_note(&definition.dest, false)
        {
            if let Some(new) = replace(&definition.dest, "markdown") {
                let raw = &text[span.clone()];
                if let Some(colon) =
                    raw.as_bytes()
                        .windows(2)
                        .enumerate()
                        .find_map(|(at, bytes)| {
                            (bytes == b"]:" && !escaped(raw.as_bytes(), at)).then_some(at)
                        })
                {
                    if let Some(range) = destination_range(text, span.start + colon + 2, span.end) {
                        edits.push((range, new));
                    }
                }
            }
        }
    }
    for (event, range) in parser.into_offset_iter() {
        let range = offset + range.start..offset + range.end;
        match event {
            Event::Start(Tag::Link {
                dest_url,
                link_type,
                ..
            }) => {
                if !excluded[range.start]
                    && link_type == LinkType::Inline
                    && local_note(&dest_url, false)
                {
                    if let Some(new) = replace(&dest_url, "markdown") {
                        // The first balanced label end separates the destination from its label.
                        let raw = &text[range.clone()];
                        let mut depth = 0usize;
                        let bytes = raw.as_bytes();
                        for at in 0..bytes.len().saturating_sub(1) {
                            if escaped(bytes, at) {
                                continue;
                            }
                            if bytes[at] == b'[' {
                                depth += 1;
                            }
                            if bytes[at] == b']' {
                                depth = depth.saturating_sub(1);
                                if depth == 0 && bytes[at + 1] == b'(' {
                                    if let Some(destination) =
                                        destination_range(text, range.start + at + 2, range.end - 1)
                                    {
                                        edits.push((destination, new));
                                    }
                                    break;
                                }
                            }
                        }
                    }
                }
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
    while at + 1 < text.len() {
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
                    let target = raw.split('|').next().unwrap_or("");
                    if local_note(target.trim(), true) {
                        if let Some(new) = replace(target.trim(), "wiki") {
                            let leading = target.len() - target.trim_start().len();
                            edits.push((at + 2 + leading..at + 2 + target.trim_end().len(), new));
                        }
                    }
                    at = end + 2;
                    continue;
                }
            }
        }
        at += 1;
        while at < text.len() && !text.is_char_boundary(at) {
            at += 1;
        }
    }
    edits.sort_by_key(|(range, _)| range.start);
    let mut result = text.to_owned();
    for (range, new) in edits.into_iter().rev() {
        result.replace_range(range, &new);
    }
    result
}

fn destination_range(text: &str, mut at: usize, end: usize) -> Option<std::ops::Range<usize>> {
    let bytes = text.as_bytes();
    while at < end && bytes[at].is_ascii_whitespace() {
        at += 1;
    }
    if at >= end {
        return None;
    }
    if bytes[at] == b'<' {
        let close = text[at + 1..end].find('>')?;
        return Some(at + 1..at + 1 + close);
    }
    let start = at;
    let mut depth = 0usize;
    while at < end {
        if !escaped(bytes, at) {
            match bytes[at] {
                b'(' => depth += 1,
                b')' if depth > 0 => depth -= 1,
                b')' => break,
                b if b.is_ascii_whitespace() && depth == 0 => break,
                _ => {}
            }
        }
        at += 1;
    }
    (at > start).then_some(start..at)
}
