use crate::Store;
use anyhow::{Context, Result};
use memory_core::{Evidence, EvidenceKind, Memory, Relation};
use rusqlite::{params, OptionalExtension, Transaction};
use serde::{de::DeserializeOwned, Serialize};

/// A logical assertion and the readable Markdown document from which it was projected.
#[derive(Debug, Clone, serde::Serialize)]
pub struct MemoryRecord {
    pub memory: Memory,
    pub document_id: i64,
    pub source_id: i64,
    pub stable_id: Option<String>,
    pub path: String,
    pub content_hash: String,
}

fn encoded<T: Serialize>(value: &T) -> Result<String> {
    serde_json::to_value(value)?
        .as_str()
        .map(str::to_owned)
        .context("expected enum string")
}

fn decoded<T: DeserializeOwned>(value: String) -> Result<T> {
    Ok(serde_json::from_value(serde_json::Value::String(value))?)
}

pub(crate) fn insert(
    tx: &Transaction<'_>,
    document_id: i64,
    source_id: i64,
    memory: &Memory,
    project_scope: Option<i64>,
) -> Result<()> {
    memory.validate()?;
    // An author's project name cannot register a project or grant source access.
    let project_id: Option<i64> = tx.query_row(
        "SELECT p.id FROM projects p JOIN project_sources ps ON ps.project_id=p.id WHERE p.name=?1 AND ps.source_id=?2 AND ps.can_read=1",
        params![memory.project, source_id], |row| row.get(0)).optional()?;
    let Some(project_id) = project_id else {
        return Ok(());
    };
    // A normal project refresh must not reconcile foreign private-source projections.
    // Owner-only rebuild passes None and refreshes every explicitly registered project.
    if project_scope.is_some_and(|scope| scope != project_id) {
        return Ok(());
    }
    tx.execute("INSERT INTO memories(project_id, memory_id, document_id, kind, status, summary) VALUES (?1,?2,?3,?4,?5,?6)",
        params![project_id, memory.id, document_id, encoded(&memory.kind)?, encoded(&memory.status)?, memory.summary])
        .context("duplicate or invalid memory.id in registered project")?;
    for (ordinal, evidence) in memory.evidence.iter().enumerate() {
        tx.execute("INSERT INTO memory_evidence(project_id, memory_id, ordinal, kind, reference, description) VALUES (?1,?2,?3,?4,?5,?6)",
            params![project_id, memory.id, ordinal as i64, encoded(&evidence.kind)?, evidence.reference, evidence.description])?;
    }
    for (ordinal, relation) in memory.relations.iter().enumerate() {
        tx.execute("INSERT INTO memory_relations(project_id, memory_id, ordinal, kind, target_memory_id) VALUES (?1,?2,?3,?4,?5)",
            params![project_id, memory.id, ordinal as i64, encoded(&relation.kind)?, relation.target])?;
    }
    Ok(())
}

impl Store {
    /// Structured assertions only from readable sources and the explicitly named project.
    pub fn list_memories(&self, project: &str) -> Result<Vec<MemoryRecord>> {
        let project_id = self.project_id(project)?;
        let ids = {
            let mut query = self.connection.prepare(
                "SELECT m.memory_id FROM memories m JOIN documents d ON d.id=m.document_id WHERE m.project_id=?1 AND EXISTS (SELECT 1 FROM project_sources ps WHERE ps.source_id=d.source_id AND ps.project_id=?1 AND ps.can_read=1) ORDER BY m.memory_id")?;
            let rows = query.query_map([project_id], |row| row.get::<_, String>(0))?;
            rows.collect::<rusqlite::Result<Vec<_>>>()?
        };
        ids.iter()
            .map(|id| {
                self.get_memory(project, id)?
                    .context("memory disappeared during list")
            })
            .collect()
    }

    /// Direct lookup is authorized in SQL before any assertion, evidence or relation is read.
    pub fn get_memory(&self, project: &str, id: &str) -> Result<Option<MemoryRecord>> {
        let project_id = self.project_id(project)?;
        let row = self.connection.query_row(
            "SELECT m.kind, m.status, m.summary, d.id, d.source_id, d.stable_id, d.relative_path, d.content_hash FROM memories m JOIN documents d ON d.id=m.document_id WHERE m.project_id=?1 AND m.memory_id=?2 AND EXISTS (SELECT 1 FROM project_sources ps WHERE ps.source_id=d.source_id AND ps.project_id=?1 AND ps.can_read=1)",
            params![project_id, id], |row| Ok((
                row.get::<_, String>(0)?, row.get::<_, String>(1)?, row.get::<_, String>(2)?,
                row.get::<_, i64>(3)?, row.get::<_, i64>(4)?, row.get::<_, Option<String>>(5)?,
                row.get::<_, String>(6)?, row.get::<_, String>(7)?,
            ))).optional()?;
        let Some((kind, status, summary, document_id, source_id, stable_id, path, content_hash)) =
            row
        else {
            return Ok(None);
        };
        let evidence = {
            let mut query = self.connection.prepare(
                "SELECT kind, reference, description FROM memory_evidence WHERE project_id=?1 AND memory_id=?2 ORDER BY ordinal")?;
            let rows = query
                .query_map(params![project_id, id], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, Option<String>>(2)?,
                    ))
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            let mut result = Vec::new();
            for (kind, reference, description) in rows {
                let kind: EvidenceKind = decoded(kind)?;
                // Document references resolve only within readable sources of this project.
                // Other evidence is returned as an unverified author-provided reference;
                // no filesystem path, URL, commit or test is automatically opened/executed.
                if kind == EvidenceKind::Document {
                    let readable: bool = self.connection.query_row(
                        "SELECT EXISTS (SELECT 1 FROM documents d WHERE (d.relative_path=?1 OR d.stable_id=?1) AND EXISTS (SELECT 1 FROM project_sources ps WHERE ps.source_id=d.source_id AND ps.project_id=?2 AND ps.can_read=1))",
                        params![reference, project_id], |row| row.get(0))?;
                    if !readable {
                        continue;
                    }
                }
                result.push(Evidence {
                    kind,
                    reference,
                    description,
                });
            }
            result
        };
        let relations = {
            let mut query = self.connection.prepare(
                "SELECT r.kind, r.target_memory_id FROM memory_relations r WHERE r.project_id=?1 AND r.memory_id=?2 AND EXISTS (SELECT 1 FROM memories target JOIN documents d ON d.id=target.document_id WHERE target.project_id=?1 AND target.memory_id=r.target_memory_id AND EXISTS (SELECT 1 FROM project_sources ps WHERE ps.source_id=d.source_id AND ps.project_id=?1 AND ps.can_read=1)) ORDER BY r.ordinal")?;
            let rows = query
                .query_map(params![project_id, id], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            rows.into_iter()
                .map(|(kind, target)| {
                    Ok(Relation {
                        kind: decoded(kind)?,
                        target,
                    })
                })
                .collect::<Result<Vec<_>>>()?
        };
        Ok(Some(MemoryRecord {
            memory: Memory {
                id: id.to_owned(),
                project: project.to_owned(),
                kind: decoded(kind)?,
                status: decoded(status)?,
                summary,
                evidence,
                relations,
            },
            document_id,
            source_id,
            stable_id,
            path,
            content_hash,
        }))
    }
}
