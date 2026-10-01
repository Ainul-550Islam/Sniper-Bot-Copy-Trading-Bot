//! Tenant-scoped execution event streams (STEP 3 files 45–48).
//!
//! A typed, per-tenant fan-out layer for execution decisions: what the
//! gateway allowed or denied, what the engines executed, what the
//! runtime registry did. The existing SaaS websocket
//! (`security/websocket.rs`) delivers JSON frames to browsers; this
//! module is the typed HUB behind such consumers — one broadcast
//! channel per tenant, strict organization scoping in the filter, and
//! no global firehose anywhere:
//!
//! | file | concern |
//! |---|---|
//! | `events.rs` | the typed [`events::TenantEvent`] envelope |
//! | `hub.rs` | per-tenant channels ([`hub::TenantStreamHub`]) |
//! | `filter.rs` | subscription filters with mandatory org scoping ([`filter::TenantStreamFilter`]) |
//! | `subscription_scope.rs` | [`subscription_scope::SubscriptionScope`] — a consumer's tenant-scoped subscription identity |
//!
//! Invariants:
//!
//! * An event NEVER crosses tenants: the filter checks the
//!   organization id before any delivery, and a subscriber without a
//!   tenant scope subscribes to nothing.
//! * Publishing is fire-and-forget and non-blocking: a slow subscriber
//!   lags (bounded buffer, recv lagged) and never delays execution.

pub mod events;
pub mod filter;
pub mod hub;
pub mod subscription_scope;

pub use events::TenantEvent;
pub use filter::TenantStreamFilter;
pub use hub::TenantStreamHub;
pub use subscription_scope::SubscriptionScope;
