//! Tenant custody subsystem entrypoint (BATCH file 05).
//!
//! Canonical boundary: SaaS tenant -> custody profile -> signer provider -> TransactionSigner.
//! Trading engines must not access raw secrets through this subsystem.

pub mod credentials;
pub mod health;
pub mod model;
pub mod policy;
pub mod provider;
pub mod provider_config;
pub mod resolve;
pub mod rotation;

pub use health::{CustodyHealthReport, HealthState, ProviderHealth};
pub use model::{
    CustodyProfile, CustodyProfileId, CustodyStatus, ProviderType, SignerId, SignerRecord,
};
pub use policy::{check as check_custody, CustodyDenyReason, CustodyRequest, CustodyVerdict};
pub use provider::{
    resolve_active_signer, CustodyProvider, CustodyProviderError, CustodyProviderRegistry,
    CustodySigner, HsmCustodyProvider, KmsCustodyProvider, LocalCustodyProvider, ResolvedSigner,
    VaultCustodyProvider,
};
pub use resolve::{check_resolve, resolve_signer_handle, ResolveDenyReason, ResolveRequest};
