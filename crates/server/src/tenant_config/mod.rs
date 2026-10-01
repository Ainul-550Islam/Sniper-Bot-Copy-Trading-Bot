//! Tenant configuration engine (STEP 3 files 29–37).
//!
//! | file | concern |
//! |---|---|
//! | `model.rs` | the typed tenant document ([`TenantConfigModel`]) |
//! | `resolver.rs` | the precedence ladder: platform bounds → tenant → runtime → operation ([`resolve`]) |
//! | `validator.rs` | pure validation with a closed issue vocabulary ([`ConfigIssue`]) |
//! | `version.rs` | document versions + optimistic concurrency ([`ConfigVersion`], [`ConfigRecord`]) |
//! | `store.rs` | durable `tenant_configs` access + CAS writes ([`ConfigStore`]) |
//! | `cache.rs` | per-process read cache with drift sweep ([`ConfigCache`]) |
//! | `diff.rs` | machine-readable change detection ([`ConfigDiff`]) |
//! | `audit.rs` | append-only audit trail ([`ConfigAuditEntry`]) |
//!
//! Design invariants (each enforced somewhere specific):
//!
//! 1. **No secrets in tenant documents.** The model has no field that
//!    can carry one (validator test `model_round_trips_through_json`
//!    asserts the serialized shape).
//! 2. **Tenants only narrow.** The resolver takes the tightest bound
//!    along the ladder; the validator refuses documents that try to
//!    exceed platform bounds rather than silently clamping them.
//! 3. **No clobbered writes.** Every update compare-and-swaps on the
//!    document version ([`ConfigWriteError::StaleVersion`]).
//! 4. **Everything audited.** A write that cannot be audited does not
//!    happen; the audit payload is the typed diff, never the raw blob.
//! 5. **Fail closed.** An unknown tenant resolves to platform defaults
//!    with paper-only modes — never to an open configuration.

pub mod audit;
pub mod cache;
pub mod diff;
pub mod model;
pub mod resolver;
pub mod store;
pub mod validator;
pub mod version;

pub use audit::{
    entry_for, ConfigAuditEntry, ConfigAuditSink, MemoryConfigAuditSink, PgConfigAuditSink,
};
pub use cache::{CacheOrigin, ConfigCache};
pub use diff::{Change, ConfigDiff};
pub use model::{TenantConfigModel, TenantRiskLimits, TenantSignerPreference};
pub use resolver::{
    resolve, EffectiveTenantConfig, GlobalSafetyBounds, OperationConstraints, RuntimeOverrides,
};
pub use store::{ConfigStore, ConfigWriteError, MemoryConfigStore, PgConfigStore};
pub use validator::{validate, validate_or_issues, ConfigIssue};
pub use version::{ConfigRecord, ConfigVersion};
