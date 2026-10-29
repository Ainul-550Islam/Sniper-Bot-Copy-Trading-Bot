//! Fixture: SQL assembled by `format!` across several lines.
//!
//! `tests/release/forensic_sql_extractor.sh` sweeps this file on its own to
//! prove the forensic SQL scanner reads statements whose text is produced by
//! `format!` — including the multi-line single-literal form — instead of
//! silently dropping them from the tenant-isolation sweep.
//!
//! Both statements stay org-scoped, so the expected sweep result is class 1
//! (tenant-safe) with zero class-4 findings.

use uuid::Uuid;

pub struct RuntimeRow {
    pub id: Uuid,
    pub organization_id: Uuid,
    pub generation: i64,
    pub status: String,
}

impl RuntimeRow {
    const COLUMNS: &'static str =
        "id, organization_id, generation, status, claimed_at, lease_until";
}

/// Every runtime of one organization, newest generation first.
pub async fn load_runtimes(
    pool: &sqlx::PgPool,
    organization_id: Uuid,
) -> Result<Vec<RuntimeRow>, sqlx::Error> {
    let rows = sqlx::query_as::<_, RuntimeRow>(&format!(
        "SELECT {} FROM tenant_runtimes
          WHERE organization_id = $1
            AND status IN ('provisioning','active')
          ORDER BY generation DESC",
        RuntimeRow::COLUMNS
    ))
    .bind(organization_id)
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

/// One runtime, still scoped to the organization that owns it.
pub async fn load_runtime(
    pool: &sqlx::PgPool,
    organization_id: Uuid,
    runtime_id: Uuid,
) -> Result<Option<RuntimeRow>, sqlx::Error> {
    let row = sqlx::query_as::<_, RuntimeRow>(&format!(
        "SELECT {} FROM tenant_runtimes
          WHERE organization_id = $1 AND id = $2",
        RuntimeRow::COLUMNS
    ))
    .bind(organization_id)
    .bind(runtime_id)
    .fetch_optional(pool)
    .await?;
    Ok(row)
}
