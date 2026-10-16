use super::*;
use openssl::{
    asn1::Asn1Time,
    hash::MessageDigest,
    pkey::{PKey, Private},
    rsa::Rsa,
    ssl::{SslAcceptor, SslMethod},
    x509::{
        X509, X509NameBuilder,
        extension::{BasicConstraints, SubjectAlternativeName},
    },
};
use serde_json::{Value, json};
use std::{
    io::{Read, Write},
    net::TcpListener,
    sync::OnceLock,
    thread,
};

fn configuration(issuer: &str) -> Configuration {
    serde_json::from_value(json!({
        "issuer": issuer,
        "public_url": "https://lens.example.test",
        "client_id": "synthetic-client",
        "client_authentication": {"method": "client_secret_basic", "secret_file": "/nonexistent/secret"},
        "account_linking": "administrator_provisioned_issuer_subject",
        "permissions": "local_only",
        "logout": "rp_initiated_and_back_channel_required",
        "local_break_glass": true
    })).unwrap()
}

fn metadata(issuer: &str) -> Value {
    json!({
        "issuer": issuer,
        "authorization_endpoint": "https://identity.example.test/authorize",
        "token_endpoint": "https://identity.example.test/token",
        "jwks_uri": "https://keys.example.test/jwks",
        "end_session_endpoint": "https://identity.example.test/logout",
        "response_types_supported": ["code"],
        "subject_types_supported": ["pairwise"],
        "id_token_signing_alg_values_supported": ["RS256"],
        "code_challenge_methods_supported": ["S256"],
        "backchannel_logout_supported": true,
        "vendor_extension": {"ignored": true}
    })
}

fn validate(value: &Value) -> Result<()> {
    validate_metadata(
        &serde_json::to_vec(value)?,
        &configuration("https://identity.example.test/tenant"),
    )
}

#[test]
fn discovery_uses_issuer_path_and_removes_only_one_trailing_slash() {
    for (issuer, expected) in [
        (
            "https://identity.example.test",
            "https://identity.example.test/.well-known/openid-configuration",
        ),
        (
            "https://identity.example.test/",
            "https://identity.example.test/.well-known/openid-configuration",
        ),
        (
            "https://identity.example.test/tenant/",
            "https://identity.example.test/tenant/.well-known/openid-configuration",
        ),
        (
            "https://identity.example.test/tenant//",
            "https://identity.example.test/tenant//.well-known/openid-configuration",
        ),
    ] {
        assert_eq!(discovery_url(issuer), expected);
    }
}

#[test]
fn discovery_checks_capabilities_defaults_exact_issuer_and_unknown_extensions() {
    let base = metadata("https://identity.example.test/tenant");
    validate(&base).unwrap();
    let mut explicit = base.clone();
    explicit["grant_types_supported"] = json!(["authorization_code"]);
    explicit["response_modes_supported"] = json!(["query"]);
    explicit["scopes_supported"] = json!(["openid"]);
    explicit["token_endpoint_auth_methods_supported"] = json!(["none", "client_secret_basic"]);
    validate(&explicit).unwrap();
    for key in [
        "grant_types_supported",
        "response_modes_supported",
        "scopes_supported",
        "token_endpoint_auth_methods_supported",
    ] {
        for invalid_value in [json!(null), json!("not-an-array"), json!([true])] {
            let mut invalid = explicit.clone();
            invalid[key] = invalid_value;
            assert!(
                validate(&invalid).is_err(),
                "malformed {key} must reject discovery"
            );
        }
    }
    let mut public_client = configuration("https://identity.example.test/tenant");
    public_client.client_authentication = ClientAuthentication::None;
    assert!(
        validate_metadata(&serde_json::to_vec(&base).unwrap(), &public_client).is_err(),
        "default authentication is basic, not none"
    );
    validate_metadata(&serde_json::to_vec(&explicit).unwrap(), &public_client).unwrap();
    for (key, value) in [
        ("issuer", json!("https://identity.example.test/tenant/")),
        ("issuer", json!("https://IDENTITY.example.test/tenant")),
        ("issuer", json!("https://other.example.test/tenant")),
        ("response_types_supported", json!(["code id_token"])),
        ("grant_types_supported", json!(["implicit"])),
        ("response_modes_supported", json!(["fragment"])),
        ("scopes_supported", json!(["email"])),
        ("subject_types_supported", json!([])),
        (
            "id_token_signing_alg_values_supported",
            json!(["none", "HS256"]),
        ),
        ("code_challenge_methods_supported", json!(["plain"])),
        (
            "token_endpoint_auth_methods_supported",
            json!(["client_secret_post"]),
        ),
        ("backchannel_logout_supported", json!(false)),
        ("backchannel_logout_supported", json!("true")),
    ] {
        let mut value_copy = base.clone();
        value_copy[key] = value;
        assert!(
            validate(&value_copy).is_err(),
            "invalid {key} must reject discovery"
        );
    }
    for key in base
        .as_object()
        .unwrap()
        .keys()
        .filter(|key| key.as_str() != "vendor_extension")
    {
        let mut missing = base.clone();
        missing.as_object_mut().unwrap().remove(key);
        assert!(
            validate(&missing).is_err(),
            "missing {key} must reject discovery"
        );
    }
    for key in [
        "authorization_endpoint",
        "token_endpoint",
        "jwks_uri",
        "end_session_endpoint",
    ] {
        for url in [
            "http://identity.example.test/path",
            "https://user:secret@identity.example.test/path",
            "https://identity.example.test/path#fragment",
            "https://identity.example.test/path?query=1",
        ] {
            let mut invalid = base.clone();
            invalid[key] = json!(url);
            assert!(
                validate(&invalid).is_err(),
                "unsafe {key} must reject discovery"
            );
        }
    }
    for bytes in [
        b"{".as_slice(),
        b"null",
        b"[]",
        b"{\"issuer\":\"synthetic-private-marker\"}",
    ] {
        let error = validate_metadata(bytes, &public_client)
            .unwrap_err()
            .to_string();
        assert!(!error.contains("synthetic-private-marker"));
    }
}

fn certificate() -> &'static (PKey<Private>, X509) {
    static CERTIFICATE: OnceLock<(PKey<Private>, X509)> = OnceLock::new();
    CERTIFICATE.get_or_init(|| {
        let key = PKey::from_rsa(Rsa::generate(2048).unwrap()).unwrap();
        let mut name = X509NameBuilder::new().unwrap();
        name.append_entry_by_text("CN", "oidc.example.test")
            .unwrap();
        let name = name.build();
        let mut builder = X509::builder().unwrap();
        builder.set_version(2).unwrap();
        builder
            .set_serial_number(
                &openssl::bn::BigNum::from_u32(1)
                    .unwrap()
                    .to_asn1_integer()
                    .unwrap(),
            )
            .unwrap();
        builder.set_subject_name(&name).unwrap();
        builder.set_issuer_name(&name).unwrap();
        builder.set_pubkey(&key).unwrap();
        builder
            .set_not_before(&Asn1Time::days_from_now(0).unwrap())
            .unwrap();
        builder
            .set_not_after(&Asn1Time::days_from_now(1).unwrap())
            .unwrap();
        builder
            .append_extension(BasicConstraints::new().critical().ca().build().unwrap())
            .unwrap();
        let san = SubjectAlternativeName::new()
            .dns("oidc.example.test")
            .build(&builder.x509v3_context(None, None))
            .unwrap();
        builder.append_extension(san).unwrap();
        builder.sign(&key, MessageDigest::sha256()).unwrap();
        (key, builder.build())
    })
}

// Real TLS transport with a synthetic trust root, never a production certificate bypass.
pub(in crate::oidc) fn server(
    reply: impl FnOnce(&str) -> Vec<u8> + Send + 'static,
    timeout: Duration,
    trust: bool,
) -> (Configuration, Client, thread::JoinHandle<String>) {
    server_stream(
        move |issuer, stream| {
            let _ = stream.write_all(&reply(issuer));
        },
        timeout,
        trust,
    )
}

fn server_stream(
    reply: impl FnOnce(&str, &mut openssl::ssl::SslStream<std::net::TcpStream>) + Send + 'static,
    timeout: Duration,
    trust: bool,
) -> (Configuration, Client, thread::JoinHandle<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let issuer = format!("https://oidc.example.test:{}/tenant/", address.port());
    let config = configuration(&issuer);
    let (key, certificate) = certificate();
    let mut acceptor = SslAcceptor::mozilla_intermediate(SslMethod::tls()).unwrap();
    acceptor.set_private_key(key).unwrap();
    acceptor.set_certificate(certificate).unwrap();
    let acceptor = acceptor.build();
    let mut builder = client_builder(timeout).resolve("oidc.example.test", address);
    if trust {
        builder = builder.add_root_certificate(
            reqwest::Certificate::from_der(&certificate.to_der().unwrap()).unwrap(),
        );
    }
    let client = builder.build().unwrap();
    let task = thread::spawn(move || {
        let (socket, _) = listener.accept().unwrap();
        socket
            .set_read_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        socket
            .set_write_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        let Ok(mut stream) = acceptor.accept(socket) else {
            return String::new();
        };
        let mut request = Vec::new();
        while !request.ends_with(b"\r\n\r\n") && request.len() < 8192 {
            let mut byte = [0];
            if stream.read(&mut byte).unwrap_or(0) == 0 {
                break;
            }
            request.push(byte[0]);
        }
        reply(&issuer, &mut stream);
        String::from_utf8(request).unwrap()
    });
    (config, client, task)
}

pub(in crate::oidc) fn response(status: &str, headers: &str, body: &[u8]) -> Vec<u8> {
    let mut reply = format!("HTTP/1.1 {status}\r\nConnection: close\r\n{headers}\r\n").into_bytes();
    reply.extend_from_slice(body);
    reply
}

#[tokio::test]
async fn https_discovery_is_pinned_anonymous_and_does_not_fetch_advertised_endpoints() {
    let (config, client, server) = server(
        |issuer| {
            let body = serde_json::to_vec(&metadata(issuer)).unwrap();
            response(
                "200 OK",
                &format!(
                    "Content-Type: application/json; charset=utf-8\r\nContent-Length: {}\r\n",
                    body.len()
                ),
                &body,
            )
        },
        REQUEST_TIMEOUT,
        true,
    );
    fetch(&client, &config).await.unwrap();
    let request = server.join().unwrap().to_ascii_lowercase();
    assert!(request.starts_with("get /tenant/.well-known/openid-configuration http/1.1\r\n"));
    assert!(request.contains("accept: application/json\r\n"));
    assert!(!request.contains("authorization:"));
    assert!(!request.contains("cookie:"));
    assert!(!request.contains("synthetic-client"));
    assert!(!request.contains("nonexistent"));
}

#[tokio::test]
async fn https_discovery_rejects_untrusted_tls_redirects_types_status_and_size() {
    let (config, client, task) = server(
        |issuer| {
            response(
                "200 OK",
                "Content-Type: application/json\r\n",
                &serde_json::to_vec(&metadata(issuer)).unwrap(),
            )
        },
        REQUEST_TIMEOUT,
        false,
    );
    assert!(
        fetch(&client, &config).await.is_err(),
        "untrusted TLS must be rejected"
    );
    task.join().unwrap();
    for (reply, expected) in [
        (
            response(
                "302 Found",
                "Location: https://unreachable.example.test/synthetic-private-marker\r\n",
                b"",
            ),
            "requires HTTP 200",
        ),
        (
            response(
                "500 Error",
                "Content-Type: application/json\r\n",
                b"synthetic-private-marker",
            ),
            "requires HTTP 200",
        ),
        (
            response(
                "200 OK",
                "Content-Type: text/html\r\n",
                b"synthetic-private-marker",
            ),
            "requires application/json",
        ),
        (
            response(
                "200 OK",
                "Content-Type: application/json\r\nContent-Length: 262145\r\n",
                b"",
            ),
            "exceeds size limit",
        ),
        (
            response(
                "200 OK",
                "Content-Type: application/json\r\n",
                &vec![b' '; MAX_DOCUMENT_BYTES + 1],
            ),
            "exceeds size limit",
        ),
        (
            response(
                "200 OK",
                "Content-Type: application/json\r\n",
                b"{\"synthetic-private-marker\":",
            ),
            "invalid OIDC discovery metadata structure",
        ),
        (
            response(
                "200 OK",
                "Content-Type: application/json\r\nContent-Length: 100\r\n",
                b"{}",
            ),
            "read OIDC discovery response failed",
        ),
    ] {
        let (config, client, task) = server(move |_| reply, REQUEST_TIMEOUT, true);
        let error = fetch(&client, &config).await.unwrap_err().to_string();
        assert!(
            error.contains(expected),
            "expected {expected}, received {error}"
        );
        assert!(!error.contains("synthetic-private-marker"));
        task.join().unwrap();
    }
}

#[tokio::test]
async fn https_discovery_has_a_total_request_deadline() {
    let (config, client, task) = server_stream(
        |issuer, stream| {
            let body = serde_json::to_vec(&metadata(issuer)).unwrap();
            let _ = stream.write_all(&response(
                "200 OK",
                &format!(
                    "Content-Type: application/json\r\nContent-Length: {}\r\n",
                    body.len()
                ),
                &body[..1],
            ));
            thread::sleep(Duration::from_millis(600));
            let _ = stream.write_all(&body[1..]);
        },
        Duration::from_millis(300),
        true,
    );
    assert!(
        fetch(&client, &config).await.is_err(),
        "slow provider must time out"
    );
    task.join().unwrap();
}

#[tokio::test]
async fn https_discovery_accepts_limit_and_bounds_chunked_responses() {
    let (config, client, task) = server(
        |issuer| {
            let mut body = serde_json::to_vec(&metadata(issuer)).unwrap();
            body.resize(MAX_DOCUMENT_BYTES, b' ');
            response(
                "200 OK",
                &format!(
                    "Content-Type: application/json\r\nContent-Length: {}\r\n",
                    body.len()
                ),
                &body,
            )
        },
        REQUEST_TIMEOUT,
        true,
    );
    fetch(&client, &config).await.unwrap();
    task.join().unwrap();
    let (config, client, task) = server(
        |_| {
            let mut body = format!("{:x}\r\n", MAX_DOCUMENT_BYTES + 1).into_bytes();
            body.extend(vec![b' '; MAX_DOCUMENT_BYTES + 1]);
            body.extend_from_slice(b"\r\n0\r\n\r\n");
            response(
                "200 OK",
                "Content-Type: application/json\r\nTransfer-Encoding: chunked\r\n",
                &body,
            )
        },
        REQUEST_TIMEOUT,
        true,
    );
    let error = fetch(&client, &config).await.unwrap_err().to_string();
    assert!(
        error.contains("exceeds size limit"),
        "chunked responses must be bounded too: {error}"
    );
    task.join().unwrap();
}
