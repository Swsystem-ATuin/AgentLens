//! Declared prerequisites, not a probe of the deployed reverse proxy.
use super::{Configuration, https_url};
use anyhow::{Result, ensure};
use password_hash::PasswordHash;
use sqlx::{Connection, SqliteConnection, sqlite::SqliteConnectOptions};
use std::path::Path;
use webauthn_rs::prelude::Url;

pub(super) async fn check(
    config: &Configuration,
    public_url: Option<&str>,
    secure_cookies: bool,
    local: bool,
    state: &Path,
) -> Result<()> {
    ensure!(
        secure_cookies && !local,
        "OIDC requires Secure cookies and authenticated mode"
    );
    let public_url = public_url
        .ok_or_else(|| anyhow::anyhow!("OIDC requires an explicit deployment public URL"))?;
    https_url(public_url, true)?;
    let configured = Url::parse(&config.public_url)
        .map_err(|_| anyhow::anyhow!("invalid OIDC configured origin"))?;
    let deployed =
        Url::parse(public_url).map_err(|_| anyhow::anyhow!("invalid OIDC deployment origin"))?;
    ensure!(
        configured.origin() == deployed.origin(),
        "OIDC public URL must match the deployment origin"
    );
    // Never create, migrate, prune, or open the disposable index during preflight.
    let mut connection = SqliteConnection::connect_with(
        &SqliteConnectOptions::new()
            .filename(state)
            .read_only(true)
            .create_if_missing(false)
            .foreign_keys(true),
    )
    .await
    .map_err(|_| anyhow::anyhow!("open existing OIDC deployment state read-only"))?;
    let result = administrator(&mut connection).await;
    connection
        .close()
        .await
        .map_err(|_| anyhow::anyhow!("close OIDC deployment state"))?;
    result
}

async fn administrator(connection: &mut SqliteConnection) -> Result<()> {
    let version: Option<String> =
        sqlx::query_scalar("SELECT value FROM app_state_metadata WHERE key='schema_version'")
            .fetch_optional(&mut *connection)
            .await
            .map_err(|_| anyhow::anyhow!("read OIDC deployment state schema"))?;
    ensure!(
        version.as_deref() == Some("10"),
        "OIDC deployment check requires state schema v10; migrate using the normal account/service workflow first"
    );
    let hashes: Vec<String> = sqlx::query_scalar(
        "SELECT password_hash FROM accounts WHERE enabled=1 AND role='admin' LIMIT 1025",
    )
    .fetch_all(connection)
    .await
    .map_err(|_| anyhow::anyhow!("inspect OIDC local administrator credentials"))?;
    ensure!(
        hashes.len() <= 1024,
        "OIDC administrator inspection limit exceeded"
    );
    ensure!(
        hashes.iter().any(|value| {
            PasswordHash::new(value).ok().is_some_and(|hash| {
                hash.algorithm.as_str() == "argon2id"
                    && hash.version == Some(19)
                    && hash.salt.is_some()
                    && hash.hash.is_some()
                    && argon2::Params::try_from(&hash).is_ok()
            })
        }),
        "OIDC requires an enabled local administrator with a valid password credential; retain and test local recovery access"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn unsafe_settings_and_missing_state_fail_without_creating_files() {
        let config: Configuration =
            serde_json::from_value(super::super::tests::configuration()).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("absent.sqlite");
        for (url, secure, local) in [
            (None, true, false),
            (Some("https://lens.example.test"), false, false),
            (Some("https://lens.example.test"), true, true),
            (Some("http://lens.example.test"), true, false),
            (Some("https://other.example.test"), true, false),
            (Some("https://lens.example.test/path"), true, false),
        ] {
            assert!(check(&config, url, secure, local, &path).await.is_err());
        }
        assert!(
            check(
                &config,
                Some("https://lens.example.test"),
                true,
                false,
                &path
            )
            .await
            .is_err()
        );
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
    }

    #[tokio::test]
    async fn current_schema_and_enabled_local_admin_are_required_without_mutation() {
        let config: Configuration =
            serde_json::from_value(super::super::tests::configuration()).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.sqlite");
        let mut db = SqliteConnection::connect_with(
            &SqliteConnectOptions::new()
                .filename(&path)
                .create_if_missing(true),
        )
        .await
        .unwrap();
        sqlx::raw_sql("CREATE TABLE app_state_metadata(key TEXT,value TEXT); INSERT INTO app_state_metadata VALUES('schema_version','10'); CREATE TABLE accounts(enabled INTEGER, role TEXT,password_hash TEXT);").execute(&mut db).await.unwrap();
        let hash = crate::auth::hash_password("Synthetic1-password").unwrap();
        for (enabled, role, password, accepted) in [
            (0, "admin", hash.as_str(), false),
            (1, "viewer", hash.as_str(), false),
            (1, "admin", "not-a-hash", false),
            (1, "admin", hash.as_str(), true),
        ] {
            sqlx::query("DELETE FROM accounts")
                .execute(&mut db)
                .await
                .unwrap();
            sqlx::query("INSERT INTO accounts VALUES(?,?,?)")
                .bind(enabled)
                .bind(role)
                .bind(password)
                .execute(&mut db)
                .await
                .unwrap();
            let before = std::fs::read(&path).unwrap();
            assert_eq!(
                check(
                    &config,
                    Some("https://lens.example.test:443/"),
                    true,
                    false,
                    &path
                )
                .await
                .is_ok(),
                accepted
            );
            assert_eq!(
                std::fs::read(&path).unwrap(),
                before,
                "preflight must not mutate persistent state"
            );
        }
        sqlx::query("UPDATE app_state_metadata SET value='11'")
            .execute(&mut db)
            .await
            .unwrap();
        assert!(
            check(
                &config,
                Some("https://lens.example.test"),
                true,
                false,
                &path
            )
            .await
            .is_err()
        );
    }
}
