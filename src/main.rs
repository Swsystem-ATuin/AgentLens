mod auth;
mod authorization;
mod db;
mod fingerprint;
mod health;
mod identity;
mod ingest;
mod metrics;
mod postmortem;
mod preferences;
mod projects;
mod search;
mod sources;
mod tokens;
mod turns;
mod usage;
mod web;

use anyhow::Result;
use clap::{Parser, Subcommand, ValueEnum};
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "agent-lens", about = "Local observability for coding agents")]
struct Cli {
    #[arg(long, env = "AGENT_LENS_DB", default_value = "agent-lens.sqlite")]
    database: PathBuf,
    /// Persistent accounts and sessions, kept separate from the disposable index.
    #[arg(
        long,
        env = "AGENT_LENS_STATE_DB",
        default_value = "agent-lens-state.sqlite"
    )]
    state_database: PathBuf,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Safely rebuild the disposable index from raw agent homes.
    Rebuild {
        #[arg(long, env = "AGENT_LENS_DATA", default_value = "data")]
        data: PathBuf,
    },
    /// Import new or changed sources, preserving the existing index.
    Refresh {
        #[arg(long, env = "AGENT_LENS_DATA", default_value = "data")]
        data: PathBuf,
    },
    /// Print a sanitised diagnostic summary of the disposable index.
    Health,
    /// Provision, recover, disable, or revoke application accounts.
    Account {
        #[command(subcommand)]
        command: AccountCommand,
    },
    /// Serve the local web UI.
    Serve {
        #[arg(long, env = "AGENT_LENS_BIND", default_value = "127.0.0.1:3001")]
        bind: String,
        /// Serve without an account on a loopback address only.
        #[arg(long, env = "AGENT_LENS_LOCAL")]
        local: bool,
        /// Optional project roots and display timezone JSON file.
        #[arg(long, env = "AGENT_LENS_PROJECTS_FILE")]
        projects_file: Option<PathBuf>,
        /// Mark session cookies Secure; required behind an HTTPS reverse proxy.
        #[arg(long, env = "AGENT_LENS_SECURE_COOKIES")]
        secure_cookies: bool,
    },
}

#[derive(Subcommand)]
enum AccountCommand {
    /// Create a locally managed application account.
    Create {
        #[arg(long)]
        username: String,
        #[arg(long)]
        password_file: PathBuf,
        /// Account role; the first account defaults to admin and later accounts to viewer.
        #[arg(long, value_enum)]
        role: Option<RoleArg>,
    },
    /// Replace an account password and revoke all existing sessions.
    ResetPassword {
        #[arg(long)]
        username: String,
        #[arg(long)]
        password_file: PathBuf,
    },
    /// Disable an account and revoke all existing sessions.
    Disable {
        #[arg(long)]
        username: String,
    },
    /// Re-enable a disabled account without restoring revoked sessions.
    Enable {
        #[arg(long)]
        username: String,
    },
    /// Revoke every current session for an account.
    RevokeSessions {
        #[arg(long)]
        username: String,
    },
    /// Change whether an account administers all data or sees granted sources.
    SetRole {
        #[arg(long)]
        username: String,
        #[arg(long, value_enum)]
        role: RoleArg,
    },
    /// Allow a viewer to read one stable source ID.
    GrantSource {
        #[arg(long)]
        username: String,
        #[arg(long)]
        source_id: String,
    },
    /// Remove a viewer's access to one stable source ID.
    RevokeSource {
        #[arg(long)]
        username: String,
        #[arg(long)]
        source_id: String,
    },
    /// Print an account's role and stable source grants.
    Permissions {
        #[arg(long)]
        username: String,
    },
}

#[derive(Clone, Copy, ValueEnum)]
enum RoleArg {
    Admin,
    Viewer,
}

impl From<RoleArg> for auth::AccountRole {
    fn from(value: RoleArg) -> Self {
        match value {
            RoleArg::Admin => Self::Admin,
            RoleArg::Viewer => Self::Viewer,
        }
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();
    let cli = Cli::parse();
    match cli.command {
        Command::Rebuild { data } => {
            let rebuild = db::begin_rebuild(&cli.database).await?;
            let report = match ingest::ingest_data_root(rebuild.pool(), &data).await {
                Ok(report) => report,
                Err(error) => {
                    if let Err(discard_error) = rebuild.discard().await {
                        return Err(error.context(format!(
                            "discard staged rebuild after failure: {discard_error:#}"
                        )));
                    }
                    return Err(error);
                }
            };
            rebuild.publish().await?;
            println!(
                "indexed {} sessions, {} messages, {} tool calls; {} errors; partial files: {}; malformed lines: {}",
                report.sessions,
                report.messages,
                report.tool_calls,
                report.errors,
                report.partial_files,
                report.malformed_lines
            );
        }
        Command::Refresh { data } => {
            let pool = db::open(&cli.database).await?;
            let report = ingest::ingest_data_root(&pool, &data).await?;
            println!(
                "indexed {} sessions, {} messages, {} tool calls; skipped {} unchanged files; {} errors; partial files: {}; malformed lines: {}; {} files seen; removed {} sessions and {} source files",
                report.sessions,
                report.messages,
                report.tool_calls,
                report.skipped,
                report.errors,
                report.partial_files,
                report.malformed_lines,
                report.files_seen,
                report.removed_sessions,
                report.removed_files
            );
        }
        Command::Health => {
            let pool = db::open(&cli.database).await?;
            print!("{}", health::cli_report(&pool).await?);
        }
        Command::Account { command } => {
            let state = auth::StateStore::open(&cli.state_database).await?;
            match command {
                AccountCommand::Create {
                    username,
                    password_file,
                    role,
                } => {
                    auth::create_account(&state, &username, &password_file, role.map(Into::into))
                        .await?;
                    println!("application account created");
                }
                AccountCommand::ResetPassword {
                    username,
                    password_file,
                } => {
                    auth::reset_password(&state, &username, &password_file).await?;
                    println!("application account password reset; sessions revoked");
                }
                AccountCommand::Disable { username } => {
                    auth::set_enabled(&state, &username, false).await?;
                    println!("application account disabled; sessions revoked");
                }
                AccountCommand::Enable { username } => {
                    auth::set_enabled(&state, &username, true).await?;
                    println!("application account enabled");
                }
                AccountCommand::RevokeSessions { username } => {
                    auth::revoke_sessions(&state, &username).await?;
                    println!("application account sessions revoked");
                }
                AccountCommand::SetRole { username, role } => {
                    auth::set_role(&state, &username, role.into()).await?;
                    println!("application account role changed");
                }
                AccountCommand::GrantSource {
                    username,
                    source_id,
                } => {
                    auth::grant_source(&state, &username, &source_id).await?;
                    println!("application account source access granted");
                }
                AccountCommand::RevokeSource {
                    username,
                    source_id,
                } => {
                    auth::revoke_source(&state, &username, &source_id).await?;
                    println!("application account source access revoked");
                }
                AccountCommand::Permissions { username } => {
                    let permissions = auth::account_permissions(&state, &username).await?;
                    println!("username: {}", permissions.username);
                    println!("role: {}", permissions.role);
                    for source_key in permissions.source_keys {
                        println!("source: {source_key}");
                    }
                }
            }
        }
        Command::Serve {
            bind,
            local,
            projects_file,
            secure_cookies,
        } => {
            let projects = projects::ProjectConfig::load(projects_file.as_deref())?;
            if local {
                web::validate_local_bind(&bind)?;
            }
            let mut state = auth::StateStore::open(&cli.state_database).await?;
            if local {
                state.enable_local_mode();
            } else {
                state.require_active_account().await?;
            }
            let pool = db::open(&cli.database).await?;
            web::serve(pool, &bind, state, projects, secure_cookies).await?;
        }
    }
    Ok(())
}
