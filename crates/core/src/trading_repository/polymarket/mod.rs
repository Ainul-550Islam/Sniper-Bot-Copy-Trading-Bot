//! Tenant-scoped polymarket repositories (PROMPT 3/10 §G42–G46).

pub mod model;
pub mod read;
pub mod reconciliation;
pub mod write;

pub use model::{TenantPolyFill, TenantPolyOrder, TenantPolyReconFinding, TenantPolySignal};
pub use read::TenantPolyRead;
pub use reconciliation::{TenantPolyDrift, TenantPolyReconRepo};
pub use write::TenantPolyWrite;
