//! Attachments become text: spreadsheets, images and audio read through the real job pipeline (Gmail download
//! mocked at Nango), stored with their kind and status, searchable, shown on the message, fed to the insight,
//! and kept off the AI path for old mail, spent budgets and provider errors. Also the OpenRouter request shapes.

use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

use async_trait::async_trait;
use axum::{
    Json, Router,
    body::{Body, Bytes},
    extract::{Path, State},
    http::{HeaderMap, Request, StatusCode},
    routing::post,
};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use email_ai_api::{
    ai::{AiError, Analyst, Insight, openrouter::OpenRouterClient},
    mail::attachments::{
        audio::{AudioSummary, OpenRouterAudio, StubAudio, read_audio},
        vision::{OpenRouterVision, StubVision, VisionReader, VisionReading},
    },
    build_state,
    platform::config::Config,
    platform::db::{self, new_id, now_ms},
    platform::domain::{AttachmentMeta, NormalizedEmail},
    http::{AppState, router},
    mail::ingest,
    jobs::worker,
};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use tower::ServiceExt;

const ADMIN: &str = "dev:admin@acme.test";
const DAY_MS: i64 = 86_400_000;

fn xlsx() -> Vec<u8> {
    let mut book = rust_xlsxwriter::Workbook::new();
    let sheet = book.add_worksheet();
    sheet.set_name("Sales").unwrap();
    sheet.write_string(0, 0, "Region").unwrap();
    sheet.write_string(0, 1, "Revenue").unwrap();
    sheet.write_string(1, 0, "Zanzibar").unwrap();
    sheet.write_number(1, 1, 1200.0).unwrap();
    book.save_to_buffer().unwrap()
}

fn png() -> Vec<u8> {
    let mut b = b"\x89PNG\r\n\x1a\n".to_vec();
    b.extend_from_slice(&[7u8; 64]);
    b
}

/// Mono 8-bit 8 kHz silence of `seconds` (8,000 bytes per second).
fn wav(seconds: usize) -> Vec<u8> {
    let data = vec![128u8; 8_000 * seconds];
    let mut out = b"RIFF".to_vec();
    out.extend_from_slice(&((36 + data.len()) as u32).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    for v in [1u16, 1] {
        out.extend_from_slice(&v.to_le_bytes());
    }
    out.extend_from_slice(&8_000u32.to_le_bytes());
    out.extend_from_slice(&8_000u32.to_le_bytes());
    for v in [1u16, 8] {
        out.extend_from_slice(&v.to_le_bytes());
    }
    out.extend_from_slice(b"data");
    out.extend_from_slice(&(data.len() as u32).to_le_bytes());
    out.extend_from_slice(&data);
    out
}

type Files = Arc<HashMap<String, Vec<u8>>>;

/// Nango proxy stand-in serving Gmail attachment bytes by attachment id.
async fn spawn_nango(files: Files) -> String {
    let app = Router::new()
        .route(
            "/proxy/gmail/v1/users/me/messages/{mid}/attachments/{aid}",
            axum::routing::get(|State(files): State<Files>, Path((_, aid)): Path<(String, String)>| async move {
                match files.get(&aid) {
                    Some(bytes) => (StatusCode::OK, Json(json!({ "data": URL_SAFE_NO_PAD.encode(bytes), "size": bytes.len() }))),
                    None => (StatusCode::NOT_FOUND, Json(json!({}))),
                }
            }),
        )
        .with_state(files);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    format!("http://{addr}")
}

#[derive(Default)]
struct RecordingAnalyst(Mutex<Vec<String>>);

#[async_trait]
impl Analyst for RecordingAnalyst {
    async fn analyze(&self, subject: &str, _sender: &str, body: &str) -> Result<(Insight, String), AiError> {
        self.0.lock().unwrap().push(body.to_string());
        Ok((Insight { summary: format!("About {subject}"), needs_reply: true, ..Default::default() }, "rules-v1".into()))
    }
}

struct H {
    state: AppState,
    app: Router,
    ws: String,
    source: String,
    vision: Arc<StubVision>,
    audio: Arc<StubAudio>,
    analyst: Arc<RecordingAnalyst>,
    _dir: tempfile::TempDir,
}

fn reading() -> VisionReading {
    VisionReading {
        kind: "receipt".into(),
        text: "Total 42 Mumbai Cafe".into(),
        description: "A cafe receipt".into(),
        fields: [("total".to_string(), Value::String("42".into()))].into_iter().collect(),
    }
}

async fn harness_with(vision: StubVision, tweak: impl FnOnce(&mut Config)) -> H {
    let dir = tempfile::tempdir().unwrap();
    let files: Files = Arc::new(HashMap::from([
        ("att-sheet".to_string(), xlsx()),
        ("att-img".to_string(), png()),
        ("att-wav".to_string(), wav(10)),
        ("att-fake".to_string(), b"<svg onload=alert(1)>".to_vec()),
    ]));
    let mut config = Config::for_tests(&format!("sqlite://{}", dir.path().join("t.db").display()));
    config.nango_api_url = spawn_nango(files).await;
    config.nango_api_key = "k".into();
    tweak(&mut config);
    let mut state = build_state(config).await.unwrap();
    db::migrate(&state.db).await.unwrap();
    let vision = Arc::new(vision);
    let audio = Arc::new(StubAudio::new(
        vec!["please send the contract"],
        AudioSummary { summary: "Caller wants the contract".into(), action_items: vec!["Send the contract".into()] },
    ));
    let analyst = Arc::new(RecordingAnalyst::default());
    state.vision = Some(vision.clone());
    state.audio = Some(audio.clone());
    state.analyst = analyst.clone();
    let app = router(state.clone());
    let (_, ws) = call(&app, "POST", "/api/v1/workspaces", None).await;
    let ws = ws["id"].as_str().unwrap().to_string();
    let source = new_id("src");
    sqlx::query(
        "INSERT INTO email_sources (id, workspace_id, kind, status, external_connection_id, provider_config_key, created_at)
         VALUES (?1, ?2, 'nango_gmail', 'connected', 'conn1', 'google-mail', ?3)",
    )
    .bind(&source)
    .bind(&ws)
    .bind(now_ms())
    .execute(&state.db)
    .await
    .unwrap();
    H { state, app, ws, source, vision, audio, analyst, _dir: dir }
}

async fn harness() -> H {
    harness_with(StubVision::new(reading()), |_| {}).await
}

async fn call(app: &Router, method: &str, path: &str, body: Option<Value>) -> (StatusCode, Value) {
    let body = if path == "/api/v1/workspaces" { Some(json!({"name": "Acme"})) } else { body };
    let req = Request::builder().method(method).uri(path).header("x-auth-token", ADMIN);
    let req = match body {
        Some(b) => req.header("content-type", "application/json").body(Body::from(b.to_string())).unwrap(),
        None => req.body(Body::empty()).unwrap(),
    };
    let res = app.clone().oneshot(req).await.unwrap();
    let status = res.status();
    let bytes = res.into_body().collect().await.unwrap().to_bytes();
    (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
}

/// `extract_kind, extract_status, extract_error, text_content` of one attachment.
type Row = (Option<String>, Option<String>, Option<String>, Option<String>);

#[derive(sqlx::FromRow)]
struct NamedRow {
    filename: String,
    extract_kind: Option<String>,
    extract_status: Option<String>,
    extract_error: Option<String>,
    text_content: Option<String>,
}

impl H {
    /// Stores a Gmail message with the given `(filename, media type, provider ref)` attachments, as sync would.
    async fn message(&self, subject: &str, age_ms: i64, files: &[(&str, &str, &str)]) -> String {
        let email = NormalizedEmail {
            source_message_id: new_id("gm"),
            sender_address: "vendor@client.example".into(),
            recipients: vec!["me@acme.com".into()],
            subject: subject.into(),
            body_text: "Please see the attached files.".into(),
            received_at: now_ms() - age_ms,
            attachments: files
                .iter()
                .map(|(name, mt, r)| AttachmentMeta {
                    filename: name.to_string(),
                    media_type: Some(mt.to_string()),
                    size_bytes: Some(1_000),
                    provider_ref: Some(r.to_string()),
                })
                .collect(),
            ..Default::default()
        };
        let mut tx = self.state.db.begin().await.unwrap();
        let id = ingest::persist_message(&mut tx, &self.source, &self.ws, &email, Some("thread-1"), json!({})).await.unwrap().unwrap();
        tx.commit().await.unwrap();
        worker::drain(&self.state).await;
        id
    }

    async fn rows(&self, message_id: &str) -> HashMap<String, Row> {
        let rows: Vec<(String, Row)> = sqlx::query_as::<_, NamedRow>(
            "SELECT filename, extract_kind, extract_status, extract_error, text_content FROM attachments WHERE message_id = ?1",
        )
        .bind(message_id)
        .fetch_all(&self.state.db)
        .await
        .unwrap()
        .into_iter()
        .map(|r| (r.filename, (r.extract_kind, r.extract_status, r.extract_error, r.text_content)))
        .collect();
        rows.into_iter().collect()
    }

    async fn detail(&self, id: &str) -> Value {
        call(&self.app, "GET", &format!("/api/v1/workspaces/{}/messages/{id}", self.ws), None).await.1
    }
}

const ALL: [(&str, &str, &str); 4] = [
    ("budget.xlsx", "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet", "att-sheet"),
    ("receipt.png", "image/png", "att-img"),
    ("voicemail.wav", "audio/wav", "att-wav"),
    ("legacy.doc", "application/msword", "att-doc"),
];

#[tokio::test]
async fn every_attachment_becomes_text_that_search_insight_and_the_ui_can_use() {
    let h = harness().await;
    let id = h.message("Q3 numbers", 0, &ALL).await;

    let rows = h.rows(&id).await;
    let (kind, status, _, text) = &rows["budget.xlsx"];
    assert_eq!((kind.as_deref(), status.as_deref()), (Some("sheet"), Some("done")));
    assert!(text.as_deref().unwrap().contains("| Zanzibar | 1200 |"));
    assert_eq!(rows["receipt.png"].1.as_deref(), Some("done"));
    let scans: HashMap<String, String> = sqlx::query_as::<_, (String, String)>("SELECT filename, scan_status FROM attachments WHERE message_id = ?1")
        .bind(&id)
        .fetch_all(&h.state.db)
        .await
        .unwrap()
        .into_iter()
        .collect();
    assert_eq!((scans["budget.xlsx"].as_str(), scans["receipt.png"].as_str(), scans["legacy.doc"].as_str()), ("done", "done", "skipped"));
    assert!(rows["receipt.png"].3.as_deref().unwrap().contains("Total 42 Mumbai Cafe"));
    assert!(rows["voicemail.wav"].3.as_deref().unwrap().contains("Action items:\n- Send the contract"));
    assert_eq!((rows["legacy.doc"].0.as_deref(), rows["legacy.doc"].1.as_deref()), (Some("unsupported"), Some("skipped")));
    let ts: String = sqlx::query_scalar("SELECT text_status FROM attachments WHERE filename = 'budget.xlsx'").fetch_one(&h.state.db).await.unwrap();
    assert_eq!(ts, "extracted", "matching and the knowledge index read the same text");

    assert_eq!(h.vision.calls.lock().unwrap().as_slice(), [("image/png".to_string(), png().len())]);
    assert_eq!(h.audio.calls.lock().unwrap().len(), 1);

    // A word that exists only inside the spreadsheet finds the email.
    let (s, found) = call(&h.app, "GET", &format!("/api/v1/workspaces/{}/search?q=Zanzibar&limit=5", h.ws), None).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(found["items"][0]["message_id"], id.as_str(), "{found}");
    assert_eq!(found["items"][0]["kind"], "attachment");

    // The message detail says what was read, with short previews and never a model name.
    let detail = h.detail(&id).await;
    let by_name = |n: &str| detail["attachments"].as_array().unwrap().iter().find(|a| a["name"] == n).unwrap().clone();
    let sheet = by_name("budget.xlsx");
    assert_eq!((sheet["extract_kind"].as_str(), sheet["extract_status"].as_str()), (Some("sheet"), Some("done")));
    assert_eq!(sheet["preview"], "Sales: 1 rows × 2 columns");
    assert_eq!(sheet["extract_meta"]["sheets"][0]["sample"][1][0], "Zanzibar");
    assert_eq!(by_name("receipt.png")["preview"], "A cafe receipt");
    assert_eq!(by_name("receipt.png")["extract_meta"]["fields"]["total"], "42");
    assert_eq!(by_name("voicemail.wav")["preview"], "Caller wants the contract");
    assert_eq!(by_name("voicemail.wav")["extract_meta"]["action_items"][0], "Send the contract");
    assert_eq!(by_name("legacy.doc")["extract_status"], "skipped");
    assert!(!detail.to_string().contains("qwen"));

    // The insight was written from the email and its fenced, untrusted attachment text.
    let seen = h.analyst.0.lock().unwrap().clone();
    let last = seen.last().unwrap();
    assert!(last.contains("<attachments>") && last.contains("Zanzibar") && last.contains("A cafe receipt"), "{last}");
    assert!(last.contains("<attachment name=\"budget.xlsx\" kind=\"sheet\">"));
    let reindexed: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM knowledge_chunks WHERE kind = 'attachment'").fetch_one(&h.state.db).await.unwrap();
    assert!(reindexed >= 3);
}

#[tokio::test]
async fn attachment_text_triggers_at_most_one_refresh_and_only_when_it_adds_enough() {
    let h = harness().await;
    let refreshes = "SELECT COUNT(*) FROM jobs WHERE kind = 'analyze_message' AND dedupe_key LIKE 'analyze:%:attachments%'";

    // A short receipt reading is not worth reading the email again.
    let small = h.message("Receipt", 0, &[("receipt.png", "image/png", "att-img")]).await;
    let added: String = sqlx::query_scalar("SELECT text_content FROM attachments WHERE message_id = ?1").bind(&small).fetch_one(&h.state.db).await.unwrap();
    assert!(added.chars().count() < 200, "{added}");
    let n: i64 = sqlx::query_scalar(refreshes).fetch_one(&h.state.db).await.unwrap();
    assert_eq!(n, 0);
    assert_eq!(h.analyst.0.lock().unwrap().len(), 1, "one reading for the receipt email");

    // A spreadsheet plus voicemail adds plenty: exactly one refresh job, one more reading.
    h.analyst.0.lock().unwrap().clear();
    let big = h.message("Q3 numbers", 0, &ALL).await;
    let n: i64 = sqlx::query_scalar(&format!("{refreshes} AND dedupe_key LIKE 'analyze:{big}:%'")).fetch_one(&h.state.db).await.unwrap();
    assert_eq!(n, 1);
    assert!(h.analyst.0.lock().unwrap().len() <= 2, "the email is read at most twice: before and after its files");
}

#[tokio::test]
async fn old_mail_reads_documents_now_and_ai_files_when_opened() {
    let h = harness().await;
    let id = h.message("Old numbers", 10 * DAY_MS, &ALL).await;
    let rows = h.rows(&id).await;
    assert_eq!(rows["budget.xlsx"].1.as_deref(), Some("done"), "readers that need no model always run");
    assert_eq!((rows["receipt.png"].1.as_deref(), rows["voicemail.wav"].1.as_deref()), (Some("skipped"), Some("skipped")));
    assert!(rows["receipt.png"].2.as_deref().unwrap().contains("Open this email"));
    assert!(h.vision.calls.lock().unwrap().is_empty() && h.audio.calls.lock().unwrap().is_empty());

    let (s, _) = call(&h.app, "POST", &format!("/api/v1/workspaces/{}/messages/{id}/insight", h.ws), None).await;
    assert_eq!(s, StatusCode::OK);
    worker::drain(&h.state).await;
    let rows = h.rows(&id).await;
    assert_eq!((rows["receipt.png"].1.as_deref(), rows["voicemail.wav"].1.as_deref()), (Some("done"), Some("done")));
    assert_eq!(h.vision.calls.lock().unwrap().len(), 1);

    call(&h.app, "POST", &format!("/api/v1/workspaces/{}/messages/{id}/insight", h.ws), None).await;
    worker::drain(&h.state).await;
    assert_eq!(h.vision.calls.lock().unwrap().len(), 1, "already read: no second model call");
}

#[tokio::test]
async fn a_spent_budget_skips_ai_files_but_not_documents() {
    let h = harness().await;
    sqlx::query(
        "INSERT INTO ai_calls (id, workspace_id, task, model, cost_usd, ok, created_at) VALUES (?1, ?2, 'draft', 'm', 99.0, 1, ?3)",
    )
    .bind(new_id("aic"))
    .bind(&h.ws)
    .bind(now_ms())
    .execute(&h.state.db)
    .await
    .unwrap();
    let id = h.message("Over budget", 0, &ALL).await;
    let rows = h.rows(&id).await;
    assert_eq!(rows["budget.xlsx"].1.as_deref(), Some("done"));
    assert_eq!(rows["receipt.png"].1.as_deref(), Some("skipped"));
    assert!(h.vision.calls.lock().unwrap().is_empty());
}

#[tokio::test]
async fn provider_errors_are_recorded_on_the_attachment_and_never_fail_the_message() {
    let h = harness_with(StubVision::failing(AiError::Permanent("OpenRouter 400 for qwen/qwen3-vl".into())), |_| {}).await;
    let id = h.message("Bad image", 0, &ALL).await;
    let rows = h.rows(&id).await;
    assert_eq!(rows["receipt.png"].1.as_deref(), Some("failed"));
    let note = rows["receipt.png"].2.as_deref().unwrap();
    assert_eq!(note, "AI could not read this image");
    assert_eq!(rows["budget.xlsx"].1.as_deref(), Some("done"));
    let detail = h.detail(&id).await;
    assert!(!detail.to_string().contains("qwen") && !detail.to_string().contains("OpenRouter"));
    let status: String = sqlx::query_scalar("SELECT processing_status FROM email_messages WHERE id = ?1").bind(&id).fetch_one(&h.state.db).await.unwrap();
    assert_ne!(status, "failed");
    let dead: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM jobs WHERE status = 'dead'").fetch_one(&h.state.db).await.unwrap();
    assert_eq!(dead, 0);
}

#[tokio::test]
async fn a_fake_image_is_never_sent_to_a_model() {
    let h = harness().await;
    let id = h.message("Fake", 0, &[("invoice.png", "image/png", "att-fake")]).await;
    let rows = h.rows(&id).await;
    assert_eq!(rows["invoice.png"].1.as_deref(), Some("failed"));
    assert!(rows["invoice.png"].2.as_deref().unwrap().contains("not a PNG, JPEG or WebP"));
    assert!(h.vision.calls.lock().unwrap().is_empty());
}

#[tokio::test]
async fn without_readers_images_and_audio_are_skipped_but_documents_still_read() {
    let mut h = harness().await;
    h.state.vision = None;
    h.state.audio = None;
    let id = h.message("Bare", 0, &ALL[..3]).await;
    let rows = h.rows(&id).await;
    assert_eq!(rows["receipt.png"].1.as_deref(), Some("skipped"));
    assert!(rows["receipt.png"].2.as_deref().unwrap().contains("not set up"));
    assert_eq!(rows["budget.xlsx"].1.as_deref(), Some("done"));
}

#[tokio::test]
async fn forwarded_mail_attachments_are_skipped_with_a_reason() {
    let h = harness().await;
    let (_, src) = call(&h.app, "POST", &format!("/api/v1/workspaces/{}/sources/forwarding", h.ws), None).await;
    let path = format!("/api/v1/workspaces/{}/sources/{}/test-email", h.ws, src["id"].as_str().unwrap());
    call(&h.app, "POST", &path, None).await;
    worker::drain(&h.state).await;
    let (kind, status, note): (String, String, String) =
        sqlx::query_as("SELECT extract_kind, extract_status, extract_error FROM attachments WHERE filename = 'timesheet-september.pdf'")
            .fetch_one(&h.state.db)
            .await
            .unwrap();
    assert_eq!((kind.as_str(), status.as_str()), ("pdf", "skipped"));
    assert!(note.contains("forwarded"));
}

// ---------------------------------------------------------------- OpenRouter request shapes

/// `(route, headers, body)` per request.
type Request3 = (String, HeaderMap, Vec<u8>);

#[derive(Clone, Default)]
struct Captured(Arc<Mutex<Vec<Request3>>>);

async fn spawn_openrouter(captured: Captured) -> String {
    let chat = |State(c): State<Captured>, headers: HeaderMap, body: Bytes| async move {
        c.0.lock().unwrap().push(("chat".into(), headers, body.to_vec()));
        let request: Value = serde_json::from_slice(&body).unwrap();
        let has_image = request.to_string().contains("image_url");
        let content = if has_image {
            json!({"kind": "receipt", "text": "Total 42", "description": "A receipt", "fields": {"total": "42"}})
        } else {
            json!({"summary": "A call about a contract", "action_items": ["Send the contract"]})
        };
        Json(json!({
            "model": "whatever",
            "choices": [{"finish_reason": "stop", "message": {"content": content.to_string()}}],
            "usage": {"prompt_tokens": 1_000, "completion_tokens": 100}
        }))
    };
    let transcribe = |State(c): State<Captured>, headers: HeaderMap, body: Bytes| async move {
        c.0.lock().unwrap().push(("transcribe".into(), headers, body.to_vec()));
        Json(json!({"text": "piece of speech", "usage": {"cost": 0.002}}))
    };
    let app = Router::new().route("/chat/completions", post(chat)).route("/audio/transcriptions", post(transcribe)).with_state(captured);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    format!("http://{addr}")
}

#[tokio::test]
async fn vision_request_is_a_private_data_url_call_recorded_as_vision() {
    let h = harness().await;
    let captured = Captured::default();
    let url = spawn_openrouter(captured.clone()).await;
    let client = OpenRouterClient::new(reqwest::Client::new(), &url, "secret-key").with_ledger(h.state.db.clone());
    let vision = OpenRouterVision::new(client, "qwen/qwen3-vl-30b-a3b-instruct");

    let r = vision.read_image("image/png", &png()).await.unwrap();
    assert_eq!((r.kind.as_str(), r.text.as_str(), r.fields["total"].as_str()), ("receipt", "Total 42", Some("42")));

    let (_, headers, body) = captured.0.lock().unwrap()[0].clone();
    assert_eq!(headers["authorization"], "Bearer secret-key");
    let body: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(body["model"], "qwen/qwen3-vl-30b-a3b-instruct");
    assert_eq!(body["reasoning"]["enabled"], false);
    assert_eq!(body["provider"]["data_collection"], "deny");
    let url = body["messages"][1]["content"][1]["image_url"]["url"].as_str().unwrap();
    assert!(url.starts_with("data:image/png;base64,"));

    let (task, model, cost): (String, String, f64) = sqlx::query_as("SELECT task, model, cost_usd FROM ai_calls").fetch_one(&h.state.db).await.unwrap();
    assert_eq!((task.as_str(), model.as_str()), ("vision", "qwen/qwen3-vl-30b-a3b-instruct"));
    assert!((cost - (1_000.0 * 0.15 + 100.0 * 0.60) / 1e6).abs() < 1e-12);
}

#[tokio::test]
async fn long_audio_is_sent_as_multipart_pieces_then_summarised_once() {
    let h = harness().await;
    let captured = Captured::default();
    let url = spawn_openrouter(captured.clone()).await;
    let client = OpenRouterClient::new(reqwest::Client::new(), &url, "k").with_ledger(h.state.db.clone());
    let audio = OpenRouterAudio::new(client, "qwen/qwen3-asr-1.7b", "qwen/qwen3.8-flash");

    let parsed = read_audio(&audio, "call.wav", Some("audio/wav"), &wav(120)).await.unwrap();
    assert!(parsed.text.contains("Transcript:\npiece of speech piece of speech piece of speech"));
    assert_eq!(parsed.meta["chunks"], 3);
    assert_eq!(parsed.meta["action_items"][0], "Send the contract");

    let calls = captured.0.lock().unwrap().clone();
    let transcriptions: Vec<_> = calls.iter().filter(|c| c.0 == "transcribe").collect();
    assert_eq!(transcriptions.len(), 3);
    for (_, headers, body) in &transcriptions {
        assert!(headers["content-type"].to_str().unwrap().starts_with("multipart/form-data"));
        let text = String::from_utf8_lossy(body);
        assert!(text.contains("name=\"model\"\r\n\r\nqwen/qwen3-asr-1.7b"), "{}", &text[..text.len().min(300)]);
        assert!(text.contains("name=\"file\"; filename=\"part-0") && text.contains("Content-Type: audio/wav"));
    }
    assert_eq!(calls.iter().filter(|c| c.0 == "chat").count(), 1, "one cheap summary call over the transcript");

    let tasks: Vec<(String, f64)> = sqlx::query_as("SELECT task, cost_usd FROM ai_calls ORDER BY rowid").fetch_all(&h.state.db).await.unwrap();
    assert_eq!(tasks.iter().filter(|t| t.0 == "audio").count(), 4);
    assert!(tasks.iter().take(3).all(|t| (t.1 - 0.002).abs() < 1e-12), "provider-reported cost is recorded: {tasks:?}");
}
