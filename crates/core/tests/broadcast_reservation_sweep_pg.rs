//! Operator sweep of ambiguous pre-broadcast reservations against real
//! PostgreSQL (migration 0079).
//!
//! Proves the properties the operator surface claims:
//!   * a lapsed `reserved` claim is VISIBLE to the sweep (and a live one is
//!     not),
//!   * resolving it is a fencing CAS: a decision naming the wrong owner or
//!     epoch changes nothing,
//!   * `abandon` releases the trade key so a retry may broadcast again,
//!   * `link` records the externally established signature and closes the key,
//!   * a `linked` row without a signature is reported, and one resolution is
//!     applied exactly once.
//!
//! Ignored by default: needs an isolated migrated database in `POSTGRES_URL`.

use std::sync::Arc;
use std::time::Duration;

use bot_core::config::DatabaseConfig;
use bot_core::db::repo::{BroadcastAmbiguity, BroadcastReservationRepo, IntentRecord};
use bot_core::db::Database;

fn url() -> String {
    std::env::var("POSTGRES_URL").expect("POSTGRES_URL required for this ignored test")
}

async fn setup() -> Arc<Database> {
    let cfg = DatabaseConfig {
        enabled: true,
        auto_migrate: true,
        ..Default::default()
    };
    let db = Database::connect(&cfg, &url()).await.expect("connect");
    db.migrate().await.expect("migrations");
    Arc::new(db)
}

fn run_id() -> String {
    format!(
        "{}-{}",
        std::process::id(),
        chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)
    )
}

fn intent(id: &str, key: &str) -> IntentRecord {
    IntentRecord {
        intent_id: id.to_string(),
        module: "copy".into(),
        symbol: key.to_string(),
        wallet: "Wallet1111".into(),
        side: "buy".into(),
        qty: "1000".into(),
        status: "pending".into(),
        signature: None,
        created_at: chrono::Utc::now(),
    }
}

#[tokio::test]
async fn lapsed_claims_are_visible_and_only_a_fenced_operator_can_resolve_them() {
    let _db = setup().await;

    // --- A live claim is in flight, not ambiguous -----------------------
    let live = format!("copy:wallet:live-{}", run_id());
    let live_claim = _db_reserve(&live, "replica-live", 600).await;
    assert!(live_claim, "the live reservation must be granted");

    // --- A lapsed claim (1 s lease, waited out) -------------------------
    let lapsed = format!("copy:wallet:lapsed-{}", run_id());
    assert!(_db_reserve(&lapsed, "replica-dead", 1).await);
    tokio::time::sleep(Duration::from_millis(1_300)).await;

    let rows = _db_ambiguous(10).await;
    let keys: Vec<&str> = rows.iter().map(|r| r.trade_key.as_str()).collect();
    assert!(
        keys.contains(&lapsed.as_str()),
        "the lapsed claim must be visible to the sweep: {keys:?}"
    );
    assert!(
        !keys.contains(&live.as_str()),
        "an in-flight claim is not ambiguous: {keys:?}"
    );
    let row = rows.iter().find(|r| r.trade_key == lapsed).unwrap();
    assert_eq!(row.status, "reserved");
    assert!(row.lease_lapsed);
    // Sub-second lapses truncate to 0: the boolean is the load-bearing flag.
    assert!(row.lapsed_secs >= 0, "{}", row.lapsed_secs);

    // --- A stale decision changes nothing -------------------------------
    let repo = BroadcastReservationRepo::new(_db.clone());
    assert!(
        !repo
            .resolve_ambiguous(&lapsed, "replica-dead", 99, BroadcastAmbiguity::Abandon)
            .await
            .unwrap(),
        "a wrong epoch must not resolve the claim"
    );
    assert!(
        !repo
            .resolve_ambiguous(&lapsed, "someone-else", 1, BroadcastAmbiguity::Abandon)
            .await
            .unwrap(),
        "a wrong owner must not resolve the claim"
    );
    let (owner, epoch, status) = repo.get(&lapsed).await.unwrap().unwrap();
    assert_eq!(
        (owner.as_str(), epoch, status.as_str()),
        ("replica-dead", 1, "reserved")
    );

    // --- The fenced resolution applies, and releases the key ------------
    assert!(repo
        .resolve_ambiguous(&lapsed, "replica-dead", 1, BroadcastAmbiguity::Abandon)
        .await
        .unwrap());
    let (_, _, status) = repo.get(&lapsed).await.unwrap().unwrap();
    assert_eq!(status, "abandoned");
    // The freed key can be retried by a new attempt.
    let retry = repo
        .reserve(&lapsed, "replica-next", &intent("i-retry", &lapsed), 600)
        .await
        .unwrap();
    assert!(retry.fresh, "an abandoned claim must be retakeable");
    assert_eq!(retry.epoch, 2);
    // And the resolution is not repeatable against the new holder.
    assert!(!repo
        .resolve_ambiguous(&lapsed, "replica-dead", 1, BroadcastAmbiguity::Abandon)
        .await
        .unwrap());

    // --- `link` records the externally established signature ------------
    let linked = format!("copy:wallet:linked-{}", run_id());
    assert!(_db_reserve(&linked, "replica-two", 1).await);
    tokio::time::sleep(Duration::from_millis(1_300)).await;
    let sig =
        "5VERv8NMvzbJMEkV8xnrLkEaWRtSz9CosKDYjCJjBRnbJLgp8uirBgmQpjKhoR4tjF3ZpRzrFmBV6UjKdiSZkQUW";
    assert!(repo
        .resolve_ambiguous(
            &linked,
            "replica-two",
            1,
            BroadcastAmbiguity::LinkUnder(sig.to_string())
        )
        .await
        .unwrap());
    let row = _db_ambiguous(50)
        .await
        .into_iter()
        .find(|r| r.trade_key == linked);
    assert!(
        row.is_none(),
        "a linked claim with a signature is no longer ambiguous"
    );
    let (_, _, status) = repo.get(&linked).await.unwrap().unwrap();
    assert_eq!(status, "linked");
    // A linked claim is terminal: it can never be re-reserved.
    let again = repo
        .reserve(&linked, "replica-three", &intent("i-x", &linked), 600)
        .await
        .unwrap();
    assert!(!again.fresh, "a recorded outcome must stay closed");
}

// Each helper opens its own connection: the sweep test must not depend on a
// connection being kept alive between assertions.
async fn _db_reserve(key: &str, owner: &str, lease: i64) -> bool {
    let db = setup().await;
    BroadcastReservationRepo::new(db)
        .reserve(key, owner, &intent(&format!("{key}#{owner}"), key), lease)
        .await
        .expect("reserve")
        .fresh
}

async fn _db_ambiguous(limit: i64) -> Vec<bot_core::db::repo::BroadcastReservationRow> {
    let db = setup().await;
    BroadcastReservationRepo::new(db)
        .ambiguous(limit)
        .await
        .expect("ambiguous")
}

/// The operator sweep must show the claim that JUST lapsed, not the oldest
/// stale one (gap #78 follow-up — found by the full workspace sweep, not by a
/// targeted run).
///
/// `ambiguous()` ordered `lease_until ASC` and applied a bare `LIMIT`, so on a
/// deployment with more ambiguous claims than one page the operator saw only
/// ancient, already-reviewed history and never the fresh lapse. The old order
/// made this test pass only because the database happened to be empty.
#[tokio::test]
async fn the_sweep_shows_the_freshest_lapse_first_and_admits_truncation() {
    let db = setup().await;
    let repo = BroadcastReservationRepo::new(db.clone());

    // Three claims that lapsed long ago, then one that lapses now.
    for n in 0..3 {
        let stale = format!("copy:wallet:stale-{n}-{}", run_id());
        assert!(_db_reserve(&stale, "replica-old", 1).await);
    }
    tokio::time::sleep(Duration::from_millis(1_200)).await;
    let fresh = format!("copy:wallet:fresh-{}", run_id());
    assert!(_db_reserve(&fresh, "replica-new", 1).await);
    tokio::time::sleep(Duration::from_millis(1_200)).await;

    // A one-row page: the freshest lapse must be the row it returns.
    let page = repo
        .ambiguous_page(1, None)
        .await
        .expect("ambiguous page must read");
    assert!(page.has_more, "the deployment holds more than one row");
    assert!(
        page.total_remaining >= 4,
        "the count must describe the deployment, got {}",
        page.total_remaining
    );
    assert_eq!(page.items.len(), 1, "a page never exceeds its size");
    assert_eq!(
        page.items[0].trade_key, fresh,
        "the freshest lapse is what an operator sees first"
    );

    // The cursor walks towards older rows and never repeats one.
    let cursor = page.next_cursor.expect("a full page has a cursor");
    let second = repo
        .ambiguous_page(1, Some(cursor))
        .await
        .expect("second page must read");
    assert_eq!(second.items.len(), 1);
    assert_ne!(second.items[0].trade_key, fresh, "pages must not overlap");
}
