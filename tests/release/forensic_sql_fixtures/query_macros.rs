//! Fixture: SQL passed to the compile-time `sqlx::query*` macros.
//!
//! `tests/release/forensic_sql_extractor.sh` sweeps this file on its own to
//! prove the forensic SQL scanner reads `sqlx::query!`, `sqlx::query_as!` and
//! `sqlx::query_scalar!` call sites. They are the same tenant-isolation
//! contract as the runtime forms — the macro only moves the check to compile
//! time — so an unread macro is an unclassified statement.
//!
//! All three statements stay org-scoped, so the expected result is class 1
//! (tenant-safe) with zero class-4 findings.

use uuid::Uuid;

pub struct OrderRow {
    pub id: Uuid,
    pub organization_id: Uuid,
    pub symbol: String,
}

/// How many positions this organization still holds open.
pub async fn count_open_positions(
    pool: &sqlx::PgPool,
    organization_id: Uuid,
) -> Result<i64, sqlx::Error> {
    let count = sqlx::query_scalar!(
        "SELECT count(*) FROM positions WHERE organization_id = $1 AND status = 'open'",
        organization_id
    )
    .fetch_one(pool)
    .await?;
    Ok(count.unwrap_or(0))
}

/// One order, scoped to its owning organization.
pub async fn load_order(
    pool: &sqlx::PgPool,
    organization_id: Uuid,
    order_id: Uuid,
) -> Result<Option<OrderRow>, sqlx::Error> {
    let row = sqlx::query_as!(
        OrderRow,
        "SELECT id, organization_id, symbol FROM orders WHERE organization_id = $1 AND id = $2",
        organization_id,
        order_id
    )
    .fetch_optional(pool)
    .await?;
    Ok(row)
}

/// Record a risk decision against this organization.
pub async fn record_risk_event(
    pool: &sqlx::PgPool,
    organization_id: Uuid,
    kind: &str,
    detail: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query!(
        "INSERT INTO risk_events (organization_id, kind, detail) VALUES ($1, $2, $3)",
        organization_id,
        kind,
        detail
    )
    .execute(pool)
    .await?;
    Ok(())
}
