//! Markdown source models, independent of storage and transport.
mod write;
use anyhow::{Context, Result};
use cap_std::fs::Dir;
use pulldown_cmark::{Event, Parser, Tag, TagEnd};
use sha2::{Digest, Sha256};
use std::{io::Read, path::Path};
pub use write::{read_markdown, write_markdown, WriteMode};

pub fn content_hash(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

#[derive(Debug, Clone)]
pub struct Chunk {
    pub heading: String,
    pub body: String,
    pub start_line: usize,
    pub end_line: usize,
}

#[derive(Debug, Clone)]
pub struct Document {
    pub relative_path: String,
    pub content_hash: String,
    pub byte_size: u64,
    pub file_mtime_ns: i64,
    pub chunks: Vec<Chunk>,
}

/// Read registered Markdown files recursively. Symbolic links are never followed.
/// A failed read aborts the scan, so callers cannot mistake partial scans for deletions.
pub fn read_source(root: &Path) -> Result<Vec<Document>> {
    let directory = write::open_root(root).context("open registered source directory")?;
    let mut documents = Vec::new();
    visit(&directory, Path::new(""), &mut documents)?;
    documents.sort_by(|a, b| a.relative_path.cmp(&b.relative_path));
    Ok(documents)
}

fn visit(directory: &Dir, relative: &Path, documents: &mut Vec<Document>) -> Result<()> {
    for entry in directory.entries().context("read source directory")? {
        let entry = entry?;
        let file_type = entry.file_type()?;
        let name = entry.file_name();
        let path = relative.join(&name);
        if file_type.is_symlink() {
            continue;
        }
        if file_type.is_dir() {
            let parent = directory.try_clone()?.into_std_file();
            let child = Dir::from_std_file(cap_primitives::fs::open_dir_nofollow(
                &parent,
                Path::new(&name),
            )?);
            visit(&child, &path, documents)?;
        } else if file_type.is_file()
            && path
                .extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("md"))
        {
            // Resolve from the pinned directory capability, never from an ambient pathname.
            let mut file = directory
                .open(&name)
                .with_context(|| format!("read {}", path.display()))?;
            let metadata = file.metadata()?;
            anyhow::ensure!(metadata.is_file(), "Markdown source is not a regular file");
            let mut bytes = Vec::new();
            file.read_to_end(&mut bytes)?;
            let text = std::str::from_utf8(&bytes)
                .with_context(|| format!("Markdown is not UTF-8: {}", path.display()))?;
            let modified = metadata
                .modified()?
                .duration_since(cap_std::time::SystemTime::from_std(std::time::UNIX_EPOCH))?;
            documents.push(Document {
                relative_path: path
                    .to_str()
                    .context("source path is not UTF-8")?
                    .replace('\\', "/"),
                content_hash: content_hash(&bytes),
                byte_size: bytes.len() as u64,
                file_mtime_ns: modified
                    .as_nanos()
                    .try_into()
                    .context("file modification time overflow")?,
                chunks: chunk_markdown(text),
            });
        }
    }
    Ok(())
}

/// Split at Markdown headings, preserving inclusive line numbers in the original file.
pub fn chunk_markdown(text: &str) -> Vec<Chunk> {
    let offset = frontmatter_end(text);
    let mut boundaries = vec![(offset, String::new())];
    let mut heading = None::<(usize, String)>;
    for (event, range) in Parser::new(&text[offset..]).into_offset_iter() {
        match event {
            Event::Start(Tag::Heading { .. }) => {
                heading = Some((offset + range.start, String::new()))
            }
            Event::Text(value) | Event::Code(value) => {
                if let Some((_, title)) = &mut heading {
                    title.push_str(&value);
                }
            }
            Event::SoftBreak | Event::HardBreak => {
                if let Some((_, title)) = &mut heading {
                    title.push(' ');
                }
            }
            Event::End(TagEnd::Heading(_)) => {
                if let Some(value) = heading.take() {
                    boundaries.push(value);
                }
            }
            _ => {}
        }
    }
    let mut chunks = Vec::new();
    for (i, (start, title)) in boundaries.iter().enumerate() {
        let end = boundaries.get(i + 1).map_or(text.len(), |next| next.0);
        let slice = &text[*start..end];
        let body = slice.trim();
        if body.is_empty() {
            continue;
        }
        let leading = slice.len() - slice.trim_start().len();
        let start_byte = start + leading;
        let end_byte = start_byte + body.len();
        chunks.push(Chunk {
            heading: title.clone(),
            body: body.to_owned(),
            start_line: 1 + text[..start_byte].bytes().filter(|b| *b == b'\n').count(),
            end_line: 1 + text[..end_byte].bytes().filter(|b| *b == b'\n').count(),
        });
    }
    chunks
}

fn frontmatter_end(text: &str) -> usize {
    let mut lines = text.split_inclusive('\n');
    let Some(first) = lines.next() else { return 0 };
    if first.trim_end_matches(['\r', '\n']) != "---" {
        return 0;
    }
    let mut offset = first.len();
    for line in lines {
        offset += line.len();
        if matches!(line.trim_end_matches(['\r', '\n']), "---" | "...") {
            return offset;
        }
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sections_retain_original_lines_and_skip_frontmatter_and_fenced_headings() {
        let text = "---\r\nid: example\r\n---\r\n\r\n# Архитектура\r\nOAuth2\r\n\r\n## Details\r\n```md\r\n# not a heading\r\n```\r\n";
        let chunks = chunk_markdown(text);
        assert_eq!(chunks.len(), 2);
        assert_eq!(chunks[0].heading, "Архитектура");
        assert_eq!((chunks[0].start_line, chunks[0].end_line), (5, 6));
        assert_eq!((chunks[1].start_line, chunks[1].end_line), (8, 11));
        assert!(!chunks
            .iter()
            .any(|chunk| chunk.body.contains("id: example")));
        for chunk in chunks {
            let lines: Vec<_> = text.lines().collect();
            assert_eq!(
                chunk.body.replace("\r\n", "\n"),
                lines[chunk.start_line - 1..chunk.end_line].join("\n")
            );
        }
    }
}
