mod auth;
mod db;
mod fingerprint;
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
    /// Import new or changed sources, preserving the existing index.
    Refresh {
        #[arg(long, default_value = "data")]
        data: PathBuf,
    },
    /// Serve the local web UI.
    Serve {
        #[arg(long, default_value = "127.0.0.1:3001")]
        bind: String,
        #[arg(long, env = "AGENT_LENS_AUTH_FILE", default_value = "auth.local.json")]
        auth_file: PathBuf,
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
        Command::Refresh { data } => {
            let pool = db::open(&cli.database).await?;
            let report = ingest::ingest_data_root(&pool, &data).await?;
            println!(
                "indexed {} sessions, {} messages, {} tool calls; skipped {} unchanged files; {} errors; {} files seen",
                report.sessions,
                report.messages,
                report.tool_calls,
                report.skipped,
                report.errors,
                report.files_seen
            );
        }
        Command::Serve { bind, auth_file } => {
            let credentials = auth::Credentials::load(&auth_file)?;
            let pool = db::open(&cli.database).await?;
            web::serve(pool, &bind, credentials).await?;
        }
    }
    Ok(())
}
