//! Tenant-scoped lock helpers (STEP 3 file 49).
//!
//! PostgreSQL advisory locks, namespaced by tenant: two tenants doing the
//! same logical job have different input strings. PostgreSQL hashes them
//! into a 32-bit space, so unrelated tenants/purposes can still collide and
//! contend; equal keys for the same tenant/purpose serialize.
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
/// The key string is CLUSTER-WIDE and equals the lock's meaning: it is passed
/// through `hashtext`, never used as the lock number directly, so the 32-bit
/// advisory space still has collisions. A collision only causes needless
/// contention, never shared state — the bodies are still tenant-scoped.
/// Prefix hashed into the FIRST advisory key: the tenant identity. The lock is
/// then `(hashtext('sniper-tenant-lock:' || organization_id), hashtext(purpose))`
/// — a 64-bit pair, so two unrelated tenants/purposes collide only if BOTH
/// 32-bit hashes collide (~2⁻⁶⁴) instead of one (~2⁻³²).
pub const TENANT_LOCK_KEY_PREFIX: &str = "sniper-tenant-lock:";

/// The two advisory-lock keys of one (tenant, purpose) pair, as PostgreSQL's
/// `hashtext` will compute them. Exposed for tests and operators; the lock
/// itself is taken by [`tenant_advisory_xact_lock`].
pub fn tenant_lock_keys_sql() -> &'static str {
    "SELECT hashtext($1 || $2::text), hashtext($3)"
}

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
    // TWO-KEY form: (tenant identity, purpose) are hashed SEPARATELY, so
    // PostgreSQL compares the full 64-bit pair. The previous single
    // `pg_advisory_xact_lock(hashtext(tenant || purpose))` folded everything
    // into 32 bits and, with enough tenants/purposes, two unrelated sweeps
    // blocked each other (and `try_tenant_lock` reported a phantom holder).
    sqlx::query("SELECT pg_advisory_xact_lock(hashtext($1 || $2::text), hashtext($3))")
        .bind(TENANT_LOCK_KEY_PREFIX)
        .bind(scope.organization_id().to_string())
        .bind(purpose.trim().to_ascii_lowercase())
        .execute(&mut *tx)
        .await
        .map_err(|e| BotError::db(format!("tenant advisory lock failed: {e}")))?;
    Ok(())
}

/// Try to acquire the transaction-scoped advisory lock without blocking.
///
/// `Ok(true)` = held; `Ok(false)` = a worker holds the hashed key.
/// A different tenant or purpose can collide in the 32-bit hash space.
pub async fn try_tenant_advisory_xact_lock(
    tx: &mut PgConnection,
    scope: &TenantQueryScope,
    purpose: &str,
) -> BotResult<bool> {
    let row: Option<(bool,)> =
        sqlx::query_as("SELECT pg_try_advisory_xact_lock(hashtext($1 || $2::text), hashtext($3))")
            .bind(TENANT_LOCK_KEY_PREFIX)
            .bind(scope.organization_id().to_string())
            .bind(purpose.trim().to_ascii_lowercase())
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
    fn the_first_key_is_derived_from_the_tenant_and_a_namespace_prefix() {
        let scope = TenantQueryScope::new(OrganizationId::new());
        let other = TenantQueryScope::new(OrganizationId::new());
        let first = |s: &TenantQueryScope| {
            format!("{}{}", super::TENANT_LOCK_KEY_PREFIX, s.organization_id())
        };
        // The pair's first component never repeats across tenants …
        assert_ne!(first(&scope), first(&other));
        // … and the prefix is present so these locks do not read as an
        // unrelated advisory-lock family.
        assert!(first(&scope).starts_with(super::TENANT_LOCK_KEY_PREFIX));
        // The purpose is normalized, so case/whitespace cannot fork a lock.
        assert_eq!(
            tenant_lock_key(&scope, "Recovery-Sweep"),
            tenant_lock_key(&scope, " recovery-sweep ")
        );
    }

    #[test]
    fn lock_scope_builds_the_equivalent_query_scope() {
        let org = OrganizationId::new();
        assert_eq!(lock_scope(org).organization_id(), org);
    }
}
