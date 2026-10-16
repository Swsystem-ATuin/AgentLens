//! Explicit provider/key/credential preflight, never an authentication activation switch.
use super::{ClientAuthentication, Configuration, discovery, protocol::Protocol};
use anyhow::{Result, ensure};
use openidconnect::ClientSecret;
use std::{io::Read, path::Path, time::Duration};

fn secret(config: &Configuration) -> Result<Option<ClientSecret>> {
    let ClientAuthentication::ClientSecretBasic { secret_file } = &config.client_authentication
    else {
        return Ok(None);
    };
    read_secret(Path::new(secret_file)).map(Some)
}

fn read_secret(path: &Path) -> Result<ClientSecret> {
    // Do not expose file names, contents, or OS diagnostics in credential failures.
    let failure = || {
        anyhow::anyhow!(
            "OIDC client secret requires a protected regular UTF-8 file containing 1–4096 bytes without control characters"
        )
    };
    let before = std::fs::symlink_metadata(path).map_err(|_| failure())?;
    ensure!(before.is_file() && before.len() <= 4098, "{}", failure());
    let file = std::fs::File::open(path).map_err(|_| failure())?;
    let after = file.metadata().map_err(|_| failure())?;
    ensure!(after.is_file(), "{}", failure());
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        ensure!(
            before.dev() == after.dev() && before.ino() == after.ino() && after.mode() & 0o077 == 0,
            "{}",
            failure()
        );
    }
    #[cfg(not(unix))]
    anyhow::bail!("OIDC client secret file protection is unsupported on this platform");
    let mut bytes = Vec::new();
    file.take(4099)
        .read_to_end(&mut bytes)
        .map_err(|_| failure())?;
    if bytes.last() == Some(&b'\n') {
        bytes.pop();
        if bytes.last() == Some(&b'\r') {
            bytes.pop();
        }
    }
    let value = String::from_utf8(bytes).map_err(|_| failure())?;
    ensure!(
        !value.is_empty() && value.len() <= 4096 && !value.chars().any(char::is_control),
        "{}",
        failure()
    );
    Ok(ClientSecret::new(value))
}

pub(super) async fn prepare(config: &Configuration) -> Result<Protocol> {
    // Validate local credentials before making any network request. No secret is sent by GETs.
    let secret = secret(config)?;
    let client = discovery::client_builder(Duration::from_secs(10))
        .build()
        .map_err(|_| anyhow::anyhow!("initialize OIDC preparation HTTPS client"))?;
    prepare_with_client(config, secret, &client).await
}

pub(super) async fn prepare_with_client(
    config: &Configuration,
    secret: Option<ClientSecret>,
    client: &reqwest::Client,
) -> Result<Protocol> {
    let metadata = discovery::documents(client, config).await?;
    #[derive(serde::Deserialize)]
    struct Keys {
        jwks_uri: String,
    }
    let keys: Keys = serde_json::from_slice(&metadata)
        .map_err(|_| anyhow::anyhow!("invalid OIDC signing key location"))?;
    let jwks = discovery::document(client, &keys.jwks_uri).await?;
    Protocol::from_documents(config, &metadata, &jwks, secret)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn credentials_are_bounded_private_and_never_reflected() {
        let file = tempfile::NamedTempFile::new().unwrap();
        for value in [
            b"synthetic-private-marker".as_slice(),
            b"synthetic-private-marker\n",
            b"synthetic-private-marker\r\n",
        ] {
            std::fs::write(file.path(), value).unwrap();
            assert_eq!(
                read_secret(file.path()).unwrap().secret(),
                "synthetic-private-marker"
            );
        }
        for value in [
            Vec::new(),
            vec![b'a'; 4097],
            b"private-marker\n\n".to_vec(),
            b"private-marker\0".to_vec(),
            vec![255],
        ] {
            std::fs::write(file.path(), value).unwrap();
            let error = read_secret(file.path()).unwrap_err().to_string();
            assert!(!error.contains("private-marker"));
            assert!(!error.contains(&file.path().display().to_string()));
        }
        std::fs::write(file.path(), vec![b'a'; 4096]).unwrap();
        assert!(read_secret(file.path()).is_ok());
        assert!(read_secret(file.path().parent().unwrap()).is_err());
        #[cfg(unix)]
        {
            use std::os::unix::fs::{PermissionsExt, symlink};
            std::fs::set_permissions(file.path(), std::fs::Permissions::from_mode(0o644)).unwrap();
            assert!(read_secret(file.path()).is_err());
            std::fs::set_permissions(file.path(), std::fs::Permissions::from_mode(0o600)).unwrap();
            let dir = tempfile::tempdir().unwrap();
            let link = dir.path().join("secret");
            symlink(file.path(), &link).unwrap();
            assert!(read_secret(&link).is_err());
        }
    }
}
