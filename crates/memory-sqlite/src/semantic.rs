//! Disposable local embeddings. Providers run outside these short DB operations.
use crate::{SearchHit, SearchOptions, Store};
use anyhow::{ensure, Context, Result};
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use serde::Serialize;
use std::collections::{HashMap, HashSet};

const MAX_DIMENSIONS: usize = 8192;
const LIMIT: usize = 50;

#[derive(Debug, Clone, Serialize)]
pub struct SemanticCandidate {
    pub hit: SearchHit,
    pub input: String,
    pub input_hash: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct SemanticHit {
    pub hit: SearchHit,
    pub similarity: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct HybridHit {
    pub hit: SearchHit,
    pub score: f64,
    pub lexical_rank: Option<usize>,
    pub semantic_rank: Option<usize>,
}

fn model(key: &str) -> Result<()> {
    ensure!(
        !key.is_empty() && key.len() <= 1024 && !key.chars().any(char::is_control),
        "model key must contain 1..1024 bytes without control characters"
    );
    Ok(())
}

fn normalized(vector: &[f32]) -> Result<Vec<f32>> {
    ensure!(
        !vector.is_empty() && vector.len() <= MAX_DIMENSIONS,
        "embedding dimensions must be within 1..8192"
    );
    ensure!(
        vector.iter().all(|value| value.is_finite()),
        "embedding contains nonfinite values"
    );
    // f64 prevents overflow for finite f32 extremes and underflow for tiny f32s.
    let norm = vector
        .iter()
        .map(|v| f64::from(*v).powi(2))
        .sum::<f64>()
        .sqrt();
    ensure!(
        norm.is_finite() && norm > 0.0,
        "embedding must have a nonzero finite norm"
    );
    Ok(vector
        .iter()
        .map(|v| (f64::from(*v) / norm) as f32)
        .collect())
}

fn enum_value<T: Serialize>(value: &Option<T>) -> Result<Option<String>> {
    value
        .as_ref()
        .map(|value| {
            serde_json::to_value(value)?
                .as_str()
                .map(str::to_owned)
                .context("invalid filter enum")
        })
        .transpose()
}

fn candidates(
    connection: &Connection,
    project_id: i64,
    options: &SearchOptions,
    chunk_id: Option<i64>,
) -> Result<Vec<SemanticCandidate>> {
    let kind = enum_value(&options.kind)?;
    let status = enum_value(&options.status)?;
    // Shared-source frontmatter and foreign Memory summaries/evidence never enter input.
    let mut query = connection.prepare(
        "SELECT d.relative_path,c.heading,c.body,c.start_line,c.end_line,c.id,d.id,d.source_id,d.content_hash,d.stable_id,d.title,coalesce(m.memory_id,''),coalesce(m.summary,'') FROM document_chunks c JOIN documents d ON d.id=c.document_id LEFT JOIN memories m ON m.document_id=d.id AND m.project_id=?1 WHERE EXISTS(SELECT 1 FROM project_sources ps WHERE ps.project_id=?1 AND ps.source_id=d.source_id AND ps.can_read=1) AND (?2 IS NULL OR m.kind=?2) AND (?3 IS NULL OR m.status=?3) AND (?4 IS NULL OR c.id=?4) ORDER BY c.id"
    )?;
    let rows = query.query_map(params![project_id, kind, status, chunk_id], |row| {
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
            row.get::<_, String>(10)?,
            row.get::<_, String>(11)?,
            row.get::<_, String>(12)?,
        ))
    })?;
    rows.map(|row| {
        let (hit, title, memory_id, summary) = row?;
        // Versioned, unambiguous serialization is also the exact embedding input.
        let input = serde_json::to_string(&serde_json::json!({
            "format":"agent-memory-semantic-v1", "title":title,"heading":hit.heading,
            "body":hit.body,"memory_id":memory_id,"summary":summary
        }))?;
        let input_hash = memory_core::content_hash(input.as_bytes());
        Ok(SemanticCandidate {
            hit,
            input,
            input_hash,
        })
    })
    .collect()
}

fn current(connection: &Connection, project_id: i64, supplied: &SemanticCandidate) -> Result<bool> {
    Ok(candidates(
        connection,
        project_id,
        &SearchOptions::default(),
        Some(supplied.hit.chunk_id),
    )?
    .into_iter()
    .next()
    .is_some_and(|candidate| {
        candidate.hit.content_hash == supplied.hit.content_hash
            && candidate.input_hash == supplied.input_hash
            && candidate.input == supplied.input
    }))
}

fn read_vector(
    connection: &Connection,
    project_id: i64,
    model_key: &str,
    candidate: &SemanticCandidate,
) -> Result<Option<Vec<f32>>> {
    let row = connection.query_row(
        "SELECT dimensions,vector_json FROM semantic_vectors WHERE project_id=?1 AND chunk_id=?2 AND model_key=?3 AND input_hash=?4",
        params![project_id,candidate.hit.chunk_id,model_key,candidate.input_hash],
        |row| Ok((row.get::<_,i64>(0)?,row.get::<_,String>(1)?)),
    ).optional()?;
    row.map(|(dimensions, body)| {
        // JSON/f32 data is bounded at writes and revalidated when read.
        ensure!(
            body.len() <= MAX_DIMENSIONS * 32,
            "cached embedding exceeds size limit"
        );
        let vector: Vec<f32> = serde_json::from_str(&body).context("invalid cached embedding")?;
        ensure!(
            vector.len() as i64 == dimensions,
            "cached embedding dimension mismatch"
        );
        normalized(&vector)
    })
    .transpose()
}

impl Store {
    pub fn semantic_candidates(
        &self,
        project: &str,
        options: &SearchOptions,
    ) -> Result<Vec<SemanticCandidate>> {
        candidates(&self.connection, self.project_id(project)?, options, None)
    }

    /// Return only a cache entry for the current authorized input/model snapshot.
    pub fn semantic_cached(
        &self,
        project: &str,
        model_key: &str,
        candidate: &SemanticCandidate,
    ) -> Result<Option<Vec<f32>>> {
        model(model_key)?;
        let project_id = self.project_id(project)?;
        let tx = self.connection.unchecked_transaction()?;
        if !current(&tx, project_id, candidate)? {
            return Ok(None);
        }
        read_vector(&tx, project_id, model_key, candidate)
    }

    /// Publish a provider result only if its input is still current and readable.
    /// The provider request must have completed before entering this transaction.
    pub fn semantic_cache(
        &mut self,
        project: &str,
        model_key: &str,
        candidate: &SemanticCandidate,
        vector: &[f32],
    ) -> Result<bool> {
        model(model_key)?;
        let vector = normalized(vector)?;
        let project_id = self.project_id(project)?;
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        if !current(&tx, project_id, candidate)? {
            return Ok(false);
        }
        let dimensions: Option<i64> = tx.query_row(
            "SELECT dimensions FROM semantic_vectors WHERE project_id=?1 AND model_key=?2 LIMIT 1",
            params![project_id,model_key], |row| row.get(0),
        ).optional()?;
        ensure!(
            dimensions.is_none_or(|size| size == vector.len() as i64),
            "model embedding dimension mismatch; use a distinct model key"
        );
        tx.execute("INSERT INTO semantic_vectors(project_id,chunk_id,model_key,input_hash,dimensions,vector_json) VALUES(?1,?2,?3,?4,?5,?6) ON CONFLICT(project_id,chunk_id,model_key) DO UPDATE SET input_hash=excluded.input_hash,dimensions=excluded.dimensions,vector_json=excluded.vector_json",
            params![project_id,candidate.hit.chunk_id,model_key,candidate.input_hash,vector.len() as i64,serde_json::to_string(&vector)?])?;
        tx.commit()?;
        Ok(true)
    }

    pub fn semantic_search(
        &self,
        project: &str,
        model_key: &str,
        query_vector: &[f32],
        options: &SearchOptions,
    ) -> Result<Vec<SemanticHit>> {
        model(model_key)?;
        let query = normalized(query_vector)?;
        let project_id = self.project_id(project)?;
        let tx = self.connection.unchecked_transaction()?;
        let mut results = Vec::new();
        for candidate in candidates(&tx, project_id, options, None)? {
            if let Some(vector) = read_vector(&tx, project_id, model_key, &candidate)? {
                ensure!(
                    vector.len() == query.len(),
                    "query/model embedding dimension mismatch"
                );
                let similarity = vector
                    .iter()
                    .zip(&query)
                    .map(|(a, b)| f64::from(*a) * f64::from(*b))
                    .sum::<f64>()
                    .clamp(-1.0, 1.0);
                results.push(SemanticHit {
                    hit: candidate.hit,
                    similarity,
                });
            }
        }
        results.sort_by(|a, b| {
            b.similarity
                .total_cmp(&a.similarity)
                .then_with(|| a.hit.chunk_id.cmp(&b.hit.chunk_id))
        });
        results.truncate(LIMIT);
        Ok(results)
    }
}

/// RRF (k=60) preserves each input's rank, ignores duplicate IDs within a list,
/// and merges a natural-language lexical result with semantic results.
pub fn reciprocal_rank_fusion(
    lexical: Vec<SearchHit>,
    semantic: Vec<SemanticHit>,
) -> Vec<HybridHit> {
    let mut results: HashMap<i64, HybridHit> = HashMap::new();
    let mut seen = HashSet::new();
    for (index, hit) in lexical.into_iter().enumerate() {
        if !seen.insert(hit.chunk_id) {
            continue;
        }
        let rank = index + 1;
        results.insert(
            hit.chunk_id,
            HybridHit {
                hit,
                score: 1.0 / (60.0 + rank as f64),
                lexical_rank: Some(rank),
                semantic_rank: None,
            },
        );
    }
    seen.clear();
    for (index, entry) in semantic.into_iter().enumerate() {
        if !seen.insert(entry.hit.chunk_id) {
            continue;
        }
        let rank = index + 1;
        let hit = results.entry(entry.hit.chunk_id).or_insert(HybridHit {
            hit: entry.hit,
            score: 0.0,
            lexical_rank: None,
            semantic_rank: None,
        });
        hit.score += 1.0 / (60.0 + rank as f64);
        hit.semantic_rank = Some(rank);
    }
    let mut results: Vec<_> = results.into_values().collect();
    results.sort_by(|a, b| {
        b.score
            .total_cmp(&a.score)
            .then_with(|| a.hit.chunk_id.cmp(&b.hit.chunk_id))
    });
    results.truncate(LIMIT);
    results
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hit(id: i64) -> SearchHit {
        SearchHit {
            stable_id: None,
            chunk_id: id,
            document_id: id,
            source_id: 1,
            content_hash: "a".repeat(64),
            path: format!("{id}.md"),
            heading: String::new(),
            body: "text".into(),
            start_line: 1,
            end_line: 1,
        }
    }

    #[test]
    fn normalization_is_bounded_finite_and_robust_for_f32_extremes() -> Result<()> {
        assert!(normalized(&[]).is_err());
        assert!(normalized(&[0.0, 0.0]).is_err());
        assert!(normalized(&[f32::NAN]).is_err());
        assert!(normalized(&[f32::INFINITY]).is_err());
        assert!(normalized(&vec![1.0; 8193]).is_err());
        for vector in [
            [f32::MAX, f32::MAX],
            [f32::MIN_POSITIVE, f32::MIN_POSITIVE],
            [3.0, 4.0],
        ] {
            let vector = normalized(&vector)?;
            let norm = vector.iter().map(|v| f64::from(*v).powi(2)).sum::<f64>();
            assert!((norm - 1.0).abs() < 1e-6);
            assert!(vector.iter().all(|v| v.is_finite()));
        }
        assert_eq!(normalized(&[f32::from_bits(1)])?, [1.0]);
        Ok(())
    }

    #[test]
    fn rrf_uses_rank_union_deduplicates_and_breaks_ties_by_chunk_id() {
        let combined = reciprocal_rank_fusion(
            vec![hit(1), hit(2), hit(2)],
            vec![
                SemanticHit {
                    hit: hit(2),
                    similarity: 0.9,
                },
                SemanticHit {
                    hit: hit(3),
                    similarity: 0.8,
                },
            ],
        );
        assert_eq!(
            combined.iter().map(|r| r.hit.chunk_id).collect::<Vec<_>>(),
            [2, 1, 3]
        );
        assert_eq!(combined[0].lexical_rank, Some(2));
        assert_eq!(combined[0].semantic_rank, Some(1));
        let tied = reciprocal_rank_fusion(
            vec![hit(7)],
            vec![SemanticHit {
                hit: hit(3),
                similarity: 1.0,
            }],
        );
        assert_eq!(tied[0].hit.chunk_id, 3);
        assert_eq!(tied[0].score, tied[1].score);
        assert_eq!(
            reciprocal_rank_fusion((0..60).map(hit).collect(), Vec::new()).len(),
            50
        );
    }
}
