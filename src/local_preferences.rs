//! Non-authentication preferences for a local browser, never an identity or access credential.
use crate::preferences::UserPreferences;
use anyhow::{Context, Result, ensure};
use axum::http::{HeaderMap, HeaderValue, header};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use tower_sessions::cookie::{Cookie, SameSite, time::Duration};

const NAME: &str = "agent_lens_local_preferences";
const MAX_VALUE_BYTES: usize = 2048;

pub(crate) fn read(headers: &HeaderMap) -> UserPreferences {
    let mut values = headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(Cookie::split_parse)
        .filter_map(std::result::Result::ok)
        .filter(|cookie| cookie.name() == NAME);
    let Some(cookie) = values.next() else {
        return UserPreferences::default();
    };
    if values.next().is_some() || cookie.value().len() > MAX_VALUE_BYTES {
        return UserPreferences::default();
    }
    let preferences = URL_SAFE_NO_PAD
        .decode(cookie.value())
        .ok()
        .and_then(|bytes| serde_json::from_slice::<UserPreferences>(&bytes).ok());
    preferences
        .filter(|preferences| preferences.validate().is_ok())
        .unwrap_or_default()
}

pub(crate) fn set(preferences: &UserPreferences) -> Result<HeaderValue> {
    preferences.validate().map_err(anyhow::Error::msg)?;
    let value = URL_SAFE_NO_PAD
        .encode(serde_json::to_vec(preferences).context("encode local display preferences")?);
    ensure!(
        value.len() <= MAX_VALUE_BYTES,
        "local display preferences exceed cookie size limit"
    );
    Cookie::build((NAME, value))
        .path("/")
        .http_only(true)
        .same_site(SameSite::Lax)
        .max_age(Duration::days(365))
        .build()
        .to_string()
        .parse()
        .context("prepare local display preference cookie")
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn local_cookie_is_bounded_validated_and_cannot_carry_authentication_state() {
        let preferences = UserPreferences {
            page_size: 0,
            appearance: "dark".into(),
            ..UserPreferences::default()
        };
        let cookie = set(&preferences).unwrap();
        let cookie = Cookie::parse(cookie.to_str().unwrap()).unwrap();
        assert!(cookie.http_only().unwrap());
        assert_eq!(cookie.same_site(), Some(SameSite::Lax));
        assert_eq!(cookie.path(), Some("/"));
        let mut headers = HeaderMap::new();
        headers.insert(
            header::COOKIE,
            format!("{NAME}={}", cookie.value()).parse().unwrap(),
        );
        assert_eq!(read(&headers), preferences);
        for invalid in [
            "not-base64!".to_owned(),
            "a".repeat(MAX_VALUE_BYTES + 1),
            URL_SAFE_NO_PAD.encode(br#"{"appearance":"<script>","page_size":0}"#),
        ] {
            headers.insert(header::COOKIE, format!("{NAME}={invalid}").parse().unwrap());
            assert_eq!(read(&headers), UserPreferences::default());
        }
        headers.insert(
            header::COOKIE,
            format!("{NAME}={}; {NAME}={}", cookie.value(), cookie.value())
                .parse()
                .unwrap(),
        );
        assert_eq!(
            read(&headers),
            UserPreferences::default(),
            "ambiguous cookies must fail to safe defaults"
        );
        let injection=URL_SAFE_NO_PAD.encode(br#"{"id":"admin","role":"admin","auth.account_id":"admin","auth.mfa_verified":true,"appearance":"dark"}"#);
        headers.insert(
            header::COOKIE,
            format!("{NAME}={injection}").parse().unwrap(),
        );
        let parsed = read(&headers);
        assert_eq!(parsed.appearance, "dark");
        assert_eq!(parsed.page_size, 50);
        assert!(
            !serde_json::to_string(&parsed)
                .unwrap()
                .contains("auth.account_id")
        );
    }
}
