use super::*;
use axum::{
    body::{Body, to_bytes},
    http::{Request, header},
};
use base64::{Engine, engine::general_purpose::STANDARD};
use std::{
    io::Write,
    sync::{Arc, Mutex},
};
use tower::ServiceExt;

#[derive(Clone, Default)]
struct CapturedLogs(Arc<Mutex<Vec<u8>>>);

struct CapturedLogWriter(Arc<Mutex<Vec<u8>>>);

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for CapturedLogs {
    type Writer = CapturedLogWriter;

    fn make_writer(&'a self) -> Self::Writer {
        CapturedLogWriter(Arc::clone(&self.0))
    }
}

impl Write for CapturedLogWriter {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl CapturedLogs {
    fn text(&self) -> String {
        String::from_utf8(self.0.lock().unwrap().clone()).unwrap()
    }
}

async fn seeded_app() -> (tempfile::TempDir, Router) {
    let dir = tempfile::tempdir().unwrap();
    let pool = crate::db::open(&dir.path().join("index.sqlite"))
        .await
        .unwrap();
    sqlx::raw_sql(
        "
        INSERT INTO agents VALUES(1,'synthetic','Synthetic');
        INSERT INTO hosts VALUES(1,'host-a'),(2,'host-b');
        INSERT INTO users VALUES(1,1,'user-a'),(2,2,'user-b');
        INSERT INTO agent_instances VALUES(1,1,1,'synthetic-a'),(2,1,2,'synthetic-b');
        INSERT INTO sessions(id,instance_id,external_id,started_at,model,status,cwd) VALUES
          (1,1,'session-a','2026-01-01','model-a','complete','/synthetic/a'),
          (2,2,'session-b','2026-01-02','model-b','aborted','/synthetic/b'),
          (3,1,'session-c','2026-01-03','model-b','unknown','/synthetic/c');
        INSERT INTO messages(session_id,sequence,role,content) VALUES
          (1,3,'assistant','second reply'),
          (1,0,'developer','<multi_agent_role>synthetic setup</multi_agent_role>'),
          (1,4,'user','<script>synthetic</script>'),
          (1,2,'assistant','first reply'),
          (1,1,'user','synthetic request');
    ",
    )
    .execute(&pool)
    .await
    .unwrap();
    let auth = dir.path().join("auth.json");
    std::fs::write(&auth, r#"{"username":"synthetic","password":"password"}"#).unwrap();
    let app = router(pool, crate::auth::Credentials::load(&auth).unwrap());
    (dir, app)
}

async fn paginated_app() -> (tempfile::TempDir, Router) {
    let dir = tempfile::tempdir().unwrap();
    let pool = crate::db::open(&dir.path().join("index.sqlite"))
        .await
        .unwrap();
    sqlx::raw_sql(
        "
        INSERT INTO agents VALUES(1,'synthetic','Synthetic');
        INSERT INTO hosts VALUES(1,'host name');
        INSERT INTO users VALUES(1,1,'user/name');
        INSERT INTO agent_instances VALUES(1,1,1,'synthetic');
        ",
    )
    .execute(&pool)
    .await
    .unwrap();
    let mut transaction = pool.begin().await.unwrap();
    for id in 1..=105_i64 {
        sqlx::query(
            "INSERT INTO sessions(id,instance_id,external_id,started_at,model,status,cwd) \
             VALUES(?,1,?,'2026-01-01','model &/+','complete','/synthetic')",
        )
        .bind(id)
        .bind(format!("session-{id}"))
        .execute(&mut *transaction)
        .await
        .unwrap();
    }
    transaction.commit().await.unwrap();
    let auth = dir.path().join("auth.json");
    std::fs::write(&auth, r#"{"username":"synthetic","password":"password"}"#).unwrap();
    let app = router(pool, crate::auth::Credentials::load(&auth).unwrap());
    (dir, app)
}

async fn get(app: &Router, uri: &str) -> (StatusCode, String) {
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(uri)
                .header(
                    header::AUTHORIZATION,
                    format!("bAsIc {}", STANDARD.encode("synthetic:password")),
                )
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
    let status = response.status();
    let body = String::from_utf8(
        to_bytes(response.into_body(), 1024 * 1024)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap();
    (status, body)
}

#[tokio::test]
async fn filters_query_real_rows_individually_together_and_with_literal_input() {
    let (_dir, app) = seeded_app().await;
    for (query, expected) in [
        ("", vec![3, 2, 1]),
        ("?host=host-a", vec![3, 1]),
        ("?user=user-b", vec![2]),
        ("?model=model-b", vec![3, 2]),
        ("?status=complete", vec![1]),
        (
            "?host=host-a&user=user-a&model=model-b&status=unknown",
            vec![3],
        ),
        ("?host=host-a&user=user-b", vec![]),
        ("?host=&user=&model=&status=", vec![3, 2, 1]),
        ("?host=%27%20OR%201%3D1--", vec![]),
    ] {
        let (status, html) = get(&app, &format!("/sessions{query}")).await;
        assert_eq!(status, StatusCode::OK, "{query}");
        assert_eq!(
            html.matches("<a href=\"/sessions/").count(),
            expected.len(),
            "{query}"
        );
        let mut previous = None;
        for id in expected {
            let position = html.find(&format!("href=\"/sessions/{id}\"")).unwrap();
            assert!(
                previous.is_none_or(|last| last < position),
                "newest first: {query}"
            );
            previous = Some(position);
        }
    }
    let (_, html) = get(&app, "/sessions?model=model-b").await;
    assert!(html.contains("<option value=\"model-b\" selected>model-b</option>"));
    assert_eq!(html.matches("<option value=\"model-b\"").count(), 1);
    assert!(
        html.contains("<option value=\"model-a\""),
        "options remain available outside current filter"
    );
    assert!(html.contains("<td>host-b</td><td>user-b</td>"));
}

#[tokio::test]
async fn sessions_are_paginated_with_stable_order_and_filter_navigation() {
    let (_dir, app) = paginated_app().await;

    let (status, first) = get(&app, "/sessions").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(first.matches("<a href=\"/sessions/").count(), 50);
    assert!(
        first.len() < 64 * 1024,
        "response was {} bytes",
        first.len()
    );
    assert_session_order(&first, (56..=105).rev());
    assert!(!first.contains("href=\"/sessions/55\""));
    assert!(!first.contains("rel=\"prev\""));
    assert!(first.contains("rel=\"next\" href=\"/sessions?page=2\""));
    assert!(first.contains("Page 1"));

    let encoded_filter = "model=model%20%26%2F%2B";
    let (status, second) = get(&app, &format!("/sessions?{encoded_filter}&page=2")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(second.matches("<a href=\"/sessions/").count(), 50);
    assert_session_order(&second, (6..=55).rev());
    assert!(
        second.contains(&format!(
            "rel=\"prev\" href=\"/sessions?{encoded_filter}&#38;page=1\""
        )),
        "{second}"
    );
    assert!(second.contains(&format!(
        "rel=\"next\" href=\"/sessions?{encoded_filter}&#38;page=3\""
    )));
    assert!(second.contains("<option value=\"model &#38;/+\" selected>"));
    assert!(second.contains("Page 2"));

    let (status, third) = get(&app, &format!("/sessions?{encoded_filter}&page=3")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(third.matches("<a href=\"/sessions/").count(), 5);
    assert_session_order(&third, (1..=5).rev());
    assert!(third.contains("rel=\"prev\""));
    assert!(!third.contains("rel=\"next\""));
    assert!(third.contains("Page 3"));

    for page in ["0", "100001", "not-a-number"] {
        assert_eq!(
            get(&app, &format!("/sessions?page={page}")).await.0,
            StatusCode::BAD_REQUEST,
            "page={page}"
        );
    }
}

fn assert_session_order(html: &str, expected: impl IntoIterator<Item = i64>) {
    let mut previous = None;
    for id in expected {
        let position = html
            .find(&format!("href=\"/sessions/{id}\""))
            .unwrap_or_else(|| panic!("missing session {id}"));
        assert!(
            previous.is_none_or(|last| last < position),
            "session {id} was out of order"
        );
        previous = Some(position);
    }
}

#[tokio::test]
async fn transcript_route_orders_groups_collapses_and_escapes_stored_messages() {
    let (_dir, app) = seeded_app().await;
    let (status, html) = get(&app, "/sessions/1").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(html.matches("<details class=\"message").count(), 4);
    assert_eq!(html.matches(" open>").count(), 3);
    assert!(html.contains(
        "<details class=\"message developer\"><summary><strong>Multi-agent role</strong>"
    ));
    assert!(html.contains("<p class=\"transcript-paragraph\">first reply</p><p class=\"transcript-paragraph\">second reply</p>"));
    assert!(html.find("synthetic request").unwrap() < html.find("first reply").unwrap());
    let normalized = html
        .replace("&#60;", "&lt;")
        .replace("&#62;", "&gt;")
        .replace("&#47;", "/");
    assert!(
        normalized.contains("&lt;script&gt;synthetic&lt;/script&gt;"),
        "synthetic HTML: {html}"
    );
    assert!(!html.contains("<script>"));
    assert_eq!(get(&app, "/sessions/999").await.0, StatusCode::NOT_FOUND);
    assert_eq!(
        get(&app, "/sessions/invalid").await.0,
        StatusCode::BAD_REQUEST
    );
    let (status, html) = get(&app, "/sessions/2").await;
    assert_eq!(status, StatusCode::OK);
    assert!(!html.contains("<details"));
}

#[tokio::test]
async fn direct_codex_home_omits_host_and_user_from_lists_and_transcripts() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("single/.codex/sessions");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::write(
        home.join("direct.jsonl"),
        include_bytes!("../../tests/fixtures/codex/rollout.jsonl"),
    )
    .unwrap();
    let pool = crate::db::open(&dir.path().join("index.sqlite"))
        .await
        .unwrap();
    crate::ingest::ingest_data_root(&pool, &dir.path().join("single/.codex"))
        .await
        .unwrap();
    let id: i64 = sqlx::query_scalar("SELECT id FROM sessions")
        .fetch_one(&pool)
        .await
        .unwrap();
    let auth = dir.path().join("auth.json");
    std::fs::write(&auth, r#"{"username":"synthetic","password":"password"}"#).unwrap();
    let app = router(pool, crate::auth::Credentials::load(&auth).unwrap());

    let (_, list) = get(&app, "/sessions?host=ignored&user=ignored").await;
    assert!(!list.contains("aria-label=\"Host\""), "{list}");
    assert!(!list.contains("aria-label=\"User\""), "{list}");
    assert!(!list.contains("<th>Host</th>"), "{list}");
    assert!(!list.contains("<th>User</th>"), "{list}");
    assert!(list.contains(&format!("href=\"/sessions/{id}\"")), "{list}");
    let (_, transcript) = get(&app, &format!("/sessions/{id}")).await;
    assert!(!transcript.contains("<span> / </span>"), "{transcript}");
}

#[test]
fn every_setup_marker_and_notice_has_correct_rendered_state_after_resume() {
    for (prefix, title, open) in [
        (
            "# AGENTS.md instructions for /synthetic",
            "Repository instructions",
            false,
        ),
        ("<permissions instructions>", "Permissions", false),
        ("<environment_context>", "Environment context", false),
        ("<skills_instructions>", "Available skills", false),
        ("<multi_agent_mode>", "Multi-agent mode", false),
        ("<multi_agent_role>", "Multi-agent role", false),
        ("<collaboration_mode>", "Collaboration mode", false),
        ("<model_switch>", "Model switch", false),
        ("<turn_aborted>", "Turn interrupted", true),
        ("<user_action>", "User action", true),
    ] {
        let html = SessionTemplate {
            session: SessionRow {
                id: 1,
                external_id: "synthetic".into(),
                started_at: None,
                ended_at: None,
                model: None,
                status: None,
                cwd: None,
                hostname: "host".into(),
                os_user: "user".into(),
                message_count: 4,
            },
            segments: group_transcript(vec![
                message(0, "user", "request"),
                message(1, "assistant", "reply"),
                message(2, "user", &format!("  {prefix}\nsynthetic")),
                message(3, "assistant", "next reply"),
            ]),
            show_origin: true,
        }
        .render()
        .unwrap();
        let open_attribute = if open { " open" } else { "" };
        assert!(
            html.contains(&format!(
                "<details class=\"message user\"{open_attribute}><summary><strong>{title}</strong>"
            )),
            "{prefix}"
        );
        assert_eq!(
            html.matches("<details").count(),
            4,
            "setup must separate assistant groups: {prefix}"
        );
        assert_eq!(
            html.matches(" open>").count(),
            if open { 4 } else { 3 },
            "{prefix}"
        );
    }
}

fn message(sequence: i64, role: &str, content: &str) -> MessageRow {
    MessageRow {
        sequence,
        timestamp: None,
        role: role.into(),
        content: content.into(),
    }
}

#[tokio::test]
async fn refreshed_deletions_disappear_from_lists_and_direct_transcript_urls() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("sources/host/user/.codex/sessions");
    std::fs::create_dir_all(&source).unwrap();
    let path = source.join("synthetic.jsonl");
    std::fs::write(
        &path,
        include_bytes!("../../tests/fixtures/codex/rollout.jsonl"),
    )
    .unwrap();
    let pool = crate::db::open(&dir.path().join("index.sqlite"))
        .await
        .unwrap();
    crate::ingest::ingest_data_root(&pool, &dir.path().join("sources"))
        .await
        .unwrap();
    let id: i64 = sqlx::query_scalar("SELECT id FROM sessions")
        .fetch_one(&pool)
        .await
        .unwrap();
    let auth = dir.path().join("auth.json");
    std::fs::write(&auth, r#"{"username":"synthetic","password":"password"}"#).unwrap();
    let app = router(pool.clone(), crate::auth::Credentials::load(&auth).unwrap());
    assert_eq!(
        get(&app, &format!("/sessions/{id}")).await.0,
        StatusCode::OK
    );
    assert!(
        get(&app, "/sessions")
            .await
            .1
            .contains(&format!("href=\"/sessions/{id}\""))
    );
    std::fs::remove_file(path).unwrap();
    crate::ingest::ingest_data_root(&pool, &dir.path().join("sources"))
        .await
        .unwrap();
    assert_eq!(
        get(&app, &format!("/sessions/{id}")).await.0,
        StatusCode::NOT_FOUND
    );
    assert!(
        !get(&app, "/sessions")
            .await
            .1
            .contains(&format!("href=\"/sessions/{id}\""))
    );
}

#[tokio::test(flavor = "current_thread")]
async fn internal_errors_are_generic_and_server_diagnostics_are_sanitized() {
    let dir = tempfile::tempdir().unwrap();
    let pool = crate::db::open(&dir.path().join("index.sqlite"))
        .await
        .unwrap();
    pool.close().await;
    let auth = dir.path().join("auth.json");
    std::fs::write(
        &auth,
        r#"{"username":"synthetic","password":"private-login-secret"}"#,
    )
    .unwrap();
    let app = router(pool, crate::auth::Credentials::load(&auth).unwrap());
    let authorization = format!(
        "Basic {}",
        STANDARD.encode("synthetic:private-login-secret")
    );
    let logs = CapturedLogs::default();
    let subscriber = tracing_subscriber::fmt()
        .without_time()
        .with_ansi(false)
        .with_writer(logs.clone())
        .finish();
    let dispatch = tracing::Dispatch::new(subscriber);
    let _guard = tracing::dispatcher::set_default(&dispatch);

    let response = app
        .oneshot(
            Request::builder()
                .uri("/sessions?model=private-transcript-secret")
                .header(header::AUTHORIZATION, &authorization)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(
        to_bytes(response.into_body(), 1024).await.unwrap(),
        "Internal server error"
    );

    let response = WebError::database(
        "load synthetic record",
        sqlx::Error::Protocol("private-database-secret".into()),
    )
    .into_response();
    assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(
        to_bytes(response.into_body(), 1024).await.unwrap(),
        "Internal server error"
    );

    let logs = logs.text();
    assert!(logs.contains("load source layout"), "{logs}");
    assert!(logs.contains("pool_closed"), "{logs}");
    assert!(logs.contains("load synthetic record"), "{logs}");
    assert!(logs.contains("protocol"), "{logs}");
    for private in [
        "private-login-secret",
        "private-transcript-secret",
        "private-database-secret",
        &authorization,
    ] {
        assert!(!logs.contains(private), "logs disclosed {private}: {logs}");
    }
}

#[tokio::test]
async fn auto_review_models_are_hidden_from_lists_filters_and_direct_urls() {
    let (dir, app) = seeded_app().await;
    let pool = crate::db::open(&dir.path().join("index.sqlite"))
        .await
        .unwrap();
    for (id, model) in [
        (4, Some("codex-auto-review")),
        (5, Some("codex-auto-review-test")),
        (6, None),
        (7, Some("ordinary-codex-auto-review")),
    ] {
        sqlx::query("INSERT INTO sessions(id,instance_id,external_id,model) VALUES(?,1,?,?)")
            .bind(id)
            .bind(format!("synthetic-{id}"))
            .bind(model)
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO messages(session_id,sequence,role,content) VALUES(?,0,'assistant','synthetic conversation')")
            .bind(id).execute(&pool).await.unwrap();
    }
    for query in [
        "",
        "?model=codex-auto-review",
        "?model=codex-auto-review-test",
        "?host=host-a",
    ] {
        let (status, html) = get(&app, &format!("/sessions{query}")).await;
        assert_eq!(status, StatusCode::OK);
        for id in [4, 5] {
            assert!(
                !html.contains(&format!("href=\"/sessions/{id}\"")),
                "hidden session leaked for {query}"
            );
        }
        assert!(
            !html.contains("<option value=\"codex-auto-review"),
            "hidden model leaked into filter options"
        );
    }
    for id in [4, 5] {
        let (status, html) = get(&app, &format!("/sessions/{id}")).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert!(!html.contains("synthetic conversation"));
    }
    let (_, html) = get(&app, "/sessions").await;
    for id in [1, 2, 3, 6, 7] {
        assert!(
            html.contains(&format!("href=\"/sessions/{id}\"")),
            "ordinary or unknown model must remain visible"
        );
        assert_eq!(
            get(&app, &format!("/sessions/{id}")).await.0,
            StatusCode::OK
        );
    }
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM sessions")
            .fetch_one(&pool)
            .await
            .unwrap(),
        7,
        "visibility must not remove indexed data"
    );
}
