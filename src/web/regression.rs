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
        INSERT INTO agents VALUES(1,'codex','Synthetic Codex');
        INSERT INTO hosts VALUES(1,'host-a'),(2,'host-b');
        INSERT INTO users VALUES(1,1,'user-a'),(2,2,'user-b');
        INSERT INTO agent_instances(id,agent_id,user_id,source_home) VALUES(1,1,1,'synthetic-a'),(2,1,2,'synthetic-b');
        INSERT INTO sessions(id,instance_id,external_id,stable_id,title,started_at,model,status,cwd) VALUES
          (1,1,'session-a','1','Session A','2026-01-01','model-a','complete','/synthetic/a'),
          (2,2,'session-b','2','Session B','2026-01-02','model-b','aborted','/synthetic/b'),
          (3,1,'session-c','3','Session C','2026-01-03','model-b','unknown','/synthetic/c');
        INSERT INTO messages(session_id,sequence,role,content) VALUES
          (1,3,'assistant','second reply'),
          (1,0,'developer','<multi_agent_role>ordinarymatch setup</multi_agent_role>'),
          (1,4,'user','**bold** <script>needle & unsafe</script> [bad](javascript:alert(1)) ![remote](https://example.invalid/pixel.png)

| A | B |
| - | - |
| one | two |

```rust
fn demo() {}
```'),
          (1,2,'assistant','first reply'),
          (1,1,'user','synthetic request ordinarymatch');
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

async fn navigation_app() -> (tempfile::TempDir, Router) {
    let dir = tempfile::tempdir().unwrap();
    let pool = crate::db::open(&dir.path().join("index.sqlite"))
        .await
        .unwrap();
    sqlx::raw_sql(
        r#"
        INSERT INTO agents VALUES(1,'codex','Codex'),(2,'other-agent','Other agent');
        INSERT INTO hosts VALUES(1,'linux'),(2,'windows');
        INSERT INTO users VALUES(1,1,'user'),(2,2,'user');
        INSERT INTO agent_instances(id,agent_id,user_id,source_home,source_key) VALUES
          (1,1,1,'linux-home','source-linux'),(2,2,2,'windows-home','source-windows');
        INSERT INTO ingest_runs(id,finished_at,status,source_layout) VALUES(1,CURRENT_TIMESTAMP,'complete','host-user');
        INSERT INTO source_files(id,instance_id,ingest_run_id,path,kind,parse_status) VALUES
          (1,1,1,'active-one','sessions','ok'),
          (2,1,1,'archived-two','archived_sessions','ok'),
          (3,2,1,'active-three','sessions','ok'),
          (4,2,1,'active-four','sessions','ok'),
          (5,1,1,'active-offset','sessions','ok');
        INSERT INTO sessions(id,instance_id,source_file_id,external_id,stable_id,title,started_at,ended_at,cwd,model,status) VALUES
          (1,1,1,'one','session-one','Root work','2026-01-01T22:00:00Z','2026-01-01T23:30:00Z','/work/mono/docs','model-a','complete'),
          (2,1,2,'two','session-two','API work','2026-01-02T10:00:00Z',NULL,'/work/mono/services/api/src','model-a','complete'),
          (3,2,3,'three','session-z','Windows newest Z','2026-01-03T12:00:00Z',NULL,'C:\Work\Repo\Apps\UI\src','model-b','unknown'),
          (4,2,4,'four','session-a','Windows newest A','2026-01-03T12:00:00Z',NULL,'c:/work/repo/other','model-b','unknown'),
          (5,2,NULL,'five','session-missing','Missing metadata',NULL,NULL,NULL,NULL,NULL),
          (6,1,5,'six','session-offset','Offset timestamp','2026-01-03T13:00:00+02:00',NULL,'/work/mono/offset','model-a','complete');
        "#,
    )
    .execute(&pool)
    .await
    .unwrap();
    let projects_path = dir.path().join("projects.json");
    std::fs::write(
        &projects_path,
        r#"{
          "timezone":"+01:00",
          "projects":[
            {"name":"Mono","host":"linux","root":"/work/mono","subprojects":[{"name":"API","path":"services/api"}]},
            {"name":"Windows Repo","host":"windows","root":"C:\\Work\\Repo","subprojects":[{"name":"UI","path":"Apps\\UI"}]}
          ]
        }"#,
    )
    .unwrap();
    let projects = crate::projects::ProjectConfig::load(Some(&projects_path)).unwrap();
    let auth = dir.path().join("auth.json");
    std::fs::write(&auth, r#"{"username":"synthetic","password":"password"}"#).unwrap();
    let app = router_with_projects(
        pool,
        crate::auth::Credentials::load(&auth).unwrap(),
        projects,
    );
    (dir, app)
}

async fn paginated_app() -> (tempfile::TempDir, Router) {
    let dir = tempfile::tempdir().unwrap();
    let pool = crate::db::open(&dir.path().join("index.sqlite"))
        .await
        .unwrap();
    sqlx::raw_sql(
        "
        INSERT INTO agents VALUES(1,'codex','Synthetic Codex');
        INSERT INTO hosts VALUES(1,'host name');
        INSERT INTO users VALUES(1,1,'user/name');
        INSERT INTO agent_instances(id,agent_id,user_id,source_home) VALUES(1,1,1,'synthetic');
        ",
    )
    .execute(&pool)
    .await
    .unwrap();
    let mut transaction = pool.begin().await.unwrap();
    for id in 1..=105_i64 {
        sqlx::query(
            "INSERT INTO sessions(id,instance_id,external_id,stable_id,title,started_at,model,status,cwd) \
             VALUES(?,1,?,?,?,'2026-01-01','model &/+','complete','/synthetic')",
        )
        .bind(id)
        .bind(format!("session-{id}"))
        .bind(format!("{id:03}"))
        .bind(format!("Session {id}"))
        .execute(&mut *transaction)
        .await
        .unwrap();
        sqlx::query("INSERT INTO messages(session_id,sequence,role,content) VALUES(?,0,'user',?)")
            .bind(id)
            .bind(format!("sharedterm bounded result {id}"))
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

async fn bounded_transcript_app() -> (tempfile::TempDir, Router) {
    let dir = tempfile::tempdir().unwrap();
    let pool = crate::db::open(&dir.path().join("index.sqlite"))
        .await
        .unwrap();
    sqlx::raw_sql(
        "
        INSERT INTO agents VALUES(1,'codex','Synthetic Codex');
        INSERT INTO hosts VALUES(1,'host');
        INSERT INTO users VALUES(1,1,'user');
        INSERT INTO agent_instances(id,agent_id,user_id,source_home) VALUES(1,1,1,'synthetic');
        INSERT INTO sessions(id,instance_id,external_id,stable_id,title,started_at,model,status,cwd)
          VALUES(1,1,'long-session','1','Long session','2026-01-01','model','complete','/synthetic');
        ",
    )
    .execute(&pool)
    .await
    .unwrap();
    let mut transaction = pool.begin().await.unwrap();
    for sequence in 0..=105_i64 {
        let role = if sequence == 0 {
            "developer"
        } else if sequence % 2 == 0 {
            "user"
        } else {
            "assistant"
        };
        let content = if sequence == 0 {
            format!("<skills_instructions>needle {}TAIL", "&".repeat(20_000))
        } else if sequence == 50 {
            "**message-50**".to_owned()
        } else {
            format!("message-{sequence}")
        };
        sqlx::query("INSERT INTO messages(session_id,sequence,role,content) VALUES(1,?,?,?)")
            .bind(sequence)
            .bind(role)
            .bind(content)
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
    assert!(
        response
            .headers()
            .contains_key(header::CONTENT_SECURITY_POLICY)
    );
    assert_eq!(response.headers()[header::REFERRER_POLICY], "no-referrer");
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
async fn projects_dates_archives_and_agents_filter_rendered_sessions() {
    let (_dir, app) = navigation_app().await;
    let (status, all) = get(&app, "/sessions").await;
    assert_eq!(status, StatusCode::OK);
    assert!(all.contains("<td>Mono</td>"));
    assert!(all.contains("<td>Mono / API</td>"));
    assert!(all.contains("<td>Windows Repo / UI</td>"));
    assert!(all.contains("<option value=\"Mono / API\">Mono / API</option>"));
    assert!(all.contains("Missing metadata"));
    assert!(all.contains("Dates in UTC+01:00"));
    assert!(all.find("/sessions/session-z").unwrap() < all.find("/sessions/session-a").unwrap());
    assert!(all.find("/sessions/session-a").unwrap() < all.find("/sessions/session-two").unwrap());
    assert!(
        all.find("/sessions/session-a").unwrap() < all.find("/sessions/session-offset").unwrap(),
        "sorting must compare timestamp instants rather than raw offsets"
    );
    assert!(
        all.contains("2026-01-01T23:30:00Z"),
        "last activity must render"
    );

    for (query, expected, absent) in [
        ("?project=Mono", "session-one", "session-two"),
        ("?project=Mono%20%2F%20API", "session-two", "session-one"),
        ("?archive=archived", "session-two", "session-one"),
        (
            "?agent=other-agent&archive=active",
            "session-z",
            "session-one",
        ),
        (
            "?host=windows&project=Windows%20Repo%20%2F%20UI&agent=other-agent&archive=active",
            "session-z",
            "session-a",
        ),
        ("?q=UI", "session-z", "session-a"),
        ("?from=2026-01-02&to=2026-01-02", "session-two", "session-z"),
    ] {
        let (status, html) = get(&app, &format!("/sessions{query}")).await;
        assert_eq!(status, StatusCode::OK, "{query}");
        assert!(
            html.contains(&format!("/sessions/{expected}")),
            "{query}: {html}"
        );
        assert!(
            !html.contains(&format!("/sessions/{absent}")),
            "{query}: {html}"
        );
    }
    let (_, local_boundary) = get(&app, "/sessions?from=2026-01-02&to=2026-01-02").await;
    assert!(
        local_boundary.contains("/sessions/session-one"),
        "23:30Z belongs to the next date at UTC+01:00"
    );
    assert!(!local_boundary.contains("Missing metadata"));

    let (_, empty) = get(&app, "/sessions?project=does-not-exist").await;
    assert!(empty.contains("No sessions match these filters."));
    for query in [
        "?from=2026-02-30",
        "?to=not-a-date",
        "?from=2026-02-02&to=2026-02-01",
    ] {
        assert_eq!(
            get(&app, &format!("/sessions{query}")).await.0,
            StatusCode::BAD_REQUEST,
            "{query}"
        );
    }

    let (status, transcript) = get(&app, "/sessions/session-z").await;
    assert_eq!(status, StatusCode::OK);
    assert!(transcript.contains("Windows Repo / UI"));
    assert!(transcript.contains("other-agent"));
    assert!(transcript.contains("active"));
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
    assert!(!first.contains("href=\"/sessions/055\""));
    assert!(!first.contains("rel=\"prev\""));
    assert!(first.contains("rel=\"next\" href=\"/sessions?page=2\""));
    assert!(first.contains("Page 1"));

    let encoded_filter = "host=host%20name&user=user%2Fname&model=model%20%26%2F%2B&agent=codex&archive=unknown&from=2026-01-01&to=2026-01-01";
    let escaped_filter = encoded_filter.replace('&', "&#38;");
    let (status, second) = get(&app, &format!("/sessions?{encoded_filter}&page=2")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(second.matches("<a href=\"/sessions/").count(), 50);
    assert_session_order(&second, (6..=55).rev());
    assert!(
        second.contains(&format!(
            "rel=\"prev\" href=\"/sessions?{escaped_filter}&#38;page=1\""
        )),
        "{second}"
    );
    assert!(second.contains(&format!(
        "rel=\"next\" href=\"/sessions?{escaped_filter}&#38;page=3\""
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

#[tokio::test]
async fn search_is_safe_composable_and_links_to_an_ordinary_matching_message() {
    let (_dir, app) = seeded_app().await;

    for (query, expected, absent) in [
        ("Session%20B", "/sessions/2", "/sessions/1"),
        ("session-c", "/sessions/3", "/sessions/2"),
        ("ordinarymatch", "/sessions/1", "/sessions/2"),
    ] {
        let (status, html) = get(&app, &format!("/sessions?q={query}")).await;
        assert_eq!(status, StatusCode::OK, "{query}");
        assert!(html.contains(expected), "{query}: {html}");
        assert!(!html.contains(absent), "{query}: {html}");
    }

    let (_, ordinary) = get(&app, "/sessions?q=ordinarymatch").await;
    assert!(
        ordinary.contains("href=\"/sessions/1/messages/1\""),
        "{ordinary}"
    );
    assert!(
        !ordinary.contains("/messages/0"),
        "setup match obscured conversation"
    );

    let (_, unsafe_html) = get(&app, "/sessions?q=needle").await;
    let normalized = unsafe_html
        .replace("&#60;", "&lt;")
        .replace("&#62;", "&gt;")
        .replace("&#38;", "&amp;");
    assert!(normalized.contains("&lt;script&gt;<mark>needle</mark> &amp; unsafe"));
    assert!(!unsafe_html.contains("<script>"));
    assert!(unsafe_html.contains("href=\"/sessions/1/messages/4\""));

    let (_, combined) = get(&app, "/sessions?q=ordinarymatch&status=complete").await;
    assert!(combined.contains("/sessions/1"));
    let (_, excluded) = get(&app, "/sessions?q=ordinarymatch&status=aborted").await;
    assert!(excluded.contains("No sessions match these filters."));

    for query in ["%22", "%27%20OR%201%3D1--", "foo-bar%2Fbaz", "%E2%9C%93"] {
        assert_eq!(
            get(&app, &format!("/sessions?q={query}")).await.0,
            StatusCode::OK,
            "special query {query}"
        );
    }
    let long_query = "x".repeat(crate::search::MAX_QUERY_CHARS + 1);
    assert_eq!(
        get(&app, &format!("/sessions?q={long_query}")).await.0,
        StatusCode::BAD_REQUEST
    );
}

#[tokio::test]
async fn transcript_search_large_results_keep_page_and_response_bounded() {
    let (_dir, app) = paginated_app().await;
    let (_, first) = get(&app, "/sessions?q=sharedterm").await;
    assert_eq!(first.matches("class=\"search-preview\"").count(), 50);
    assert!(
        first.len() < 96 * 1024,
        "response was {} bytes",
        first.len()
    );
    assert!(first.contains("rel=\"next\" href=\"/sessions?q=sharedterm&#38;page=2\""));

    let (_, second) = get(&app, "/sessions?q=sharedterm&page=2").await;
    assert_eq!(second.matches("class=\"search-preview\"").count(), 50);
    assert!(second.contains("rel=\"prev\" href=\"/sessions?q=sharedterm&#38;page=1\""));
    assert!(second.contains("rel=\"next\" href=\"/sessions?q=sharedterm&#38;page=3\""));
}

fn assert_session_order(html: &str, expected: impl IntoIterator<Item = i64>) {
    let mut previous = None;
    for id in expected {
        let position = html
            .find(&format!("href=\"/sessions/{id:03}\""))
            .unwrap_or_else(|| panic!("missing session {id}"));
        assert!(
            previous.is_none_or(|last| last < position),
            "session {id} was out of order"
        );
        previous = Some(position);
    }
}

#[tokio::test]
async fn long_transcripts_use_bounded_sections_and_lazy_message_parts() {
    let (_dir, app) = bounded_transcript_app().await;
    let (_, search) = get(&app, "/sessions?q=needle").await;
    assert!(
        search.len() < 64 * 1024,
        "search response was {} bytes",
        search.len()
    );

    let (status, first) = get(&app, "/sessions/1").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(first.matches("<details class=\"message").count(), 50);
    assert!(
        first.len() < 128 * 1024,
        "response was {} bytes",
        first.len()
    );
    assert!(first.contains("message-49"));
    assert!(!first.contains("message-50"));
    assert!(!first.contains("TAIL"));
    assert!(first.contains(
        "<details class=\"message developer\"><summary><strong>Available skills</strong>"
    ));
    assert!(first.contains("href=\"/sessions/1/messages/0\""));
    assert!(first.contains("rel=\"next\" href=\"/sessions/1?section=2\""));
    assert!(!first.contains("rel=\"prev\""));

    let (status, second) = get(&app, "/sessions/1?section=2").await;
    assert_eq!(status, StatusCode::OK);
    assert!(second.find("message-50").unwrap() < second.find("message-99").unwrap());
    assert!(second.contains("<strong>message-50</strong>"));
    assert!(!second.contains("message-100"));
    assert!(second.contains("rel=\"prev\" href=\"/sessions/1?section=1\""));
    assert!(second.contains("rel=\"next\" href=\"/sessions/1?section=3\""));

    let (status, third) = get(&app, "/sessions/1?section=3").await;
    assert_eq!(status, StatusCode::OK);
    assert!(third.find("message-100").unwrap() < third.find("message-105").unwrap());
    assert!(!third.contains("rel=\"next\""));

    let (status, part_one) = get(&app, "/sessions/1/messages/0").await;
    assert_eq!(status, StatusCode::OK);
    assert!(part_one.len() < 128 * 1024);
    assert!(!part_one.contains("TAIL"));
    assert!(part_one.contains("rel=\"next\" href=\"/sessions/1/messages/0?part=2\""));
    assert!(part_one.contains("href=\"/sessions/1?section=1\""));

    let (status, part_two) = get(&app, "/sessions/1/messages/0?part=2").await;
    assert_eq!(status, StatusCode::OK);
    assert!(part_two.len() < 128 * 1024);
    assert!(part_two.contains("TAIL"));
    assert!(part_two.contains("rel=\"prev\""));
    assert!(!part_two.contains("rel=\"next\""));

    for path in [
        "/sessions/1?section=0",
        "/sessions/1?section=100001",
        "/sessions/1?section=invalid",
        "/sessions/1/messages/0?part=0",
        "/sessions/1/messages/0?part=3",
        "/sessions/1/messages/0?part=invalid",
    ] {
        assert_eq!(get(&app, path).await.0, StatusCode::BAD_REQUEST, "{path}");
    }
    assert_eq!(
        get(&app, "/sessions/1/messages/999").await.0,
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn transcript_route_orders_groups_collapses_and_escapes_stored_messages() {
    let (_dir, app) = seeded_app().await;
    let (status, html) = get(&app, "/sessions/1").await;
    assert_eq!(status, StatusCode::OK);
    assert!(html.contains("<h1>Session A</h1>"));
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
        .replace("&#47;", "/")
        .replace("&#38;", "&amp;");
    assert!(
        normalized.contains("&lt;script&gt;needle &amp; unsafe&lt;/script&gt;"),
        "synthetic HTML: {html}"
    );
    assert!(!html.contains("<script>"));
    assert!(html.contains("<strong>bold</strong>"));
    assert!(html.contains("<table>"));
    assert!(html.contains("<code class=\"language-rust\">fn demo() {}"));
    assert!(!html.contains("<img"));
    assert!(!html.contains("href=\"javascript:"));
    let (_, message_html) = get(&app, "/sessions/1/messages/4").await;
    assert!(message_html.contains("<strong>bold</strong>"));
    assert!(message_html.contains("<table>"));
    assert!(!message_html.contains("<img"));
    assert!(!message_html.contains("href=\"javascript:"));
    assert_eq!(get(&app, "/sessions/999").await.0, StatusCode::NOT_FOUND);
    assert_eq!(
        get(&app, "/sessions/invalid").await.0,
        StatusCode::NOT_FOUND
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
    let id: String = sqlx::query_scalar("SELECT stable_id FROM sessions")
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
    assert_eq!(
        get(&app, "/sessions/1").await.0,
        StatusCode::NOT_FOUND,
        "rebuildable SQLite row IDs must not be public routes"
    );
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
                stable_id: "stable-session".into(),
                agent_kind: "codex".into(),
                external_id: "synthetic".into(),
                title: "Synthetic title".into(),
                started_at: None,
                last_activity: None,
                model: None,
                status: None,
                cwd: None,
                project: None,
                archive_state: "unknown".into(),
                hostname: "host".into(),
                os_user: "user".into(),
                message_count: 4,
                match_sequence: None,
                match_snippet: None,
            },
            segments: group_transcript(vec![
                message(0, "user", "request"),
                message(1, "assistant", "reply"),
                message(2, "user", &format!("  {prefix}\nsynthetic")),
                message(3, "assistant", "next reply"),
            ]),
            show_origin: true,
            pagination: no_pagination(),
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
        content_truncated: false,
    }
}

fn no_pagination() -> Pagination {
    Pagination {
        page: 1,
        previous_href: None,
        next_href: None,
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
    let searchable: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM message_search WHERE message_search MATCH 'redacted'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(searchable, 2);
    let id: String = sqlx::query_scalar("SELECT stable_id FROM sessions")
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
    let searchable: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM message_search WHERE message_search MATCH 'redacted'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(searchable, 0, "cascade deletion must update transcript FTS");
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
        sqlx::query("INSERT INTO sessions(id,instance_id,external_id,stable_id,title,model) VALUES(?,1,?,?,?,?)")
            .bind(id)
            .bind(format!("synthetic-{id}"))
            .bind(id.to_string())
            .bind(format!("Synthetic {id}"))
            .bind(model)
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO messages(session_id,sequence,role,content) VALUES(?,0,'assistant',?)",
        )
        .bind(id)
        .bind(format!("synthetic conversation private-hidden-{id}"))
        .execute(&pool)
        .await
        .unwrap();
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
    let (_, hidden_search) = get(&app, "/sessions?q=private-hidden-4").await;
    assert!(hidden_search.contains("No sessions match these filters."));
    assert!(!hidden_search.contains("href=\"/sessions/4\""));
    assert!(!hidden_search.contains("class=\"search-preview\""));
    sqlx::raw_sql(
        "
        INSERT INTO agents VALUES(2,'other-agent','Other agent');
        INSERT INTO agent_instances(id,agent_id,user_id,source_home) VALUES(3,2,1,'other-agent-home');
        INSERT INTO sessions(id,instance_id,external_id,stable_id,title,model)
          VALUES(8,3,'other-session','8','Other session','codex-auto-review');
        ",
    )
    .execute(&pool)
    .await
    .unwrap();
    let (_, html) = get(&app, "/sessions").await;
    for id in [1, 2, 3, 6, 7, 8] {
        assert!(
            html.contains(&format!("href=\"/sessions/{id}\"")),
            "ordinary or unknown model must remain visible"
        );
        assert_eq!(
            get(&app, &format!("/sessions/{id}")).await.0,
            StatusCode::OK
        );
    }
    let (_, filtered) = get(&app, "/sessions?model=codex-auto-review").await;
    assert!(filtered.contains("href=\"/sessions/8\""));
    assert!(filtered.contains("<option value=\"codex-auto-review\" selected>"));
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM sessions")
            .fetch_one(&pool)
            .await
            .unwrap(),
        8,
        "visibility must not remove indexed data"
    );
}
