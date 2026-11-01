use crate::Store;
use anyhow::{ensure, Result};
use rusqlite::{params, Transaction};
use serde::Serialize;

#[derive(Debug, Serialize)]
pub struct Change {
    pub sequence: i64,
    /// Available only while this exact document state is still current.
    pub chunk_id: Option<i64>,
    pub revision: i64,
    pub document_id: i64,
    pub source_id: i64,
    pub kind: String,
    pub path: String,
    pub previous_path: Option<String>,
    pub content_hash: String,
    pub stable_id: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct Changes {
    pub has_more: bool,
    pub cursor: String,
    pub revision: i64,
    pub changes: Vec<Change>,
}

/// Compare project observations independently of shared document indexing.
pub(crate) fn capture(tx: &Transaction<'_>, project: i64) -> Result<()> {
    tx.execute(
        "INSERT OR IGNORE INTO project_clocks(project_id) VALUES (?1)",
        [project],
    )?;
    tx.execute_batch("CREATE TEMP TABLE IF NOT EXISTS observed_documents (document_id INTEGER PRIMARY KEY, source_id INTEGER, path TEXT, content_hash TEXT, stable_id TEXT); DELETE FROM observed_documents;")?;
    tx.execute("INSERT INTO observed_documents SELECT d.id,d.source_id,d.relative_path,d.content_hash,d.stable_id FROM documents d JOIN project_sources ps ON ps.source_id=d.source_id WHERE ps.project_id=?1 AND ps.can_read=1", [project])?;
    let changed: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM observed_documents n LEFT JOIN project_snapshot o ON o.project_id=?1 AND o.document_id=n.document_id WHERE o.document_id IS NULL OR o.path<>n.path OR o.content_hash<>n.content_hash OR o.stable_id IS NOT n.stable_id) OR EXISTS(SELECT 1 FROM project_snapshot o WHERE o.project_id=?1 AND NOT EXISTS(SELECT 1 FROM observed_documents n WHERE n.document_id=o.document_id))", [project], |r| r.get(0))?;
    if !changed {
        return Ok(());
    }
    tx.execute(
        "UPDATE project_clocks SET revision=revision+1 WHERE project_id=?1",
        [project],
    )?;
    tx.execute("INSERT INTO project_changes(project_id,revision,document_id,source_id,kind,path,previous_path,content_hash,stable_id) SELECT ?1,c.revision,n.document_id,n.source_id,CASE WHEN o.document_id IS NULL THEN 'added' WHEN o.path<>n.path THEN 'renamed' ELSE 'updated' END,n.path,o.path,n.content_hash,n.stable_id FROM observed_documents n JOIN project_clocks c ON c.project_id=?1 LEFT JOIN project_snapshot o ON o.project_id=?1 AND o.document_id=n.document_id WHERE o.document_id IS NULL OR o.path<>n.path OR o.content_hash<>n.content_hash OR o.stable_id IS NOT n.stable_id", [project])?;
    tx.execute("INSERT INTO project_changes(project_id,revision,document_id,source_id,kind,path,previous_path,content_hash,stable_id) SELECT ?1,c.revision,o.document_id,o.source_id,'deleted',o.path,NULL,o.content_hash,o.stable_id FROM project_snapshot o JOIN project_clocks c ON c.project_id=?1 WHERE o.project_id=?1 AND NOT EXISTS(SELECT 1 FROM observed_documents n WHERE n.document_id=o.document_id)", [project])?;
    tx.execute(
        "DELETE FROM project_snapshot WHERE project_id=?1",
        [project],
    )?;
    tx.execute("INSERT INTO project_snapshot SELECT ?1,document_id,source_id,path,content_hash,stable_id FROM observed_documents", [project])?;
    Ok(())
}

impl Store {
    /// Refresh and read a consistent project journal. None establishes a baseline.
    pub fn changes(&mut self, project: &str, since: Option<&str>) -> Result<Changes> {
        self.index(project)?;
        let project_id = self.project_id(project)?;
        let tx = self.connection.transaction()?;
        let (epoch, revision): (String, i64) = tx.query_row(
            "SELECT epoch,revision FROM project_clocks WHERE project_id=?1",
            [project_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        let latest: i64 = tx.query_row(
            "SELECT coalesce(max(sequence),0) FROM project_changes WHERE project_id=?1",
            [project_id],
            |r| r.get(0),
        )?;
        let start = if let Some(cursor) = since {
            let (prefix, number) = cursor.rsplit_once(':').ok_or_else(|| {
                anyhow::anyhow!("invalid changes cursor; establish a new baseline")
            })?;
            ensure!(
                prefix == epoch,
                "changes cursor belongs to another project or index; establish a new baseline"
            );
            let number: i64 = number.parse()?;
            ensure!((0..=latest).contains(&number), "invalid changes sequence");
            number
        } else {
            latest
        };
        let mut query = tx.prepare("SELECT c.revision,c.document_id,c.source_id,c.kind,c.path,c.previous_path,c.content_hash,c.stable_id,c.sequence,(SELECT min(ch.id) FROM document_chunks ch JOIN documents d ON d.id=ch.document_id WHERE d.id=c.document_id AND d.content_hash=c.content_hash AND d.relative_path=c.path AND c.kind<>'deleted') FROM project_changes c WHERE c.project_id=?1 AND c.sequence>?2 AND c.sequence<=?3 AND EXISTS(SELECT 1 FROM project_sources ps WHERE ps.project_id=c.project_id AND ps.source_id=c.source_id AND ps.can_read=1) ORDER BY c.sequence LIMIT 101")?;
        let mut changes = query
            .query_map(params![project_id, start, latest], |r| {
                Ok(Change {
                    sequence: r.get(8)?,
                    chunk_id: r.get(9)?,
                    revision: r.get(0)?,
                    document_id: r.get(1)?,
                    source_id: r.get(2)?,
                    kind: r.get(3)?,
                    path: r.get(4)?,
                    previous_path: r.get(5)?,
                    content_hash: r.get(6)?,
                    stable_id: r.get(7)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        drop(query);
        let has_more = changes.len() > 100;
        changes.truncate(100);
        let next = if has_more {
            changes.last().expect("full page").sequence
        } else {
            latest
        };
        tx.commit()?;
        Ok(Changes {
            has_more,
            cursor: format!("{epoch}:{next}"),
            revision,
            changes,
        })
    }
}
