//! Tenant-scoped observability (STEP 3 files 53–56 + the safe-fields
//! layer).
//!
//! The platform-wide metrics and audit trail stay exactly as they are
//! (`obs.rs`, the core audit layer). This module adds the TENANT slice:
//! per-tenant decision counters, a decision log with the gateway's
//! vocabulary, and a health roll-up (runtime liveness + recent
//! decisions) that the dashboard shows per organization.
//!
//! | file | concern |
//! |---|---|
//! | `metrics.rs` | per-tenant decision/execution counters ([`TenantMetrics`]) |
//! | `decision_log.rs` | the append-only decision log ([`DecisionLogEntry`], PG + memory) |
//! | `health.rs` | per-tenant health roll-up ([`TenantHealth`]) |
//! | `fields.rs` | safe structured log fields ([`TenantLogFields`]) |
//! | `redaction.rs` | tenant-safe redaction rules ([`Redaction`]) |
//! | `audit_context.rs` | tenant context → audit correlation ([`AuditContext`]) |
//!
//! Security invariants:
//!
//! * Log fields may carry organization/runtime/module identities and
//!   correlation ids — never seeds, private keys, bearer tokens,
//!   signer secrets or provider credentials.
//! * Every redaction decision goes through [`Redaction`] — one rule
//!   set, applied everywhere.
//! * Audit entries correlate the FULL context (tenant + principal +
//!   origin), so a deny in the trail can always be traced back to
//!   the entry path that produced it.

pub mod audit_context;
pub mod decision_log;
pub mod fields;
pub mod health;
pub mod metrics;
pub mod redaction;

pub use audit_context::AuditContext;
pub use decision_log::{
    DecisionLogEntry, DecisionLogSink, MemoryDecisionLogSink, PgDecisionLogSink,
};
pub use fields::{TenantLogField, TenantLogFields};
pub use health::{HealthStatus, TenantHealth, TenantHealthRollup};
pub use metrics::TenantMetrics;
pub use redaction::Redaction;
