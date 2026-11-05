//! Reviewable single-source batches with durable, conflict-aware roll-forward recovery.
use crate::Store;
use anyhow::{ensure, Context, Result};
use memory_core::{SourceWriter, WriteMode};
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{collections::HashSet, path::PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BatchEdit {
    pub path: String,
    pub content: String,
    /// None creates a new file without overwriting; updates require its full hash.
    pub expected_hash: Option<String>,
}
impl BatchEdit {
    fn mode(&self) -> WriteMode<'_> {
        match &self.expected_hash {
            Some(hash) => WriteMode::Update {
                expected_hash: hash,
            },
            None => WriteMode::Create,
        }
    }
    fn desired(&self) -> String {
        memory_core::content_hash(self.content.as_bytes())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BatchPlan {
    pub project: String,
    pub source_id: i64,
    pub edits: Vec<BatchEdit>,
    pub plan_hash: String,
}

#[derive(Debug, Serialize)]
pub struct BatchResult {
    pub operation_id: String,
    pub complete: bool,
    pub partial: bool,
    /// Files observed at their desired bytes, including recognized prior publications.
    pub applied: Vec<String>,
    pub error: Option<String>,
    pub indexed: bool,
    pub index_error: Option<String>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Journal {
    operation_kind: String,
    version: u32,
    nonce: String,
    plan: BatchPlan,
}

fn root(connection: &Connection, project: i64, source: i64, write: bool) -> Result<PathBuf> {
    let root: String = connection.query_row(
        "SELECT s.root_path FROM sources s JOIN project_sources ps ON ps.source_id=s.id WHERE ps.project_id=?1 AND s.id=?2 AND ps.can_read=1 AND (?3=0 OR ps.can_write=1)",
        params![project,source,write], |r| r.get(0)).optional()?.context("source not found or access denied")?;
    Ok(PathBuf::from(root))
}

fn validate_edits(project: &str, edits: &[BatchEdit]) -> Result<()> {
    ensure!(
        (1..=50).contains(&edits.len()),
        "batch must contain 1..50 edits"
    );
    ensure!(
        edits.iter().map(|e| e.content.len()).sum::<usize>() <= 4 * 1024 * 1024,
        "batch content exceeds 4 MiB"
    );
    let mut paths = HashSet::new();
    for (i, edit) in edits.iter().enumerate() {
        for other in &edits[i + 1..] {
            let a = edit.path.to_lowercase();
            let b = other.path.to_lowercase();
            ensure!(
                !a.starts_with(&(b.clone() + "/")) && !b.starts_with(&(a + "/")),
                "batch file cannot be another edit's parent directory"
            );
        }
    }
    for edit in edits {
        SourceWriter::validate(&edit.path, &edit.content, &edit.mode())?;
        ensure!(
            paths.insert(edit.path.to_lowercase()),
            "duplicate or case-aliased batch path"
        );
        let metadata = memory_core::parse_metadata(&edit.content)?;
        ensure!(
            metadata.memory.is_none_or(|m| m.project == project),
            "memory.project must match batch project"
        );
    }
    Ok(())
}

fn hash(plan: &BatchPlan) -> Result<String> {
    let mut copy = plan.clone();
    copy.plan_hash.clear();
    Ok(memory_core::content_hash(&serde_json::to_vec(&copy)?))
}

/// Validate the final IDs against current readable Markdown, not stale projections.
/// This also permits coordinated ID moves/swaps whose intermediate state duplicates IDs.
fn validate_final(connection: &Connection, project_id: i64, plan: &BatchPlan) -> Result<()> {
    let sources = {
        let mut query = connection.prepare("SELECT s.id,s.root_path FROM sources s JOIN project_sources ps ON ps.source_id=s.id WHERE ps.project_id=?1 AND ps.can_read=1 ORDER BY s.id")?;
        let rows = query.query_map([project_id], |r| {
            Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?))
        })?;
        rows.collect::<rusqlite::Result<Vec<_>>>()?
    };
    let mut memory_ids = HashSet::new();
    for (source_id, path) in sources {
        let mut metadata = Vec::new();
        for document in memory_core::read_source(std::path::Path::new(&path))? {
            if source_id == plan.source_id
                && plan.edits.iter().any(|e| e.path == document.relative_path)
            {
                ensure!(
                    document
                        .memory
                        .as_ref()
                        .is_none_or(|m| m.project == plan.project),
                    "batch cannot overwrite another project's structured memory"
                );
                continue;
            }
            metadata.push((document.stable_id, document.memory));
        }
        if source_id == plan.source_id {
            for edit in &plan.edits {
                let m = memory_core::parse_metadata(&edit.content)?;
                metadata.push((m.stable_id, m.memory));
            }
        }
        let mut document_ids = HashSet::new();
        for (id, memory) in metadata {
            if let Some(id) = id {
                ensure!(
                    document_ids.insert(id),
                    "duplicate document.id in final batch state"
                );
            }
            if let Some(m) = memory.filter(|m| m.project == plan.project) {
                ensure!(
                    memory_ids.insert(m.id),
                    "duplicate memory.id in final batch state"
                );
            }
        }
    }
    Ok(())
}

fn read_journal(root: &std::path::Path, id: &str, project: &str) -> Result<Journal> {
    let journal: Journal = serde_json::from_slice(&memory_core::read_operation(root, id)?)?;
    ensure!(
        journal.operation_kind == "batch_write"
            && journal.version == 1
            && journal.plan.project == project,
        "batch operation scope/version mismatch"
    );
    validate_edits(project, &journal.plan.edits)?;
    ensure!(
        hash(&journal.plan)? == journal.plan.plan_hash,
        "batch plan checksum mismatch"
    );
    Ok(journal)
}

impl Store {
    pub fn batch_preview(
        &self,
        project: &str,
        source_id: i64,
        mut edits: Vec<BatchEdit>,
    ) -> Result<BatchPlan> {
        validate_edits(project, &edits)?;
        let project_id = self.project_id(project)?;
        let root = root(&self.connection, project_id, source_id, false)?;
        edits.sort_by(|a, b| a.path.cmp(&b.path));
        for edit in &edits {
            ensure!(
                memory_core::markdown_hash(&root, &edit.path)? == edit.expected_hash,
                "batch conflict: {} changed or already exists",
                edit.path
            );
        }
        let mut plan = BatchPlan {
            project: project.into(),
            source_id,
            edits,
            plan_hash: String::new(),
        };
        validate_final(&self.connection, project_id, &plan)?;
        plan.plan_hash = hash(&plan)?;
        Ok(plan)
    }

    pub fn batch_apply(
        &mut self,
        project: &str,
        source_id: i64,
        edits: Vec<BatchEdit>,
        expected_plan_hash: &str,
    ) -> Result<BatchResult> {
        let plan = self.batch_preview(project, source_id, edits)?;
        ensure!(
            plan.plan_hash == expected_plan_hash,
            "batch plan changed; review a fresh preview"
        );
        self.batch_start(plan, |_| Ok(()))
    }

    fn batch_start(
        &mut self,
        plan: BatchPlan,
        mut stage: impl FnMut(&str) -> Result<()>,
    ) -> Result<BatchResult> {
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let project_id = tx.query_row(
            "SELECT id FROM projects WHERE name=?1",
            [&plan.project],
            |r| r.get::<_, i64>(0),
        )?;
        let root = root(&tx, project_id, plan.source_id, true)?;
        let writer = SourceWriter::open(&root)?;
        for edit in &plan.edits {
            ensure!(
                writer.hash(&edit.path)? == edit.expected_hash,
                "batch conflict: {} changed",
                edit.path
            );
        }
        validate_final(&tx, project_id, &plan)?;
        let journal = Journal {
            operation_kind: "batch_write".into(),
            version: 1,
            nonce: format!(
                "{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)?
                    .as_nanos()
            ),
            plan,
        };
        let id = memory_core::save_operation(&root, &serde_json::to_vec(&journal)?)?;
        let mut result = publish(&root, &writer, &journal.plan, &id, &mut stage);
        drop(writer);
        if let Err(error) = tx.commit() {
            result.complete = false;
            result.error = Some(format!(
                "files may be saved; database transaction failed: {error}"
            ));
        }
        self.batch_index(&journal.plan.project, result)
    }

    pub fn batch_resume(
        &mut self,
        project: &str,
        source_id: i64,
        operation_id: &str,
    ) -> Result<BatchResult> {
        self.batch_resume_inner(project, source_id, operation_id, |_| Ok(()))
    }

    fn batch_resume_inner(
        &mut self,
        project: &str,
        source_id: i64,
        operation_id: &str,
        mut stage: impl FnMut(&str) -> Result<()>,
    ) -> Result<BatchResult> {
        let project_id = self.project_id(project)?;
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let root = root(&tx, project_id, source_id, true)?;
        let writer = SourceWriter::open(&root)?;
        let mut journal = read_journal(&root, operation_id, project)?;
        // Source-local journals survive index rebuilds with new numeric source IDs.
        // Authorize by the selected current source; original plan hash was checked above.
        journal.plan.source_id = source_id;
        // Completed operations are immutable receipts; never reapply after later edits.
        let mut result = if memory_core::operation_complete(&root, operation_id)? {
            BatchResult {
                operation_id: operation_id.into(),
                complete: true,
                partial: false,
                applied: Vec::new(),
                error: None,
                indexed: false,
                index_error: None,
            }
        } else {
            validate_final(&tx, project_id, &journal.plan)?;
            publish(&root, &writer, &journal.plan, operation_id, &mut stage)
        };
        drop(writer);
        if let Err(error) = tx.commit() {
            result.complete = false;
            result.error = Some(format!(
                "files may be saved; database transaction failed: {error}"
            ));
        }
        self.batch_index(project, result)
    }

    fn batch_index(&mut self, project: &str, mut result: BatchResult) -> Result<BatchResult> {
        result.partial = !result.complete && !result.applied.is_empty();
        let indexed = self.index(project);
        result.indexed = indexed.is_ok();
        result.index_error = indexed.err().map(|e| format!("{e:#}"));
        Ok(result)
    }

    /// Discovery bypasses index refresh so partial ID swaps remain recoverable.
    pub fn batch_operations(&self, project: &str, source_id: i64) -> Result<Value> {
        let root = root(
            &self.connection,
            self.project_id(project)?,
            source_id,
            false,
        )?;
        let mut operations = Vec::new();
        for id in memory_core::list_operations(&root)? {
            let raw: Value = serde_json::from_slice(&memory_core::read_operation(&root, &id)?)?;
            if raw["operation_kind"] != "batch_write" || raw["plan"]["project"] != project {
                continue;
            }
            let journal = read_journal(&root, &id, project)?;
            operations.push(json!({"operation_id":id,"complete":memory_core::operation_complete(&root,&id)?,"paths":journal.plan.edits.iter().map(|e|&e.path).collect::<Vec<_>>()}));
        }
        Ok(json!({"operations":operations}))
    }
}

fn publish(
    root: &std::path::Path,
    writer: &SourceWriter,
    plan: &BatchPlan,
    id: &str,
    stage: &mut impl FnMut(&str) -> Result<()>,
) -> BatchResult {
    let mut result = BatchResult {
        operation_id: id.into(),
        complete: false,
        partial: false,
        applied: Vec::new(),
        error: None,
        indexed: false,
        index_error: None,
    };
    let operation = (|| -> Result<()> {
        // All conflicts are checked before any further publication, including resume.
        for edit in &plan.edits {
            let current = writer.hash(&edit.path)?;
            ensure!(
                current == edit.expected_hash
                    || current.as_deref() == Some(edit.desired().as_str()),
                "batch conflict: {} changed or is missing",
                edit.path
            );
        }
        stage("journal")?;
        for edit in &plan.edits {
            let desired = edit.desired();
            let mode = if writer.hash(&edit.path)?.as_deref() == Some(desired.as_str()) {
                WriteMode::Update {
                    expected_hash: &desired,
                }
            } else {
                edit.mode()
            };
            writer.write(&edit.path, &edit.content, mode)?;
            stage("write")?;
        }
        stage("written")?;
        memory_core::finish_operation(root, id)?;
        Ok(())
    })();
    result.complete = operation.is_ok();
    result.error = operation.err().map(|e| format!("{e:#}"));
    for edit in &plan.edits {
        if writer.hash(&edit.path).ok().flatten().as_deref() == Some(edit.desired().as_str()) {
            result.applied.push(edit.path.clone());
        }
    }
    result.partial = !result.complete && !result.applied.is_empty();
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    fn note(id: &str) -> String {
        format!("---\nid: {id}\nmemory:\n  id: {id}\n  project: demo\n  kind: decision\n  status: accepted\n  summary: {id}\n---\n# {id}\n")
    }

    #[test]
    fn interrupted_id_swap_survives_new_database_source_ids_and_conflicts() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let root = temp.path().join("notes");
        fs::create_dir(&root)?;
        fs::write(root.join("a.md"), note("a"))?;
        fs::write(root.join("b.md"), note("b"))?;
        let mut store = Store::open(&temp.path().join("db"))?;
        store.create_project("demo")?;
        let source = store.register_source("demo", &root, true)?;
        store.index("demo")?;
        let edits = vec![
            BatchEdit {
                path: "a.md".into(),
                content: note("b"),
                expected_hash: Some(memory_core::content_hash(note("a").as_bytes())),
            },
            BatchEdit {
                path: "b.md".into(),
                content: note("a"),
                expected_hash: Some(memory_core::content_hash(note("b").as_bytes())),
            },
        ];
        let plan = store.batch_preview("demo", source, edits)?;
        let partial = store.batch_start(plan, |stage| {
            if stage == "write" {
                anyhow::bail!("injected interruption");
            }
            Ok(())
        })?;
        assert!(
            partial.partial && !partial.complete && !partial.indexed,
            "{partial:?}"
        );
        assert_eq!(partial.applied, ["a.md"]);
        assert_eq!(
            store.batch_operations("demo", source)?["operations"][0]["complete"],
            false
        );
        // Rebuild with a different numeric ID, keeping source-local journal/history.
        let mut rebuilt = Store::open(&temp.path().join("rebuilt"))?;
        rebuilt.create_project("demo")?;
        let other = temp.path().join("other");
        fs::create_dir(&other)?;
        rebuilt.register_source("demo", &other, false)?;
        let new_source = rebuilt.register_source("demo", &root, true)?;
        assert_ne!(source, new_source);
        assert_eq!(
            rebuilt.batch_operations("demo", new_source)?["operations"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
        fs::write(root.join("b.md"), "# External edit\n")?;
        let conflict = rebuilt.batch_resume("demo", new_source, &partial.operation_id)?;
        assert!(!conflict.complete && conflict.error.is_some());
        assert_eq!(fs::read_to_string(root.join("b.md"))?, "# External edit\n");
        fs::write(root.join("b.md"), note("b"))?;
        // Permission revocation is checked even when no valid index can be built.
        rebuilt.register_source("demo", &root, false)?;
        assert!(rebuilt
            .batch_resume("demo", new_source, &partial.operation_id)
            .is_err());
        rebuilt.register_source("demo", &root, true)?;
        let done = rebuilt.batch_resume("demo", new_source, &partial.operation_id)?;
        assert!(done.complete && done.indexed, "{done:?}");
        assert_eq!(fs::read_to_string(root.join("a.md"))?, note("b"));
        assert_eq!(fs::read_to_string(root.join("b.md"))?, note("a"));
        assert!(
            rebuilt
                .batch_resume("demo", new_source, &partial.operation_id)?
                .complete
        );
        Ok(())
    }

    #[test]
    fn interruptions_at_each_stage_resume_without_rewriting_conflicts() -> Result<()> {
        for target in ["journal", "write", "written"] {
            let temp = tempfile::tempdir()?;
            let root = temp.path().join("notes");
            fs::create_dir(&root)?;
            let mut store = Store::open(&temp.path().join("db"))?;
            store.create_project("demo")?;
            let source = store.register_source("demo", &root, true)?;
            let edits = vec![
                BatchEdit {
                    path: "a.md".into(),
                    content: "# A".into(),
                    expected_hash: None,
                },
                BatchEdit {
                    path: "b.md".into(),
                    content: "# B".into(),
                    expected_hash: None,
                },
            ];
            let plan = store.batch_preview("demo", source, edits)?;
            let partial = store.batch_start(plan, |stage| {
                if stage == target {
                    anyhow::bail!("interrupted");
                }
                Ok(())
            })?;
            assert!(!partial.complete);
            assert!(
                store
                    .batch_resume("demo", source, &partial.operation_id)?
                    .complete
            );
            assert_eq!(fs::read_to_string(root.join("a.md"))?, "# A");
            assert_eq!(fs::read_to_string(root.join("b.md"))?, "# B");
        }
        Ok(())
    }
}
