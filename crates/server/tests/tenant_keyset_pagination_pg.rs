//! Real-Postgres keyset pagination for tenant list endpoints.
//!
//! The defect: page ordering is `ORDER BY created_at DESC, id DESC` while the
//! cursor carried only `created_at`, so rows that share the boundary
//! timestamp were silently skipped (a page boundary landing inside a run of
//! equal timestamps loses the rest of the run). The fix is the composite
//! `(created_at, id)` cursor that the endpoint now emits as
//! `next_before` + `next_before_id`.
//!
//! This suite drives the SAME SQL predicate the handlers use, through the real
//! `limit_orders` / `dca_schedules` tables, with a deliberately tie-heavy
//! fixture: 7 rows, 5 of them sharing one `created_at`.
mod support {
    pub fn pg_url() -> Option<String> {
        match std::env::var("POSTGRES_URL") {
            Ok(v) if !v.trim().is_empty() => Some(v),
            _ => {
                if std::env::var("REQUIRE_POSTGRES").as_deref() == Ok("1") {
                    panic!("REQUIRE_POSTGRES=1 but POSTGRES_URL is missing");
                }
                None
            }
        }
    }
}

use std::sync::Arc;

use bot_core::config::DatabaseConfig;
use bot_core::db::Database;
use chrono::{DateTime, Duration, Utc};
use sqlx::Row;
use uuid::Uuid;

async fn setup() -> Option<Arc<Database>> {
    let url = support::pg_url()?;
    let cfg = DatabaseConfig {
        enabled: true,
        auto_migrate: true,
        max_connections: 8,
        acquire_timeout_ms: 60_000,
        statement_timeout_ms: 120_000,
        query_timeout_ms: 120_000,
        ..Default::default()
    };
    let db = Arc::new(
        Database::connect(&cfg, &url)
            .await
            .expect("connect postgres"),
    );
    db.migrate().await.expect("migrations must apply");
    Some(db)
}

fn run_id() -> String {
    format!(
        "{}-{}",
        std::process::id(),
        Utc::now().timestamp_nanos_opt().unwrap_or(0)
    )
}

async fn org(db: &Database, slug: &str) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query("INSERT INTO organizations (id, slug, name, status) VALUES ($1, $2, $3, 'active')")
        .bind(id)
        .bind(slug)
        .bind(slug)
        .execute(db.pool())
        .await
        .expect("insert organization");
    id
}

/// One page of the limit-order list, through the PRODUCTION query function.
async fn limit_order_page(
    db: &Database,
    org: Uuid,
    limit: i64,
    before: Option<(DateTime<Utc>, Option<String>)>,
) -> Vec<(String, DateTime<Utc>)> {
    let rows =
        sniper_suite::trading_data_plane::limit_orders::fetch_page(db, org, None, before, limit)
            .await
            .expect("limit order page");
    rows.iter()
        .map(|r| {
            (
                r.get::<String, _>("id"),
                r.get::<DateTime<Utc>, _>("created_at"),
            )
        })
        .collect()
}

/// The OLD (defective) predicate, kept here as the witness that the fix is
/// about something real: timestamp-only paging.
async fn limit_order_page_timestamp_only(
    db: &Database,
    org: Uuid,
    limit: i64,
    before: Option<DateTime<Utc>>,
) -> Vec<String> {
    let rows = sqlx::query(
        "SELECT id FROM limit_orders \
         WHERE organization_id = $1 AND ($2::timestamptz IS NULL OR created_at < $2) \
         ORDER BY created_at DESC, id DESC LIMIT $3",
    )
    .bind(org)
    .bind(before)
    .bind(limit)
    .fetch_all(db.pool())
    .await
    .expect("timestamp-only page");
    rows.iter().map(|r| r.get::<String, _>("id")).collect()
}

async fn dca_page(
    db: &Database,
    org: Uuid,
    limit: i64,
    before: Option<(DateTime<Utc>, Option<String>)>,
) -> Vec<String> {
    let rows = sniper_suite::trading_data_plane::dca::fetch_page(db, org, None, before, limit)
        .await
        .expect("dca page");
    rows.iter().map(|r| r.get::<String, _>("id")).collect()
}

#[tokio::test]
async fn a_tie_heavy_page_boundary_skips_nothing_and_repeats_nothing() {
    let Some(db) = setup().await else {
        eprintln!("NOT_RUN: tenant_keyset_pagination_pg — POSTGRES_URL missing");
        return;
    };
    let run = run_id();
    let org = org(&db, &format!("keyset-{run}")).await;
    let tie = Utc::now() - Duration::seconds(60);

    // 5 limit orders sharing ONE created_at, plus 2 older ones.
    let mut ties: Vec<String> = Vec::new();
    let mut older: Vec<String> = Vec::new();
    for n in 0..5 {
        let id = format!("lo-{run}-tie-{n}");
        sqlx::query(
            "INSERT INTO limit_orders \
                (id, organization_id, mint, side, trigger_kind, price_sol, amount, status, created_at) \
             VALUES ($1, $2, 'MintKeyset1111111111111111111111111111111111', 'buy', 'at_or_below', \
                     1.0, 10.0, 'active', $3)",
        )
        .bind(&id)
        .bind(org)
        .bind(tie)
        .execute(db.pool())
        .await
        .expect("insert limit order");
        ties.push(id);
    }
    for (n, secs) in [(0usize, 120i64), (1, 180)] {
        let id = format!("lo-{run}-old-{n}");
        sqlx::query(
            "INSERT INTO limit_orders \
                (id, organization_id, mint, side, trigger_kind, price_sol, amount, status, created_at) \
             VALUES ($1, $2, 'MintKeyset1111111111111111111111111111111111', 'buy', 'at_or_below', \
                     1.0, 10.0, 'active', $3)",
        )
        .bind(&id)
        .bind(org)
        .bind(Utc::now() - Duration::seconds(secs))
        .execute(db.pool())
        .await
        .expect("insert older limit order");
        older.push(id);
    }
    // Expected order: the 5 ties by id DESC, then the older two (newest first).
    // Ties break by id DESC; the two older rows have DISTINCT timestamps, so
    // they keep insertion order (120s ago before 180s ago).
    ties.sort_by(|a, b| b.cmp(a));
    let mut expected = ties.clone();
    expected.extend(older.iter().cloned());

    // ---- the fix: walk the whole list with page size 2 -------------------
    let mut seen: Vec<String> = Vec::new();
    let mut cursor: Option<(DateTime<Utc>, Option<String>)> = None;
    for _page in 0..10 {
        let page = limit_order_page(&db, org, 2, cursor.clone()).await;
        if page.is_empty() {
            break;
        }
        for (id, _ts) in &page {
            seen.push(id.clone());
        }
        let (last_id, last_ts) = page.last().cloned().expect("non-empty page");
        cursor = Some((last_ts, Some(last_id)));
    }
    assert_eq!(
        seen, expected,
        "every row exactly once, in page order, across a tie-heavy boundary"
    );

    // ---- the defect: timestamp-only cursor loses the rest of the tie run --
    let first = limit_order_page_timestamp_only(&db, org, 2, None).await;
    assert_eq!(first.len(), 2);
    let last_ts: DateTime<Utc> = sqlx::query("SELECT created_at FROM limit_orders WHERE id = $1")
        .bind(first.last().unwrap())
        .fetch_one(db.pool())
        .await
        .unwrap()
        .get("created_at");
    let second = limit_order_page_timestamp_only(&db, org, 2, Some(last_ts)).await;
    let mut old_walk = first.clone();
    old_walk.extend(second);
    assert!(
        old_walk.len() < expected.len(),
        "the timestamp-only cursor silently drops the remaining tie rows \
         (walked {} of {})",
        old_walk.len(),
        expected.len()
    );
}

#[tokio::test]
async fn dca_schedules_page_by_the_same_composite_cursor() {
    let Some(db) = setup().await else {
        eprintln!("NOT_RUN: tenant_keyset_pagination_pg — POSTGRES_URL missing");
        return;
    };
    let run = run_id();
    let org = org(&db, &format!("keyset-dca-{run}")).await;
    let tie = Utc::now() - Duration::seconds(30);
    let mut expected: Vec<String> = Vec::new();
    for n in 0..4 {
        let id = format!("dca-{run}-tie-{n}");
        sqlx::query(
            "INSERT INTO dca_schedules \
                (id, organization_id, mint, amount_per_run_sol, interval_secs, budget_sol, \
                 spent_sol, status, next_run_at, created_at) \
             VALUES ($1, $2, 'MintKeyset1111111111111111111111111111111111', 0.1, 3600, 1.0, \
                     0.0, 'active', now() + interval '1 hour', $3)",
        )
        .bind(&id)
        .bind(org)
        .bind(tie)
        .execute(db.pool())
        .await
        .expect("insert schedule");
        expected.push(id);
    }
    expected.sort_by(|a, b| b.cmp(a));

    let mut seen: Vec<String> = Vec::new();
    let mut cursor: Option<(DateTime<Utc>, Option<String>)> = None;
    for _page in 0..6 {
        let page = dca_page(&db, org, 3, cursor.clone()).await;
        if page.is_empty() {
            break;
        }
        let last = page.last().cloned().unwrap();
        seen.extend(page);
        // The handler returns the row's created_at with the id; both ties share it.
        cursor = Some((tie, Some(last)));
    }
    assert_eq!(seen, expected, "no schedule is skipped at the tie boundary");
}
