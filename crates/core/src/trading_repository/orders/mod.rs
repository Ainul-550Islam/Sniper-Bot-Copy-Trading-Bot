//! Tenant-scoped orders repository (PROMPT 3/10 §B9–B13).
//!
//! Exports the model, the read side, the write side and the conflict
//! audit. The legacy deployment repositories (`db::repo::OrderRepo`)
//! remain for the single-deployment operator plane; after the 0026
//! swap they bind `public.deployment_organization_id()` and name the
//! same composite arbiter.

pub mod conflicts;
pub mod model;
pub mod read;
pub mod write;

pub use conflicts::{key_is_taken, ORDERS_ID_ARBITER, ORDERS_TENANT_ARBITER};
pub use model::{TenantOrder, TenantOrderStatusEntry};
pub use read::TenantOrderRead;
pub use write::TenantOrderWrite;
