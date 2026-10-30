use crate::{memories, new_id, IndexStats};
use anyhow::{ensure, Context, Result};
use memory_core::Document;
use rusqlite::{params, Transaction};
use std::{
    collections::{HashMap, HashSet},
    path::Path,
};

struct Existing {
    id: i64,
    path: String,
    stable_id: Option<String>,
    hash: String,
}

/// All source scans, document reconciliation and structured projections share one transaction.
pub(crate) fn index_sources(
    tx: &Transaction<'_>,
    sources: &[(i64, String)],
    project_scope: Option<i64>,
) -> Result<IndexStats> {
    // Finish scanning first: a partial/failed scan is never interpreted as a deletion.
    let scans = sources
        .iter()
        .map(|(id, root)| {
            let documents = memory_core::read_source(Path::new(root))?;
            let mut ids = HashSet::new();
            for document in &documents {
                if let Some(id) = &document.stable_id {
                    ensure!(ids.insert(id), "duplicate document.id in source: {id}");
                }
            }
            Ok((*id, documents))
        })
        .collect::<Result<Vec<_>>>()?;

    let mut stats = IndexStats::default();
    let mut projections = Vec::new();
    let mut link_documents = Vec::new();
    for (source_id, documents) in scans {
        let existing = {
            let mut query = tx.prepare(
                "SELECT id, relative_path, stable_id, content_hash FROM documents WHERE source_id=?1")?;
            let rows = query.query_map([source_id], |row| {
                Ok(Existing {
                    id: row.get(0)?,
                    path: row.get(1)?,
                    stable_id: row.get(2)?,
                    hash: row.get(3)?,
                })
            })?;
            rows.collect::<rusqlite::Result<Vec<_>>>()?
        };
        let by_path: HashMap<_, _> = existing
            .iter()
            .map(|old| (old.path.as_str(), old))
            .collect();
        let by_id: HashMap<_, _> = existing
            .iter()
            .filter_map(|old| old.stable_id.as_deref().map(|id| (id, old)))
            .collect();
        let mut used = HashSet::new();
        let matches = documents
            .iter()
            .map(|document| {
                let previous = if let Some(stable_id) = &document.stable_id {
                    by_id.get(stable_id.as_str()).copied().or_else(|| {
                        // A document may acquire its first stable ID without losing its row.
                        by_path
                            .get(document.relative_path.as_str())
                            .copied()
                            .filter(|old| old.stable_id.is_none())
                    })
                } else {
                    by_path
                        .get(document.relative_path.as_str())
                        .copied()
                        .filter(|old| old.stable_id.is_none())
                };
                if let Some(old) = previous {
                    ensure!(
                        used.insert(old.id),
                        "document identity matches more than one file"
                    );
                }
                Ok(previous)
            })
            .collect::<Result<Vec<_>>>()?;

        // Remove obsolete identities before inserting replacements at the same path/ID.
        for old in existing.iter().filter(|old| !used.contains(&old.id)) {
            tx.execute("DELETE FROM documents WHERE id=?1", [old.id])?;
            stats.deleted += 1;
        }
        // Stage moved paths so swaps and rename-over-deletion cannot violate UNIQUE(path).
        let mut reserved: HashSet<String> = existing
            .iter()
            .map(|old| old.path.clone())
            .chain(documents.iter().map(|doc| doc.relative_path.clone()))
            .collect();
        for (document, previous) in documents.iter().zip(&matches) {
            if let Some(old) = previous.filter(|old| old.path != document.relative_path) {
                let mut staging = format!(".agent-memory-reconcile/{}", old.id);
                while !reserved.insert(staging.clone()) {
                    staging.push('_');
                }
                tx.execute(
                    "UPDATE documents SET relative_path=?1 WHERE id=?2",
                    params![staging, old.id],
                )?;
            }
        }
        for (document, previous) in documents.iter().zip(matches) {
            let unchanged = previous.is_some_and(|old| {
                old.hash == document.content_hash
                    && old.path == document.relative_path
                    && old.stable_id == document.stable_id
            });
            let document_id = if unchanged {
                stats.unchanged += 1;
                previous.unwrap().id
            } else {
                let byte_size = i64::try_from(document.byte_size).context("document too large")?;
                let title = document
                    .chunks
                    .first()
                    .map_or("", |chunk| chunk.heading.as_str());
                let id = if let Some(old) = previous {
                    tx.execute("UPDATE documents SET relative_path=?1, stable_id=?2, title=?3, content_hash=?4, file_mtime_ns=?5, byte_size=?6, indexed_at=unixepoch() WHERE id=?7",
                        params![document.relative_path, document.stable_id, title, document.content_hash,
                            document.file_mtime_ns, byte_size, old.id])?;
                    // A rename with identical bytes keeps valid chunk IDs and line references.
                    if old.hash != document.content_hash {
                        tx.execute("DELETE FROM document_chunks WHERE document_id=?1", [old.id])?;
                        // Metadata from the old physical document is stale for every binding.
                        // Only the requested project's fresh assertion is subsequently projected.
                        tx.execute("DELETE FROM memories WHERE document_id=?1", [old.id])?;
                    }
                    stats.updated += 1;
                    old.id
                } else {
                    let id = new_id(tx, "documents")?;
                    tx.execute("INSERT INTO documents(id, source_id, relative_path, stable_id, title, content_hash, file_mtime_ns, byte_size) VALUES (?1,?2,?3,?4,?5,?6,?7,?8)",
                        params![id, source_id, document.relative_path, document.stable_id, title,
                            document.content_hash, document.file_mtime_ns, byte_size])?;
                    stats.added += 1;
                    id
                };
                if previous.is_none_or(|old| old.hash != document.content_hash) {
                    insert_chunks(tx, id, document)?;
                }
                id
            };
            link_documents.push((document_id, document.clone()));
            if let Some(memory) = &document.memory {
                projections.push((document_id, source_id, memory.clone()));
            }
        }
    }
    // Delete all refreshed source projections first, allowing a Memory to move across sources.
    for (source_id, _) in sources {
        tx.execute("DELETE FROM memories WHERE document_id IN (SELECT id FROM documents WHERE source_id=?1) AND (?2 IS NULL OR project_id=?2)", params![source_id, project_scope])?;
    }
    for (document_id, source_id, memory) in projections {
        memories::insert(tx, document_id, source_id, &memory, project_scope)?;
    }
    crate::links::index(tx, &link_documents)?;
    Ok(stats)
}

fn insert_chunks(tx: &Transaction<'_>, document_id: i64, document: &Document) -> Result<()> {
    for (ordinal, chunk) in document.chunks.iter().enumerate() {
        let id = new_id(tx, "document_chunks")?;
        tx.execute("INSERT INTO document_chunks(id, document_id, ordinal, heading, body, start_line, end_line) VALUES (?1,?2,?3,?4,?5,?6,?7)",
            params![id, document_id, ordinal as i64, chunk.heading, chunk.body,
                chunk.start_line as i64, chunk.end_line as i64])?;
    }
    Ok(())
}
