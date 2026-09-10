use anyhow::{Context, Result};
use sqlx::{
    SqlitePool,
    sqlite::{SqliteConnectOptions, SqlitePoolOptions},
};
use std::path::Path;

const SCHEMA: &str = include_str!("../schema.sql");

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
    migration.commit().await?;
    Ok(pool)
}

pub async fn rebuild(path: &Path) -> Result<SqlitePool> {
    if path.exists() {
        std::fs::remove_file(path)
            .with_context(|| format!("remove old index {}", path.display()))?;
    }
    for suffix in ["-wal", "-shm"] {
        let sidecar = path.with_file_name(format!(
            "{}{}",
            path.file_name().unwrap_or_default().to_string_lossy(),
            suffix
        ));
        if sidecar.exists() {
            std::fs::remove_file(sidecar)?;
        }
    }
    open(path).await
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
}
