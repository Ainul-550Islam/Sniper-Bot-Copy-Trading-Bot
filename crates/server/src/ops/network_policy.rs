//! Safe network dependency policy model (Batch 7).
//! Classify: local, internal, external, restricted, blocked.
//! Prevent accidental live-provider use during ordinary unit tests.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NetworkZone {
    Local,
    Internal,
    External,
    Restricted,
    Blocked,
}

impl NetworkZone {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Local => "local",
            Self::Internal => "internal",
            Self::External => "external",
            Self::Restricted => "restricted",
            Self::Blocked => "blocked",
        }
    }

    pub fn allows_live(&self) -> bool {
        matches!(self, Self::External)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetworkDependency {
    pub name: String,
    pub zone: NetworkZone,
    pub endpoint_ref: String,
    pub requires_opt_in: bool,
}

impl NetworkDependency {
    pub fn new(
        name: impl Into<String>,
        zone: NetworkZone,
        endpoint_ref: impl Into<String>,
        requires_opt_in: bool,
    ) -> Self {
        Self {
            name: name.into(),
            zone,
            endpoint_ref: endpoint_ref.into(),
            requires_opt_in,
        }
    }

    pub fn to_safe_json(&self) -> serde_json::Value {
        serde_json::json!({
            "name": self.name,
            "zone": self.zone.as_str(),
            "endpoint_ref": self.endpoint_ref,
            "requires_opt_in": self.requires_opt_in,
        })
    }
}

pub struct NetworkPolicy;

impl NetworkPolicy {
    pub fn default_dependencies() -> Vec<NetworkDependency> {
        vec![
            NetworkDependency::new(
                "postgres",
                NetworkZone::Internal,
                "POSTGRES_URL (redacted host)",
                true,
            ),
            NetworkDependency::new(
                "redis",
                NetworkZone::Internal,
                "REDIS_URL (redacted host)",
                true,
            ),
            NetworkDependency::new("stripe", NetworkZone::External, "STRIPE_API_KEY ref", true),
            NetworkDependency::new("paddle", NetworkZone::External, "PADDLE_API_KEY ref", true),
            NetworkDependency::new("vault", NetworkZone::External, "VAULT_ADDR ref", true),
            NetworkDependency::new("kms", NetworkZone::External, "KMS_KEY_ID ref", true),
            NetworkDependency::new("hsm", NetworkZone::External, "HSM_SLOT ref", true),
            NetworkDependency::new("solana_rpc", NetworkZone::External, "RPC_URL ref", true),
            NetworkDependency::new("geyser", NetworkZone::External, "GEYSER_URL ref", true),
            NetworkDependency::new(
                "deployment",
                NetworkZone::External,
                "DEPLOYMENT_BASE_URL ref",
                true,
            ),
            NetworkDependency::new(
                "staking_validator",
                NetworkZone::Local,
                "STAKING_E2E local validator",
                true,
            ),
        ]
    }

    pub fn classify(endpoint: &str) -> NetworkZone {
        let lower = endpoint.to_lowercase();
        if lower.contains("localhost") || lower.contains("127.0.0.1") || lower.contains("::1") {
            NetworkZone::Local
        } else if lower.contains("internal")
            || lower.contains("postgres")
            || lower.contains("redis")
        {
            NetworkZone::Internal
        } else if lower.contains("blocked") || lower.contains("forbidden") {
            NetworkZone::Blocked
        } else if lower.contains("restricted") {
            NetworkZone::Restricted
        } else {
            NetworkZone::External
        }
    }

    pub fn is_live_allowed(zone: NetworkZone, opt_in: bool) -> bool {
        match zone {
            NetworkZone::External => opt_in,
            NetworkZone::Local => opt_in, // even local validator needs STAKING_E2E=1
            _ => false,
        }
    }

    pub fn prevent_accidental_live_in_tests() -> bool {
        // Ordinary cargo test --workspace should NOT have LIVE_BILLING etc.
        // If any live flag is set, we are in explicit live mode
        let live_flags = [
            "LIVE_BILLING",
            "LIVE_CUSTODY",
            "DEPLOYMENT_SMOKE_LIVE",
            "STAKING_E2E",
            "RPC_URL",
            "GEYSER_URL",
            "DEPLOYMENT_BASE_URL",
        ];
        for f in live_flags {
            if std::env::var(f)
                .map(|v| !v.trim().is_empty() && v != "0")
                .unwrap_or(false)
            {
                return false; // live mode explicitly enabled
            }
        }
        true // safe — no live flags
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_local() {
        assert_eq!(
            NetworkPolicy::classify("http://localhost:5432"),
            NetworkZone::Local
        );
        assert_eq!(
            NetworkPolicy::classify("http://127.0.0.1:8080"),
            NetworkZone::Local
        );
    }

    #[test]
    fn classify_external_requires_opt_in() {
        assert_eq!(
            NetworkPolicy::classify("https://api.stripe.com"),
            NetworkZone::External
        );
        assert!(
            NetworkPolicy::default_dependencies()
                .iter()
                .find(|d| d.name == "stripe")
                .unwrap()
                .requires_opt_in
        );
    }

    #[test]
    fn live_not_allowed_without_opt_in() {
        assert!(!NetworkPolicy::is_live_allowed(
            NetworkZone::External,
            false
        ));
        assert!(NetworkPolicy::is_live_allowed(NetworkZone::External, true));
        assert!(!NetworkPolicy::is_live_allowed(NetworkZone::Internal, true));
        assert!(!NetworkPolicy::is_live_allowed(NetworkZone::Blocked, true));
    }

    #[test]
    fn ordinary_tests_prevent_accidental_live() {
        // In hermetic test env, live flags should not be set
        // We test that function returns true when no live env is set
        // (If env has live flags, this test would be in live mode and should not run as hermetic)
        // So we ensure that without live env, it prevents live
        let prev = std::env::var("LIVE_BILLING").ok();
        std::env::remove_var("LIVE_BILLING");
        std::env::remove_var("LIVE_CUSTODY");
        std::env::remove_var("DEPLOYMENT_SMOKE_LIVE");
        std::env::remove_var("STAKING_E2E");
        assert!(NetworkPolicy::prevent_accidental_live_in_tests());
        if let Some(v) = prev {
            std::env::set_var("LIVE_BILLING", v);
        }
    }

    #[test]
    fn zone_allows_live_only_external() {
        assert!(!NetworkZone::Local.allows_live());
        assert!(!NetworkZone::Internal.allows_live());
        assert!(NetworkZone::External.allows_live());
        assert!(!NetworkZone::Restricted.allows_live());
        assert!(!NetworkZone::Blocked.allows_live());
    }
}
