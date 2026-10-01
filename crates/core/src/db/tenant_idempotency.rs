//! Tenant-aware idempotency helpers (STEP 3 file 50).
//!
//! The `idempotency_keys` primary key is the TENANT-COMPOSITE
//! `(organization_id, scope, key)` since migration 0027 (PROMPT 3/10
//! STEP 10 atomic swap #2 — the constraint swap and this writer shipped
//! in the same batch). What this module enforces is the TENANT-SAFE
//! access discipline:
//!
//! * every insert/lookup binds `organization_id` alongside
//!   `(scope, key)` and names the composite as the ON CONFLICT arbiter;
//! * a second tenant reusing the same `(scope, key)` now INSERTS ITS
//!   OWN ROW (tenant-local business identity) — the transitional
//!   "already used" limitation of the pre-0027 global key is retired;
//! * the organization echo check below stays as defense in depth: the
//!   returned row must carry the acting tenant or the answer is false.

use sqlx::PgConnection;

use crate::error::{BotError, BotResult};
use crate::tenant::OrganizationId;

use super::tenant_query::TenantQueryScope;

/// A tenant-scoped view of one idempotency key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TenantIdempotencyKey {
    /// The tenant that owns the key.
    pub organization_id: OrganizationId,
    /// The existing scope vocabulary (unchanged: `telegram`, `api`, …).
    pub scope: String,
    /// The caller's unique key.
    pub key: String,
}

impl TenantIdempotencyKey {
    /// Build and validate (bounded lengths, no whitespace control chars).
    pub fn new(
        organization_id: OrganizationId,
        scope: impl Into<String>,
        key: impl Into<String>,
    ) -> BotResult<Self> {
        let scope = scope.into();
        let key = key.into();
        if scope.trim().is_empty() || scope.len() > 64 {
            return Err(BotError::db("idempotency scope must be 1..=64 chars"));
        }
        if key.trim().is_empty() || key.len() > 256 {
            return Err(BotError::db("idempotency key must be 1..=256 chars"));
        }
        Ok(TenantIdempotencyKey {
            organization_id,
            scope,
            key,
        })
    }

    /// Build from a query scope (the tenant comes from the scope).
    pub fn for_scope(
        query_scope: &TenantQueryScope,
        scope: impl Into<String>,
        key: impl Into<String>,
    ) -> BotResult<Self> {
        TenantIdempotencyKey::new(query_scope.organization_id(), scope, key)
    }
}

/// Insert-once for a tenant: `Ok(true)` when THIS tenant's key was fresh
/// and is now recorded; `Ok(false)` when THIS tenant already used it.
/// Another tenant holding the same `(scope, key)` is INVISIBLE — after
/// 0027 both rows coexist (tenant-local identity), and the echo check
/// below keeps that guarantee even if the arbiter regressed.
pub async fn insert_once(conn: &mut PgConnection, idem: &TenantIdempotencyKey) -> BotResult<bool> {
    let row: Option<(uuid::Uuid,)> = sqlx::query_as(
        "INSERT INTO idempotency_keys (organization_id, scope, key) \
         VALUES ($1, $2, $3) \
         ON CONFLICT (organization_id, scope, key) DO NOTHING \
         RETURNING organization_id",
    )
    .bind(idem.organization_id.as_uuid())
    .bind(&idem.scope)
    .bind(&idem.key)
    .fetch_optional(&mut *conn)
    .await
    .map_err(|e| BotError::db(format!("tenant idempotency insert failed: {e}")))?;
    match row {
        // Our row: the tenant attribution must echo back exactly ours.
        Some((org,)) if org == idem.organization_id.as_uuid() => Ok(true),
        // A row attributed to another tenant can no longer surface
        // after 0027 (the arbiter is tenant-composite); if it ever
        // does, fail closed: NOT ours.
        Some(_) => Ok(false),
        // Nothing returned: this tenant already used the key.
        None => Ok(false),
    }
}

/// Tenant-scoped lookup: is THIS tenant recorded for `(scope, key)`?
///
/// `Ok(true)` only when the stored row belongs to the acting tenant.
/// A different tenant's row answers `false` — existence of another
/// tenant's key is not leaked.
pub async fn held_by_tenant(
    conn: &mut PgConnection,
    idem: &TenantIdempotencyKey,
) -> BotResult<bool> {
    let row: Option<(uuid::Uuid,)> = sqlx::query_as(
        "SELECT organization_id FROM idempotency_keys \
         WHERE organization_id = $1 AND scope = $2 AND key = $3",
    )
    .bind(idem.organization_id.as_uuid())
    .bind(&idem.scope)
    .bind(&idem.key)
    .fetch_optional(&mut *conn)
    .await
    .map_err(|e| BotError::db(format!("tenant idempotency lookup failed: {e}")))?;
    Ok(row.is_some())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_validate_bounds() {
        let org = OrganizationId::new();
        assert!(TenantIdempotencyKey::new(org, "telegram", "cmd-1").is_ok());
        assert!(TenantIdempotencyKey::new(org, "", "k").is_err());
        assert!(TenantIdempotencyKey::new(org, "s", " ").is_err());
        assert!(TenantIdempotencyKey::new(org, "s", "k".repeat(257)).is_err());
        assert!(TenantIdempotencyKey::new(org, "x".repeat(65), "k").is_err());
    }

    #[test]
    fn for_scope_inherits_the_tenant() {
        let org = OrganizationId::new();
        let qs = TenantQueryScope::new(org);
        let idem = TenantIdempotencyKey::for_scope(&qs, "telegram", "k").unwrap();
        assert_eq!(idem.organization_id, org);
    }
}
