//! Custody subsystem — the key-custody boundary of the server (§F).
//!
//! Two layers, one rule: **keys never widen their blast radius**.
//!
//! * `bot-core::custody` owns the domain: providers (local/vault/kms/hsm),
//!   profiles, signer records, policy, resolution, health, rotation.
//! * This module owns the server boundary: the deployment posture
//!   (`provider_registry`), the sign request/response contract
//!   (`sign_request` / `sign_response`), the guard-ordered sign executor
//!   (`sign_boundary`), boundary health (`health`), and the audit trail
//!   (`audit`).
//! * `live_provider_contract` / `live_provider_fixture` (Batch 7) remain
//!   the ops-surface contract and the NON-LIVE test fixture contract
//!   respectively.
//!
//! Invariants enforced here, not merely documented:
//!
//! * no private key material ever enters a request, response, audit row,
//!   or log line;
//! * a remote-bound signer is never satisfied by a local wallet — there
//!   is no local fallback;
//! * every outcome (signed or refused) is audited with a machine-readable
//!   code;
//! * unimplemented providers refuse with the EXACT deployment dependency
//!   they need — nothing pretends to be live (OPTION B posture);
//! * the single-operator deployment's existing wallet signing path is
//!   untouched — this boundary is additive.

pub mod audit;
pub mod health;
pub mod kms;
pub mod live_provider_contract;
pub mod live_provider_fixture;
pub mod provider_registry;
pub mod sign_boundary;
pub mod sign_request;
pub mod sign_response;
pub mod vault;

pub use audit::{CustodyAuditLog, CustodyAuditOutcome, CustodyAuditRecord};
pub use health::{CustodyBoundaryHealth, CustodyBoundaryStatus};
pub use provider_registry::{CustodyDeployment, ProviderAvailability};
pub use sign_boundary::CustodySignBoundary;
pub use sign_request::{CustodySignRequest, InvalidSignRequest};
pub use sign_response::{CustodySignResponse, RefusalReason};

/// Process-wide lock for tests that touch environment variables.
/// Custody env tests live in several modules of this one test binary;
/// a single shared lock keeps them from racing each other.
#[cfg(test)]
pub mod test_support {
    pub static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
}
