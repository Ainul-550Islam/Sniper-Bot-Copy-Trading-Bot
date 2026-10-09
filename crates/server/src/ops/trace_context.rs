//! Request/correlation ID propagation (Batch 5).
//! HTTP -> app -> DB/audit/event boundaries where supported.
//! IDs never contain secrets. Validation and redaction.

use serde::{Deserialize, Serialize};

/// Correlation ID — 8-64 chars, alphanumeric + hyphen/underscore only.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct CorrelationId(pub String);

impl CorrelationId {
    pub fn new(id: impl Into<String>) -> Result<Self, String> {
        let s = id.into();
        Self::validate(&s)?;
        Ok(Self(s))
    }

    pub fn validate(s: &str) -> Result<(), String> {
        let t = s.trim();
        if t.len() < 8 || t.len() > 64 {
            return Err(format!("correlation id length {} not in 8..64", t.len()));
        }
        if !t
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        {
            return Err("correlation id must be alphanumeric, hyphen or underscore".into());
        }
        let lower = t.to_ascii_lowercase();
        if lower.contains("password")
            || lower.contains("secret")
            || lower.contains("token")
            || lower.contains("bearer")
            || lower.contains("api_key")
        {
            return Err("correlation id must not contain secret-like substrings".into());
        }
        Ok(())
    }

    pub fn generate() -> Self {
        // Simple deterministic-like: uuid without hyphens truncated to 32 hex chars, no secrets
        let u = uuid::Uuid::new_v4().to_string().replace('-', "");
        Self(u[..32].to_string())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn redacted(&self) -> String {
        // Never leak, but correlation IDs are not secrets — still show first 8 chars only in logs?
        // For safety, show full but validated (no secrets). Provide redacted helper for sensitive contexts.
        if self.0.len() > 8 {
            format!("{}…", &self.0[..8])
        } else {
            self.0.clone()
        }
    }
}

/// Trace context propagated through layers.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TraceContext {
    pub correlation_id: CorrelationId,
    pub request_id: Option<CorrelationId>,
    pub parent_span: Option<String>,
}

impl TraceContext {
    pub fn new(correlation_id: CorrelationId) -> Self {
        Self {
            correlation_id,
            request_id: None,
            parent_span: None,
        }
    }

    pub fn with_request_id(mut self, id: CorrelationId) -> Self {
        self.request_id = Some(id);
        self
    }

    pub fn to_headers(&self) -> Vec<(String, String)> {
        let mut h = vec![("x-correlation-id".into(), self.correlation_id.0.clone())];
        if let Some(r) = &self.request_id {
            h.push(("x-request-id".into(), r.0.clone()));
        }
        h
    }

    pub fn from_headers(headers: &[(String, String)]) -> Option<Self> {
        let mut corr: Option<CorrelationId> = None;
        let mut req: Option<CorrelationId> = None;
        for (k, v) in headers {
            if k.eq_ignore_ascii_case("x-correlation-id") {
                if let Ok(c) = CorrelationId::new(v.clone()) {
                    corr = Some(c);
                }
            }
            if k.eq_ignore_ascii_case("x-request-id") {
                if let Ok(c) = CorrelationId::new(v.clone()) {
                    req = Some(c);
                }
            }
        }
        corr.map(|c| {
            let mut ctx = TraceContext::new(c);
            if let Some(r) = req {
                ctx = ctx.with_request_id(r);
            }
            ctx
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_correlation_id() {
        assert!(CorrelationId::new("abc12345").is_ok());
        assert!(CorrelationId::new("a-b_c-1234-5678").is_ok());
        assert!(CorrelationId::generate().0.len() == 32);
    }

    #[test]
    fn rejects_short_and_secret() {
        assert!(CorrelationId::new("short").is_err());
        assert!(CorrelationId::new("password-token-12345678").is_err());
        assert!(CorrelationId::new("api_key_12345678").is_err());
        assert!(CorrelationId::new("has spaces 12345678").is_err());
    }

    #[test]
    fn redacted_never_contains_secret() {
        let id = CorrelationId::new("abcd1234-efgh5678").unwrap();
        let r = id.redacted();
        assert!(!r.to_ascii_lowercase().contains("secret"));
        assert!(r.len() <= id.0.len());
    }

    #[test]
    fn header_roundtrip() {
        let corr = CorrelationId::new("corr-id-12345678").unwrap();
        let req = CorrelationId::new("req-id-87654321").unwrap();
        let ctx = TraceContext::new(corr.clone()).with_request_id(req.clone());
        let headers = ctx.to_headers();
        let back = TraceContext::from_headers(&headers).unwrap();
        assert_eq!(back.correlation_id, corr);
        assert_eq!(back.request_id, Some(req));
    }

    #[test]
    fn rejects_invalid_header_not_propagated() {
        let headers = vec![("x-correlation-id".into(), "bad secret".into())];
        assert!(TraceContext::from_headers(&headers).is_none());
    }
}
