//! HashiCorp Vault custody integration (§G, spec files 48–52).
//!
//! A REAL Vault transit-engine integration built on the workspace's
//! `reqwest` dependency — Vault's transit API is plain REST + JSON, so no
//! vendor SDK is required. The module layout:
//!
//! * [`config`] — reference-only configuration (`VAULT_ADDR`,
//!   `VAULT_TOKEN`, `VAULT_TRANSIT_MOUNT`, `VAULT_TRANSIT_KEY`); the
//!   token is held in a redacted wrapper and never rendered anywhere;
//! * [`client`] — the HTTP client wrapper: `sys/health`,
//!   `token/lookup-self`, `transit/keys/{name}`, `transit/sign/{name}`;
//! * [`signer`] — `VaultSigner` (implements the core `CustodySigner`
//!   trait; every signature comes from a real Vault response) and the
//!   server-side `VaultCustodyProvider` adapter with full fail-closed
//!   resolution checks (provider type, status, key reference, key type,
//!   public-key match);
//! * [`health`] — the ordered connectivity → permission → readiness
//!   probe and its `ProviderHealth` mapping.
//!
//! What this integration will never do: fabricate a signature, fall back
//! to a local key, expose the service token, or report healthy without
//! having talked to Vault. When Vault is unreachable or misconfigured,
//! every path fails closed with the exact missing dependency
//! ([`client::VAULT_TRANSIT_DEPENDENCY`]).

pub mod client;
pub mod config;
pub mod health;
pub mod signer;

pub use client::{
    parse_vault_signature, TransitKeyInfo, VaultClient, VaultClientError, VaultServiceState,
    VAULT_TRANSIT_DEPENDENCY,
};
pub use config::{VaultConfig, VaultReferenceState, VaultToken};
pub use health::{check_vault_readiness, deployment_readiness, VaultReadiness};
pub use signer::{VaultCustodyProvider, VaultSigner};
