//! Outgoing transactional email (verification codes, alerts) over SMTP, e.g. a Hostinger mailbox
//! (`smtp.hostinger.com`, port 465 implicit TLS or 587 STARTTLS). Callers go through `deliver`,
//! which retries with backoff and records every attempt in `email_outbox`.

use std::{sync::Mutex, time::Duration};

use anyhow::Context;
use async_trait::async_trait;
use lettre::{
    AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor,
    message::{
        Mailbox, MultiPart,
        header::{HeaderName, HeaderValue},
    },
    transport::smtp::authentication::Credentials,
};
use sqlx::SqlitePool;

use crate::platform::db::{new_id, now_ms};

#[async_trait]
pub trait Mailer: Send + Sync {
    async fn send(&self, to: &str, subject: &str, text: &str, html: &str) -> anyhow::Result<()>;
}

/// Value of the `X-PostRoom` header on every message we send (the trait carries no per-mail kind).
const SYSTEM_MAIL_KIND: &str = "notice";

pub struct SmtpMailer {
    transport: AsyncSmtpTransport<Tokio1Executor>,
    from: Mailbox,
}

impl SmtpMailer {
    pub fn new(host: &str, port: u16, username: &str, password: &str, from: &str) -> anyhow::Result<Self> {
        let builder = if port == 465 {
            AsyncSmtpTransport::<Tokio1Executor>::relay(host)?
        } else {
            AsyncSmtpTransport::<Tokio1Executor>::starttls_relay(host)?
        };
        let transport = builder
            .port(port)
            .credentials(Credentials::new(username.to_string(), password.to_string()))
            .timeout(Some(Duration::from_secs(15)))
            .build();
        Ok(Self { transport, from: from.parse().context("SMTP_FROM must be like 'Post Room <no-reply@example.com>'")? })
    }
}

#[async_trait]
impl Mailer for SmtpMailer {
    async fn send(&self, to: &str, subject: &str, text: &str, html: &str) -> anyhow::Result<()> {
        let mut email = Message::builder()
            .from(self.from.clone())
            .to(to.parse().context("invalid recipient")?)
            .subject(subject)
            .multipart(MultiPart::alternative_plain_html(text.to_string(), html.to_string()))?;
        // Lets the receiving Post Room recognise its own mail and never draft, case or alert on it.
        let headers = email.headers_mut();
        headers.insert_raw(HeaderValue::new(HeaderName::new_from_ascii_str("X-PostRoom"), SYSTEM_MAIL_KIND.to_string()));
        headers.insert_raw(HeaderValue::new(HeaderName::new_from_ascii_str("Auto-Submitted"), "auto-generated".to_string()));
        self.transport.send(email).await.context("SMTP send failed")?;
        Ok(())
    }
}

/// Test double that records what would have been sent.
#[derive(Default)]
pub struct MemoryMailer {
    pub sent: Mutex<Vec<(String, String, String)>>,
}

#[async_trait]
impl Mailer for MemoryMailer {
    async fn send(&self, to: &str, subject: &str, text: &str, _html: &str) -> anyhow::Result<()> {
        self.sent.lock().expect("mailer lock").push((to.into(), subject.into(), text.into()));
        Ok(())
    }
}

/// Browser E2E only (`MAIL_CAPTURE_FILE`): appends each message as one JSON line instead of sending it.
pub struct FileMailer {
    path: std::path::PathBuf,
}

impl FileMailer {
    pub fn new(path: &str) -> Self {
        Self { path: path.into() }
    }
}

#[async_trait]
impl Mailer for FileMailer {
    async fn send(&self, to: &str, subject: &str, text: &str, _html: &str) -> anyhow::Result<()> {
        use std::io::Write;
        let line = serde_json::json!({ "to": to, "subject": subject, "text": text, "at": now_ms() });
        let mut file = std::fs::OpenOptions::new().create(true).append(true).open(&self.path).context("opening MAIL_CAPTURE_FILE")?;
        writeln!(file, "{line}").context("writing MAIL_CAPTURE_FILE")?;
        Ok(())
    }
}

pub mod kind {
    pub const OTP: &str = "otp";
    pub const ALERT: &str = "alert";
    pub const ALERT_DIGEST: &str = "alert_digest";
    pub const ALERT_TEST: &str = "alert_test";
    pub const MATCH_DIGEST: &str = "match_digest";
    pub const TIMESHEET_SUMMARY: &str = "timesheet_summary";
}

pub const MAX_ATTEMPTS: u32 = 3;
/// Waits before attempt 2 and 3.
const RETRY_DELAYS_MS: [u64; 2] = [400, 1_200];

pub struct OutgoingEmail<'a> {
    pub kind: &'a str,
    pub to: &'a str,
    pub subject: &'a str,
    pub text: &'a str,
    pub html: &'a str,
}

/// Subject as stored in the outbox: verification subjects carry the code, so they are replaced.
fn logged_subject(kind: &str, subject: &str) -> String {
    if kind == kind::OTP { "Verification code".into() } else { crate::mail::sources::normalize::truncate_chars(subject, 200) }
}

/// Sends with up to `MAX_ATTEMPTS` tries (backoff between them) and records the outcome in
/// `email_outbox`. Returns the last error when every attempt failed.
pub async fn deliver(db: &SqlitePool, mailer: &dyn Mailer, email: OutgoingEmail<'_>) -> anyhow::Result<()> {
    let id = new_id("out");
    let now = now_ms();
    let logged = sqlx::query(
        "INSERT INTO email_outbox (id, kind, to_address, subject, status, attempts, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, 'sending', 0, ?5, ?5)",
    )
    .bind(&id)
    .bind(email.kind)
    .bind(email.to)
    .bind(logged_subject(email.kind, email.subject))
    .bind(now)
    .execute(db)
    .await;
    if let Err(e) = logged {
        tracing::warn!(error = %e, "could not record outgoing email");
    }
    let mut last_error = String::new();
    let mut attempts = 0;
    for attempt in 0..MAX_ATTEMPTS {
        if attempt > 0 {
            tokio::time::sleep(Duration::from_millis(RETRY_DELAYS_MS[(attempt as usize - 1).min(RETRY_DELAYS_MS.len() - 1)])).await;
        }
        attempts = attempt + 1;
        match mailer.send(email.to, email.subject, email.text, email.html).await {
            Ok(()) => {
                last_error.clear();
                break;
            }
            Err(e) => {
                last_error = format!("{e:#}");
                tracing::warn!(kind = email.kind, attempt = attempts, error = %last_error, "transactional email attempt failed");
            }
        }
    }
    let ok = last_error.is_empty();
    let recorded = sqlx::query("UPDATE email_outbox SET status = ?1, attempts = ?2, last_error = ?3, updated_at = ?4 WHERE id = ?5")
        .bind(if ok { "sent" } else { "failed" })
        .bind(attempts)
        .bind((!ok).then(|| crate::mail::sources::normalize::truncate_chars(&last_error, 500)))
        .bind(now_ms())
        .bind(&id)
        .execute(db)
        .await;
    if let Err(e) = recorded {
        tracing::warn!(error = %e, "could not record outgoing email result");
    }
    if ok { Ok(()) } else { Err(anyhow::anyhow!("email not sent after {attempts} attempts: {last_error}")) }
}

pub fn verification_email(code: &str) -> (String, String, String) {
    let subject = format!("{code} is your Post Room verification code");
    let text = format!(
        "Your Post Room verification code is {code}.\n\nIt expires in 10 minutes. If you didn't try to sign up, you can ignore this email."
    );
    let html = format!(
        r#"<div style="font-family:Georgia,serif;background:#F3EFE6;padding:32px;color:#1C1B18">
<div style="height:6px;background:repeating-linear-gradient(135deg,#B73A26 0 10px,#F3EFE6 10px 20px,#34557A 20px 30px,#F3EFE6 30px 40px)"></div>
<h1 style="font-weight:600;font-size:24px;margin:24px 0 8px">Verify your email</h1>
<p style="font-family:Arial,sans-serif;color:#5E5A50;margin:0 0 16px">Enter this code in Post Room. It expires in 10 minutes.</p>
<p style="font-family:'Courier New',monospace;font-size:32px;letter-spacing:8px;margin:0 0 24px"><strong>{code}</strong></p>
<p style="font-family:Arial,sans-serif;font-size:12px;color:#5E5A50">If you didn't try to sign up, you can ignore this email.</p>
</div>"#
    );
    (subject, text, html)
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    struct Flaky {
        fail_first: usize,
        calls: AtomicUsize,
    }

    #[async_trait]
    impl Mailer for Flaky {
        async fn send(&self, _to: &str, _s: &str, _t: &str, _h: &str) -> anyhow::Result<()> {
            if self.calls.fetch_add(1, Ordering::SeqCst) < self.fail_first { anyhow::bail!("smtp 421") } else { Ok(()) }
        }
    }

    async fn pool() -> (SqlitePool, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let db = crate::platform::db::connect(&format!("sqlite://{}", dir.path().join("t.db").display())).await.unwrap();
        crate::platform::db::migrate(&db).await.unwrap();
        (db, dir)
    }

    fn email<'a>(kind: &'a str, subject: &'a str) -> OutgoingEmail<'a> {
        OutgoingEmail { kind, to: "a@b.com", subject, text: "t", html: "h" }
    }

    #[tokio::test]
    async fn retries_then_records_outcome() {
        let (db, _dir) = pool().await;
        let flaky = Flaky { fail_first: 2, calls: AtomicUsize::new(0) };
        deliver(&db, &flaky, email(kind::ALERT, "[Post Room] 2 emails")).await.unwrap();
        assert_eq!(flaky.calls.load(Ordering::SeqCst), 3);
        let row: (String, i64, Option<String>, String) =
            sqlx::query_as("SELECT status, attempts, last_error, subject FROM email_outbox").fetch_one(&db).await.unwrap();
        assert_eq!(row, ("sent".into(), 3, None, "[Post Room] 2 emails".into()));

        let dead = Flaky { fail_first: 99, calls: AtomicUsize::new(0) };
        let err = deliver(&db, &dead, email(kind::OTP, "123456 is your code")).await.unwrap_err();
        assert!(err.to_string().contains("after 3 attempts"));
        let row: (String, i64, Option<String>, String) = sqlx::query_as(
            "SELECT status, attempts, last_error, subject FROM email_outbox WHERE kind = 'otp'",
        )
        .fetch_one(&db)
        .await
        .unwrap();
        assert_eq!((row.0.as_str(), row.1, row.3.as_str()), ("failed", 3, "Verification code"), "OTP code never stored");
        assert!(row.2.unwrap().contains("smtp 421"));
    }
}
