//! Rebuildable document links. Resolution never searches outside the origin source.
use crate::Store;
use anyhow::Result;
use rusqlite::{params, OptionalExtension, Transaction};
use std::collections::{HashMap, HashSet};

#[derive(Debug, Clone, serde::Serialize)]
pub struct LinkDocument {
    pub document_id: i64,
    pub source_id: i64,
    pub path: String,
    pub stable_id: Option<String>,
    pub content_hash: String,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct LinkRecord {
    pub kind: String,
    pub target: String,
    pub start_line: usize,
    pub status: String,
    pub source: LinkDocument,
    pub target_document: Option<LinkDocument>,
}

#[derive(Debug, serde::Serialize)]
pub struct DocumentLinks {
    pub outgoing: Vec<LinkRecord>,
    pub incoming: Vec<LinkRecord>,
}

fn normalize(base: &str, target: &str) -> Option<String> {
    if target.starts_with('/') || target.contains('\\') || target.contains(':') {
        return None;
    }
    let mut components: Vec<&str> = base.split('/').filter(|part| !part.is_empty()).collect();
    for part in target.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                components.pop()?;
            }
            value => components.push(value),
        }
    }
    if components.is_empty() {
        None
    } else {
        Some(components.join("/"))
    }
}

fn decoded(target: &str) -> Option<String> {
    let mut bytes = Vec::new();
    let raw = target.as_bytes();
    let mut pos = 0;
    while pos < raw.len() {
        if raw[pos] == b'%' {
            let pair = std::str::from_utf8(raw.get(pos + 1..pos + 3)?).ok()?;
            bytes.push(u8::from_str_radix(pair, 16).ok()?);
            pos += 3;
        } else {
            bytes.push(raw[pos]);
            pos += 1;
        }
    }
    String::from_utf8(bytes).ok()
}

fn resolve(
    origin: &LinkDocument,
    raw: &str,
    kind: &str,
    documents: &[LinkDocument],
) -> (Option<i64>, &'static str) {
    let target = raw.split('#').next().unwrap_or(raw);
    let Some(target) = decoded(target) else {
        return (None, "missing");
    };
    if let Some(id) = target.strip_prefix("id:") {
        return match documents
            .iter()
            .find(|document| document.stable_id.as_deref() == Some(id))
        {
            Some(document) => (Some(document.document_id), "resolved"),
            None => (None, "missing"),
        };
    }
    let target = if kind == "wiki" && !target.to_ascii_lowercase().ends_with(".md") {
        format!("{target}.md")
    } else {
        target
    };
    if !target.to_ascii_lowercase().ends_with(".md") {
        return (None, "missing");
    }
    let directory = origin
        .path
        .rsplit_once('/')
        .map_or("", |(directory, _)| directory);
    let candidates = if kind == "wiki" {
        vec![normalize("", &target), normalize(directory, &target)]
    } else {
        vec![normalize(directory, &target)]
    };
    for path in candidates.into_iter().flatten() {
        if let Some(document) = documents.iter().find(|document| document.path == path) {
            return (Some(document.document_id), "resolved");
        }
    }
    if kind == "wiki" && !target.contains('/') {
        let matches: Vec<_> = documents
            .iter()
            .filter(|document| document.path.rsplit('/').next() == Some(target.as_str()))
            .collect();
        return match matches.as_slice() {
            [document] => (Some(document.document_id), "resolved"),
            [] => (None, "missing"),
            _ => (None, "ambiguous"),
        };
    }
    (None, "missing")
}

pub(crate) fn index(
    tx: &Transaction<'_>,
    documents: &[(i64, memory_core::Document)],
) -> Result<()> {
    // All document identities and final paths have been reconciled before resolution.
    let mut sources = HashSet::new();
    for (id, _) in documents {
        let source: i64 =
            tx.query_row("SELECT source_id FROM documents WHERE id=?1", [id], |row| {
                row.get(0)
            })?;
        sources.insert(source);
    }
    let mut by_source = HashMap::new();
    for source in sources {
        let mut query = tx.prepare("SELECT id,source_id,relative_path,stable_id,content_hash FROM documents WHERE source_id=?1")?;
        let rows = query.query_map([source], read_document)?;
        by_source.insert(source, rows.collect::<rusqlite::Result<Vec<_>>>()?);
    }
    for (id, parsed) in documents {
        let origin: LinkDocument = tx.query_row(
            "SELECT id,source_id,relative_path,stable_id,content_hash FROM documents WHERE id=?1",
            [id],
            read_document,
        )?;
        let old = {
            let mut query = tx.prepare("SELECT ordinal,target,source_hash,target_document_id FROM document_links WHERE document_id=?1")?;
            let rows = query.query_map([id], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, Option<i64>>(3)?,
                ))
            })?;
            rows.collect::<rusqlite::Result<Vec<_>>>()?
        };
        tx.execute("DELETE FROM document_links WHERE document_id=?1", [id])?;
        let available = &by_source[&origin.source_id];
        for (ordinal, link) in parsed.links.iter().enumerate() {
            // Existing identity bindings survive target renames only when a stable ID
            // exists and the origin link text has not been edited.
            let retained = old
                .iter()
                .find(|(position, target, hash, _)| {
                    *position == ordinal as i64
                        && target == &link.target
                        && hash == &parsed.content_hash
                })
                .and_then(|(_, _, _, target)| *target)
                .filter(|target| {
                    available.iter().any(|document| {
                        document.document_id == *target && document.stable_id.is_some()
                    })
                });
            let (target, status) = retained.map_or_else(
                || resolve(&origin, &link.target, &link.kind, available),
                |id| (Some(id), "resolved"),
            );
            tx.execute("INSERT INTO document_links(document_id,ordinal,target,kind,start_line,source_hash,target_document_id,status) VALUES(?1,?2,?3,?4,?5,?6,?7,?8)", params![id,ordinal as i64,link.target,link.kind,link.start_line as i64,parsed.content_hash,target,status])?;
        }
    }
    Ok(())
}

fn read_document(row: &rusqlite::Row<'_>) -> rusqlite::Result<LinkDocument> {
    Ok(LinkDocument {
        document_id: row.get(0)?,
        source_id: row.get(1)?,
        path: row.get(2)?,
        stable_id: row.get(3)?,
        content_hash: row.get(4)?,
    })
}

impl Store {
    pub fn document_links(
        &self,
        project: &str,
        source_id: i64,
        path: &str,
    ) -> Result<Option<DocumentLinks>> {
        let project_id = self.project_id(project)?;
        let id: Option<i64> = self.connection.query_row("SELECT d.id FROM documents d WHERE d.source_id=?1 AND d.relative_path=?2 AND EXISTS(SELECT 1 FROM project_sources ps WHERE ps.source_id=d.source_id AND ps.project_id=?3 AND ps.can_read=1)", params![source_id,path,project_id], |row| row.get(0)).optional()?;
        let Some(id) = id else {
            return Ok(None);
        };
        let mut query = self.connection.prepare("SELECT l.document_id,l.target,l.kind,l.start_line,l.status,l.target_document_id FROM document_links l JOIN documents d ON d.id=l.document_id WHERE (l.document_id=?1 OR l.target_document_id=?1) AND EXISTS(SELECT 1 FROM project_sources ps WHERE ps.source_id=d.source_id AND ps.project_id=?2 AND ps.can_read=1) ORDER BY d.source_id,d.relative_path,l.ordinal")?;
        let rows = query.query_map(params![id, project_id], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, i64>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, Option<i64>>(5)?,
            ))
        })?;
        let mut result = DocumentLinks {
            outgoing: Vec::new(),
            incoming: Vec::new(),
        };
        for row in rows {
            let (origin, target, kind, line, status, target_id) = row?;
            let source = self.connection.query_row("SELECT id,source_id,relative_path,stable_id,content_hash FROM documents WHERE id=?1", [origin], read_document)?;
            let target_document = if let Some(target) = target_id {
                self.connection.query_row("SELECT d.id,d.source_id,d.relative_path,d.stable_id,d.content_hash FROM documents d WHERE d.id=?1 AND EXISTS(SELECT 1 FROM project_sources ps WHERE ps.source_id=d.source_id AND ps.project_id=?2 AND ps.can_read=1)", params![target,project_id], read_document).optional()?
            } else {
                None
            };
            let status = if target_document.is_none() && status == "resolved" {
                "missing".to_owned()
            } else {
                status
            };
            let record = LinkRecord {
                kind,
                target,
                start_line: line as usize,
                status,
                source,
                target_document,
            };
            if origin == id {
                result.outgoing.push(record.clone());
            }
            if target_id == Some(id) && record.target_document.is_some() {
                result.incoming.push(record);
            }
        }
        Ok(Some(result))
    }
}
