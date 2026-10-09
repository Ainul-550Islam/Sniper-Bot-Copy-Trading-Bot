//! Closed repository error vocabulary (PROMPT 3/10 §A5).
//!
//! Every tenant trading-repository operation returns
//! [`RepositoryError`]. The vocabulary is CLOSED on purpose: callers
//! match on the kind, never on string matching, and the variants
//! separate the four failure families the data plane can produce:
//!
//! * [`RepositoryError::TenantMismatch`] — a row (or claim) that came
//!   back does not belong to the acting tenant: the runtime ownership
//!   assertion tripped. This is a SECURITY event, never a retry.
//! * [`RepositoryError::NotFound`] — tenant-safe miss (see
//!   [`super::not_found`]).
//! * [`RepositoryError::Conflict`] — the ON CONFLICT path decided
//!   against the caller (duplicate business key for this tenant).
//! * [`RepositoryError::StaleWrite`] — a guarded update (CAS on status
//!   / version / epoch) matched zero rows because the row moved on.
//! * [`RepositoryError::Validation`] — the caller's own inputs were
//!   rejected before SQL ran (blank id, non-positive limit, …).
//! * [`RepositoryError::Storage`] — the database itself (error or
//!   client-side timeout), with the underlying cause preserved for
//!   logs but never for API responses.

use crate::db::TimedDbError;

/// The closed error vocabulary of the tenant trading repositories.
#[derive(Debug, Clone)]
pub enum RepositoryError {
    /// A returned/attempted row does not belong to the acting tenant.
    TenantMismatch,
    /// Tenant-safe miss: the resource kind, nothing else.
    NotFound(&'static str),
    /// The conflict path refused the write (duplicate business key).
    Conflict(&'static str),
    /// A guarded update lost its race (status/version/epoch moved on).
    StaleWrite(&'static str),
    /// Caller input rejected before SQL ran.
    Validation(&'static str),
    /// The storage layer failed or timed out.
    Storage(String),
}

impl RepositoryError {
    /// Stable machine-readable label (metrics/API mapping).
    pub fn as_str(&self) -> &'static str {
        match self {
            RepositoryError::TenantMismatch => "tenant_mismatch",
            RepositoryError::NotFound(_) => "not_found",
            RepositoryError::Conflict(_) => "conflict",
            RepositoryError::StaleWrite(_) => "stale_write",
            RepositoryError::Validation(_) => "validation",
            RepositoryError::Storage(_) => "storage",
        }
    }

    /// True when the caller should treat this as retryable (transient
    /// storage trouble). Security and correctness errors never are.
    pub fn is_retryable(&self) -> bool {
        matches!(self, RepositoryError::Storage(_))
    }
}

impl std::fmt::Display for RepositoryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RepositoryError::TenantMismatch => {
                write!(f, "the resource belongs to another tenant")
            }
            RepositoryError::NotFound(what) => {
                write!(f, "{what} not found for the acting tenant")
            }
            RepositoryError::Conflict(what) => {
                write!(f, "{what} conflicts with an existing record of this tenant")
            }
            RepositoryError::StaleWrite(what) => {
                write!(f, "{what} moved on before the write; re-read and retry")
            }
            RepositoryError::Validation(what) => write!(f, "invalid {what}"),
            RepositoryError::Storage(cause) => write!(f, "storage failure: {cause}"),
        }
    }
}

impl std::error::Error for RepositoryError {}

impl From<TimedDbError> for RepositoryError {
    fn from(e: TimedDbError) -> Self {
        RepositoryError::Storage(e.to_string())
    }
}

impl From<sqlx::Error> for RepositoryError {
    fn from(e: sqlx::Error) -> Self {
        RepositoryError::Storage(e.to_string())
    }
}

impl From<RepositoryError> for crate::error::BotError {
    fn from(e: RepositoryError) -> Self {
        crate::error::BotError::db(e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_are_stable_and_closed() {
        assert_eq!(RepositoryError::TenantMismatch.as_str(), "tenant_mismatch");
        assert_eq!(RepositoryError::NotFound("order").as_str(), "not_found");
        assert_eq!(
            RepositoryError::Conflict("idempotency_key").as_str(),
            "conflict"
        );
        assert_eq!(
            RepositoryError::StaleWrite("status").as_str(),
            "stale_write"
        );
        assert_eq!(
            RepositoryError::Validation("order_id").as_str(),
            "validation"
        );
        assert_eq!(RepositoryError::Storage("boom".into()).as_str(), "storage");
    }

    #[test]
    fn only_storage_is_retryable() {
        assert!(RepositoryError::Storage("x".into()).is_retryable());
        assert!(!RepositoryError::TenantMismatch.is_retryable());
        assert!(!RepositoryError::NotFound("order").is_retryable());
        assert!(!RepositoryError::Conflict("k").is_retryable());
        assert!(!RepositoryError::StaleWrite("s").is_retryable());
        assert!(!RepositoryError::Validation("v").is_retryable());
    }

    #[test]
    fn timed_db_errors_become_storage() {
        let err: RepositoryError = TimedDbError::Timeout.into();
        assert!(matches!(err, RepositoryError::Storage(_)));
        assert!(err.is_retryable());
    }
}
