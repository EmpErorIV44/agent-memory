//! Owner-controlled registrations. Markdown metadata never grants these permissions.

use std::{collections::HashSet, path::PathBuf};

use anyhow::{ensure, Context, Result};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub version: u32,
    pub projects: Vec<ProjectConfig>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectConfig {
    pub name: String,
    pub sources: Vec<SourceConfig>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceConfig {
    pub path: PathBuf,
    #[serde(default = "read_default")]
    pub can_read: bool,
    #[serde(default)]
    pub can_write: bool,
}

fn read_default() -> bool {
    true
}

impl Config {
    /// Validate the complete owner manifest before changing any registration.
    /// Relative roots are resolved against the current directory; CLI callers first
    /// resolve them against the manifest's directory so invocation CWD is irrelevant.
    pub fn normalized(&self) -> Result<Self> {
        ensure!(
            self.version == 1,
            "unsupported config version {}",
            self.version
        );
        let mut names = HashSet::new();
        let mut normalized = self.clone();
        for project in &mut normalized.projects {
            memory_core::validate_document_id(&project.name)
                .context("invalid project name in config")?;
            ensure!(
                names.insert(project.name.clone()),
                "duplicate project in config: {}",
                project.name
            );
            let mut roots = HashSet::new();
            for source in &mut project.sources {
                ensure!(
                    source.can_read || !source.can_write,
                    "write permission requires read permission for project {}",
                    project.name
                );
                ensure!(!source.path.as_os_str().is_empty(), "source path is empty");
                source.path = source.path.canonicalize().with_context(|| {
                    format!(
                        "resolve source {} for {}",
                        source.path.display(),
                        project.name
                    )
                })?;
                ensure!(
                    source.path.is_dir(),
                    "source is not a directory: {}",
                    source.path.display()
                );
                ensure!(source.path.to_str().is_some(), "source path must be UTF-8");
                ensure!(
                    roots.insert(source.path.clone()),
                    "duplicate source in config for {}: {}",
                    project.name,
                    source.path.display()
                );
            }
        }
        Ok(normalized)
    }
}
