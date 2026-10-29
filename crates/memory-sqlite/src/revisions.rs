//! Authorization boundary for durable source history. SQLite stores no revision data.

use std::path::Path;

use anyhow::{Context, Result};
use rusqlite::{params, OptionalExtension};

use crate::{Revision, RevisionDocument, Store, WriteResult};

impl Store {
    /// Resolve an explicitly registered source only after checking current project rights.
    /// Keep unknown projects/sources and inaccessible sources indistinguishable.
    fn revision_source_root(&self, project: &str, source_id: i64, write: bool) -> Result<String> {
        self.connection
            .query_row(
                "SELECT s.root_path FROM sources s JOIN project_sources ps ON ps.source_id=s.id JOIN projects p ON p.id=ps.project_id WHERE s.id=?1 AND p.name=?2 AND ps.can_read=1 AND (?3=0 OR ps.can_write=1)",
                params![source_id, project, write],
                |row| row.get(0),
            )
            .optional()?
            .context("source not found or access denied")
    }

    /// List durable revisions in chronological order within one currently readable source.
    pub fn list_revisions(
        &self,
        project: &str,
        source_id: i64,
        path: &str,
    ) -> Result<Vec<Revision>> {
        let root = self.revision_source_root(project, source_id, false)?;
        memory_core::list_revisions(Path::new(&root), path)
    }

    /// Read the complete historical document; guessed IDs never bypass the source scope.
    pub fn get_revision(
        &self,
        project: &str,
        source_id: i64,
        path: &str,
        revision_id: &str,
    ) -> Result<Option<RevisionDocument>> {
        let root = self.revision_source_root(project, source_id, false)?;
        memory_core::get_revision(Path::new(&root), path, revision_id)
    }

    /// Restore a historical full document through the normal guarded write pipeline.
    /// Current source write permission and exact current-file hash are required.
    /// Restoration appends a revision; the source history is never truncated.
    pub fn restore_revision(
        &mut self,
        project: &str,
        source_id: i64,
        path: &str,
        revision_id: &str,
        expected_hash: &str,
    ) -> Result<WriteResult> {
        let root = self.revision_source_root(project, source_id, true)?;
        let historical = memory_core::get_revision(Path::new(&root), path, revision_id)?
            .context("revision not found or inaccessible")?;
        // Recheck current SQL permissions in Store::write. Its compare-and-update guard
        // also detects an intervening writer after the historical read releases its lock.
        self.write(
            project,
            source_id,
            path,
            &historical.content,
            Some(expected_hash),
        )
    }
}
