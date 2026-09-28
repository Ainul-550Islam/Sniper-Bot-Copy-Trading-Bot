//! Safe runtime configuration summary (Batch 4). Never expose secret values.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfigCategory {
    pub name: String,
    pub configured: bool,
    pub status: String, // "configured"|"not_configured"|"disabled"|"enabled"
    pub detail: String, // safe, redacted
}

#[derive(Clone, Serialize, Deserialize)]
pub struct RuntimeConfigReport {
    pub environment: String,
    pub categories: Vec<ConfigCategory>,
    pub migration_high_water: Option<String>,
    pub generated_at: String,
}

fn redact(s: &str) -> String {
    let l = s.to_ascii_lowercase();
    if l.contains("secret")
        || l.contains("password")
        || l.contains("token")
        || l.contains("private")
    {
        "<redacted>".into()
    } else {
        s.to_string()
    }
}

#[allow(clippy::too_many_arguments)]
pub fn build_report(
    env: &str,
    database_configured: bool,
    redis_configured: bool,
    billing_provider: &str,
    billing_configured: bool,
    custody_provider: &str,
    custody_configured: bool,
    live_trading_enabled: bool,
    cors_mode: &str,
    migration_high_water: Option<u32>,
) -> RuntimeConfigReport {
    let mut cats = Vec::new();
    cats.push(ConfigCategory {
        name: "database".into(),
        configured: database_configured,
        status: if database_configured {
            "configured".into()
        } else {
            "not_configured".into()
        },
        detail: redact(if database_configured {
            "DATABASE_URL present"
        } else {
            "DATABASE_URL missing"
        }),
    });
    cats.push(ConfigCategory {
        name: "redis".into(),
        configured: redis_configured,
        status: if redis_configured {
            "configured".into()
        } else {
            "not_configured".into()
        },
        detail: redact(if redis_configured {
            "REDIS_URL present"
        } else {
            "REDIS_URL missing"
        }),
    });
    cats.push(ConfigCategory {
        name: "billing".into(),
        configured: billing_configured,
        status: if billing_configured {
            "configured".into()
        } else {
            "not_configured".into()
        },
        detail: redact(&format!("provider={billing_provider}")),
    });
    cats.push(ConfigCategory {
        name: "custody".into(),
        configured: custody_configured,
        status: if custody_configured {
            "configured".into()
        } else {
            "not_configured".into()
        },
        detail: redact(&format!("provider={custody_provider}")),
    });
    cats.push(ConfigCategory {
        name: "live_trading".into(),
        configured: live_trading_enabled,
        status: if live_trading_enabled {
            "enabled".into()
        } else {
            "disabled".into()
        },
        detail: if live_trading_enabled {
            "live enabled".into()
        } else {
            "paper only".into()
        },
    });
    cats.push(ConfigCategory {
        name: "cors".into(),
        configured: !cors_mode.is_empty() && cors_mode != "empty",
        status: cors_mode.to_string(),
        detail: redact(&format!("mode={cors_mode}")),
    });
    cats.push(ConfigCategory {
        name: "environment".into(),
        configured: true,
        status: env.to_string(),
        detail: redact(&format!("env={env}")),
    });

    RuntimeConfigReport {
        environment: env.to_string(),
        categories: cats,
        migration_high_water: migration_high_water.map(|v| format!("{v:04}")),
        generated_at: chrono::Utc::now().to_rfc3339(),
    }
}

impl std::fmt::Debug for RuntimeConfigReport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RuntimeConfigReport")
            .field("environment", &self.environment)
            .field("categories", &self.categories.len())
            .field("migration_high_water", &self.migration_high_water)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn redacts_secrets() {
        let r = build_report(
            "production",
            true,
            false,
            "stripe",
            false,
            "vault",
            false,
            false,
            "strict",
            Some(21),
        );
        let j = serde_json::to_string(&r).unwrap().to_ascii_lowercase();
        assert!(!j.contains("secret"));
        assert!(!j.contains("password"));
    }
    #[test]
    fn live_disabled_by_default() {
        let r = build_report(
            "development",
            false,
            false,
            "manual",
            true,
            "local",
            true,
            false,
            "empty",
            None,
        );
        let live = r
            .categories
            .iter()
            .find(|c| c.name == "live_trading")
            .unwrap();
        assert_eq!(live.status, "disabled");
    }
    #[test]
    fn migration_format() {
        let r = build_report(
            "production",
            true,
            true,
            "manual",
            true,
            "local",
            true,
            false,
            "strict",
            Some(21),
        );
        assert_eq!(r.migration_high_water, Some("0021".into()));
    }
    #[test]
    fn does_not_leak_url() {
        let r = build_report(
            "production",
            true,
            true,
            "stripe",
            true,
            "vault",
            true,
            false,
            "https://example.com",
            Some(21),
        );
        let j = serde_json::to_string(&r).unwrap();
        assert!(!j.contains("postgres://"));
    }
}
