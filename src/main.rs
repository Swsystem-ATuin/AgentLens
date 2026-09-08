mod db;
mod ingest;
mod web;

use anyhow::Result;
use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "agent-lens", about = "Local observability for coding agents")]
struct Cli {
    #[arg(long, env = "AGENT_LENS_DB", default_value = "agent-lens.sqlite")]
    database: PathBuf,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Delete and rebuild the disposable index from raw agent homes.
    Rebuild {
        #[arg(long, default_value = "data")]
        data: PathBuf,
    },
    /// Serve the local web UI.
    Serve {
        #[arg(long, default_value = "127.0.0.1:3000")]
        bind: String,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();
    let cli = Cli::parse();
    match cli.command {
        Command::Rebuild { data } => {
            let pool = db::rebuild(&cli.database).await?;
            let report = ingest::ingest_data_root(&pool, &data).await?;
            println!(
                "indexed {} sessions, {} messages, {} tool calls; {} errors",
                report.sessions, report.messages, report.tool_calls, report.errors
            );
        }
        Command::Serve { bind } => {
            let pool = db::open(&cli.database).await?;
            web::serve(pool, &bind).await?;
        }
    }
    Ok(())
}
