use super::*;

fn logout_claims() -> Value {
    let now = crate::auth::unix_now();
    json!({"iss":ISSUER,"aud":CLIENT_ID,"iat":now,"exp":now+300,"jti":"synthetic-private-jti",
        "sub":SUBJECT,"sid":"synthetic-private-sid","events":{"http://schemas.openid.net/event/backchannel-logout":{}}})
}
fn logout_token(claims: &Value) -> String {
    signed(
        claims,
        &json!({"alg":"RS256","typ":"logout+jwt","kid":"synthetic-key"}),
        key(),
    )
}

#[test]
fn backchannel_signed_targets_are_exact_bounded_and_replay_is_one_use() {
    for removed in [None, Some("sub"), Some("sid")] {
        let engine = protocol(true);
        let mut claims = logout_claims();
        if let Some(field) = removed {
            claims.as_object_mut().unwrap().remove(field);
        }
        claims["unknown_extension"] = json!({"ignored":true});
        let jwt = logout_token(&claims);
        let verified = engine.verify_logout(&jwt).unwrap();
        assert_eq!(verified.issuer, ISSUER);
        assert_eq!(verified.client_id, CLIENT_ID);
        assert_eq!(
            verified.subject.as_deref(),
            if removed == Some("sub") {
                None
            } else {
                Some(SUBJECT)
            }
        );
        assert_eq!(
            verified.session_id.as_deref(),
            if removed == Some("sid") {
                None
            } else {
                Some("synthetic-private-sid")
            }
        );
        assert_eq!(verified.expires, claims["exp"].as_i64().unwrap());
        assert_ne!(verified.replay_key, [0; 32]);
        assert!(engine.verify_logout(&jwt).is_err());
        assert!(
            protocol(true).verify_logout(&jwt).is_ok(),
            "the in-process guard intentionally does not claim restart durability"
        );
    }
}

#[test]
fn backchannel_rejects_claim_algorithm_signature_and_token_type_substitution_without_echo() {
    let now = crate::auth::unix_now();
    let mut invalid = Vec::new();
    for (field, value) in [
        ("iss", json!("https://IDENTITY.example.test/tenant")),
        ("aud", json!([CLIENT_ID, "other"])),
        ("aud", json!([])),
        ("azp", json!("other")),
        ("iat", json!(now + 120)),
        ("iat", json!(now - 700)),
        ("exp", json!(now)),
        ("exp", json!(now + 700)),
        ("exp", json!(i64::MIN)),
        ("iat", json!(i64::MAX)),
        ("nonce", json!(null)),
        ("nonce", json!("synthetic-private-nonce")),
        ("sid", json!(null)),
        ("sub", json!(null)),
        ("sub", json!("nonascii-é")),
        ("sid", json!("a".repeat(1025))),
        ("jti", json!("")),
        ("jti", json!("a".repeat(256))),
        ("jti", json!("private\n")),
        ("events", json!({})),
        (
            "events",
            json!({"http://schemas.openid.net/event/backchannel-logout":null}),
        ),
        (
            "events",
            json!({"http://schemas.openid.net/event/backchannel-logout":{"unexpected":true}}),
        ),
    ] {
        let mut claims = logout_claims();
        claims[field] = value;
        invalid.push(logout_token(&claims));
    }
    for field in ["iss", "aud", "iat", "exp", "jti", "events"] {
        let mut claims = logout_claims();
        claims.as_object_mut().unwrap().remove(field);
        invalid.push(logout_token(&claims));
    }
    let mut no_target = logout_claims();
    no_target.as_object_mut().unwrap().remove("sub");
    no_target.as_object_mut().unwrap().remove("sid");
    invalid.push(logout_token(&no_target));
    for header in [
        json!({"alg":"none","typ":"logout+jwt"}),
        json!({"alg":"HS256","typ":"logout+jwt"}),
        json!({"alg":"RS256","typ":"JWT"}),
        json!({"alg":"RS256"}),
        json!({"alg":"RS256","typ":"logout+jwt","crit":[]}),
        json!({"alg":"RS256","typ":"logout+jwt","b64":true}),
        json!({"alg":"RS256","typ":"logout+jwt","kid":"unknown"}),
    ] {
        invalid.push(signed(&logout_claims(), &header, key()));
    }
    let wrong = PKey::from_rsa(Rsa::generate(2048).unwrap()).unwrap();
    invalid.push(signed(
        &logout_claims(),
        &json!({"alg":"RS256","typ":"logout+jwt","kid":"synthetic-key"}),
        &wrong,
    ));
    invalid.extend([
        "".into(),
        "a.b.c.d.e".into(),
        "a".repeat(65537),
        "eyJhbGciOiJSUzI1NiJ9.bad.bad".into(),
    ]);
    let engine = protocol(true);
    for jwt in invalid {
        let error = engine
            .verify_logout(&jwt)
            .err()
            .expect("invalid logout token must fail");
        assert_eq!(error.to_string(), "OIDC back-channel logout token rejected");
    }
    engine
        .verify_logout(&logout_token(&logout_claims()))
        .unwrap();
}

#[test]
fn backchannel_ignores_embedded_key_urls_rejects_ambiguous_keys_and_serializes_replay() {
    let engine = std::sync::Arc::new(protocol(true));
    let jwt = signed(
        &logout_claims(),
        &json!({"alg":"RS256","typ":"logout+jwt","kid":"synthetic-key","jku":"https://untrusted.example.test/keys","jwk":{"kty":"oct","k":"ignored"}}),
        key(),
    );
    std::thread::scope(|scope| {
        let tasks: Vec<_> = (0..8)
            .map(|_| scope.spawn(|| engine.verify_logout(&jwt).is_ok()))
            .collect();
        assert_eq!(
            tasks
                .into_iter()
                .map(|task| task.join().unwrap())
                .filter(|accepted| *accepted)
                .count(),
            1
        );
    });
    let mut keys: Value = serde_json::from_slice(&jwks(key())).unwrap();
    let duplicate = keys["keys"][0].clone();
    keys["keys"].as_array_mut().unwrap().push(duplicate);
    let engine = Protocol::from_documents(
        &config(true),
        &discovery(),
        &serde_json::to_vec(&keys).unwrap(),
        None,
    )
    .unwrap();
    assert!(
        engine
            .verify_logout(&logout_token(&logout_claims()))
            .is_err()
    );
}
