use anyhow::{bail, Context, Result};
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use std::time::Duration;
use std::{collections::HashMap, path::Path};

pub struct Store {
    connection: Connection,
}

#[derive(Debug, serde::Serialize)]
pub struct SourceInfo {
    pub source_id: i64,
    pub root_path: String,
    pub can_write: bool,
}

#[derive(Debug, serde::Serialize)]
pub struct FullDocument {
    pub source_id: i64,
    pub path: String,
    pub content_hash: String,
    pub content: String,
}

/// A saved Markdown file may outlive a failed index update. Do not retry the write blindly.
#[derive(Debug, serde::Serialize)]
pub struct WriteResult {
    pub source_id: i64,
    pub path: String,
    pub content_hash: String,
    pub indexed: bool,
    pub index_error: Option<String>,
}

/// Counts committed document changes across the project's readable sources.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct IndexStats {
    pub added: usize,
    pub updated: usize,
    pub unchanged: usize,
    pub deleted: usize,
}

#[derive(Debug, serde::Serialize)]
pub struct SearchHit {
    pub chunk_id: i64,
    pub document_id: i64,
    pub source_id: i64,
    pub content_hash: String,
    pub path: String,
    pub heading: String,
    pub body: String,
    pub start_line: usize,
    pub end_line: usize,
}

impl Store {
    pub fn open(path: &Path) -> Result<Self> {
        let connection =
            Connection::open(path).with_context(|| format!("open {}", path.display()))?;
        connection.busy_timeout(Duration::from_secs(5))?;
        connection.execute_batch("PRAGMA foreign_keys = ON; PRAGMA journal_mode = WAL;")?;
        let version: i64 = connection.query_row("PRAGMA user_version", [], |r| r.get(0))?;
        match version {
            0 => {
                if let Err(error) =
                    connection.execute_batch(include_str!("../../../migrations/001_initial.sql"))
                {
                    let _ = connection.execute_batch("ROLLBACK");
                    return Err(error.into());
                }
            }
            1 => {}
            other => bail!("unsupported database version {other}"),
        }
        Ok(Self { connection })
    }

    pub fn create_project(&self, name: &str) -> Result<()> {
        self.connection
            .execute("INSERT INTO projects(name) VALUES (?1)", [name])?;
        Ok(())
    }

    fn project_id(&self, name: &str) -> Result<i64> {
        self.connection
            .query_row("SELECT id FROM projects WHERE name = ?1", [name], |r| {
                r.get(0)
            })
            .with_context(|| format!("unknown project: {name}"))
    }

    pub fn add_source(&self, project: &str, path: &Path) -> Result<()> {
        self.register_source(project, path, false).map(|_| ())
    }

    /// Owner-only registration: explicitly set the binding's write permission.
    pub fn register_source(&self, project: &str, path: &Path, writable: bool) -> Result<i64> {
        let project_id = self.project_id(project)?;
        let root = path
            .canonicalize()
            .with_context(|| format!("resolve {}", path.display()))?;
        if !root.is_dir() {
            bail!("source is not a directory: {}", root.display());
        }
        let root = root.to_str().context("source path must be UTF-8")?;
        let tx = self.connection.unchecked_transaction()?;
        tx.execute(
            "INSERT INTO sources(root_path) VALUES (?1) ON CONFLICT(root_path) DO NOTHING",
            [root],
        )?;
        let source_id: i64 = tx.query_row(
            "SELECT id FROM sources WHERE root_path = ?1",
            [root],
            |row| row.get(0),
        )?;
        tx.execute("INSERT INTO project_sources(project_id, source_id, can_write) VALUES (?1, ?2, ?3) ON CONFLICT(project_id, source_id) DO UPDATE SET can_write = excluded.can_write", params![project_id, source_id, writable])?;
        tx.commit()?;
        Ok(source_id)
    }

    pub fn sources(&self, project: &str) -> Result<Vec<SourceInfo>> {
        let project_id = self.project_id(project)?;
        let mut statement = self.connection.prepare("SELECT s.id, s.root_path, ps.can_write FROM sources s JOIN project_sources ps ON ps.source_id = s.id WHERE ps.project_id = ?1 AND ps.can_read = 1 ORDER BY s.id")?;
        let rows = statement.query_map([project_id], |row| {
            Ok(SourceInfo {
                source_id: row.get(0)?,
                root_path: row.get(1)?,
                can_write: row.get(2)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// File publication and SQLite cannot be one atomic transaction: Markdown is authoritative.
    pub fn write(
        &mut self,
        project: &str,
        source_id: i64,
        path: &str,
        content: &str,
        expected_hash: Option<&str>,
    ) -> Result<WriteResult> {
        let project_id = self.project_id(project)?;
        // Serialize with permission changes and SQLite writers until file publication completes.
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let root: String = tx.query_row("SELECT s.root_path FROM sources s JOIN project_sources ps ON ps.source_id = s.id WHERE s.id = ?1 AND ps.project_id = ?2 AND ps.can_read = 1 AND ps.can_write = 1", params![source_id, project_id], |row| row.get(0)).optional()?.context("source not found or write access denied")?;
        let mode = match expected_hash {
            Some(expected_hash) => memory_core::WriteMode::Update { expected_hash },
            None => memory_core::WriteMode::Create,
        };
        let content_hash = memory_core::write_markdown(Path::new(&root), path, content, mode)?;
        let refresh = tx
            .commit()
            .map_err(anyhow::Error::from)
            .and_then(|_| self.index(project));
        Ok(WriteResult {
            source_id,
            path: path.to_owned(),
            content_hash,
            indexed: refresh.is_ok(),
            index_error: refresh.err().map(|error| format!("{error:#}")),
        })
    }

    /// Reconcile readable sources atomically; failed scans preserve the previous index.
    pub fn index(&mut self, project: &str) -> Result<IndexStats> {
        let project_id = self.project_id(project)?;
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let sources = {
            let mut statement = tx.prepare("SELECT s.id, s.root_path FROM sources s JOIN project_sources ps ON ps.source_id = s.id WHERE ps.project_id = ?1 AND ps.can_read = 1 ORDER BY s.id")?;
            let rows = statement.query_map([project_id], |r| {
                Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?))
            })?;
            rows.collect::<rusqlite::Result<Vec<_>>>()?
        };
        let mut stats = IndexStats::default();
        for (source_id, root) in sources {
            let documents = memory_core::read_source(Path::new(&root))?;
            let mut existing = {
                let mut statement = tx.prepare(
                    "SELECT relative_path, id, content_hash FROM documents WHERE source_id = ?1",
                )?;
                let rows = statement.query_map([source_id], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        (row.get::<_, i64>(1)?, row.get::<_, String>(2)?),
                    ))
                })?;
                rows.collect::<rusqlite::Result<HashMap<_, _>>>()?
            };
            for document in documents {
                let previous = existing.remove(&document.relative_path);
                if previous
                    .as_ref()
                    .is_some_and(|(_, hash)| *hash == document.content_hash)
                {
                    stats.unchanged += 1;
                    continue;
                }
                let byte_size = i64::try_from(document.byte_size).context("document too large")?;
                let title = document
                    .chunks
                    .first()
                    .map(|c| c.heading.as_str())
                    .unwrap_or("");
                let document_id = if let Some((id, _)) = previous {
                    tx.execute("UPDATE documents SET title = ?1, content_hash = ?2, file_mtime_ns = ?3, byte_size = ?4, indexed_at = unixepoch() WHERE id = ?5", params![title, document.content_hash, document.file_mtime_ns, byte_size, id])?;
                    tx.execute("DELETE FROM document_chunks WHERE document_id = ?1", [id])?;
                    stats.updated += 1;
                    id
                } else {
                    tx.execute("INSERT INTO documents(source_id, relative_path, title, content_hash, file_mtime_ns, byte_size) VALUES (?1, ?2, ?3, ?4, ?5, ?6)", params![source_id, document.relative_path, title, document.content_hash, document.file_mtime_ns, byte_size])?;
                    stats.added += 1;
                    tx.last_insert_rowid()
                };
                for (ordinal, chunk) in document.chunks.iter().enumerate() {
                    tx.execute("INSERT INTO document_chunks(document_id, ordinal, heading, body, start_line, end_line) VALUES (?1, ?2, ?3, ?4, ?5, ?6)", params![document_id, ordinal as i64, chunk.heading, chunk.body, chunk.start_line as i64, chunk.end_line as i64])?;
                }
            }
            for (id, _) in existing.into_values() {
                tx.execute("DELETE FROM documents WHERE id = ?1", [id])?;
                stats.deleted += 1;
            }
        }
        tx.commit()?;
        Ok(stats)
    }

    pub fn search(&self, project: &str, query: &str) -> Result<Vec<SearchHit>> {
        let project_id = self.project_id(project)?;
        let mut statement = self.connection.prepare("SELECT d.relative_path, c.heading, c.body, c.start_line, c.end_line, c.id, d.id, d.source_id, d.content_hash FROM document_chunks_fts JOIN document_chunks c ON c.id = document_chunks_fts.rowid JOIN documents d ON d.id = c.document_id WHERE document_chunks_fts MATCH ?1 AND EXISTS (SELECT 1 FROM project_sources ps WHERE ps.source_id = d.source_id AND ps.project_id = ?2 AND ps.can_read = 1) ORDER BY bm25(document_chunks_fts), d.relative_path, c.ordinal LIMIT 50")?;
        let hits = statement.query_map(params![query, project_id], |row| {
            Ok(SearchHit {
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
        Ok(hits.collect::<rusqlite::Result<_>>()?)
    }
    /// Resolve an indexed chunk only inside the project's readable sources.
    /// The hash prevents stale chunk IDs from returning a different document revision.
    pub fn get(
        &self,
        project: &str,
        chunk_id: i64,
        content_hash: &str,
    ) -> Result<Option<SearchHit>> {
        let project_id = self.project_id(project)?;
        Ok(self.connection.query_row(
            "SELECT d.relative_path, c.heading, c.body, c.start_line, c.end_line, c.id, d.id, d.source_id, d.content_hash FROM document_chunks c JOIN documents d ON d.id = c.document_id WHERE c.id = ?1 AND d.content_hash = ?2 AND EXISTS (SELECT 1 FROM project_sources ps WHERE ps.source_id = d.source_id AND ps.project_id = ?3 AND ps.can_read = 1)",
            params![chunk_id, content_hash, project_id],
            |row| Ok(SearchHit {
                path: row.get(0)?, heading: row.get(1)?, body: row.get(2)?,
                start_line: row.get::<_, i64>(3)? as usize,
                end_line: row.get::<_, i64>(4)? as usize,
                chunk_id: row.get(5)?, document_id: row.get(6)?,
                source_id: row.get(7)?, content_hash: row.get(8)?,
            }),
        ).optional()?)
    }

    /// Full original Markdown (including frontmatter), authorized via the chunk scope.
    pub fn get_document(
        &self,
        project: &str,
        chunk_id: i64,
        content_hash: &str,
    ) -> Result<Option<FullDocument>> {
        let Some(hit) = self.get(project, chunk_id, content_hash)? else {
            return Ok(None);
        };
        let project_id = self.project_id(project)?;
        let root: String = self.connection.query_row("SELECT s.root_path FROM sources s JOIN project_sources ps ON ps.source_id = s.id WHERE s.id = ?1 AND ps.project_id = ?2 AND ps.can_read = 1", params![hit.source_id, project_id], |row| row.get(0))?;
        let content = memory_core::read_markdown(Path::new(&root), &hit.path)?;
        anyhow::ensure!(
            memory_core::content_hash(content.as_bytes()) == content_hash,
            "document changed on disk; repeat memory_search"
        );
        Ok(Some(FullDocument {
            source_id: hit.source_id,
            path: hit.path,
            content_hash: hit.content_hash,
            content,
        }))
    }
}
