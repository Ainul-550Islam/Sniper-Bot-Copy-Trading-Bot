//! Tenant recovery reads must not truncate silently, and a lifecycle row's
//! organization must come from the ROW, not from the caller's scope (gap #78),
//! against REAL PostgreSQL.
//!
//! Two defects:
//!
//!   * `TenantLifecycleRepo::list_open` selected non-settled attempts with a
//!     bare `LIMIT 5000` and no cursor and no marker. It is the crash-recovery
//!     input, so a tenant with more than 5,000 unsettled attempts lost the rest
//!     from the recovery pass with nothing said. Same shape in the copy
//!     open-links and Polymarket open-orders reads.
//!   * the lifecycle mapper stamped `organization_id: scope.organization_id()`
//!     on EVERY row instead of reading the stored column, so
//!     `assert_rows_org` could never fail — the tenant assertion was decorative
//!     and a mapping mistake would silently present a foreign row as the
//!     caller's.
//!
//! Gated on `POSTGRES_URL`.

use std::sync::Arc;

use bot_core::config::DatabaseConfig;
use bot_core::db::Database;
use bot_core::tenant::OrganizationId;
use bot_core::trading_repository::copy::TenantCopyRead;
use bot_core::trading_repository::executions::TenantLifecycleRepo;
use bot_core::trading_repository::polymarket::TenantPolyRead;
use bot_core::trading_repository::query_scope::TradingQueryScope;
use bot_core::trading_repository::repository_error::RepositoryError;
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
        .bind(format!("bounds-{id}"))
        .bind("recovery bounds witness")
        .execute(pool)
        .await
        .expect("organization insert");
    id
}

async fn seed_lifecycle(pool: &PgPool, org: Uuid, id: &str, secs: i64) {
    sqlx::query(
        "INSERT INTO execution_lifecycle
            (organization_id, intent_id, module, label, wallet, symbol, state, attempts, updated_at)
         VALUES ($1, $2, 'sniper', 'bounds', 'wallet', 'BOUNDSMINT', 'pending', 1,
                 now() - interval '1 hour' + make_interval(secs => $3::float8))",
    )
    .bind(org)
    .bind(id)
    .bind(secs as f64)
    .execute(pool)
    .await
    .expect("lifecycle insert");
}

#[tokio::test]
async fn unsettled_lifecycle_reads_page_and_report_truncation() {
    let Some(db) = setup().await else {
        eprintln!("NOT_RUN: recovery_bounds_pg — POSTGRES_URL missing");
        return;
    };
    let pool = db.pool();
    let a = org(pool).await;
    let b = org(pool).await;
    let run = Uuid::new_v4().to_string();
    // 4 unsettled attempts for A (three pages at the 2-row test page size), one
    // settled attempt that must never be listed, and one for B.
    for i in 0..4 {
        seed_lifecycle(pool, a, &format!("lb-open-{run}-{i}"), i).await;
    }
    sqlx::query(
        "INSERT INTO execution_lifecycle
            (organization_id, intent_id, module, state, updated_at)
         VALUES ($1, $2, 'sniper', 'confirmed', now())",
    )
    .bind(a)
    .bind(format!("lb-settled-{run}"))
    .execute(pool)
    .await
    .expect("settled row");
    seed_lifecycle(pool, b, &format!("lb-b-{run}"), 0).await;

    let repo = TenantLifecycleRepo::new(db.clone());
    let scope_a = TradingQueryScope::new(OrganizationId::from(a));

    // ── 1. the recovery input PAGES and every unsettled attempt is reachable ─
    let mut seen: Vec<String> = Vec::new();
    let mut cursor = None;
    let mut pages = 0;
    loop {
        let page = repo
            .open_page(&scope_a, cursor.clone(), 2)
            .await
            .expect("open_page");
        pages += 1;
        assert!(page.items.len() <= 2, "a page never exceeds the page size");
        for item in &page.items {
            assert_ne!(item.state, "confirmed", "a settled attempt is not open");
            assert_eq!(
                item.organization_id,
                OrganizationId::from(a),
                "every row is attributed to its OWNER, read from the row"
            );
        }
        seen.extend(page.items.iter().map(|i| i.intent_id.clone()));
        cursor = page.next_cursor.clone();
        if page.items.is_empty() {
            assert!(!page.has_more, "an empty page is the end of the walk");
            break;
        }
        assert!(pages < 10, "paging terminates");
    }
    assert_eq!(pages, 3, "4 unsettled attempts over 2-row pages: +,+,empty");
    let mut deduped = seen.clone();
    deduped.sort();
    deduped.dedup();
    assert_eq!(deduped.len(), seen.len(), "no attempt is listed twice");
    assert_eq!(seen.len(), 4, "every unsettled attempt is reachable");
    assert!(
        !seen.iter().any(|i| i == &format!("lb-settled-{run}")),
        "and the settled attempt is never part of the recovery input"
    );
    assert!(
        !seen.iter().any(|i| i == &format!("lb-b-{run}")),
        "nor is another tenant's"
    );

    // ── 2. truncation is VISIBLE, not silent ────────────────────────────────
    let first = repo.open_page(&scope_a, None, 2).await.expect("first page");
    assert!(first.has_more, "a page that is not the whole story says so");
    assert_eq!(
        first.total_remaining, 4,
        "and reports how much work the cutoff window still holds"
    );
    let all = repo.open_page(&scope_a, None, 100).await.expect("one page");
    assert!(!all.has_more, "a complete page reports has_more = false");
    assert_eq!(all.total_remaining, 4);

    // ── 3. the legacy `list_open` no longer hides the cap ───────────────────
    // It is a first-page convenience over the same statement: it returns the
    // page it fetched and logs the truncation, so nothing is claimed that the
    // query did not return.
    let legacy = repo.list_open(&scope_a).await.expect("list_open");
    assert_eq!(legacy.len(), 4, "list_open returns the unsettled attempts");
    assert!(legacy
        .iter()
        .all(|r| r.organization_id == OrganizationId::from(a)));

    // ── 4. the tenant assertion now has TEETH (the mapping is not a stamp) ──
    // A row whose stored organization differs from the scope is refused rather
    // than relabelled as the caller's.
    let foreign = repo.ownership_from_row(&scope_a, Some(b));
    assert!(
        matches!(foreign, Err(RepositoryError::TenantMismatch)),
        "a foreign row is refused, not relabelled: {foreign:?}"
    );
    assert!(
        matches!(
            repo.ownership_from_row(&scope_a, None),
            Err(RepositoryError::TenantMismatch)
        ),
        "an unattributed row (NULL organization) is refused too"
    );
    assert_eq!(
        repo.ownership_from_row(&scope_a, Some(a)).unwrap(),
        OrganizationId::from(a)
    );

    sqlx::query("DELETE FROM execution_lifecycle WHERE organization_id = ANY($1)")
        .bind(vec![a, b])
        .execute(pool)
        .await
        .expect("cleanup");
}

#[tokio::test]
async fn copy_and_polymarket_recovery_reads_page_without_silent_caps() {
    let Some(db) = setup().await else {
        eprintln!("NOT_RUN: recovery_bounds_pg — POSTGRES_URL missing");
        return;
    };
    let pool = db.pool();
    let a = org(pool).await;
    let run = Uuid::new_v4().to_string();
    // Five open copy links + two closed ones that must never be listed.
    for i in 0..5 {
        sqlx::query(
            "INSERT INTO copy_links
                (organization_id, position_id, leader, mint, entry_event_id, entry_signature,
                 status, opened_at)
             VALUES ($1, $2, 'leader', 'MINT', $3, 'sig', 'open',
                     now() - interval '1 hour' + make_interval(secs => $4::float8))",
        )
        .bind(a)
        .bind(format!("link-{run}-{i}"))
        .bind(format!("ev-{run}-{i}"))
        .bind(i as f64)
        .execute(pool)
        .await
        .expect("copy link");
    }
    for i in 0..2 {
        sqlx::query(
            "INSERT INTO copy_links
                (organization_id, position_id, leader, mint, entry_event_id, entry_signature,
                 status, opened_at)
             VALUES ($1, $2, 'leader', 'MINT', $3, 'sig', 'closed', now())",
        )
        .bind(a)
        .bind(format!("link-closed-{run}-{i}"))
        .bind(format!("ev-closed-{run}-{i}"))
        .execute(pool)
        .await
        .expect("closed copy link");
    }

    let copy = TenantCopyRead::new(db.clone());
    let scope = TradingQueryScope::new(OrganizationId::from(a));
    let page1 = copy
        .open_links_page(&scope, None, 2)
        .await
        .expect("open_links_page");
    assert_eq!(page1.items.len(), 2, "the page size is honoured");
    assert!(page1.has_more, "more open links exist");
    assert_eq!(page1.total_remaining, 5, "and the total is honest");
    let page2 = copy
        .open_links_page(&scope, page1.next_cursor.clone(), 2)
        .await
        .expect("page 2");
    let page3 = copy
        .open_links_page(&scope, page2.next_cursor.clone(), 5)
        .await
        .expect("page 3");
    let mut ids: Vec<String> = page1
        .items
        .iter()
        .chain(page2.items.iter())
        .chain(page3.items.iter())
        .map(|l| l.position_id.clone())
        .collect();
    ids.sort();
    ids.dedup();
    assert_eq!(
        ids.len(),
        5,
        "all five open links are reachable, none twice"
    );
    assert!(
        !ids.iter().any(|i| i.contains("closed")),
        "closed links are not reconciliation input"
    );
    // The legacy first-page read reports truncation instead of hiding it.
    let legacy = copy.open_links(&scope).await.expect("open_links");
    assert_eq!(legacy.len(), 5, "one page covers all five here");

    // ── Polymarket open mirror orders ───────────────────────────────────────
    for i in 0..4 {
        sqlx::query(
            "INSERT INTO poly_orders
                (organization_id, venue_order_id, order_id, signal_id, condition_id, token_id,
                 outcome, side, limit_price, size_tokens, mode, state, submitted_at)
             VALUES ($1, $2, $3, 'sig', 'cond', 'token', 'yes', 'buy', 0.5, 10, 'paper', 'live',
                     now() - interval '1 hour' + make_interval(secs => $4::float8))",
        )
        .bind(a)
        .bind(format!("venue-{run}-{i}"))
        .bind(format!("order-{run}-{i}"))
        .bind(i as f64)
        .execute(pool)
        .await
        .expect("poly order");
    }
    let poly = TenantPolyRead::new(db.clone());
    let p1 = poly
        .open_orders_page(&scope, None, 3)
        .await
        .expect("open_orders_page");
    assert_eq!(p1.items.len(), 3);
    assert!(p1.has_more, "a fourth open order exists");
    assert_eq!(p1.total_remaining, 4);
    let p2 = poly
        .open_orders_page(&scope, p1.next_cursor.clone(), 3)
        .await
        .expect("poly page 2");
    assert_eq!(p2.items.len(), 1);
    assert!(!p2.has_more, "the last page says it is the last");
    assert!(p2.total_remaining <= 3);
    let mut venues: Vec<String> = p1
        .items
        .iter()
        .chain(p2.items.iter())
        .map(|o| o.venue_order_id.clone())
        .collect();
    venues.sort();
    venues.dedup();
    assert_eq!(venues.len(), 4, "every open mirror order is reachable once");

    sqlx::query("DELETE FROM copy_links WHERE organization_id = $1")
        .bind(a)
        .execute(pool)
        .await
        .expect("cleanup links");
    sqlx::query("DELETE FROM poly_orders WHERE organization_id = $1")
        .bind(a)
        .execute(pool)
        .await
        .expect("cleanup poly");
}
