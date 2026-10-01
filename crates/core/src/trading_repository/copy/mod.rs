//! Tenant-scoped copy trading repositories (PROMPT 3/10 §F36–F40).

pub mod events;
pub mod model;
pub mod read;
pub mod write;

pub use events::TenantCopyEventRepo;
pub use model::{TenantCopyEvent, TenantCopyLink, TenantLeader, TenantLeaderEvent};
pub use read::TenantCopyRead;
pub use write::TenantCopyWrite;
