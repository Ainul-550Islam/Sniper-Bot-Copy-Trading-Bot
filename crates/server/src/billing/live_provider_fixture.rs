//! Deterministic local provider fixtures (Batch 7).
//! Keep fixture tests completely separate from live-provider tests.
//! Model success/failure/retry/duplicate events. Do not use fixture results as live evidence.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum FixtureEvent {
    Success { id: String, amount: i64 },
    Failure { id: String, reason: String },
    Retry { id: String, attempt: u32 },
    Duplicate { id: String, original_id: String },
}

impl FixtureEvent {
    pub fn id(&self) -> &str {
        match self {
            Self::Success { id, .. } => id,
            Self::Failure { id, .. } => id,
            Self::Retry { id, .. } => id,
            Self::Duplicate { id, .. } => id,
        }
    }

    pub fn is_success(&self) -> bool {
        matches!(self, Self::Success { .. })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FixtureResult {
    pub event: FixtureEvent,
    pub signature_valid: bool,
    pub idempotency_key: String,
    pub processed: bool,
    pub detail: String,
}

impl FixtureResult {
    pub fn to_safe_json(&self) -> serde_json::Value {
        serde_json::json!({
            "event": self.event,
            "signature_valid": self.signature_valid,
            "idempotency_key": self.idempotency_key,
            "processed": self.processed,
            "detail": self.detail,
        })
    }
}

pub struct LiveProviderFixture;

impl LiveProviderFixture {
    pub fn process(event: FixtureEvent, signature_valid: bool) -> FixtureResult {
        let idempotency_key = format!("idem_{}", event.id());
        let (processed, detail) = match &event {
            FixtureEvent::Success { .. } if signature_valid => {
                (true, "fixture success processed".into())
            }
            FixtureEvent::Success { .. } => {
                (false, "fixture signature invalid — not processed".into())
            }
            FixtureEvent::Failure { reason, .. } => (false, format!("fixture failure: {}", reason)),
            FixtureEvent::Retry { attempt, .. } => {
                (true, format!("fixture retry attempt {}", attempt))
            }
            FixtureEvent::Duplicate { original_id, .. } => {
                (false, format!("fixture duplicate of {}", original_id))
            }
        };
        FixtureResult {
            event,
            signature_valid,
            idempotency_key,
            processed,
            detail,
        }
    }

    pub fn success(id: &str, amount: i64) -> FixtureEvent {
        FixtureEvent::Success {
            id: id.into(),
            amount,
        }
    }

    pub fn failure(id: &str, reason: &str) -> FixtureEvent {
        FixtureEvent::Failure {
            id: id.into(),
            reason: reason.into(),
        }
    }

    pub fn retry(id: &str, attempt: u32) -> FixtureEvent {
        FixtureEvent::Retry {
            id: id.into(),
            attempt,
        }
    }

    pub fn duplicate(id: &str, original: &str) -> FixtureEvent {
        FixtureEvent::Duplicate {
            id: id.into(),
            original_id: original.into(),
        }
    }

    pub fn label() -> &'static str {
        "NON-LIVE FIXTURE — not live evidence"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixture_success_with_valid_signature() {
        let e = LiveProviderFixture::success("evt_1", 1000);
        let r = LiveProviderFixture::process(e, true);
        assert!(r.processed);
        assert!(r.signature_valid);
        assert_eq!(r.idempotency_key, "idem_evt_1");
    }

    #[test]
    fn fixture_success_with_invalid_signature_not_processed() {
        let e = LiveProviderFixture::success("evt_2", 1000);
        let r = LiveProviderFixture::process(e, false);
        assert!(!r.processed);
    }

    #[test]
    fn fixture_duplicate_not_processed() {
        let e = LiveProviderFixture::duplicate("evt_3_dup", "evt_3");
        let r = LiveProviderFixture::process(e, true);
        assert!(!r.processed);
        assert!(r.detail.contains("duplicate"));
    }

    #[test]
    fn fixture_retry_processed() {
        let e = LiveProviderFixture::retry("evt_4", 2);
        let r = LiveProviderFixture::process(e, true);
        assert!(r.processed);
        assert!(r.detail.contains("retry"));
    }

    #[test]
    fn fixture_label_is_non_live() {
        assert!(LiveProviderFixture::label().contains("NON-LIVE"));
    }

    #[test]
    fn fixture_not_live_evidence() {
        let e = LiveProviderFixture::success("evt_5", 500);
        let r = LiveProviderFixture::process(e, true);
        // Fixture result must not be mistaken for live
        assert!(r.processed);
        let json = r.to_safe_json().to_string();
        assert!(!json.contains("live"));
        assert!(LiveProviderFixture::label().contains("NON-LIVE"));
    }
}
