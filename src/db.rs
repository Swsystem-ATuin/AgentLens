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
    sqlx::raw_sql(SCHEMA).execute(&pool).await?;
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
