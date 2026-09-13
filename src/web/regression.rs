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

async fn tool_timeline_app() -> (tempfile::TempDir, Router) {
    let dir = tempfile::tempdir().unwrap();
    let pool = crate::db::open(&dir.path().join("index.sqlite"))
        .await
        .unwrap();
    sqlx::raw_sql(
        r#"
        INSERT INTO agents VALUES(1,'codex','Synthetic Codex');
        INSERT INTO hosts VALUES(1,'host-a');
        INSERT INTO users VALUES(1,1,'user-a');
        INSERT INTO agent_instances(id,agent_id,user_id,source_home) VALUES(1,1,1,'synthetic-a');
        INSERT INTO sessions(id,instance_id,external_id,stable_id,title,started_at) VALUES
          (1,1,'timeline','timeline','Tool timeline','2026-01-01');
        INSERT INTO messages(session_id,sequence,role,content) VALUES
          (1,0,'user','initial ordinary conversation'),
          (1,9,'assistant','resumed ordinary conversation');
        INSERT INTO tool_calls(id,session_id,sequence,call_id,tool_name,arguments,status,kind) VALUES
          (1,1,1,'observed','shell','<unsafe> {not json','called','tool'),
          (2,1,3,'question','request_user_input','{"questions":[{"question":"Proceed?"}]}','called','user_input'),
          (3,1,5,'failed','runner','{"task":"synthetic"}','called','tool'),
          (4,1,7,'aborted','request_user_input','question without answer','called','user_input'),
          (5,1,10,'long','shell','placeholder','called','tool');
        INSERT INTO tool_results(session_id,source_sequence,timestamp,call_id,tool_call_id,output,outcome) VALUES
          (1,2,'2026-01-01T00:00:02Z','observed',1,'plain result','observed'),
          (1,4,'2026-01-01T00:00:04Z','question',2,'{"answers":{"Proceed?":"yes"}}','observed'),
          (1,6,'2026-01-01T00:00:06Z','failed',3,'{"is_error":true,"message":"failed safely"}','failed'),
          (1,8,'2026-01-01T00:00:08Z','orphan',NULL,'orphan output','observed'),
          (1,11,'2026-01-01T00:00:11Z','long',5,'placeholder','observed');
        "#,
    )
    .execute(&pool)
    .await
    .unwrap();
    let long_arguments = format!("{}ARGUMENT_TAIL", "a".repeat(20_000));
    let long_output = format!("{}OUTPUT_TAIL", "o".repeat(20_000));
    sqlx::query("UPDATE tool_calls SET arguments=? WHERE id=5")
        .bind(long_arguments)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("UPDATE tool_results SET output=? WHERE source_sequence=11")
        .bind(long_output)
        .execute(&pool)
        .await
        .unwrap();
    let auth = dir.path().join("auth.json");
    std::fs::write(&auth, r#"{"username":"synthetic","password":"password"}"#).unwrap();
    let app = router(pool, crate::auth::Credentials::load(&auth).unwrap());
    (dir, app)
}

async fn bounded_tool_timeline_app() -> (tempfile::TempDir, Router) {
    let dir = tempfile::tempdir().unwrap();
    let pool = crate::db::open(&dir.path().join("index.sqlite"))
        .await
        .unwrap();
    sqlx::raw_sql(
        "INSERT INTO agents VALUES(1,'codex','Synthetic Codex');
         INSERT INTO hosts VALUES(1,'host-a');
         INSERT INTO users VALUES(1,1,'user-a');
         INSERT INTO agent_instances(id,agent_id,user_id,source_home) VALUES(1,1,1,'synthetic-a');
         INSERT INTO sessions(id,instance_id,external_id,stable_id,title) VALUES(1,1,'bounded-tools','bounded-tools','Bounded tools');",
    )
    .execute(&pool)
    .await
    .unwrap();
    let mut transaction = pool.begin().await.unwrap();
    for index in 0..30_i64 {
        let call_id = format!("call-{index}");
        let call = sqlx::query(
            "INSERT INTO tool_calls(session_id,sequence,call_id,tool_name,arguments,status,kind) VALUES(1,?,?,?,?,'called','tool') RETURNING id",
        )
        .bind(index * 2)
        .bind(&call_id)
        .bind(format!("tool-{index}"))
        .bind(format!("request-{index}-{}", "x".repeat(5_000)))
        .fetch_one(&mut *transaction)
        .await
        .unwrap();
        let tool_call_id: i64 = sqlx::Row::get(&call, 0);
        sqlx::query(
            "INSERT INTO tool_results(session_id,source_sequence,call_id,tool_call_id,output,outcome) VALUES(1,?,?,?,?,'observed')",
        )
        .bind(index * 2 + 1)
        .bind(call_id)
        .bind(tool_call_id)
        .bind(format!("result-{index}-{}", "y".repeat(5_000)))
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

async fn usage_dashboard_app() -> (tempfile::TempDir, SqlitePool, Router) {
    let dir = tempfile::tempdir().unwrap();
    let pool = crate::db::open(&dir.path().join("index.sqlite"))
        .await
        .unwrap();
    sqlx::raw_sql(
        r#"
        INSERT INTO agents VALUES(1,'codex','Codex');
        INSERT INTO hosts VALUES(1,'alpha'),(2,'beta');
        INSERT INTO users VALUES(1,1,'one'),(2,2,'two');
        INSERT INTO agent_instances(id,agent_id,user_id,source_home) VALUES
          (1,1,1,'alpha-home'),(2,1,2,'beta-home');
        INSERT INTO sessions(id,instance_id,external_id,stable_id,title,cwd,model) VALUES
          (1,1,'one','usage-one','Usage one','/work/repo/component','model-a'),
          (2,2,'two','usage-two','Usage two','/elsewhere',NULL),
          (3,1,'three','usage-three','January usage','/work/repo','model-b'),
          (4,1,'hidden','usage-hidden','Hidden usage','/work/repo','codex-auto-review-private');
        INSERT INTO usage_events(session_id,source_sequence,sequence_known,timestamp,delta_input_tokens,delta_output_tokens,delta_cached_input_tokens,delta_reasoning_output_tokens,delta_total_tokens,cumulative_present,has_missing) VALUES
          (1,1,1,'2026-01-31T23:30:00Z',100,20,40,5,120,1,0),
          (1,2,1,'2026-02-02T10:00:00Z',50,10,10,NULL,60,1,0),
          (2,1,1,'2026-02-03T12:00:00Z',0,7,0,NULL,7,1,0),
          (2,2,1,NULL,999,999,999,NULL,1998,1,1),
          (3,1,1,'2026-01-30T10:00:00Z',25,5,5,1,30,1,0),
          (4,1,1,'2026-02-03T12:00:00Z',5000,5000,5000,5000,10000,1,0);
        "#,
    )
    .execute(&pool)
    .await
    .unwrap();
    let projects_path = dir.path().join("projects.json");
    std::fs::write(
        &projects_path,
        r#"{"timezone":"+01:00","projects":[{"name":"Fixture project","host":"alpha","root":"/work/repo"}]}"#,
    )
    .unwrap();
    let projects = crate::projects::ProjectConfig::load(Some(&projects_path)).unwrap();
    let auth = dir.path().join("auth.json");
    std::fs::write(&auth, r#"{"username":"synthetic","password":"password"}"#).unwrap();
    let app = router_with_projects(
        pool.clone(),
        crate::auth::Credentials::load(&auth).unwrap(),
        projects,
    );
    (dir, pool, app)
}

async fn turn_history_app() -> (tempfile::TempDir, SqlitePool, Router) {
    let dir = tempfile::tempdir().unwrap();
    let pool = crate::db::open(&dir.path().join("index.sqlite"))
        .await
        .unwrap();
    sqlx::raw_sql(
        r#"
        INSERT INTO agents VALUES(1,'codex','Codex');
        INSERT INTO hosts VALUES(1,'host');
        INSERT INTO users VALUES(1,1,'user');
        INSERT INTO agent_instances(id,agent_id,user_id,source_home) VALUES(1,1,1,'home');
        INSERT INTO sessions(id,instance_id,external_id,stable_id,title,model) VALUES
          (1,1,'turns','turn-history','Synthetic turn history','initial-model');
        INSERT INTO turns(session_id,ordinal,turn_id,start_sequence,start_timestamp,start_kind,end_sequence,end_timestamp,outcome,resumed,requested_model,requested_model_sequence,observed_model,observed_model_sequence) VALUES
          (1,1,'turn-one',1,'2026-01-01T00:00:00Z','requested_model',6,'2026-01-01T00:01:02Z','complete',0,'requested-a2',2,'served-b',3),
          (1,2,'turn-two',7,'2026-01-03T00:00:00Z','requested_model',9,'2026-01-02T00:00:00Z','interrupted',1,'requested-c',7,NULL,NULL),
          (1,3,NULL,10,'2026-01-04T00:00:00Z','user_activity',NULL,NULL,'unclosed',1,NULL,NULL,NULL,NULL),
          (1,4,NULL,NULL,NULL,'unknown',12,'2026-01-05T00:00:00Z','complete',0,NULL,NULL,NULL,NULL);
        INSERT INTO turn_events(session_id,turn_ordinal,source_sequence,timestamp,kind,turn_id,requested_model,served_model,is_resumption,is_model_change,raw_json) VALUES
          (1,1,1,'2026-01-01T00:00:00Z','requested_model','turn-one','requested-a',NULL,0,0,'{}'),
          (1,1,2,'2026-01-01T00:00:01Z','requested_model','turn-one','requested-a2',NULL,0,1,'{}'),
          (1,1,3,'2026-01-01T00:00:02Z','served_model_fallback','turn-one','requested-a2','served-b',0,0,'{}'),
          (1,1,6,'2026-01-01T00:01:02Z','completed','turn-one',NULL,NULL,0,0,'{}'),
          (1,2,7,'2026-01-03T00:00:00Z','requested_model','turn-two','requested-c',NULL,1,0,'{}'),
          (1,2,9,'2026-01-02T00:00:00Z','interrupted','turn-two',NULL,NULL,0,0,'{}'),
          (1,3,10,'2026-01-04T00:00:00Z','user_activity',NULL,NULL,NULL,1,0,'{}'),
          (1,4,12,'2026-01-05T00:00:00Z','completed',NULL,NULL,NULL,0,0,'{}');
        INSERT INTO tool_calls(id,session_id,sequence,call_id,tool_name,status,turn_ordinal) VALUES
          (1,1,3,'ok','one','called',1),(2,1,4,'failed','two','called',1),(3,1,8,'unknown','three','called',2);
        INSERT INTO tool_results(session_id,source_sequence,call_id,tool_call_id,outcome,turn_ordinal) VALUES
          (1,4,'ok',1,'observed',1),(1,5,'failed',2,'failed',1);
        "#,
    )
    .execute(&pool)
    .await
    .unwrap();
    let auth = dir.path().join("auth.json");
    std::fs::write(&auth, r#"{"username":"synthetic","password":"password"}"#).unwrap();
    let app = router(pool.clone(), crate::auth::Credentials::load(&auth).unwrap());
    (dir, pool, app)
}

async fn diagnostics_app() -> (tempfile::TempDir, SqlitePool, Router) {
    let dir = tempfile::tempdir().unwrap();
    let pool = crate::db::open(&dir.path().join("index.sqlite"))
        .await
        .unwrap();
    sqlx::raw_sql(
        r#"
        INSERT INTO agents VALUES(1,'codex','Codex');
        INSERT INTO hosts VALUES(1,'synthetic-host');
        INSERT INTO users VALUES(1,1,'synthetic-user');
        INSERT INTO agent_instances(id,agent_id,user_id,source_home) VALUES(1,1,1,'home');
        INSERT INTO sessions(id,instance_id,external_id,stable_id,title,model) VALUES
          (1,1,'diagnostics','diagnostics','Diagnostics <safe>','visible-model'),
          (2,1,'hidden','hidden-diagnostics','Hidden diagnostics','codex-auto-review-hidden');
        INSERT INTO turns(session_id,ordinal,start_sequence,start_timestamp,start_kind,end_sequence,end_timestamp,outcome) VALUES
          (1,1,1,'2020-01-01T00:00:00Z','started',2,'2020-01-01T00:01:00Z','interrupted'),
          (1,2,7,'2020-01-01T00:02:00Z','user_activity',NULL,NULL,'unclosed'),
          (2,1,1,'2020-01-01T00:00:00Z','started',2,'2020-01-01T00:01:00Z','interrupted');
        INSERT INTO turn_events(session_id,turn_ordinal,source_sequence,timestamp,kind) VALUES
          (1,1,1,'2020-01-01T00:00:00Z','started'),
          (1,1,2,'2020-01-01T00:01:00Z','interrupted'),
          (1,2,7,'2020-01-01T00:02:00Z','user_activity');
        INSERT INTO usage_events(session_id,source_sequence,sequence_known,timestamp,context_window,context_tokens,rate_limit_id,primary_used_percent,primary_window_minutes,primary_resets_at,primary_resets_in_seconds,secondary_used_percent,secondary_window_minutes,secondary_resets_at,has_inconsistent,raw_json) VALUES
          (1,3,1,'2020-01-01T00:01:10Z',100000,60000,'synthetic-scope',88.0,300,NULL,60,42.0,10080,4102444800,1,'{}'),
          (2,3,1,'2020-01-01T00:01:10Z',999999,999999,'hidden-scope',99.0,300,1,NULL,NULL,NULL,NULL,1,'{}');
        INSERT INTO usage_events(session_id,source_sequence,sequence_known,timestamp,rate_limit_name,primary_used_percent,primary_window_minutes,primary_resets_at,has_missing,raw_json) VALUES
          (1,4,1,datetime('now'),'recent-scope',25.0,300,unixepoch('now')+3600,1,'{}');
        INSERT INTO usage_events(session_id,source_sequence,sequence_known,timestamp,context_window,context_tokens,primary_used_percent,raw_json) VALUES
          (1,8,1,datetime('now'),100000,120000,120.0,'{}');
        INSERT INTO tool_calls(id,session_id,sequence,call_id,tool_name,status,kind) VALUES
          (1,1,5,'failed','synthetic','failed','tool'),(2,2,5,'hidden','synthetic','failed','tool');
        INSERT INTO tool_results(session_id,source_sequence,timestamp,call_id,tool_call_id,outcome) VALUES
          (1,6,'2020-01-01T00:01:30Z','failed',1,'failed'),(2,6,'2020-01-01T00:01:30Z','hidden',2,'failed');
        "#,
    )
    .execute(&pool)
    .await
    .unwrap();
    let auth = dir.path().join("auth.json");
    std::fs::write(&auth, r#"{"username":"synthetic","password":"password"}"#).unwrap();
    let app = router(pool.clone(), crate::auth::Credentials::load(&auth).unwrap());
    (dir, pool, app)
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
async fn usage_dashboard_uses_event_time_and_reconciles_grouped_detail() {
    let (_dir, pool, app) = usage_dashboard_app().await;
    let uri = "/usage?period=month&group=model&from=2026-02-01&to=2026-02-28";
    let (status, html) = get(&app, uri).await;
    assert_eq!(status, StatusCode::OK);
    assert!(html.contains("Historical usage"));
    assert!(html.contains("UTC+01:00"));
    assert!(html.contains("<strong>150</strong>"));
    assert!(html.contains("<strong>50</strong><small>33.3%</small>"));
    assert!(html.contains("<strong>37</strong>"));
    assert!(html.contains("<strong>5 (partial)</strong>"));
    assert!(html.contains("<strong>187</strong>"));
    assert!(html.contains("3 attributed observations across 2 sessions"));
    assert!(html.contains("1 observations have no usable timestamp"));
    assert!(html.contains("Unknown model"));
    assert!(html.contains("<td>0</td><td>0</td><td>Unavailable</td><td>7</td>"));
    assert!(html.contains("href=\"/sessions/usage-one\""));
    assert!(!html.contains("Hidden usage"));
    assert!(!html.contains("10,000"));

    let projects_path = _dir.path().join("projects.json");
    let projects = crate::projects::ProjectConfig::load(Some(&projects_path)).unwrap();
    let filters = UsageFilters {
        period: "month",
        group: "model",
        from: Some("2026-02-01"),
        to: Some("2026-02-28"),
    };
    let totals = queries::usage_totals(&pool, &filters, &projects)
        .await
        .unwrap();
    let detail = queries::usage_groups(&pool, &filters, &projects, 0)
        .await
        .unwrap();
    assert_eq!(
        (
            detail
                .iter()
                .filter_map(|row| row.input_tokens)
                .sum::<i64>(),
            detail
                .iter()
                .filter_map(|row| row.output_tokens)
                .sum::<i64>(),
            detail
                .iter()
                .filter_map(|row| row.cached_input_tokens)
                .sum::<i64>(),
            detail
                .iter()
                .filter_map(|row| row.reasoning_output_tokens)
                .sum::<i64>(),
            detail
                .iter()
                .filter_map(|row| row.total_tokens)
                .sum::<i64>(),
        ),
        (
            totals.input_tokens.unwrap(),
            totals.output_tokens.unwrap(),
            totals.cached_input_tokens.unwrap(),
            totals.reasoning_output_tokens.unwrap(),
            totals.total_tokens.unwrap(),
        ),
        "the complete grouped result must reconcile to the selected overview"
    );

    let (_, weekly) = get(&app, "/usage?period=week&group=session").await;
    assert!(weekly.contains("2026-01-26"));
    assert!(weekly.contains("2026-02-02"));
    assert!(weekly.contains("href=\"/sessions/usage-one\""));
    let (_, daily) = get(
        &app,
        "/usage?period=day&group=agent&from=2026-02-01&to=2026-02-01",
    )
    .await;
    assert!(daily.contains("<td>2026-02-01</td><td>codex</td>"));
    for (group, expected) in [
        ("project", "Fixture project"),
        ("host", "alpha"),
        ("user", "one"),
        ("agent", "codex"),
    ] {
        let (_, grouped) = get(
            &app,
            &format!("/usage?period=month&group={group}&from=2026-02-01&to=2026-02-28"),
        )
        .await;
        assert!(grouped.contains(expected), "group={group}: {grouped}");
        if group == "project" {
            assert!(grouped.contains("Unassigned project"));
        }
    }

    for uri in [
        "/usage?period=year",
        "/usage?group=invalid",
        "/usage?from=2026-02-30",
        "/usage?from=2026-03-01&to=2026-02-01",
        "/usage?page=0",
    ] {
        assert_eq!(get(&app, uri).await.0, StatusCode::BAD_REQUEST, "{uri}");
    }
}

#[tokio::test]
async fn usage_dashboard_bounds_rows_and_preserves_range_navigation() {
    let (_dir, pool, app) = usage_dashboard_app().await;
    let mut transaction = pool.begin().await.unwrap();
    for index in 0..=USAGE_PAGE_SIZE as i64 {
        let session_id = 100 + index;
        sqlx::query(
            "INSERT INTO sessions(id,instance_id,external_id,stable_id,title,model) VALUES(?,1,?,?,?,'model-page')",
        )
        .bind(session_id)
        .bind(format!("page-{index}"))
        .bind(format!("usage-page-{index}"))
        .bind(format!("Usage page {index}"))
        .execute(&mut *transaction)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO usage_events(session_id,source_sequence,sequence_known,timestamp,delta_input_tokens,delta_output_tokens,delta_cached_input_tokens,delta_total_tokens,cumulative_present) VALUES(?,1,1,'2026-02-10T12:00:00Z',?,?,?, ?,1)",
        )
        .bind(session_id)
        .bind(index + 1)
        .bind(index + 1)
        .bind(0_i64)
        .bind((index + 1) * 2)
        .execute(&mut *transaction)
        .await
        .unwrap();
    }
    transaction.commit().await.unwrap();

    let query = "period=day&group=session&from=2026-02-10&to=2026-02-10";
    let (status, first) = get(&app, &format!("/usage?{query}")).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        first.len() < 256 * 1024,
        "usage page was {} bytes",
        first.len()
    );
    assert!(first.contains(
        "rel=\"next\" href=\"/usage?period=day&#38;group=session&#38;from=2026-02-10&#38;to=2026-02-10&#38;page=2\""
    ));
    let (_, second) = get(&app, &format!("/usage?{query}&page=2")).await;
    assert!(second.contains("rel=\"prev\""));
    assert!(!second.contains("rel=\"next\""));
}

#[tokio::test]
async fn turn_history_renders_models_durations_tool_outcomes_and_evidence_links() {
    let (_dir, _pool, app) = turn_history_app().await;
    let (status, html) = get(&app, "/sessions/turn-history/turns").await;
    assert_eq!(status, StatusCode::OK);
    assert!(html.contains("Turn and model history"));
    assert!(html.contains("Turn 1 · complete"));
    assert!(html.contains("Duration 1m 2s"));
    assert!(html.contains("Requested: <a href="));
    assert!(html.contains(">requested-a2</a>"));
    assert!(html.contains("Observed served fallback: <a href="));
    assert!(html.contains(">served-b</a>"));
    assert!(
        html.contains("Tools: 2 requested · 2 results observed · 1 failed · 0 outcome unknown")
    );
    assert!(
        html.contains("Tools: 1 requested · 0 results observed · 0 failed · 1 outcome unknown")
    );
    assert!(html.contains("Turn 2 · interrupted <span class=\"timeline-state\">resumed</span>"));
    assert!(html.matches("Duration Unavailable").count() >= 3);
    assert!(html.contains("Turn 3 · unclosed"));
    assert!(html.contains("Terminal evidence unavailable"));
    assert!(html.contains("Opening evidence unavailable"));
    assert_eq!(html.matches(">Opening evidence</a>").count(), 3);
    assert_eq!(html.matches(">Terminal evidence</a>").count(), 3);
    assert!(html.contains("event-2-turns"));
    assert!(html.contains("event-9-turns"));

    let (status, timeline) = get(&app, "/sessions/turn-history?event=turns").await;
    assert_eq!(status, StatusCode::OK);
    for evidence in [
        "Requested model",
        "Requested model changed",
        "Observed served-model fallback",
        "Turn completed",
        "Turn interrupted",
        "Turn resumed",
    ] {
        assert!(timeline.contains(evidence), "{evidence}: {timeline}");
    }
    assert!(timeline.find("Requested model").unwrap() < timeline.find("Turn completed").unwrap());
    assert!(timeline.contains("Requested model: requested-a"));
    assert!(timeline.contains("Observed served model: served-b"));
    assert!(timeline.contains("Turn turn-one"));
    assert!(!timeline.contains("Tool request:"));
    assert!(!timeline.contains("Synthetic turn history</strong>"));

    assert_eq!(
        get(&app, "/sessions/turn-history/turns?page=0").await.0,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        get(&app, "/sessions/missing/turns").await.0,
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn turn_history_is_bounded_and_paginated() {
    let (_dir, pool, app) = turn_history_app().await;
    let mut transaction = pool.begin().await.unwrap();
    for ordinal in 5..=55_i64 {
        sqlx::query("INSERT INTO turns(session_id,ordinal,start_kind,outcome) VALUES(1,?,'unknown','unclosed')")
            .bind(ordinal)
            .execute(&mut *transaction)
            .await
            .unwrap();
    }
    transaction.commit().await.unwrap();
    let (status, first) = get(&app, "/sessions/turn-history/turns").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(first.matches("<article class=\"message turn\"").count(), 50);
    assert!(first.len() < 192 * 1024);
    assert!(first.contains("rel=\"next\" href=\"/sessions/turn-history/turns?page=2\""));
    let (_, second) = get(&app, "/sessions/turn-history/turns?page=2").await;
    assert_eq!(second.matches("<article class=\"message turn\"").count(), 5);
    assert!(second.contains("rel=\"prev\""));
    assert!(!second.contains("rel=\"next\""));
}

#[tokio::test]
async fn diagnostics_render_freshness_scope_summaries_and_evidence_links() {
    let (_dir, _pool, app) = diagnostics_app().await;
    let (status, html) = get(&app, "/diagnostics").await;
    assert_eq!(status, StatusCode::OK);
    assert!(html.contains("Observed pressure and diagnostics"));
    assert!(html.contains("not live subscription allowances or task-quality scores"));
    for (label, count) in [
        ("Interrupted turns", 1),
        ("Incomplete turns", 1),
        ("Accounting anomalies", 2),
        ("Observed tool failures", 1),
    ] {
        assert!(
            html.contains(&format!("{label}</span><strong>{count}</strong>")),
            "{label}: {html}"
        );
    }
    assert!(html.contains("60,000 / 100,000 (60.0%)"));
    assert!(html.contains("88.0% used · 300 minute window"));
    assert!(html.contains("reported reset in 60s"));
    assert!(html.contains("expired observation"));
    assert!(html.contains("stale observation"));
    assert!(html.contains("recent observation"));
    assert!(html.contains("inconsistent observation"));
    assert!(html.contains("synthetic-scope"));
    assert!(html.contains("recent-scope"));
    assert!(html.contains("synthetic-host"));
    assert!(html.contains("synthetic-user"));
    assert!(!html.contains("hidden-scope"));
    assert!(!html.contains("999,999"));
    assert!(html.contains("Diagnostics &#60;safe&#62;"));
    assert!(html.contains("event-2-turns"));
    assert!(html.contains("/sessions/diagnostics/turns?page=1#turn-2"));
    assert!(html.contains("event-3-diagnostics"));
    assert!(html.contains("event-6-result"));

    let (status, evidence) = get(&app, "/sessions/diagnostics?event=diagnostics").await;
    assert_eq!(status, StatusCode::OK);
    assert!(evidence.contains("Context and rate observation"));
    assert!(evidence.contains("Accounting flags: inconsistent"));
    assert!(evidence.contains("Accounting flags: missing"));
    assert!(
        evidence.find("event-3-diagnostics").unwrap()
            < evidence.find("event-4-diagnostics").unwrap()
    );
}

#[tokio::test]
async fn diagnostics_bound_snapshot_and_evidence_queries() {
    let (_dir, pool, app) = diagnostics_app().await;
    let mut transaction = pool.begin().await.unwrap();
    for index in 0..55_i64 {
        sqlx::query("INSERT INTO usage_events(session_id,source_sequence,sequence_known,timestamp,context_window,context_tokens,raw_json) VALUES(1,?,1,datetime('now'),100000,?, '{}')")
            .bind(100 + index)
            .bind(index)
            .execute(&mut *transaction)
            .await
            .unwrap();
    }
    for ordinal in 10..35_i64 {
        sqlx::query("INSERT INTO turns(session_id,ordinal,start_kind,end_sequence,outcome) VALUES(1,?,'unknown',?,'interrupted')")
            .bind(ordinal)
            .bind(200 + ordinal)
            .execute(&mut *transaction)
            .await
            .unwrap();
    }
    transaction.commit().await.unwrap();

    assert_eq!(queries::pressure_snapshots(&pool).await.unwrap().len(), 50);
    assert_eq!(
        queries::diagnostic_evidence(&pool, "interruption")
            .await
            .unwrap()
            .len(),
        20
    );
    let (status, html) = get(&app, "/diagnostics").await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        html.len() < 256 * 1024,
        "diagnostics page was {} bytes",
        html.len()
    );
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
    assert!(part_one.contains("href=\"/sessions/1?section=1#event-0-message\""));

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
    assert_eq!(html.matches("<strong>assistant</strong>").count(), 1);
    assert!(html.find("first reply").unwrap() < html.find("second reply").unwrap());
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
async fn tool_timeline_interleaves_filters_anchors_states_and_lazy_details() {
    let (_dir, app) = tool_timeline_app().await;
    let (status, html) = get(&app, "/sessions/timeline").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(html.matches("<details class=\"message").count(), 12);
    assert!(
        html.len() < 128 * 1024,
        "timeline response was {} bytes",
        html.len()
    );
    let ordered = [
        "initial ordinary conversation",
        "Tool request: shell",
        "plain result",
        "<strong>Question</strong>",
        "<strong>Answer</strong>",
        "Tool request: runner",
        "failed safely",
        "question without answer",
        "Orphan tool result",
        "resumed ordinary conversation",
    ];
    let mut last = 0;
    for value in ordered {
        let position = html
            .find(value)
            .unwrap_or_else(|| panic!("missing {value}"));
        assert!(position >= last, "{value} was out of source order");
        last = position;
    }
    assert!(html.contains("requested · output observed"));
    assert!(html.contains("requested · failed"));
    assert!(html.contains("requested · outcome unknown"));
    assert!(html.contains("id=\"event-3-tools\""));
    assert!(html.contains("id=\"event-4-results\""));
    assert!(html.contains("id=\"event-8-results\""));
    assert!(html.contains("Call question"));
    assert!(html.contains("&lt;unsafe&gt; {not json"));
    assert!(!html.contains("<unsafe>"));
    assert_eq!(html.matches("class=\"message user-input\" open").count(), 3);
    assert!(html.contains("href=\"/sessions/timeline/tools/10\""));
    assert!(html.contains("href=\"/sessions/timeline/results/11\""));
    assert!(!html.contains("ARGUMENT_TAIL"));
    assert!(!html.contains("OUTPUT_TAIL"));

    let (_, messages) = get(&app, "/sessions/timeline?event=messages").await;
    assert_eq!(messages.matches("<details class=\"message").count(), 2);
    assert!(!messages.contains("Tool request:"));
    let (_, tools) = get(&app, "/sessions/timeline?event=tools").await;
    assert_eq!(tools.matches("<details class=\"message").count(), 7);
    assert!(!tools.contains("<strong>Question</strong>"));
    assert!(!tools.contains("initial ordinary conversation"));
    let (_, questions) = get(&app, "/sessions/timeline?event=questions").await;
    assert_eq!(questions.matches("<details class=\"message").count(), 3);
    assert!(questions.contains("<strong>Question</strong>"));
    assert!(questions.contains("<strong>Answer</strong>"));
    assert!(!questions.contains("Tool request: shell"));
    assert_eq!(
        get(&app, "/sessions/timeline?event=invalid").await.0,
        StatusCode::BAD_REQUEST
    );

    let (_, call_one) = get(&app, "/sessions/timeline/tools/10").await;
    assert!(call_one.len() < 128 * 1024);
    assert!(!call_one.contains("ARGUMENT_TAIL"));
    assert!(call_one.contains("rel=\"next\" href=\"/sessions/timeline/tools/10?part=2\""));
    assert!(call_one.contains("href=\"/sessions/timeline?section=1#event-10-tools\""));
    let (_, call_two) = get(&app, "/sessions/timeline/tools/10?part=2").await;
    assert!(call_two.contains("ARGUMENT_TAIL"));
    assert!(!call_two.contains("rel=\"next\""));

    let (_, result_one) = get(&app, "/sessions/timeline/results/11").await;
    assert!(!result_one.contains("OUTPUT_TAIL"));
    assert!(result_one.contains("rel=\"next\" href=\"/sessions/timeline/results/11?part=2\""));
    let (_, result_two) = get(&app, "/sessions/timeline/results/11?part=2").await;
    assert!(result_two.contains("OUTPUT_TAIL"));
    for path in [
        "/sessions/timeline/tools/10?part=0",
        "/sessions/timeline/tools/10?part=3",
        "/sessions/timeline/results/99",
        "/sessions/other/tools/10",
    ] {
        let expected = if path.contains("part=0") || path.contains("part=3") {
            StatusCode::BAD_REQUEST
        } else {
            StatusCode::NOT_FOUND
        };
        assert_eq!(get(&app, path).await.0, expected, "{path}");
    }
}

#[tokio::test]
async fn tool_timeline_pages_and_previews_have_measured_bounds() {
    let (_dir, app) = bounded_tool_timeline_app().await;
    let (status, first) = get(&app, "/sessions/bounded-tools?event=tools").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(first.matches("<details class=\"message").count(), 50);
    assert!(
        first.len() < 160 * 1024,
        "first timeline page was {} bytes",
        first.len()
    );
    assert!(first.contains("request-0-"));
    assert!(first.contains("result-24-"));
    assert!(!first.contains("request-25-"));
    assert!(first.contains("section=2&#38;event=tools"));

    let (status, second) = get(&app, "/sessions/bounded-tools?section=2&event=tools").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(second.matches("<details class=\"message").count(), 10);
    assert!(
        second.len() < 48 * 1024,
        "second timeline page was {} bytes",
        second.len()
    );
    assert!(second.contains("request-25-"));
    assert!(second.contains("result-29-"));
    assert!(!second.contains("request-24-"));
    assert!(second.contains("section=1&#38;event=tools"));
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
    let (_, usage) = get(&app, "/usage?group=host").await;
    assert!(!usage.contains("<option value=\"host\""), "{usage}");
    assert!(!usage.contains("<option value=\"user\""), "{usage}");
    assert!(
        usage.contains("<option value=\"session\" selected>session</option>"),
        "{usage}"
    );
    let (_, diagnostics) = get(&app, "/diagnostics").await;
    assert!(!diagnostics.contains("<th>Host</th>"), "{diagnostics}");
    assert!(!diagnostics.contains("<th>User</th>"), "{diagnostics}");
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
            event_options: vec![],
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
