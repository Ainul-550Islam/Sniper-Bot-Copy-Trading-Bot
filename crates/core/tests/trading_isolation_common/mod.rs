//! Shared harness for the PROMPT 3/10 tenant-isolation PostgreSQL
//! suites (`orders_cross_tenant_pg.rs`, `execution_cross_tenant_pg.rs`,
//! …, `reporting_cross_tenant_pg.rs`).
//!
//! GATED exactly like `db_integration.rs`: every test prints NOT_RUN
//! and returns early unless `POSTGRES_URL` points at a real database.
//! Run with `--test-threads=1` (the suites share one database and use
//! run-unique keys rather than per-test schemas).

#![allow(dead_code)]

use std::sync::Arc;

use bot_core::config::DatabaseConfig;
use bot_core::db::Database;
use bot_core::tenant::OrganizationId;
use bot_core::trading_repository::query_scope::TradingQueryScope;
use bot_core::trading_repository::write_scope::{TenantWriteScope, WriteOrigin};
use chrono::{DateTime, Utc};

/// The gated connection (None ⇒ suite reports NOT_RUN).
pub async fn setup() -> Option<Arc<Database>> {
    let url = pg_url()?;
    let cfg = DatabaseConfig {
        enabled: true,
        auto_migrate: true,
        ..Default::default()
    };
    let db = Database::connect(&cfg, &url)
        .await
        .expect("configured database must connect");
    db.migrate().await.expect("migrations must apply");
    Some(Arc::new(db))
}

fn pg_url() -> Option<String> {
    std::env::var("POSTGRES_URL")
        .ok()
        .filter(|s| !s.trim().is_empty())
}

/// Unique namespace per test run (unique keys never collide between
/// CI jobs or repeated runs).
pub fn run_id() -> String {
    format!(
        "{}-{}",
        std::process::id(),
        Utc::now().timestamp_nanos_opt().unwrap_or(0)
    )
}

/// Create a real organization row and return its typed id.
pub async fn org(db: &Database, slug: &str) -> OrganizationId {
    let id = uuid::Uuid::new_v4();
    sqlx::query(
        r#"INSERT INTO organizations (id, slug, name, status)
           VALUES ($1, $2, $3, 'active')"#,
    )
    .bind(id)
    .bind(slug)
    .bind(format!("Org {slug}"))
    .execute(db.pool())
    .await
    .expect("insert organization");
    OrganizationId::from(id)
}

/// The read scope for one tenant.
pub fn read_scope(org: OrganizationId) -> TradingQueryScope {
    TradingQueryScope::new(org)
}

/// The write scope for one tenant's API actor.
pub fn write_scope(org: OrganizationId) -> TenantWriteScope {
    TenantWriteScope::new(org, "user:test", WriteOrigin::Http).expect("write scope")
}

/// A timestamp safely in the past/future relative to `now`.
pub fn at(days_before: f64) -> DateTime<Utc> {
    Utc::now() - chrono::Duration::milliseconds((days_before * 86_400_000.0) as i64)
}

/// Seed one order row for `org` via direct SQL (attack tests exercise
/// the REPOSITORIES, the seed only arranges the battlefield).
pub async fn seed_order(
    db: &Database,
    org: OrganizationId,
    id: &str,
    idempotency_key: Option<&str>,
    status: &str,
) {
    sqlx::query(
        r#"INSERT INTO orders
               (organization_id, id, idempotency_key, module, side, symbol, venue,
                mode, status, qty)
           VALUES ($1, $2, $3, 'sniper', 'buy', 'SOL/USDC', 'paper', 'paper', $4, 1.0)
           ON CONFLICT (id) DO NOTHING"#,
    )
    .bind(org.as_uuid())
    .bind(id)
    .bind(idempotency_key)
    .bind(status)
    .execute(db.pool())
    .await
    .expect("seed order");
}
