use std::{io::Write, path::Path};

use anyhow::{Context, Result};
use memory_sqlite::config::Config;

/// Read only the explicit owner manifest; never discover configuration in Markdown.
pub fn load(path: &Path) -> Result<Config> {
    let absolute = path
        .canonicalize()
        .with_context(|| format!("resolve config {}", path.display()))?;
    let content = std::fs::read_to_string(&absolute)
        .with_context(|| format!("read config {}", absolute.display()))?;
    let mut config: Config = serde_json::from_str(&content)
        .with_context(|| format!("parse config {}", absolute.display()))?;
    let parent = absolute
        .parent()
        .context("config has no parent directory")?;
    for project in &mut config.projects {
        for source in &mut project.sources {
            if source.path.is_relative() {
                source.path = parent.join(&source.path);
            }
        }
    }
    config.normalized()
}

/// Create a durable export without overwriting an existing owner configuration.
pub fn export(config: &Config, output: Option<&Path>) -> Result<()> {
    let content = serde_json::to_string_pretty(config)? + "\n";
    if let Some(path) = output {
        let parent = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
        // Publish a complete, flushed file in the same directory without replacing
        // any existing file (including symlinks). Errors remove the temporary file.
        let mut temporary = tempfile::Builder::new()
            .prefix(".agent-memory-config-")
            .tempfile_in(parent)
            .with_context(|| format!("create temporary config in {}", parent.display()))?;
        temporary.write_all(content.as_bytes())?;
        temporary.as_file().sync_all()?;
        temporary
            .persist_noclobber(path)
            .map_err(|error| error.error)
            .with_context(|| {
                format!(
                    "publish config {} (existing files are not overwritten)",
                    path.display()
                )
            })?;
        #[cfg(unix)]
        std::fs::File::open(parent)?.sync_all()?;
        println!("Exported config {}", path.display());
    } else {
        print!("{content}");
    }
    Ok(())
}
