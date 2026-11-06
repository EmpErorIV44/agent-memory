//! Scoped discovery and bounded full-document reads. No Markdown writes.
use crate::{MemorySummary, MemorySummaryMetadata, Store};
use anyhow::{ensure, Context, Result};
use rusqlite::params;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

#[derive(Debug, Default, Clone)]
pub struct MemoryListOptions {
    pub kind: Option<String>,
    pub status: Option<String>,
    pub tag: Option<String>,
    pub source_id: Option<i64>,
    pub limit: Option<usize>,
    pub offset: Option<usize>,
    pub include_details: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DocumentSelector {
    pub source_id: i64,
    pub path: String,
    pub content_hash: String,
}

impl Store {
    /// Optional pagination retains the legacy envelope when neither limit nor offset is supplied.
    pub fn memory_list(&self, project: &str, options: &MemoryListOptions) -> Result<Value> {
        let project_id = self.project_id(project)?;
        if let Some(kind) = &options.kind {
            serde_json::from_value::<memory_core::MemoryKind>(json!(kind))
                .context("invalid memory kind")?;
        }
        if let Some(status) = &options.status {
            serde_json::from_value::<memory_core::MemoryStatus>(json!(status))
                .context("invalid memory status")?;
        }
        if let Some(tag) = &options.tag {
            ensure!(
                !tag.trim().is_empty() && tag.len() <= 256,
                "tag must be nonempty and at most 256 UTF-8 bytes"
            );
        }
        let paged = options.limit.is_some() || options.offset.is_some();
        let limit = options.limit.unwrap_or(50);
        ensure!((1..=200).contains(&limit), "limit must be 1..200");
        let offset = i64::try_from(options.offset.unwrap_or(0)).context("offset is too large")?;
        let sql_limit = if paged { (limit + 1) as i64 } else { -1 };
        let mut query = self.connection.prepare(
            "SELECT m.memory_id,m.kind,m.status,m.summary,d.title,d.source_id,d.relative_path,d.content_hash FROM memories m JOIN documents d ON d.id=m.document_id WHERE m.project_id=?1 AND EXISTS(SELECT 1 FROM project_sources ps WHERE ps.source_id=d.source_id AND ps.project_id=?1 AND ps.can_read=1) AND (?2 IS NULL OR m.kind=?2) AND (?3 IS NULL OR m.status=?3) AND (?4 IS NULL OR EXISTS(SELECT 1 FROM json_each(d.tags_json) WHERE value=?4)) AND (?5 IS NULL OR d.source_id=?5) ORDER BY m.memory_id LIMIT ?6 OFFSET ?7")?;
        let rows = query.query_map(
            params![
                project_id,
                options.kind,
                options.status,
                options.tag,
                options.source_id,
                sql_limit,
                offset
            ],
            |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, String>(3)?,
                    r.get::<_, String>(4)?,
                    r.get::<_, i64>(5)?,
                    r.get::<_, String>(6)?,
                    r.get::<_, String>(7)?,
                ))
            },
        )?;
        let mut summaries = Vec::new();
        for row in rows {
            let (id, kind, status, summary, title, source_id, path, content_hash) = row?;
            summaries.push(MemorySummary {
                memory: MemorySummaryMetadata {
                    id,
                    kind: serde_json::from_value(json!(kind))?,
                    status: serde_json::from_value(json!(status))?,
                    summary,
                },
                title,
                source_id,
                path,
                content_hash,
            });
        }
        let has_more = paged && summaries.len() > limit;
        if has_more {
            summaries.truncate(limit);
        }
        let mut entries = Vec::with_capacity(summaries.len());
        for summary in summaries {
            entries.push(if options.include_details {
                serde_json::to_value(
                    self.get_memory(project, &summary.memory.id)?
                        .context("memory disappeared during list")?,
                )?
            } else {
                serde_json::to_value(summary)?
            });
        }
        let count = entries.len();
        let mut value = json!({"memories":entries});
        if paged {
            value["limit"] = json!(limit);
            value["offset"] = json!(offset);
            value["has_more"] = json!(has_more);
            value["next_offset"] = if has_more {
                json!(options
                    .offset
                    .unwrap_or(0)
                    .checked_add(count)
                    .context("offset overflow")?)
            } else {
                Value::Null
            };
        }
        Ok(value)
    }

    /// Total compact JSON byte budget includes every result, metadata and the byte counter itself.
    pub fn get_many(
        &self,
        project: &str,
        documents: &[DocumentSelector],
        max_bytes: usize,
    ) -> Result<Value> {
        self.project_id(project)?;
        ensure!(
            (1..=50).contains(&documents.len()),
            "documents must contain 1..50 selectors"
        );
        ensure!(
            (4096..=1_048_576).contains(&max_bytes),
            "max_bytes must be 4096..1048576"
        );
        let mut seen = std::collections::HashSet::new();
        for d in documents {
            ensure!(
                d.path.len() <= 4096
                    && d.content_hash.len() == 64
                    && d.content_hash.bytes().all(|c| c.is_ascii_hexdigit()),
                "invalid document selector"
            );
            ensure!(
                seen.insert((d.source_id, &d.path)),
                "duplicate document selector"
            );
        }
        let mut report = json!({"results":documents.iter().enumerate().map(|(index,_)|json!({"index":index,"status":"omitted_budget"})).collect::<Vec<_>>(),
            "max_bytes":max_bytes,"used_bytes":max_bytes,"omitted_count":documents.len(),"unavailable_count":documents.len(),"complete":false});
        ensure!(
            serde_json::to_vec(&report)?.len() <= max_bytes,
            "result metadata exceeds max_bytes"
        );
        for (index, selector) in documents.iter().enumerate() {
            match self.get_document_by_path(
                project,
                selector.source_id,
                &selector.path,
                &selector.content_hash,
            ) {
                Ok(Some(document)) => {
                    report["results"][index] =
                        json!({"index":index,"status":"ok","document":document});
                    if serde_json::to_vec(&report)?.len() > max_bytes {
                        report["results"][index] = json!({"index":index,"status":"omitted_budget"});
                    }
                }
                _ => report["results"][index] = json!({"index":index,"status":"unavailable"}),
            }
        }
        let results = report["results"].as_array().unwrap();
        let omitted = results
            .iter()
            .filter(|r| r["status"] == "omitted_budget")
            .count();
        let unavailable = results
            .iter()
            .filter(|r| r["status"] == "unavailable")
            .count();
        report["omitted_count"] = json!(omitted);
        report["unavailable_count"] = json!(unavailable);
        report["complete"] = json!(omitted == 0 && unavailable == 0);
        loop {
            let size = serde_json::to_vec(&report)?.len();
            if report["used_bytes"] == size {
                break;
            }
            report["used_bytes"] = json!(size);
        }
        Ok(report)
    }
}
