//! Sends each draft to the drafter for its quality tier.

use std::sync::Arc;

use async_trait::async_trait;

use super::{AiError, DraftInput, DraftOutput, DraftQuality, Drafter};

pub struct RouterDrafter {
    standard: Arc<dyn Drafter>,
    premium: Arc<dyn Drafter>,
}

impl RouterDrafter {
    pub fn new(standard: Arc<dyn Drafter>, premium: Arc<dyn Drafter>) -> Self {
        Self { standard, premium }
    }
}

#[async_trait]
impl Drafter for RouterDrafter {
    async fn draft(&self, input: &DraftInput) -> Result<DraftOutput, AiError> {
        match input.quality {
            DraftQuality::Standard => self.standard.draft(input).await,
            DraftQuality::Premium => self.premium.draft(input).await,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Named(&'static str);

    #[async_trait]
    impl Drafter for Named {
        async fn draft(&self, _: &DraftInput) -> Result<DraftOutput, AiError> {
            Ok(DraftOutput { body_text: String::new(), model_id: self.0.into(), unanswered: vec![] })
        }
    }

    fn input(quality: DraftQuality) -> DraftInput {
        DraftInput {
            category: "other".into(),
            category_name: "Other".into(),
            category_description: String::new(),
            subject: "s".into(),
            sender: "a@b.co".into(),
            body_text: "b".into(),
            memory_notes: vec![],
            sender_history: None,
            insight: None,
            examples: vec![],
            quality,
            reply_instructions: String::new(),
            signature: None,
            default_sign_off: "HR Operations".into(),
            thread_digest: None,
        }
    }

    #[tokio::test]
    async fn routes_by_quality() {
        let r = RouterDrafter::new(Arc::new(Named("standard")), Arc::new(Named("premium")));
        assert_eq!(r.draft(&input(DraftQuality::Standard)).await.unwrap().model_id, "standard");
        assert_eq!(r.draft(&input(DraftQuality::Premium)).await.unwrap().model_id, "premium");
    }
}
