//! Deterministic backend policy. `needs_review` is never taken from the model.

use crate::{
    ai::ClassifyOutput,
    domain::{ClassificationSource, OTHER_LABEL},
};

/// Uncalibrated placeholder; recalibrate on held-out labelled examples before relying on it.
pub const AUTO_TRUST_CONFIDENCE: f64 = 0.9;

pub fn needs_review(c: &ClassifyOutput) -> bool {
    match c.source {
        ClassificationSource::Manual => false,
        ClassificationSource::Fallback => true,
        ClassificationSource::Jev | ClassificationSource::Llm => {
            c.category == OTHER_LABEL || c.category == "payroll"
                || c.confidence.is_none_or(|conf| conf < AUTO_TRUST_CONFIDENCE)
        }
    }
}

/// Provider-side drafts require a capable source *and* an explicit user action.
pub fn can_create_provider_draft(capabilities: &crate::domain::Capabilities) -> bool {
    capabilities.read && capabilities.provider_drafts
}
