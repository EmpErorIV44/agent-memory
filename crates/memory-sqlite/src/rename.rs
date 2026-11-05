//! Reviewable guarded rename with individually durable repairs, never an atomic multi-file claim.
use crate::{
    links::{decoded, resolve},
    Store,
};
use anyhow::{ensure, Context, Result};
use rusqlite::{params, OptionalExtension, TransactionBehavior};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RenameEdit {
    pub path: String,
    pub expected_hash: String,
    pub content: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RenamePlan {
    pub project: String,
    pub source_id: i64,
    pub from: String,
    pub to: String,
    /// Every Markdown path/hash in this source, including files with no incoming links.
    pub manifest: Vec<(String, String)>,
    pub edits: Vec<RenameEdit>,
    pub moved_content: String,
    pub expected_hash: String,
    pub plan_hash: String,
    pub warnings: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct RenameResult {
    pub operation_id: String,
    pub complete: bool,
    pub partial: bool,
    pub moved: bool,
    /// Observed after the attempt, useful when publication succeeded but cleanup failed.
    pub destination_present: bool,
    pub source_present: bool,
    pub applied: Vec<String>,
    pub error: Option<String>,
    pub indexed: bool,
    pub index_error: Option<String>,
}

fn validate(path: &str) -> Result<()> {
    ensure!(
        path.len() <= 4096 && !path.is_empty() && path.to_ascii_lowercase().ends_with(".md"),
        "invalid Markdown path"
    );
    for part in path.split('/') {
        ensure!(
            !part.is_empty()
                && !part.starts_with('.')
                && !part.ends_with(['.', ' '])
                && !part
                    .chars()
                    .any(|c| c.is_control() || "\\:<>\"|?*".contains(c)),
            "rename requires relative visible Markdown paths without traversal"
        );
        let stem = part.split('.').next().unwrap().to_ascii_uppercase();
        ensure!(
            !["CON", "PRN", "AUX", "NUL"].contains(&stem.as_str())
                && !(stem.len() == 4
                    && (stem.starts_with("COM") || stem.starts_with("LPT"))
                    && matches!(stem.as_bytes()[3], b'1'..=b'9')),
            "reserved filename"
        );
    }
    Ok(())
}

fn destination_absent(root: &Path, to: &str) -> Result<()> {
    let mut path = root.to_path_buf();
    for part in to.split('/') {
        path.push(part);
        match std::fs::symlink_metadata(&path) {
            Ok(metadata) => {
                ensure!(
                    !metadata.file_type().is_symlink(),
                    "rename refuses symlink paths"
                );
                ensure!(path != root.join(to), "rename destination already exists");
                ensure!(metadata.is_dir(), "rename parent must be a directory");
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
}

fn relative(origin: &str, target: &str) -> String {
    let base: Vec<_> = origin
        .rsplit_once('/')
        .map_or("", |(directory, _)| directory)
        .split('/')
        .filter(|part| !part.is_empty())
        .collect();
    let target: Vec<_> = target.split('/').collect();
    let shared = base.iter().zip(&target).take_while(|(a, b)| a == b).count();
    std::iter::repeat_n("..", base.len() - shared)
        .chain(target[shared..].iter().copied())
        .collect::<Vec<_>>()
        .join("/")
}

fn encode(path: &str) -> String {
    let mut result = String::new();
    for byte in path.bytes() {
        if byte.is_ascii_alphanumeric() || b"/._-~".contains(&byte) {
            result.push(byte as char);
        } else {
            result.push_str(&format!("%{byte:02X}"));
        }
    }
    result
}

impl Store {
    pub fn rename_preview(
        &mut self,
        project: &str,
        source_id: i64,
        from: &str,
        to: &str,
    ) -> Result<RenamePlan> {
        validate(from)?;
        validate(to)?;
        ensure!(from != to, "rename paths must differ");
        self.index(project)?;
        let source = self
            .sources(project)?
            .into_iter()
            .find(|source| source.source_id == source_id)
            .context("source not found or inaccessible")?;
        let root = Path::new(&source.root_path);
        destination_absent(root, to)?;
        let documents = self.link_documents(source_id)?;
        let moving = documents
            .iter()
            .find(|document| document.path == from)
            .context("rename source not found")?;
        let mut plan = RenamePlan { project:project.into(),source_id,from:from.into(),to:to.into(),manifest:Vec::new(),edits:Vec::new(),moved_content:String::new(),expected_hash:moving.content_hash.clone(),plan_hash:String::new(),warnings:vec!["Repairs are published individually; a failure can leave a partial result. History stays path-scoped; the destination starts a new history. Heading anchors are preserved but not validated.".into()] };
        for document in &documents {
            let content = memory_core::read_markdown(root, &document.path)?;
            ensure!(
                memory_core::content_hash(content.as_bytes()) == document.content_hash,
                "document changed during preview; retry"
            );
            plan.manifest
                .push((document.path.clone(), document.content_hash.clone()));
            let bound = self
                .document_links(project, source_id, &document.path)?
                .context("document links unavailable")?
                .outgoing;
            let rewritten = memory_core::rewrite_links(&content, |target, kind| {
                if decoded(target.split('#').next().unwrap_or(""))
                    .is_some_and(|target| target.starts_with("id:"))
                {
                    return None;
                }
                let resolved = resolve(document, target, kind, &documents).0.or_else(|| {
                    bound
                        .iter()
                        .find(|link| link.target == target && link.kind == kind)
                        .and_then(|link| {
                            link.target_document
                                .as_ref()
                                .map(|target| target.document_id)
                        })
                });
                let target_document = documents
                    .iter()
                    .find(|document| Some(document.document_id) == resolved)?;
                let moving_origin = document.document_id == moving.document_id;
                let moving_target = target_document.document_id == moving.document_id;
                if !moving_origin && !moving_target {
                    return None;
                }
                let new_origin = if moving_origin { to } else { &document.path };
                let new_target = if moving_target {
                    to
                } else {
                    &target_document.path
                };
                let path = if kind == "wiki" {
                    new_target.to_owned()
                } else {
                    relative(new_origin, new_target)
                };
                let mut encoded = encode(&path);
                if kind == "wiki"
                    && !target
                        .split('#')
                        .next()
                        .unwrap_or("")
                        .to_ascii_lowercase()
                        .ends_with(".md")
                {
                    encoded.truncate(encoded.len().saturating_sub(3));
                }
                if let Some((_, fragment)) = target.split_once('#') {
                    encoded.push('#');
                    encoded.push_str(fragment);
                }
                (encoded != target).then_some(encoded)
            });
            if document.document_id == moving.document_id {
                plan.moved_content = rewritten;
            } else if rewritten != content {
                plan.edits.push(RenameEdit {
                    path: document.path.clone(),
                    expected_hash: document.content_hash.clone(),
                    content: rewritten,
                });
            }
        }
        let encoded = serde_json::to_vec(&plan)?;
        plan.plan_hash = memory_core::content_hash(&encoded);
        Ok(plan)
    }

    /// Recompute trusted content from the current source; persist before the first mutation.
    pub fn rename_apply(
        &mut self,
        project: &str,
        source_id: i64,
        from: &str,
        to: &str,
        expected_plan_hash: &str,
    ) -> Result<RenameResult> {
        self.rename_apply_inner(project, source_id, from, to, expected_plan_hash, |_| Ok(()))
    }

    fn rename_apply_inner(
        &mut self,
        project: &str,
        source_id: i64,
        from: &str,
        to: &str,
        expected_plan_hash: &str,
        stage: impl FnMut(&str) -> Result<()>,
    ) -> Result<RenameResult> {
        let plan = self.rename_preview(project, source_id, from, to)?;
        ensure!(
            plan.plan_hash == expected_plan_hash,
            "rename conflict: plan changed; review a fresh preview"
        );
        let project_id = self.project_id(project)?;
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let root = authorized_root(&tx, project_id, source_id, true)?;
        let scanned = memory_core::read_source(&root)?;
        let manifest: Vec<_> = scanned
            .iter()
            .map(|d| (d.relative_path.clone(), d.content_hash.clone()))
            .collect();
        ensure!(
            manifest == plan.manifest,
            "rename conflict: source files changed; review a fresh preview"
        );
        destination_absent(&root, to)?;
        let journal = RenameJournal {
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
        validate_journal(&journal, project)?;
        let operation_id = memory_core::save_operation(&root, &serde_json::to_vec(&journal)?)?;
        tx.commit()?;
        self.rename_resume_inner(project, source_id, &operation_id, stage)
    }

    /// List source-local operations without relying on the rebuildable document index.
    pub fn rename_operations(&self, project: &str, source_id: i64) -> Result<Vec<RenameOperation>> {
        let root = authorized_root(
            &self.connection,
            self.project_id(project)?,
            source_id,
            false,
        )?;
        let mut operations = Vec::new();
        for operation_id in memory_core::list_operations(&root)? {
            let bytes = memory_core::read_operation(&root, &operation_id)?;
            let value: serde_json::Value = serde_json::from_slice(&bytes)?;
            if value["operation_kind"] == "batch_write" {
                continue;
            }
            let journal: RenameJournal = serde_json::from_slice(&bytes)?;
            if journal.plan.project != project {
                continue;
            }
            validate_journal(&journal, project)?;
            operations.push(RenameOperation {
                complete: memory_core::operation_complete(&root, &operation_id)?,
                operation_id,
                from: journal.plan.from,
                to: journal.plan.to,
            });
        }
        Ok(operations)
    }

    /// Recognize already published bytes; never overwrite divergent current content.
    pub fn rename_resume(
        &mut self,
        project: &str,
        source_id: i64,
        operation_id: &str,
    ) -> Result<RenameResult> {
        self.rename_resume_inner(project, source_id, operation_id, |_| Ok(()))
    }

    fn rename_resume_inner(
        &mut self,
        project: &str,
        source_id: i64,
        operation_id: &str,
        mut stage: impl FnMut(&str) -> Result<()>,
    ) -> Result<RenameResult> {
        let project_id = self.project_id(project)?;
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        // Keep permissions serialized against revocation throughout publication.
        let root = authorized_root(&tx, project_id, source_id, true)?;
        let journal: RenameJournal =
            serde_json::from_slice(&memory_core::read_operation(&root, operation_id)?)?;
        validate_journal(&journal, project)?;
        let plan = journal.plan;
        let mut result = RenameResult {
            operation_id: operation_id.into(),
            complete: false,
            partial: false,
            moved: false,
            destination_present: false,
            source_present: false,
            applied: Vec::new(),
            error: None,
            indexed: false,
            index_error: None,
        };
        let operation = (|| -> Result<()> {
            if memory_core::operation_complete(&root, operation_id)? {
                return Ok(());
            }
            let destination_hash = memory_core::content_hash(plan.moved_content.as_bytes());
            let source = memory_core::markdown_hash(&root, &plan.from)?;
            let destination = memory_core::markdown_hash(&root, &plan.to)?;
            ensure!(
                source.as_ref().is_none_or(|h| h == &plan.expected_hash),
                "rename conflict: source changed"
            );
            ensure!(
                destination.as_ref().is_none_or(|h| h == &destination_hash),
                "rename conflict: destination changed"
            );
            ensure!(
                source.is_some() || destination.is_some(),
                "rename conflict: both source and destination are missing"
            );
            // Preflight every repair before making further progress.
            for edit in &plan.edits {
                let current = memory_core::markdown_hash(&root, &edit.path)?;
                let desired = memory_core::content_hash(edit.content.as_bytes());
                ensure!(
                    current
                        .as_ref()
                        .is_some_and(|h| h == &edit.expected_hash || h == &desired),
                    "rename conflict: repair {} changed or is missing",
                    edit.path
                );
            }
            stage("journal")?;
            if destination.is_none() {
                memory_core::write_markdown(
                    &root,
                    &plan.to,
                    &plan.moved_content,
                    memory_core::WriteMode::Create,
                )?;
            }
            stage("destination")?;
            if source.is_some() {
                memory_core::finish_move_markdown(
                    &root,
                    &plan.from,
                    &plan.to,
                    &plan.moved_content,
                    &plan.expected_hash,
                )?;
            }
            stage("source_removed")?;
            for edit in &plan.edits {
                let desired = memory_core::content_hash(edit.content.as_bytes());
                if memory_core::markdown_hash(&root, &edit.path)?.as_ref() != Some(&desired) {
                    memory_core::write_markdown(
                        &root,
                        &edit.path,
                        &edit.content,
                        memory_core::WriteMode::Update {
                            expected_hash: &edit.expected_hash,
                        },
                    )?;
                }
                stage("repair")?;
            }
            stage("repairs_done")?;
            memory_core::finish_operation(&root, operation_id)?;
            Ok(())
        })();
        result.complete = operation.is_ok();
        if let Err(error) = operation {
            result.error = Some(format!("{error:#}"));
        }
        // Report observed publications even after a post-publication error.
        result.destination_present = memory_core::markdown_hash(&root, &plan.to)
            .ok()
            .flatten()
            .is_some();
        result.source_present = memory_core::markdown_hash(&root, &plan.from)
            .ok()
            .flatten()
            .is_some();
        result.moved = result.destination_present && !result.source_present;
        if memory_core::markdown_hash(&root, &plan.to).ok().flatten()
            == Some(memory_core::content_hash(plan.moved_content.as_bytes()))
        {
            result.applied.push(plan.to.clone());
        }
        for edit in &plan.edits {
            if memory_core::markdown_hash(&root, &edit.path).ok().flatten()
                == Some(memory_core::content_hash(edit.content.as_bytes()))
            {
                result.applied.push(edit.path.clone());
            }
        }
        result.partial = !result.complete && !result.applied.is_empty();
        if let Err(error) = tx.commit() {
            result.complete = false;
            result.error = Some(format!(
                "rename files may be saved, but database transaction failed: {error}"
            ));
        }
        result.partial = !result.complete && !result.applied.is_empty();
        let indexed = self.index(project);
        result.indexed = indexed.is_ok();
        result.index_error = indexed.err().map(|error| format!("{error:#}"));
        Ok(result)
    }
}

#[derive(Debug, Serialize)]
pub struct RenameOperation {
    pub operation_id: String,
    pub from: String,
    pub to: String,
    pub complete: bool,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RenameJournal {
    version: u32,
    nonce: String,
    plan: RenamePlan,
}

fn authorized_root(
    connection: &rusqlite::Connection,
    project_id: i64,
    source_id: i64,
    write: bool,
) -> Result<PathBuf> {
    let root: String = connection.query_row("SELECT s.root_path FROM sources s JOIN project_sources ps ON ps.source_id=s.id WHERE ps.project_id=?1 AND ps.source_id=?2 AND ps.can_read=1 AND (?3=0 OR ps.can_write=1)", params![project_id,source_id,write], |row| row.get(0)).optional()?.context("source not found or access denied")?;
    Ok(PathBuf::from(root))
}

fn validate_journal(journal: &RenameJournal, project: &str) -> Result<()> {
    ensure!(
        journal.version == 1 && journal.plan.project == project,
        "operation project/version mismatch"
    );
    let plan = &journal.plan;
    let mut unhashed = plan.clone();
    unhashed.plan_hash.clear();
    ensure!(
        memory_core::content_hash(&serde_json::to_vec(&unhashed)?) == plan.plan_hash,
        "operation plan checksum mismatch"
    );
    validate(&plan.from)?;
    validate(&plan.to)?;
    ensure!(plan.from != plan.to, "operation paths must differ");
    let mut paths = std::collections::HashSet::new();
    for edit in &plan.edits {
        validate(&edit.path)?;
        ensure!(
            edit.path != plan.from && edit.path != plan.to && paths.insert(&edit.path),
            "invalid operation repair path"
        );
        if let Some(memory) = memory_core::parse_metadata(&edit.content)?.memory {
            ensure!(
                memory.project == project,
                "operation Memory project mismatch"
            );
        }
    }
    if let Some(memory) = memory_core::parse_metadata(&plan.moved_content)?.memory {
        ensure!(
            memory.project == project,
            "operation Memory project mismatch"
        );
    }
    Ok(())
}

#[cfg(all(test, unix))]
mod recovery_tests {
    use super::*;
    use std::fs;

    #[test]
    fn crash_child() -> Result<()> {
        let Some(base) = std::env::var_os("AGENT_MEMORY_RENAME_CRASH_ROOT") else {
            return Ok(());
        };
        let base = PathBuf::from(base);
        let stage = std::env::var("AGENT_MEMORY_RENAME_CRASH_STAGE")?;
        let mut store = Store::open(&base.join("index.sqlite"))?;
        let source = store.sources("demo")?[0].source_id;
        let plan = store.rename_preview("demo", source, "old.md", "sub/new.md")?;
        store.rename_apply_inner(
            "demo",
            source,
            "old.md",
            "sub/new.md",
            &plan.plan_hash,
            |at| {
                if at == stage {
                    std::process::exit(73);
                }
                Ok(())
            },
        )?;
        anyhow::bail!("crash hook was not reached")
    }

    fn interrupt(stage: &str) -> Result<tempfile::TempDir> {
        let base = tempfile::tempdir()?;
        let root = base.path().join("notes");
        fs::create_dir(&root)?;
        fs::write(
            root.join("old.md"),
            "---\nid: moved\n---\n# Old\n[Peer](a.md)\n",
        )?;
        fs::write(root.join("a.md"), "# A\n[Old](old.md)\n")?;
        fs::write(root.join("b.md"), "# B\n[[old]]\n")?;
        let store = Store::open(&base.path().join("index.sqlite"))?;
        store.create_project("demo")?;
        store.register_source("demo", &root, true)?;
        drop(store);
        let child = std::process::Command::new(std::env::current_exe()?)
            .args([
                "--exact",
                "rename::recovery_tests::crash_child",
                "--nocapture",
            ])
            .env("AGENT_MEMORY_RENAME_CRASH_ROOT", base.path())
            .env("AGENT_MEMORY_RENAME_CRASH_STAGE", stage)
            .output()?;
        ensure!(
            child.status.code() == Some(73),
            "unexpected crash subprocess status {:?}: {}",
            child.status,
            String::from_utf8_lossy(&child.stderr)
        );
        Ok(base)
    }

    #[test]
    fn abrupt_exit_at_every_publication_boundary_resumes_with_new_database() -> Result<()> {
        for stage in [
            "journal",
            "destination",
            "source_removed",
            "repair",
            "repairs_done",
        ] {
            let base = interrupt(stage)?;
            let root = base.path().join("notes");
            // A recreated database has independent numeric source IDs. Journal identity
            // and authorization are resolved from the newly registered physical source.
            let mut store = Store::open(&base.path().join("rebuilt.sqlite"))?;
            store.create_project("other")?;
            let extra = base.path().join("extra");
            fs::create_dir(&extra)?;
            store.register_source("other", &extra, false)?;
            store.create_project("demo")?;
            let source = store.register_source("demo", &root, true)?;
            let operations = store.rename_operations("demo", source)?;
            assert_eq!(operations.len(), 1);
            assert!(!operations[0].complete);
            assert_ne!(source, 1);
            let id = &operations[0].operation_id;
            let result = store.rename_resume("demo", source, id)?;
            assert!(result.complete, "{stage}: {result:?}");
            assert!(result.indexed, "{stage}: {result:?}");
            assert!(!root.join("old.md").exists());
            assert!(fs::read_to_string(root.join("sub/new.md"))?.contains("[Peer](../a.md)"));
            assert!(fs::read_to_string(root.join("a.md"))?.contains("(sub/new.md)"));
            assert!(fs::read_to_string(root.join("b.md"))?.contains("[[sub/new]]"));
            let revision_count = memory_core::list_revisions(&root, "a.md")?.len();
            assert!(store.rename_resume("demo", source, id)?.complete);
            assert_eq!(
                memory_core::list_revisions(&root, "a.md")?.len(),
                revision_count
            );
            assert!(store.rename_operations("demo", source)?[0].complete);
        }
        Ok(())
    }

    #[test]
    fn conflict_before_resume_does_not_overwrite_or_advance_other_files() -> Result<()> {
        let base = interrupt("destination")?;
        let root = base.path().join("notes");
        let mut store = Store::open(&base.path().join("index.sqlite"))?;
        let source = store.sources("demo")?[0].source_id;
        let id = store.rename_operations("demo", source)?[0]
            .operation_id
            .clone();
        fs::write(root.join("b.md"), "# Owner's conflicting edit\n")?;
        let result = store.rename_resume("demo", source, &id)?;
        assert!(!result.complete);
        assert!(result.error.as_deref().unwrap().contains("conflict"));
        assert!(root.join("old.md").exists());
        assert_eq!(
            fs::read_to_string(root.join("a.md"))?,
            "# A\n[Old](old.md)\n"
        );
        assert_eq!(
            fs::read_to_string(root.join("b.md"))?,
            "# Owner's conflicting edit\n"
        );
        fs::write(root.join("b.md"), "# B\n[[old]]\n")?;
        assert!(store.rename_resume("demo", source, &id)?.complete);
        Ok(())
    }
}
