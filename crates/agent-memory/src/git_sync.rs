//! Opt-in synchronization of Markdown sources. Never pulls or force-pushes.
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
    checked(&root, &["symbolic-ref", "--quiet", "HEAD"])
        .context("Git sync requires an attached branch")?;
    checked(&root, &["rev-parse", "--verify", "@{upstream}"])
        .context("Git sync requires an existing upstream branch")?;
    for state in [
        "MERGE_HEAD",
        "CHERRY_PICK_HEAD",
        "REVERT_HEAD",
        "rebase-merge",
        "rebase-apply",
        "sequencer",
    ] {
        let state_path = checked(&root, &["rev-parse", "--git-path", state])?;
        ensure!(
            !root.join(state_path.trim()).exists(),
            "Git sync refuses an in-progress merge, rebase or sequencer operation"
        );
    }
    ensure!(
        checked(&root, &["ls-files", "--unmerged", "-z"])?.is_empty(),
        "Git sync refuses unresolved conflicts"
    );
    ensure!(
        checked(&root, &["diff", "--cached", "--name-only", "-z"])?.is_empty(),
        "Git sync refuses existing staged changes; commit or unstage them first"
    );
    let paths = checked(
        &root,
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
        if !visible_markdown(&root, path) {
            continue;
        }
        // Deleted symlinks have no filesystem entry; consult the index as well.
        let indexed = checked(&root, &["ls-files", "--stage", "--", path])?;
        if indexed.starts_with("120000 ") || indexed.starts_with("160000 ") {
            continue;
        }
        let ignored = git(
            &root,
            &["check-ignore", "--no-index", "--quiet", "--", path],
        )?;
        match ignored.status.code() {
            Some(0) => continue,
            Some(1) => {
                selected.insert(path);
            }
            _ => bail!("git check-ignore failed: {}", ignored.stderr.trim()),
        }
    }
    for path in &selected {
        checked(&root, &["add", "--all", "--", path])?;
    }
    let staged = checked(&root, &["diff", "--cached", "--name-only", "-z"])?;
    ensure!(
        staged
            .split('\0')
            .filter(|p| !p.is_empty())
            .all(|p| selected.contains(p)),
        "Git index changed concurrently; refusing to commit unrelated staged changes"
    );
    if !staged.is_empty() {
        checked(
            &root,
            &["commit", "--no-verify", "-m", "Sync agent memory Markdown"],
        )?;
    }
    // Use the explicitly configured upstream, independent of push.default or
    // remote push refspecs that might otherwise push additional branches.
    let branch = checked(&root, &["symbolic-ref", "--quiet", "--short", "HEAD"])?;
    let remote_key = format!("branch.{}.remote", branch.trim());
    let merge_key = format!("branch.{}.merge", branch.trim());
    let remote = checked(&root, &["config", "--get", &remote_key])?;
    let merge = checked(&root, &["config", "--get", &merge_key])?;
    ensure!(
        !remote.trim().starts_with('-') && merge.trim().starts_with("refs/heads/"),
        "invalid Git upstream configuration"
    );
    let refspec = format!("HEAD:{}", merge.trim());
    checked(&root, &["push", "--no-verify", remote.trim(), &refspec])
        .context("Git push failed; local commits are retained (no pull or force push attempted)")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
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
        fs::write(local.join(".gitignore"), "ignored.md\n").unwrap();
        fs::write(local.join("ignored.md"), "ignored").unwrap();
        fs::remove_file(local.join("seed.md")).unwrap();
        sync_source(&local).unwrap();
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
        assert!(checked(&local, &["show", "HEAD:local.md"]).is_ok());
        assert!(checked(&local, &["diff", "--cached", "--name-only"])
            .unwrap()
            .is_empty());
    }
}
