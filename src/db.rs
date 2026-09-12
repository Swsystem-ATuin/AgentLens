use anyhow::{Context, Result, ensure};
use sqlx::{
    SqlitePool,
    sqlite::{SqliteConnectOptions, SqlitePoolOptions},
};
use std::{
    fs::OpenOptions,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

const SCHEMA: &str = include_str!("../schema.sql");
const DROP_MESSAGE_SEARCH_TRIGGERS: &str = "
    DROP TRIGGER IF EXISTS messages_search_insert;
    DROP TRIGGER IF EXISTS messages_search_delete;
    DROP TRIGGER IF EXISTS messages_search_update;
";
const CREATE_MESSAGE_SEARCH_TRIGGERS: &str = "
    CREATE TRIGGER IF NOT EXISTS messages_search_insert AFTER INSERT ON messages BEGIN
      INSERT INTO message_search(rowid,content,session_id,sequence,role) VALUES(new.id,new.content,new.session_id,new.sequence,new.role);
    END;
    CREATE TRIGGER IF NOT EXISTS messages_search_delete AFTER DELETE ON messages BEGIN
      INSERT INTO message_search(message_search,rowid,content,session_id,sequence,role) VALUES('delete',old.id,old.content,old.session_id,old.sequence,old.role);
    END;
    CREATE TRIGGER IF NOT EXISTS messages_search_update AFTER UPDATE ON messages BEGIN
      INSERT INTO message_search(message_search,rowid,content,session_id,sequence,role) VALUES('delete',old.id,old.content,old.session_id,old.sequence,old.role);
      INSERT INTO message_search(rowid,content,session_id,sequence,role) VALUES(new.id,new.content,new.session_id,new.sequence,new.role);
    END;
";
static REBUILD_SEQUENCE: AtomicU64 = AtomicU64::new(0);

pub struct Rebuild {
    pool: SqlitePool,
    staged_path: PathBuf,
    target_path: PathBuf,
}

pub async fn open(path: &Path) -> Result<SqlitePool> {
    let options = SqliteConnectOptions::new()
        .filename(path)
        .create_if_missing(true)
        .foreign_keys(true);
    let pool = SqlitePoolOptions::new()
        .max_connections(5)
        .connect_with(options)
        .await
        .with_context(|| format!("open SQLite index {}", path.display()))?;
    let existing_tables: Vec<String> = sqlx::query_scalar(
        "SELECT name FROM sqlite_master WHERE type='table' AND name IN ('index_metadata','message_search')",
    )
    .fetch_all(&pool)
    .await
    .context("inspect transcript search schema")?;
    let search_schema_current = existing_tables.iter().any(|name| name == "message_search")
        && existing_tables.iter().any(|name| name == "index_metadata")
        && sqlx::query_scalar::<_, String>(
            "SELECT value FROM index_metadata WHERE key='message_search_version'",
        )
        .fetch_optional(&pool)
        .await
        .context("inspect transcript search version")?
        .as_deref()
            == Some("1");
    sqlx::raw_sql(SCHEMA).execute(&pool).await?;
    sqlx::raw_sql(CREATE_MESSAGE_SEARCH_TRIGGERS)
        .execute(&pool)
        .await
        .context("enable transcript search maintenance")?;
    // Upgrade old disposable indexes without discarding their existing projections.
    let mut migration = pool.begin().await?;
    let columns: Vec<String> =
        sqlx::query_scalar("SELECT name FROM pragma_table_info('source_files')")
            .fetch_all(&mut *migration)
            .await?;
    for (column, definition) in [("content_hash", "TEXT"), ("parser_version", "INTEGER")] {
        if !columns.iter().any(|name| name == column) {
            sqlx::query(&format!(
                "ALTER TABLE source_files ADD COLUMN {column} {definition}"
            ))
            .execute(&mut *migration)
            .await
            .context("upgrade source checkpoint schema")?;
        }
    }
    let ingest_columns: Vec<String> =
        sqlx::query_scalar("SELECT name FROM pragma_table_info('ingest_runs')")
            .fetch_all(&mut *migration)
            .await?;
    if !ingest_columns.iter().any(|name| name == "source_layout") {
        sqlx::query(
            "ALTER TABLE ingest_runs ADD COLUMN source_layout TEXT NOT NULL DEFAULT 'host-user'",
        )
        .execute(&mut *migration)
        .await
        .context("upgrade ingest source-layout schema")?;
    }
    let instance_columns: Vec<String> =
        sqlx::query_scalar("SELECT name FROM pragma_table_info('agent_instances')")
            .fetch_all(&mut *migration)
            .await?;
    if !instance_columns.iter().any(|name| name == "source_key") {
        sqlx::query("ALTER TABLE agent_instances ADD COLUMN source_key TEXT")
            .execute(&mut *migration)
            .await
            .context("upgrade source identity schema")?;
    }
    let instances: Vec<(i64, String, String, String, String)> = sqlx::query_as(
        "SELECT i.id,a.kind,h.name,u.os_user,i.source_home FROM agent_instances i JOIN agents a ON a.id=i.agent_id JOIN users u ON u.id=i.user_id JOIN hosts h ON h.id=u.host_id WHERE i.source_key IS NULL",
    )
    .fetch_all(&mut *migration)
    .await
    .context("load source identities for upgrade")?;
    for (id, kind, host, user, source_home) in instances {
        let source_key = crate::identity::source_id(&kind, &host, &user, &source_home);
        sqlx::query("UPDATE agent_instances SET source_key=? WHERE id=?")
            .bind(source_key)
            .bind(id)
            .execute(&mut *migration)
            .await
            .context("backfill source identity")?;
    }
    let session_columns: Vec<String> =
        sqlx::query_scalar("SELECT name FROM pragma_table_info('sessions')")
            .fetch_all(&mut *migration)
            .await?;
    for (column, definition) in [("stable_id", "TEXT"), ("derived_title", "TEXT")] {
        if !session_columns.iter().any(|name| name == column) {
            sqlx::query(&format!(
                "ALTER TABLE sessions ADD COLUMN {column} {definition}"
            ))
            .execute(&mut *migration)
            .await
            .context("upgrade session identity schema")?;
        }
    }
    let sessions: Vec<(i64, String, String)> = sqlx::query_as(
        "SELECT s.id,i.source_key,s.external_id FROM sessions s JOIN agent_instances i ON i.id=s.instance_id WHERE s.stable_id IS NULL",
    )
    .fetch_all(&mut *migration)
    .await
    .context("load session identities for upgrade")?;
    for (id, source_key, native_id) in sessions {
        let stable_id = crate::identity::session_id(&source_key, &native_id);
        sqlx::query("UPDATE sessions SET stable_id=?,title=coalesce(title,external_id) WHERE id=?")
            .bind(stable_id)
            .bind(id)
            .execute(&mut *migration)
            .await
            .context("backfill session identity")?;
    }
    sqlx::query("CREATE UNIQUE INDEX IF NOT EXISTS idx_agent_instances_source_key ON agent_instances(source_key)")
        .execute(&mut *migration)
        .await
        .context("index source identities")?;
    sqlx::query("CREATE UNIQUE INDEX IF NOT EXISTS idx_sessions_stable_id ON sessions(stable_id)")
        .execute(&mut *migration)
        .await
        .context("index session identities")?;
    sqlx::query("CREATE INDEX IF NOT EXISTS idx_sessions_activity_stable ON sessions(julianday(COALESCE(ended_at,started_at)) DESC,stable_id DESC)")
        .execute(&mut *migration)
        .await
        .context("index stable session activity order")?;
    if !search_schema_current {
        sqlx::query("INSERT INTO message_search(message_search) VALUES('rebuild')")
            .execute(&mut *migration)
            .await
            .context("backfill transcript search index")?;
        sqlx::query(
            "INSERT INTO index_metadata(key,value) VALUES('message_search_version','1') \
             ON CONFLICT(key) DO UPDATE SET value=excluded.value",
        )
        .execute(&mut *migration)
        .await
        .context("record transcript search schema version")?;
    }
    migration.commit().await?;
    Ok(pool)
}

pub async fn begin_rebuild(path: &Path) -> Result<Rebuild> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let file_name = path
        .file_name()
        .context("SQLite index path must name a file")?
        .to_string_lossy();
    let staged_path = loop {
        let sequence = REBUILD_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let candidate = parent.join(format!(
            ".{file_name}.rebuild-{}-{sequence}",
            std::process::id()
        ));
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&candidate)
        {
            Ok(_) => break candidate,
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => {
                return Err(error).with_context(|| {
                    format!("create staged SQLite index {}", candidate.display())
                });
            }
        }
    };
    let pool = match open(&staged_path).await {
        Ok(pool) => pool,
        Err(error) => {
            let _ = remove_database_files(&staged_path);
            return Err(error).context("initialize staged rebuild");
        }
    };
    if let Err(error) = sqlx::raw_sql(DROP_MESSAGE_SEARCH_TRIGGERS)
        .execute(&pool)
        .await
        .context("prepare staged transcript search rebuild")
    {
        pool.close().await;
        let _ = remove_database_files(&staged_path);
        return Err(error);
    }
    Ok(Rebuild {
        pool,
        staged_path,
        target_path: path.to_path_buf(),
    })
}

impl Rebuild {
    pub fn pool(&self) -> &SqlitePool {
        &self.pool
    }

    pub async fn publish(self) -> Result<()> {
        let result = self.publish_inner().await;
        if result.is_err() {
            self.pool.close().await;
            let _ = remove_database_files(&self.staged_path);
        }
        result
    }

    pub async fn discard(self) -> Result<()> {
        self.pool.close().await;
        remove_database_files(&self.staged_path)
            .with_context(|| format!("discard staged rebuild {}", self.staged_path.display()))
    }

    async fn publish_inner(&self) -> Result<()> {
        let pool = self.pool();
        sqlx::query("INSERT INTO message_search(message_search) VALUES('rebuild')")
            .execute(pool)
            .await
            .context("build staged transcript search index")?;
        sqlx::raw_sql(CREATE_MESSAGE_SEARCH_TRIGGERS)
            .execute(pool)
            .await
            .context("enable staged transcript search maintenance")?;
        let integrity: Vec<String> = sqlx::query_scalar("PRAGMA quick_check")
            .fetch_all(pool)
            .await
            .context("validate staged SQLite index integrity")?;
        ensure!(
            integrity.as_slice() == ["ok"],
            "staged SQLite index failed integrity validation"
        );
        ensure!(
            sqlx::query("PRAGMA foreign_key_check")
                .fetch_optional(pool)
                .await
                .context("validate staged SQLite foreign keys")?
                .is_none(),
            "staged SQLite index failed foreign-key validation"
        );
        let unfinished_runs: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM ingest_runs WHERE status != 'complete' OR finished_at IS NULL",
        )
        .fetch_one(pool)
        .await
        .context("validate staged ingest runs")?;
        ensure!(
            unfinished_runs == 0,
            "staged SQLite index has an unfinished ingest run"
        );

        sqlx::query("PRAGMA wal_checkpoint(TRUNCATE)")
            .execute(pool)
            .await
            .context("checkpoint staged SQLite index")?;
        self.pool.close().await;
        remove_sidecars(&self.staged_path).with_context(|| {
            format!(
                "remove staged SQLite sidecars {}",
                self.staged_path.display()
            )
        })?;

        prepare_target_for_replacement(&self.target_path).await?;
        std::fs::rename(&self.staged_path, &self.target_path).with_context(|| {
            format!(
                "publish staged SQLite index {} as {}",
                self.staged_path.display(),
                self.target_path.display()
            )
        })?;
        Ok(())
    }
}

async fn prepare_target_for_replacement(path: &Path) -> Result<()> {
    let has_sidecars = sidecar_path(path, "-wal").exists() || sidecar_path(path, "-shm").exists();
    if path.exists() && has_sidecars {
        let options = SqliteConnectOptions::new()
            .filename(path)
            .create_if_missing(false)
            .foreign_keys(true);
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(options)
            .await
            .with_context(|| format!("open existing SQLite index {}", path.display()))?;
        let checkpoint: (i64, i64, i64) = sqlx::query_as("PRAGMA wal_checkpoint(TRUNCATE)")
            .fetch_one(&pool)
            .await
            .context("checkpoint existing SQLite index before replacement")?;
        pool.close().await;
        ensure!(
            checkpoint.0 == 0,
            "existing SQLite index is busy; stop the web server and retry rebuild"
        );
    } else {
        remove_sidecars(path)
            .with_context(|| format!("remove orphaned SQLite sidecars for {}", path.display()))?;
    }
    ensure!(
        !sidecar_path(path, "-wal").exists() && !sidecar_path(path, "-shm").exists(),
        "existing SQLite index is still open; stop the web server and retry rebuild"
    );
    Ok(())
}

fn sidecar_path(path: &Path, suffix: &str) -> PathBuf {
    path.with_file_name(format!(
        "{}{}",
        path.file_name().unwrap_or_default().to_string_lossy(),
        suffix
    ))
}

fn remove_sidecars(path: &Path) -> Result<()> {
    for suffix in ["-wal", "-shm"] {
        let sidecar = sidecar_path(path, suffix);
        match std::fs::remove_file(&sidecar) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(error).with_context(|| format!("remove {}", sidecar.display()));
            }
        }
    }
    Ok(())
}

fn remove_database_files(path: &Path) -> Result<()> {
    match std::fs::remove_file(path) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(error).with_context(|| format!("remove {}", path.display()));
        }
    }
    remove_sidecars(path)
}

#[cfg(test)]
mod tests {
    #[tokio::test]
    async fn every_pool_connection_enforces_foreign_keys_and_reopen_preserves_rows() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("index.sqlite");
        let pool = super::open(&path).await.unwrap();
        let mut connections = Vec::new();
        for _ in 0..5 {
            let mut connection = pool.acquire().await.unwrap();
            assert!(
                sqlx::query("INSERT INTO users(host_id,os_user) VALUES(999,'synthetic')")
                    .execute(&mut *connection)
                    .await
                    .is_err()
            );
            connections.push(connection);
        }
        drop(connections);
        sqlx::query("INSERT INTO hosts(name) VALUES('synthetic')")
            .execute(&pool)
            .await
            .unwrap();
        pool.close().await;
        let pool = super::open(&path).await.unwrap();
        let hosts: Vec<String> = sqlx::query_scalar("SELECT name FROM hosts")
            .fetch_all(&pool)
            .await
            .unwrap();
        assert_eq!(hosts, ["synthetic"]);
    }

    #[tokio::test]
    async fn opening_a_legacy_index_backfills_transcript_search_once() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("index.sqlite");
        let pool = super::open(&path).await.unwrap();
        sqlx::raw_sql(
            "INSERT INTO agents VALUES(1,'synthetic','Synthetic');
             INSERT INTO hosts VALUES(1,'host');
             INSERT INTO users VALUES(1,1,'user');
             INSERT INTO agent_instances(id,agent_id,user_id,source_home) VALUES(1,1,1,'home');
             INSERT INTO sessions(id,instance_id,external_id,stable_id,title) VALUES(1,1,'native','stable','title');
             INSERT INTO messages(session_id,sequence,role,content) VALUES(1,0,'user','legacy searchable text');
             DROP TABLE message_search;",
        )
        .execute(&pool)
        .await
        .unwrap();
        pool.close().await;

        let pool = super::open(&path).await.unwrap();
        let matches: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM message_search WHERE message_search MATCH 'searchable'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(matches, 1);
        pool.close().await;

        let pool = super::open(&path).await.unwrap();
        let matches: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM message_search WHERE message_search MATCH 'searchable'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(matches, 1, "normal reopen must not duplicate the index");
    }

    #[tokio::test]
    async fn discarded_staged_rebuild_preserves_the_existing_index() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("index.sqlite");
        let pool = super::open(&path).await.unwrap();
        sqlx::query("INSERT INTO hosts(name) VALUES('existing')")
            .execute(&pool)
            .await
            .unwrap();
        pool.close().await;

        let rebuild = super::begin_rebuild(&path).await.unwrap();
        sqlx::query("INSERT INTO hosts(name) VALUES('replacement')")
            .execute(rebuild.pool())
            .await
            .unwrap();
        rebuild.discard().await.unwrap();

        let pool = super::open(&path).await.unwrap();
        let hosts: Vec<String> = sqlx::query_scalar("SELECT name FROM hosts ORDER BY name")
            .fetch_all(&pool)
            .await
            .unwrap();
        assert_eq!(hosts, ["existing"]);
    }

    #[tokio::test]
    async fn validated_staged_rebuild_replaces_the_existing_index() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("index.sqlite");
        let existing = super::open(&path).await.unwrap();
        sqlx::query("INSERT INTO hosts(name) VALUES('existing')")
            .execute(&existing)
            .await
            .unwrap();
        existing.close().await;

        let rebuild = super::begin_rebuild(&path).await.unwrap();
        sqlx::query("INSERT INTO hosts(name) VALUES('replacement')")
            .execute(rebuild.pool())
            .await
            .unwrap();
        rebuild.publish().await.unwrap();

        let published = super::open(&path).await.unwrap();
        let hosts: Vec<String> = sqlx::query_scalar("SELECT name FROM hosts ORDER BY name")
            .fetch_all(&published)
            .await
            .unwrap();
        assert_eq!(hosts, ["replacement"]);
    }

    #[tokio::test]
    async fn publication_refuses_an_open_existing_index() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("index.sqlite");
        let existing = super::open(&path).await.unwrap();
        sqlx::query("INSERT INTO hosts(name) VALUES('existing')")
            .execute(&existing)
            .await
            .unwrap();

        let rebuild = super::begin_rebuild(&path).await.unwrap();
        sqlx::query("INSERT INTO hosts(name) VALUES('replacement')")
            .execute(rebuild.pool())
            .await
            .unwrap();
        let error = rebuild.publish().await.unwrap_err();
        assert!(
            format!("{error:#}").contains("stop the web server"),
            "unexpected publication error: {error:#}"
        );
        let hosts: Vec<String> = sqlx::query_scalar("SELECT name FROM hosts ORDER BY name")
            .fetch_all(&existing)
            .await
            .unwrap();
        assert_eq!(hosts, ["existing"]);
        existing.close().await;
    }
}
