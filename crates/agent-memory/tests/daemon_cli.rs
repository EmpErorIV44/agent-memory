#![cfg(unix)]

use anyhow::{Context, Result};
use memory_sqlite::Store;
use std::{
    fs,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant},
};

struct RunningDaemon {
    child: Child,
    log: PathBuf,
}

impl RunningDaemon {
    fn start(database: &Path, log: &Path) -> Result<Self> {
        let child = Command::new(env!("CARGO_BIN_EXE_agent-memory"))
            .arg("--database")
            .arg(database)
            .args([
                "daemon",
                "run",
                "--project",
                "demo",
                "--poll-interval-ms",
                "50",
                "--reconcile-interval-secs",
                "1",
            ])
            .stdout(Stdio::null())
            .stderr(fs::File::create(log)?)
            .spawn()?;
        Ok(Self {
            child,
            log: log.into(),
        })
    }

    fn wait(&mut self, mut ready: impl FnMut() -> bool) -> Result<()> {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if ready() {
                return Ok(());
            }
            anyhow::ensure!(
                self.child.try_wait()?.is_none(),
                "daemon exited: {}",
                fs::read_to_string(&self.log).unwrap_or_default()
            );
            anyhow::ensure!(
                Instant::now() < deadline,
                "daemon timed out: {}",
                fs::read_to_string(&self.log).unwrap_or_default()
            );
            thread::sleep(Duration::from_millis(20));
        }
    }
}

impl Drop for RunningDaemon {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn cli(database: &Path, command: &str) -> Result<std::process::Output> {
    Ok(Command::new(env!("CARGO_BIN_EXE_agent-memory"))
        .arg("--database")
        .arg(database)
        .args(["daemon", command])
        .output()?)
}

fn hits(database: &Path, query: &str) -> Vec<memory_sqlite::SearchHit> {
    Store::open(database)
        .and_then(|s| s.search("demo", query))
        .unwrap_or_default()
}

#[test]
fn daemon_indexes_external_changes_rolls_back_bad_scans_and_reconciles_same_timestamp() -> Result<()>
{
    let temp = tempfile::tempdir()?;
    let root = temp.path().join("notes");
    fs::create_dir(&root)?;
    let document = "---\nid: stable\n---\n# Note\nfirstmarker\n";
    fs::write(root.join("note.md"), document)?;
    let database = temp.path().join("index.sqlite");
    let store = Store::open(&database)?;
    store.create_project("demo")?;
    let source = store.register_source("demo", &root, false)?;
    drop(store);
    let mut daemon = RunningDaemon::start(&database, &temp.path().join("daemon.log"))?;
    daemon.wait(|| !hits(&database, "firstmarker").is_empty())?;
    let original_id = hits(&database, "firstmarker")[0].document_id;
    let status = cli(&database, "status")?;
    assert!(
        status.status.success(),
        "{}",
        String::from_utf8_lossy(&status.stderr)
    );
    serde_json::from_slice::<serde_json::Value>(&status.stdout).context("JSON daemon status")?;
    fs::write(root.join("new.md"), "# New\ncreatedmarker\n")?;
    daemon.wait(|| !hits(&database, "createdmarker").is_empty())?;
    fs::rename(root.join("note.md"), root.join("renamed.md"))?;
    daemon.wait(|| {
        hits(&database, "firstmarker")
            .first()
            .is_some_and(|h| h.path == "renamed.md")
    })?;
    assert_eq!(hits(&database, "firstmarker")[0].document_id, original_id);
    // Failed source scans must retain the previous complete index, including deletions.
    fs::write(
        root.join("broken.md"),
        "---\ncreated: invalid\n---\n# Broken\n",
    )?;
    fs::remove_file(root.join("new.md"))?;
    daemon.wait(|| {
        cli(&database, "status")
            .ok()
            .and_then(|output| serde_json::from_slice::<serde_json::Value>(&output.stdout).ok())
            .is_some_and(|status| status["healthy"] == false)
    })?;
    assert!(!hits(&database, "createdmarker").is_empty());
    assert!(!hits(&database, "firstmarker").is_empty());
    assert!(daemon.child.try_wait()?.is_none());
    fs::remove_file(root.join("broken.md"))?;
    daemon.wait(|| hits(&database, "createdmarker").is_empty())?;
    let recovered: serde_json::Value = serde_json::from_slice(&cli(&database, "status")?.stdout)?;
    assert_eq!(recovered["healthy"], true);
    let note = root.join("renamed.md");
    let modified = fs::metadata(&note)?.modified()?;
    fs::write(&note, document.replace("firstmarker", "othermarker"))?;
    fs::File::options()
        .write(true)
        .open(&note)?
        .set_times(fs::FileTimes::new().set_modified(modified))?;
    daemon.wait(|| !hits(&database, "othermarker").is_empty())?;
    assert!(hits(&database, "firstmarker").is_empty());
    assert!(!root.join(".agent-memory-history").exists());
    assert!(!root.join(".agent-memory-operations").exists());
    assert!(!root.join(".agent-memory.lock").exists());
    let connection = rusqlite::Connection::open(&database)?;
    connection.execute(
        "UPDATE project_sources SET can_read=0 WHERE source_id=?1",
        [source],
    )?;
    assert!(hits(&database, "othermarker").is_empty());
    let stop = cli(&database, "stop")?;
    assert!(
        stop.status.success(),
        "{}",
        String::from_utf8_lossy(&stop.stderr)
    );
    let deadline = Instant::now() + Duration::from_secs(5);
    while daemon.child.try_wait()?.is_none() && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(20));
    }
    assert!(daemon.child.try_wait()?.is_some(), "daemon did not stop");
    Ok(())
}

#[test]
fn daemon_singleton_survives_database_aliases_crash_and_restart() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let root = temp.path().join("notes");
    fs::create_dir(&root)?;
    fs::write(root.join("note.md"), "# Note\ninitialmarker\n")?;
    let database = temp.path().join("index.sqlite");
    let store = Store::open(&database)?;
    store.create_project("demo")?;
    store.register_source("demo", &root, false)?;
    drop(store);
    let mut first = RunningDaemon::start(&database, &temp.path().join("first.log"))?;
    first.wait(|| !hits(&database, "initialmarker").is_empty())?;
    let alias = temp.path().join("alias.sqlite");
    std::os::unix::fs::symlink(&database, &alias)?;
    let second = Command::new(env!("CARGO_BIN_EXE_agent-memory"))
        .arg("--database")
        .arg(&alias)
        .args(["daemon", "run", "--project", "demo"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    let mut second = RunningDaemon {
        child: second,
        log: temp.path().join("unused.log"),
    };
    let deadline = Instant::now() + Duration::from_secs(5);
    while second.child.try_wait()?.is_none() && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(20));
    }
    assert!(
        second.child.try_wait()?.is_some_and(|s| !s.success()),
        "duplicate daemon did not reject a database alias"
    );
    assert!(cli(&alias, "status")?.status.success());
    first.child.kill()?;
    first.child.wait()?;
    fs::write(root.join("note.md"), "# Note\nrestartedmarker\n")?;
    let mut restarted = RunningDaemon::start(&database, &temp.path().join("restarted.log"))?;
    restarted.wait(|| !hits(&database, "restartedmarker").is_empty())?;
    assert!(cli(&database, "stop")?.status.success());
    Ok(())
}

#[test]
fn multiple_projects_start_independently_and_bad_source_recovers_without_restart() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let demo = temp.path().join("demo");
    let other = temp.path().join("other");
    fs::create_dir(&demo)?;
    fs::create_dir(&other)?;
    fs::write(demo.join("note.md"), "---\ncreated: invalid\n---\n# Demo\n")?;
    fs::write(other.join("note.md"), "# Other\notherprojectmarker\n")?;
    let database = temp.path().join("index.sqlite");
    let store = Store::open(&database)?;
    for (project, root) in [("demo", &demo), ("other", &other)] {
        store.create_project(project)?;
        store.register_source(project, root, false)?;
    }
    drop(store);
    let log = temp.path().join("multi.log");
    let child = Command::new(env!("CARGO_BIN_EXE_agent-memory"))
        .arg("--database")
        .arg(&database)
        .args([
            "daemon",
            "run",
            "--project",
            "demo",
            "--project",
            "other",
            "--poll-interval-ms",
            "50",
        ])
        .stdout(Stdio::null())
        .stderr(fs::File::create(&log)?)
        .spawn()?;
    let mut daemon = RunningDaemon { child, log };
    daemon.wait(|| {
        Store::open(&database)
            .and_then(|s| s.search("other", "otherprojectmarker"))
            .is_ok_and(|hits| hits.len() == 1)
    })?;
    let status: serde_json::Value = serde_json::from_slice(&cli(&database, "status")?.stdout)?;
    assert_eq!(status["healthy"], false);
    assert_eq!(status["projects"].as_array().unwrap().len(), 2);
    assert!(status["projects"]
        .as_array()
        .unwrap()
        .iter()
        .any(|p| p["project"] == "demo" && p["last_error"].is_string()));
    assert!(status["projects"]
        .as_array()
        .unwrap()
        .iter()
        .any(|p| p["project"] == "other" && p["last_error"].is_null()));
    fs::write(demo.join("note.md"), "# Demo\nrecoveredprojectmarker\n")?;
    daemon.wait(|| !hits(&database, "recoveredprojectmarker").is_empty())?;
    assert!(Store::open(&database)?
        .search("demo", "otherprojectmarker")?
        .is_empty());
    assert!(cli(&database, "stop")?.status.success());
    Ok(())
}

#[test]
fn missing_source_retains_last_good_index_and_sigterm_stops_gracefully() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let root = temp.path().join("notes");
    let moved = temp.path().join("temporarily-unavailable");
    fs::create_dir(&root)?;
    let content = "# Note\nretainedmarker\n";
    fs::write(root.join("note.md"), content)?;
    let database = temp.path().join("index.sqlite");
    let store = Store::open(&database)?;
    store.create_project("demo")?;
    store.register_source("demo", &root, false)?;
    drop(store);
    let mut daemon = RunningDaemon::start(&database, &temp.path().join("missing.log"))?;
    daemon.wait(|| !hits(&database, "retainedmarker").is_empty())?;
    fs::rename(&root, &moved)?;
    daemon.wait(|| {
        cli(&database, "status")
            .ok()
            .and_then(|o| serde_json::from_slice::<serde_json::Value>(&o.stdout).ok())
            .is_some_and(|s| s["healthy"] == false)
    })?;
    assert!(!hits(&database, "retainedmarker").is_empty());
    fs::rename(&moved, &root)?;
    daemon.wait(|| {
        cli(&database, "status")
            .ok()
            .and_then(|o| serde_json::from_slice::<serde_json::Value>(&o.stdout).ok())
            .is_some_and(|s| s["healthy"] == true)
    })?;
    assert!(Command::new("kill")
        .args(["-TERM", &daemon.child.id().to_string()])
        .status()?
        .success());
    let deadline = Instant::now() + Duration::from_secs(5);
    while daemon.child.try_wait()?.is_none() && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(20));
    }
    assert!(
        daemon.child.try_wait()?.is_some_and(|s| s.success()),
        "SIGTERM did not finish gracefully"
    );
    assert_eq!(fs::read_to_string(root.join("note.md"))?, content);
    Ok(())
}

#[test]
fn database_replacement_stops_daemon_without_modifying_replacement() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let root = temp.path().join("notes");
    fs::create_dir(&root)?;
    fs::write(root.join("note.md"), "# Note\nreplacementguardmarker\n")?;
    let database = temp.path().join("index.sqlite");
    let store = Store::open(&database)?;
    store.create_project("demo")?;
    store.register_source("demo", &root, false)?;
    drop(store);
    let replacement = temp.path().join("replacement.sqlite");
    let store = Store::open(&replacement)?;
    store.create_project("sentinel")?;
    drop(store);
    let expected = fs::read(&replacement)?;
    let mut daemon = RunningDaemon::start(&database, &temp.path().join("replacement.log"))?;
    daemon.wait(|| !hits(&database, "replacementguardmarker").is_empty())?;
    fs::rename(&database, temp.path().join("old.sqlite"))?;
    fs::rename(&replacement, &database)?;
    let deadline = Instant::now() + Duration::from_secs(5);
    while daemon.child.try_wait()?.is_none() && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(20));
    }
    assert!(
        daemon.child.try_wait()?.is_some_and(|s| !s.success()),
        "daemon did not detect replacement"
    );
    assert_eq!(fs::read(&database)?, expected);
    Ok(())
}

#[test]
fn hardlinked_database_paths_are_rejected_before_serving() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let database = temp.path().join("index.sqlite");
    let store = Store::open(&database)?;
    store.create_project("demo")?;
    drop(store);
    let alias = temp.path().join("hardlink.sqlite");
    fs::hard_link(&database, &alias)?;
    let mut daemon = RunningDaemon::start(&alias, &temp.path().join("hardlink.log"))?;
    let deadline = Instant::now() + Duration::from_secs(5);
    while daemon.child.try_wait()?.is_none() && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(20));
    }
    assert!(
        daemon.child.try_wait()?.is_some_and(|s| !s.success()),
        "hardlinked database was accepted"
    );
    Ok(())
}
