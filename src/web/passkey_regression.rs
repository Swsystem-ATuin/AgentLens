use super::*;
use super::{
    administration::{csrf, fixture, get},
    tests::login_cookie,
};
use crate::{mfa, passkeys};
use axum::{
    body::{Body, to_bytes},
    http::Request,
};
use tower::ServiceExt;
use webauthn_authenticator_rs::{WebauthnAuthenticator, softpasskey::SoftPasskey};
use webauthn_rs::prelude::{CreationChallengeResponse, RequestChallengeResponse, Url};
const ORIGIN: &str = "https://lens.example.test";
const PASSWORD: &str = "Synthetic1-password";
async fn json(
    app: &Router,
    cookie: &str,
    path: &str,
    value: serde_json::Value,
    origin: &str,
) -> Response {
    app.clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(path)
                .header(header::COOKIE, cookie)
                .header(header::ORIGIN, origin)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(serde_json::to_vec(&value).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap()
}
async fn value(response: Response) -> serde_json::Value {
    serde_json::from_slice(&to_bytes(response.into_body(), 256 * 1024).await.unwrap()).unwrap()
}
fn cookie(response: &Response) -> String {
    response.headers()[header::SET_COOKIE]
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .into()
}

#[tokio::test]
async fn browser_passkeys_are_disabled_until_configured_and_password_first_when_enabled() {
    let (dir, mut state, disabled) = fixture().await;
    let old_cookie = login_cookie(&disabled, "member", PASSWORD).await;
    let (_, page) = get(&disabled, &old_cookie, "/account/passkeys").await;
    assert!(page.contains("Passkeys are disabled"));
    assert_eq!(json(&disabled,&old_cookie,"/account/passkeys/register/start",serde_json::json!({"csrf_token":"disabled","current_password":PASSWORD,"name":"Synthetic"}),ORIGIN).await.status(),StatusCode::FORBIDDEN);
    state.auth.passkeys = passkeys::Runtime::configured(Some(ORIGIN)).unwrap();
    let app = session_router(
        crate::db::open(&dir.path().join("index.sqlite"))
            .await
            .unwrap(),
        state.clone(),
    );
    let (_, page) = get(&app, &old_cookie, "/account/passkeys").await;
    let token = csrf(&page);
    assert!(page.contains(ORIGIN));
    let start = serde_json::json!({"csrf_token":token,"current_password":PASSWORD,"name":"<b>Synthetic device</b>"});
    assert_eq!(
        json(
            &app,
            &old_cookie,
            "/account/passkeys/register/start",
            start.clone(),
            "https://evil.example.test"
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
    let mut wrong = start.clone();
    wrong["csrf_token"] = "wrong".into();
    assert_eq!(
        json(
            &app,
            &old_cookie,
            "/account/passkeys/register/start",
            wrong,
            ORIGIN
        )
        .await
        .status(),
        StatusCode::BAD_REQUEST
    );
    let mut wrong = start.clone();
    wrong["current_password"] = "wrong".into();
    assert_eq!(
        json(
            &app,
            &old_cookie,
            "/account/passkeys/register/start",
            wrong,
            ORIGIN
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
    let response = json(
        &app,
        &old_cookie,
        "/account/passkeys/register/start",
        start,
        ORIGIN,
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let challenge = value(response).await;
    let mut device = WebauthnAuthenticator::new(SoftPasskey::new(true));
    let options: CreationChallengeResponse =
        serde_json::from_value(challenge["options"].clone()).unwrap();
    let credential = device
        .do_registration(Url::parse(ORIGIN).unwrap(), options)
        .unwrap();
    let response = json(
        &app,
        &old_cookie,
        "/account/passkeys/register/finish",
        serde_json::json!({"csrf_token":token,"ticket":challenge["ticket"],"response":credential}),
        ORIGIN,
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let enrolled_cookie = cookie(&response);
    let codes = value(response).await["codes"].as_array().unwrap().clone();
    assert_eq!(codes.len(), 10);
    assert_eq!(
        get(&app, &old_cookie, "/account").await.0,
        StatusCode::UNAUTHORIZED
    );
    let (_, page) = get(&app, &enrolled_cookie, "/account/passkeys").await;
    assert!(page.contains("&#60;b&#62;Synthetic device&#60;/b&#62;"));
    assert!(!page.contains("<b>Synthetic device</b>"));
    assert!(!page.contains(codes[0].as_str().unwrap()));
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/login")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let anonymous = cookie(&response);
    let login_html = String::from_utf8(
        to_bytes(response.into_body(), 256 * 1024)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap();
    assert!(login_html.contains("data-passkey-login"));
    let token = csrf(&login_html);
    let response = json(
        &app,
        &anonymous,
        "/login/passkey/start",
        serde_json::json!({"csrf_token":token,"username":"member","password":"wrong"}),
        ORIGIN,
    )
    .await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    let response = json(
        &app,
        &anonymous,
        "/login/passkey/start",
        serde_json::json!({"csrf_token":token,"username":"member","password":PASSWORD}),
        ORIGIN,
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let challenge = value(response).await;
    let options: RequestChallengeResponse =
        serde_json::from_value(challenge["options"].clone()).unwrap();
    let assertion = device
        .do_authentication(Url::parse(ORIGIN).unwrap(), options)
        .unwrap();
    let response = json(
        &app,
        &anonymous,
        "/login/passkey/finish",
        serde_json::json!({"csrf_token":token,"ticket":challenge["ticket"],"response":assertion}),
        ORIGIN,
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let signed_cookie = cookie(&response);
    assert_eq!(
        get(&app, &signed_cookie, "/account").await.0,
        StatusCode::OK
    );
    let user = state
        .auth
        .authenticate("member", PASSWORD)
        .await
        .unwrap()
        .unwrap()
        .0;
    assert!(!mfa::verify(&state.auth, &user, "").await.unwrap());
    let (_, page) = get(&app, &signed_cookie, "/account/passkeys").await;
    let token = csrf(&page);
    let response = json(
        &app,
        &signed_cookie,
        "/account/passkeys/auth/start",
        serde_json::json!({"csrf_token":token,"current_password":PASSWORD}),
        ORIGIN,
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let challenge = value(response).await;
    let options = serde_json::from_value(challenge["options"].clone()).unwrap();
    let assertion = device
        .do_authentication(Url::parse(ORIGIN).unwrap(), options)
        .unwrap();
    let response = json(
        &app,
        &signed_cookie,
        "/account/passkeys/auth/finish",
        serde_json::json!({"csrf_token":token,"ticket":challenge["ticket"],"response":assertion}),
        ORIGIN,
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let proof = value(response).await["proof"].as_str().unwrap().to_owned();
    let key = passkeys::list(&state.auth, &user.id)
        .await
        .unwrap()
        .remove(0);
    let response=json(&app,&signed_cookie,"/account/passkeys/delete",serde_json::json!({"csrf_token":token,"current_password":PASSWORD,"mfa_code":proof,"id":key.id}),ORIGIN).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert!(!mfa::status(&state.auth, &user.id).await.unwrap().enrolled);
}
