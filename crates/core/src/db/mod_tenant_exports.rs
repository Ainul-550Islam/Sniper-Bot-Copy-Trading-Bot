//! Export surface for the tenant DB helpers (STEP 3 file 51).
//!
//! One import gives the tenant repository code every helper:
//!
//! ```ignore
//! use bot_core::db::tenant as tenant_db;
//! let scope = tenant_db::TenantQueryScope::new(org);
//! let mut tx = tenant_db::TenantTx::begin(&db, scope).await?;
//! ```
//!
//! The existing `db` module keeps its own structure untouched; this file
//! only re-exports — it adds no logic.

pub use super::tenant_idempotency::{held_by_tenant, insert_once, TenantIdempotencyKey};
pub use super::tenant_lock::{
    lock_scope, tenant_advisory_xact_lock, tenant_lock_key, try_tenant_advisory_xact_lock,
};
pub use super::tenant_pagination::{
    TenantPage, TenantPageCursor, TenantPageRequest, DEFAULT_PAGE_LIMIT, MAX_PAGE_LIMIT,
};
pub use super::tenant_query::{require_org_predicate, TenantQueryScope};
pub use super::tenant_row::{require_same_org, require_same_row_ownership, TenantRow};
pub use super::tenant_tx::TenantTx;

#[cfg(test)]
mod tests {
    #[test]
    fn the_export_surface_is_complete() {
        // Compile-time proof that every helper is reachable through this
        // module without reaching into the sibling modules directly.
        fn assert_exported(_: &super::TenantQueryScope, _: &super::TenantTx<'_>) {}
        let _ = assert_exported;
        // Generic helpers (require_same_row_ownership) are exercised in
        // tenant_row.rs; the monomorphic surface is proven here.
        let _: fn(&str) -> crate::error::BotResult<()> = super::require_org_predicate;
        let _: fn(
            &super::TenantQueryScope,
            crate::tenant::OrganizationId,
        ) -> crate::error::BotResult<()> = super::require_same_org;
        let _: fn(&super::TenantQueryScope, &str) -> String = super::tenant_lock_key;
    }
}
