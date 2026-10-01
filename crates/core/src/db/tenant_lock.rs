//! Tenant-scoped lock helpers (STEP 3 file 49).
//!
//! PostgreSQL advisory locks, namespaced by tenant: two tenants doing the
//! same logical job (recovery sweep for wallet X, recon for venue Y)
//! never contend, while two workers of the SAME tenant still serialize
//! exactly as the existing global design requires.
//!
//! The transaction-scoped form (`pg_advisory_xact_lock`) releases
//! automatically at commit/rollback — the same discipline the existing
//! repositories use for their guarded writes.

use sqlx::PgConnection;

use crate::error::{BotError, BotResult};
use crate::tenant::OrganizationId;

use super::tenant_query::TenantQueryScope;

/// Build the advisory-lock key for one (tenant, purpose) pair.
///
/// The key is hashed by PostgreSQL (`hashtext`) into the 32-bit advisory
/// space; the human-readable string keeps different purposes from
/// colliding while the tenant prefix keeps tenants independent.
pub fn tenant_lock_key(scope: &TenantQueryScope, purpose: &str) -> String {
    format!(
        "tenant:{}:{}",
        scope.organization_id(),
        purpose.trim().to_ascii_lowercase()
    )
}

/// Acquire the transaction-scoped advisory lock for (tenant, purpose).
///
/// Blocks until held. MUST run inside the transaction it guards — the
/// lock evaporates at its commit/rollback.
pub async fn tenant_advisory_xact_lock(
    tx: &mut PgConnection,
    scope: &TenantQueryScope,
    purpose: &str,
) -> BotResult<()> {
    let key = tenant_lock_key(scope, purpose);
    sqlx::query("SELECT pg_advisory_xact_lock(hashtext($1))")
        .bind(&key)
        .execute(&mut *tx)
        .await
        .map_err(|e| BotError::db(format!("tenant advisory lock failed: {e}")))?;
    Ok(())
}

/// Try to acquire the transaction-scoped advisory lock without blocking.
///
/// `Ok(true)` = held; `Ok(false)` = another worker of the same tenant
/// holds it (skip, don't wait — the HA design prefers one worker to
/// proceed while others move on).
pub async fn try_tenant_advisory_xact_lock(
    tx: &mut PgConnection,
    scope: &TenantQueryScope,
    purpose: &str,
) -> BotResult<bool> {
    let key = tenant_lock_key(scope, purpose);
    let row: Option<(bool,)> = sqlx::query_as("SELECT pg_try_advisory_xact_lock(hashtext($1))")
        .bind(&key)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|e| BotError::db(format!("tenant advisory try-lock failed: {e}")))?;
    Ok(row.map(|(held,)| held).unwrap_or(false))
}

/// Convenience: a one-tenant scope for lock callers that have not built
/// a [`TenantQueryScope`] yet. Prefer building the scope at the call site.
pub fn lock_scope(organization_id: OrganizationId) -> TenantQueryScope {
    TenantQueryScope::new(organization_id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_are_tenant_and_purpose_scoped() {
        let a = TenantQueryScope::new(OrganizationId::new());
        let b = TenantQueryScope::new(OrganizationId::new());
        assert_ne!(
            tenant_lock_key(&a, "recovery-sweep"),
            tenant_lock_key(&b, "recovery-sweep")
        );
        assert_ne!(
            tenant_lock_key(&a, "recovery-sweep"),
            tenant_lock_key(&a, "recon")
        );
        // Purpose is normalized so case/whitespace does not fork keys.
        assert_eq!(
            tenant_lock_key(&a, "Recovery-Sweep"),
            tenant_lock_key(&a, " recovery-sweep ")
        );
    }

    #[test]
    fn lock_scope_builds_the_equivalent_query_scope() {
        let org = OrganizationId::new();
        assert_eq!(lock_scope(org).organization_id(), org);
    }
}
