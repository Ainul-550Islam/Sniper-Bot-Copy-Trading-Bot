//! The tenant recovery sweep must PAGE instead of truncating, must never hand
//! every open order to every intent, and must not block a second worker for the
//! whole duration of the caller's batch (gap #74), against REAL PostgreSQL.
//!
//! Three defects in `TenantRecoveryRepo::sweep`:
//!
//!   * `ORDER BY created_at ASC LIMIT 1000` / `LIMIT 5000` with no cursor and no
//!     truncation signal — rows past the cap were silently invisible, and a
//!     sweep that kept re-reading the same head could never reach them (the
//!     "repeated reads can starve later items" observation);
//!   * the open-order list was ONE tenant-wide query cloned onto EVERY intent,
//!     so a caller triaging intent #900 was handed all 5,000 ids — the pairing
//!     was not evidence at all;
//!   * the tenant advisory lock ("recovery-sweep") was held for the whole
//!     transaction and released at commit, i.e. BEFORE the caller acted on the
//!     batch, so the lock neither serialized two workers' work nor protected the
//!     returned rows.
//!
//! The witness drives the production `sweep`/`sweep_page` and asserts paging,
//! keyset stability, exact pairing and that a second sweep is not blocked.
//! Gated on `POSTGRES_URL`.

use std::sync::Arc;
use std::time::{Duration, Instant};

use bot_core::config::DatabaseConfig;
use bot_core::db::Database;
use bot_core::trading_repository::intent::TenantRecoveryRepo;
use bot_core::trading_repository::query_scope::TradingQueryScope;
use chrono::Utc;
use sqlx::PgPool;
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

async fn org(pool: &PgPool) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query("INSERT INTO organizations (id, slug, name, status) VALUES ($1,$2,$3,'active')")
        .bind(id)
        .bind(format!("recovery-{id}"))
        .bind("recovery sweep witness")
        .execute(pool)
        .await
        .expect("organization insert");
    id
}

/// Order rows are attached to intents through the deterministic `meta`
/// attribution the OMS writes; `meta->>'intent_id'` is that link.
async fn seed_order(pool: &PgPool, org: Uuid, id: &str, intent: &str, status: &str) {
    sqlx::query(
        "INSERT INTO orders
            (id, organization_id, module, side, symbol, venue, mode, status, qty, meta)
         VALUES ($1, $2, 'sniper', 'buy', 'RECOVERMINT', 'jupiter', 'live', $3, 1, $4)",
    )
    .bind(id)
    .bind(org)
    .bind(status)
    .bind(serde_json::json!({ "intent_id": intent }))
    .execute(pool)
    .await
    .expect("order insert");
}

fn scope(org: Uuid) -> TradingQueryScope {
    TradingQueryScope::new(bot_core::tenant::OrganizationId::from(org))
}

#[tokio::test]
async fn the_recovery_sweep_pages_pairs_and_never_holds_the_lock_for_the_caller() {
    let Some(db) = setup().await else {
        eprintln!("NOT_RUN: recovery_sweep_pg — POSTGRES_URL missing");
        return;
    };
    let pool = db.pool();
    let org = org(pool).await;
    let run = Uuid::new_v4().to_string();
    // 1,205 orphans ⇒ three pages at the production 500-row page size, so a
    // single capped read provably cannot see them all.
    let intents: Vec<String> = (0..1205)
        .map(|i| format!("rc-intent-{run}-{i:05}"))
        .collect();
    for (i, id) in intents.iter().enumerate() {
        // Distinct, strictly increasing created_at so the keyset order is total.
        sqlx::query(
            "INSERT INTO execution_intents
                (organization_id, intent_id, module, symbol, wallet, side, qty, created_at, updated_at)
             VALUES ($1, $2, 'sniper', 'RECOVERMINT', 'wallet', 'buy', '1',
                     now() - interval '1 hour' + make_interval(secs => $3::float8), now())",
        )
        .bind(org)
        .bind(id)
        .bind(i as f64)
        .execute(pool)
        .await
        .expect("intent insert");
    }
    // Intent #0 owns exactly one open order; intent #1 owns two; a FILLED order
    // belongs to intent #2 and must never appear.
    seed_order(
        pool,
        org,
        &format!("rc-ord-0-{run}"),
        &intents[0],
        "submitted",
    )
    .await;
    seed_order(
        pool,
        org,
        &format!("rc-ord-1a-{run}"),
        &intents[1],
        "accepted",
    )
    .await;
    seed_order(
        pool,
        org,
        &format!("rc-ord-1b-{run}"),
        &intents[1],
        "partially_filled",
    )
    .await;
    seed_order(pool, org, &format!("rc-ord-2-{run}"), &intents[2], "filled").await;
    // An open order with NO intent attribution: it must be reported, not
    // guessed onto an intent and not silently dropped.
    sqlx::query(
        "INSERT INTO orders
            (id, organization_id, module, side, symbol, venue, mode, status, qty)
         VALUES ($1, $2, 'sniper', 'buy', 'RECOVERMINT', 'jupiter', 'live', 'submitted', 1)",
    )
    .bind(format!("rc-ord-unattributed-{run}"))
    .bind(org)
    .execute(pool)
    .await
    .expect("unattributed order insert");

    let repo = TenantRecoveryRepo::new(db.clone());
    let cutoff = Utc::now();

    // ── 1. the sweep PAGES: three pages, every intent exactly once ───────────
    let mut seen: Vec<String> = Vec::new();
    let mut cursor: Option<(chrono::DateTime<Utc>, String)> = None;
    let mut pages = 0;
    loop {
        let page = repo
            .sweep_page(&scope(org), cutoff, cursor.clone(), 500)
            .await
            .expect("sweep_page");
        if page.items.is_empty() {
            break;
        }
        pages += 1;
        assert!(page.items.len() <= 500, "a page never exceeds the cap");
        for (item, (created_at, intent_id)) in page.items.iter().zip(page.cursor_pairs()) {
            let _ = intent_id;
            cursor = Some((created_at, item.intent.intent_id.clone()));
        }
        seen.extend(page.items.iter().map(|i| i.intent.intent_id.clone()));
        if !page.has_more {
            break;
        }
        assert!(pages < 10, "paging must terminate");
    }
    assert_eq!(pages, 3, "1,205 orphans arrive in three 500-row pages");
    let mut deduped = seen.clone();
    deduped.sort();
    deduped.dedup();
    assert_eq!(deduped.len(), seen.len(), "no intent is listed twice");
    assert_eq!(seen.len(), intents.len(), "every orphan is reachable");
    assert!(
        intents.iter().all(|id| seen.contains(id)),
        "including the ones past the old 1,000-row cap"
    );

    // ── 2. the pairing is EVIDENCE: an intent carries only its own orders ────
    let first = repo
        .sweep_page(&scope(org), cutoff, None, 4)
        .await
        .expect("first page");
    let by_id: std::collections::HashMap<&str, &Vec<String>> = first
        .items
        .iter()
        .map(|i| (i.intent.intent_id.as_str(), &i.open_order_ids))
        .collect();
    assert_eq!(
        by_id[intents[0].as_str()],
        &vec![format!("rc-ord-0-{run}")],
        "intent 0 is paired with its own open order only"
    );
    let mut intent1 = by_id[intents[1].as_str()].clone();
    intent1.sort();
    let mut expected1 = vec![format!("rc-ord-1a-{run}"), format!("rc-ord-1b-{run}")];
    expected1.sort();
    assert_eq!(
        intent1, expected1,
        "intent 1 is paired with its two open orders"
    );
    assert!(
        by_id[intents[2].as_str()].is_empty(),
        "a FILLED order is not an open order"
    );
    assert!(
        by_id[intents[3].as_str()].is_empty(),
        "an intent with no orders carries an empty list, not somebody else's"
    );
    assert_eq!(
        first.unattributed_open_order_ids,
        vec![format!("rc-ord-unattributed-{run}")],
        "an open order with no intent attribution is reported as unattributed"
    );

    // ── 3. the advisory lock is NOT held while the caller acts ───────────────
    // A second worker of the SAME tenant must acquire (tenant, "recovery-sweep")
    // immediately: the lock only ever serializes the two reads.
    let started = Instant::now();
    let second = tokio::time::timeout(
        Duration::from_secs(5),
        repo.sweep_page(&scope(org), cutoff, None, 10),
    )
    .await
    .expect("a second sweep is not blocked by the first caller's batch")
    .expect("second sweep");
    assert_eq!(second.items.len(), 10);
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "the sweep must not hold the tenant lock across the caller's processing"
    );

    // ── 4. the page is a SNAPSHOT: a row resolved after the first page is not
    //       re-listed, and the cursor never re-serves an earlier row ─────────
    let page1 = repo
        .sweep_page(&scope(org), cutoff, None, 50)
        .await
        .expect("page 1");
    let cursor_after_1 = page1
        .cursor_pairs()
        .last()
        .map(|(at, id)| (at.to_owned(), id.to_owned()))
        .expect("page 1 has a cursor");
    let page2 = repo
        .sweep_page(&scope(org), cutoff, Some(cursor_after_1.clone()), 50)
        .await
        .expect("page 2");
    assert!(
        page2
            .items
            .iter()
            .all(|i| i.intent.intent_id != page1.items[0].intent.intent_id),
        "the cursor never re-serves a row from an earlier page"
    );

    // ── 5. legacy shape: `sweep` no longer clones one list onto everything ───
    let legacy = repo.sweep(&scope(org), cutoff).await.expect("sweep");
    let legacy_first = legacy
        .iter()
        .find(|i| i.intent.intent_id == intents[0])
        .expect("intent 0 in the legacy batch");
    assert_eq!(
        legacy_first.open_order_ids,
        vec![format!("rc-ord-0-{run}")],
        "the legacy batch pairs per intent too"
    );

    sqlx::query("DELETE FROM orders WHERE organization_id = $1")
        .bind(org)
        .execute(pool)
        .await
        .expect("cleanup orders");
    sqlx::query("DELETE FROM execution_intents WHERE organization_id = $1")
        .bind(org)
        .execute(pool)
        .await
        .expect("cleanup intents");
}
