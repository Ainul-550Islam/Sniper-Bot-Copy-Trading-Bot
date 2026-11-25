//! A DCA schedule paused or cancelled while a worker holds its lease must not
//! get a run recorded (gap #87), against REAL PostgreSQL.
//!
//! `record_slot` is documented as "the lease owner check is repeated inside
//! every write, so a replica whose lease expired mid-tick writes nothing" — but
//! the statements only compared `lease_owner`. Two holes followed:
//!
//!   * a PAUSED or CANCELLED schedule still matched (the run statement never
//!     looked at `status`), so a worker that was busy when the operator
//!     transitioned the schedule still recorded an intent and advanced the
//!     clock. `transition()` refuses to write while a LIVE lease exists, so this
//!     window is exactly the one it cannot protect: the pause is accepted
//!     BETWEEN claim and record;
//!   * a worker whose lease had already EXPIRED still matched its own stale
//!     `lease_owner` row, so it could mark a schedule `completed` (budget
//!     exhausted) or record a slot long after the lease lapsed and another
//!     replica became entitled to claim.
//!
//! The witness drives the real worker functions (`claim_due` → operator
//! transition → `record_slot`), so it fails on the old statements and passes on
//! the fenced ones. Gated on `POSTGRES_URL`.

use bot_core::config::DatabaseConfig;
use bot_core::db::Database;
use bot_core::tenant::rls_context::scope_tenant_context;
use bot_core::tenant::OrganizationId;
use sniper_suite::trading_data_plane::dca_worker::{self, RunOutcome};
use sqlx::{PgPool, Row};
use uuid::Uuid;

async fn setup() -> Option<Database> {
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
    Some(db)
}

async fn seed(pool: &PgPool, org: Uuid, id: &str, budget: f64, spent: f64) {
    sqlx::query(
        "INSERT INTO dca_schedules
            (id, organization_id, mint, amount_per_run_sol, interval_secs, budget_sol, spent_sol, next_run_at)
         VALUES ($1, $2, 'DCALEASE', 0.25, 3600, $3, $4, now() - interval '1 hour')",
    )
    .bind(id)
    .bind(org)
    .bind(budget)
    .bind(spent)
    .execute(pool)
    .await
    .expect("seed the due schedule");
}

async fn row(pool: &PgPool, id: &str) -> (String, f64, Option<String>, i32) {
    let row = sqlx::query(
        "SELECT status, spent_sol::float8 AS spent_sol, lease_owner, attempts
           FROM dca_schedules WHERE id = $1",
    )
    .bind(id)
    .fetch_one(pool)
    .await
    .expect("schedule read");
    (
        row.get("status"),
        row.get("spent_sol"),
        row.get("lease_owner"),
        row.get("attempts"),
    )
}

async fn run_count(pool: &PgPool, id: &str) -> i64 {
    sqlx::query_scalar::<_, i64>("SELECT count(*) FROM dca_runs WHERE schedule_id = $1")
        .bind(id)
        .fetch_one(pool)
        .await
        .expect("run count")
}

#[tokio::test]
async fn a_paused_or_expired_schedule_cannot_be_recorded_by_a_stale_worker() {
    let Some(db) = setup().await else {
        eprintln!("NOT_RUN: dca_lease_pg — POSTGRES_URL missing");
        return;
    };
    let pool = db.pool();
    let org = Uuid::new_v4();
    sqlx::query("INSERT INTO organizations (id, slug, name, status) VALUES ($1,$2,$3,'active')")
        .bind(org)
        .bind(format!("dca-lease-{org}"))
        .bind("dca lease witness")
        .execute(pool)
        .await
        .expect("organization insert");
    let org = OrganizationId::from(org);
    let run = Uuid::new_v4();
    // Distinct schedules: a paused one, a cancelled one, an expired-lease one,
    // and one that keeps a live lease and must still record normally.
    let paused = format!("dca-paused-{run}");
    let cancelled = format!("dca-cancelled-{run}");
    let lapsed = format!("dca-lapsed-{run}");
    let live = format!("dca-live-{run}");
    for id in [&paused, &cancelled, &lapsed, &live] {
        seed(pool, org.as_uuid(), id, 1.0, 0.0).await;
    }

    let claimed = scope_tenant_context(
        org.as_uuid(),
        dca_worker::claim_due(pool, "replica-1", 30.0),
    )
    .await
    .expect("claim the due schedules");
    assert_eq!(claimed.len(), 4, "all four due schedules are claimed");

    // ── the operator pauses one and cancels another BETWEEN claim and record ─
    // (transition() refuses while a LIVE lease exists, so this is precisely the
    // window it cannot close itself.)
    for (id, status) in [(&paused, "paused"), (&cancelled, "cancelled")] {
        sqlx::query("UPDATE dca_schedules SET status = $2 WHERE id = $1")
            .bind(id)
            .bind(status)
            .execute(pool)
            .await
            .expect("operator transition");
    }
    // ── and the lease on a third schedule lapses, then a peer takes it over ──
    sqlx::query(
        "UPDATE dca_schedules SET lease_expires_at = now() - interval '1 second' WHERE id = $1",
    )
    .bind(&lapsed)
    .execute(pool)
    .await
    .expect("force the lease to lapse");
    sqlx::query(
        "UPDATE dca_schedules SET lease_owner = 'replica-2', \
            lease_expires_at = now() + interval '30 seconds' WHERE id = $1",
    )
    .bind(&lapsed)
    .execute(pool)
    .await
    .expect("peer takeover");

    for schedule in &claimed {
        let outcome = scope_tenant_context(
            org.as_uuid(),
            dca_worker::record_slot(pool, schedule, "replica-1"),
        )
        .await
        .expect("record_slot returns");
        match schedule.id.as_str() {
            id if id == paused => {
                assert!(
                    matches!(outcome, RunOutcome::AlreadyRecorded),
                    "a paused schedule records nothing, got {outcome:?}"
                );
                assert_eq!(run_count(pool, id).await, 0, "no intent for a paused run");
                let (status, spent, _, _) = row(pool, id).await;
                assert_eq!(status, "paused", "the operator's pause survives");
                assert_eq!(spent, 0.0, "and nothing is charged");
            }
            id if id == cancelled => {
                assert!(
                    matches!(outcome, RunOutcome::AlreadyRecorded),
                    "a cancelled schedule records nothing, got {outcome:?}"
                );
                assert_eq!(
                    run_count(pool, id).await,
                    0,
                    "no intent for a cancelled run"
                );
                let (status, spent, _, _) = row(pool, id).await;
                assert_eq!(status, "cancelled");
                assert_eq!(spent, 0.0);
            }
            id if id == lapsed => {
                assert!(
                    matches!(outcome, RunOutcome::AlreadyRecorded),
                    "a worker whose lease lapsed records nothing, got {outcome:?}"
                );
                assert_eq!(
                    run_count(pool, id).await,
                    0,
                    "no intent from the stale worker"
                );
                let (_, spent, owner, _) = row(pool, id).await;
                assert_eq!(spent, 0.0, "and nothing is charged");
                assert_eq!(
                    owner.as_deref(),
                    Some("replica-2"),
                    "the live lease belongs to the new holder, untouched"
                );
            }
            id if id == live => {
                match outcome {
                    RunOutcome::Recorded {
                        amount_sol,
                        completed,
                    } => {
                        assert!((amount_sol - 0.25).abs() < 1e-9, "one interval is charged");
                        assert!(!completed, "the budget still has room");
                    }
                    other => panic!("a live-lease schedule must record, got {other:?}"),
                }
                assert_eq!(run_count(pool, id).await, 1, "exactly one intent");
                let (_, spent, owner, _) = row(pool, id).await;
                assert!((spent - 0.25).abs() < 1e-9, "the spend is persisted");
                assert_eq!(owner, None, "the lease is released as before");
            }
            other => panic!("unexpected schedule {other}"),
        }
    }

    // ── a budget-exhausted schedule owned by a lapsed worker stays untouched ─
    let exhausted = format!("dca-exhausted-{run}");
    sqlx::query(
        "INSERT INTO dca_schedules
            (id, organization_id, mint, amount_per_run_sol, interval_secs, budget_sol, spent_sol, next_run_at)
         VALUES ($1, $2, 'DCALEASE', 0.25, 3600, 0.25, 0.25, now() - interval '1 hour')",
    )
    .bind(&exhausted)
    .bind(org.as_uuid())
    .execute(pool)
    .await
    .expect("seed the exhausted schedule");
    // claim_due refuses rows whose remaining budget is spent? It does not — the
    // budget is enforced inside record_slot — so claim it and lapse the lease.
    let exhausted_claim = scope_tenant_context(
        org.as_uuid(),
        dca_worker::claim_due(pool, "replica-1", 30.0),
    )
    .await
    .expect("claim the exhausted schedule");
    let exhausted_schedule = exhausted_claim
        .iter()
        .find(|s| s.id == exhausted)
        .expect("the exhausted schedule is claimable");
    sqlx::query("UPDATE dca_schedules SET status = 'paused' WHERE id = $1")
        .bind(&exhausted)
        .execute(pool)
        .await
        .expect("operator pauses the exhausted schedule");
    let outcome = scope_tenant_context(
        org.as_uuid(),
        dca_worker::record_slot(pool, exhausted_schedule, "replica-1"),
    )
    .await
    .expect("record_slot returns");
    assert!(
        matches!(outcome, RunOutcome::AlreadyRecorded),
        "a paused schedule is never marked completed by the worker, got {outcome:?}"
    );
    let (status, _, _, _) = row(pool, &exhausted).await;
    assert_eq!(
        status, "paused",
        "the pause is not overwritten with 'completed'"
    );

    // ── a lost lease cannot charge an attempt either (no starvation) ─────────
    let attempts_id = format!("dca-attempts-{run}");
    seed(pool, org.as_uuid(), &attempts_id, 1.0, 0.0).await;
    let claimed = scope_tenant_context(
        org.as_uuid(),
        dca_worker::claim_due(pool, "replica-1", 30.0),
    )
    .await
    .expect("claim the attempts schedule");
    let attempts_schedule = claimed
        .iter()
        .find(|s| s.id == attempts_id)
        .expect("the attempts schedule is claimable");
    sqlx::query(
        "UPDATE dca_schedules SET lease_expires_at = now() - interval '1 second' WHERE id = $1",
    )
    .bind(&attempts_id)
    .execute(pool)
    .await
    .expect("force the lease to lapse");
    let charged = scope_tenant_context(
        org.as_uuid(),
        dca_worker::record_failure(pool, attempts_schedule, "replica-1", "storage hiccup"),
    )
    .await
    .expect("record_failure returns");
    assert_eq!(charged, 0, "a lapsed lease cannot charge an attempt");
    let (status, _, _, attempts) = row(pool, &attempts_id).await;
    assert_eq!(status, "active");
    assert_eq!(
        attempts, 0,
        "the schedule is not pushed toward MAX_ATTEMPTS"
    );

    // ── and a LIVE holder still charges its own attempt ─────────────────────
    let claimed = scope_tenant_context(
        org.as_uuid(),
        dca_worker::claim_due(pool, "replica-1", 30.0),
    )
    .await
    .expect("re-claim with a live lease");
    let live_attempt = claimed
        .iter()
        .find(|s| s.id == attempts_id)
        .expect("the schedule is still claimable");
    let charged = scope_tenant_context(
        org.as_uuid(),
        dca_worker::record_failure(pool, live_attempt, "replica-1", "storage hiccup"),
    )
    .await
    .expect("record_failure returns");
    assert_eq!(charged, 1, "the live holder charges its attempt");
    let (_, _, owner, attempts) = row(pool, &attempts_id).await;
    assert_eq!(attempts, 1);
    assert_eq!(owner, None, "and releases the lease");

    sqlx::query("DELETE FROM dca_runs WHERE schedule_id = ANY($1)")
        .bind(vec![
            paused.clone(),
            cancelled.clone(),
            lapsed.clone(),
            live.clone(),
            exhausted.clone(),
            attempts_id.clone(),
        ])
        .execute(pool)
        .await
        .expect("cleanup runs");
    sqlx::query("DELETE FROM dca_schedules WHERE organization_id = $1")
        .bind(org.as_uuid())
        .execute(pool)
        .await
        .expect("cleanup schedules");
}
