//! Server-side custody provider registry (§F, Batch 8).
//!
//! Resolves THIS deployment's custody posture from the operator
//! environment and exposes the core `CustodyProviderRegistry` plus an
//! honest, per-provider status. Two hard rules:
//!
//! 1. **Fail closed.** An unconfigured or unimplemented provider is
//!    reported as such and every sign attempt through it is refused with
//!    the exact dependency named. Nothing pretends to be live.
//! 2. **Single-operator deployments keep working.** The default active
//!    provider is `local`, matching the existing deployment where wallet
//!    keys are provided through the operator environment and signing runs
//!    through the established solana module path. The new SaaS custody
//!    boundary does not intercept or change that path — it is additive.
//!
//! Environment contract (reusing the `LiveCustodyConfig` contract from
//! `live_provider_contract.rs`, rule #35 — one env contract, not two):
//!
//! * `CUSTODY_PROVIDER` — `local` (default) | `vault` | `kms` | `hsm`
//! * `LIVE_CUSTODY` — `1` opts into remote custody (no local fallback)
//! * vault: `VAULT_ADDR` + `VAULT_TOKEN`; kms: `KMS_KEY_ID`; hsm: `HSM_SLOT`

use std::sync::Arc;

use crate::custody::kms::KmsCustodyProvider as ServerKmsProvider;
use crate::custody::live_provider_contract::LiveCustodyConfig;
use crate::custody::vault::VaultCustodyProvider as ServerVaultProvider;
use bot_core::custody::{
    CustodyProviderRegistry, HealthState, HsmCustodyProvider, LocalCustodyProvider, ProviderType,
};

/// Status of one provider type in this deployment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProviderAvailability {
    /// Selected and reference configuration present. Note: with the
    /// current OPTION-B provider adapters, signing still refuses until a
    /// real backend integration is deployed — `dependency` says exactly
    /// what is missing.
    Configured {
        provider: ProviderType,
        dependency: &'static str,
    },
    /// Provider type selected but required references are absent.
    MissingReferences {
        provider: ProviderType,
        dependency: &'static str,
    },
    /// Provider type is not selected by this deployment.
    NotSelected { provider: ProviderType },
}

impl ProviderAvailability {
    pub fn provider(&self) -> ProviderType {
        match self {
            ProviderAvailability::Configured { provider, .. }
            | ProviderAvailability::MissingReferences { provider, .. }
            | ProviderAvailability::NotSelected { provider } => *provider,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            ProviderAvailability::Configured { .. } => "configured",
            ProviderAvailability::MissingReferences { .. } => "missing_references",
            ProviderAvailability::NotSelected { .. } => "not_selected",
        }
    }

    pub fn dependency(&self) -> Option<&'static str> {
        match self {
            ProviderAvailability::Configured { dependency, .. }
            | ProviderAvailability::MissingReferences { dependency, .. } => Some(dependency),
            ProviderAvailability::NotSelected { .. } => None,
        }
    }

    /// True only when the provider is selected AND its references are
    /// present. NOTE: with the OPTION-B adapters, "ready" still does not
    /// mean "can sign" — `HealthState::Configured` fails closed in
    /// `ProviderHealth::is_signing_allowed`.
    pub fn is_ready(&self) -> bool {
        matches!(self, ProviderAvailability::Configured { .. })
    }
}

/// The exact integration dependency per remote provider, stated once.
const VAULT_DEPENDENCY: &str =
    "HashiCorp Vault transit engine with an ed25519 key, reachable at VAULT_ADDR, with a VAULT_TOKEN authorized for update on transit/keys/* and transit/sign/* (vault transit sign integration)";
const KMS_DEPENDENCY: &str =
    "AWS KMS with an Ed25519 (ECC_ED25519) key named by KMS_KEY_ID, an AWS region (KMS_REGION), and credentials from the standard AWS env chain (AWS_ACCESS_KEY_ID + AWS_SECRET_ACCESS_KEY, optional AWS_SESSION_TOKEN) authorized for kms:GetPublicKey and kms:Sign (aws kms ed25519 sign integration)";
const HSM_DEPENDENCY: &str =
    "PKCS#11 module with HSM_SLOT and an HSM_PIN reference (lunacia/pkcs11 sign integration)";
/// Local wallet custody through this boundary is not implemented — the
/// single-operator deployment signs via the existing solana module wallet
/// path (OPERATOR_KEY/WALLET env), which remains authoritative.
const LOCAL_DEPENDENCY: &str =
    "local wallet signing for the multi-tenant custody boundary (single-operator deployments sign via the existing solana module wallet path)";

/// The resolved custody posture of this deployment.
#[derive(Debug, Clone)]
pub struct CustodyDeployment {
    /// Provider this deployment routes custody traffic to.
    pub active: ProviderType,
    /// `LIVE_CUSTODY=1` — remote custody explicitly opted into.
    pub live_enabled: bool,
    /// Availability of every known provider type.
    pub providers: Vec<ProviderAvailability>,
}

impl CustodyDeployment {
    /// Resolve the deployment posture from the environment.
    ///
    /// Never fails: an unreadable or unknown `CUSTODY_PROVIDER` resolves
    /// to the fail-closed local default with `missing_references`/`not_selected`
    /// statuses — it can never silently enable a remote provider.
    pub fn resolve_from_env() -> Self {
        let requested = std::env::var("CUSTODY_PROVIDER")
            .unwrap_or_else(|_| "local".to_string())
            .trim()
            .to_ascii_lowercase();
        let live_enabled = std::env::var("LIVE_CUSTODY")
            .map(|v| v.trim() == "1")
            .unwrap_or(false);
        let active = ProviderType::parse(&requested).unwrap_or(ProviderType::Local);

        let mut providers = Vec::with_capacity(4);
        for provider in [
            ProviderType::Local,
            ProviderType::Vault,
            ProviderType::Kms,
            ProviderType::Hsm,
        ] {
            if provider != active {
                providers.push(ProviderAvailability::NotSelected { provider });
                continue;
            }
            if provider == ProviderType::Local {
                // The local adapter needs no remote references; its
                // OPTION-B refusal names the wallet-path dependency.
                providers.push(ProviderAvailability::Configured {
                    provider,
                    dependency: LOCAL_DEPENDENCY,
                });
                continue;
            }
            let contract = LiveCustodyConfig::from_env(provider.as_str());
            if contract.credential_ref_valid {
                providers.push(ProviderAvailability::Configured {
                    provider,
                    dependency: dependency_for(provider),
                });
            } else {
                providers.push(ProviderAvailability::MissingReferences {
                    provider,
                    dependency: dependency_for(provider),
                });
            }
        }
        Self {
            active,
            live_enabled,
            providers,
        }
    }

    /// Build the core provider registry matching the posture.
    ///
    /// The active provider is always registered so that attempts against
    /// it produce a real provider error (refused, exact reason) instead of
    /// a generic "not configured" — the registry itself is honest.
    pub fn registry(&self) -> CustodyProviderRegistry {
        let mut registry = CustodyProviderRegistry::new();
        match self.active {
            ProviderType::Local => {
                registry.register(Arc::new(LocalCustodyProvider::new()));
            }
            ProviderType::Vault => {
                // Real transit-engine adapter (§G): talks to Vault via
                // REST + JSON. Fails closed when references are absent.
                let (config, _state) = crate::custody::vault::VaultConfig::from_env();
                registry.register(Arc::new(ServerVaultProvider::new(config)));
            }
            ProviderType::Kms => {
                // Real SigV4-signed KMS adapter (§H). Fails closed when
                // credentials or the key reference are absent.
                let (config, _state) = crate::custody::kms::KmsConfig::from_env();
                registry.register(Arc::new(ServerKmsProvider::new(config)));
            }
            ProviderType::Hsm => {
                registry.register(Arc::new(HsmCustodyProvider));
            }
        }
        registry
    }

    /// Availability of the active provider.
    pub fn active_availability(&self) -> &ProviderAvailability {
        self.providers
            .iter()
            .find(|p| p.provider() == self.active)
            .expect("active provider always has an availability entry")
    }

    /// Secret-free one-line summary for logs and health endpoints.
    pub fn summary(&self) -> String {
        let active = self.active_availability();
        format!(
            "custody active={} status={} live_enabled={}",
            self.active.as_str(),
            active.as_str(),
            self.live_enabled
        )
    }
}

fn dependency_for(provider: ProviderType) -> &'static str {
    match provider {
        ProviderType::Local => LOCAL_DEPENDENCY,
        ProviderType::Vault => VAULT_DEPENDENCY,
        ProviderType::Kms => KMS_DEPENDENCY,
        ProviderType::Hsm => HSM_DEPENDENCY,
    }
}

/// Map a core `CustodyProviderError` to the honest availability story for
/// the health endpoint: `NotConfigured`/`UnsupportedProvider` are
/// permanent states (integration absent), everything else is a runtime
/// failure.
pub fn health_state_for_error(err: &bot_core::custody::CustodyProviderError) -> HealthState {
    match err {
        bot_core::custody::CustodyProviderError::NotConfigured(_)
        | bot_core::custody::CustodyProviderError::UnsupportedProvider(_) => {
            HealthState::Configured
        }
        _ => HealthState::Unavailable,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::custody::test_support::ENV_LOCK;

    fn with_env<F: FnOnce() -> T, T>(vars: &[(&str, Option<&str>)], f: F) -> T {
        struct EnvGuard(Vec<String>);
        impl Drop for EnvGuard {
            fn drop(&mut self) {
                for key in &self.0 {
                    std::env::remove_var(key);
                }
            }
        }
        // Remember the PRE-EXISTING value (or absence) of every var so
        // restoration is exact — including vars that were not set before.
        let saved: Vec<(String, Option<String>)> = vars
            .iter()
            .map(|(k, _)| ((*k).to_string(), std::env::var(k).ok()))
            .collect();
        for (k, v) in vars {
            match v {
                Some(val) => std::env::set_var(k, val),
                None => std::env::remove_var(k),
            }
        }
        let guard = EnvGuard(vars.iter().map(|(k, _)| k.to_string()).collect());
        let out = f();
        drop(guard);
        // restore pre-existing values (or their absence)
        for (key, prev) in saved {
            match prev {
                Some(val) => std::env::set_var(&key, val),
                None => std::env::remove_var(&key),
            }
        }
        out
    }

    #[test]
    fn default_is_local_and_not_live() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        with_env(
            &[
                ("CUSTODY_PROVIDER", None),
                ("LIVE_CUSTODY", None),
                ("VAULT_ADDR", None),
                ("VAULT_TOKEN", None),
            ],
            || {
                let d = CustodyDeployment::resolve_from_env();
                assert_eq!(d.active, ProviderType::Local);
                assert!(!d.live_enabled);
                assert!(d.active_availability().is_ready());
                assert!(d.summary().contains("active=local"));
                let not_vault = d
                    .providers
                    .iter()
                    .find(|p| p.provider() == ProviderType::Vault)
                    .unwrap();
                assert_eq!(not_vault.as_str(), "not_selected");
            },
        );
    }

    #[test]
    fn vault_selected_without_references_is_missing_references() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        with_env(
            &[
                ("CUSTODY_PROVIDER", Some("vault")),
                ("LIVE_CUSTODY", Some("1")),
                ("VAULT_ADDR", None),
                ("VAULT_TOKEN", None),
            ],
            || {
                let d = CustodyDeployment::resolve_from_env();
                assert_eq!(d.active, ProviderType::Vault);
                assert!(d.live_enabled);
                let active = d.active_availability();
                assert_eq!(active.as_str(), "missing_references");
                assert!(!active.is_ready());
                assert!(active.dependency().unwrap().contains("VAULT_ADDR"));
            },
        );
    }

    #[test]
    fn vault_selected_with_references_is_configured() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        with_env(
            &[
                ("CUSTODY_PROVIDER", Some("vault")),
                ("LIVE_CUSTODY", Some("1")),
                ("VAULT_ADDR", Some("https://vault.internal:8200")),
                ("VAULT_TOKEN", None),
            ],
            || {
                // VAULT_TOKEN missing -> still missing references
                let d = CustodyDeployment::resolve_from_env();
                assert_eq!(
                    d.active_availability().as_str(),
                    "missing_references",
                    "both VAULT_ADDR and VAULT_TOKEN are required"
                );
            },
        );
        with_env(
            &[
                ("CUSTODY_PROVIDER", Some("vault")),
                ("LIVE_CUSTODY", Some("1")),
                ("VAULT_ADDR", Some("https://vault.internal:8200")),
                ("VAULT_TOKEN", Some("hvs.token")),
            ],
            || {
                let d = CustodyDeployment::resolve_from_env();
                let active = d.active_availability();
                assert_eq!(active.as_str(), "configured");
                assert!(active.dependency().unwrap().contains("transit"));
            },
        );
    }

    #[test]
    fn unknown_provider_falls_back_to_local_fail_closed() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        with_env(
            &[
                ("CUSTODY_PROVIDER", Some("telepathy")),
                ("LIVE_CUSTODY", None),
            ],
            || {
                let d = CustodyDeployment::resolve_from_env();
                assert_eq!(d.active, ProviderType::Local);
            },
        );
    }

    #[test]
    fn registry_registers_exactly_the_active_provider() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        with_env(
            &[
                ("CUSTODY_PROVIDER", Some("kms")),
                ("LIVE_CUSTODY", Some("1")),
            ],
            || {
                let d = CustodyDeployment::resolve_from_env();
                let registry = d.registry();
                assert!(registry.provider(ProviderType::Kms).is_some());
                assert!(registry.provider(ProviderType::Local).is_none());
                assert!(registry.provider(ProviderType::Vault).is_none());
            },
        );
    }

    #[test]
    fn health_state_maps_not_configured_to_configured() {
        use bot_core::custody::CustodyProviderError;
        assert_eq!(
            health_state_for_error(&CustodyProviderError::NotConfigured(ProviderType::Vault)),
            HealthState::Configured
        );
        assert_eq!(
            health_state_for_error(&CustodyProviderError::UnsupportedProvider(
                ProviderType::Hsm
            )),
            HealthState::Configured
        );
        assert_eq!(
            health_state_for_error(&CustodyProviderError::Transport("boom".into())),
            HealthState::Unavailable
        );
    }
}
