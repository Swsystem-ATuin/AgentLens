//! Bounded, non-serializable browser-bound contexts. Never held in a database or cookie payload.
use anyhow::{Context, Result, ensure};
use std::{
    collections::HashMap,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::Instant,
};

const CAPACITY: usize = 1024;

pub(super) struct Entry<T> {
    binding: [u8; 32],
    expires: Instant,
    live: AtomicBool,
    pub(super) value: tokio::sync::Mutex<Option<T>>,
}
impl<T> Entry<T> {
    pub(super) fn check(&self) -> Result<()> {
        ensure!(
            self.live.load(Ordering::Acquire) && self.expires > Instant::now(),
            "OIDC context unavailable; restart sign-in"
        );
        Ok(())
    }
}

pub(super) struct Store<T> {
    entries: Mutex<HashMap<String, Arc<Entry<T>>>>,
}

fn binding(browser: &str) -> Result<[u8; 32]> {
    ensure!(
        !browser.is_empty() && browser.len() <= 256 && !browser.chars().any(char::is_control),
        "invalid OIDC runtime browser binding"
    );
    Ok(blake3::derive_key(
        "agent-lens-oidc-runtime-context-browser-v1",
        browser.as_bytes(),
    ))
}

impl<T> Store<T> {
    pub(super) fn new() -> Self {
        Self {
            entries: Mutex::new(HashMap::new()),
        }
    }

    pub(super) fn insert(&self, browser: &str, value: T, expires: Instant) -> Result<String> {
        let binding = binding(browser)?;
        ensure!(expires > Instant::now(), "OIDC context already expired");
        let mut entries = self
            .entries
            .lock()
            .map_err(|_| anyhow::anyhow!("OIDC context store unavailable"))?;
        entries.retain(|_, entry| {
            let retain = entry.expires > Instant::now() && entry.binding != binding;
            if !retain {
                entry.live.store(false, Ordering::Release);
            }
            retain
        });
        ensure!(entries.len() < CAPACITY, "OIDC context capacity reached");
        let handle = crate::auth::random_token();
        entries.insert(
            handle.clone(),
            Arc::new(Entry {
                binding,
                expires,
                live: AtomicBool::new(true),
                value: tokio::sync::Mutex::new(Some(value)),
            }),
        );
        Ok(handle)
    }

    pub(super) fn get(&self, browser: &str, handle: &str) -> Result<Arc<Entry<T>>> {
        ensure!(
            handle.len() == 43
                && handle
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"-_".contains(&b)),
            "invalid OIDC context handle"
        );
        let binding = binding(browser)?;
        let mut entries = self
            .entries
            .lock()
            .map_err(|_| anyhow::anyhow!("OIDC context store unavailable"))?;
        entries.retain(|_, entry| {
            let retain = entry.expires > Instant::now();
            if !retain {
                entry.live.store(false, Ordering::Release);
            }
            retain
        });
        let entry = entries
            .get(handle)
            .context("OIDC runtime context unavailable")?;
        ensure!(
            entry.binding == binding,
            "OIDC context belongs to another browser"
        );
        entry.check()?;
        Ok(entry.clone())
    }

    pub(super) fn cancel(&self, browser: &str, handle: &str) -> Result<()> {
        let binding = binding(browser)?;
        let mut entries = self
            .entries
            .lock()
            .map_err(|_| anyhow::anyhow!("OIDC context store unavailable"))?;
        let entry = entries
            .get(handle)
            .context("OIDC runtime context unavailable")?;
        ensure!(
            entry.binding == binding,
            "OIDC context belongs to another browser"
        );
        entry.check()?;
        entry.live.store(false, Ordering::Release);
        entries.remove(handle);
        Ok(())
    }

    pub(super) async fn take(&self, browser: &str, handle: &str) -> Result<T> {
        let entry = self.get(browser, handle)?;
        let mut value = entry.value.lock().await;
        entry.check()?;
        self.cancel(browser, handle)?;
        value.take().context("OIDC context already consumed")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    #[tokio::test]
    async fn contexts_are_bound_one_use_cancelled_expiring_bounded_and_restart_ephemeral() {
        let store = Store::new();
        let deadline = Instant::now() + Duration::from_secs(60);
        let handle = store.insert("browser", 1, deadline).unwrap();
        assert!(store.take("other", &handle).await.is_err());
        assert_eq!(store.take("browser", &handle).await.unwrap(), 1);
        assert!(store.take("browser", &handle).await.is_err());
        let old = store.insert("browser", 2, deadline).unwrap();
        let held = store.get("browser", &old).unwrap();
        let new = store.insert("browser", 3, deadline).unwrap();
        assert!(held.check().is_err());
        assert!(store.get("browser", &old).is_err());
        let (one, two) = tokio::join!(store.take("browser", &new), store.take("browser", &new));
        assert_ne!(one.is_ok(), two.is_ok());
        let handle = store.insert("browser", 4, deadline).unwrap();
        store.cancel("browser", &handle).unwrap();
        assert!(store.get("browser", &handle).is_err());
        assert!(store.insert("browser", 4, Instant::now()).is_err());
        let expired = store.insert("expires", 5, deadline).unwrap();
        Arc::get_mut(store.entries.lock().unwrap().get_mut(&expired).unwrap())
            .unwrap()
            .expires = Instant::now();
        assert!(store.get("expires", &expired).is_err());
        assert!(!store.entries.lock().unwrap().contains_key(&expired));
        for i in 0..CAPACITY {
            store.insert(&format!("browser-{i}"), i, deadline).unwrap();
        }
        assert!(store.insert("overflow", 0, deadline).is_err());
        store.insert("browser-0", 0, deadline).unwrap();
        let handle = store.entries.lock().unwrap().keys().next().unwrap().clone();
        let restarted = Store::<usize>::new();
        assert!(restarted.get("browser-0", &handle).is_err());
    }
}
