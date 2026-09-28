//! Deterministic custody-provider fixture contract (Batch 7).
//! Test unavailable, unauthorized, successful remote sign fixture, revoked key, timeout.
//! Clearly label fixtures as NON-LIVE.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum CustodyFixtureState {
    Unavailable,
    Unauthorized,
    Success {
        public_key: String,
        signature: String,
    },
    Revoked,
    Timeout,
}

impl CustodyFixtureState {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Unavailable => "unavailable",
            Self::Unauthorized => "unauthorized",
            Self::Success { .. } => "success",
            Self::Revoked => "revoked",
            Self::Timeout => "timeout",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CustodyFixtureResult {
    pub state: CustodyFixtureState,
    pub sign_available: bool,
    pub public_key_redacted: Option<String>,
    pub detail: String,
    pub is_live: bool,
}

impl CustodyFixtureResult {
    pub fn to_safe_json(&self) -> serde_json::Value {
        serde_json::json!({
            "state": self.state.label(),
            "sign_available": self.sign_available,
            "public_key_redacted": self.public_key_redacted,
            "detail": self.detail,
            "is_live": self.is_live,
            "label": "NON-LIVE FIXTURE",
        })
    }
}

pub struct CustodyFixture;

impl CustodyFixture {
    pub fn unavailable() -> CustodyFixtureResult {
        CustodyFixtureResult {
            state: CustodyFixtureState::Unavailable,
            sign_available: false,
            public_key_redacted: None,
            detail: "fixture: provider unavailable — FAIL".into(),
            is_live: false,
        }
    }

    pub fn unauthorized() -> CustodyFixtureResult {
        CustodyFixtureResult {
            state: CustodyFixtureState::Unauthorized,
            sign_available: false,
            public_key_redacted: None,
            detail: "fixture: unauthorized — FAIL".into(),
            is_live: false,
        }
    }

    pub fn success(public_key: &str, signature: &str) -> CustodyFixtureResult {
        CustodyFixtureResult {
            state: CustodyFixtureState::Success {
                public_key: public_key.into(),
                signature: signature.into(),
            },
            sign_available: true,
            public_key_redacted: Some(format!(
                "{}...<redacted>",
                &public_key[..8.min(public_key.len())]
            )),
            detail: "fixture: successful remote sign (NON-LIVE)".into(),
            is_live: false,
        }
    }

    pub fn revoked() -> CustodyFixtureResult {
        CustodyFixtureResult {
            state: CustodyFixtureState::Revoked,
            sign_available: false,
            public_key_redacted: None,
            detail: "fixture: revoked key — FAIL".into(),
            is_live: false,
        }
    }

    pub fn timeout() -> CustodyFixtureResult {
        CustodyFixtureResult {
            state: CustodyFixtureState::Timeout,
            sign_available: false,
            public_key_redacted: None,
            detail: "fixture: timeout — FAIL".into(),
            is_live: false,
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
    fn unavailable_is_not_live() {
        let r = CustodyFixture::unavailable();
        assert!(!r.is_live);
        assert!(!r.sign_available);
        assert!(r.detail.contains("unavailable"));
        assert_eq!(r.state.label(), "unavailable");
    }

    #[test]
    fn unauthorized_is_not_live() {
        let r = CustodyFixture::unauthorized();
        assert!(!r.is_live);
        assert!(!r.sign_available);
    }

    #[test]
    fn success_is_redacted_and_non_live() {
        let r = CustodyFixture::success("pubkey1234567890", "sigABC");
        assert!(!r.is_live);
        assert!(r.sign_available);
        assert!(r.public_key_redacted.is_some());
        assert!(!r
            .public_key_redacted
            .as_ref()
            .unwrap()
            .contains("pubkey1234567890"));
        assert!(r.to_safe_json()["label"] == "NON-LIVE FIXTURE");
    }

    #[test]
    fn revoked_is_fail() {
        let r = CustodyFixture::revoked();
        assert!(!r.sign_available);
        assert_eq!(r.state.label(), "revoked");
    }

    #[test]
    fn timeout_is_fail() {
        let r = CustodyFixture::timeout();
        assert!(!r.sign_available);
        assert_eq!(r.state.label(), "timeout");
    }

    #[test]
    fn never_expose_private_key() {
        let r = CustodyFixture::success("pubkey", "sig");
        let json = r.to_safe_json().to_string();
        assert!(!json.to_lowercase().contains("private"));
        assert!(!json.contains("BEGIN PRIVATE KEY"));
    }
}
