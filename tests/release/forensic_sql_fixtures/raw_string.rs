//! Fixture: SQL held in raw string literals (`r#"…"#`).
//!
//! `tests/release/forensic_sql_extractor.sh` sweeps this file on its own to
//! prove the forensic SQL scanner reads both forms: a statement passed as a
//! raw-string literal inline, and a statement hoisted into a `const` that the
//! call site references by name. Both were invisible to the previous
//! extractor, which only handled `r#"` immediately after the open paren.
//!
//! Both statements stay org-scoped, so the expected result is class 1
//! (tenant-safe) with zero class-4 findings.

use uuid::Uuid;

pub struct PositionRow {
    pub id: Uuid,
    pub organization_id: Uuid,
    pub symbol: String,
    pub quantity: f64,
}

const LIST_OPEN_POSITIONS: &str = r#"
SELECT id, organization_id, symbol, quantity
  FROM positions
 WHERE organization_id = $1
   AND status = 'open'
 ORDER BY opened_at ASC
"#;

/// Open positions of exactly one organization (a `const` statement).
pub async fn open_positions(
    pool: &sqlx::PgPool,
    organization_id: Uuid,
) -> Result<Vec<PositionRow>, sqlx::Error> {
    let rows = sqlx::query_as::<_, PositionRow>(LIST_OPEN_POSITIONS)
        .bind(organization_id)
        .fetch_all(pool)
        .await?;
    Ok(rows)
}

/// Close one position — the UPDATE is scoped by organization_id as well.
pub async fn close_position(
    pool: &sqlx::PgPool,
    organization_id: Uuid,
    position_id: Uuid,
) -> Result<bool, sqlx::Error> {
    let outcome = sqlx::query(
        r#"UPDATE positions
              SET status    = 'closed',
                  closed_at = now()
            WHERE organization_id = $1
              AND id = $2
              AND status = 'open'"#,
    )
    .bind(organization_id)
    .bind(position_id)
    .execute(pool)
    .await?;
    Ok(outcome.rows_affected() == 1)
}
