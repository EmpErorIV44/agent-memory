//! Read-only local diagnostics. Never repairs, migrates, writes notes or contacts a remote.
use anyhow::Result;
use rusqlite::{Connection, OpenFlags, OptionalExtension};
use serde_json::{json, Value};
use std::{
    collections::HashSet,
    fs,
    io::{Read, Seek, SeekFrom},
    path::Path,
    process::{Command, Stdio},
    time::{Duration, Instant},
};

fn check(
    checks: &mut Vec<Value>,
    name: &str,
    status: &str,
    message: impl Into<String>,
    source: Option<i64>,
) {
    let mut entry = json!({"name":name,"status":status,"message":message.into()});
    if let Some(source) = source {
        entry["source_id"] = json!(source);
    }
    checks.push(entry);
}

fn report(checks: Vec<Value>) -> Value {
    json!({"healthy":!checks.iter().any(|item| item["status"] == "error"),"checks":checks})
}

// Only local read-only Git operations use this helper. Discard stderr: it may
// contain credential-bearing remotes or other configuration details.
fn git(root: &Path, args: &[&str]) -> Option<String> {
    let mut out = tempfile::tempfile().ok()?;
    let mut child = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .env_remove("GIT_COMMON_DIR")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(Stdio::null())
        .stdout(out.try_clone().ok()?)
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let deadline = Instant::now() + Duration::from_secs(5);
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(10)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    };
    if !status.success() {
        return None;
    }
    out.seek(SeekFrom::Start(0)).ok()?;
    let mut text = String::new();
    out.read_to_string(&mut text).ok()?;
    Some(text)
}

fn git_checks(checks: &mut Vec<Value>, root: &Path, source: i64) {
    let Some(top) = git(root, &["rev-parse", "--show-toplevel"]) else {
        check(
            checks,
            "git_repository",
            "ok",
            "Source is not a Git working tree; Git synchronization is optional",
            Some(source),
        );
        return;
    };
    if Path::new(top.trim()).canonicalize().ok() != root.canonicalize().ok() {
        check(
            checks,
            "git_repository",
            "warning",
            "Source is nested in a Git repository; Git sync requires the exact repository root",
            Some(source),
        );
        return;
    }
    check(
        checks,
        "git_repository",
        "ok",
        "Source is a Git repository root",
        Some(source),
    );
    let identity = ["user.name", "user.email"].iter().all(|key| {
        git(root, &["config", "--get", key]).is_some_and(|value| !value.trim().is_empty())
    });
    check(
        checks,
        "git_identity",
        if identity { "ok" } else { "warning" },
        if identity {
            "Git commit identity is configured"
        } else {
            "Git user.name or user.email is missing; configure a commit identity before Git sync"
        },
        Some(source),
    );
    if git(root, &["symbolic-ref", "--quiet", "HEAD"]).is_none() {
        check(
            checks,
            "git_branch",
            "warning",
            "Detached HEAD; attach a branch before enabling Git sync",
            Some(source),
        );
    } else {
        check(
            checks,
            "git_branch",
            "ok",
            "Current branch is attached",
            Some(source),
        );
    }
    if git(root, &["rev-parse", "--verify", "@{upstream}"]).is_none() {
        check(
            checks,
            "git_upstream",
            "warning",
            "No existing upstream branch; configure it before enabling Git sync",
            Some(source),
        );
    } else {
        check(
            checks,
            "git_upstream",
            "ok",
            "Existing upstream branch is configured; remote reachability was not checked",
            Some(source),
        );
    }
    match git(root, &["ls-files", "--unmerged", "-z"]) {
        Some(value) if value.is_empty() => check(
            checks,
            "git_conflicts",
            "ok",
            "No unresolved conflicts",
            Some(source),
        ),
        _ => check(
            checks,
            "git_conflicts",
            "warning",
            "Conflicts or an unreadable Git index prevent synchronization",
            Some(source),
        ),
    }
    match git(root, &["diff", "--cached", "--name-only", "-z"]) {
        Some(value) if value.is_empty() => check(
            checks,
            "git_staging",
            "ok",
            "No staged changes",
            Some(source),
        ),
        _ => check(
            checks,
            "git_staging",
            "warning",
            "Existing staged changes or an unreadable index prevent automatic synchronization",
            Some(source),
        ),
    }
    for state in [
        "MERGE_HEAD",
        "CHERRY_PICK_HEAD",
        "REVERT_HEAD",
        "rebase-merge",
        "rebase-apply",
        "sequencer",
    ] {
        if let Some(path) = git(root, &["rev-parse", "--git-path", state]) {
            if root.join(path.trim()).exists() {
                check(
                    checks,
                    "git_operation",
                    "warning",
                    "An unfinished merge, rebase or sequencer operation prevents synchronization",
                    Some(source),
                );
                break;
            }
        }
    }
}

pub fn run(database: &Path, project: &str) -> Result<Value> {
    let mut checks = Vec::new();
    if !database.is_file() {
        check(
            &mut checks,
            "database",
            "error",
            "Database does not exist or is not a regular file; doctor does not create it",
            None,
        );
        return Ok(report(checks));
    }
    let connection = match Connection::open_with_flags(
        database,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    ) {
        Ok(connection) => connection,
        Err(_) => {
            check(
                &mut checks,
                "database",
                "error",
                "Cannot open database read-only",
                None,
            );
            return Ok(report(checks));
        }
    };
    if connection.busy_timeout(Duration::from_secs(2)).is_err() {
        check(
            &mut checks,
            "database",
            "error",
            "Cannot configure read-only SQLite inspection",
            None,
        );
        return Ok(report(checks));
    }
    let version: i64 = match connection.query_row("PRAGMA user_version", [], |row| row.get(0)) {
        Ok(version) => version,
        Err(_) => {
            check(
                &mut checks,
                "database",
                "error",
                "Cannot read SQLite schema; database may be corrupt",
                None,
            );
            return Ok(report(checks));
        }
    };
    if version != 4 {
        check(&mut checks,"schema","error",format!("Unsupported schema version {version}; expected 4. Doctor does not migrate databases"),None);
        return Ok(report(checks));
    }
    check(&mut checks, "schema", "ok", "SQLite schema version 4", None);
    let quick = connection
        .prepare("PRAGMA quick_check")
        .and_then(|mut statement| {
            statement
                .query_map([], |row| row.get::<_, String>(0))?
                .collect::<rusqlite::Result<Vec<_>>>()
        });
    match quick {
        Ok(rows) if rows == ["ok"] => check(
            &mut checks,
            "integrity",
            "ok",
            "SQLite quick_check passed",
            None,
        ),
        _ => {
            check(
                &mut checks,
                "integrity",
                "error",
                "SQLite quick_check failed",
                None,
            );
            return Ok(report(checks));
        }
    }
    let foreign_keys = connection
        .prepare("PRAGMA foreign_key_check")
        .and_then(|mut statement| Ok(statement.query([])?.next()?.is_none()));
    match foreign_keys {
        Ok(true) => check(
            &mut checks,
            "foreign_keys",
            "ok",
            "No foreign key violations",
            None,
        ),
        _ => check(
            &mut checks,
            "foreign_keys",
            "error",
            "Foreign key violations or unreadable schema",
            None,
        ),
    }
    let project_id: Option<i64> = match connection
        .query_row("SELECT id FROM projects WHERE name=?1", [project], |row| {
            row.get(0)
        })
        .optional()
    {
        Ok(value) => value,
        Err(_) => {
            check(
                &mut checks,
                "project",
                "error",
                "Cannot read project registrations",
                None,
            );
            return Ok(report(checks));
        }
    };
    let Some(project_id) = project_id else {
        check(
            &mut checks,
            "project",
            "error",
            "Requested project is not registered",
            None,
        );
        return Ok(report(checks));
    };
    check(
        &mut checks,
        "project",
        "ok",
        "Requested project is registered",
        None,
    );
    let sources = connection.prepare("SELECT s.id,s.root_path,ps.can_read,ps.can_write FROM sources s JOIN project_sources ps ON ps.source_id=s.id WHERE ps.project_id=?1 ORDER BY s.id")
        .and_then(|mut statement| statement.query_map([project_id], |row| Ok((row.get::<_,i64>(0)?,row.get::<_,String>(1)?,row.get::<_,bool>(2)?,row.get::<_,bool>(3)?)))?.collect::<rusqlite::Result<Vec<_>>>());
    let sources = match sources {
        Ok(sources) => sources,
        Err(_) => {
            check(
                &mut checks,
                "sources",
                "error",
                "Cannot read source registrations; database schema is incomplete or invalid",
                None,
            );
            return Ok(report(checks));
        }
    };
    if sources.is_empty() {
        check(
            &mut checks,
            "sources",
            "warning",
            "Project has no registered sources",
            None,
        );
    }
    let git_available = git(Path::new("."), &["--version"]).is_some();
    check(
        &mut checks,
        "git_executable",
        if git_available { "ok" } else { "warning" },
        if git_available {
            "Git is available"
        } else {
            "Git is not available; local memory works, Git sync does not"
        },
        None,
    );
    let mut memory_ids = HashSet::new();
    for (id, root, can_read, can_write) in sources {
        if !can_read {
            check(
                &mut checks,
                "source_access",
                if can_write { "error" } else { "ok" },
                if can_write {
                    "Invalid binding: write permission requires read permission"
                } else {
                    "Source is not readable for this project and was not inspected"
                },
                Some(id),
            );
            continue;
        }
        let root = Path::new(&root);
        if !root.is_dir()
            || fs::symlink_metadata(root).is_ok_and(|metadata| metadata.file_type().is_symlink())
        {
            check(
                &mut checks,
                "source_directory",
                "error",
                "Source directory is missing, inaccessible or replaced by a symlink",
                Some(id),
            );
            continue;
        }
        match memory_core::read_source(root) {
            Ok(documents) => {
                let mut document_ids = HashSet::new();
                let duplicate_document = documents
                    .iter()
                    .filter_map(|document| document.stable_id.as_deref())
                    .any(|value| !document_ids.insert(value));
                let duplicate_memory = documents
                    .iter()
                    .filter_map(|document| document.memory.as_ref())
                    .filter(|memory| memory.project == project)
                    .any(|memory| !memory_ids.insert(memory.id.clone()));
                if duplicate_document || duplicate_memory {
                    check(&mut checks,"source_identifiers","error","Duplicate document IDs in source or Memory IDs in project would prevent indexing",Some(id));
                }
                check(
                    &mut checks,
                    "source_markdown",
                    "ok",
                    format!(
                        "Read and parsed {} Markdown documents without changing the index",
                        documents.len()
                    ),
                    Some(id),
                );
            }
            Err(_) => check(
                &mut checks,
                "source_markdown",
                "error",
                "Source cannot be scanned or contains invalid UTF-8/Markdown metadata",
                Some(id),
            ),
        }
        if can_write {
            if fs::metadata(root).is_err() {
                check(
                    &mut checks,
                    "source_write",
                    "error",
                    "Source became inaccessible during permission inspection",
                    Some(id),
                );
            } else if fs::metadata(root).is_ok_and(|metadata| metadata.permissions().readonly()) {
                check(&mut checks,"source_write","warning","Source permits writes in configuration but directory permissions are read-only",Some(id));
            } else {
                check(&mut checks,"source_write","ok","Source permits writes; directory is not marked read-only. ACLs and actual write access were not probed",Some(id));
            }
        } else {
            check(
                &mut checks,
                "source_write",
                "ok",
                "Source is configured read-only",
                Some(id),
            );
        }
        if git_available {
            git_checks(&mut checks, root, id);
        }
    }
    Ok(report(checks))
}
