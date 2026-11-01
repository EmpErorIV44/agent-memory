//! Reviewable guarded rename with individually durable repairs, never an atomic multi-file claim.
use crate::{
    links::{decoded, resolve},
    Store,
};
use anyhow::{ensure, Context, Result};
use rusqlite::{params, OptionalExtension, TransactionBehavior};
use serde::Serialize;
use std::path::{Path, PathBuf};

#[derive(Debug, Serialize)]
pub struct RenameEdit {
    pub path: String,
    pub expected_hash: String,
    pub content: String,
}

#[derive(Debug, Serialize)]
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

#[derive(Debug, Serialize)]
pub struct RenameResult {
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

    /// Recompute trusted content from the current source. Client-supplied edits are never accepted.
    pub fn rename_apply(
        &mut self,
        project: &str,
        source_id: i64,
        from: &str,
        to: &str,
        expected_plan_hash: &str,
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
        let root: String = tx.query_row("SELECT s.root_path FROM sources s JOIN project_sources ps ON ps.source_id=s.id WHERE ps.project_id=?1 AND ps.source_id=?2 AND ps.can_read=1 AND ps.can_write=1",params![project_id,source_id],|row| row.get(0)).optional()?.context("source not found or write access denied")?;
        let root = PathBuf::from(root);
        let scanned = memory_core::read_source(&root)?;
        let manifest: Vec<_> = scanned
            .iter()
            .map(|document| {
                (
                    document.relative_path.clone(),
                    document.content_hash.clone(),
                )
            })
            .collect();
        ensure!(
            manifest == plan.manifest,
            "rename conflict: source files changed; review a fresh preview"
        );
        destination_absent(&root, to)?;
        let mut result = RenameResult {
            complete: false,
            partial: false,
            moved: false,
            destination_present: false,
            source_present: true,
            applied: Vec::new(),
            error: None,
            indexed: false,
            index_error: None,
        };
        let operation = (|| -> Result<()> {
            // Publish the move first, then repair referring files. A partial result keeps
            // either a destination or both copies, and reports exactly which repairs landed.
            memory_core::move_markdown(&root, from, to, &plan.moved_content, &plan.expected_hash)?;
            result.moved = true;
            result.applied.push(to.into());
            for edit in &plan.edits {
                match memory_core::write_markdown(
                    &root,
                    &edit.path,
                    &edit.content,
                    memory_core::WriteMode::Update {
                        expected_hash: &edit.expected_hash,
                    },
                ) {
                    Ok(_) => result.applied.push(edit.path.clone()),
                    Err(error) => {
                        if error
                            .downcast_ref::<memory_core::PublishedWriteError>()
                            .is_some()
                        {
                            result.applied.push(edit.path.clone());
                        }
                        return Err(error);
                    }
                }
            }
            Ok(())
        })();
        result.complete = operation.is_ok();
        if let Err(error) = operation {
            result.error = Some(format!("{error:#}"));
            result.partial =
                !result.applied.is_empty() || memory_core::read_markdown(&root, to).is_ok();
        }
        if let Err(error) = tx.commit() {
            result.complete = false;
            result.partial =
                !result.applied.is_empty() || memory_core::read_markdown(&root, to).is_ok();
            result.error = Some(format!(
                "rename files may be saved, but database transaction failed: {error}"
            ));
        }
        let indexed = self.index(project);
        result.destination_present = std::fs::symlink_metadata(root.join(to)).is_ok();
        result.source_present = std::fs::symlink_metadata(root.join(from)).is_ok();
        result.indexed = indexed.is_ok();
        result.index_error = indexed.err().map(|error| format!("{error:#}"));
        Ok(result)
    }
}
