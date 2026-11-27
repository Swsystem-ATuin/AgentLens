//! Codex desktop SQLite projections. Rollouts remain authoritative for transcript events.
mod snapshot;

use super::sanitized_title;
use crate::sources::NormalizedRecord;
use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};
use sqlx::{Connection, Row, SqliteConnection, sqlite::SqliteConnectOptions};
use std::{
    collections::{BTreeMap, HashSet, VecDeque},
    fs,
    io::ErrorKind,
    path::{Path, PathBuf},
};

pub(crate) struct Thread {
    pub(crate) path: PathBuf,
    pub(crate) title: Option<String>,
    pub(crate) originator: Option<String>,
    pub(crate) source: Option<String>,
    pub(crate) thread_source: Option<String>,
    pub(crate) created: Option<String>,
    pub(crate) updated: Option<String>,
    pub(crate) archived: Option<bool>,
    pub(crate) model: Option<String>,
}

pub(crate) struct History {
    catalogs: Vec<Catalog>,
    pub(crate) threads: BTreeMap<String, Thread>,
    pub(crate) diagnostics: Vec<(PathBuf, &'static str)>,
    // Drop the SQLite connections before attempting to remove the copies.
    _scratch: tempfile::TempDir,
}

struct Catalog {
    connection: SqliteConnection,
    path: PathBuf,
    items: Option<String>,
    turns: Option<String>,
}

struct Item {
    updated: i64,
    order: i64,
    phase: u8,
    id: String,
    record: NormalizedRecord,
}

fn candidate_paths(home: &Path) -> Result<Vec<PathBuf>> {
    let mut paths = Vec::new();
    for entry in home.read_dir()? {
        let entry = entry?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        let version = ["state_", "thread_history_"]
            .iter()
            .find_map(|prefix| name.strip_prefix(prefix))
            .and_then(|name| name.strip_suffix(".sqlite"))
            .and_then(|version| version.parse::<u64>().ok());
        if let Some(version) = version {
            paths.push((version, entry.path()));
        }
    }
    paths.sort_unstable_by(|left, right| right.cmp(left));
    let mut paths: Vec<_> = paths.into_iter().map(|(_, path)| path).collect();
    let optional = home.join("sqlite");
    // Do not traverse a reparse point into a different store.
    if ordinary(&optional, true)? {
        for name in ["codex.db", "codex-dev.db"] {
            let path = optional.join(name);
            if ordinary(&path, false)? {
                paths.push(path);
            }
        }
    }
    Ok(paths)
}

fn ordinary(path: &Path, directory: bool) -> Result<bool> {
    use std::os::windows::fs::MetadataExt;
    match path.symlink_metadata() {
        Ok(metadata) => Ok(metadata.file_attributes() & 0x400 == 0
            && if directory { metadata.is_dir() } else { metadata.is_file() }),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error).context("inspect Codex SQLite location"),
    }
}

async fn columns(connection: &mut SqliteConnection, table: &str) -> Result<HashSet<String>> {
    let names: Vec<String> = sqlx::query_scalar("SELECT lower(name) FROM pragma_table_info(?)")
        .bind(table)
        .fetch_all(connection)
        .await?;
    Ok(names.into_iter().collect())
}

// All proposed column and table identifiers are internal constants; only
// identifiers present in the selected schema are used.
fn col(columns: &HashSet<String>, choices: &[&str]) -> String {
    choices.iter().find(|name| columns.contains(**name)).map_or_else(
        || "NULL".into(),
        |name| format!("CAST([{name}] AS TEXT)"),
    )
}

impl History {
    pub(crate) async fn open(homes: &[PathBuf]) -> Result<Self> {
        let scratch = tempfile::tempdir().context("create private Codex SQLite snapshot directory")?;
        let mut history = Self {
            catalogs: Vec::new(),
            threads: BTreeMap::new(),
            diagnostics: Vec::new(),
            _scratch: scratch,
        };
        let mut candidates = Vec::new();
        for home in homes {
            match candidate_paths(home) {
                Ok(paths) => candidates.extend(paths),
                Err(_) => history.diagnostics.push((home.clone(), "access failed")),
            }
        }
        for (index, path) in candidates.into_iter().enumerate() {
            match history.add(&path, index).await {
                Ok(true) => history.diagnostics.push((path, "history available")),
                Ok(false) => history.diagnostics.push((path, "storage detected but unsupported schema")),
                Err(_) => history.diagnostics.push((path, "access failed or unreadable SQLite snapshot")),
            }
        }
        Ok(history)
    }

    async fn add(&mut self, path: &Path, index: usize) -> Result<bool> {
        ensure!(ordinary(path, false)?, "not a regular Codex SQLite history file");
        let target = self._scratch.path().join(format!("{index}.sqlite"));
        let source = path.to_owned();
        let work_target = target.clone();
        tokio::task::spawn_blocking(move || snapshot::make(&source, &work_target)).await??;
        let mut connection =
            SqliteConnection::connect_with(&SqliteConnectOptions::new().filename(target)).await?;
        let threads = columns(&mut connection, "threads").await?;
        let items = columns(&mut connection, "thread_items").await?;
        let turns = columns(&mut connection, "thread_turns").await?;
        let has_threads = threads.contains("id") || threads.contains("thread_id");
        let has_items = ["thread_id", "item_id", "item_json"]
            .iter()
            .all(|&name| items.contains(name));
        if has_threads {
            let fields = [
                &["id", "thread_id"][..],
                &["title", "name"], &["originator"], &["source"], &["thread_source"],
                &["created_at_ms", "created_at"], &["updated_at_ms", "updated_at"],
                &["archived", "archived_at"], &["model"],
            ];
            let projection = fields.map(|choices| col(&threads, choices)).join(",");
            let rows = sqlx::query(&format!("SELECT {projection} FROM threads"))
                .fetch_all(&mut connection).await?;
            for row in rows {
                if let Some(id) = row.try_get::<Option<String>, _>(0)?
                    && !id.is_empty()
                {
                    self.threads.entry(id).or_insert(Thread {
                        path: path.to_owned(),
                        title: row.try_get::<Option<String>, _>(1)?
                            .and_then(|title| sanitized_title(&title, 160)),
                        originator: label(row.try_get(2)?),
                        source: label(row.try_get(3)?),
                        thread_source: label(row.try_get(4)?),
                        created: time(row.try_get(5)?),
                        updated: time(row.try_get(6)?),
                        archived: row.try_get::<Option<String>, _>(7)?.map(|value| value != "0" && value != "false"),
                        model: row.try_get(8)?,
                    });
                }
            }
        }
        let items_query = if has_items {
            let rows = sqlx::query_scalar::<_, String>("SELECT DISTINCT CAST(thread_id AS TEXT) FROM thread_items WHERE thread_id IS NOT NULL")
                .fetch_all(&mut connection).await?;
            for id in rows.into_iter().filter(|id| !id.is_empty()) {
                self.threads.entry(id).or_insert_with(|| Thread {
                    path: path.to_owned(), title: None, originator: None, source: None,
                    thread_source: None, created: None, updated: None, archived: None, model: None,
                });
            }
            let fields = [
                &["item_id"][..], &["turn_id"], &["item_json"], &["item_type"],
                &["rollout_ordinal", "ordinal"], &["updated_at_ordinal", "rollout_ordinal", "ordinal"],
                &["created_at_ms", "started_at_ms", "created_at"],
            ];
            let projection = fields.map(|choices| col(&items, choices)).join(",");
            Some(format!("SELECT {projection} FROM thread_items WHERE thread_id=?"))
        } else {
            None
        };
        let turns_query = (turns.contains("thread_id") && turns.contains("turn_id")).then(|| {
            let fields = [
                &["turn_id"][..], &["rollout_ordinal", "ordinal"], &["rollout_end_ordinal"],
                &["status"], &["started_at", "started_at_ms"],
                &["completed_at", "completed_at_ms"],
            ];
            let projection = fields.map(|choices| col(&turns, choices)).join(",");
            format!("SELECT {projection} FROM thread_turns WHERE thread_id=?")
        });
        if has_threads || has_items {
            self.catalogs.push(Catalog {
                connection, path: path.to_owned(), items: items_query, turns: turns_query,
            });
            Ok(true)
        } else {
            connection.close().await?;
            Ok(false)
        }
    }

    pub(crate) async fn records(
        &mut self,
        thread: &str,
    ) -> Result<(VecDeque<(i64, NormalizedRecord)>, Option<PathBuf>, usize)> {
        let mut items = BTreeMap::new();
        let mut first_path = None;
        let mut malformed = 0;
        let mut events = BTreeMap::new();
        for catalog in &mut self.catalogs {
            if let Some(query) = &catalog.turns {
                for row in sqlx::query(query).bind(thread).fetch_all(&mut catalog.connection).await? {
                    let Some(id) = row.try_get::<Option<String>, _>(0)? else { continue };
                    let start = number(row.try_get(1)?);
                    let end: Option<String> = row.try_get(2)?;
                    let status: Option<String> = row.try_get(3)?;
                    let started = time(row.try_get(4)?);
                    let completed = time(row.try_get(5)?);
                    events.entry((id.clone(), 0)).or_insert_with(|| turn(start, 0, &id, "started", started));
                    let kind = match status.as_deref() {
                        Some("completed" | "complete") => "completed",
                        Some("interrupted" | "aborted" | "failed") => "interrupted",
                        _ => continue,
                    };
                    if let Some(end) = end.and_then(|ordinal| ordinal.parse().ok()) {
                        events.entry((id.clone(), 2)).or_insert_with(|| turn(end, 2, &id, kind, completed));
                    }
                }
            }
            let Some(query) = &catalog.items else { continue };
            let rows = sqlx::query(query).bind(thread).fetch_all(&mut catalog.connection).await?;
            for row in rows {
                let Some(id) = row.try_get::<Option<String>, _>(0)? else { continue };
                let turn: Option<String> = row.try_get(1)?;
                let Some(raw) = row.try_get::<Option<String>, _>(2)? else { continue };
                let value: Value = match serde_json::from_str(&raw) {
                    Ok(value) => value,
                    Err(_) => { malformed += 1; continue; }
                };
                let kind: Option<String> = row.try_get(3)?;
                let Some((role, content)) = message(&value, kind.as_deref()) else { continue };
                let order = number(row.try_get(4)?);
                let updated = number(row.try_get(5)?);
                let item = Item {
                    updated, order, phase: 1, id: id.clone(),
                    record: NormalizedRecord::Message {
                        timestamp: time(row.try_get(6)?),
                        role: role.into(),
                        content,
                        opens_turn: role == "user",
                        raw_json: json!({"type":"codex_sqlite_item","turn_id":turn,"item_id":id}).to_string(),
                    },
                };
                let key = (turn, id);
                if items.get(&key).is_none_or(|prior: &Item| updated > prior.updated) {
                    first_path.get_or_insert_with(|| catalog.path.clone());
                    items.insert(key, item);
                }
            }
        }
        let mut items: Vec<_> = items.into_values().collect();
        items.extend(events.into_values());
        items.sort_by(|a, b| (a.order, a.phase, &a.id).cmp(&(b.order, b.phase, &b.id)));
        let records = items.into_iter().enumerate()
            .map(|(index, item)| (index as i64, item.record)).collect();
        Ok((records, first_path, malformed))
    }

    pub(crate) async fn close(self) -> Result<()> {
        for catalog in self.catalogs {
            catalog.connection.close().await?;
        }
        Ok(())
    }
}

fn turn(order: i64, phase: u8, id: &str, kind: &'static str, timestamp: Option<String>) -> Item {
    Item {
        updated: order,
        order,
        phase,
        id: id.into(),
        record: NormalizedRecord::TurnEvent {
            timestamp,
            kind,
            turn_id: Some(id.into()),
            requested_model: None,
            served_model: None,
            raw_json: json!({"type":"codex_sqlite_turn","turn_id":id,"kind":kind}).to_string(),
        },
    }
}

fn number(value: Option<String>) -> i64 {
    value.and_then(|value| value.parse().ok()).unwrap_or_default()
}

fn label(value: Option<String>) -> Option<String> {
    value.filter(|value| {
        !value.is_empty()
            && value.len() <= 120
            && value.chars().all(|ch| ch.is_alphanumeric() || " _-.".contains(ch))
    })
}

fn time(value: Option<String>) -> Option<String> {
    use chrono::{DateTime, Utc};
    let value = value?;
    if let Ok(epoch) = value.parse::<i64>() {
        if epoch.unsigned_abs() >= 100_000_000_000 {
            return DateTime::<Utc>::from_timestamp_millis(epoch).map(|time| time.to_rfc3339());
        }
        return DateTime::<Utc>::from_timestamp(epoch, 0).map(|time| time.to_rfc3339());
    }
    DateTime::parse_from_rfc3339(&value).ok().map(|time| time.to_rfc3339())
}

fn message(value: &Value, kind: Option<&str>) -> Option<(&'static str, String)> {
    let role = match value["type"].as_str().or(kind)? {
        "userMessage" | "user_message" => "user",
        "agentMessage" | "agent_message" => "assistant",
        _ => return None,
    };
    let content = value["text"].as_str().map(str::to_owned).or_else(|| {
        let parts = value["content"].as_array()?;
        let text = parts.iter().filter(|part| {
            matches!(part["type"].as_str(), Some("text" | "input_text" | "output_text"))
        }).filter_map(|part| part["text"].as_str()).collect::<Vec<_>>().join("\n");
        Some(text)
    })?;
    (!content.is_empty()).then_some((role, content))
}
