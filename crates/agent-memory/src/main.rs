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
    Init,
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
    Add {
        #[arg(long)]
        project: String,
        path: PathBuf,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let mut store = Store::open(&cli.database)?;
    match cli.command {
        Command::Init => println!("Initialized {}", cli.database.display()),
        Command::Project {
            command: ProjectCommand::Create { name },
        } => {
            store.create_project(&name)?;
            println!("Created project {name}");
        }
        Command::Source {
            command: SourceCommand::Add { project, path },
        } => {
            store.add_source(&project, &path)?;
            println!("Registered {} for {project}", path.display());
        }
        Command::Index { project } => println!("Indexed {} documents", store.index(&project)?),
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
