//! Server-side custody health (§F, Batch 8).
//!
//! One honest health picture of the custody boundary, built from:
//!
//! * the deployment posture (`CustodyDeployment` — what is selected and
//!   whether references exist),
//! * a live `health_check()` probe against the active provider adapter,
//! * the pre-existing `LiveCustodyContract` check (LIVE_CUSTODY opt-in
//!   and reference validity — rule #35, reuse, don't duplicate).
//!
//! The report never claims `ready` unless the active provider's adapter
//! actually answered its health probe. With the OPTION-B adapters the
//! remote providers answer `Ok(())` only for `LocalCustodyProvider`;
//! remote adapters fail closed, which surfaces here as
//! `configured_unsupported` with the exact integration dependency — never
//! as a fake "ready".

use bot_core::custody::CustodyProviderError;
use chrono::{DateTime, Utc};
use serde::Serialize;

use crate::custody::live_provider_contract::{LiveCustodyConfig, LiveCustodyContract};
use crate::custody::provider_registry::{CustodyDeployment, ProviderAvailability};

/// Deployment-level readiness of the custody boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CustodyBoundaryStatus {
    /// Active provider answered its health probe and is usable.
    Ready,
    /// Selected with valid references, but the provider integration is
    /// not implemented in this deployment (OPTION B) — signing refuses.
    ConfiguredUnsupported,
    /// Selected but required environment references are missing.
    MissingReferences,
    /// Provider probe failed at runtime (transport / upstream).
    Unreachable,
    /// Remote custody selected but LIVE_CUSTODY is not opted in.
    NotOptedIn,
}

impl CustodyBoundaryStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            CustodyBoundaryStatus::Ready => "ready",
            CustodyBoundaryStatus::ConfiguredUnsupported => "configured_unsupported",
            CustodyBoundaryStatus::MissingReferences => "missing_references",
            CustodyBoundaryStatus::Unreachable => "unreachable",
            CustodyBoundaryStatus::NotOptedIn => "not_opted_in",
        }
    }

    /// Can signing succeed right now?
    pub fn allows_signing(&self) -> bool {
        matches!(self, CustodyBoundaryStatus::Ready)
    }
}

/// The full, secret-free health report.
#[derive(Debug, Clone, Serialize)]
pub struct CustodyBoundaryHealth {
    pub status: CustodyBoundaryStatus,
    pub active_provider: String,
    pub live_custody_opted_in: bool,
    /// Machine + human readable refusal reason when not ready.
    pub dependency: Option<String>,
    /// Provider probe outcome detail (secret-free).
    pub probe: String,
    pub checked_at: DateTime<Utc>,
}

impl CustodyBoundaryHealth {
    /// Probe the active provider adapter and build the report.
    pub async fn probe(deployment: &CustodyDeployment) -> Self {
        let now = Utc::now();
        let active = deployment.active;
        let availability = deployment.active_availability();
        let dependency = availability.dependency().map(|d| d.to_string());

        // 1. Remote custody must be explicitly opted in.
        if active.is_remote() && !deployment.live_enabled {
            return Self {
                status: CustodyBoundaryStatus::NotOptedIn,
                active_provider: active.as_str().to_string(),
                live_custody_opted_in: false,
                dependency,
                probe: "LIVE_CUSTODY != 1 — remote custody not opted in; boundary refuses to sign"
                    .to_string(),
                checked_at: now,
            };
        }

        // 2. References must exist for the selected provider.
        if let ProviderAvailability::MissingReferences { dependency, .. } = availability {
            return Self {
                status: CustodyBoundaryStatus::MissingReferences,
                active_provider: active.as_str().to_string(),
                live_custody_opted_in: deployment.live_enabled,
                dependency: Some(dependency.to_string()),
                probe: "provider references incomplete".to_string(),
                checked_at: now,
            };
        }

        // 3. Live probe of the registered adapter.
        let registry = deployment.registry();
        let probe_result = match registry.provider(active) {
            Some(provider) => provider.health_check().await,
            None => Err(CustodyProviderError::NotConfigured(active)),
        };

        match probe_result {
            Ok(()) => {
                if active.is_remote() {
                    // §G/§H: the vault and kms adapters are REAL
                    // integrations — an answered health probe means the
                    // remote provider is live and signing through it is
                    // implemented. (The HSM adapter remains OPTION-B and
                    // refuses in health_check, so it never reaches here.)
                    Self {
                        status: CustodyBoundaryStatus::Ready,
                        active_provider: active.as_str().to_string(),
                        live_custody_opted_in: deployment.live_enabled,
                        dependency,
                        probe: format!(
                            "remote custody adapter {} answered the health probe; signing is live through it",
                            active.as_str()
                        ),
                        checked_at: now,
                    }
                } else {
                    Self {
                        status: CustodyBoundaryStatus::Ready,
                        active_provider: active.as_str().to_string(),
                        live_custody_opted_in: deployment.live_enabled,
                        dependency,
                        probe: "local custody adapter healthy".to_string(),
                        checked_at: now,
                    }
                }
            }
            Err(err) => {
                let permanent = matches!(
                    err,
                    CustodyProviderError::NotConfigured(_)
                        | CustodyProviderError::UnsupportedProvider(_)
                );
                Self {
                    status: if permanent {
                        // Integration absent in this deployment — NOT a
                        // transient outage. Report configured_unsupported.
                        CustodyBoundaryStatus::ConfiguredUnsupported
                    } else {
                        CustodyBoundaryStatus::Unreachable
                    },
                    active_provider: active.as_str().to_string(),
                    live_custody_opted_in: deployment.live_enabled,
                    dependency,
                    probe: err.to_string(),
                    checked_at: now,
                }
            }
        }
    }

    /// Cross-check with the Batch-7 `LiveCustodyContract` so both health
    /// views stay consistent (they describe the same boundary).
    ///
    /// The Batch-7 contract reports PRECONDITIONS (LIVE_CUSTODY opt-in +
    /// reference validity) and is hermetic — `sign_available: true` with
    /// status `NotRun` means "a live provider could sign", not "this
    /// deployment can sign now". This health additionally knows whether
    /// the provider integration is implemented. Consistency therefore
    /// means: both views agree on the PRECONDITION state.
    ///
    /// The contract has no `local` arm — the local deployment posture is
    /// out of its scope, so the cross-check is defined for remote
    /// providers only and trivially true for local.
    pub fn cross_check_live_contract(&self) -> bool {
        let Some(active) = bot_core::custody::ProviderType::parse(&self.active_provider) else {
            return false;
        };
        if !active.is_remote() {
            return true;
        }
        let contract = LiveCustodyConfig::from_env(self.active_provider.clone());
        let result = LiveCustodyContract::check(contract);
        // Preconditions present = references valid + opted in + capability
        // (the contract guarantees all three in its success path).
        let contract_preconditions_ok = result.credential_valid && result.sign_available;
        match self.status {
            // Ready/ConfiguredUnsupported both require opt-in + valid
            // references — the contract must agree they are present.
            CustodyBoundaryStatus::Ready | CustodyBoundaryStatus::ConfiguredUnsupported => {
                contract_preconditions_ok
            }
            // Missing references or missing opt-in — the contract must
            // agree they are absent.
            CustodyBoundaryStatus::MissingReferences | CustodyBoundaryStatus::NotOptedIn => {
                !contract_preconditions_ok
            }
            // A runtime failure is invisible to the static contract; no
            // contradiction is possible.
            CustodyBoundaryStatus::Unreachable => true,
        }
    }

    /// Safe for the ops API — no secrets, no credentials.
    pub fn to_safe_json(&self) -> serde_json::Value {
        serde_json::json!({
            "status": self.status.as_str(),
            "active_provider": self.active_provider,
            "live_custody_opted_in": self.live_custody_opted_in,
            "dependency": self.dependency,
            "probe": self.probe,
            "checked_at": self.checked_at.to_rfc3339(),
            "allows_signing": self.status.allows_signing(),
        })
    }
}

impl std::fmt::Display for CustodyBoundaryHealth {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "custody status={} provider={} allows_signing={}",
            self.status.as_str(),
            self.active_provider,
            self.status.allows_signing()
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bot_core::custody::ProviderType;

    use crate::custody::test_support::ENV_LOCK;

    fn health_for(
        provider: &str,
        live: Option<&str>,
    ) -> (CustodyBoundaryHealth, CustodyDeployment) {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let prev_provider = std::env::var("CUSTODY_PROVIDER").ok();
        let prev_live = std::env::var("LIVE_CUSTODY").ok();
        std::env::set_var("CUSTODY_PROVIDER", provider);
        match live {
            Some(v) => std::env::set_var("LIVE_CUSTODY", v),
            None => std::env::remove_var("LIVE_CUSTODY"),
        }
        let deployment = CustodyDeployment::resolve_from_env();
        let health = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("test runtime")
            .block_on(CustodyBoundaryHealth::probe(&deployment));
        match prev_provider {
            Some(v) => std::env::set_var("CUSTODY_PROVIDER", v),
            None => std::env::remove_var("CUSTODY_PROVIDER"),
        }
        match prev_live {
            Some(v) => std::env::set_var("LIVE_CUSTODY", v),
            None => std::env::remove_var("LIVE_CUSTODY"),
        }
        (health, deployment)
    }

    #[test]
    fn local_default_is_ready() {
        let (health, deployment) = health_for("local", None);
        assert_eq!(health.status, CustodyBoundaryStatus::Ready);
        assert!(health.status.allows_signing());
        assert_eq!(deployment.active, ProviderType::Local);
        assert!(health.to_safe_json()["allows_signing"].as_bool().unwrap());
    }

    #[test]
    fn remote_without_opt_in_is_not_opted_in() {
        std::env::remove_var("VAULT_ADDR");
        std::env::remove_var("VAULT_TOKEN");
        let (health, _) = health_for("vault", None);
        assert_eq!(health.status, CustodyBoundaryStatus::NotOptedIn);
        assert!(!health.status.allows_signing());
        assert!(health.probe.contains("LIVE_CUSTODY"));
    }

    #[test]
    fn remote_vault_with_refs_but_unreachable_backend_is_unreachable() {
        // NOTE: no ENV_LOCK here — `health_for` takes it itself.
        // With the §G real transit adapter, references being present no
        // longer means "integration absent": the adapter actually probes
        // Vault. An unreachable Vault is an honest runtime Unreachable —
        // never a fake Ready, and never a stale ConfiguredUnsupported.
        std::env::set_var("VAULT_ADDR", "http://127.0.0.1:1");
        std::env::set_var("VAULT_TOKEN", "hvs.token");
        let (health, _) = health_for("vault", Some("1"));
        assert_eq!(health.status, CustodyBoundaryStatus::Unreachable);
        assert!(!health.status.allows_signing());
        assert!(health
            .dependency
            .as_deref()
            .unwrap_or_default()
            .contains("transit"));
        std::env::remove_var("VAULT_ADDR");
        std::env::remove_var("VAULT_TOKEN");
    }

    #[test]
    fn remote_kms_without_credentials_is_configured_unsupported() {
        // NOTE: no ENV_LOCK here — `health_for` takes it itself.
        // Missing AWS credentials are a permanent deployment gap for the
        // kms adapter — NotConfigured maps to ConfiguredUnsupported.
        std::env::set_var("KMS_KEY_ID", "1234abcd-12ab-34cd-56ef-1234567890ab");
        std::env::remove_var("AWS_ACCESS_KEY_ID");
        std::env::remove_var("AWS_SECRET_ACCESS_KEY");
        let (health, _) = health_for("kms", Some("1"));
        assert_eq!(health.status, CustodyBoundaryStatus::ConfiguredUnsupported);
        assert!(!health.status.allows_signing());
        // The refusal names the exact credential dependency.
        assert!(health
            .dependency
            .as_deref()
            .unwrap_or_default()
            .contains("AWS_ACCESS_KEY_ID"));
        std::env::remove_var("KMS_KEY_ID");
    }

    #[test]
    fn remote_without_refs_is_missing_references() {
        std::env::remove_var("KMS_KEY_ID");
        let (health, _) = health_for("kms", Some("1"));
        assert_eq!(health.status, CustodyBoundaryStatus::MissingReferences);
        assert!(health
            .dependency
            .as_deref()
            .unwrap_or_default()
            .contains("KMS_KEY_ID"));
    }

    #[test]
    fn health_json_never_carries_secrets() {
        std::env::remove_var("VAULT_ADDR");
        std::env::remove_var("VAULT_TOKEN");
        let (health, _) = health_for("vault", None);
        let json = health.to_safe_json().to_string();
        assert!(!json.contains("hvs"));
        assert!(!json.contains("VAULT_TOKEN="));
    }
}
