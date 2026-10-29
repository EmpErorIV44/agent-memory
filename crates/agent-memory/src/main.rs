mod mcp;

use std::path::PathBuf;

use anyhow::Result;
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
    },
    Init,
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
    } = cli.command
    {
        return mcp::run(cli.database, project, allow_write, write_policy).await;
    }
    let mut store = Store::open(&cli.database)?;
    match cli.command {
        Command::Mcp { .. } => unreachable!(),
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
