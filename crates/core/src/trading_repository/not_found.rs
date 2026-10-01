//! Tenant-safe not-found mapping (PROMPT 3/10 §A4).
//!
//! A miss on a tenant-scoped lookup must be indistinguishable from
//! "the resource does not exist" — never "it exists but belongs to
//! someone else" (existence is itself cross-tenant information). Every
//! repository `get_*` maps a `None` row through [`tenant_not_found`],
//! which yields [`crate::trading_repository::RepositoryError::NotFound`]
//! carrying the RESOURCE KIND only (never an id of another tenant, never
//! a distinguishing message).

use super::repository_error::RepositoryError;

/// The kind of resource a lookup missed (single stable word, no ids).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ResourceKind {
    Order,
    OrderStatusEntry,
    Execution,
    Transaction,
    Position,
    Trade,
    BalanceSnapshot,
    Intent,
    Claim,
    Lifecycle,
    CopyLeader,
    CopyEvent,
    CopyLink,
    PolySignal,
    PolyOrder,
    PolyFill,
    PolyFinding,
    ReconState,
}

impl ResourceKind {
    /// Stable machine-readable label (log/metrics; no tenant data).
    pub fn as_str(&self) -> &'static str {
        match self {
            ResourceKind::Order => "order",
            ResourceKind::OrderStatusEntry => "order_status_entry",
            ResourceKind::Execution => "execution",
            ResourceKind::Transaction => "transaction",
            ResourceKind::Position => "position",
            ResourceKind::Trade => "trade",
            ResourceKind::BalanceSnapshot => "balance_snapshot",
            ResourceKind::Intent => "intent",
            ResourceKind::Claim => "claim",
            ResourceKind::Lifecycle => "lifecycle",
            ResourceKind::CopyLeader => "copy_leader",
            ResourceKind::CopyEvent => "copy_event",
            ResourceKind::CopyLink => "copy_link",
            ResourceKind::PolySignal => "poly_signal",
            ResourceKind::PolyOrder => "poly_order",
            ResourceKind::PolyFill => "poly_fill",
            ResourceKind::PolyFinding => "poly_finding",
            ResourceKind::ReconState => "recon_state",
        }
    }
}

/// Map a lookup miss to the tenant-safe not-found error.
///
/// `Ok(Some(row))` passes through; `Ok(None)` becomes
/// `Err(RepositoryError::NotFound(kind))` — one indistinguishable
/// response for "absent" and "owned by another tenant" alike.
pub fn tenant_not_found<T>(kind: ResourceKind, found: Option<T>) -> Result<T, RepositoryError> {
    match found {
        Some(v) => Ok(v),
        None => Err(RepositoryError::NotFound(kind.as_str())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn none_maps_to_kind_only_not_found() {
        let err = tenant_not_found(ResourceKind::Order, Option::<u8>::None).unwrap_err();
        match err {
            RepositoryError::NotFound(what) => assert_eq!(what, "order"),
            other => panic!("unexpected error: {other:?}"),
        }
        // The display never carries another tenant's ids.
        assert_eq!(err.to_string(), "order not found for the acting tenant");
    }

    #[test]
    fn some_passes_through() {
        assert_eq!(tenant_not_found(ResourceKind::Trade, Some(7u8)).unwrap(), 7);
    }
}
