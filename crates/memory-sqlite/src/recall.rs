//! Explicit recall tiers in existing document tags; no schema migration or inference.
use crate::{MemoryRecord, MemorySummaryMetadata, SearchOptions, Store};
use anyhow::{ensure, Result};
use memory_core::{MemoryKind, MemoryStatus};
use serde_json::json;
use std::collections::{BTreeSet, HashMap, HashSet};

fn tagged(tags: &[String], tag: &str) -> bool {
    tags.iter().any(|value| value == tag)
}

fn required_fields(kind: &MemoryKind, status: &MemoryStatus, tags: &[String]) -> bool {
    *status == MemoryStatus::Accepted
        && !tagged(tags, "recall:task")
        && (tagged(tags, "recall:always")
            || *kind == MemoryKind::Constraint
            || tagged(tags, "preference")
            || tagged(tags, "preferences"))
}

pub(crate) fn required(record: &MemoryRecord) -> bool {
    required_fields(&record.memory.kind, &record.memory.status, &record.tags)
}

/// Lightweight indexed metadata, not a hydrated Memory/Evidence/Relation record.
#[derive(Debug)]
pub struct RecallRecord {
    pub memory: MemorySummaryMetadata,
    pub document_id: i64,
    pub source_id: i64,
    pub path: String,
    pub content_hash: String,
    pub tags: Vec<String>,
}

fn words(text: &str) -> BTreeSet<String> {
    crate::search_metadata::russian(&text.to_lowercase())
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(str::to_owned)
        .collect()
}

pub struct RecallPlan {
    pub required: Vec<RecallRecord>,
    pub preferences: Vec<RecallRecord>,
    pub matched_count: usize,
    pub deferred_count: usize,
    /// Prevent deferred preferences from leaking back into general task excerpts.
    pub managed_keys: HashSet<(i64, String)>,
}

impl Store {
    /// Select explicit task preferences from indexed metadata. No full-note reads.
    /// Topic tags override body matching; untagged legacy baselines remain required.
    pub fn recall_plan(
        &self,
        project: &str,
        task: Option<&str>,
        limit: usize,
    ) -> Result<RecallPlan> {
        ensure!(limit <= 20, "preference_limit must be 0..20");
        if let Some(task) = task {
            ensure!(
                !task.trim().is_empty() && task.len() <= 8192,
                "task must contain 1..8192 UTF-8 bytes"
            );
        }
        let project_id = self.project_id(project)?;
        let mut statement = self.connection.prepare(
            "SELECT m.memory_id,m.kind,m.summary,d.id,d.source_id,d.relative_path,d.content_hash,d.tags_json FROM memories m JOIN documents d ON d.id=m.document_id WHERE m.project_id=?1 AND m.status='accepted' AND EXISTS(SELECT 1 FROM project_sources ps WHERE ps.project_id=?1 AND ps.source_id=d.source_id AND ps.can_read=1) ORDER BY m.memory_id"
        )?;
        let rows = statement.query_map([project_id], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, i64>(3)?,
                r.get::<_, i64>(4)?,
                r.get::<_, String>(5)?,
                r.get::<_, String>(6)?,
                r.get::<_, String>(7)?,
            ))
        })?;
        let mut records = Vec::new();
        for row in rows {
            let (id, kind, summary, document_id, source_id, path, content_hash, tags) = row?;
            records.push(RecallRecord {
                memory: MemorySummaryMetadata {
                    id,
                    kind: serde_json::from_value(json!(kind))?,
                    status: MemoryStatus::Accepted,
                    summary,
                },
                document_id,
                source_id,
                path,
                content_hash,
                tags: serde_json::from_str(&tags)?,
            });
        }
        let mut required_records = Vec::new();
        let mut contextual_records = Vec::new();
        let mut managed_keys = HashSet::new();
        for record in records {
            if required_fields(&record.memory.kind, &record.memory.status, &record.tags) {
                managed_keys.insert((record.source_id, record.path.clone()));
                required_records.push(record);
            } else if tagged(&record.tags, "recall:task") {
                managed_keys.insert((record.source_id, record.path.clone()));
                contextual_records.push(record);
            }
        }
        required_records
            .sort_by_key(|r| (r.memory.kind != MemoryKind::Constraint, r.memory.id.clone()));
        let total = contextual_records.len();
        let mut matches = Vec::new();
        if let Some(task) = task {
            let task_words = words(task);
            // The existing lexical search remains bounded to 50 chunks. Explicit
            // topic matching uses metadata directly and is not lost in that window.
            let query = task
                .split(|c: char| !c.is_alphanumeric())
                .filter(|w| !w.is_empty())
                .take(128)
                .map(|w| format!("\"{w}\""))
                .collect::<Vec<_>>()
                .join(" OR ");
            let hits = if query.is_empty() {
                Vec::new()
            } else {
                self.search_filtered(project, &query, &SearchOptions::default())?
            };
            let mut ranks = HashMap::new();
            for (rank, hit) in hits.iter().enumerate() {
                ranks.entry(hit.document_id).or_insert(rank);
            }
            for record in contextual_records {
                let topics: Vec<_> = record
                    .tags
                    .iter()
                    .filter_map(|tag| tag.strip_prefix("topic:"))
                    .map(words)
                    .filter(|topic| !topic.is_empty())
                    .collect();
                let score = if topics.is_empty() {
                    // Use indexed chunk matches, not summaries of full documents.
                    hits.iter()
                        .filter(|hit| hit.document_id == record.document_id)
                        .map(|hit| {
                            task_words
                                .intersection(&words(&format!("{} {}", hit.heading, hit.body)))
                                .count()
                        })
                        .max()
                        .unwrap_or(0)
                } else {
                    topics
                        .iter()
                        .filter(|topic| topic.is_subset(&task_words))
                        .flat_map(|topic| topic.iter().cloned())
                        .collect::<BTreeSet<_>>()
                        .len()
                };
                let eligible = if topics.is_empty() {
                    ranks.contains_key(&record.document_id)
                } else {
                    score > 0
                };
                if eligible {
                    matches.push((
                        !topics.is_empty(),
                        score,
                        *ranks.get(&record.document_id).unwrap_or(&usize::MAX),
                        record,
                    ));
                }
            }
        }
        matches.sort_by(|a, b| {
            b.0.cmp(&a.0)
                .then(b.1.cmp(&a.1))
                .then(a.2.cmp(&b.2))
                .then(a.3.memory.id.cmp(&b.3.memory.id))
        });
        let matched_count = matches.len();
        let preferences: Vec<_> = matches
            .into_iter()
            .take(limit)
            .map(|(_, _, _, record)| record)
            .collect();
        Ok(RecallPlan {
            deferred_count: total - preferences.len(),
            required: required_records,
            preferences,
            matched_count,
            managed_keys,
        })
    }
}
