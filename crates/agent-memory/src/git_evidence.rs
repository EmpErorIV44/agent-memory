//! Local, owner-scoped evidence checks. References never select filesystem roots.
use anyhow::{ensure, Context, Result};
use cap_std::{ambient_authority, fs::Dir};
use memory_core::EvidenceKind;
use memory_sqlite::Store;
use serde_json::{json, Value};
use std::{
    collections::{HashMap, HashSet},
    fs,
    io::{Read, Seek, SeekFrom},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};

const MAX_FILE: u64 = 16 * 1024 * 1024;

#[derive(Default)]
pub struct Repositories(HashMap<(String, String), Repository>);

struct Repository {
    root: PathBuf,
    #[cfg(unix)]
    identity: (u64, u64),
}

impl Repositories {
    /// PROJECT:ALIAS=PATH comes exclusively from owner-controlled process arguments.
    pub fn new(specs: Vec<String>, projects: &HashSet<String>) -> Result<Self> {
        let mut result = Self::default();
        for spec in specs {
            let (key, path) = spec
                .split_once('=')
                .context("expected PROJECT:ALIAS=PATH")?;
            let (project, alias) = key.split_once(':').context("expected PROJECT:ALIAS=PATH")?;
            ensure!(
                projects.contains(project),
                "evidence repository project is not allowed"
            );
            ensure!(valid_alias(alias), "invalid evidence repository alias");
            let root = fs::canonicalize(path).context("cannot open evidence repository")?;
            let top = git(&root, &["rev-parse", "--show-toplevel"], None)?;
            ensure!(
                fs::canonicalize(String::from_utf8(top)?.trim())? == root,
                "evidence repository must be a working tree root"
            );
            #[cfg(unix)]
            let identity = {
                use std::os::unix::fs::MetadataExt;
                let metadata = fs::metadata(&root)?;
                (metadata.dev(), metadata.ino())
            };
            let repository = Repository {
                root,
                #[cfg(unix)]
                identity,
            };
            ensure!(
                result
                    .0
                    .insert((project.to_owned(), alias.to_owned()), repository)
                    .is_none(),
                "duplicate evidence repository alias"
            );
        }
        Ok(result)
    }

    pub fn verify(&self, store: &Store, project: &str, memory_id: &str) -> Result<Value> {
        // Authorize before inspecting any references or opening a Git repository.
        let record = store
            .get_memory(project, memory_id)?
            .context("Memory not found or not accessible")?;
        let evidence: Vec<_> = record.memory.evidence.iter().map(|item| {
            let mut value = json!({"kind":item.kind,"reference":item.reference,"status":"unknown","reason":"unsupported_reference"});
            if matches!(item.kind, EvidenceKind::File | EvidenceKind::Commit) {
                match self.check(project, &item.reference, item.kind == EvidenceKind::Commit) {
                    Ok(check) => { for (key, entry) in check.as_object().unwrap() { value[key] = entry.clone(); } }
                    Err(_) => value["reason"] = json!("repository_unavailable_or_check_failed"),
                }
            }
            value
        }).collect();
        Ok(
            json!({"memory_id":memory_id,"content_hash":record.content_hash,"local_only":true,"comparison":"raw_worktree_bytes","evidence":evidence}),
        )
    }

    fn check(&self, project: &str, reference: &str, commit_only: bool) -> Result<Value> {
        let unknown = |reason| json!({"status":"unknown","reason":reason});
        let Some(reference) = reference.strip_prefix("git:") else {
            return Ok(unknown("unsupported_reference"));
        };
        let Some((alias, target)) = reference.split_once('@') else {
            return Ok(unknown("invalid_git_reference"));
        };
        let (commit, path) = target
            .split_once(':')
            .map_or((target, None), |(c, p)| (c, Some(p)));
        if !valid_alias(alias)
            || !matches!(commit.len(), 40 | 64)
            || !commit.bytes().all(|b| b.is_ascii_hexdigit())
            || commit_only != path.is_none()
        {
            return Ok(unknown("invalid_git_reference"));
        }
        let Some(repository) = self.0.get(&(project.to_owned(), alias.to_owned())) else {
            return Ok(unknown("repository_not_configured"));
        };
        let root = &repository.root;
        if fs::canonicalize(root).ok().as_ref() != Some(root) {
            return Ok(unknown("repository_root_changed"));
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            let metadata = fs::metadata(root)?;
            if (metadata.dev(), metadata.ino()) != repository.identity {
                return Ok(unknown("repository_root_changed"));
            }
        }
        let kind = git(root, &["cat-file", "-t", commit], None);
        if !kind.is_ok_and(|v| v == b"commit\n") {
            return Ok(unknown("baseline_commit_unavailable"));
        }
        if commit_only {
            return Ok(
                json!({"status":"unchanged","reason":"commit_exists_locally","baseline_commit":commit.to_ascii_lowercase()}),
            );
        }
        let path = path.unwrap();
        if !valid_path(path) {
            return Ok(unknown("unsafe_path"));
        }
        let tree = git(root, &["ls-tree", "-z", commit, "--", path], None)?;
        let Some(tab) = tree.iter().position(|b| *b == b'\t') else {
            return Ok(unknown("baseline_file_unavailable"));
        };
        // Only an exact regular blob is supported: never directories, submodules or symlinks.
        if tree[tab + 1..] != [path.as_bytes(), b"\0"].concat() {
            return Ok(unknown("baseline_file_unavailable"));
        }
        let header = std::str::from_utf8(&tree[..tab])?;
        let parts: Vec<_> = header.split_whitespace().collect();
        if parts.len() != 3 || !matches!(parts[0], "100644" | "100755") || parts[1] != "blob" {
            return Ok(unknown("unsupported_file_type"));
        }
        let baseline = parts[2];
        let mut value =
            json!({"baseline_commit":commit.to_ascii_lowercase(),"baseline_blob":baseline});
        let mut directory = Dir::open_ambient_dir(root, ambient_authority())?;
        let components: Vec<_> = path.split('/').collect();
        // Open each directory as a capability; a symlink replacement cannot escape root.
        for component in &components[..components.len() - 1] {
            let metadata = match directory.symlink_metadata(component) {
                Ok(m) => m,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    value["status"] = json!("missing");
                    value["reason"] = json!("worktree_file_missing");
                    return Ok(value);
                }
                Err(e) => return Err(e.into()),
            };
            if !metadata.is_dir() || metadata.is_symlink() {
                return Ok(unknown("unsafe_worktree_path"));
            }
            let anchor = directory.into_std_file();
            directory = Dir::from_std_file(cap_primitives::fs::open_dir_nofollow(
                &anchor,
                Path::new(component),
            )?);
            if directory.symlink_metadata(".git").is_ok() {
                return Ok(unknown("nested_repository"));
            }
        }
        let name = components.last().unwrap();
        let metadata = match directory.symlink_metadata(name) {
            Ok(m) => m,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                value["status"] = json!("missing");
                value["reason"] = json!("worktree_file_missing");
                return Ok(value);
            }
            Err(e) => return Err(e.into()),
        };
        if !metadata.is_file() || metadata.is_symlink() {
            return Ok(unknown("unsupported_file_type"));
        }
        if metadata.len() > MAX_FILE {
            return Ok(unknown("file_too_large"));
        }
        let anchor = directory.into_std_file();
        let mut options = cap_primitives::fs::OpenOptions::new();
        options
            .read(true)
            ._cap_fs_ext_follow(cap_primitives::fs::FollowSymlinks::No);
        let file = cap_primitives::fs::open(&anchor, Path::new(name), &options)?;
        if !file.metadata()?.is_file() {
            return Ok(unknown("unsupported_file_type"));
        }
        let mut input = tempfile::tempfile()?;
        let count = std::io::copy(&mut file.take(MAX_FILE + 1), &mut input)?;
        if count > MAX_FILE {
            return Ok(unknown("file_too_large"));
        }
        input.seek(SeekFrom::Start(0))?;
        let current = String::from_utf8(git(
            root,
            &["hash-object", "--stdin", "--no-filters"],
            Some(input),
        )?)?;
        let current = current.trim();
        value["current_blob"] = json!(current);
        value["status"] = json!(if current == baseline {
            "unchanged"
        } else {
            "changed"
        });
        value["reason"] = json!(if current == baseline {
            "blob_matches"
        } else {
            "worktree_bytes_differ"
        });
        Ok(value)
    }
}

fn valid_alias(alias: &str) -> bool {
    !alias.is_empty()
        && alias.len() <= 64
        && alias
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
}

fn valid_path(path: &str) -> bool {
    !path.is_empty()
        && path.len() <= 4096
        && path.split('/').all(|p| {
            !p.is_empty()
                && p != "."
                && p != ".."
                && !p.eq_ignore_ascii_case(".git")
                && !p.chars().any(|c| c.is_control() || "\\:".contains(c))
        })
}

/// No shell, user revision expressions, filters, external diff, network, or inherited Git routing.
fn git(root: &Path, args: &[&str], input: Option<fs::File>) -> Result<Vec<u8>> {
    let mut output = tempfile::tempfile()?;
    let mut command = Command::new("git");
    for (name, _) in std::env::vars_os() {
        if name.to_string_lossy().starts_with("GIT_") {
            command.env_remove(name);
        }
    }
    command
        .args([
            "--no-replace-objects",
            "--literal-pathspecs",
            "-c",
            "core.fsmonitor=false",
            "-C",
        ])
        .arg(root)
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_NO_LAZY_FETCH", "1")
        .stdin(input.map_or_else(Stdio::null, Stdio::from))
        .stdout(Stdio::from(output.try_clone()?))
        .stderr(Stdio::null());
    let mut child = command.spawn().context("cannot launch Git")?;
    let start = Instant::now();
    loop {
        if let Some(status) = child.try_wait()? {
            ensure!(status.success(), "local Git check failed");
            break;
        }
        if start.elapsed() > Duration::from_secs(5) || output.metadata()?.len() > 1024 * 1024 {
            let _ = child.kill();
            let _ = child.wait();
            anyhow::bail!("local Git check exceeded limit");
        }
        thread::sleep(Duration::from_millis(5));
    }
    output.seek(SeekFrom::Start(0))?;
    let mut bytes = Vec::new();
    output.take(1024 * 1024 + 1).read_to_end(&mut bytes)?;
    ensure!(bytes.len() <= 1024 * 1024, "Git output exceeded limit");
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fixture {
        _temp: tempfile::TempDir,
        root: PathBuf,
        notes: PathBuf,
        commit: String,
        store: Store,
        repositories: Repositories,
    }
    fn command(root: &Path, args: &[&str]) -> String {
        let out = Command::new("git")
            .arg("-C")
            .arg(root)
            .args(args)
            .output()
            .unwrap();
        assert!(out.status.success(), "{:?}", out);
        String::from_utf8(out.stdout).unwrap().trim().to_owned()
    }
    impl Fixture {
        fn new() -> Result<Self> {
            let temp = tempfile::tempdir()?;
            let root = temp.path().join("code");
            let notes = temp.path().join("notes");
            fs::create_dir(&root)?;
            fs::create_dir(&notes)?;
            command(&root, &["init", "-q"]);
            command(&root, &["config", "user.name", "Test"]);
            command(&root, &["config", "user.email", "test@example.invalid"]);
            fs::write(root.join("код file.rs"), b"baseline\n")?;
            command(&root, &["add", "."]);
            command(&root, &["commit", "-qm", "baseline"]);
            let commit = command(&root, &["rev-parse", "HEAD"]);
            let store = Store::open(&temp.path().join("memory.sqlite"))?;
            store.create_project("demo")?;
            store.create_project("other")?;
            store.register_source("demo", &notes, true)?;
            let repositories = Repositories::new(
                vec![format!("demo:code={}", root.display())],
                &HashSet::from(["demo".to_owned(), "other".to_owned()]),
            )?;
            Ok(Self {
                _temp: temp,
                root,
                notes,
                commit,
                store,
                repositories,
            })
        }
        fn note(&mut self, items: Vec<(EvidenceKind, String)>) -> Result<Value> {
            let evidence: Vec<_> = items
                .into_iter()
                .map(|(kind, reference)| json!({"kind":kind,"reference":reference}))
                .collect();
            let memory = json!({"memory":{"id":"m","project":"demo","kind":"fact","status":"accepted","summary":"claim","evidence":evidence}});
            // JSON is valid YAML, including as a frontmatter document.
            fs::write(
                self.notes.join("m.md"),
                format!("---\n{memory}\n---\n# Claim\n"),
            )?;
            self.store.index("demo")?;
            self.repositories.verify(&self.store, "demo", "m")
        }
        fn file_ref(&self) -> String {
            format!("git:code@{}:код file.rs", self.commit)
        }
    }

    #[test]
    fn compares_worktree_bytes_not_head_and_keeps_legacy_evidence_inert() -> Result<()> {
        let mut f = Fixture::new()?;
        let reference = f.file_ref();
        let items = vec![
            (EvidenceKind::File, reference.clone()),
            (EvidenceKind::Commit, format!("git:code@{}", f.commit)),
            (
                EvidenceKind::Test,
                "touch /tmp/never-execute-evidence".into(),
            ),
            (EvidenceKind::File, "legacy.rs".into()),
        ];
        let report = f.note(items.clone())?;
        assert_eq!(report["evidence"][0]["status"], "unchanged");
        assert_eq!(report["evidence"][1]["status"], "unchanged");
        assert_eq!(report["evidence"][2]["status"], "unknown");
        assert_eq!(report["evidence"][3]["reason"], "unsupported_reference");
        fs::write(f.root.join("unrelated"), "other")?;
        command(&f.root, &["add", "."]);
        command(&f.root, &["commit", "-qm", "unrelated"]);
        assert_eq!(f.note(items.clone())?["evidence"][0]["status"], "unchanged");
        fs::write(f.root.join("код file.rs"), "dirty\n")?;
        assert_eq!(f.note(items.clone())?["evidence"][0]["status"], "changed");
        command(&f.root, &["add", "."]);
        assert_eq!(f.note(items.clone())?["evidence"][0]["status"], "changed");
        fs::remove_file(f.root.join("код file.rs"))?;
        assert_eq!(f.note(items)?["evidence"][0]["status"], "missing");
        assert_eq!(
            f.store.get_memory("demo", "m")?.unwrap().memory.status,
            memory_core::MemoryStatus::Accepted
        );
        Ok(())
    }

    #[test]
    fn rejects_unconfigured_roots_invalid_revisions_and_traversal() -> Result<()> {
        let mut f = Fixture::new()?;
        let refs = [
            format!("git:private@{}:x", f.commit),
            format!("git:code@{}:../secret", f.commit),
            "git:code@HEAD:x".into(),
            format!("git:code@{}:.git/config", f.commit),
            format!("git:code@{}:absent", f.commit),
            format!("git:code@{}:x", "f".repeat(40)),
        ];
        let report = f.note(refs.into_iter().map(|r| (EvidenceKind::File, r)).collect())?;
        for evidence in report["evidence"].as_array().unwrap() {
            assert_eq!(evidence["status"], "unknown");
        }
        assert_eq!(report["evidence"][0]["reason"], "repository_not_configured");
        assert!(f.repositories.verify(&f.store, "other", "m").is_err());
        assert_eq!(
            f.repositories.check("other", &f.file_ref(), false)?["reason"],
            "repository_not_configured"
        );
        f.store.register_source("demo", &f.notes, false)?;
        // Read-only sources can still verify, but revocation of can_read cannot.
        assert!(f.repositories.verify(&f.store, "demo", "m").is_ok());
        assert!(Repositories::new(
            vec![format!("other:code={}", f.root.display())],
            &HashSet::from(["demo".into()])
        )
        .is_err());
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn refuses_symlink_files_and_replaced_repository_root() -> Result<()> {
        use std::os::unix::fs::symlink;
        let mut f = Fixture::new()?;
        let reference = f.file_ref();
        fs::remove_file(f.root.join("код file.rs"))?;
        symlink("/etc/passwd", f.root.join("код file.rs"))?;
        assert_eq!(
            f.note(vec![(EvidenceKind::File, reference.clone())])?["evidence"][0]["status"],
            "unknown"
        );
        let saved = f.root.with_extension("saved");
        fs::rename(&f.root, &saved)?;
        symlink(&saved, &f.root)?;
        assert_eq!(
            f.repositories.check("demo", &reference, false)?["reason"],
            "repository_root_changed"
        );
        fs::remove_file(&f.root)?;
        fs::create_dir(&f.root)?;
        assert_eq!(
            f.repositories.check("demo", &reference, false)?["reason"],
            "repository_root_changed"
        );
        Ok(())
    }
}
