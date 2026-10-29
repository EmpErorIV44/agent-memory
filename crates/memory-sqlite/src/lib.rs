use anyhow::{bail, Context, Result};
use rusqlite::{params, Connection, TransactionBehavior};
use std::path::Path;
use std::time::Duration;

pub struct Store {
    connection: Connection,
}

#[derive(Debug)]
pub struct SearchHit {
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
        tx.execute("INSERT INTO project_sources(project_id, source_id) SELECT ?1, id FROM sources WHERE root_path = ?2 ON CONFLICT DO NOTHING", params![project_id, root])?;
        tx.commit()?;
        Ok(())
    }

    /// Rebuild all readable sources of a project atomically.
    pub fn index(&mut self, project: &str) -> Result<usize> {
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
        let mut count = 0;
        for (source_id, root) in sources {
            let documents = memory_core::read_source(Path::new(&root))?;
            tx.execute("DELETE FROM documents WHERE source_id = ?1", [source_id])?;
            for document in documents {
                let byte_size = i64::try_from(document.byte_size).context("document too large")?;
                let title = document
                    .chunks
                    .first()
                    .map(|c| c.heading.as_str())
                    .unwrap_or("");
                tx.execute("INSERT INTO documents(source_id, relative_path, title, content_hash, file_mtime_ns, byte_size) VALUES (?1, ?2, ?3, ?4, ?5, ?6)", params![source_id, document.relative_path, title, document.content_hash, document.file_mtime_ns, byte_size])?;
                let document_id = tx.last_insert_rowid();
                for (ordinal, chunk) in document.chunks.iter().enumerate() {
                    tx.execute("INSERT INTO document_chunks(document_id, ordinal, heading, body, start_line, end_line) VALUES (?1, ?2, ?3, ?4, ?5, ?6)", params![document_id, ordinal as i64, chunk.heading, chunk.body, chunk.start_line as i64, chunk.end_line as i64])?;
                }
                count += 1;
            }
        }
        tx.commit()?;
        Ok(count)
    }

    pub fn search(&self, project: &str, query: &str) -> Result<Vec<SearchHit>> {
        let project_id = self.project_id(project)?;
        let mut statement = self.connection.prepare("SELECT d.relative_path, c.heading, c.body, c.start_line, c.end_line FROM document_chunks_fts JOIN document_chunks c ON c.id = document_chunks_fts.rowid JOIN documents d ON d.id = c.document_id WHERE document_chunks_fts MATCH ?1 AND EXISTS (SELECT 1 FROM project_sources ps WHERE ps.source_id = d.source_id AND ps.project_id = ?2 AND ps.can_read = 1) ORDER BY bm25(document_chunks_fts), d.relative_path, c.ordinal LIMIT 50")?;
        let hits = statement.query_map(params![query, project_id], |row| {
            Ok(SearchHit {
                path: row.get(0)?,
                heading: row.get(1)?,
                body: row.get(2)?,
                start_line: row.get::<_, i64>(3)? as usize,
                end_line: row.get::<_, i64>(4)? as usize,
            })
        })?;
        Ok(hits.collect::<rusqlite::Result<_>>()?)
    }
}
