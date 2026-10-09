//! Vault connectivity / permission / readiness checks (§G, spec file 51).
//!
//! One probe, three ordered questions, all answered by REAL calls to the
//! configured Vault:
//!
//! 1. **Connectivity** — `GET /v1/sys/health`: is Vault reachable, and is
//!    the node active (not sealed / standby / not-initialized)?
//! 2. **Permission** — `GET /v1/auth/token/lookup-self`: is the service
//!    token valid, and can it read the signer's transit key?
//! 3. **Readiness** — `GET /v1/{mount}/keys/{key}`: does the key exist,
//!    is it ed25519, and is it not scheduled for deletion?
//!
//! The result is a secret-free `VaultReadiness` that maps onto the core
//! `ProviderHealth` used by the boundary and by health endpoints. When
//! any step fails the state is honestly `Unavailable` (or `Degraded` for
//! a reachable Vault that cannot serve this key) with the exact missing
//! dependency named — never a fabricated "healthy".

use bot_core::custody::{HealthState, ProviderHealth, ProviderType, SignerRecord};
use chrono::Utc;

use crate::custody::vault::client::{VaultClient, VaultClientError, VAULT_TRANSIT_DEPENDENCY};
use crate::custody::vault::config::VaultConfig;

/// The full, ordered result of a Vault readiness probe.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VaultReadiness {
    /// The service was reachable and reported itself active.
    pub reachable: bool,
    /// The token was accepted (`lookup-self` succeeded).
    pub token_valid: bool,
    /// The transit key exists, is ed25519, and is not being deleted.
    pub key_ready: bool,
    /// Secret-free detail for each failed stage.
    pub detail: String,
}

impl VaultReadiness {
    /// True only when every probe passed — the only state in which the
    /// Vault provider may be allowed to sign.
    pub fn is_ready(&self) -> bool {
        self.reachable && self.token_valid && self.key_ready
    }

    /// Honest health-state mapping:
    /// * ready → `Reachable`;
    /// * Vault reachable but key/token not usable → `Degraded`;
    /// * Vault not reachable (or references missing) → `Unavailable`.
    pub fn health_state(&self) -> HealthState {
        if self.is_ready() {
            HealthState::Reachable
        } else if self.reachable {
            HealthState::Degraded
        } else {
            HealthState::Unavailable
        }
    }

    /// Render as the core `ProviderHealth` for boundary gating and
    /// health endpoints.
    pub fn to_provider_health(&self) -> ProviderHealth {
        ProviderHealth::new(
            ProviderType::Vault,
            self.health_state(),
            self.detail.clone(),
            Utc::now(),
        )
    }
}

/// Run the full ordered probe against the configured Vault for a
/// specific signer's transit key (or the deployment default).
///
/// Never panics, never logs the token, never fabricates success.
pub async fn check_vault_readiness(
    config: &VaultConfig,
    signer: Option<&SignerRecord>,
) -> VaultReadiness {
    let key_name = match signer {
        Some(record) => config.transit_key_for(record),
        None => config.default_key().map(|s| s.to_string()),
    }
    .filter(|name| !name.is_empty());

    // Stage 1 — connectivity.
    let client = VaultClient::new(config);
    let service = client.sys_health().await;
    if !service.can_serve() {
        return VaultReadiness {
            reachable: false,
            token_valid: false,
            key_ready: false,
            detail: format!(
                "vault at {} is {}: service must be active before custody signing is allowed; requires {}",
                config.addr(),
                service.as_str(),
                VAULT_TRANSIT_DEPENDENCY
            ),
        };
    }

    // Stage 2 — token permission.
    if let Err(err) = client.token_lookup_self().await {
        return VaultReadiness {
            reachable: true,
            token_valid: false,
            key_ready: false,
            detail: format!(
                "vault reachable but token was rejected: {err}; requires {VAULT_TRANSIT_DEPENDENCY}"
            ),
        };
    }

    // Stage 3 — key readiness.
    let key_name = match key_name {
        Some(name) => name,
        None => {
            return VaultReadiness {
                reachable: true,
                token_valid: true,
                key_ready: false,
                detail: format!(
                    "vault reachable and token valid, but no transit key resolved (set the signer provider_ref or VAULT_TRANSIT_KEY); requires {VAULT_TRANSIT_DEPENDENCY}"
                ),
            }
        }
    };
    match client.transit_key(&key_name).await {
        Ok(info) if info.is_ed25519() && !info.deletion_time_present => VaultReadiness {
            reachable: true,
            token_valid: true,
            key_ready: true,
            detail: format!(
                "vault active, token valid, transit key '{key_name}' (v{}) is ed25519 and ready",
                info.latest_version
            ),
        },
        Ok(info) => VaultReadiness {
            reachable: true,
            token_valid: true,
            key_ready: false,
            detail: format!(
                "transit key '{key_name}' is not usable: type={} (ed25519 required), deletion_scheduled={}; requires {VAULT_TRANSIT_DEPENDENCY}",
                info.key_type,
                info.deletion_time_present
            ),
        },
        Err(err) => VaultReadiness {
            reachable: true,
            token_valid: true,
            key_ready: false,
            detail: format!(
                "vault reachable but key '{key_name}' is not usable: {err}; requires {VAULT_TRANSIT_DEPENDENCY}"
            ),
        },
    }
}

/// Convenience wrapper used by health endpoints: the deployment-level
/// probe (default key, no specific signer).
pub async fn deployment_readiness(config: &VaultConfig) -> VaultReadiness {
    check_vault_readiness(config, None).await
}

/// Map a client error from a readiness probe to an honest health state
/// (used by callers that already hold a typed error).
pub fn health_state_for_client_error(err: &VaultClientError) -> HealthState {
    match err.code {
        "vault_unreachable" => HealthState::Unavailable,
        _ => HealthState::Degraded,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::custody::vault::config::VaultConfig;

    #[tokio::test]
    async fn probe_of_unconfigured_vault_is_unavailable_not_fake_healthy() {
        let _lock = crate::custody::test_support::ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        std::env::remove_var("VAULT_ADDR");
        std::env::remove_var("VAULT_TOKEN");
        std::env::remove_var("VAULT_TRANSIT_KEY");
        let (config, state) = VaultConfig::from_env();
        assert!(!state.base_ready());
        drop(_lock); // release the env lock before awaiting
        let readiness = deployment_readiness(&config).await;
        assert!(!readiness.is_ready());
        assert_eq!(readiness.health_state(), HealthState::Unavailable);
        assert!(readiness.detail.contains("unreachable"));
        // The rendered provider health must be secret-free.
        let health = readiness.to_provider_health();
        assert!(!health.is_signing_allowed());
        assert!(!health.safe_summary().contains("VAULT_TOKEN="));
    }

    #[tokio::test]
    async fn probe_of_unreachable_vault_is_unavailable_with_dependency_named() {
        let _lock = crate::custody::test_support::ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        std::env::set_var("VAULT_ADDR", "http://127.0.0.1:1");
        std::env::set_var("VAULT_TOKEN", "hvs.readiness.test");
        std::env::set_var("VAULT_TRANSIT_KEY", "k1");
        let (config, state) = VaultConfig::from_env();
        assert!(state.sign_ready());
        drop(_lock); // release the env lock before awaiting
        let readiness = deployment_readiness(&config).await;
        assert!(!readiness.is_ready());
        assert_eq!(readiness.health_state(), HealthState::Unavailable);
        assert!(readiness.detail.contains("VAULT_ADDR"));
        assert!(!readiness.detail.contains("hvs.readiness.test"));
        std::env::remove_var("VAULT_ADDR");
        std::env::remove_var("VAULT_TOKEN");
        std::env::remove_var("VAULT_TRANSIT_KEY");
    }
}
