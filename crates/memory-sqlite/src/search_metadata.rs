use crate::SearchHit;
use anyhow::Result;
use memory_core::{MemoryKind, MemoryStatus};
use rusqlite::{params, Connection, Transaction};
use rust_stemmers::{Algorithm, Stemmer};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SearchOptions {
    pub kind: Option<MemoryKind>,
    pub status: Option<MemoryStatus>,
}

impl SearchOptions {
    pub fn from_filters(kind: Option<&str>, status: Option<&str>) -> Result<Self> {
        Ok(Self {
            kind: kind
                .map(|v| serde_json::from_value(serde_json::Value::String(v.into())))
                .transpose()?,
            status: status
                .map(|v| serde_json::from_value(serde_json::Value::String(v.into())))
                .transpose()?,
        })
    }
}

fn enum_string<T: Serialize>(value: &Option<T>) -> Result<Option<String>> {
    value
        .as_ref()
        .map(|v| {
            Ok(serde_json::to_value(v)?
                .as_str()
                .expect("enum string")
                .to_owned())
        })
        .transpose()
}

/// Preserve punctuation, FTS operators and English; normalize Russian words only.
fn russian(text: &str) -> String {
    let stemmer = Stemmer::create(Algorithm::Russian);
    let mut result = String::with_capacity(text.len());
    let mut word = String::new();
    let flush = |word: &mut String, result: &mut String| {
        if word.chars().any(|c| matches!(c, '\u{0400}'..='\u{04ff}')) {
            result.push_str(&stemmer.stem(&word.to_lowercase().replace('ё', "е")));
        } else {
            result.push_str(word);
        }
        word.clear();
    };
    for c in text.chars() {
        if c.is_alphanumeric() {
            word.push(c);
        } else {
            flush(&mut word, &mut result);
            result.push(c);
        }
    }
    flush(&mut word, &mut result);
    result
}

// Preserve lexical prefix semantics for Russian prefix queries.
fn has_russian_prefix(query: &str) -> bool {
    query.char_indices().any(|(i, c)| {
        c == '*'
            && query[..i]
                .trim_end_matches('"')
                .rsplit(|v: char| !v.is_alphanumeric())
                .next()
                .is_some_and(|word| word.chars().any(|v| matches!(v, '\u{0400}'..='\u{04ff}')))
    })
}

/// Incremental combined projections: no foreign project's summary enters a row.
pub(crate) fn index_morphology(tx: &Transaction<'_>) -> Result<()> {
    let existing = {
        let mut query =
            tx.prepare("SELECT rowid,project_id,chunk_id,fingerprint FROM search_exact_fts")?;
        let rows = query.query_map([], |r| {
            Ok((
                (r.get::<_, i64>(1)?, r.get::<_, i64>(2)?),
                (r.get::<_, i64>(0)?, r.get::<_, String>(3)?),
            ))
        })?;
        rows.collect::<rusqlite::Result<HashMap<_, _>>>()?
    };
    let documents = {
        let mut query = tx.prepare("SELECT ps.project_id,c.id,c.heading,c.body,d.title,d.relative_path,coalesce(d.stable_id,''),coalesce(m.memory_id,''),coalesce(m.summary,'') FROM document_chunks c JOIN documents d ON d.id=c.document_id JOIN project_sources ps ON ps.source_id=d.source_id AND ps.can_read=1 LEFT JOIN memories m ON m.document_id=d.id AND m.project_id=ps.project_id ORDER BY ps.project_id,c.id")?;
        let rows = query.query_map([], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, i64>(1)?,
                (2..9)
                    .map(|i| r.get::<_, String>(i))
                    .collect::<rusqlite::Result<Vec<_>>>()?,
            ))
        })?;
        rows.collect::<rusqlite::Result<Vec<_>>>()?
    };
    let mut retained = HashSet::new();
    for (project, chunk, fields) in documents {
        retained.insert((project, chunk));
        let fingerprint = memory_core::content_hash(serde_json::to_string(&fields)?.as_bytes());
        let previous = existing.get(&(project, chunk));
        if previous.is_some_and(|(_, old)| old == &fingerprint) {
            continue;
        }
        if let Some((rowid, _)) = previous {
            tx.execute("DELETE FROM search_exact_fts WHERE rowid=?1", [rowid])?;
            tx.execute("DELETE FROM search_russian_fts WHERE rowid=?1", [rowid])?;
        }
        tx.execute("INSERT INTO search_exact_fts(heading,body,title,path,stable_id,memory_id,summary,project_id,chunk_id,fingerprint) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)", params![fields[0],fields[1],fields[2],fields[3],fields[4],fields[5],fields[6],project,chunk,fingerprint])?;
        let rowid = tx.last_insert_rowid();
        let normalized: Vec<_> = fields.iter().map(|field| russian(field)).collect();
        tx.execute("INSERT INTO search_russian_fts(rowid,heading,body,title,path,stable_id,memory_id,summary,project_id,chunk_id,fingerprint) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)", params![rowid,normalized[0],normalized[1],normalized[2],normalized[3],normalized[4],normalized[5],normalized[6],project,chunk,fingerprint])?;
    }
    for (key, (rowid, _)) in existing {
        if !retained.contains(&key) {
            tx.execute("DELETE FROM search_exact_fts WHERE rowid=?1", [rowid])?;
            tx.execute("DELETE FROM search_russian_fts WHERE rowid=?1", [rowid])?;
        }
    }
    Ok(())
}

pub(crate) fn search(
    connection: &Connection,
    project_id: i64,
    query: &str,
) -> Result<Vec<SearchHit>> {
    search_filtered(connection, project_id, query, &SearchOptions::default())
}

pub(crate) fn search_filtered(
    connection: &Connection,
    project_id: i64,
    query: &str,
    options: &SearchOptions,
) -> Result<Vec<SearchHit>> {
    // Evaluate the original query first so normalization never hides malformed syntax.
    let exact = {
        let mut statement = connection.prepare("SELECT chunk_id,bm25(search_exact_fts,6.0,1.0,8.0,3.0,4.0,4.0,5.0) FROM search_exact_fts WHERE search_exact_fts MATCH ?1 AND project_id=?2")?;
        let rows = statement.query_map(params![query, project_id], |r| {
            Ok((r.get::<_, i64>(0)?, r.get::<_, f64>(1)?))
        })?;
        rows.collect::<rusqlite::Result<HashMap<_, _>>>()?
    };
    let kind = enum_string(&options.kind)?;
    let status = enum_string(&options.status)?;
    // Only the full normalized expression determines eligibility, including NOT.
    // Exact matches affect ordering: OR-ing exact/stem would bypass exclusions.
    let mut statement = connection.prepare("SELECT d.relative_path,c.heading,c.body,c.start_line,c.end_line,c.id,d.id,d.source_id,d.content_hash,d.stable_id,bm25(search_russian_fts,6.0,1.0,8.0,3.0,4.0,4.0,5.0) FROM search_russian_fts JOIN document_chunks c ON c.id=search_russian_fts.chunk_id JOIN documents d ON d.id=c.document_id WHERE search_russian_fts MATCH ?1 AND search_russian_fts.project_id=?2 AND EXISTS(SELECT 1 FROM project_sources ps WHERE ps.project_id=?2 AND ps.source_id=d.source_id AND ps.can_read=1) AND (?3 IS NULL OR EXISTS(SELECT 1 FROM memories m WHERE m.project_id=?2 AND m.document_id=d.id AND m.kind=?3)) AND (?4 IS NULL OR EXISTS(SELECT 1 FROM memories m WHERE m.project_id=?2 AND m.document_id=d.id AND m.status=?4))")?;
    let rows = statement.query_map(params![russian(query), project_id, kind, status], |row| {
        Ok((
            SearchHit {
                stable_id: row.get(9)?,
                path: row.get(0)?,
                heading: row.get(1)?,
                body: row.get(2)?,
                start_line: row.get::<_, i64>(3)? as usize,
                end_line: row.get::<_, i64>(4)? as usize,
                chunk_id: row.get(5)?,
                document_id: row.get(6)?,
                source_id: row.get(7)?,
                content_hash: row.get(8)?,
            },
            row.get::<_, f64>(10)?,
        ))
    })?;
    let mut hits = rows.collect::<rusqlite::Result<Vec<_>>>()?;
    if has_russian_prefix(query) {
        hits.retain(|(hit, _)| exact.contains_key(&hit.chunk_id));
    }
    hits.sort_by(|(a, ascore), (b, bscore)| {
        let ae = exact.get(&a.chunk_id);
        let be = exact.get(&b.chunk_id);
        be.is_some()
            .cmp(&ae.is_some())
            .then_with(|| ae.unwrap_or(ascore).total_cmp(be.unwrap_or(bscore)))
            .then_with(|| a.path.cmp(&b.path))
            .then_with(|| a.start_line.cmp(&b.start_line))
    });
    Ok(hits.into_iter().take(50).map(|(hit, _)| hit).collect())
}
