use crate::{
    fingerprint::hash_file,
    sources::{self, Adapter, NormalizedRecord, SourceLayout, codex, gemini, hermes},
};
use anyhow::{Context, Result, ensure};
use sqlx::{Acquire, SqliteConnection, SqlitePool};
use std::{collections::HashSet, fs::Metadata, path::Path};
use walkdir::WalkDir;

#[cfg(test)]
mod regression;

#[derive(sqlx::FromRow)]
struct SourceCheckpoint {
    content_hash: Option<String>,
    parser_version: Option<i64>,
    parse_status: String,
    has_session: bool,
}

#[derive(Default, Debug)]
pub struct IngestReport {
    pub sessions: usize,
    pub skipped: usize,
    pub files_seen: usize,
    pub removed_sessions: u64,
    pub removed_files: u64,
    pub messages: usize,
    pub tool_calls: usize,
    pub errors: usize,
    pub partial_files: usize,
    pub malformed_lines: usize,
}

struct ImportedRollout {
    sessions: usize,
    messages: usize,
    tool_calls: usize,
    malformed_lines: usize,
}

pub async fn ingest_data_root(pool: &SqlitePool, root: &Path) -> Result<IngestReport> {
    let run =
        sqlx::query("INSERT INTO ingest_runs(status,source_layout) VALUES('running','unknown')")
            .execute(pool)
            .await
            .context("start ingest health record")?
            .last_insert_rowid();
    match ingest_data_root_run(pool, root, run).await {
        Ok(report) => Ok(report),
        Err(error) => {
            let reason = classify_run_failure(&format!("{error:#}"));
            if let Err(record_error) = sqlx::query("UPDATE ingest_runs SET finished_at=CURRENT_TIMESTAMP,status='failed',errors=1,failure_reason=? WHERE id=?")
                .bind(reason)
                .bind(run)
                .execute(pool)
                .await
            {
                // Avoid leaving a misleading running row if even the sanitised
                // failure finalization cannot be committed.
                let _ = sqlx::query("DELETE FROM ingest_runs WHERE id=?")
                    .bind(run)
                    .execute(pool)
                    .await;
                return Err(error.context(format!(
                    "record sanitised ingest failure: {record_error:#}"
                )));
            }
            Err(error)
        }
    }
}

async fn ingest_data_root_run(pool: &SqlitePool, root: &Path, run: i64) -> Result<IngestReport> {
    let discovered = sources::discover(root)?;
    let mut transaction = pool.begin().await?;
    let previous_layout: Option<String> = sqlx::query_scalar(
        "SELECT source_layout FROM ingest_runs WHERE status='complete' ORDER BY id DESC LIMIT 1",
    )
    .fetch_optional(&mut *transaction)
    .await
    .context("load previous ingest source layout")?;
    ensure!(
        previous_layout
            .as_deref()
            .is_none_or(|previous| previous == discovered.database_layout),
        "source layout does not match the previous rebuild or refresh; run rebuild to change between a direct agent home and host/user data"
    );
    sqlx::query("UPDATE ingest_runs SET source_layout=? WHERE id=?")
        .bind(discovered.database_layout)
        .bind(run)
        .execute(&mut *transaction)
        .await?;
    let mut report = IngestReport::default();
    for home in discovered.homes {
        let adapter = home.adapter;
        let (instance, source_key) = ensure_instance(
            &mut transaction,
            adapter.kind(),
            adapter.display_name(),
            &home.host,
            &home.user,
            &home.path,
        )
        .await?;
        let mut roots: Vec<(std::path::PathBuf, &'static str)> = Vec::new();
        match adapter {
            Adapter::Codex => {
                for area in codex::SOURCE_AREAS {
                    let path = home.path.join(area);
                    if codex::optional_directory(&path)? {
                        roots.push((path, area));
                    }
                }
            }
            Adapter::Gemini => roots.extend(
                gemini::chat_roots(&home.path)?
                    .into_iter()
                    .map(|path| (path, "chats")),
            ),
            Adapter::Hermes => {
                for path in hermes::state_databases(&home.path)? {
                    report.files_seen += 1;
                    let mut file_transaction = transaction.begin().await?;
                    match ingest_hermes_state(
                        &mut file_transaction,
                        run,
                        instance,
                        &source_key,
                        &home.path,
                        &path,
                    )
                    .await
                    {
                        Ok(None) => {
                            file_transaction.commit().await?;
                            report.skipped += 1;
                        }
                        Ok(Some(imported)) => {
                            file_transaction.commit().await?;
                            report.sessions += imported.sessions;
                            report.messages += imported.messages;
                            report.tool_calls += imported.tool_calls;
                        }
                        Err(error) => {
                            file_transaction.rollback().await?;
                            report.errors += 1;
                            record_file_error(
                                &mut transaction,
                                run,
                                instance,
                                &path,
                                "state",
                                &format!("{error:#}"),
                            )
                            .await?;
                        }
                    }
                }
                if let Some(path) = hermes::session_root(&home.path)? {
                    roots.push((path, "sessions"));
                }
                if let Some(path) = hermes::cron_output_root(&home.path)? {
                    roots.push((path, "cron"));
                }
            }
        }
        for (dir, area) in roots {
            for entry in WalkDir::new(&dir)
                .follow_links(true)
                .sort_by_file_name()
                .into_iter()
            {
                let entry = entry.map_err(|error| {
                    anyhow::anyhow!(
                        "source traversal failed: {}",
                        error
                            .io_error()
                            .map_or(std::io::ErrorKind::Other, std::io::Error::kind)
                    )
                })?;
                ensure!(
                    !entry.path_is_symlink() || entry.file_type().is_dir(),
                    "source traversal encountered an unsupported file symbolic link"
                );
                if entry.file_type().is_file()
                    && match adapter {
                        Adapter::Codex => entry.path().extension().is_some_and(|x| x == "jsonl"),
                        Adapter::Gemini => gemini::is_session_file(entry.path()),
                        Adapter::Hermes => {
                            (area == "sessions" && hermes::is_session_file(entry.path()))
                                || (area == "cron" && hermes::is_cron_output(entry.path()))
                        }
                    }
                {
                    report.files_seen += 1;
                    let mut file_transaction = transaction.begin().await?;
                    match ingest_source(
                        &mut file_transaction,
                        adapter,
                        run,
                        instance,
                        &source_key,
                        entry.path(),
                        area,
                    )
                    .await
                    {
                        Ok(None) => {
                            file_transaction.commit().await?;
                            report.skipped += 1;
                        }
                        Ok(Some(imported)) => {
                            file_transaction.commit().await?;
                            report.sessions += imported.sessions;
                            report.messages += imported.messages;
                            report.tool_calls += imported.tool_calls;
                            if imported.malformed_lines != 0 {
                                report.partial_files += 1;
                                report.malformed_lines += imported.malformed_lines;
                            }
                        }
                        Err(error) => {
                            file_transaction.rollback().await?;
                            report.errors += 1;
                            record_file_error(
                                &mut transaction,
                                run,
                                instance,
                                entry.path(),
                                area,
                                &format!("{error:#}"),
                            )
                            .await?;
                        }
                    }
                }
            }
        }
        if adapter == Adapter::Codex {
            apply_codex_titles(&mut transaction, instance, &home.path).await?;
        }
    }
    (report.removed_sessions, report.removed_files) =
        prune_missing_sources(&mut transaction, root, discovered.layout).await?;
    sqlx::query("UPDATE ingest_runs SET finished_at=CURRENT_TIMESTAMP,status='complete',files_seen=?,errors=? WHERE id=?")
        .bind(report.files_seen as i64).bind(report.errors as i64).bind(run).execute(&mut *transaction).await?;
    transaction.commit().await?;
    Ok(report)
}

fn classify_run_failure(detail: &str) -> &'static str {
    let detail = detail.to_ascii_lowercase();
    if detail.contains("layout does not match") {
        "source_layout_mismatch"
    } else if detail.contains("traversal") || detail.contains("symbolic link") {
        "source_traversal"
    } else if detail.contains("discover")
        || detail.contains("source root")
        || detail.contains("codex home")
        || detail.contains("optional source")
        || detail.contains("inspect data")
    {
        "source_discovery"
    } else {
        "ingest_failed"
    }
}

async fn prune_missing_sources(
    connection: &mut SqliteConnection,
    root: &Path,
    layout: SourceLayout,
) -> Result<(u64, u64)> {
    // Recheck the root before deleting: an unavailable root is not an empty snapshot.
    ensure!(
        codex::optional_directory(root)?,
        "source root disappeared; refresh cancelled"
    );
    let mut cursor = 0i64;
    let mut sessions = 0;
    let mut files = 0;
    loop {
        // Bound path buffering even when a database contains many source roots.
        let rows: Vec<(i64, String, String, String)> = sqlx::query_as(
            "SELECT source_files.id,source_files.path,agent_instances.source_home,agents.kind FROM source_files JOIN agent_instances ON agent_instances.id=source_files.instance_id JOIN agents ON agents.id=agent_instances.agent_id WHERE source_files.id>? ORDER BY source_files.id LIMIT 256")
            .bind(cursor).fetch_all(&mut *connection).await.context("query source removal candidates")?;
        if rows.is_empty() {
            break;
        }
        for (id, path, home, agent_kind) in rows {
            cursor = id;
            let adapter = match agent_kind.as_str() {
                codex::KIND => Adapter::Codex,
                gemini::KIND => Adapter::Gemini,
                hermes::KIND => Adapter::Hermes,
                _ => continue,
            };
            let home = Path::new(&home);
            if !sources::home_belongs_to_root(adapter, home, root, layout) {
                continue;
            }
            ensure!(
                Path::new(&path).starts_with(home),
                "indexed source is outside its home; refresh cancelled"
            );
            let source_present = match std::fs::symlink_metadata(&path) {
                Ok(metadata) => {
                    ensure!(
                        !metadata.file_type().is_symlink(),
                        "indexed source became a symbolic link; refresh cancelled"
                    );
                    metadata.is_file()
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
                Err(error) => return Err(error).context("check indexed source for removal"),
            };
            if !source_present {
                sessions += sqlx::query("DELETE FROM sessions WHERE source_file_id=?")
                    .bind(id)
                    .execute(&mut *connection)
                    .await
                    .context("remove missing source sessions")?
                    .rows_affected();
                // Session children cascade; deleting the checkpoint also removes parse errors and paths.
                files += sqlx::query("DELETE FROM source_files WHERE id=?")
                    .bind(id)
                    .execute(&mut *connection)
                    .await
                    .context("remove missing source checkpoint")?
                    .rows_affected();
            }
        }
    }
    Ok((sessions, files))
}

async fn ensure_instance(
    connection: &mut SqliteConnection,
    agent_kind: &str,
    display_name: &str,
    host: &str,
    user: &str,
    home: &Path,
) -> Result<(i64, String)> {
    sqlx::query("INSERT OR IGNORE INTO agents(kind,display_name) VALUES(?,?)")
        .bind(agent_kind)
        .bind(display_name)
        .execute(&mut *connection)
        .await?;
    sqlx::query("INSERT OR IGNORE INTO hosts(name) VALUES(?)")
        .bind(host)
        .execute(&mut *connection)
        .await?;
    let host_id: i64 = sqlx::query_scalar("SELECT id FROM hosts WHERE name=?")
        .bind(host)
        .fetch_one(&mut *connection)
        .await?;
    sqlx::query("INSERT OR IGNORE INTO users(host_id,os_user) VALUES(?,?)")
        .bind(host_id)
        .bind(user)
        .execute(&mut *connection)
        .await?;
    let user_id: i64 = sqlx::query_scalar("SELECT id FROM users WHERE host_id=? AND os_user=?")
        .bind(host_id)
        .bind(user)
        .fetch_one(&mut *connection)
        .await?;
    let agent_id: i64 = sqlx::query_scalar("SELECT id FROM agents WHERE kind=?")
        .bind(agent_kind)
        .fetch_one(&mut *connection)
        .await?;
    let source = home.to_string_lossy();
    let source_key = crate::identity::source_id(agent_kind, host, user, source.as_ref());
    sqlx::query(
        "INSERT INTO agent_instances(agent_id,user_id,source_home,source_key) VALUES(?,?,?,?) ON CONFLICT(source_home) DO UPDATE SET source_key=excluded.source_key",
    )
    .bind(agent_id)
    .bind(user_id)
    .bind(source.as_ref())
    .bind(&source_key)
    .execute(&mut *connection)
    .await?;
    Ok((
        sqlx::query_scalar("SELECT id FROM agent_instances WHERE source_home=?")
            .bind(source.as_ref())
            .fetch_one(&mut *connection)
            .await?,
        source_key,
    ))
}

async fn ingest_source(
    connection: &mut SqliteConnection,
    adapter: Adapter,
    run: i64,
    instance: i64,
    source_key: &str,
    path: &Path,
    kind: &str,
) -> Result<Option<ImportedRollout>> {
    let previous: Option<SourceCheckpoint> = sqlx::query_as(
        "SELECT content_hash,parser_version,parse_status,EXISTS(SELECT 1 FROM sessions WHERE source_file_id=source_files.id) AS has_session FROM source_files WHERE path=?")
        .bind(path.to_string_lossy().as_ref()).fetch_optional(&mut *connection).await?;
    if let Some(checkpoint) = &previous
        && checkpoint.has_session
        && checkpoint.parser_version == Some(adapter.parser_version())
        && checkpoint.parse_status == "ok"
        && let Some(hash) = &checkpoint.content_hash
        && hash_file(path)? == *hash
    {
        sqlx::query("UPDATE source_files SET observed_at=CURRENT_TIMESTAMP WHERE path=?")
            .bind(path.to_string_lossy().as_ref())
            .execute(&mut *connection)
            .await
            .context("record unchanged source observation")?;
        return Ok(None);
    }
    let summary = match adapter {
        Adapter::Codex => codex::scan_rollout(path)?,
        Adapter::Gemini => gemini::scan_session(path)?,
        Adapter::Hermes => hermes::scan_session(path)?,
    };
    if previous.as_ref().is_some_and(|row| row.has_session) {
        ensure!(
            summary.malformed_lines == 0,
            "partial replacement; previous records retained; retry refresh"
        );
    }
    let existing: Option<(i64, String, String)> = sqlx::query_as("SELECT sessions.id,source_files.path,source_files.kind FROM sessions JOIN source_files ON source_files.id=sessions.source_file_id WHERE sessions.instance_id=? AND external_id=?")
        .bind(instance).bind(&summary.external_id).fetch_optional(&mut *connection).await?;
    if existing.is_some() {
        ensure!(
            summary.malformed_lines == 0,
            "partial replacement; previous records retained; retry refresh"
        );
    }
    if let Some((_, old_path, old_kind)) = &existing {
        ensure!(
            Path::new(old_path) == path
                || !Path::new(old_path)
                    .try_exists()
                    .context("inspect prior source location")?
                || source_precedes(adapter, kind, path, old_kind, Path::new(old_path)),
            "duplicate session identity at another source; previous records retained"
        );
    }
    let source_id: i64 = sqlx::query_scalar("INSERT INTO source_files(instance_id,ingest_run_id,path,kind,size_bytes,parse_status) VALUES(?,?,?,?,?,'parsing') ON CONFLICT(path) DO UPDATE SET ingest_run_id=excluded.ingest_run_id,kind=excluded.kind,size_bytes=excluded.size_bytes,parse_status='parsing' RETURNING id")
        .bind(instance).bind(run).bind(path.to_string_lossy().as_ref()).bind(kind).bind(summary.metadata.len() as i64).fetch_one(&mut *connection).await?;
    // A replaced path can now identify a different session. Remove only its old projection.
    sqlx::query("DELETE FROM sessions WHERE source_file_id=? AND external_id<>?")
        .bind(source_id)
        .bind(&summary.external_id)
        .execute(&mut *connection)
        .await?;
    let stable_id = crate::identity::session_id(source_key, &summary.external_id);
    let fallback_title = summary
        .derived_title
        .as_deref()
        .unwrap_or(&summary.external_id);
    let session_id: i64 = sqlx::query_scalar("INSERT INTO sessions(instance_id,source_file_id,external_id,stable_id,started_at,ended_at,cwd,model,status,title,derived_title,raw_metadata) VALUES(?,?,?,?,?,?,?,?,?,?,?,?) ON CONFLICT(instance_id,external_id) DO UPDATE SET source_file_id=excluded.source_file_id,stable_id=excluded.stable_id,started_at=excluded.started_at,ended_at=excluded.ended_at,cwd=excluded.cwd,model=excluded.model,status=excluded.status,title=excluded.title,derived_title=excluded.derived_title,raw_metadata=excluded.raw_metadata RETURNING id")
        .bind(instance).bind(source_id).bind(&summary.external_id).bind(stable_id).bind(summary.started_at.as_deref()).bind(summary.ended_at.as_deref()).bind(summary.cwd.as_deref()).bind(summary.model.as_deref()).bind(summary.status).bind(fallback_title).bind(summary.derived_title.as_deref()).bind(&summary.raw_metadata).fetch_one(&mut *connection).await?;
    if existing.is_some() {
        for table in [
            "messages",
            "tool_calls",
            "usage_events",
            "session_usage",
            "turn_events",
            "turns",
        ] {
            sqlx::query(&format!("DELETE FROM {table} WHERE session_id=?"))
                .bind(session_id)
                .execute(&mut *connection)
                .await?;
        }
    }
    let (messages, tools, second_pass_malformed) =
        import_source_records(connection, adapter, path, &summary.metadata, session_id).await?;
    ensure!(
        second_pass_malformed == summary.malformed_lines,
        "source changed between parsing passes; retry refresh"
    );
    let parse_status = if summary.malformed_lines == 0 {
        "ok"
    } else {
        "partial"
    };
    sqlx::query("UPDATE source_files SET parse_status=?,parse_error=?,content_hash=?,parser_version=?,observed_at=CURRENT_TIMESTAMP WHERE id=?")
        .bind(parse_status)
        .bind(summary.parse_error.as_deref())
        .bind(&summary.content_hash)
        .bind(adapter.parser_version())
        .bind(source_id)
        .execute(&mut *connection)
        .await?;
    Ok(Some(ImportedRollout {
        sessions: 1,
        messages,
        tool_calls: tools,
        malformed_lines: summary.malformed_lines,
    }))
}

async fn ingest_hermes_state(
    connection: &mut SqliteConnection,
    run: i64,
    instance: i64,
    source_key: &str,
    home: &Path,
    path: &Path,
) -> Result<Option<ImportedRollout>> {
    let previous: Option<SourceCheckpoint> = sqlx::query_as(
        "SELECT content_hash,parser_version,parse_status,EXISTS(SELECT 1 FROM sessions WHERE source_file_id=source_files.id) AS has_session FROM source_files WHERE path=?",
    )
    .bind(path.to_string_lossy().as_ref())
    .fetch_optional(&mut *connection)
    .await?;
    let mut reader = hermes::StateReader::open(path, home)
        .await
        .context("read Hermes state database")?;
    let scan = reader.scan().await?;
    if previous.as_ref().is_some_and(|checkpoint| {
        checkpoint.parser_version == Some(hermes::PARSER_VERSION)
            && checkpoint.parse_status == "ok"
            && checkpoint.content_hash.as_deref() == Some(scan.content_hash.as_str())
    }) {
        sqlx::query("UPDATE source_files SET observed_at=CURRENT_TIMESTAMP WHERE path=?")
            .bind(path.to_string_lossy().as_ref())
            .execute(&mut *connection)
            .await
            .context("record unchanged Hermes state observation")?;
        return Ok(None);
    }
    let source_id: i64 = sqlx::query_scalar(
        "INSERT INTO source_files(instance_id,ingest_run_id,path,kind,size_bytes,parse_status) VALUES(?,?,?,?,?,'parsing') ON CONFLICT(path) DO UPDATE SET ingest_run_id=excluded.ingest_run_id,kind=excluded.kind,size_bytes=excluded.size_bytes,parse_status='parsing' RETURNING id",
    )
    .bind(instance)
    .bind(run)
    .bind(path.to_string_lossy().as_ref())
    .bind("state")
    .bind(scan.metadata.len() as i64)
    .fetch_one(&mut *connection)
    .await?;

    let existing: Vec<(i64, String)> =
        sqlx::query_as("SELECT id,external_id FROM sessions WHERE source_file_id=?")
            .bind(source_id)
            .fetch_all(&mut *connection)
            .await?;
    let seen: HashSet<&str> = scan
        .sessions
        .iter()
        .map(|session| session.external_id.as_str())
        .collect();
    for (id, external_id) in existing {
        if !seen.contains(external_id.as_str()) {
            sqlx::query("DELETE FROM sessions WHERE id=?")
                .bind(id)
                .execute(&mut *connection)
                .await
                .context("remove deleted Hermes state session")?;
        }
    }

    let mut message_count = 0usize;
    let mut tool_count = 0usize;
    let session_count = scan.sessions.len();
    for session in scan.sessions {
        let stable_id = crate::identity::session_id(source_key, &session.external_id);
        let title = session.title.as_deref().unwrap_or(&session.external_id);
        let session_id: i64 = sqlx::query_scalar(
            "INSERT INTO sessions(instance_id,source_file_id,external_id,stable_id,started_at,ended_at,cwd,model,status,title,derived_title,raw_metadata) VALUES(?,?,?,?,?,?,?,?,?,?,?,?) ON CONFLICT(instance_id,external_id) DO UPDATE SET source_file_id=excluded.source_file_id,stable_id=excluded.stable_id,started_at=excluded.started_at,ended_at=excluded.ended_at,cwd=excluded.cwd,model=excluded.model,status=excluded.status,title=excluded.title,derived_title=excluded.derived_title,raw_metadata=excluded.raw_metadata RETURNING id",
        )
        .bind(instance)
        .bind(source_id)
        .bind(&session.external_id)
        .bind(stable_id)
        .bind(session.started_at.as_deref())
        .bind(session.ended_at.as_deref())
        .bind(session.cwd.as_deref())
        .bind(session.model.as_deref())
        .bind(session.status)
        .bind(title)
        .bind(session.title.as_deref())
        .bind(&session.raw_metadata)
        .fetch_one(&mut *connection)
        .await?;
        for table in [
            "messages",
            "tool_calls",
            "usage_events",
            "session_usage",
            "turn_events",
            "turns",
        ] {
            sqlx::query(&format!("DELETE FROM {table} WHERE session_id=?"))
                .bind(session_id)
                .execute(&mut *connection)
                .await?;
        }
        let records = reader.records(&session).await?;
        let mut records = SourceRecords::HermesState(records);
        let (messages, tools, _) = import_records(connection, session_id, &mut records).await?;
        message_count = message_count
            .checked_add(messages)
            .context("Hermes message count exceeds supported range")?;
        tool_count = tool_count
            .checked_add(tools)
            .context("Hermes tool count exceeds supported range")?;
    }
    sqlx::query("UPDATE source_files SET parse_status='ok',parse_error=NULL,content_hash=?,parser_version=?,observed_at=CURRENT_TIMESTAMP WHERE id=?")
        .bind(&scan.content_hash)
        .bind(hermes::PARSER_VERSION)
        .bind(source_id)
        .execute(&mut *connection)
        .await?;
    Ok(Some(ImportedRollout {
        sessions: session_count,
        messages: message_count,
        tool_calls: tool_count,
        malformed_lines: 0,
    }))
}

fn source_precedes(
    adapter: Adapter,
    kind: &str,
    path: &Path,
    old_kind: &str,
    old_path: &Path,
) -> bool {
    // SOURCE_AREAS defines active-before-archived precedence; sorted logical paths
    // make the winner reproducible without trusting filesystem enumeration order.
    let rank = |value: &str| adapter.source_rank(value);
    (rank(kind), path.as_os_str()) < (rank(old_kind), old_path.as_os_str())
}

async fn apply_codex_titles(
    connection: &mut SqliteConnection,
    instance: i64,
    home: &Path,
) -> Result<()> {
    sqlx::query(
        "UPDATE sessions SET title=coalesce(derived_title,external_id) WHERE instance_id=? AND title IS NOT coalesce(derived_title,external_id)",
    )
    .bind(instance)
    .execute(&mut *connection)
    .await
    .context("reset session titles to indexed fallbacks")?;
    let Some(mut advisory_titles) = codex::AdvisoryTitles::open(home) else {
        return Ok(());
    };
    while let Some((external_id, title)) = advisory_titles.next_title() {
        sqlx::query(
            "UPDATE sessions SET title=? WHERE instance_id=? AND external_id=? AND title IS NOT ?",
        )
        .bind(&title)
        .bind(instance)
        .bind(&external_id)
        .bind(&title)
        .execute(&mut *connection)
        .await
        .context("apply advisory session title")?;
    }
    Ok(())
}

enum SourceRecords<'a> {
    Codex(codex::RolloutRecords<'a>),
    Gemini(gemini::SessionRecords),
    Hermes(hermes::SessionRecords),
    HermesState(std::collections::VecDeque<(i64, NormalizedRecord)>),
}

impl SourceRecords<'_> {
    fn next_record(&mut self) -> Result<Option<(i64, NormalizedRecord)>> {
        match self {
            Self::Codex(records) => records.next_record(),
            Self::Gemini(records) => Ok(records.next_record()),
            Self::Hermes(records) => Ok(records.next_record()),
            Self::HermesState(records) => Ok(records.pop_front()),
        }
    }

    fn malformed_lines(&self) -> usize {
        match self {
            Self::Codex(records) => records.malformed_lines(),
            Self::Gemini(records) => records.malformed_lines(),
            Self::Hermes(_) | Self::HermesState(_) => 0,
        }
    }
}

async fn import_source_records(
    connection: &mut SqliteConnection,
    adapter: Adapter,
    path: &Path,
    metadata: &Metadata,
    session_id: i64,
) -> Result<(usize, usize, usize)> {
    let mut records = match adapter {
        Adapter::Codex => SourceRecords::Codex(codex::RolloutRecords::open(path, metadata)?),
        Adapter::Gemini => SourceRecords::Gemini(gemini::SessionRecords::open(path, metadata)?),
        Adapter::Hermes => SourceRecords::Hermes(hermes::SessionRecords::open(path, metadata)?),
    };
    import_records(connection, session_id, &mut records).await
}

async fn import_records(
    connection: &mut SqliteConnection,
    session_id: i64,
    records: &mut SourceRecords<'_>,
) -> Result<(usize, usize, usize)> {
    let mut messages = 0usize;
    let mut tools = 0usize;
    let mut usage_accounting = crate::usage::UsageAccounting::default();
    let mut turn_tracker = crate::turns::TurnTracker::default();
    while let Some((sequence, record)) = records.next_record()? {
        match record {
            NormalizedRecord::Message {
                timestamp,
                role,
                content,
                opens_turn,
                raw_json,
            } => {
                if opens_turn {
                    let update = turn_tracker.observe(
                        sequence,
                        timestamp.as_deref(),
                        crate::turns::TurnSignal {
                            kind: "user_activity",
                            turn_id: None,
                            requested_model: None,
                            served_model: None,
                        },
                    );
                    if update.opened {
                        record_turn_signal(
                            connection,
                            session_id,
                            sequence,
                            timestamp.as_deref(),
                            "user_activity",
                            None,
                            None,
                            None,
                            &raw_json,
                            update,
                        )
                        .await?;
                    }
                }
                sqlx::query("INSERT INTO messages(session_id,sequence,timestamp,role,content,raw_json) VALUES(?,?,?,?,?,?)")
                        .bind(session_id).bind(sequence).bind(timestamp).bind(role).bind(content).bind(raw_json).execute(&mut *connection).await?;
                messages += 1;
            }
            NormalizedRecord::ToolCall {
                timestamp,
                call_id,
                tool_name,
                arguments,
                kind,
                raw_json,
            } => {
                sqlx::query("INSERT INTO tool_calls(session_id,sequence,timestamp,call_id,tool_name,arguments,status,raw_json,kind,turn_ordinal) VALUES(?,?,?,?,?,?,'called',?,?,?)")
                        .bind(session_id).bind(sequence).bind(timestamp).bind(call_id).bind(tool_name).bind(arguments).bind(raw_json).bind(kind).bind(turn_tracker.active_ordinal()).execute(&mut *connection).await?;
                tools += 1;
            }
            NormalizedRecord::ToolOutput {
                timestamp,
                call_id,
                output,
                outcome,
                raw_json,
            } => {
                sqlx::query("INSERT INTO tool_results(session_id,source_sequence,timestamp,call_id,tool_call_id,output,outcome,raw_json,turn_ordinal) VALUES(?,?,?,?,(SELECT id FROM tool_calls WHERE session_id=? AND call_id=? AND sequence<? ORDER BY sequence DESC LIMIT 1),?,?,?,COALESCE((SELECT turn_ordinal FROM tool_calls WHERE session_id=? AND call_id=? AND sequence<? ORDER BY sequence DESC LIMIT 1),?))")
                    .bind(session_id)
                    .bind(sequence)
                    .bind(timestamp)
                    .bind(&call_id)
                    .bind(session_id)
                    .bind(call_id.as_deref().filter(|call_id| !call_id.is_empty()))
                    .bind(sequence)
                    .bind(output)
                    .bind(outcome)
                    .bind(raw_json)
                    .bind(session_id)
                    .bind(call_id.as_deref().filter(|call_id| !call_id.is_empty()))
                    .bind(sequence)
                    .bind(turn_tracker.active_ordinal())
                    .execute(&mut *connection)
                    .await?;
            }
            NormalizedRecord::Usage {
                timestamp,
                cumulative,
                per_turn,
                pressure,
                raw_json,
            } => {
                let processed = usage_accounting.observe(cumulative.as_ref(), per_turn.as_ref());
                let cumulative = cumulative.unwrap_or_default();
                let per_turn = per_turn.unwrap_or_default();
                sqlx::query(
                    "INSERT INTO usage_events( \
                     session_id,source_sequence,sequence_known,timestamp, \
                     input_tokens,output_tokens,cached_input_tokens,reasoning_output_tokens,total_tokens, \
                     per_turn_input_tokens,per_turn_output_tokens,per_turn_cached_input_tokens,per_turn_reasoning_output_tokens,per_turn_total_tokens, \
                     delta_input_tokens,delta_output_tokens,delta_cached_input_tokens,delta_reasoning_output_tokens,delta_total_tokens, \
                     cumulative_present,per_turn_present,is_duplicate,has_reset,has_missing,has_inconsistent, \
                     context_window,context_tokens,rate_limit_id,rate_limit_name,primary_used_percent,primary_window_minutes,primary_resets_at,primary_resets_in_seconds,secondary_used_percent,secondary_window_minutes,secondary_resets_at,secondary_resets_in_seconds,plan_type,rate_limit_reached_type,raw_json \
                     ) VALUES(?1,?2,1,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19,?20,?21,?22,?23,?24,?25,?26,?27,?28,?29,?30,?31,?32,?33,?34,?35,?36,?37,?38,?39)",
                )
                .bind(session_id)
                .bind(sequence)
                .bind(timestamp)
                .bind(cumulative.input_tokens)
                .bind(cumulative.output_tokens)
                .bind(cumulative.cached_input_tokens)
                .bind(cumulative.reasoning_output_tokens)
                .bind(cumulative.total_tokens)
                .bind(per_turn.input_tokens)
                .bind(per_turn.output_tokens)
                .bind(per_turn.cached_input_tokens)
                .bind(per_turn.reasoning_output_tokens)
                .bind(per_turn.total_tokens)
                .bind(processed.delta.input_tokens)
                .bind(processed.delta.output_tokens)
                .bind(processed.delta.cached_input_tokens)
                .bind(processed.delta.reasoning_output_tokens)
                .bind(processed.delta.total_tokens)
                .bind(processed.cumulative_present)
                .bind(processed.per_turn_present)
                .bind(processed.duplicate)
                .bind(processed.reset)
                .bind(processed.missing)
                .bind(processed.inconsistent)
                .bind(pressure.context_window)
                .bind(pressure.context_tokens)
                .bind(pressure.rate_limit_id)
                .bind(pressure.rate_limit_name)
                .bind(pressure.primary.used_percent)
                .bind(pressure.primary.window_minutes)
                .bind(pressure.primary.resets_at)
                .bind(pressure.primary.resets_in_seconds)
                .bind(pressure.secondary.used_percent)
                .bind(pressure.secondary.window_minutes)
                .bind(pressure.secondary.resets_at)
                .bind(pressure.secondary.resets_in_seconds)
                .bind(pressure.plan_type)
                .bind(pressure.rate_limit_reached_type)
                .bind(raw_json)
                .execute(&mut *connection)
                .await?;
            }
            NormalizedRecord::TurnEvent {
                timestamp,
                kind,
                turn_id,
                requested_model,
                served_model,
                raw_json,
            } => {
                let update = turn_tracker.observe(
                    sequence,
                    timestamp.as_deref(),
                    crate::turns::TurnSignal {
                        kind,
                        turn_id: turn_id.as_deref(),
                        requested_model: requested_model.as_deref(),
                        served_model: served_model.as_deref(),
                    },
                );
                record_turn_signal(
                    connection,
                    session_id,
                    sequence,
                    timestamp.as_deref(),
                    kind,
                    turn_id.as_deref(),
                    requested_model.as_deref(),
                    served_model.as_deref(),
                    &raw_json,
                    update,
                )
                .await?;
            }
        }
    }
    if let Some(turn) = turn_tracker.finish() {
        insert_turn(connection, session_id, turn).await?;
    }
    if usage_accounting.snapshot_count != 0 {
        let summary = usage_accounting.summary();
        sqlx::query(
            "INSERT INTO session_usage(session_id,input_tokens,output_tokens,cached_input_tokens,reasoning_output_tokens,total_tokens,snapshot_count,per_turn_observation_count,reset_count,duplicate_count,missing_count,inconsistent_count) VALUES(?,?,?,?,?,?,?,?,?,?,?,?)",
        )
        .bind(session_id)
        .bind(summary.usage.input_tokens)
        .bind(summary.usage.output_tokens)
        .bind(summary.usage.cached_input_tokens)
        .bind(summary.usage.reasoning_output_tokens)
        .bind(summary.usage.total_tokens)
        .bind(summary.snapshot_count)
        .bind(summary.per_turn_observation_count)
        .bind(summary.reset_count)
        .bind(summary.duplicate_count)
        .bind(summary.missing_count)
        .bind(summary.inconsistent_count)
        .execute(&mut *connection)
        .await?;
    }
    Ok((messages, tools, records.malformed_lines()))
}

#[allow(clippy::too_many_arguments)]
async fn record_turn_signal(
    connection: &mut SqliteConnection,
    session_id: i64,
    sequence: i64,
    timestamp: Option<&str>,
    kind: &str,
    turn_id: Option<&str>,
    requested_model: Option<&str>,
    served_model: Option<&str>,
    raw_json: &str,
    update: crate::turns::TurnUpdate,
) -> Result<()> {
    if let Some(turn) = update.finalized {
        insert_turn(connection, session_id, turn).await?;
    }
    sqlx::query("INSERT INTO turn_events(session_id,turn_ordinal,source_sequence,timestamp,kind,turn_id,requested_model,served_model,is_resumption,is_model_change,raw_json) VALUES(?,?,?,?,?,?,?,?,?,?,?)")
        .bind(session_id)
        .bind(update.ordinal)
        .bind(sequence)
        .bind(timestamp)
        .bind(kind)
        .bind(turn_id)
        .bind(requested_model)
        .bind(served_model)
        .bind(update.is_resumption)
        .bind(update.is_model_change)
        .bind(raw_json)
        .execute(&mut *connection)
        .await?;
    Ok(())
}

async fn insert_turn(
    connection: &mut SqliteConnection,
    session_id: i64,
    turn: crate::turns::FinalizedTurn,
) -> Result<()> {
    sqlx::query("INSERT INTO turns(session_id,ordinal,turn_id,start_sequence,start_timestamp,start_kind,end_sequence,end_timestamp,outcome,resumed,requested_model,requested_model_sequence,observed_model,observed_model_sequence) VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?,?)")
        .bind(session_id)
        .bind(turn.ordinal)
        .bind(turn.turn_id)
        .bind(turn.start_sequence)
        .bind(turn.start_timestamp)
        .bind(turn.start_kind)
        .bind(turn.end_sequence)
        .bind(turn.end_timestamp)
        .bind(turn.outcome)
        .bind(turn.resumed)
        .bind(turn.requested_model)
        .bind(turn.requested_model_sequence)
        .bind(turn.observed_model)
        .bind(turn.observed_model_sequence)
        .execute(&mut *connection)
        .await?;
    Ok(())
}

async fn record_file_error(
    connection: &mut SqliteConnection,
    run: i64,
    instance: i64,
    path: &Path,
    kind: &str,
    error: &str,
) -> Result<()> {
    sqlx::query("INSERT INTO source_files(instance_id,ingest_run_id,path,kind,size_bytes,observed_at,parse_status,parse_error) VALUES(?,?,?,?,?,CURRENT_TIMESTAMP,'error',?) ON CONFLICT(path) DO UPDATE SET ingest_run_id=excluded.ingest_run_id,observed_at=CURRENT_TIMESTAMP,parse_status='error',parse_error=excluded.parse_error")
        .bind(instance).bind(run).bind(path.to_string_lossy().as_ref()).bind(kind).bind(std::fs::metadata(path).ok().map(|m| m.len() as i64)).bind(error).execute(&mut *connection).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn indexes_redacted_rollout_fixture() {
        let temp = tempfile::tempdir().unwrap();
        let sessions = temp
            .path()
            .join("fixture-host/fixture-user/.codex/sessions/2026/01/02");
        std::fs::create_dir_all(&sessions).unwrap();
        std::fs::write(
            sessions.join("rollout.jsonl"),
            include_str!("../tests/fixtures/codex/rollout.jsonl"),
        )
        .unwrap();
        let pool = crate::db::open(&temp.path().join("index.sqlite"))
            .await
            .unwrap();
        let report = ingest_data_root(&pool, temp.path()).await.unwrap();
        assert_eq!((report.sessions, report.messages, report.errors), (1, 2, 0));
        let row: (String, String, i64) = sqlx::query_as(
            "SELECT external_id, model, (SELECT count(*) FROM messages) FROM sessions",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(
            row,
            (
                "00000000-0000-0000-0000-000000000001".into(),
                "test-model".into(),
                2
            )
        );
    }
}
