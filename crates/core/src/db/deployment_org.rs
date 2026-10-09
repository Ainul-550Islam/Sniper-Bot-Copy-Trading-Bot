//! The deployment organization — the LEGACY plane's explicit tenant
//! (PROMPT 3/10 legacy-mode preservation).
//!
//! Since the 0026–0033 tenant-composite swaps, the legacy
//! repositories (`db::repo`, `db::claims`, `db::copy`,
//! `db::polymarket`, `db::execution`, `db::accounting`) are no longer
//! tenant-blind: every one of their statements binds this module's
//! uuid in its `organization_id` predicate / INSERT column, exactly
//! like the tenant data plane binds its authenticated organization.
//! There is no implicit global tenant — the legacy engine is simply
//! ALWAYS one explicit tenant: the deployment organization.
//!
//! The uuid comes from the same SQL function the 0024 column DEFAULTs
//! use (`public.deployment_organization_id()`), resolved or created
//! on first use. It is immutable for the lifetime of the database, so
//! it is resolved ONCE per process and cached (two concurrent first
//! callers race to the same idempotent answer).

use uuid::Uuid;

use crate::db::Database;
use crate::db::TimedDbError;

static DEPLOYMENT_ORG: tokio::sync::OnceCell<Uuid> = tokio::sync::OnceCell::const_new();

/// Resolve (and cache) the deployment organization's uuid.
///
/// Fails closed: if the function errors, the legacy plane does NOT
/// proceed with a guessed tenant.
pub async fn deployment_org_uuid(db: &Database) -> Result<Uuid, TimedDbError> {
    if let Some(cached) = DEPLOYMENT_ORG.get() {
        return Ok(*cached);
    }
    let row: (Uuid,) = db
        .timed(
            "deployment_org_uuid",
            sqlx::query_as("SELECT public.deployment_organization_id()").fetch_one(db.pool()),
        )
        .await?;
    let _ = DEPLOYMENT_ORG.set(row.0);
    Ok(row.0)
}

#[cfg(test)]
mod tests {
    #[test]
    fn cache_cell_starts_empty() {
        // The OnceCell is process-global; this only asserts the static
        // exists and is initially unset in a fresh test binary (the
        // cached path itself is exercised by the PostgreSQL suites).
        assert!(super::DEPLOYMENT_ORG.get().is_none());
    }
}
