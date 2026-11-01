//! A derived, read-only review queue; Markdown remains the source of truth.
use crate::git_evidence::Repositories;
use anyhow::{Context, Result};
use memory_core::{EvidenceKind, MemoryStatus};
use memory_sqlite::Store;
use serde_json::{json, Value};

/// The caller refreshes the project index before invoking this function.
/// No results are persisted, and neither evidence nor knowledge status is edited.
pub fn queue(
    store: &Store,
    repositories: &Repositories,
    project: &str,
    include_unknown: bool,
) -> Result<Value> {
    let mut entries = Vec::new();
    let mut scanned_count = 0;
    let mut unknown_count = 0;
    for record in store.list_memories(project)? {
        if matches!(
            record.memory.status,
            MemoryStatus::Deprecated | MemoryStatus::Superseded
        ) {
            continue;
        }
        scanned_count += 1;
        // Manual, test, URL, and legacy file evidence are not failed Git checks.
        if !record.memory.evidence.iter().any(|item| {
            matches!(item.kind, EvidenceKind::File | EvidenceKind::Commit)
                && item.reference.starts_with("git:")
        }) {
            continue;
        }
        let verification = repositories.verify(store, project, &record.memory.id)?;
        let evidence = verification["evidence"]
            .as_array()
            .context("evidence verification did not return an evidence array")?;
        let mut changed = false;
        let mut missing = false;
        let mut unknown = false;
        for item in evidence {
            if !matches!(item["kind"].as_str(), Some("file" | "commit"))
                || !item["reference"]
                    .as_str()
                    .is_some_and(|reference| reference.starts_with("git:"))
            {
                continue;
            }
            match item["status"].as_str() {
                Some("changed") => changed = true,
                Some("missing") => missing = true,
                Some("unknown") => unknown = true,
                _ => {}
            }
        }
        if unknown {
            unknown_count += 1;
        }
        if changed || missing || (include_unknown && unknown) {
            let mut reasons = Vec::new();
            if changed {
                reasons.push("changed");
            }
            if missing {
                reasons.push("missing");
            }
            if unknown {
                reasons.push("unknown");
            }
            entries.push(json!({"memory":record,"reasons":reasons,"verification":verification}));
        }
    }
    Ok(json!({
        "project":project,
        "local_only":true,
        "scanned_count":scanned_count,
        "unknown_count":unknown_count,
        "include_unknown":include_unknown,
        "entries":entries
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{collections::HashSet, fs, path::Path, process::Command};

    fn git(root: &Path, args: &[&str]) -> String {
        let output = Command::new("git")
            .arg("-C")
            .arg(root)
            .args(args)
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
        String::from_utf8(output.stdout).unwrap().trim().to_owned()
    }

    fn note(notes: &Path, id: &str, project: &str, status: &str, evidence: Value) -> Result<()> {
        let metadata = json!({"memory":{
            "id":id,"project":project,"kind":"fact","status":status,
            "summary":"Reviewable claim","evidence":evidence
        }});
        fs::write(
            notes.join(format!("{id}.md")),
            format!("---\n{metadata}\n---\n# Claim\n"),
        )?;
        Ok(())
    }

    #[test]
    fn derives_queue_from_current_bytes_without_mutating_notes_or_statuses() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let root = temp.path().join("code");
        let notes = temp.path().join("notes");
        fs::create_dir(&root)?;
        fs::create_dir(&notes)?;
        git(&root, &["init", "-q"]);
        git(&root, &["config", "user.name", "Test"]);
        git(&root, &["config", "user.email", "test@example.invalid"]);
        for name in ["changed.rs", "missing.rs", "unchanged.rs"] {
            fs::write(root.join(name), "baseline\n")?;
        }
        git(&root, &["add", "."]);
        git(&root, &["commit", "-qm", "baseline"]);
        let commit = git(&root, &["rev-parse", "HEAD"]);
        let file =
            |name: &str| json!({"kind":"file","reference":format!("git:code@{commit}:{name}")});
        for (id, status, evidence) in [
            ("changed", "accepted", json!([file("changed.rs")])),
            ("missing", "implemented", json!([file("missing.rs")])),
            ("unchanged", "planned", json!([file("unchanged.rs")])),
            ("deprecated", "deprecated", json!([file("changed.rs")])),
            ("superseded", "superseded", json!([file("missing.rs")])),
            (
                "unknown",
                "accepted",
                json!([{"kind":"file","reference":format!("git:private@{commit}:file.rs")}]),
            ),
            (
                "legacy",
                "accepted",
                json!([{"kind":"file","reference":"legacy.rs"},{"kind":"test","reference":"do not execute"}]),
            ),
            (
                "mixed",
                "accepted",
                json!([file("changed.rs"),{"kind":"manual","reference":"user decision"}]),
            ),
        ] {
            note(&notes, id, "demo", status, evidence)?;
        }
        let mut store = Store::open(&temp.path().join("memory.sqlite"))?;
        store.create_project("demo")?;
        store.create_project("other")?;
        // Sharing a source does not grant access to another project's Memory.
        store.register_source("demo", &notes, false)?;
        store.register_source("other", &notes, false)?;
        note(
            &notes,
            "foreign",
            "other",
            "accepted",
            json!([file("changed.rs")]),
        )?;
        store.index("demo")?;
        store.index("other")?;
        let repositories = Repositories::new(
            vec![format!("demo:code={}", root.display())],
            &HashSet::from(["demo".into(), "other".into()]),
        )?;
        let before = fs::read(notes.join("changed.md"))?;
        fs::write(root.join("changed.rs"), "dirty\n")?;
        fs::remove_file(root.join("missing.rs"))?;
        let report = queue(&store, &repositories, "demo", false)?;
        let ids: Vec<_> = report["entries"]
            .as_array()
            .unwrap()
            .iter()
            .map(|entry| entry["memory"]["memory"]["id"].as_str().unwrap())
            .collect();
        assert_eq!(ids, ["changed", "missing", "mixed"]);
        assert_eq!(report["scanned_count"], 6);
        assert_eq!(report["unknown_count"], 1);
        assert_eq!(report["entries"][0]["reasons"], json!(["changed"]));
        assert_eq!(report["entries"][1]["reasons"], json!(["missing"]));
        assert_eq!(report["entries"][2]["reasons"], json!(["changed"]));
        let record = store.get_memory("demo", "changed")?.unwrap();
        assert_eq!(
            report["entries"][0]["memory"]["content_hash"],
            record.content_hash
        );
        assert_eq!(
            report["entries"][0]["verification"]["content_hash"],
            record.content_hash
        );
        assert_eq!(record.memory.status, MemoryStatus::Accepted);
        assert_eq!(fs::read(notes.join("changed.md"))?, before);
        let extended = queue(&store, &repositories, "demo", true)?;
        assert_eq!(extended["entries"].as_array().unwrap().len(), 4);
        assert_eq!(extended["entries"][3]["reasons"], json!(["unknown"]));
        let foreign = queue(&store, &repositories, "other", false)?;
        assert_eq!(foreign["scanned_count"], 1);
        assert_eq!(foreign["unknown_count"], 1);
        assert!(foreign["entries"].as_array().unwrap().is_empty());
        let foreign = queue(&store, &repositories, "other", true)?;
        assert_eq!(foreign["entries"][0]["memory"]["memory"]["id"], "foreign");
        assert_eq!(
            foreign["entries"][0]["verification"]["evidence"][0]["reason"],
            "repository_not_configured"
        );
        assert!(queue(&store, &repositories, "unregistered", false).is_err());
        // Recompute after fixing the working tree: no persisted queue to clear.
        fs::write(root.join("changed.rs"), "baseline\n")?;
        fs::write(root.join("missing.rs"), "baseline\n")?;
        assert!(queue(&store, &repositories, "demo", false)?["entries"]
            .as_array()
            .unwrap()
            .is_empty());
        Ok(())
    }
}
