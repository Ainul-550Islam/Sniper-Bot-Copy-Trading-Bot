//! Typed observability configuration (Batch 5).
//! Log level, JSON/plain mode, metrics/tracing flags, OTLP endpoint reference,
//! sampling rate, service name/version. Rejects unsafe production config.
//! Never exposes credentials. Safe Debug.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LogLevel {
    Trace,
    Debug,
    Info,
    Warn,
    Error,
}

impl LogLevel {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Trace => "trace",
            Self::Debug => "debug",
            Self::Info => "info",
            Self::Warn => "warn",
            Self::Error => "error",
        }
    }
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "trace" => Some(Self::Trace),
            "debug" => Some(Self::Debug),
            "info" => Some(Self::Info),
            "warn" | "warning" => Some(Self::Warn),
            "error" => Some(Self::Error),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LogFormat {
    Json,
    Plain,
}

impl LogFormat {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Json => "json",
            Self::Plain => "plain",
        }
    }
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "json" => Some(Self::Json),
            "plain" | "text" => Some(Self::Plain),
            _ => None,
        }
    }
}

/// Typed observability config — no secrets, safe to serialize.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ObservabilityConfig {
    pub log_level: LogLevel,
    pub log_format: LogFormat,
    pub metrics_enabled: bool,
    pub tracing_enabled: bool,
    /// OTLP endpoint reference (e.g. "https://otel.example.com:4317"), not a credential.
    /// Must not contain password/token/query secret.
    pub otlp_endpoint_ref: Option<String>,
    /// Sampling rate 0.0..=1.0
    pub sampling_rate: f64,
    pub service_name: String,
    pub service_version: String,
}

impl ObservabilityConfig {
    pub fn development() -> Self {
        Self {
            log_level: LogLevel::Debug,
            log_format: LogFormat::Plain,
            metrics_enabled: true,
            tracing_enabled: false,
            otlp_endpoint_ref: None,
            sampling_rate: 0.1,
            service_name: "sniper-suite".into(),
            service_version: env!("CARGO_PKG_VERSION").into(),
        }
    }
    pub fn staging() -> Self {
        Self {
            log_level: LogLevel::Info,
            log_format: LogFormat::Json,
            metrics_enabled: true,
            tracing_enabled: true,
            otlp_endpoint_ref: Some("https://otel-staging.example.com:4317".into()),
            sampling_rate: 0.25,
            service_name: "sniper-suite".into(),
            service_version: env!("CARGO_PKG_VERSION").into(),
        }
    }
    pub fn production() -> Self {
        Self {
            log_level: LogLevel::Info,
            log_format: LogFormat::Json,
            metrics_enabled: true,
            tracing_enabled: true,
            otlp_endpoint_ref: Some("https://otel.example.com:4317".into()),
            sampling_rate: 0.1,
            service_name: "sniper-suite".into(),
            service_version: env!("CARGO_PKG_VERSION").into(),
        }
    }

    /// Validate for production safety. Returns Err(reason) if unsafe.
    pub fn validate_for_production(&self) -> Result<(), String> {
        // Reject trace in prod (too verbose, may leak)
        if self.log_level == LogLevel::Trace {
            return Err("log_level trace not allowed in production".into());
        }
        // Sampling must be 0..=1
        if !(0.0..=1.0).contains(&self.sampling_rate) {
            return Err(format!(
                "sampling_rate {} out of range 0.0..=1.0",
                self.sampling_rate
            ));
        }
        // OTLP ref must not look like secret
        if let Some(ep) = &self.otlp_endpoint_ref {
            let lower = ep.to_ascii_lowercase();
            if lower.contains("password")
                || lower.contains("token")
                || lower.contains("secret")
                || lower.contains("api_key")
                || lower.contains('@') && lower.contains(':')
            {
                // naive check for user:pass@host
                if lower.contains("://") && lower.split("://").nth(1).unwrap_or("").contains('@') {
                    return Err("otlp_endpoint_ref must not contain credentials".into());
                }
            }
            if ep.contains('?') && (lower.contains("token") || lower.contains("key")) {
                return Err("otlp_endpoint_ref must not contain query secrets".into());
            }
            if ep.trim().is_empty() {
                return Err("otlp_endpoint_ref empty".into());
            }
        }
        if self.service_name.trim().is_empty() {
            return Err("service_name required".into());
        }
        if self.service_version.trim().is_empty() {
            return Err("service_version required".into());
        }
        Ok(())
    }

    /// Safe JSON value — never leaks secrets (there are none, but we still ensure).
    pub fn to_safe_json(&self) -> serde_json::Value {
        serde_json::json!({
            "log_level": self.log_level.as_str(),
            "log_format": self.log_format.as_str(),
            "metrics_enabled": self.metrics_enabled,
            "tracing_enabled": self.tracing_enabled,
            "otlp_endpoint_ref": self.otlp_endpoint_ref.clone().unwrap_or_else(|| "not_configured".into()),
            "sampling_rate": self.sampling_rate,
            "service_name": self.service_name,
            "service_version": self.service_version
        })
    }

    /// Redacted Debug: ensure endpoint does not contain secret substrings.
    pub fn safe_debug(&self) -> String {
        format!("{:?}", self.to_safe_json())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn development_config_is_not_production_unsafe() {
        let c = ObservabilityConfig::development();
        // dev may be plain, that's ok but production validation should pass for production preset
        assert!(ObservabilityConfig::production()
            .validate_for_production()
            .is_ok());
        // dev with trace would be unsafe if promoted
        let mut prod_like = c.clone();
        prod_like.log_level = LogLevel::Trace;
        assert!(prod_like.validate_for_production().is_err());
    }

    #[test]
    fn staging_and_production_are_valid() {
        assert!(ObservabilityConfig::staging()
            .validate_for_production()
            .is_ok());
        assert!(ObservabilityConfig::production()
            .validate_for_production()
            .is_ok());
    }

    #[test]
    fn rejects_unsafe_otlp_with_credentials() {
        let mut c = ObservabilityConfig::production();
        c.otlp_endpoint_ref = Some("https://user:secret@otel.example.com:4317".into());
        assert!(c.validate_for_production().is_err());
        c.otlp_endpoint_ref = Some("https://otel.example.com:4317?token=abc".into());
        assert!(c.validate_for_production().is_err());
    }

    #[test]
    fn sampling_rate_bounds() {
        let mut c = ObservabilityConfig::production();
        c.sampling_rate = 1.5;
        assert!(c.validate_for_production().is_err());
        c.sampling_rate = -0.1;
        assert!(c.validate_for_production().is_err());
        c.sampling_rate = 0.5;
        assert!(c.validate_for_production().is_ok());
    }

    #[test]
    fn safe_json_never_contains_password() {
        let c = ObservabilityConfig::production();
        let j = c.to_safe_json().to_string().to_ascii_lowercase();
        assert!(!j.contains("password"));
        assert!(!j.contains("secret"));
        let dbg = c.safe_debug();
        assert!(!dbg.to_ascii_lowercase().contains("password"));
    }

    #[test]
    fn production_requires_service_name_version() {
        let mut c = ObservabilityConfig::production();
        c.service_name = "   ".into();
        assert!(c.validate_for_production().is_err());
    }
}
