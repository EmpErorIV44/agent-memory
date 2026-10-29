use crate::{
    config::{Config, ProjectConfig, SourceConfig},
    index, new_id, IndexStats, Store,
};
use anyhow::Result;
use rusqlite::{params, TransactionBehavior};
use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
};

impl Store {
    /// Owner-only configuration export, including unreadable and read-only bindings.
    pub fn export_config(&self) -> Result<Config> {
        let projects = {
            let mut query = self
                .connection
                .prepare("SELECT id, name FROM projects ORDER BY name")?;
            let rows = query.query_map([], |row| {
                Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
            })?;
            rows.collect::<rusqlite::Result<Vec<_>>>()?
        };
        let mut result = Vec::new();
        for (id, name) in projects {
            let mut query = self.connection.prepare(
                "SELECT s.root_path, ps.can_read, ps.can_write FROM sources s JOIN project_sources ps ON ps.source_id=s.id WHERE ps.project_id=?1 ORDER BY s.root_path")?;
            let sources = query
                .query_map([id], |row| {
                    Ok(SourceConfig {
                        path: PathBuf::from(row.get::<_, String>(0)?),
                        can_read: row.get(1)?,
                        can_write: row.get(2)?,
                    })
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            result.push(ProjectConfig { name, sources });
        }
        Ok(Config {
            version: 1,
            projects: result,
        })
    }

    /// Atomically make the owner manifest authoritative and reconstruct every readable source.
    /// Existing registrations and unchanged documents retain their IDs. On any scan/index error
    /// the previous registration, permissions, Markdown projection and FTS remain intact.
    pub fn rebuild_config(&mut self, configuration: &Config) -> Result<IndexStats> {
        let configuration = configuration.normalized()?;
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let mut projects = {
            let mut query = tx.prepare("SELECT name, id FROM projects")?;
            let rows = query.query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
            })?;
            rows.collect::<rusqlite::Result<HashMap<_, _>>>()?
        };
        let mut sources = {
            let mut query = tx.prepare("SELECT root_path, id FROM sources")?;
            let rows = query.query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
            })?;
            rows.collect::<rusqlite::Result<HashMap<_, _>>>()?
        };
        let mut retained_projects = HashSet::new();
        for project in &configuration.projects {
            let project_id = if let Some(id) = projects.get(&project.name) {
                *id
            } else {
                let id = new_id(&tx, "projects")?;
                tx.execute(
                    "INSERT INTO projects(id, name) VALUES (?1,?2)",
                    params![id, project.name],
                )?;
                projects.insert(project.name.clone(), id);
                id
            };
            retained_projects.insert(project_id);
            // Reset bindings, never infer them from author-controlled Markdown.
            tx.execute(
                "DELETE FROM project_sources WHERE project_id=?1",
                [project_id],
            )?;
            for source in &project.sources {
                let root = source.path.to_str().expect("normalized UTF-8 path");
                let source_id = if let Some(id) = sources.get(root) {
                    *id
                } else {
                    let id = new_id(&tx, "sources")?;
                    tx.execute(
                        "INSERT INTO sources(id, root_path) VALUES (?1,?2)",
                        params![id, root],
                    )?;
                    sources.insert(root.to_owned(), id);
                    id
                };
                tx.execute("INSERT INTO project_sources(project_id, source_id, can_read, can_write) VALUES (?1,?2,?3,?4)",
                    params![project_id, source_id, source.can_read, source.can_write])?;
            }
        }
        let before_removal: i64 =
            tx.query_row("SELECT count(*) FROM documents", [], |row| row.get(0))?;
        for id in projects
            .values()
            .filter(|id| !retained_projects.contains(*id))
        {
            tx.execute("DELETE FROM projects WHERE id=?1", [id])?;
        }
        tx.execute("DELETE FROM sources WHERE NOT EXISTS (SELECT 1 FROM project_sources ps WHERE ps.source_id=sources.id)", [])?;
        tx.execute("DELETE FROM documents WHERE NOT EXISTS (SELECT 1 FROM project_sources ps WHERE ps.source_id=documents.source_id AND ps.can_read=1)", [])?;
        let after_removal: i64 =
            tx.query_row("SELECT count(*) FROM documents", [], |row| row.get(0))?;
        let readable = {
            let mut query = tx.prepare("SELECT s.id, s.root_path FROM sources s WHERE EXISTS (SELECT 1 FROM project_sources ps WHERE ps.source_id=s.id AND ps.can_read=1) ORDER BY s.id")?;
            let rows = query.query_map([], |row| {
                Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
            })?;
            rows.collect::<rusqlite::Result<Vec<_>>>()?
        };
        let mut stats = index::index_sources(&tx, &readable, None)?;
        stats.deleted += usize::try_from(before_removal - after_removal)?;
        tx.commit()?;
        Ok(stats)
    }
}
