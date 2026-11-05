//! Opt-in synchronization and guarded fast-forward updates of Markdown sources.
use anyhow::{bail, ensure, Context, Result};
use std::{
    collections::BTreeSet,
    fs::{self, OpenOptions},
    io::{Read, Seek, SeekFrom},
    path::{Component, Path},
    process::{Command, ExitStatus, Stdio},
    thread,
    time::{Duration, Instant},
};

const TIMEOUT: Duration = Duration::from_secs(60);

#[derive(Debug, serde::Serialize)]
pub struct GitFreshness {
    pub status: String,
    pub ahead: Option<u64>,
    pub behind: Option<u64>,
    pub checked_remote: bool,
    /// True means comparison uses only potentially stale local remote-tracking refs.
    pub remote_tracking_only: bool,
}

impl GitFreshness {
    fn unknown(status: &str) -> Self {
        Self {
            status: status.into(),
            ahead: None,
            behind: None,
            checked_remote: false,
            remote_tracking_only: true,
        }
    }
}

// Keep Git publication out of a batch's publication gap. An interrupted batch
// must be explicitly resumed before any automatic commit/push or incoming pull.
fn batch_source_guard(root: &Path) -> Result<memory_core::SourceWriter> {
    let writer = memory_core::SourceWriter::open(root)?;
    for id in memory_core::list_operations(root)? {
        let value: serde_json::Value =
            serde_json::from_slice(&memory_core::read_operation(root, &id)?)?;
        ensure!(
            value["operation_kind"] != "batch_write" || memory_core::operation_complete(root, &id)?,
            "Git synchronization refuses an unfinished batch; resume it first"
        );
    }
    Ok(writer)
}

fn repository_lock(git_dir: &Path) -> Result<fs::File> {
    let path = git_dir.join("agent-memory-sync.lock");
    if let Ok(metadata) = fs::symlink_metadata(&path) {
        ensure!(
            !metadata.file_type().is_symlink(),
            "Git sync lock must not be a symlink"
        );
    }
    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(path)?;
    lock.try_lock()
        .context("another Git sync holds the repository lock")?;
    Ok(lock)
}

fn upstream(root: &Path) -> Result<(String, String)> {
    let branch = checked(root, &["symbolic-ref", "--quiet", "--short", "HEAD"])
        .context("Git update requires an attached branch")?;
    let remote = checked(
        root,
        &[
            "config",
            "--get",
            &format!("branch.{}.remote", branch.trim()),
        ],
    )?;
    let merge = checked(
        root,
        &[
            "config",
            "--get",
            &format!("branch.{}.merge", branch.trim()),
        ],
    )?;
    let remote = remote.trim().to_owned();
    let merge = merge.trim().to_owned();
    ensure!(
        !remote.is_empty()
            && !remote.starts_with('-')
            && !remote.contains('\n')
            && merge.starts_with("refs/heads/")
            && !merge.contains('\n'),
        "invalid Git upstream configuration"
    );
    ensure!(
        git(root, &["check-ref-format", &merge])?.status.success(),
        "invalid Git upstream reference"
    );
    Ok((remote, merge))
}

fn fetch_upstream(root: &Path) -> Result<()> {
    let (remote, reference) =
        upstream(root).context("Git update requires a configured upstream")?;
    // Empty refmap prevents configured fetch refspecs from updating unrelated refs.
    let result = git(
        root,
        &[
            "fetch",
            "--no-tags",
            "--no-recurse-submodules",
            "--refmap=",
            "--",
            &remote,
            &reference,
        ],
    );
    ensure!(
        result.is_ok_and(|output| output.status.success()),
        "Git fetch failed; check remote access and connectivity"
    );
    Ok(())
}

fn compare(root: &Path, target: &str, checked_remote: bool) -> Result<GitFreshness> {
    let range = format!("HEAD...{target}");
    let output = git(root, &["rev-list", "--left-right", "--count", &range])?;
    if !output.status.success() {
        return Ok(GitFreshness::unknown("unknown"));
    }
    let values: Vec<u64> = output
        .stdout
        .split_whitespace()
        .map(str::parse)
        .collect::<std::result::Result<_, _>>()?;
    ensure!(values.len() == 2, "invalid Git comparison output");
    let (ahead, behind) = (values[0], values[1]);
    let status = match (ahead, behind) {
        (0, 0) => "up_to_date",
        (0, _) => "behind",
        (_, 0) => "ahead",
        _ => "diverged",
    };
    Ok(GitFreshness {
        status: status.into(),
        ahead: Some(ahead),
        behind: Some(behind),
        checked_remote,
        remote_tracking_only: !checked_remote,
    })
}

/// Without fetch, remote-tracking refs can be stale; no remote contact is implied.
pub fn freshness(root: &Path, fetch: bool) -> Result<GitFreshness> {
    let Some(git_dir) = status_directory(root)? else {
        return Ok(GitFreshness::unknown("not_repository"));
    };
    let _lock = repository_lock(&git_dir)?;
    if upstream(root).is_err() {
        return Ok(GitFreshness::unknown("unknown"));
    }
    if fetch {
        fetch_upstream(root)?;
        compare(root, "FETCH_HEAD", true)
    } else {
        compare(root, "@{upstream}", false)
    }
}

/// Fetch the configured upstream and fast-forward only a completely clean worktree.
/// Does not commit, stash, push, rebase, or modify push synchronization status.
pub fn pull_source(root: &Path) -> Result<GitFreshness> {
    let git_dir = status_directory(root)?.context("Git update requires a source repository")?;
    let _lock = repository_lock(&git_dir)?;
    let _source_lock = batch_source_guard(root)?;
    upstream(root).context("Git update requires a configured upstream")?;
    for state in [
        "MERGE_HEAD",
        "CHERRY_PICK_HEAD",
        "REVERT_HEAD",
        "rebase-merge",
        "rebase-apply",
        "sequencer",
    ] {
        let path = checked(root, &["rev-parse", "--git-path", state])?;
        ensure!(
            !root.join(path.trim()).exists(),
            "Git update refuses an in-progress operation"
        );
    }
    ensure!(
        checked(root, &["ls-files", "--unmerged", "-z"])?.is_empty(),
        "Git update refuses unresolved conflicts"
    );
    ensure!(
        checked(root, &["status", "--porcelain", "--untracked-files=normal"])?
            .lines()
            .all(|line| line == "?? .agent-memory.lock"),
        "Git update requires a clean worktree and index, including untracked files"
    );
    fetch_upstream(root)?;
    let before = compare(root, "FETCH_HEAD", true)?;
    ensure!(
        before.status != "diverged" && before.status != "unknown",
        "Git update requires a fast-forward; local commits are retained"
    );
    let merged = git(
        root,
        &[
            "merge",
            "--ff-only",
            "--no-overwrite-ignore",
            "--no-autostash",
            "--no-edit",
            "--no-stat",
            "FETCH_HEAD",
        ],
    )?;
    ensure!(
        merged.status.success(),
        "Git fast-forward failed; local changes are retained"
    );
    compare(root, "FETCH_HEAD", true)
}

struct GitOutput {
    status: ExitStatus,
    stdout: String,
    stderr: String,
}

fn git(root: &Path, args: &[&str]) -> Result<GitOutput> {
    // Files avoid pipe-buffer deadlocks when a remote produces substantial output.
    let mut stdout = tempfile::tempfile()?;
    let mut stderr = tempfile::tempfile()?;
    let mut child = Command::new("git")
        .current_dir(root)
        .args([
            "-c",
            "core.hooksPath=/dev/null",
            "-c",
            "commit.gpgsign=false",
        ])
        .args(args)
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .env_remove("GIT_COMMON_DIR")
        .env_remove("GIT_OBJECT_DIRECTORY")
        .env_remove("GIT_ALTERNATE_OBJECT_DIRECTORIES")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env(
            "GIT_LITERAL_PATHSPECS",
            if args.first() == Some(&"check-ignore") {
                "0"
            } else {
                "1"
            },
        )
        .stdin(Stdio::null())
        .stdout(stdout.try_clone()?)
        .stderr(stderr.try_clone()?)
        .spawn()
        .context("launch git (Git must be installed)")?;
    let deadline = Instant::now() + TIMEOUT;
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            bail!(
                "git {} timed out after 60 seconds",
                args.first().unwrap_or(&"command")
            );
        }
        thread::sleep(Duration::from_millis(20));
    };
    stdout.seek(SeekFrom::Start(0))?;
    stderr.seek(SeekFrom::Start(0))?;
    let mut out = String::new();
    let mut err = String::new();
    stdout
        .read_to_string(&mut out)
        .context("Git output must be UTF-8")?;
    stderr.read_to_string(&mut err)?;
    Ok(GitOutput {
        status,
        stdout: out,
        stderr: err,
    })
}

fn checked(root: &Path, args: &[&str]) -> Result<String> {
    let output = git(root, args)?;
    ensure!(
        output.status.success(),
        "git {} failed: {}",
        args.first().unwrap_or(&"command"),
        output.stderr.trim()
    );
    Ok(output.stdout)
}

/// Resolve an exact source repository without creating runtime metadata.
pub(crate) fn status_directory(root: &Path) -> Result<Option<std::path::PathBuf>> {
    ensure!(
        !fs::symlink_metadata(root)?.file_type().is_symlink(),
        "Git sync source root must not be a symlink"
    );
    let root = root.canonicalize().context("resolve Git status source")?;
    let top = git(&root, &["rev-parse", "--show-toplevel"])?;
    if !top.status.success() {
        return Ok(None);
    }
    ensure!(
        Path::new(top.stdout.trim()).canonicalize()? == root,
        "Git sync source must be the repository root, not a subdirectory"
    );
    let directory = checked(&root, &["rev-parse", "--absolute-git-dir"])?;
    Ok(Some(Path::new(directory.trim()).to_path_buf()))
}

fn visible_markdown(root: &Path, relative: &str) -> bool {
    let path = Path::new(relative);
    if path.extension().and_then(|s| s.to_str()) != Some("md") {
        return false;
    }
    let mut absolute = root.to_path_buf();
    for component in path.components() {
        let Component::Normal(name) = component else {
            return false;
        };
        if name.to_string_lossy().starts_with('.') {
            return false;
        }
        absolute.push(name);
        match fs::symlink_metadata(&absolute) {
            Ok(metadata) if metadata.file_type().is_symlink() => return false,
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => return false,
        }
    }
    fs::symlink_metadata(&absolute).map_or_else(
        |error| error.kind() == std::io::ErrorKind::NotFound,
        |metadata| metadata.is_file(),
    )
}

/// Commit visible Markdown changes in an explicitly enabled repository root and
/// push to its existing upstream. Existing staged changes and conflicted states
/// are refused. A rejected push leaves the local commit for a later retry.
pub fn sync_source(root: &Path) -> Result<()> {
    ensure!(
        !fs::symlink_metadata(root)?.file_type().is_symlink(),
        "Git sync source root must not be a symlink"
    );
    let root = root.canonicalize().context("resolve Git source")?;
    let top = checked(&root, &["rev-parse", "--show-toplevel"])?;
    ensure!(
        Path::new(top.trim()).canonicalize()? == root,
        "Git sync source must be the repository root, not a subdirectory"
    );
    let git_dir = checked(&root, &["rev-parse", "--absolute-git-dir"])?;
    let git_dir = Path::new(git_dir.trim());
    let lock_path = git_dir.join("agent-memory-sync.lock");
    if let Ok(metadata) = fs::symlink_metadata(&lock_path) {
        ensure!(
            !metadata.file_type().is_symlink(),
            "Git sync lock must not be a symlink"
        );
    }
    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&lock_path)?;
    lock.try_lock()
        .context("another Git sync holds the repository lock")?;
    let status = crate::git_status::start(git_dir)?;
    let result = (|| {
        let _source_lock = batch_source_guard(&root)?;
        sync_locked(&root)
    })();
    let saved_status = crate::git_status::finish(git_dir, status, &result);
    result?;
    saved_status.context("Git synchronization completed but status could not be saved")?;
    Ok(())
}

fn sync_locked(root: &Path) -> Result<()> {
    checked(root, &["symbolic-ref", "--quiet", "HEAD"])
        .context("Git sync requires an attached branch")?;
    checked(root, &["rev-parse", "--verify", "@{upstream}"])
        .context("Git sync requires an existing upstream branch")?;
    for state in [
        "MERGE_HEAD",
        "CHERRY_PICK_HEAD",
        "REVERT_HEAD",
        "rebase-merge",
        "rebase-apply",
        "sequencer",
    ] {
        let state_path = checked(root, &["rev-parse", "--git-path", state])?;
        ensure!(
            !root.join(state_path.trim()).exists(),
            "Git sync refuses an in-progress merge, rebase or sequencer operation"
        );
    }
    ensure!(
        checked(root, &["ls-files", "--unmerged", "-z"])?.is_empty(),
        "Git sync refuses unresolved conflicts"
    );
    ensure!(
        checked(root, &["diff", "--cached", "--name-only", "-z"])?.is_empty(),
        "Git sync refuses existing staged changes; commit or unstage them first"
    );
    let paths = checked(
        root,
        &[
            "ls-files",
            "--modified",
            "--deleted",
            "--others",
            "--exclude-standard",
            "-z",
        ],
    )?;
    let mut selected = BTreeSet::new();
    for path in paths.split('\0').filter(|p| !p.is_empty()) {
        if !visible_markdown(root, path) {
            continue;
        }
        // Deleted symlinks have no filesystem entry; consult the index as well.
        let indexed = checked(root, &["ls-files", "--stage", "--", path])?;
        if indexed.starts_with("120000 ") || indexed.starts_with("160000 ") {
            continue;
        }
        let ignored = git(root, &["check-ignore", "--no-index", "--quiet", "--", path])?;
        match ignored.status.code() {
            Some(0) => continue,
            Some(1) => {
                selected.insert(path);
            }
            _ => bail!("git check-ignore failed: {}", ignored.stderr.trim()),
        }
    }
    for path in &selected {
        checked(root, &["add", "--all", "--", path])?;
    }
    let staged = checked(root, &["diff", "--cached", "--name-only", "-z"])?;
    ensure!(
        staged
            .split('\0')
            .filter(|p| !p.is_empty())
            .all(|p| selected.contains(p)),
        "Git index changed concurrently; refusing to commit unrelated staged changes"
    );
    if !staged.is_empty() {
        checked(
            root,
            &["commit", "--no-verify", "-m", "Sync agent memory Markdown"],
        )?;
    }
    // Use the explicitly configured upstream, independent of push.default or
    // remote push refspecs that might otherwise push additional branches.
    let branch = checked(root, &["symbolic-ref", "--quiet", "--short", "HEAD"])?;
    let remote_key = format!("branch.{}.remote", branch.trim());
    let merge_key = format!("branch.{}.merge", branch.trim());
    let remote = checked(root, &["config", "--get", &remote_key])?;
    let merge = checked(root, &["config", "--get", &merge_key])?;
    ensure!(
        !remote.trim().starts_with('-') && merge.trim().starts_with("refs/heads/"),
        "invalid Git upstream configuration"
    );
    let refspec = format!("HEAD:{}", merge.trim());
    checked(root, &["push", "--no-verify", remote.trim(), &refspec])
        .context("Git push failed; local commits are retained (no pull or force push attempted)")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn external_commit(temp: &Path, remote: &Path) {
        let other = temp.join("external");
        run(
            temp,
            &["clone", remote.to_str().unwrap(), other.to_str().unwrap()],
        );
        run(&other, &["config", "user.name", "External"]);
        run(&other, &["config", "user.email", "external@example.test"]);
        fs::write(other.join("external.md"), "# External\n").unwrap();
        run(&other, &["add", "external.md"]);
        run(&other, &["commit", "-m", "External"]);
        run(&other, &["push"]);
    }

    #[test]
    fn git_sync_and_pull_refuse_locked_or_unfinished_batches() {
        let (_temp, local, _remote) = fixture();
        let writer = memory_core::SourceWriter::open(&local).unwrap();
        assert!(sync_source(&local).is_err());
        assert!(pull_source(&local).is_err());
        drop(writer);
        let journal = serde_json::json!({"operation_kind":"batch_write","version":1});
        let id =
            memory_core::save_operation(&local, &serde_json::to_vec(&journal).unwrap()).unwrap();
        fs::write(local.join("batch.md"), "# Partial batch").unwrap();
        let head = checked(&local, &["rev-parse", "HEAD"]).unwrap();
        assert!(sync_source(&local)
            .unwrap_err()
            .to_string()
            .contains("unfinished batch"));
        assert!(pull_source(&local)
            .unwrap_err()
            .to_string()
            .contains("unfinished batch"));
        assert_eq!(checked(&local, &["rev-parse", "HEAD"]).unwrap(), head);
        memory_core::finish_operation(&local, &id).unwrap();
        sync_source(&local).unwrap();
    }

    #[test]
    fn freshness_fetch_and_pull_fast_forward_without_touching_push_status() {
        let (temp, local, remote) = fixture();
        external_commit(temp.path(), &remote);
        let stale = freshness(&local, false).unwrap();
        assert_eq!(stale.status, "up_to_date");
        assert!(!stale.checked_remote);
        assert!(stale.remote_tracking_only);
        let fresh = freshness(&local, true).unwrap();
        assert_eq!(fresh.status, "behind");
        assert_eq!(fresh.behind, Some(1));
        assert!(fresh.checked_remote);
        assert!(!fresh.remote_tracking_only);
        let result = pull_source(&local).unwrap();
        assert_eq!(result.status, "up_to_date");
        assert_eq!(result.ahead, Some(0));
        assert!(local.join("external.md").exists());
        assert!(!local.join(".git/agent-memory-sync-status.json").exists());
    }

    #[test]
    fn pull_refuses_divergence_and_preserves_head() {
        let (temp, local, remote) = fixture();
        external_commit(temp.path(), &remote);
        fs::write(local.join("local.md"), "# Local\n").unwrap();
        run(&local, &["add", "local.md"]);
        run(&local, &["commit", "-m", "Local"]);
        let head = checked(&local, &["rev-parse", "HEAD"]).unwrap();
        assert_eq!(freshness(&local, true).unwrap().status, "diverged");
        assert!(pull_source(&local).is_err());
        assert_eq!(checked(&local, &["rev-parse", "HEAD"]).unwrap(), head);
        assert!(!local.join("external.md").exists());
    }

    #[test]
    fn pull_preserves_ignored_runtime_file_when_remote_tracks_its_path() {
        let (temp, local, remote) = fixture();
        fs::write(local.join(".gitignore"), "agent-memory.sqlite\n").unwrap();
        run(&local, &["add", ".gitignore"]);
        run(&local, &["commit", "-m", "Ignore runtime database"]);
        run(&local, &["push"]);
        let original = b"local runtime database bytes\0\xff";
        fs::write(local.join("agent-memory.sqlite"), original).unwrap();
        let head = checked(&local, &["rev-parse", "HEAD"]).unwrap();
        let other = temp.path().join("external");
        run(
            temp.path(),
            &["clone", remote.to_str().unwrap(), other.to_str().unwrap()],
        );
        run(&other, &["config", "user.name", "External"]);
        run(&other, &["config", "user.email", "external@example.test"]);
        fs::write(other.join("agent-memory.sqlite"), "remote replacement").unwrap();
        run(&other, &["add", "-f", "agent-memory.sqlite"]);
        run(
            &other,
            &["commit", "-m", "Force-track ignored runtime path"],
        );
        run(&other, &["push"]);
        assert!(pull_source(&local).is_err());
        assert_eq!(checked(&local, &["rev-parse", "HEAD"]).unwrap(), head);
        assert_eq!(
            fs::read(local.join("agent-memory.sqlite")).unwrap(),
            original
        );
        assert!(checked(&local, &["status", "--porcelain"])
            .unwrap()
            .lines()
            .all(|line| line == "?? .agent-memory.lock"));
    }

    #[test]
    fn pull_refuses_dirty_staged_and_untracked_worktrees() {
        let (temp, local, remote) = fixture();
        external_commit(temp.path(), &remote);
        let head = checked(&local, &["rev-parse", "HEAD"]).unwrap();
        fs::write(local.join("seed.md"), "dirty").unwrap();
        assert!(pull_source(&local).is_err());
        assert_eq!(fs::read_to_string(local.join("seed.md")).unwrap(), "dirty");
        run(&local, &["add", "seed.md"]);
        assert!(pull_source(&local).is_err());
        run(&local, &["reset", "--hard", "HEAD"]);
        fs::write(local.join("untracked.txt"), "keep").unwrap();
        assert!(pull_source(&local).is_err());
        assert_eq!(checked(&local, &["rev-parse", "HEAD"]).unwrap(), head);
        assert_eq!(
            fs::read_to_string(local.join("untracked.txt")).unwrap(),
            "keep"
        );
    }

    #[test]
    fn freshness_without_upstream_and_non_repository() {
        let (temp, local, _) = fixture();
        run(&local, &["branch", "--unset-upstream"]);
        assert_eq!(freshness(&local, false).unwrap().status, "unknown");
        assert_eq!(freshness(&local, true).unwrap().status, "unknown");
        assert!(pull_source(&local).is_err());
        assert_eq!(
            freshness(temp.path(), false).unwrap().status,
            "not_repository"
        );
    }

    #[test]
    fn pull_uses_sync_lock_and_refuses_in_progress_operations() {
        let (_temp, local, _) = fixture();
        let git_dir = status_directory(&local).unwrap().unwrap();
        let lock = repository_lock(&git_dir).unwrap();
        assert!(pull_source(&local).is_err());
        assert!(freshness(&local, true).is_err());
        drop(lock);
        fs::create_dir(git_dir.join("rebase-merge")).unwrap();
        assert!(pull_source(&local).is_err());
    }

    fn run(root: &Path, args: &[&str]) {
        checked(root, args).unwrap();
    }
    fn fixture() -> (tempfile::TempDir, std::path::PathBuf, std::path::PathBuf) {
        let temp = tempfile::tempdir().unwrap();
        let remote = temp.path().join("remote.git");
        let local = temp.path().join("notes");
        fs::create_dir(&local).unwrap();
        run(temp.path(), &["init", "--bare", remote.to_str().unwrap()]);
        run(&local, &["init"]);
        run(&local, &["config", "user.name", "Memory Test"]);
        run(&local, &["config", "user.email", "memory@example.test"]);
        fs::write(local.join("seed.md"), "# Seed\n").unwrap();
        run(&local, &["add", "seed.md"]);
        run(&local, &["commit", "-m", "Seed"]);
        run(
            &local,
            &["remote", "add", "origin", remote.to_str().unwrap()],
        );
        run(&local, &["push", "-u", "origin", "HEAD"]);
        (temp, local, remote)
    }
    #[test]
    fn commits_only_visible_markdown_and_retries_pending_pushes() {
        let (_temp, local, remote) = fixture();
        fs::write(local.join("new.md"), "# New\n").unwrap();
        fs::write(local.join("secret.txt"), "do not commit").unwrap();
        fs::create_dir(local.join(".agent-memory-history")).unwrap();
        fs::write(local.join(".agent-memory-history/snapshot.md"), "hidden").unwrap();
        fs::create_dir(local.join(".agent-memory-operations")).unwrap();
        // Use a valid immutable journal: synchronization now inspects journals
        // to keep unfinished batches private, while still ignoring their payloads.
        memory_core::save_operation(&local, br#"{"private_plan":true}"#).unwrap();
        fs::write(
            local.join(".agent-memory-operations/snapshot.md"),
            "private content",
        )
        .unwrap();
        fs::write(local.join(".gitignore"), "ignored.md\n").unwrap();
        fs::write(local.join("ignored.md"), "ignored").unwrap();
        fs::remove_file(local.join("seed.md")).unwrap();
        sync_source(&local).unwrap();
        let status = crate::git_status::read(&local).unwrap().unwrap();
        assert_eq!(status.status, "ok");
        assert!(status.last_success_at.is_some());
        let files = checked(&local, &["ls-tree", "-r", "--name-only", "HEAD"]).unwrap();
        assert_eq!(files.trim(), "new.md");
        let head = checked(&local, &["rev-parse", "HEAD"]).unwrap();
        sync_source(&local).unwrap();
        assert_eq!(checked(&local, &["rev-parse", "HEAD"]).unwrap(), head);
        assert_eq!(checked(&remote, &["rev-parse", "HEAD"]).unwrap(), head);
        fs::write(local.join("pending.md"), "pending").unwrap();
        run(&local, &["add", "pending.md"]);
        run(&local, &["commit", "-m", "Pending local"]);
        sync_source(&local).unwrap();
        assert_eq!(
            checked(&remote, &["rev-parse", "HEAD"]).unwrap(),
            checked(&local, &["rev-parse", "HEAD"]).unwrap()
        );
    }
    #[test]
    fn refuses_staged_changes_and_nested_source() {
        let (_temp, local, _) = fixture();
        fs::write(local.join("user.txt"), "staged").unwrap();
        run(&local, &["add", "user.txt"]);
        fs::write(local.join("new.md"), "new").unwrap();
        assert!(sync_source(&local)
            .unwrap_err()
            .to_string()
            .contains("staged"));
        assert!(!checked(&local, &["diff", "--cached", "--name-only"])
            .unwrap()
            .contains("new.md"));
        fs::create_dir(local.join("nested")).unwrap();
        assert!(sync_source(&local.join("nested"))
            .unwrap_err()
            .to_string()
            .contains("repository root"));
    }
    #[test]
    fn non_fast_forward_retains_local_commit() {
        let (temp, local, remote) = fixture();
        let other = temp.path().join("other");
        run(
            temp.path(),
            &["clone", remote.to_str().unwrap(), other.to_str().unwrap()],
        );
        run(&other, &["config", "user.name", "Other"]);
        run(&other, &["config", "user.email", "other@example.test"]);
        fs::write(other.join("other.md"), "other").unwrap();
        run(&other, &["add", "other.md"]);
        run(&other, &["commit", "-m", "Other"]);
        run(&other, &["push"]);
        fs::write(local.join("local.md"), "local").unwrap();
        assert!(sync_source(&local).is_err());
        let status = crate::git_status::read(&local).unwrap().unwrap();
        assert_eq!(status.status, "error");
        assert_eq!(
            status.last_error.as_deref(),
            Some("Git push failed; local commits are retained")
        );
        assert!(checked(&local, &["show", "HEAD:local.md"]).is_ok());
        assert!(checked(&local, &["diff", "--cached", "--name-only"])
            .unwrap()
            .is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn replaced_source_root_cannot_redirect_status_or_sync() {
        let (temp, local, _) = fixture();
        let moved = temp.path().join("moved");
        fs::rename(&local, &moved).unwrap();
        std::os::unix::fs::symlink(&moved, &local).unwrap();
        assert!(crate::git_status::read(&local).is_err());
        assert!(sync_source(&local).is_err());
        assert!(!moved.join(".git/agent-memory-sync-status.json").exists());
    }
}
