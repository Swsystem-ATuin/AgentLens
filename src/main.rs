mod access;
mod admin;
mod auth;
mod authorization;
mod db;
mod fingerprint;
mod health;
mod identity;
mod ingest;
mod local_preferences;
mod metrics;
mod mfa;
mod oidc;
mod passkeys;
#[cfg(windows)]
mod personal;
mod postmortem;
mod preferences;
mod projects;
mod redaction;
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
#[command(name = "agent-lens", version, about = "Local observability for coding agents")]
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
    /// Owner-only MFA encryption keyring; defaults beside the persistent state database.
    #[arg(long, env = "AGENT_LENS_MFA_KEY_FILE")]
    mfa_key_file: Option<PathBuf>,
    /// Import masking policy, stored outside copied agent homes.
    #[arg(long, env = "AGENT_LENS_REDACTION_FILE")]
    redaction_file: Option<PathBuf>,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Rebuild and open your personal Usage dashboard on Windows.
    #[cfg(windows)]
    Personal {
        #[arg(long, default_value_t = 3001, value_parser = clap::value_parser!(u16).range(1..))]
        port: u16,
        /// Rebuild the index without starting a server.
        #[arg(long)]
        index_only: bool,
        /// Serve and print the URL without opening the browser.
        #[arg(long)]
        no_browser: bool,
    },
    /// Validate OIDC configuration without enabling sign-in; optionally check provider discovery.
    OidcCheck {
        #[arg(long)]
        config: PathBuf,
        /// Contact the configured HTTPS issuer and validate its discovery metadata.
        #[arg(long)]
        discover: bool,
        /// Load protected credentials and verified provider signing keys; does not enable sign-in.
        #[arg(long, conflicts_with = "discover")]
        prepare: bool,
        /// Check declared deployment settings and existing local administrator credentials.
        #[arg(long, requires_all = ["prepare", "public_url", "secure_cookies"])]
        deployment: bool,
        /// Declared HTTPS origin for the intended deployment.
        #[arg(long, requires = "deployment")]
        public_url: Option<String>,
        /// Confirm the intended deployment will use Secure session cookies.
        #[arg(long, requires = "deployment")]
        secure_cookies: bool,
    },
    /// Safely rebuild the disposable index from raw agent homes.
    Rebuild {
        /// Source tree, defaulting to AGENT_LENS_DATA or data/.
        #[arg(
            long,
            conflicts_with_all = ["codex_home", "gemini_home", "hermes_home"]
        )]
        data: Option<PathBuf>,
        /// Read a personal Codex home in place.
        #[arg(long)]
        codex_home: Option<PathBuf>,
        /// Read a personal Gemini CLI home in place.
        #[arg(long)]
        gemini_home: Option<PathBuf>,
        /// Read a personal Hermes home in place.
        #[arg(long)]
        hermes_home: Option<PathBuf>,
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
    /// Manage teams and their additive source grants through local recovery access.
    Team {
        #[command(subcommand)]
        command: TeamCommand,
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
        /// Exact public HTTPS origin required before enabling passkey second factors.
        #[arg(long, env = "AGENT_LENS_PUBLIC_URL")]
        public_url: Option<String>,
    },
}

#[derive(Subcommand)]
enum AccountCommand {
    /// Link an exact configured issuer/subject to an existing enabled account; does not enable OIDC.
    LinkOidc {
        #[arg(long)]
        username: String,
        #[arg(long)]
        config: PathBuf,
        /// Owner-only file containing the subject; never pass identity claims in command arguments.
        #[arg(long)]
        subject_file: PathBuf,
    },
    /// Remove one exact identity link and revoke the account's existing credentials.
    UnlinkOidc {
        #[arg(long)]
        username: String,
        #[arg(long)]
        link_id: String,
    },
    /// List bounded OIDC link metadata without disclosing subject claims.
    ListOidc {
        #[arg(long)]
        username: String,
    },
    /// Recover lost MFA without removing an administrator's enrollment requirement.
    ResetMfa {
        #[arg(long)]
        username: String,
    },
    /// Rotate the encryption key and rewrap enrolled and pending MFA secrets.
    RotateMfaKey,
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
    /// Select the account's documented capability role.
    SetRole {
        #[arg(long)]
        username: String,
        #[arg(long, value_enum)]
        role: RoleArg,
    },
    /// Grant a scoped account access to one stable source ID.
    GrantSource {
        #[arg(long)]
        username: String,
        #[arg(long)]
        source_id: String,
    },
    /// Remove one direct source grant; independent team grants remain effective.
    RevokeSource {
        #[arg(long)]
        username: String,
        #[arg(long)]
        source_id: String,
    },
    /// Explain an account's role, effective sources, and direct/team grant origins.
    Permissions {
        #[arg(long)]
        username: String,
    },
}

#[derive(Clone, Copy, ValueEnum)]
enum RoleArg {
    Admin,
    Viewer,
    Auditor,
    Operator,
}

impl From<RoleArg> for auth::AccountRole {
    fn from(value: RoleArg) -> Self {
        match value {
            RoleArg::Admin => Self::Admin,
            RoleArg::Viewer => Self::Viewer,
            RoleArg::Auditor => Self::Auditor,
            RoleArg::Operator => Self::Operator,
        }
    }
}

#[derive(Subcommand)]
enum TeamCommand {
    Create {
        #[arg(long)]
        name: String,
    },
    Rename {
        #[arg(long)]
        team_id: String,
        #[arg(long)]
        name: String,
    },
    Delete {
        #[arg(long)]
        team_id: String,
    },
    AddMember {
        #[arg(long)]
        team_id: String,
        #[arg(long)]
        username: String,
    },
    RemoveMember {
        #[arg(long)]
        team_id: String,
        #[arg(long)]
        username: String,
    },
    GrantSource {
        #[arg(long)]
        team_id: String,
        #[arg(long)]
        source_id: String,
    },
    RevokeSource {
        #[arg(long)]
        team_id: String,
        #[arg(long)]
        source_id: String,
    },
    Inspect {
        #[arg(long)]
        team_id: String,
    },
    List {
        #[arg(long,default_value="1",value_parser=clap::value_parser!(u32).range(1..=100_000))]
        page: u32,
    },
}

async fn open_state(
    path: &std::path::Path,
    key: Option<&std::path::Path>,
) -> Result<auth::StateStore> {
    match key {
        Some(key) => auth::StateStore::open_with_mfa_key(path, Some(key)).await,
        None => auth::StateStore::open(path).await,
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();
    let cli = Cli::parse();
    match cli.command {
        #[cfg(windows)]
        Command::Personal {
            port,
            index_only,
            no_browser,
        } => personal::run(port, index_only, no_browser, cli.redaction_file.as_deref()).await?,
        Command::Rebuild {
            data,
            codex_home,
            gemini_home,
            hermes_home,
        } => {
            let homes = [
                (sources::Adapter::Codex, codex_home),
                (sources::Adapter::Gemini, gemini_home),
                (sources::Adapter::Hermes, hermes_home),
            ];
            let homes: Vec<_> = homes
                .into_iter()
                .filter_map(|(adapter, path)| path.map(|path| (adapter, path)))
                .collect();
            let data = data
                .or_else(|| std::env::var_os("AGENT_LENS_DATA").map(PathBuf::from))
                .unwrap_or_else(|| PathBuf::from("data"));
            let input = if homes.is_empty() {
                ingest::SourceInput::Tree(&data)
            } else {
                ingest::SourceInput::Personal(&homes)
            };
            rebuild_index(&cli.database, input, cli.redaction_file.as_deref()).await?;
        }
        Command::Refresh { data } => {
            let policy = redaction::Policy::load(cli.redaction_file.as_deref())?;
            let pool = db::open(&cli.database).await?;
            let report = ingest::ingest_with_policy(&pool, &data, &policy).await?;
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
            let state = if matches!(command, AccountCommand::ResetMfa { .. }) {
                auth::StateStore::open_for_mfa_recovery(
                    &cli.state_database,
                    cli.mfa_key_file.as_deref(),
                )
                .await?
            } else {
                open_state(&cli.state_database, cli.mfa_key_file.as_deref()).await?
            };
            match command {
                AccountCommand::LinkOidc {
                    username,
                    config,
                    subject_file,
                } => {
                    let id = oidc::links::link_file(&state.auth, &username, &config, &subject_file)
                        .await?;
                    println!(
                        "OIDC link: {id}; credentials revoked; federated sign-in remains unavailable"
                    );
                }
                AccountCommand::UnlinkOidc { username, link_id } => {
                    oidc::links::unlink(&state.auth, &username, &link_id).await?;
                    println!(
                        "OIDC identity link removed; credentials revoked; local password and MFA preserved"
                    );
                }
                AccountCommand::ListOidc { username } => {
                    for link in oidc::links::list(&state.auth, &username).await? {
                        println!(
                            "link: {}\nissuer: {}\ncreated: {}",
                            link.id, link.issuer, link.created_at
                        );
                    }
                }
                AccountCommand::ResetMfa { username } => {
                    mfa::cli_reset(&state.auth, &username).await?;
                    println!("MFA reset; credentials revoked; enrollment requirement preserved");
                }
                AccountCommand::RotateMfaKey => {
                    mfa::rotate_key(&state.auth).await?;
                    println!("MFA encryption key rotated; existing secrets rewrapped");
                }
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
                    for origin in permissions.origins {
                        println!(
                            "grant: {} via {}{}",
                            origin.source_key,
                            origin.origin,
                            origin
                                .team_id
                                .map(|id| format!(" ({id})"))
                                .unwrap_or_default()
                        );
                    }
                }
            }
        }
        Command::Team { command } => {
            let state = open_state(&cli.state_database, cli.mfa_key_file.as_deref()).await?;
            match command {
                TeamCommand::Create { name } => {
                    println!("team: {}", access::create(&state, &name).await?)
                }
                TeamCommand::Rename { team_id, name } => {
                    access::rename(&state, &team_id, &name).await?;
                    println!("team renamed");
                }
                TeamCommand::Delete { team_id } => {
                    access::delete(&state, &team_id).await?;
                    println!("team deleted; memberships and team source grants removed");
                }
                TeamCommand::AddMember { team_id, username } => {
                    access::membership(&state, &team_id, &username, true).await?;
                    println!("team membership added");
                }
                TeamCommand::RemoveMember { team_id, username } => {
                    access::membership(&state, &team_id, &username, false).await?;
                    println!("team membership removed");
                }
                TeamCommand::GrantSource { team_id, source_id } => {
                    access::grant(&state, &team_id, &source_id, true).await?;
                    println!("team source access granted");
                }
                TeamCommand::RevokeSource { team_id, source_id } => {
                    access::grant(&state, &team_id, &source_id, false).await?;
                    println!("team source access revoked");
                }
                TeamCommand::Inspect { team_id } => {
                    let (team, members, sources) = access::inspect(&state, &team_id).await?;
                    println!("team: {}\nname: {}", team.id, team.name);
                    for member in members {
                        println!("member: {member}");
                    }
                    for source in sources {
                        println!("source: {source}");
                    }
                }
                TeamCommand::List { page } => {
                    let mut teams =
                        access::list(&state.auth.pool, i64::from(page - 1) * 50, 51).await?;
                    let next = teams.len() > 50;
                    teams.truncate(50);
                    for team in teams {
                        println!("team: {}\t{}", team.id, team.name);
                    }
                    if next {
                        println!("more teams available on page {}", page + 1);
                    }
                }
            }
        }
        Command::OidcCheck {
            config,
            discover,
            prepare,
            deployment,
            public_url,
            secure_cookies,
        } => {
            if deployment {
                oidc::prepare_deployment_file(
                    &config,
                    public_url.as_deref(),
                    secure_cookies,
                    &cli.state_database,
                )
                .await?;
                println!(
                    "OIDC declared deployment prerequisites and protocol preparation successful; sign-in remains unavailable pending browser/session/logout integration"
                );
                return Ok(());
            }
            if prepare {
                oidc::prepare_file(&config).await?;
                println!(
                    "OIDC protocol preparation successful; sign-in remains unavailable pending browser/session/logout integration"
                );
                return Ok(());
            }
            if discover {
                oidc::discover_file(&config).await?;
                println!(
                    "OIDC discovery metadata compatible; sign-in remains unavailable pending browser/session/logout integration"
                );
                return Ok(());
            }
            oidc::check_file(&config)?;
            println!(
                "OIDC configuration valid; sign-in remains unavailable pending browser/session/logout integration"
            );
        }
        Command::Serve {
            bind,
            local,
            projects_file,
            secure_cookies,
            public_url,
        } => {
            let projects = projects::ProjectConfig::load(projects_file.as_deref())?;
            if local {
                web::validate_local_bind(&bind)?;
            }
            let mut state = open_state(&cli.state_database, cli.mfa_key_file.as_deref()).await?;
            if public_url.is_some() {
                anyhow::ensure!(
                    secure_cookies && !local,
                    "passkeys require authenticated mode and Secure cookies"
                );
            }
            state.auth.passkeys = passkeys::Runtime::configured(public_url.as_deref())?;
            if local {
                state.enable_local_mode();
            } else {
                state.require_active_account().await?;
            }
            let pool = db::open(&cli.database).await?;
            let policy = redaction::Policy::load(cli.redaction_file.as_deref())?;
            policy.validate_index(&pool).await?;
            web::serve(pool, &bind, state, projects, secure_cookies).await?;
        }
    }
    Ok(())
}

async fn rebuild_index(
    database: &std::path::Path,
    input: ingest::SourceInput<'_>,
    redaction_file: Option<&std::path::Path>,
) -> Result<()> {
    let policy = redaction::Policy::load(redaction_file)?;
    let rebuild = db::begin_rebuild(database).await?;
    let report = match ingest::ingest_sources(rebuild.pool(), input, &policy).await {
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
    Ok(())
}
