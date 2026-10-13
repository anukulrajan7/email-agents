//! Deterministic backend policy. `needs_review` is never taken from the model.

use crate::{
    ai::ClassifyOutput,
    domain::ClassificationSource,
};

/// Labels at or above this confidence are auto-approved. Recalibrate on labelled examples.
pub const AUTO_TRUST_CONFIDENCE: f64 = 0.8;

pub fn needs_review(c: &ClassifyOutput) -> bool {
    // Labelling is fully automatic. A person only looks when the AI could not label
    // at all (dead-lettered → "other" error fallback). Confidence is kept for metrics.
    let _ = AUTO_TRUST_CONFIDENCE;
    matches!(c.source, ClassificationSource::Fallback) && c.model_id.as_deref() == Some("error-fallback")
}

/// Sending requires a capable source; it always also needs an explicit user action.
pub fn can_send(capabilities: &crate::domain::Capabilities) -> bool {
    capabilities.read && capabilities.send
}

/// Provider-side drafts require a capable source *and* an explicit user action.
pub fn can_create_provider_draft(capabilities: &crate::domain::Capabilities) -> bool {
    capabilities.read && capabilities.provider_drafts
}
