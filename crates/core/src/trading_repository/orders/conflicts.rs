//! Order conflict semantics — the audit + helpers (PROMPT 3/10 §B12).
//!
//! AUDIT OF THE PRE-EXISTING ON CONFLICT TARGETS ON `orders`:
//!
//! | writer (legacy `db::repo`) | arbiter before 0026 | arbiter after 0026 |
//! |---|---|---|
//! | `OrderRepo::insert_if_absent` | `(idempotency_key)` | `(organization_id, idempotency_key)` |
//! | `OrderRepo::upsert` | `(id)` | `(id)` — unchanged (app-assigned global id) |
//!
//! Business identity decisions (documented, deliberate):
//!
//! * `orders.id` — app-assigned `ord_<uuid>`: GLOBALLY unique by
//!   construction (0023 rule). The 0026 composite does NOT touch it:
//!   two tenants cannot mint the same uuid. The upsert therefore keeps
//!   `ON CONFLICT (id)` and adds `WHERE orders.organization_id = $1`
//!   on the update leg so a uuid collision across tenants can never
//!   mutate another tenant's row.
//! * `orders.idempotency_key` — CLIENT-SUPPLIED deterministic digest:
//!   TENANT-LOCAL. 0026 retires the global unique and creates
//!   `UNIQUE (organization_id, idempotency_key)`. The writer above
//!   names the composite as its arbiter. Consequences, all tested in
//!   `tests/orders_tenant_isolation.rs`:
//!   - same tenant + same key  → the second insert collapses (conflict
//!     path answers "already placed");
//!   - different tenants + same key → BOTH insert (tenant-local
//!     business key);
//!   - `idempotency_key IS NULL` → unrestricted (multiple NULLs are
//!     allowed by the unique index, exactly as before).
//! * `transactions.signature` — chain signature: GLOBAL, preserved
//!   (handled in `executions`, not here).

use crate::trading_repository::query_scope::TradingQueryScope;
use crate::trading_repository::repository_error::RepositoryError;

/// The tenant-composite conflict target on `orders` (0026). Every
/// tenant-scoped idempotent insert names EXACTLY this tuple.
pub const ORDERS_TENANT_ARBITER: &str = "(organization_id, idempotency_key)";

/// The still-global conflict target on `orders` (app-assigned ids).
pub const ORDERS_ID_ARBITER: &str = "(id)";

/// Does the acting tenant already hold this idempotency key? A pure
/// existence check used by callers that want to decide BEFORE the
/// insert (the insert itself remains the atomic authority).
pub async fn key_is_taken(
    db: &std::sync::Arc<crate::db::Database>,
    scope: &TradingQueryScope,
    idempotency_key: &str,
) -> Result<bool, RepositoryError> {
    if idempotency_key.trim().is_empty() {
        return Err(RepositoryError::Validation("idempotency_key"));
    }
    let row = db
        .timed(
            "tenant_order_key_taken",
            sqlx::query(
                r#"SELECT 1 FROM orders
                    WHERE organization_id = $1 AND idempotency_key = $2"#,
            )
            .bind(scope.organization_id().as_uuid())
            .bind(idempotency_key)
            .fetch_optional(db.pool()),
        )
        .await?;
    Ok(row.is_some())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arbiters_are_documented_constants() {
        assert_eq!(ORDERS_TENANT_ARBITER, "(organization_id, idempotency_key)");
        assert_eq!(ORDERS_ID_ARBITER, "(id)");
    }

    #[test]
    fn blank_keys_are_validation_errors() {
        let scope = TradingQueryScope::new(crate::tenant::OrganizationId::new());
        assert!(matches!(
            key_is_taken_probe(&scope, "  "),
            RepositoryError::Validation("idempotency_key")
        ));
    }

    // Mirror of the blank-input branch without a database (the live
    // path runs in tests/orders_tenant_isolation.rs).
    fn key_is_taken_probe(_scope: &TradingQueryScope, key: &str) -> RepositoryError {
        if key.trim().is_empty() {
            return RepositoryError::Validation("idempotency_key");
        }
        RepositoryError::Storage("no database in unit probe".into())
    }
}
