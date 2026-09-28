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
}

pub fn evaluate(input: PreflightInput) -> PreflightReport {
    let mut checks = Vec::new();
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

    fn base() -> PreflightInput {
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
