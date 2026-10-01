//! Tenant-scoped trading repositories (PROMPT 3/10 — STEP 10–14 data-plane
//! closure).
//!
//! This module is the REAL data plane of the tenant-isolation program:
//! every SELECT/UPDATE/DELETE/UPSERT against the trading-truth tables
//! (`orders`, `order_status_history`, `executions`, `transactions`,
//! `idempotency_keys`, `dedup_keys`, `positions`, `trades`,
//! `balance_snapshots`, `execution_intents`, `execution_claims`,
//! `execution_lifecycle`, `copy_*`, `poly_*`, `ledger_*`,
//! `global_positions`, `reconciliation_state`) executes with the acting
//! [`OrganizationId`] in the SQL predicate itself — never as a
//! post-load filter in Rust.
//!
//! Layering:
//!
//! * **Where the tenant comes from.** A repository call receives a
//!   [`query_scope::TradingQueryScope`] (reads) or a
//!   [`write_scope::TenantWriteScope`] (writes). Both are built from an
//!   AUTHENTICATED context (session / tenant API key / telegram identity /
//!   operator-authorized explicit selector, resolved by the server's
//!   `trading_data_plane`), never from a user-supplied header value
//!   directly. [`write_scope::TenantWriteScope`] additionally records WHO
//!   is writing (actor) so every mutation is attributable.
//! * **The transaction boundary.** [`transaction::TradingTransaction`]
//!   wraps [`crate::db::tenant_tx::TenantTx`] — a transaction that cannot
//!   be opened without a scope.
//! * **Errors.** [`repository_error::RepositoryError`] is the closed
//!   vocabulary; [`not_found`] maps misses to a tenant-safe `NotFound`
//!   that never reveals whether the resource exists for another tenant.
//! * **Row ownership.** [`tenant_assert`] re-verifies that rows RETURNED
//!   by a query carry the acting organization (defense in depth behind
//!   the SQL predicate).
//! * **Pagination.** [`pagination`] re-exports the STEP 3 keyset
//!   primitives and adds the (organization_id, ts, sort_key) cursor SQL
//!   fragment used by the list queries.
//!
//! Legacy / deployment-global repositories (`db::repo`, `db::claims`, …)
//! remain the single-deployment operator plane; after the STEP 10–14
//! atomic swaps (migrations 0026–0034) they bind the deployment
//! organization explicitly through `public.deployment_organization_id()`,
//! so no writer is tenant-blind. Operator-global scans that are
//! intentionally privileged (HA worker views, runtime flags, recovery
//! checkpoints) stay global BY TYPE and are never exposed as tenant API
//! data.

pub mod copy;
pub mod executions;
pub mod intent;
pub mod not_found;
pub mod orders;
pub mod pagination;
pub mod polymarket;
pub mod positions;
pub mod query_scope;
pub mod reporting;
pub mod repository_error;
pub mod tenant_assert;
pub mod transaction;
pub mod worker_claim;
pub mod write_scope;

pub use not_found::tenant_not_found;
pub use pagination::{page_where_clause, MAX_PAGE_LIMIT};
pub use query_scope::TradingQueryScope;
pub use repository_error::RepositoryError;
pub use transaction::TradingTransaction;
pub use write_scope::{TenantWriteScope, WriteOrigin};

/// One page of tenant-verified rows plus the continuation cursor.
pub use crate::db::tenant_pagination::{TenantPage, TenantPageCursor, TenantPageRequest};

/// The acting organization every query in this module is scoped to.
pub use crate::tenant::OrganizationId;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn module_surface_is_reachable() {
        // The public surface composes: a scope, a write scope, an error
        // and the pagination fragment.
        let org = OrganizationId::new();
        let scope = TradingQueryScope::new(org);
        assert_eq!(scope.organization_id(), org);
        let write = TenantWriteScope::new(org, "user:1", WriteOrigin::Http).expect("write scope");
        assert_eq!(write.organization_id(), org);
        assert_eq!(RepositoryError::TenantMismatch.as_str(), "tenant_mismatch");
        assert!(page_where_clause(2).contains("organization_id = $2"));
    }
}
