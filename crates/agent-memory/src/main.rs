mod config;
mod git_runtime;
mod git_sync;
mod mcp;

use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use memory_sqlite::Store;

#[derive(Parser)]
#[command(version, about = "Search local Markdown by project")]
struct Cli {
    #[arg(long, global = true, default_value = "agent-memory.sqlite")]
    database: PathBuf,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Serve MCP tools over stdin/stdout for explicitly allowed projects.
    Mcp {
        #[arg(long, required = true)]
        project: Vec<String>,
        /// Enable memory_write (also requires a writable source binding).
        #[arg(long)]
        allow_write: bool,
        #[arg(long, value_enum, default_value = "confirm")]
        write_policy: mcp::WritePolicy,
        /// Opt in writable source IDs for automatic Git commit and push.
        #[arg(long)]
        git_sync_source: Vec<i64>,
        /// Also sync an opted-in source after each saved MCP write or restore.
        #[arg(long)]
        git_sync_after_write: bool,
        /// Interval in seconds; otherwise AGENT_MEMORY_GIT_SYNC_INTERVAL_SECS or 900.
        #[arg(long)]
        git_sync_interval_secs: Option<u64>,
    },
    /// Commit and push visible Markdown in an explicitly writable Git source.
    GitSync {
        #[arg(long)]
        project: String,
        #[arg(long)]
        source_id: i64,
    },
    Init,
    /// Export owner-controlled project/source registrations outside the search index.
    Config {
        #[command(subcommand)]
        command: ConfigCommand,
    },
    /// Restore registrations and the complete index from an explicit owner config.
    Rebuild {
        #[arg(long)]
        config: PathBuf,
    },
    /// Inspect structured project knowledge projected from Markdown frontmatter.
    Memory {
        #[command(subcommand)]
        command: MemoryCommand,
    },
    /// Inspect durable Markdown revisions or restore one as a new guarded write.
    History {
        #[command(subcommand)]
        command: HistoryCommand,
    },
    /// Save complete Markdown from a file; source write permission is required.
    Write {
        #[arg(long)]
        project: String,
        #[arg(long)]
        source_id: i64,
        #[arg(long)]
        path: String,
        #[arg(long)]
        content_file: PathBuf,
        #[arg(long)]
        expected_hash: Option<String>,
    },
    Project {
        #[command(subcommand)]
        command: ProjectCommand,
    },
    Source {
        #[command(subcommand)]
        command: SourceCommand,
    },
    Index {
        #[arg(long)]
        project: String,
    },
    Search {
        #[arg(long)]
        project: String,
        query: String,
    },
}

#[derive(Subcommand)]
enum ProjectCommand {
    Create { name: String },
}

#[derive(Subcommand)]
enum ConfigCommand {
    Export {
        /// Write a new JSON file. Omit to print JSON to stdout.
        #[arg(long)]
        output: Option<PathBuf>,
    },
}

#[derive(Subcommand)]
enum MemoryCommand {
    List {
        #[arg(long)]
        project: String,
    },
    Get {
        #[arg(long)]
        project: String,
        id: String,
    },
}

#[derive(Subcommand)]
enum HistoryCommand {
    List {
        #[arg(long)]
        project: String,
        #[arg(long)]
        source_id: i64,
        #[arg(long)]
        path: String,
    },
    Get {
        #[arg(long)]
        project: String,
        #[arg(long)]
        source_id: i64,
        #[arg(long)]
        path: String,
        #[arg(long)]
        revision_id: String,
    },
    Restore {
        #[arg(long)]
        project: String,
        #[arg(long)]
        source_id: i64,
        #[arg(long)]
        path: String,
        #[arg(long)]
        revision_id: String,
        /// Current complete-document SHA-256; restoration never force-overwrites.
        #[arg(long)]
        expected_hash: String,
    },
}

#[derive(Subcommand)]
enum SourceCommand {
    List {
        #[arg(long)]
        project: String,
    },
    Add {
        #[arg(long)]
        project: String,
        path: PathBuf,
        /// Allow writes for this project/source binding. Re-register without this flag to revoke.
        #[arg(long)]
        write: bool,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    if let Command::Mcp {
        project,
        allow_write,
        write_policy,
        git_sync_source,
        git_sync_after_write,
        git_sync_interval_secs,
    } = cli.command
    {
        return mcp::run(
            cli.database,
            project,
            allow_write,
            write_policy,
            git_runtime::Options {
                source_ids: git_sync_source,
                after_write: git_sync_after_write,
                interval_secs: git_sync_interval_secs,
            },
        )
        .await;
    }
    // Parse and validate the complete owner config before opening/creating the DB.
    let rebuild_config = match &cli.command {
        Command::Rebuild { config } => Some(config::load(config)?),
        _ => None,
    };
    let mut store = Store::open(&cli.database)?;
    match cli.command {
        Command::Mcp { .. } => unreachable!(),
        Command::GitSync { project, source_id } => {
            let source = store
                .sources(&project)?
                .into_iter()
                .find(|source| source.source_id == source_id && source.can_write)
                .context("Git sync requires a writable source in the selected project")?;
            git_sync::sync_source(std::path::Path::new(&source.root_path))?;
            println!(
                "{}",
                serde_json::json!({"status":"ok", "source_id":source_id})
            );
        }
        Command::Config {
            command: ConfigCommand::Export { output },
        } => config::export(&store.export_config()?, output.as_deref())?,
        Command::Rebuild { .. } => {
            let stats = store.rebuild_config(&rebuild_config.expect("validated rebuild config"))?;
            println!(
                "Rebuilt {}: added={} updated={} unchanged={} deleted={}",
                cli.database.display(),
                stats.added,
                stats.updated,
                stats.unchanged,
                stats.deleted
            );
        }
        Command::Memory {
            command: MemoryCommand::List { project },
        } => {
            store.index(&project)?;
            println!(
                "{}",
                serde_json::to_string_pretty(&store.list_memories(&project)?)?
            );
        }
        Command::Memory {
            command: MemoryCommand::Get { project, id },
        } => {
            store.index(&project)?;
            let memory = store
                .get_memory(&project, &id)?
                .context("Memory not found or inaccessible")?;
            println!("{}", serde_json::to_string_pretty(&memory)?);
        }
        Command::History { command } => match command {
            HistoryCommand::List {
                project,
                source_id,
                path,
            } => println!(
                "{}",
                serde_json::to_string_pretty(&store.list_revisions(&project, source_id, &path)?)?
            ),
            HistoryCommand::Get {
                project,
                source_id,
                path,
                revision_id,
            } => {
                let revision = store
                    .get_revision(&project, source_id, &path, &revision_id)?
                    .context("Revision not found or inaccessible")?;
                println!("{}", serde_json::to_string_pretty(&revision)?);
            }
            HistoryCommand::Restore {
                project,
                source_id,
                path,
                revision_id,
                expected_hash,
            } => println!(
                "{}",
                serde_json::to_string_pretty(&store.restore_revision(
                    &project,
                    source_id,
                    &path,
                    &revision_id,
                    &expected_hash,
                )?)?
            ),
        },
        Command::Write {
            project,
            source_id,
            path,
            content_file,
            expected_hash,
        } => {
            let content = std::fs::read_to_string(content_file)?;
            let result = store.write(
                &project,
                source_id,
                &path,
                &content,
                expected_hash.as_deref(),
            )?;
            println!("{}", serde_json::to_string_pretty(&result)?);
        }
        Command::Source {
            command: SourceCommand::List { project },
        } => {
            println!(
                "{}",
                serde_json::to_string_pretty(&store.sources(&project)?)?
            );
        }
        Command::Init => println!("Initialized {}", cli.database.display()),
        Command::Project {
            command: ProjectCommand::Create { name },
        } => {
            store.create_project(&name)?;
            println!("Created project {name}");
        }
        Command::Source {
            command:
                SourceCommand::Add {
                    project,
                    path,
                    write,
                },
        } => {
            let source_id = store.register_source(&project, &path, write)?;
            println!(
                "Registered {} for {project}: source_id={source_id} can_write={write}",
                path.display()
            );
        }
        Command::Index { project } => {
            let stats = store.index(&project)?;
            println!(
                "added={} updated={} unchanged={} deleted={}",
                stats.added, stats.updated, stats.unchanged, stats.deleted
            );
        }
        Command::Search { project, query } => {
            for hit in store.search(&project, &query)? {
                println!(
                    "{}:{}-{}\n{}\n{}\n",
                    hit.path, hit.start_line, hit.end_line, hit.heading, hit.body
                );
            }
        }
    }
    Ok(())
}
