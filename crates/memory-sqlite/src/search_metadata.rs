use crate::SearchHit;
use anyhow::{ensure, Context, Result};
use memory_core::{MemoryKind, MemoryStatus};
use rusqlite::{params, Connection, Transaction};
use rust_stemmers::{Algorithm, Stemmer};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SearchMode {
    #[default]
    Fts,
    Natural,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SearchOptions {
    pub kind: Option<MemoryKind>,
    pub status: Option<MemoryStatus>,
    /// Maximum returned lexical hits, from 1 through 50.
    pub limit: usize,
    /// Treat the whole query as one FTS5 phrase, rather than an expression.
    pub literal: bool,
    pub mode: SearchMode,
}

impl Default for SearchOptions {
    fn default() -> Self {
        Self {
            kind: None,
            status: None,
            limit: 50,
            literal: false,
            mode: SearchMode::Fts,
        }
    }
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
            ..Self::default()
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
pub(crate) fn russian(text: &str) -> String {
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
    ensure!(
        (1..=50).contains(&options.limit),
        "Search limit must be between 1 and 50"
    );
    ensure!(
        !(options.literal && options.mode == SearchMode::Natural),
        "natural mode cannot be combined with literal"
    );
    if options.mode == SearchMode::Natural {
        return search_natural(connection, project_id, query, options);
    }
    let mut hits = search_ranked(connection, project_id, query, options)?;
    hits.truncate(options.limit);
    Ok(hits)
}

fn search_ranked(
    connection: &Connection,
    project_id: i64,
    query: &str,
    options: &SearchOptions,
) -> Result<Vec<SearchHit>> {
    let literal_query;
    let query = if options.literal {
        literal_query = format!("\"{}\"", query.replace('"', "\"\""));
        literal_query.as_str()
    } else {
        query
    };
    // Evaluate the original expression (or explicitly quoted phrase) first so
    // normalization never hides malformed syntax. Never retry a changed query.
    let mut statement = connection.prepare("SELECT chunk_id,bm25(search_exact_fts,6.0,1.0,8.0,3.0,4.0,4.0,5.0) FROM search_exact_fts WHERE search_exact_fts MATCH ?1 AND project_id=?2")?;
    let exact = (|| -> rusqlite::Result<HashMap<i64, f64>> {
        let rows = statement.query_map(params![query, project_id], |r| {
            Ok((r.get::<_, i64>(0)?, r.get::<_, f64>(1)?))
        })?;
        rows.collect::<rusqlite::Result<HashMap<_, _>>>()
    })()
    .with_context(|| "Unable to evaluate search query")
    .map_err(|error| {
        let is_query_syntax = error.chain().any(|cause| {
            matches!(cause.downcast_ref::<rusqlite::Error>(),
                Some(rusqlite::Error::SqliteFailure(_, Some(message)))
                    if message.starts_with("fts5: syntax error")
                        || message.starts_with("fts5: unterminated string")
                        || message.starts_with("unterminated string")
                        || message.starts_with("no such column:")
                        || message.starts_with("expected integer, got "))
        });
        if is_query_syntax {
            error.context("Invalid FTS5 search expression. Double-quote literal terms containing punctuation, for example \"Co-Authored-By\", and double embedded quotes. Use literal=true in MCP or --literal in CLI to search the whole input as a phrase.")
        } else {
            error
        }
    })?;
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
    if !options.literal && has_russian_prefix(query) {
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
    Ok(hits.into_iter().map(|(hit, _)| hit).collect())
}

/// Safe word queries reuse project-scoped FTS eligibility and ranking. Per-word
/// matches count index tokens, including metadata, rather than substring matches.
fn search_natural(
    connection: &Connection,
    project_id: i64,
    query: &str,
    options: &SearchOptions,
) -> Result<Vec<SearchHit>> {
    ensure!(
        query.len() <= 8192,
        "Natural query must be at most 8192 UTF-8 bytes"
    );
    let terms: std::collections::BTreeSet<String> = query
        .split(|c: char| !c.is_alphanumeric())
        .filter(|s| !s.is_empty())
        .map(str::to_lowercase)
        .collect();
    ensure!(
        terms.len() <= 128,
        "Natural query must contain at most 128 distinct words"
    );
    if terms.is_empty() {
        return Ok(Vec::new());
    }
    let expression = terms
        .iter()
        .map(|s| format!("\"{s}\""))
        .collect::<Vec<_>>()
        .join(" OR ");
    // Retrieve all eligible candidates before applying the public result limit.
    // The shared FTS helper deliberately has no SQL limit.
    let mut lexical = options.clone();
    lexical.mode = SearchMode::Fts;
    lexical.limit = 50;
    let mut hits = search_ranked(connection, project_id, &expression, &lexical)?;
    let mut coverage: HashMap<i64, (usize, usize)> = HashMap::new();
    let mut document_coverage: HashMap<i64, (usize, usize)> = HashMap::new();
    let mut normalized_terms = HashSet::new();
    for term in terms {
        let quoted = format!("\"{term}\"");
        let normalized = russian(&quoted);
        for (table, expression, exact) in [
            ("search_russian_fts", normalized.as_str(), false),
            ("search_exact_fts", quoted.as_str(), true),
        ] {
            if !exact && !normalized_terms.insert(expression.to_owned()) {
                continue;
            }
            let mut statement = connection.prepare(&format!(
                "SELECT chunk_id,c.document_id FROM {table} JOIN document_chunks c ON c.id={table}.chunk_id WHERE {table} MATCH ?1 AND project_id=?2"
            ))?;
            let ids = statement.query_map(params![expression, project_id], |r| {
                Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?))
            })?;
            let mut documents = HashSet::new();
            for id in ids {
                let (chunk_id, document_id) = id?;
                documents.insert(document_id);
                let counts = coverage.entry(chunk_id).or_default();
                if exact {
                    counts.1 += 1;
                } else {
                    counts.0 += 1;
                }
            }
            for document in documents {
                let counts = document_coverage.entry(document).or_default();
                if exact {
                    counts.1 += 1;
                } else {
                    counts.0 += 1;
                }
            }
        }
    }
    // Stable sort retains exact/BM25/path ordering for equal coverage.
    hits.sort_by_key(|hit| {
        std::cmp::Reverse((
            document_coverage
                .get(&hit.document_id)
                .copied()
                .unwrap_or_default(),
            coverage.get(&hit.chunk_id).copied().unwrap_or_default(),
        ))
    });
    let mut seen = HashSet::new();
    hits.retain(|hit| seen.insert(hit.document_id));
    hits.truncate(options.limit);
    Ok(hits)
}
