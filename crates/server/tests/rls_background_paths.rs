//! Service-backed isolation tests for the durable DCA and limit-order workers.
//!
//! The worker is invoked with tenant A explicitly bound while rows for A and B
//! are both due. A's rows must be processed, while B's rows remain untouched.
//! The same app pool, without a tenant binding, must neither expose rows nor
//! allow the worker's tenant-specific claim helpers to run.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use bot_core::config::DatabaseConfig;
use bot_core::db::Database;
use bot_core::tenant::rls_context::scope_tenant_context;
use chrono::Utc;
use sniper_suite::trading_data_plane::dca_worker::{self, DcaWorkerConfig};
use sniper_suite::trading_data_plane::limit_order_worker::{
    self, ClaimedOrder, MarkSource, TriggerSink, WorkerConfig,
};
use sqlx::postgres::PgPoolOptions;
use sqlx::{PgPool, Row};
use uuid::Uuid;

fn required_url(name: &str) -> String {
    std::env::var(name)
        .ok()
        .filter(|url| !url.trim().is_empty())
        .unwrap_or_else(|| panic!("{name} is required when REQUIRE_SERVICES=1"))
}

async fn database() -> Option<Arc<Database>> {
    let app_url = match std::env::var("POSTGRES_APP_URL") {
        Ok(url) if !url.trim().is_empty() => url,
        _ if std::env::var("REQUIRE_SERVICES").as_deref() == Ok("1") => {
            panic!("POSTGRES_APP_URL is required when REQUIRE_SERVICES=1")
        }
        _ => {
            eprintln!("NOT_RUN: rls_background_paths — PostgreSQL service is not configured");
            return None;
        }
    };
    let _ = required_url("POSTGRES_SYSTEM_URL");
    let _ = required_url("POSTGRES_DDL_URL");
    let cfg = DatabaseConfig {
        enabled: true,
        url_env: "POSTGRES_APP_URL".into(),
        rls_enabled: true,
        auto_migrate: true,
        ..DatabaseConfig::default()
    };
    Some(Arc::new(
        Database::connect(&cfg, &app_url)
            .await
            .expect("connect app/system pools and apply migrations"),
    ))
}

#[derive(Default)]
struct FiredOrders(Mutex<Vec<String>>);

#[async_trait]
impl MarkSource for FiredOrders {
    async fn mark_sol(&self, _mint: &str) -> Option<f64> {
        Some(5.0)
    }
}

#[async_trait]
impl TriggerSink for FiredOrders {
    async fn hand_off(&self, order: &ClaimedOrder, _fired_at_sol: f64) -> Result<(), String> {
        self.0
            .lock()
            .map_err(|_| "test sink mutex poisoned".to_owned())?
            .push(order.id.clone());
        Ok(())
    }
}

async fn insert_org(pool: &PgPool, id: Uuid, suffix: &str) {
    sqlx::query("INSERT INTO organizations (id, slug, name) VALUES ($1, $2, $3)")
        .bind(id)
        .bind(format!("rls-bg-{suffix}-{}", id.simple()))
        .bind(format!("background worker test {suffix}"))
        .execute(pool)
        .await
        .expect("seed test organization through DDL connection");
}

#[tokio::test]
async fn background_workers_only_touch_the_bound_tenant_and_fail_closed_unbound() {
    let Some(db) = database().await else { return };
    let ddl_url = required_url("POSTGRES_DDL_URL");
    let ddl = PgPoolOptions::new()
        .max_connections(3)
        .connect(&ddl_url)
        .await
        .expect("connect DDL-only test setup pool");
    let system_pool = db.system_pool().expect("RLS system pool configured");
    let org_a = Uuid::new_v4();
    let org_b = Uuid::new_v4();
    let dca_a = format!("rls-bg-dca-a-{}", Uuid::new_v4());
    let dca_b = format!("rls-bg-dca-b-{}", Uuid::new_v4());
    let order_a = format!("rls-bg-order-a-{}", Uuid::new_v4());
    let order_b = format!("rls-bg-order-b-{}", Uuid::new_v4());

    insert_org(&ddl, org_a, "a").await;
    insert_org(&ddl, org_b, "b").await;
    for (id, org) in [(&dca_a, org_a), (&dca_b, org_b)] {
        sqlx::query(
            "INSERT INTO dca_schedules
                (id, organization_id, mint, amount_per_run_sol, interval_secs, budget_sol, next_run_at)
             VALUES ($1, $2, 'RLSBG', 0.1, 60, 1, now() - interval '1 minute')",
        )
        .bind(id)
        .bind(org)
        .execute(&ddl)
        .await
        .expect("seed due DCA schedule");
    }
    for (id, org) in [(&order_a, org_a), (&order_b, org_b)] {
        sqlx::query(
            "INSERT INTO limit_orders
                (id, organization_id, mint, side, trigger_kind, price_sol, amount, status)
             VALUES ($1, $2, 'RLSBG', 'buy', 'at_or_below', 10, 0.1, 'active')",
        )
        .bind(id)
        .bind(org)
        .execute(&ddl)
        .await
        .expect("seed active limit order");
    }

    let dca_report = scope_tenant_context(
        org_a,
        dca_worker::tick_once(
            db.pool(),
            system_pool,
            &DcaWorkerConfig {
                worker_id: format!("rls-bg-dca-{}", Uuid::new_v4()),
                lease_secs: 30.0,
                tick: Duration::from_secs(1),
            },
        ),
    )
    .await
    .expect("tenant A DCA tick succeeds");
    assert_eq!(dca_report.claimed, 1);
    assert_eq!(dca_report.recorded, 1);

    let sink = Arc::new(FiredOrders::default());
    let limit_report = scope_tenant_context(
        org_a,
        limit_order_worker::tick_once(
            db.pool(),
            system_pool,
            &WorkerConfig {
                worker_id: format!("rls-bg-limit-{}", Uuid::new_v4()),
                lease_secs: 30.0,
                tick: Duration::from_secs(1),
            },
            Utc::now(),
            sink.as_ref(),
            sink.as_ref(),
        ),
    )
    .await
    .expect("tenant A limit-order tick succeeds");
    assert_eq!(limit_report.claimed, 1);
    assert_eq!(limit_report.fired, 1);
    assert_eq!(
        sink.0.lock().expect("read test sink").as_slice(),
        std::slice::from_ref(&order_a)
    );

    let dca_rows = sqlx::query(
        "SELECT id, spent_sol::float8 AS spent_sol, status FROM dca_schedules WHERE id = ANY($1)",
    )
    .bind(vec![dca_a.clone(), dca_b.clone()])
    .fetch_all(&ddl)
    .await
    .expect("inspect both DCA tenants through DDL");
    assert_eq!(dca_rows.len(), 2);
    for row in &dca_rows {
        let id: String = row.get("id");
        let spent: f64 = row.get("spent_sol");
        if id == dca_a {
            assert!(
                (spent - 0.1).abs() < 0.000_001,
                "tenant A spend recorded once"
            );
        } else {
            assert_eq!(id, dca_b);
            assert_eq!(spent, 0.0, "tenant B spend must remain untouched");
        }
    }
    let run_counts: Vec<(String, i64)> = sqlx::query_as(
        "SELECT schedule_id, count(*) FROM dca_runs WHERE schedule_id = ANY($1) GROUP BY schedule_id",
    )
    .bind(vec![dca_a.clone(), dca_b.clone()])
    .fetch_all(&ddl)
    .await
    .expect("inspect tenant DCA run ledger");
    assert_eq!(run_counts, vec![(dca_a.clone(), 1)]);

    let statuses: Vec<(String, String)> =
        sqlx::query_as("SELECT id, status FROM limit_orders WHERE id = ANY($1) ORDER BY id")
            .bind(vec![order_a.clone(), order_b.clone()])
            .fetch_all(&ddl)
            .await
            .expect("inspect both limit-order tenants through DDL");
    assert_eq!(statuses.len(), 2);
    assert_eq!(
        statuses.iter().find(|row| row.0 == order_a).unwrap().1,
        "triggered"
    );
    assert_eq!(
        statuses.iter().find(|row| row.0 == order_b).unwrap().1,
        "active"
    );

    // Tenant-specific claim functions require a bound tenant before they
    // acquire a connection. A raw unbound app-pool SELECT is also hidden by RLS.
    assert!(dca_worker::claim_due(db.pool(), "unbound-test", 30.0)
        .await
        .is_err());
    assert!(
        limit_order_worker::claim_due(db.pool(), "unbound-test", 30.0)
            .await
            .is_err()
    );
    let visible: i64 = sqlx::query_scalar("SELECT count(*) FROM dca_schedules WHERE id = ANY($1)")
        .bind(vec![dca_a.clone(), dca_b.clone()])
        .fetch_one(db.pool())
        .await
        .expect("unbound app query is a valid fail-closed read");
    assert_eq!(visible, 0, "unbound app role must not see tenant rows");

    sqlx::query("DELETE FROM dca_schedules WHERE id = ANY($1)")
        .bind(vec![dca_a, dca_b])
        .execute(&ddl)
        .await
        .expect("clean up DCA schedules and run rows");
    sqlx::query("DELETE FROM limit_orders WHERE id = ANY($1)")
        .bind(vec![order_a, order_b])
        .execute(&ddl)
        .await
        .expect("clean up limit orders");
    sqlx::query("DELETE FROM organizations WHERE id IN ($1, $2)")
        .bind(org_a)
        .bind(org_b)
        .execute(&ddl)
        .await
        .expect("clean up test organizations");
    ddl.close().await;
    db.close().await;
}
