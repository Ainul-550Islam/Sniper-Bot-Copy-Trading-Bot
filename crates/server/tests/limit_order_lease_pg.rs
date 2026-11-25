//! A slow limit-order tick must not act on a lease that lapsed under it
//! (gap #86), against REAL PostgreSQL.
//!
//! The worker claims rows under a short lease, then fetches a mark and only
//! AFTER that hands the order off and writes `triggered`/`expired`. Those
//! writes used to be guarded on `lease_owner` and `status` only — never on the
//! lease still being unexpired — so a mark slower than the lease left a worker
//! acting on a row another replica is entitled to claim: two live hand-offs for
//! one order, or a stale worker writing a terminal state over the new holder's
//! lease.
//!
//! The takeover is simulated deterministically (the lapsed row is re-claimed by
//! a second owner from the same pool while the worker is inside its slow mark),
//! so the witness cannot hang on cross-pool locking.
//!
//! Gated on `POSTGRES_URL`.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use bot_core::config::DatabaseConfig;
use bot_core::db::Database;
use bot_core::tenant::rls_context::scope_tenant_context;
use bot_core::tenant::OrganizationId;
use chrono::Utc;
use sniper_suite::trading_data_plane::limit_order_worker::{
    self, ClaimedOrder, MarkSource, TriggerSink, WorkerConfig,
};
use sqlx::{PgPool, Row};
use uuid::Uuid;

async fn setup() -> Option<Arc<Database>> {
    let url = std::env::var("POSTGRES_URL")
        .ok()
        .filter(|u| !u.trim().is_empty())?;
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

async fn org(db: &Database, slug: &str) -> OrganizationId {
    let id = Uuid::new_v4();
    sqlx::query("INSERT INTO organizations (id, slug, name, status) VALUES ($1,$2,$3,'active')")
        .bind(id)
        .bind(slug)
        .bind(format!("limit lease {slug}"))
        .execute(db.pool())
        .await
        .expect("organization insert");
    OrganizationId::from(id)
}

/// Re-claim the row from another "replica" while the first worker is stuck.
#[derive(Clone)]
struct Takeover {
    pool: PgPool,
    order_id: String,
    organization_id: Uuid,
}

/// A mark source that burns the lease on its FIRST call (the slow tick), then
/// answers instantly for anybody else.
struct SlowMark {
    delay: Duration,
    calls: AtomicUsize,
    takeover: Option<Takeover>,
}

#[async_trait]
impl MarkSource for SlowMark {
    async fn mark_sol(&self, _mint: &str) -> Option<f64> {
        if self.calls.fetch_add(1, Ordering::SeqCst) == 0 {
            // Let the lease lapse, then a second replica claims the row.
            tokio::time::sleep(self.delay).await;
            if let Some(takeover) = &self.takeover {
                sqlx::query(
                    "UPDATE limit_orders
                        SET lease_owner = 'replica-2',
                            lease_expires_at = now() + interval '30 seconds'
                      WHERE id = $1 AND organization_id = $2",
                )
                .bind(&takeover.order_id)
                .bind(takeover.organization_id)
                .execute(&takeover.pool)
                .await
                .expect("the second replica claims the lapsed row");
            }
        }
        // A price that fires an at_or_below 10 SOL order.
        Some(1.0)
    }
}

/// Records every hand-off it is asked to perform.
#[derive(Default)]
struct CountingSink {
    ids: std::sync::Mutex<Vec<String>>,
}

#[async_trait]
impl TriggerSink for CountingSink {
    async fn hand_off(&self, order: &ClaimedOrder, _fired_at_sol: f64) -> Result<(), String> {
        self.ids.lock().unwrap().push(order.id.clone());
        Ok(())
    }
}

async fn row_state(pool: &PgPool, id: &str) -> (String, Option<String>) {
    let row = sqlx::query("SELECT status, lease_owner FROM limit_orders WHERE id = $1")
        .bind(id)
        .fetch_one(pool)
        .await
        .expect("row read");
    (row.get("status"), row.get("lease_owner"))
}

fn worker(id: &str, lease_secs: f64) -> WorkerConfig {
    WorkerConfig {
        worker_id: id.to_string(),
        lease_secs,
        tick: Duration::from_secs(1),
    }
}

#[tokio::test]
async fn a_lapsed_lease_cannot_hand_off_or_mark_a_limit_order() {
    let Some(db) = setup().await else {
        eprintln!("NOT_RUN: limit_order_lease_pg — POSTGRES_URL missing");
        return;
    };
    let run = format!(
        "{}-{}",
        std::process::id(),
        Utc::now().timestamp_nanos_opt().unwrap_or(0)
    );
    let a = org(&db, &format!("lo-lease-{run}")).await;
    let fire_id = format!("lo-fire-{run}");
    let expire_id = format!("lo-expire-{run}");
    let system_pool = db.system_pool().unwrap_or_else(|| db.pool()).clone();
    for (id, expiring) in [(&fire_id, false), (&expire_id, true)] {
        // The second order is already past its own expiry: the tick would
        // expire it — unless the lease lapsed first.
        sqlx::query(
            "INSERT INTO limit_orders
                (id, organization_id, mint, side, trigger_kind, price_sol, amount, status, expires_at)
             VALUES ($1, $2, 'LEASEMINT', 'buy', 'at_or_below', 10, 0.1, 'active',
                     CASE WHEN $3 THEN now() - interval '1 second' ELSE NULL END)",
        )
        .bind(id)
        .bind(a.as_uuid())
        .bind(expiring)
        .execute(db.pool())
        .await
        .expect("seed the active order");
    }

    // ── 1. The order the slow tick wanted to FIRE ───────────────────────────
    let slow = Arc::new(SlowMark {
        delay: Duration::from_millis(500),
        calls: AtomicUsize::new(0),
        takeover: Some(Takeover {
            pool: db.pool().clone(),
            order_id: fire_id.clone(),
            organization_id: a.as_uuid(),
        }),
    });
    let sink = Arc::new(CountingSink::default());
    let report = tokio::time::timeout(
        Duration::from_secs(30),
        scope_tenant_context(
            a.as_uuid(),
            limit_order_worker::tick_once(
                db.pool(),
                &system_pool,
                &worker(&format!("slow-{run}"), 0.2),
                Utc::now(),
                slow.as_ref(),
                sink.as_ref(),
            ),
        ),
    )
    .await
    .expect("the tick terminates")
    .expect("the tick succeeds");

    assert_eq!(
        report.claimed, 2,
        "the tick claimed BOTH seeded rows in one batch"
    );
    assert!(
        sink.ids.lock().unwrap().is_empty(),
        "a worker whose lease lapsed must not hand the order off: {:?}",
        sink.ids.lock().unwrap()
    );
    assert_eq!(
        report.fired, 0,
        "and it must not report a fire for either row"
    );
    assert_eq!(
        report.waiting, 2,
        "both rows are reported as skipped, not fired/expired"
    );
    let (status, owner) = row_state(db.pool(), &fire_id).await;
    assert_eq!(
        status, "active",
        "the row is NOT triggered by the stale worker"
    );
    assert_eq!(
        owner.as_deref(),
        Some("replica-2"),
        "the row still belongs to the replica that holds a live lease"
    );

    // ── 2. The order the slow tick wanted to EXPIRE ─────────────────────────
    let slow_expire = Arc::new(SlowMark {
        delay: Duration::from_millis(500),
        calls: AtomicUsize::new(0),
        takeover: Some(Takeover {
            pool: db.pool().clone(),
            order_id: expire_id.clone(),
            organization_id: a.as_uuid(),
        }),
    });
    let sink2 = Arc::new(CountingSink::default());
    let report2 = tokio::time::timeout(
        Duration::from_secs(30),
        scope_tenant_context(
            a.as_uuid(),
            limit_order_worker::tick_once(
                db.pool(),
                &system_pool,
                &worker(&format!("slow-exp-{run}"), 0.2),
                Utc::now(),
                slow_expire.as_ref(),
                sink2.as_ref(),
            ),
        ),
    )
    .await
    .expect("the tick terminates")
    .expect("the tick succeeds");
    assert_eq!(report2.expired, 0, "the stale worker expires nothing");
    assert!(sink2.ids.lock().unwrap().is_empty());
    let (status2, owner2) = row_state(db.pool(), &expire_id).await;
    assert_eq!(
        status2, "active",
        "the row belongs to the new lease holder and stays active"
    );
    assert_eq!(owner2.as_deref(), Some("replica-2"));

    // ── 3. A worker that DOES hold a live lease still fires normally ────────
    let live_id = format!("lo-live-{run}");
    sqlx::query(
        "INSERT INTO limit_orders
            (id, organization_id, mint, side, trigger_kind, price_sol, amount, status)
         VALUES ($1, $2, 'LEASEMINT', 'buy', 'at_or_below', 10, 0.1, 'active')",
    )
    .bind(&live_id)
    .bind(a.as_uuid())
    .execute(db.pool())
    .await
    .expect("seed the live order");
    let fast = Arc::new(SlowMark {
        delay: Duration::ZERO,
        calls: AtomicUsize::new(1),
        takeover: None,
    });
    let sink3 = Arc::new(CountingSink::default());
    let report3 = scope_tenant_context(
        a.as_uuid(),
        limit_order_worker::tick_once(
            db.pool(),
            &system_pool,
            &worker(&format!("fast-{run}"), 30.0),
            Utc::now(),
            fast.as_ref(),
            sink3.as_ref(),
        ),
    )
    .await
    .expect("the tick succeeds");
    assert_eq!(report3.claimed, 1, "only the third row is still claimable");
    assert_eq!(report3.fired, 1, "a live lease fires as before");
    assert_eq!(
        sink3.ids.lock().unwrap().as_slice(),
        std::slice::from_ref(&live_id)
    );
    let (status3, owner3) = row_state(db.pool(), &live_id).await;
    assert_eq!(status3, "triggered");
    assert_eq!(owner3, None, "the terminal row holds no lease");

    sqlx::query("DELETE FROM limit_orders WHERE id = ANY($1)")
        .bind(vec![fire_id, expire_id, live_id])
        .execute(db.pool())
        .await
        .expect("cleanup");
}
