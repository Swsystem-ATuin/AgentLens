use super::*;
use openssl::{
    hash::MessageDigest,
    pkey::{PKey, Private},
    rsa::Rsa,
    sign::Signer,
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    future::{Ready, ready},
    sync::OnceLock,
};
use webauthn_rs::prelude::Url;

const ISSUER: &str = "https://identity.example.test/tenant";
const CLIENT_ID: &str = "synthetic-client";
const ACCESS: &str = "synthetic-access-token-PrivateMarker";
const SUBJECT: &str = "synthetic-subject-PrivateMarker";

#[tokio::test]
async fn preparation_fetches_pinned_metadata_and_keys_without_credentials() {
    use crate::oidc::{
        discovery::tests::{response, server},
        preparation::prepare_with_client,
    };
    use std::time::Duration;
    for valid_keys in [true, false] {
        let (keys_config, _, keys_server) = server(
            move |_| {
                response(
                    "200 OK",
                    "Content-Type: application/json\r\n",
                    &if valid_keys {
                        jwks(key())
                    } else {
                        b"{\"keys\":[]}".to_vec()
                    },
                )
            },
            Duration::from_secs(3),
            true,
        );
        let keys_url = format!("{}/keys", keys_config.issuer);
        let (mut config, client, discovery_server) = server(
            move |issuer| {
                let mut metadata: Value = serde_json::from_slice(&discovery()).unwrap();
                metadata["issuer"] = json!(issuer);
                metadata["jwks_uri"] = json!(keys_url);
                response(
                    "200 OK",
                    "Content-Type: application/json\r\n",
                    &serde_json::to_vec(&metadata).unwrap(),
                )
            },
            Duration::from_secs(3),
            true,
        );
        config.client_authentication = ClientAuthentication::None;
        assert_eq!(
            prepare_with_client(&config, None, &client).await.is_ok(),
            valid_keys
        );
        for request in [
            discovery_server.join().unwrap(),
            keys_server.join().unwrap(),
        ] {
            let request = request.to_ascii_lowercase();
            assert!(request.starts_with("get "));
            assert!(!request.contains("authorization:"));
            assert!(!request.contains("cookie:"));
            assert!(!request.contains("private-marker"));
        }
    }
}

fn key() -> &'static PKey<Private> {
    static KEY: OnceLock<PKey<Private>> = OnceLock::new();
    KEY.get_or_init(|| PKey::from_rsa(Rsa::generate(2048).unwrap()).unwrap())
}

fn jwks(key: &PKey<Private>) -> Vec<u8> {
    let rsa = key.rsa().unwrap();
    serde_json::to_vec(&json!({"keys":[{
        "kty":"RSA", "kid":"synthetic-key", "use":"sig", "alg":"RS256",
        "n":URL_SAFE_NO_PAD.encode(rsa.n().to_vec()), "e":URL_SAFE_NO_PAD.encode(rsa.e().to_vec())
    }]}))
    .unwrap()
}

fn config(public: bool) -> Configuration {
    serde_json::from_value(json!({
        "issuer":ISSUER, "public_url":"https://lens.example.test/", "client_id":CLIENT_ID,
        "client_authentication":if public { json!({"method":"none"}) } else { json!({"method":"client_secret_basic","secret_file":"/nonexistent/secret"}) },
        "account_linking":"administrator_provisioned_issuer_subject", "permissions":"local_only",
        "logout":"rp_initiated_and_back_channel_required", "local_break_glass":true
    })).unwrap()
}

fn discovery() -> Vec<u8> {
    serde_json::to_vec(&json!({
        "issuer":ISSUER, "authorization_endpoint":"https://identity.example.test/authorize",
        "token_endpoint":"https://identity.example.test/token", "jwks_uri":"https://identity.example.test/jwks",
        "end_session_endpoint":"https://identity.example.test/logout",
        "response_types_supported":["code"], "subject_types_supported":["pairwise"],
        "id_token_signing_alg_values_supported":["RS256"], "code_challenge_methods_supported":["S256"],
        "token_endpoint_auth_methods_supported":["none","client_secret_basic"], "backchannel_logout_supported":true
    })).unwrap()
}

fn protocol(public: bool) -> Protocol {
    Protocol::from_documents(
        &config(public),
        &discovery(),
        &jwks(key()),
        (!public).then(|| ClientSecret::new("synthetic-secret".into())),
    )
    .unwrap()
}

fn params(url: &str) -> BTreeMap<String, String> {
    Url::parse(url)
        .unwrap()
        .query_pairs()
        .map(|(key, value)| (key.into_owned(), value.into_owned()))
        .collect()
}

fn claims(nonce: &str) -> Value {
    let now = chrono::Utc::now().timestamp();
    json!({"iss":ISSUER, "sub":SUBJECT, "aud":CLIENT_ID, "iat":now, "exp":now+300, "nonce":nonce,
        "azp":CLIENT_ID, "email":"untrusted-synthetic@example.test", "roles":["admin"], "unknown_extension":{"ignored":true}})
}

fn signed(claims: &Value, header: &Value, signing_key: &PKey<Private>) -> String {
    let input = format!(
        "{}.{}",
        URL_SAFE_NO_PAD.encode(serde_json::to_vec(header).unwrap()),
        URL_SAFE_NO_PAD.encode(serde_json::to_vec(claims).unwrap())
    );
    let mut signer = Signer::new(MessageDigest::sha256(), signing_key).unwrap();
    signer.update(input.as_bytes()).unwrap();
    format!(
        "{input}.{}",
        URL_SAFE_NO_PAD.encode(signer.sign_to_vec().unwrap())
    )
}

fn token(claims: &Value) -> String {
    signed(
        claims,
        &json!({"alg":"RS256", "typ":"JWT", "kid":"synthetic-key"}),
        key(),
    )
}

struct Provider {
    body: Vec<u8>,
    status: u16,
    requests: Mutex<Vec<HttpRequest>>,
}
impl Provider {
    fn new(id_token: &str) -> Self {
        Self { body:serde_json::to_vec(&json!({"access_token":ACCESS, "token_type":"Bearer", "id_token":id_token, "expires_in":300, "unknown_field":true})).unwrap(), status:200, requests:Mutex::new(Vec::new()) }
    }
}
impl<'c> AsyncHttpClient<'c> for Provider {
    type Error = io::Error;
    type Future = Ready<std::result::Result<HttpResponse, io::Error>>;
    fn call(&'c self, request: HttpRequest) -> Self::Future {
        self.requests.lock().unwrap().push(request);
        ready(Ok(axum::http::Response::builder()
            .status(self.status)
            .header("content-type", "application/json")
            .body(self.body.clone())
            .unwrap()))
    }
}

#[tokio::test]
async fn signed_code_flow_uses_s256_exact_redirect_and_correct_client_authentication() {
    for public in [false, true] {
        let protocol = protocol(public);
        let authorization = protocol.begin("opaque-browser-session").unwrap();
        let args = params(&authorization);
        assert_eq!(args["response_type"], "code");
        assert_eq!(args["response_mode"], "query");
        assert_eq!(args["client_id"], CLIENT_ID);
        assert_eq!(args["scope"], "openid");
        assert_eq!(
            args["redirect_uri"],
            "https://lens.example.test/login/oidc/callback"
        );
        assert_eq!(args["code_challenge_method"], "S256");
        assert!(args["state"].len() >= 32 && args["nonce"].len() >= 32);
        assert!(!authorization.contains("synthetic-secret"));
        let mut payload = claims(&args["nonce"]);
        payload["at_hash"] =
            json!(URL_SAFE_NO_PAD.encode(&Sha256::digest(ACCESS.as_bytes())[..16]));
        let jwt = token(&payload);
        let provider = Provider::new(&jwt);
        let identity = protocol
            .finish(
                "opaque-browser-session",
                &args["state"],
                "synthetic-code",
                &provider,
            )
            .await
            .unwrap();
        assert_eq!(identity.issuer, ISSUER);
        assert_eq!(
            identity.subject, SUBJECT,
            "email and role claims must not replace verified subject identity"
        );
        assert_eq!(identity.id_token_hint, jwt);
        assert!(
            protocol
                .finish(
                    "opaque-browser-session",
                    &args["state"],
                    "synthetic-code",
                    &provider
                )
                .await
                .is_err(),
            "callback replay must not exchange again"
        );
        let requests = provider.requests.lock().unwrap();
        assert_eq!(requests.len(), 1);
        let request = &requests[0];
        assert_eq!(request.method(), "POST");
        assert_eq!(request.uri(), "https://identity.example.test/token");
        let fields = params(&format!(
            "https://form.example.test/?{}",
            String::from_utf8_lossy(request.body())
        ));
        assert_eq!(fields["grant_type"], "authorization_code");
        assert_eq!(fields["code"], "synthetic-code");
        assert_eq!(fields["redirect_uri"], args["redirect_uri"]);
        assert_eq!(
            URL_SAFE_NO_PAD.encode(Sha256::digest(fields["code_verifier"].as_bytes())),
            args["code_challenge"]
        );
        if public {
            assert!(!request.headers().contains_key("authorization"));
            assert_eq!(fields["client_id"], CLIENT_ID);
        } else {
            use base64::engine::general_purpose::STANDARD;
            let expected = format!(
                "Basic {}",
                STANDARD.encode(format!("{CLIENT_ID}:synthetic-secret"))
            );
            assert_eq!(request.headers()["authorization"], expected);
            assert!(!fields.contains_key("client_secret"));
        }
    }
}

#[tokio::test]
async fn state_browser_binding_expiry_replacement_restart_and_concurrency_are_one_use() {
    let engine = protocol(true);
    let args = params(&engine.begin("browser-a").unwrap());
    let provider = Provider::new(&token(&claims(&args["nonce"])));
    assert!(
        engine
            .finish("browser-b", &args["state"], "code", &provider)
            .await
            .is_err()
    );
    let provider_ref = &provider;
    let delayed = move |request| async move {
        // Let the second callback run while the first token exchange is still pending.
        tokio::task::yield_now().await;
        provider_ref.call(request).await
    };
    let (one, two) = tokio::join!(
        engine.finish("browser-a", &args["state"], "code", &delayed),
        engine.finish("browser-a", &args["state"], "code", &delayed)
    );
    assert_ne!(one.is_ok(), two.is_ok());
    assert_eq!(provider.requests.lock().unwrap().len(), 1);
    let engine = protocol(true);
    let args = params(&engine.begin("browser-a").unwrap());
    let provider = Provider::new(&token(&claims(&args["nonce"])));
    assert!(
        engine
            .finish("browser-a", "wrong-state", "code", &provider)
            .await
            .is_err()
    );
    assert!(
        engine
            .finish("browser-a", &args["state"], "code", &provider)
            .await
            .is_err()
    );
    assert!(provider.requests.lock().unwrap().is_empty());
    let old = params(&engine.begin("browser-a").unwrap());
    let replacement = params(&engine.begin("browser-a").unwrap());
    assert_ne!(old["state"], replacement["state"]);
    assert!(
        engine
            .finish("browser-a", &old["state"], "code", &provider)
            .await
            .is_err()
    );
    assert!(
        engine
            .finish("browser-a", &replacement["state"], "code", &provider)
            .await
            .is_err(),
        "a mismatched callback consumes the browser's transaction too"
    );
    let args = params(&engine.begin("browser-a").unwrap());
    engine
        .pending
        .lock()
        .unwrap()
        .values_mut()
        .next()
        .unwrap()
        .expires = Instant::now() - Duration::from_secs(1);
    assert!(
        engine
            .finish("browser-a", &args["state"], "code", &provider)
            .await
            .is_err()
    );
    let args = params(&engine.begin("browser-a").unwrap());
    assert!(
        protocol(true)
            .finish("browser-a", &args["state"], "code", &provider)
            .await
            .is_err(),
        "process restart must lose outstanding challenges"
    );
}

#[tokio::test]
async fn invalid_signatures_keys_algorithms_claims_and_hashes_fail_without_disclosing_values() {
    let now = chrono::Utc::now().timestamp();
    for (name, replacement) in [
        ("iss", json!("https://other.example.test")),
        ("iss", json!("https://IDENTITY.example.test/tenant")),
        ("iss", json!(format!("{ISSUER}/"))),
        ("aud", json!("another-client")),
        ("aud", json!([CLIENT_ID, "another-client"])),
        ("aud", json!([])),
        ("azp", json!("another-client")),
        ("exp", json!(now - 1)),
        ("iat", json!(now + 120)),
        ("iat", json!(now - 900)),
        ("nonce", json!("synthetic-wrong-nonce-PrivateMarker")),
        ("sub", json!("")),
        ("sub", json!("non-ascii-é")),
        ("sub", json!("subject\n")),
        ("sub", json!("s".repeat(256))),
        ("at_hash", json!("synthetic-invalid-hash-PrivateMarker")),
    ] {
        let engine = protocol(true);
        let args = params(&engine.begin("browser").unwrap());
        let mut payload = claims(&args["nonce"]);
        payload[name] = replacement;
        let provider = Provider::new(&token(&payload));
        let error = engine
            .finish("browser", &args["state"], "code", &provider)
            .await
            .err()
            .unwrap()
            .to_string();
        assert!(
            !error.contains("PrivateMarker"),
            "{name} must return sanitized errors"
        );
        assert!(
            engine
                .finish("browser", &args["state"], "code", &provider)
                .await
                .is_err()
        );
        assert_eq!(provider.requests.lock().unwrap().len(), 1);
    }
    for missing in ["iss", "sub", "aud", "iat", "exp", "nonce"] {
        let engine = protocol(true);
        let args = params(&engine.begin("browser").unwrap());
        let mut payload = claims(&args["nonce"]);
        payload.as_object_mut().unwrap().remove(missing);
        let provider = Provider::new(&token(&payload));
        assert!(
            engine
                .finish("browser", &args["state"], "code", &provider)
                .await
                .is_err(),
            "missing {missing} must fail"
        );
    }
    let attacker = PKey::from_rsa(Rsa::generate(2048).unwrap()).unwrap();
    for header in [
        json!({"alg":"RS256","kid":"unknown-key"}),
        json!({"alg":"HS256","kid":"synthetic-key"}),
        json!({"alg":"none"}),
        json!({"alg":"RS256","kid":"synthetic-key"}),
    ] {
        let engine = protocol(true);
        let args = params(&engine.begin("browser").unwrap());
        let mut jwt = signed(&claims(&args["nonce"]), &header, &attacker);
        if header["alg"] == "none" {
            jwt.truncate(jwt.rfind('.').unwrap() + 1);
        }
        let provider = Provider::new(&jwt);
        assert!(
            engine
                .finish("browser", &args["state"], "code", &provider)
                .await
                .is_err(),
            "bad signature/algorithm must fail"
        );
    }
}

#[tokio::test]
async fn malformed_provider_errors_and_invalid_codes_consume_transactions_without_secret_echo() {
    for (body, status) in [
        (
            b"{\"error\":\"invalid_grant\",\"error_description\":\"PrivateMarker\"}".to_vec(),
            400,
        ),
        (b"{".to_vec(), 200),
        (
            serde_json::to_vec(&json!({"access_token":ACCESS,"token_type":"Bearer"})).unwrap(),
            200,
        ),
    ] {
        let engine = protocol(true);
        let args = params(&engine.begin("browser").unwrap());
        let provider = Provider {
            body,
            status,
            requests: Mutex::new(vec![]),
        };
        let error = engine
            .finish("browser", &args["state"], "code", &provider)
            .await
            .err()
            .unwrap()
            .to_string();
        assert!(!error.contains("PrivateMarker"));
        assert!(
            engine
                .finish("browser", &args["state"], "code", &provider)
                .await
                .is_err()
        );
        assert_eq!(provider.requests.lock().unwrap().len(), 1);
    }
    for code in ["".to_string(), "s".repeat(4097), "code\n".to_string()] {
        let engine = protocol(true);
        let args = params(&engine.begin("browser").unwrap());
        let provider = Provider::new(&token(&claims(&args["nonce"])));
        assert!(
            engine
                .finish("browser", &args["state"], &code, &provider)
                .await
                .is_err()
        );
        assert!(provider.requests.lock().unwrap().is_empty());
    }
}

#[tokio::test]
async fn signing_key_locations_do_not_expand_trust_and_ambiguous_keys_fail_closed() {
    let engine = protocol(true);
    let args = params(&engine.begin("browser").unwrap());
    let jwt = signed(
        &claims(&args["nonce"]),
        &json!({
            "alg":"RS256", "kid":"synthetic-key", "jku":"https://unreachable.example.test/keys",
            "jwk":{"kty":"RSA","n":"untrusted","e":"AQAB"}
        }),
        key(),
    );
    let provider = Provider::new(&jwt);
    let _result = engine
        .finish("browser", &args["state"], "code", &provider)
        .await;
    assert_eq!(
        provider.requests.lock().unwrap().len(),
        1,
        "token header key URLs must never cause an extra request"
    );
    assert_eq!(
        provider.requests.lock().unwrap()[0].uri(),
        "https://identity.example.test/token"
    );
    let mut keys: Value = serde_json::from_slice(&jwks(key())).unwrap();
    let mut duplicate = keys["keys"][0].clone();
    duplicate["kid"] = json!("another-signing-key");
    keys["keys"].as_array_mut().unwrap().push(duplicate);
    let engine = Protocol::from_documents(
        &config(true),
        &discovery(),
        &serde_json::to_vec(&keys).unwrap(),
        None,
    )
    .unwrap();
    let args = params(&engine.begin("browser").unwrap());
    let jwt = signed(&claims(&args["nonce"]), &json!({"alg":"RS256"}), key());
    assert!(
        engine
            .finish("browser", &args["state"], "code", &Provider::new(&jwt))
            .await
            .is_err(),
        "a token without kid must not guess between signing keys"
    );
}

#[test]
fn preparation_and_pending_storage_are_bounded_and_require_matching_client_secrets() {
    assert!(BoundedHttp::new().is_ok());
    assert!(Protocol::from_documents(&config(false), &discovery(), &jwks(key()), None).is_err());
    for secret in ["".to_string(), "s".repeat(4097), "secret\n".into()] {
        assert!(
            Protocol::from_documents(
                &config(false),
                &discovery(),
                &jwks(key()),
                Some(ClientSecret::new(secret))
            )
            .is_err()
        );
    }
    assert!(
        Protocol::from_documents(
            &config(true),
            &discovery(),
            &jwks(key()),
            Some(ClientSecret::new("unexpected".into()))
        )
        .is_err()
    );
    for keys in [
        b"{".to_vec(),
        br#"{"keys":[]}"#.to_vec(),
        vec![b' '; MAX_DOCUMENT_BYTES + 1],
        jwks(&PKey::from_rsa(Rsa::generate(1024).unwrap()).unwrap()),
    ] {
        assert!(Protocol::from_documents(&config(true), &discovery(), &keys, None).is_err());
    }
    let mut many: Value = serde_json::from_slice(&jwks(key())).unwrap();
    many["keys"] = json!(vec![many["keys"][0].clone(); 33]);
    assert!(
        Protocol::from_documents(
            &config(true),
            &discovery(),
            &serde_json::to_vec(&many).unwrap(),
            None
        )
        .is_err()
    );
    let engine = protocol(true);
    for binding in ["".to_string(), "s".repeat(257), "binding\n".into()] {
        assert!(engine.begin(&binding).is_err());
    }
    for i in 0..MAX_PENDING {
        engine.begin(&format!("browser-{i}")).unwrap();
    }
    assert!(engine.begin("overflow").is_err());
    engine.begin("browser-0").unwrap();
    assert_eq!(engine.pending.lock().unwrap().len(), MAX_PENDING);
    engine
        .pending
        .lock()
        .unwrap()
        .values_mut()
        .next()
        .unwrap()
        .expires = Instant::now() - Duration::from_secs(1);
    engine.begin("after-expiry").unwrap();
    assert_eq!(engine.pending.lock().unwrap().len(), MAX_PENDING);
}

#[tokio::test]
async fn protocol_transport_is_verified_https_bounded_and_never_follows_redirects() {
    use crate::oidc::discovery::tests::{response, server};
    for (reply, trust, success) in [
        (
            response("200 OK", "Content-Type: application/json\r\n", b"{}"),
            true,
            true,
        ),
        (
            response("200 OK", "Content-Type: application/json\r\n", b"{}"),
            false,
            false,
        ),
        (
            response(
                "302 Found",
                "Location: https://unreachable.example.test/PrivateMarker\r\n",
                b"",
            ),
            true,
            false,
        ),
        (
            response("200 OK", "Content-Type: text/plain\r\n", b"{}"),
            true,
            false,
        ),
        (
            response(
                "200 OK",
                "Content-Type: application/json\r\nContent-Length: 262145\r\n",
                b"",
            ),
            true,
            false,
        ),
        (
            response(
                "200 OK",
                "Content-Type: application/json\r\n",
                &vec![b' '; MAX_DOCUMENT_BYTES + 1],
            ),
            true,
            false,
        ),
    ] {
        let (config, client, task) = server(move |_| reply, Duration::from_secs(10), trust);
        let http = BoundedHttp(client);
        let request = axum::http::Request::builder()
            .method("POST")
            .uri(format!("{}/token", config.issuer))
            .body(b"grant_type=authorization_code".to_vec())
            .unwrap();
        let result = http.call(request).await;
        assert_eq!(result.is_ok(), success);
        if let Err(error) = result {
            assert!(!error.to_string().contains("PrivateMarker"));
        }
        task.join().unwrap();
    }
    let http = BoundedHttp::new().unwrap();
    let request = axum::http::Request::builder()
        .uri("http://127.0.0.1:9/")
        .body(vec![])
        .unwrap();
    assert!(
        http.call(request).await.is_err(),
        "plaintext HTTP must never be attempted"
    );
}
