//! Fixture: a tenant table read with NO organization_id predicate.
//!
//! `tests/release/forensic_sql_extractor.sh` sweeps this file on its own and
//! REQUIRES the scanner to fail: `all_open_positions` reads `positions` — a
//! tenant-truth table — with no org scope in the SQL, which is class 4
//! (MISSING-TENANT-ENFORCEMENT). If the extractor ever stops reading this
//! statement the gate goes green for the wrong reason, which is precisely the
//! failure mode this fixture exists to catch.
//!
//! The second statement is the correct form of the same query and must be
//! reported class 1, so the fixture also proves the scanner still discriminates
//! rather than failing everything it sees.

use uuid::Uuid;

pub struct PositionRow {
    pub id: Uuid,
    pub organization_id: Uuid,
    pub symbol: String,
    pub quantity: f64,
}

/// CROSS-TENANT READ. There is no organization predicate anywhere in this
/// statement: it returns every organization's open positions. This line is a
/// class-4 finding by construction and must fail the sweep.
pub async fn all_open_positions(
    pool: &sqlx::PgPool,
) -> Result<Vec<PositionRow>, sqlx::Error> {
    let rows = sqlx::query_as::<_, PositionRow>(
        "SELECT id, organization_id, symbol, quantity FROM positions WHERE status = 'open' ORDER BY opened_at ASC",
    )
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

/// The same query, correctly scoped to one organization (class 1).
pub async fn open_positions_for(
    pool: &sqlx::PgPool,
    organization_id: Uuid,
) -> Result<Vec<PositionRow>, sqlx::Error> {
    let rows = sqlx::query_as::<_, PositionRow>(
        "SELECT id, organization_id, symbol, quantity FROM positions WHERE organization_id = $1 AND status = 'open' ORDER BY opened_at ASC",
    )
    .bind(organization_id)
    .fetch_all(pool)
    .await?;
    Ok(rows)
}
