//! Outgoing transactional email (verification codes) over SMTP, e.g. a Hostinger mailbox
//! (`smtp.hostinger.com`, port 465 implicit TLS or 587 STARTTLS).

use std::{sync::Mutex, time::Duration};

use anyhow::Context;
use async_trait::async_trait;
use lettre::{
    AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor,
    message::{Mailbox, MultiPart},
    transport::smtp::authentication::Credentials,
};

#[async_trait]
pub trait Mailer: Send + Sync {
    async fn send(&self, to: &str, subject: &str, text: &str, html: &str) -> anyhow::Result<()>;
}

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
        let email = Message::builder()
            .from(self.from.clone())
            .to(to.parse().context("invalid recipient")?)
            .subject(subject)
            .multipart(MultiPart::alternative_plain_html(text.to_string(), html.to_string()))?;
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
