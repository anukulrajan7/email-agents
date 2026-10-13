//! Deterministic backend policy. `needs_review` is never taken from the model.

use crate::{
    ai::ClassifyOutput,
    domain::ClassificationSource,
};

/// Labels at or above this confidence are auto-approved. Recalibrate on labelled examples.
pub const AUTO_TRUST_CONFIDENCE: f64 = 0.8;

pub fn needs_review(c: &ClassifyOutput) -> bool {
    match c.source {
        ClassificationSource::Manual => false,
        ClassificationSource::Fallback => true,
        // A confident "Other" (newsletters, notifications) needs no human; low confidence
        // or money-related labels do. Error fallbacks never come through here.
        ClassificationSource::Jev | ClassificationSource::Llm => {
            c.category == "payroll" || c.confidence.is_none_or(|conf| conf < AUTO_TRUST_CONFIDENCE)
        }
    }
}

/// Sending requires a capable source; it always also needs an explicit user action.
pub fn can_send(capabilities: &crate::domain::Capabilities) -> bool {
    capabilities.read && capabilities.send
}

/// Provider-side drafts require a capable source *and* an explicit user action.
pub fn can_create_provider_draft(capabilities: &crate::domain::Capabilities) -> bool {
    capabilities.read && capabilities.provider_drafts
}
