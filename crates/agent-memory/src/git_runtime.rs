//! Opt-in scheduling around the Git adapter. Markdown saves never depend on push success.
use std::{
    collections::{BTreeMap, HashSet},
    path::PathBuf,
    sync::{Arc, Mutex},
    time::Duration,
};

use anyhow::{ensure, Context, Result};
use memory_sqlite::Store;
use serde_json::{json, Value};

pub struct Options {
    pub source_ids: Vec<i64>,
    pub after_write: bool,
    pub interval_secs: Option<u64>,
}

struct Target {
    root: PathBuf,
    projects: Vec<String>,
}

pub struct Runtime {
    database: PathBuf,
    targets: BTreeMap<i64, Target>,
    after_write: bool,
    interval: Duration,
    gate: Mutex<()>,
}

impl Runtime {
    pub fn new(
        database: PathBuf,
        projects: &HashSet<String>,
        allow_write: bool,
        options: Options,
    ) -> Result<Option<Arc<Self>>> {
        if options.source_ids.is_empty() {
            ensure!(
                !options.after_write && options.interval_secs.is_none(),
                "Git sync options require --git-sync-source"
            );
            return Ok(None);
        }
        ensure!(allow_write, "Git sync requires --allow-write");
        let interval_secs = match options.interval_secs {
            Some(seconds) => seconds,
            None => match std::env::var("AGENT_MEMORY_GIT_SYNC_INTERVAL_SECS") {
                Ok(value) => value
                    .parse::<u64>()
                    .context("AGENT_MEMORY_GIT_SYNC_INTERVAL_SECS must be positive seconds")?,
                Err(std::env::VarError::NotPresent) => 900,
                Err(error) => return Err(error.into()),
            },
        };
        ensure!(interval_secs > 0, "Git sync interval must be positive");
        let interval = Duration::from_secs(interval_secs);
        ensure!(
            std::time::Instant::now().checked_add(interval).is_some(),
            "Git sync interval is too large"
        );
        let store = Store::open(&database)?;
        let ids: HashSet<_> = options.source_ids.into_iter().collect();
        let mut targets = BTreeMap::new();
        for project in projects {
            for source in store.sources(project)? {
                if ids.contains(&source.source_id) && source.can_write {
                    targets
                        .entry(source.source_id)
                        .or_insert_with(|| Target {
                            root: PathBuf::from(&source.root_path),
                            projects: Vec::new(),
                        })
                        .projects
                        .push(project.clone());
                }
            }
        }
        ensure!(
            ids.len() == targets.len(),
            "Every Git sync source must be writable in an allowed project"
        );
        Ok(Some(Arc::new(Self {
            database,
            targets,
            after_write: options.after_write,
            interval,
            gate: Mutex::new(()),
        })))
    }

    fn authorized(&self, source_id: i64, target: &Target, project: Option<&str>) -> Result<()> {
        let store = Store::open(&self.database)?;
        for allowed in &target.projects {
            if project.is_some_and(|requested| requested != allowed) {
                continue;
            }
            if store.sources(allowed)?.into_iter().any(|source| {
                source.source_id == source_id
                    && source.can_write
                    && std::path::Path::new(&source.root_path) == target.root
            }) {
                return Ok(());
            }
        }
        anyhow::bail!("Git sync source is no longer writable in the allowed project scope")
    }

    pub fn after_saved_write(&self, project: &str, source_id: i64) -> Option<Value> {
        let target = self.targets.get(&source_id)?;
        if !self.after_write {
            return None;
        }
        let result = (|| -> Result<()> {
            let _guard = self
                .gate
                .lock()
                .map_err(|_| anyhow::anyhow!("Git sync scheduler lock failed"))?;
            self.authorized(source_id, target, Some(project))?;
            crate::git_sync::sync_source(&target.root)
        })();
        Some(match result {
            Ok(()) => json!({"status":"ok"}),
            Err(error) => json!({"status":"error", "error":format!("{error:#}")}),
        })
    }

    fn sync_all(&self) {
        let Ok(_guard) = self.gate.lock() else {
            eprintln!("Git sync scheduler lock failed");
            return;
        };
        for (&source_id, target) in &self.targets {
            if let Err(error) = self
                .authorized(source_id, target, None)
                .and_then(|()| crate::git_sync::sync_source(&target.root))
            {
                eprintln!("Git sync source {source_id}: {error:#}");
            }
        }
    }

    pub fn start(self: &Arc<Self>) -> Task {
        let runtime = self.clone();
        Task(tokio::spawn(async move {
            loop {
                tokio::time::sleep(runtime.interval).await;
                let next = runtime.clone();
                if let Err(error) = tokio::task::spawn_blocking(move || next.sync_all()).await {
                    eprintln!("Git sync task failed: {error}");
                }
            }
        }))
    }
}

pub struct Task(tokio::task::JoinHandle<()>);
impl Drop for Task {
    fn drop(&mut self) {
        self.0.abort();
    }
}
