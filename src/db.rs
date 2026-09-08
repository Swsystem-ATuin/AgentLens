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
