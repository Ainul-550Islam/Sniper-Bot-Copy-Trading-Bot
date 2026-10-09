//! Safe operator report for rate limits (Batch 5).
//! Expose policy names and thresholds only. No Redis credentials or internal keys.
//! Distinguish configured / not-configured.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RateLimitStatus {
    Configured,
    NotConfigured,
    Disabled,
}

impl RateLimitStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Configured => "configured",
            Self::NotConfigured => "not_configured",
            Self::Disabled => "disabled",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RateLimitPolicy {
    pub name: String,
    pub status: RateLimitStatus,
    /// Requests per window
    pub limit: Option<u32>,
    pub window_secs: Option<u64>,
    /// Human-readable detail, never contains credentials
    pub detail: String,
}

impl RateLimitPolicy {
    pub fn new(
        name: impl Into<String>,
        status: RateLimitStatus,
        limit: Option<u32>,
        window_secs: Option<u64>,
        detail: impl Into<String>,
    ) -> Self {
        Self {
            name: name.into(),
            status,
            limit,
            window_secs,
            detail: detail.into(),
        }
    }

    pub fn redacted_detail(&self) -> String {
        let lower = self.detail.to_ascii_lowercase();
        if lower.contains("redis://")
            || lower.contains("password")
            || lower.contains("secret")
            || lower.contains("token")
        {
            "<redacted>".into()
        } else {
            self.detail.clone()
        }
    }

    pub fn to_safe_json(&self) -> serde_json::Value {
        serde_json::json!({
            "name": self.name,
            "status": self.status.as_str(),
            "limit": self.limit,
            "window_secs": self.window_secs,
            "detail": self.redacted_detail()
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RateLimitReport {
    pub policies: Vec<RateLimitPolicy>,
    pub generated_at: String,
}

impl RateLimitReport {
    pub fn new(policies: Vec<RateLimitPolicy>, generated_at: impl Into<String>) -> Self {
        Self {
            policies,
            generated_at: generated_at.into(),
        }
    }

    pub fn is_configured(&self, name: &str) -> bool {
        self.policies
            .iter()
            .any(|p| p.name == name && p.status == RateLimitStatus::Configured)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn configured_vs_not_configured_distinction() {
        let r = RateLimitReport::new(
            vec![
                RateLimitPolicy::new(
                    "api",
                    RateLimitStatus::Configured,
                    Some(100),
                    Some(60),
                    "100 req/60s",
                ),
                RateLimitPolicy::new(
                    "ws",
                    RateLimitStatus::NotConfigured,
                    None,
                    None,
                    "not configured",
                ),
            ],
            "2026-09-24T00:00:00Z",
        );
        assert!(r.is_configured("api"));
        assert!(!r.is_configured("ws"));
    }

    #[test]
    fn never_exposes_redis_url() {
        let p = RateLimitPolicy::new(
            "api",
            RateLimitStatus::Configured,
            Some(60),
            Some(60),
            "redis://:secret@localhost:6379",
        );
        assert_eq!(p.redacted_detail(), "<redacted>");
        let j = p.to_safe_json().to_string().to_ascii_lowercase();
        assert!(!j.contains("redis://"));
    }

    #[test]
    fn safe_json_contains_thresholds_only() {
        let p = RateLimitPolicy::new(
            "billing",
            RateLimitStatus::Configured,
            Some(10),
            Some(60),
            "10 per minute",
        );
        let v = p.to_safe_json();
        assert_eq!(v["limit"], 10);
        assert_eq!(v["window_secs"], 60);
        assert_eq!(v["name"], "billing");
    }

    #[test]
    fn disabled_is_distinct() {
        let p = RateLimitPolicy::new("test", RateLimitStatus::Disabled, None, None, "disabled");
        assert_eq!(p.status, RateLimitStatus::Disabled);
    }
}
