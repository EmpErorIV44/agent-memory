//! Local index maintenance; no note writes, Git operations or network listeners.
use anyhow::{ensure, Context, Result};
use memory_sqlite::Store;
use serde::Serialize;
use serde_json::{json, Value};
use std::{
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

#[derive(Serialize)]
struct ProjectState {
    project: String,
    last_success_at: Option<u128>,
    last_error: Option<String>,
    index_runs: u64,
    last_trigger: Option<String>,
    #[serde(skip)]
    fingerprint: Option<String>,
}

/// A successful RPC only coordinates indexing. Callers still enforce their own
/// project permissions and document hashes on every subsequent read.
pub fn try_refresh(database: &Path, project: &str) -> bool {
    crate::daemon_ipc::request(database, "refresh", Some(project))
        .is_ok_and(|reply| reply["ok"] == true && reply["indexed"] == true)
}

pub async fn run(
    database: PathBuf,
    projects: Vec<String>,
    poll_ms: u64,
    reconcile_secs: u64,
) -> Result<()> {
    ensure!((20..=60000).contains(&poll_ms), "invalid poll interval");
    ensure!(
        (1..=86400).contains(&reconcile_secs),
        "invalid reconciliation interval"
    );
    let stopping = Arc::new(AtomicBool::new(false));
    let worker_stop = stopping.clone();
    let mut worker = tokio::task::spawn_blocking(move || {
        run_blocking(&database, projects, poll_ms, reconcile_secs, worker_stop)
    });
    tokio::select! {
        result = &mut worker => result.context("daemon worker failed")?,
        signal = shutdown_signal() => {
            stopping.store(true, Ordering::Release);
            let result = worker.await.context("daemon worker failed")?;
            signal?;
            result
        }
    }
}

async fn shutdown_signal() -> Result<()> {
    #[cfg(unix)]
    {
        let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
        tokio::select! {
            result = tokio::signal::ctrl_c() => { result?; }
            _ = term.recv() => {}
        }
    }
    #[cfg(not(unix))]
    tokio::signal::ctrl_c().await?;
    Ok(())
}

fn fingerprint(store: &Store, project: &str) -> Result<String> {
    let mut sources = Vec::new();
    for source in store.sources(project)? {
        sources.push((
            source.source_id,
            source.root_path.clone(),
            crate::daemon_watch::fingerprint(Path::new(&source.root_path))?,
        ));
    }
    Ok(memory_core::content_hash(&serde_json::to_vec(&sources)?))
}

fn refresh(store: &mut Store, state: &mut ProjectState, trigger: &str) -> Result<()> {
    state.index_runs += 1;
    state.last_trigger = Some(trigger.into());
    let result = (|| -> Result<()> {
        // Only successful complete scans advance the remembered fingerprint.
        let observed = fingerprint(store, &state.project)?;
        store.index(&state.project)?;
        state.fingerprint = Some(observed);
        state.last_success_at = Some(SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis());
        Ok(())
    })();
    match result {
        Ok(()) => {
            state.last_error = None;
            Ok(())
        }
        Err(error) => {
            let message = format!("{error:#}");
            if state.last_error.as_ref() != Some(&message) {
                eprintln!("daemon index error for {}: {}", state.project, message);
            }
            state.last_error = Some(message);
            Err(error)
        }
    }
}

#[cfg(unix)]
fn database_identity(path: &Path) -> Result<(u64, u64)> {
    use std::os::unix::fs::MetadataExt;
    let metadata = std::fs::metadata(path)?;
    Ok((metadata.dev(), metadata.ino()))
}
#[cfg(not(unix))]
fn database_identity(_path: &Path) -> Result<(u64, u64)> {
    Ok((0, 0))
}

fn status(
    database: &Path,
    endpoint: &Path,
    projects: &[ProjectState],
    poll_ms: u64,
    reconcile_secs: u64,
) -> Value {
    json!({"running":true,"pid":std::process::id(),"database":database,"endpoint":endpoint,
        "healthy":projects.iter().all(|state| state.last_error.is_none()),
        "projects":projects,"poll_interval_ms":poll_ms,"reconcile_interval_secs":reconcile_secs,
        "watcher":"polling","local_only":true})
}

fn run_blocking(
    database: &Path,
    mut projects: Vec<String>,
    poll_ms: u64,
    reconcile_secs: u64,
    stopping: Arc<AtomicBool>,
) -> Result<()> {
    // Starting a daemon requires an owner-initialized existing database.
    let database = std::fs::canonicalize(database)
        .context("initialize the database before starting the daemon")?;
    ensure!(database.is_file(), "daemon database must be a regular file");
    projects.sort();
    projects.dedup();
    ensure!(!projects.is_empty(), "daemon requires at least one project");
    let lease = crate::daemon_ipc::Lease::acquire(&database)?;
    let identity = database_identity(&database)?;
    let mut store = Store::open(&database)?;
    let mut states = Vec::new();
    for project in projects {
        // Reject typos before entering the service loop.
        store.sources(&project)?;
        let mut state = ProjectState {
            project,
            last_success_at: None,
            last_error: None,
            index_runs: 0,
            last_trigger: None,
            fingerprint: None,
        };
        let _ = refresh(&mut store, &mut state, "startup");
        states.push(state);
    }
    println!(
        "{}",
        status(
            &database,
            lease.endpoint(),
            &states,
            poll_ms,
            reconcile_secs
        )
    );
    let mut next_poll = Instant::now() + Duration::from_millis(poll_ms);
    let mut next_full = Instant::now() + Duration::from_secs(reconcile_secs);
    while !stopping.load(Ordering::Acquire) {
        ensure!(
            database_identity(&database)? == identity,
            "database was replaced while daemon was running; restart it"
        );
        if let Some(mut connection) = lease.accept()? {
            let response = match connection.read_request() {
                Err(error) => json!({"ok":false,"error":error.to_string()}),
                Ok(request) => match request.command.as_str() {
                    "status" => status(
                        &database,
                        lease.endpoint(),
                        &states,
                        poll_ms,
                        reconcile_secs,
                    ),
                    "stop" => {
                        stopping.store(true, Ordering::Release);
                        json!({"ok":true,"stopping":true})
                    }
                    "refresh" => {
                        if let Some(state) = states.iter_mut().find(|state| {
                            Some(state.project.as_str()) == request.project.as_deref()
                        }) {
                            match refresh(&mut store, state, "request") {
                                Ok(()) => json!({"ok":true,"indexed":true,"project":state.project}),
                                Err(error) => {
                                    json!({"ok":false,"indexed":false,"error":format!("{error:#}")})
                                }
                            }
                        } else {
                            json!({"ok":false,"error":"Project is not allowed for this daemon"})
                        }
                    }
                    _ => json!({"ok":false,"error":"Unknown daemon command"}),
                },
            };
            // A disconnected client must not terminate background maintenance.
            let _ = connection.reply(&response);
        }
        let now = Instant::now();
        if now >= next_poll || now >= next_full {
            let full = now >= next_full;
            for state in &mut states {
                match fingerprint(&store, &state.project) {
                    Ok(observed)
                        if !full
                            && state.last_error.is_none()
                            && state.fingerprint.as_ref() == Some(&observed) => {}
                    _ => {
                        let _ = refresh(&mut store, state, if full { "reconcile" } else { "poll" });
                    }
                }
            }
            let after = Instant::now();
            next_poll = after + Duration::from_millis(poll_ms);
            if full {
                next_full = after + Duration::from_secs(reconcile_secs);
            }
        }
        std::thread::sleep(Duration::from_millis(poll_ms.min(50)));
    }
    Ok(())
}
