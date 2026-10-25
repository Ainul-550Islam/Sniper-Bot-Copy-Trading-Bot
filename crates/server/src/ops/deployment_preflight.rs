//! Deployment preflight validator (Batch 4). Fail closed on unsafe production config. Never print secret values.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PreflightStatus {
    Pass,
    Warn,
    Block,
}

impl PreflightStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Pass => "pass",
            Self::Warn => "warn",
            Self::Block => "block",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PreflightCheck {
    pub name: String,
    pub status: PreflightStatus,
    pub detail: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PreflightReport {
    pub checks: Vec<PreflightCheck>,
    pub overall: PreflightStatus,
    pub mode: String,
}

fn contains_secret_hint(s: &str) -> bool {
    let l = s.to_ascii_lowercase();
    l.contains("secret")
        || l.contains("password")
        || l.contains("token")
        || l.contains("BEGIN PRIVATE KEY")
}

pub fn redacted(detail: &str) -> String {
    if contains_secret_hint(detail) {
        "<redacted>".into()
    } else {
        detail.to_string()
    }
}

#[derive(Debug, Clone)]
pub struct PreflightInput {
    pub env_mode: String, // "production"|"staging"|"development"
    pub database_url_present: bool,
    pub redis_url_present: bool,
    pub cors_origins: Vec<String>,
    pub secret_refs_present: bool,
    pub migration_high_water: Option<u32>,
    pub expected_high_water: u32,
    pub signer_mode: String, // "local"|"vault"|"kms"|"hsm"
    pub signer_configured: bool,
    pub billing_provider: String, // "manual"|"stripe"|"paddle"
    pub billing_configured: bool,
    pub live_trading_requested: bool,
    pub live_trading_allowed: bool,
    pub telemetry_enabled: bool,
    pub bind_host: String,
    pub trusted_proxies: Vec<String>,
    /// Webhook storage is enabled whenever the durable database is enabled,
    /// or live billing is requested. Never infer this from key presence.
    pub secret_storage_enabled: bool,
    /// Result of webhooks::decode_webhook_secret_key, not secret material.
    pub webhook_key_valid: bool,
}

/// The same security checks drive process startup and the deployment report.
/// Block is the existing report format's FAIL status.
pub fn security_checks(
    bind_host: &str,
    trusted_proxies: &[String],
    secret_storage_enabled: bool,
    env_mode: &str,
    live_trading_requested: bool,
    webhook_key_valid: bool,
) -> Vec<PreflightCheck> {
    let loopback = bot_core::config::is_loopback_host(bind_host);
    let proxy_status =
        match crate::saas::trusted_proxy::TrustedProxies::from_config(trusted_proxies) {
            Err(_) => PreflightStatus::Block,
            Ok(proxies) if !loopback && proxies.is_empty() => PreflightStatus::Warn,
            Ok(_) => PreflightStatus::Pass,
        };
    let local_paper_dev = loopback
        && !live_trading_requested
        && matches!(env_mode, "development" | "dev" | "local" | "test");
    let key_required = secret_storage_enabled && !local_paper_dev;
    vec![
        PreflightCheck {
            name: "trusted_proxies".into(),
            status: proxy_status,
            detail: "Configure only actual proxy CIDRs; empty trust on a reachable bind ignores all client-IP headers".into(),
        },
        PreflightCheck {
            name: "webhook_secret_encryption_key".into(),
            status: if webhook_key_valid || !secret_storage_enabled {
                PreflightStatus::Pass
            } else if key_required {
                PreflightStatus::Block
            } else {
                PreflightStatus::Warn
            },
            detail: "Secret storage requires a base64-encoded 32-byte WEBHOOK_SECRET_ENCRYPTION_KEY; local paper development keeps request-time refusal when unset".into(),
        },
    ]
}

/// Validate before opening databases, spawning workers, or serving HTTP.
/// Secret values never enter the report, errors or logs.
pub fn startup_security_checks(
    cfg: &bot_core::config::Config,
    env_mode: &str,
    live_billing: bool,
    encoded_webhook_key: Option<&str>,
) -> Vec<PreflightCheck> {
    security_checks(
        &cfg.api.bind_host,
        &cfg.api.trusted_proxies,
        cfg.database.enabled || live_billing,
        env_mode,
        cfg.execution.mode.is_live() || live_billing,
        encoded_webhook_key
            .is_some_and(|value| crate::saas::webhooks::decode_webhook_secret_key(value).is_ok()),
    )
}

pub fn evaluate(input: PreflightInput) -> PreflightReport {
    let mut checks = Vec::new();
    checks.extend(security_checks(
        &input.bind_host,
        &input.trusted_proxies,
        input.secret_storage_enabled,
        &input.env_mode,
        input.live_trading_requested,
        input.webhook_key_valid,
    ));
    let mode = input.env_mode.clone();
    let is_prod = mode == "production";

    // database
    checks.push(PreflightCheck {
        name: "database".into(),
        status: if input.database_url_present {
            PreflightStatus::Pass
        } else if is_prod {
            PreflightStatus::Block
        } else {
            PreflightStatus::Warn
        },
        detail: if input.database_url_present {
            "DATABASE_URL present".into()
        } else {
            "DATABASE_URL missing".into()
        },
    });
    // redis
    checks.push(PreflightCheck {
        name: "redis".into(),
        status: if input.redis_url_present {
            PreflightStatus::Pass
        } else {
            PreflightStatus::Warn
        },
        detail: if input.redis_url_present {
            "REDIS_URL present".into()
        } else {
            "REDIS_URL not set (optional)".into()
        },
    });
    // cors
    let cors_status = if is_prod
        && (input.cors_origins.is_empty() || input.cors_origins.iter().any(|o| o == "*"))
    {
        PreflightStatus::Block
    } else {
        PreflightStatus::Pass
    };
    checks.push(PreflightCheck {
        name: "cors".into(),
        status: cors_status,
        detail: redacted(&format!("origins={:?}", input.cors_origins)),
    });
    // secret refs
    checks.push(PreflightCheck {
        name: "secrets".into(),
        status: if input.secret_refs_present {
            PreflightStatus::Pass
        } else if is_prod {
            PreflightStatus::Block
        } else {
            PreflightStatus::Warn
        },
        detail: "secret references checked (values redacted)".into(),
    });
    // migrations
    let mig_status = match input.migration_high_water {
        Some(v) if v == input.expected_high_water => PreflightStatus::Pass,
        Some(v) if v < input.expected_high_water => PreflightStatus::Block,
        None => {
            if is_prod {
                PreflightStatus::Block
            } else {
                PreflightStatus::Warn
            }
        }
        _ => PreflightStatus::Block,
    };
    checks.push(PreflightCheck {
        name: "migrations".into(),
        status: mig_status,
        detail: format!(
            "high_water {:?} expected {:04}",
            input.migration_high_water.map(|v| format!("{v:04}")),
            input.expected_high_water
        ),
    });
    // signer
    let signer_status = if input.signer_configured || (input.signer_mode == "local" && !is_prod) {
        PreflightStatus::Pass
    } else {
        PreflightStatus::Block
    };
    checks.push(PreflightCheck {
        name: "signer".into(),
        status: signer_status,
        detail: format!(
            "mode={} configured={}",
            input.signer_mode, input.signer_configured
        ),
    });
    // billing
    let billing_status = if input.billing_configured || input.billing_provider == "manual" {
        PreflightStatus::Pass
    } else if is_prod {
        PreflightStatus::Block
    } else {
        PreflightStatus::Warn
    };
    checks.push(PreflightCheck {
        name: "billing".into(),
        status: billing_status,
        detail: format!(
            "provider={} configured={}",
            input.billing_provider, input.billing_configured
        ),
    });
    // live trading gate
    let live_status = if input.live_trading_requested && !input.live_trading_allowed {
        PreflightStatus::Block
    } else {
        PreflightStatus::Pass
    };
    checks.push(PreflightCheck {
        name: "live_trading".into(),
        status: live_status,
        detail: if input.live_trading_requested {
            "live requested".into()
        } else {
            "live not requested (paper)".into()
        },
    });
    // telemetry
    checks.push(PreflightCheck {
        name: "telemetry".into(),
        status: if input.telemetry_enabled {
            PreflightStatus::Pass
        } else {
            PreflightStatus::Warn
        },
        detail: if input.telemetry_enabled {
            "telemetry on".into()
        } else {
            "telemetry off".into()
        },
    });

    let overall = if checks.iter().any(|c| c.status == PreflightStatus::Block) {
        PreflightStatus::Block
    } else if checks.iter().any(|c| c.status == PreflightStatus::Warn) {
        PreflightStatus::Warn
    } else {
        PreflightStatus::Pass
    };
    PreflightReport {
        checks,
        overall,
        mode,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(super) fn base() -> PreflightInput {
        PreflightInput {
            env_mode: "production".into(),
            database_url_present: true,
            redis_url_present: true,
            cors_origins: vec!["https://example.com".into()],
            secret_refs_present: true,
            migration_high_water: Some(21),
            expected_high_water: 21,
            signer_mode: "local".into(),
            signer_configured: true,
            billing_provider: "manual".into(),
            billing_configured: true,
            live_trading_requested: false,
            live_trading_allowed: false,
            telemetry_enabled: true,
            bind_host: "0.0.0.0".into(),
            trusted_proxies: vec!["10.0.0.0/8".into()],
            secret_storage_enabled: true,
            webhook_key_valid: true,
        }
    }
    #[test]
    fn production_requires_db() {
        let mut i = base();
        i.database_url_present = false;
        let r = evaluate(i);
        assert_eq!(r.overall, PreflightStatus::Block);
        assert!(r
            .checks
            .iter()
            .any(|c| c.name == "database" && c.status == PreflightStatus::Block));
    }
    #[test]
    fn wildcard_cors_blocks_production() {
        let mut i = base();
        i.cors_origins = vec!["*".into()];
        let r = evaluate(i);
        assert!(r
            .checks
            .iter()
            .any(|c| c.name == "cors" && c.status == PreflightStatus::Block));
    }
    #[test]
    fn live_requested_without_allowed_blocks() {
        let mut i = base();
        i.live_trading_requested = true;
        i.live_trading_allowed = false;
        let r = evaluate(i);
        assert_eq!(r.overall, PreflightStatus::Block);
    }
    #[test]
    fn all_pass_when_configured() {
        let r = evaluate(base());
        assert_eq!(r.overall, PreflightStatus::Pass);
    }
    #[test]
    fn secrets_never_printed() {
        let d = redacted("password=secret123");
        assert_eq!(d, "<redacted>");
        let ok = redacted("origins=[https://example.com]");
        assert!(ok.contains("example.com"));
    }
}

#[cfg(test)]
mod startup_security_tests {
    use super::*;
    use base64::Engine;

    #[test]
    fn reachable_bind_without_proxy_trust_warns() {
        let mut cfg = bot_core::config::Config::default();
        cfg.api.bind_host = "0.0.0.0".into();
        let checks = startup_security_checks(&cfg, "production", false, None);
        assert!(checks
            .iter()
            .any(|c| c.name == "trusted_proxies" && c.status == PreflightStatus::Warn));
    }

    #[test]
    fn secret_storage_refuses_empty_short_invalid_keys_and_accepts_32_bytes() {
        let mut cfg = bot_core::config::Config::default();
        cfg.api.bind_host = "0.0.0.0".into();
        cfg.database.enabled = true;
        let base64 = base64::engine::general_purpose::STANDARD;
        for key in [
            None,
            Some(String::new()),
            Some("invalid!".into()),
            Some(base64.encode([1; 31])),
        ] {
            let checks = startup_security_checks(&cfg, "production", false, key.as_deref());
            assert!(checks
                .iter()
                .any(|c| c.name == "webhook_secret_encryption_key"
                    && c.status == PreflightStatus::Block));
        }
        let key = base64.encode([1; 32]);
        let checks = startup_security_checks(&cfg, "production", false, Some(&key));
        assert!(checks.iter().any(
            |c| c.name == "webhook_secret_encryption_key" && c.status == PreflightStatus::Pass
        ));
        cfg.api.bind_host = "127.0.0.1".into();
        let checks = startup_security_checks(&cfg, "development", false, None);
        assert!(checks.iter().all(|c| c.status != PreflightStatus::Block));
        let checks = startup_security_checks(&cfg, "production", false, None);
        assert!(checks.iter().any(|c| c.status == PreflightStatus::Block));
        let checks = startup_security_checks(&cfg, "development", true, None);
        assert!(checks.iter().any(|c| c.status == PreflightStatus::Block));
    }

    #[test]
    fn deployment_report_blocks_weak_webhook_key_and_invalid_proxy_cidr() {
        let mut input = super::tests::base();
        input.webhook_key_valid = false;
        let report = evaluate(input);
        assert!(report.checks.iter().any(
            |c| c.name == "webhook_secret_encryption_key" && c.status == PreflightStatus::Block
        ));
        let mut input = super::tests::base();
        input.trusted_proxies = vec!["not-a-cidr".into()];
        assert_eq!(evaluate(input).overall, PreflightStatus::Block);
    }
}
