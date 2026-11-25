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
//! on first use. It is immutable for the lifetime of one DATABASE.
//!
//! **The cache is per database, not per process.** It used to be one
//! `OnceCell` shared by the whole process, which is wrong twice over:
//!
//! 1. two [`Database`] handles in one process may point at DIFFERENT
//!    databases (tests, a migration run + a live pool, a second
//!    environment during a cutover) and each database has its own
//!    `deployment` organization — the second handle would silently write
//!    its rows under the FIRST database's tenant id;
//! 2. two concurrent first callers both ran the resolver and the loser's
//!    answer was dropped without ever being checked against the winner
//!    (the winner is idempotent, so this was usually benign — but it hid
//!    a genuinely inconsistent database instead of failing loudly).

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use uuid::Uuid;

use crate::db::Database;
use crate::db::TimedDbError;

/// Per-database cache: `Database::identity()` → deployment organization uuid.
fn cache() -> &'static Mutex<HashMap<String, Uuid>> {
    static CACHE: OnceLock<Mutex<HashMap<String, Uuid>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// The cached deployment organization for THIS database, if already resolved.
pub fn cached_deployment_org_uuid(db: &Database) -> Option<Uuid> {
    cache()
        .lock()
        .ok()
        .and_then(|cache| cache.get(db.identity()).copied())
}

/// Forget the cached deployment organization of one database (used by tests and
/// by an operator after a restore/rename). Other databases are untouched.
pub fn forget_deployment_org_uuid(db: &Database) {
    if let Ok(mut cache) = cache().lock() {
        cache.remove(db.identity());
    }
}

/// Resolve (and cache per database) the deployment organization's uuid.
///
/// Fails closed: if the function errors, the legacy plane does NOT
/// proceed with a guessed tenant. The cache is only written for the
/// database that answered, and only after the answer was actually
/// received — a failure never poisons another database's entry.
pub async fn deployment_org_uuid(db: &Database) -> Result<Uuid, TimedDbError> {
    let identity = db.identity().to_string();
    if let Some(cached) = cached_deployment_org_uuid(db) {
        return Ok(cached);
    }
    // Deployment identity is control-plane metadata, not a request-scoped
    // tenant lookup. Use the system pool when RLS is configured so an empty
    // or unrelated request tenant cannot hide the deployment organization.
    let pool = db.system_pool().unwrap_or_else(|| db.pool());
    let row: (Uuid,) = db
        .timed(
            "deployment_org_uuid",
            sqlx::query_as("SELECT public.deployment_organization_id()").fetch_one(pool),
        )
        .await?;
    // The row was read from THIS database; store it under THIS database's
    // identity only. A concurrent caller that resolved first wins and its
    // value is what everyone gets — and if the two answers ever differ the
    // disagreement is now visible instead of being silently overwritten.
    let mut cache = cache().lock().unwrap_or_else(|poisoned| {
        // A poisoned lock means another thread panicked while holding the
        // cache; the map itself is still intact, so keep using it rather than
        // wedging every legacy repository in the process.
        poisoned.into_inner()
    });
    Ok(*cache.entry(identity).or_insert(row.0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_entries_are_per_database_identity() {
        // Two databases never share an entry: the key is the identity string,
        // so a lookup for one can never return the other's answer.
        let mut cache: HashMap<String, Uuid> = HashMap::new();
        let a = Uuid::new_v4();
        let b = Uuid::new_v4();
        cache.insert("sniper@host:5432/alpha".to_string(), a);
        cache.insert("sniper@host:5432/beta".to_string(), b);
        assert_eq!(
            cache.get("sniper@host:5432/alpha").copied(),
            Some(a),
            "the alpha database keeps its own deployment organization"
        );
        assert_eq!(
            cache.get("sniper@host:5432/beta").copied(),
            Some(b),
            "the beta database keeps its own deployment organization"
        );
        assert_ne!(a, b, "a shared cell would have made these equal");
        // Same server, different database name ⇒ different key.
        assert!(cache
            .get("sniper@host:5432/alpha")
            .is_some_and(|id| *id != b));
    }
}
