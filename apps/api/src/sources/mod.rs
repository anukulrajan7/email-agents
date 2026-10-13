//! Source adapters. Everything downstream of ingestion works on `NormalizedEmail`
//! and `Capabilities`; nothing else branches on provider-specific fields.

pub mod nango;
pub mod normalize;
pub mod resend;

use rand::{Rng, distributions::Alphanumeric};

use crate::domain::Capabilities;

pub fn capabilities(kind: &str) -> Capabilities {
    match kind {
        "forwarding" => Capabilities::forwarding(),
        // Drafts and sends happen only on an explicit user click, never automatically.
        "nango_gmail" => Capabilities { read: true, sync: true, provider_labels: false, provider_drafts: true, send: true },
        // Connected mailboxes get real flags from granted scopes once implemented (P1).
        _ => Capabilities { read: false, sync: false, provider_labels: false, provider_drafts: false, send: false },
    }
}

/// Hard-to-guess routing alias, e.g. `inbox+k3j9...@inbound.example.com`.
pub fn generate_alias(domain: &str) -> String {
    let token: String = rand::thread_rng()
        .sample_iter(&Alphanumeric)
        .take(20)
        .map(|b| (b as char).to_ascii_lowercase())
        .collect();
    format!("inbox+{token}@{domain}")
}
