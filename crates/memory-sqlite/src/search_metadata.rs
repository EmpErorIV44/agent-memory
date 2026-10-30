use crate::SearchHit;
use anyhow::Result;
use rusqlite::{params, Connection};
use std::collections::HashSet;

pub(crate) fn search(
    connection: &Connection,
    project_id: i64,
    query: &str,
) -> Result<Vec<SearchHit>> {
    // Both indexes evaluate the complete query independently. Metadata matches
    // expand to the document's real chunks, keeping their IDs and line ranges.
    // Filter memory ownership before returning even an otherwise readable shared
    // document: its foreign project's frontmatter must not influence matches.
    let mut statement = connection.prepare(
        "SELECT d.relative_path, c.heading, c.body, c.start_line, c.end_line,
                c.id, d.id, d.source_id, d.content_hash, d.stable_id
         FROM (
             SELECT rowid AS chunk_id, bm25(document_chunks_fts) AS score
             FROM document_chunks_fts WHERE document_chunks_fts MATCH ?1
             UNION ALL
             SELECT c.id AS chunk_id, bm25(document_metadata_fts) AS score
             FROM document_metadata_fts
             JOIN document_chunks c ON c.document_id = document_metadata_fts.document_id
             WHERE document_metadata_fts MATCH ?1
               AND (document_metadata_fts.project_id IS NULL OR document_metadata_fts.project_id = ?2)
         ) AS matches
         JOIN document_chunks c ON c.id = matches.chunk_id
         JOIN documents d ON d.id = c.document_id
         WHERE EXISTS (SELECT 1 FROM project_sources ps
                       WHERE ps.source_id = d.source_id AND ps.project_id = ?2 AND ps.can_read = 1)
         ORDER BY matches.score, d.relative_path, c.ordinal",
    )?;
    let rows = statement.query_map(params![query, project_id], |row| {
        Ok(SearchHit {
            stable_id: row.get(9)?,
            chunk_id: row.get(5)?,
            document_id: row.get(6)?,
            source_id: row.get(7)?,
            content_hash: row.get(8)?,
            path: row.get(0)?,
            heading: row.get(1)?,
            body: row.get(2)?,
            start_line: row.get::<_, i64>(3)? as usize,
            end_line: row.get::<_, i64>(4)? as usize,
        })
    })?;
    let mut seen = HashSet::new();
    let mut hits = Vec::new();
    for row in rows {
        let hit = row?;
        if seen.insert(hit.chunk_id) {
            hits.push(hit);
            if hits.len() == 50 {
                break;
            }
        }
    }
    Ok(hits)
}
