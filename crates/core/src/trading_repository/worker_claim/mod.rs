//! Tenant-scoped worker claim lanes (PROMPT 3/10 §H48–H51).
//!
//! Backed by the 0032 `worker_claims` table — one lane per
//! `(organization_id, purpose)`. The global `ha_leases` plane stays
//! global; these lanes are the tenant data plane's leadership.

pub mod acquire;
pub mod model;
pub mod recovery;
pub mod release;

pub use acquire::TenantWorkerClaimRepo;
pub use model::{ClaimDecision, TenantWorkClaim};
