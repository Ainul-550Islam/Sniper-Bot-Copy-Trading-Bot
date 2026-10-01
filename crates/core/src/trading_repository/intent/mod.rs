//! Tenant-scoped execution-intent journal and recovery
//! (PROMPT 3/10 §E30–E34).

pub mod model;
pub mod read;
pub mod recovery;
pub mod write;

pub use model::TenantIntent;
pub use read::TenantIntentRead;
pub use recovery::{TenantRecoveryItem, TenantRecoveryRepo};
pub use write::TenantIntentWrite;
