//! PostgreSQL integration tests (BUILD PLAN §4-iii).
//!
//! GATED: skipped unless `POSTGRES_URL` points at a real, empty database
//! (CI provides one as a service container; locally:
//! `docker run -e POSTGRES_PASSWORD=x -p 5432:5432 postgres:16` then
//! `POSTGRES_URL=postgres://postgres:x@127.0.0.1:5432/postgres cargo test
//!  -p bot-core --test db_integration -- --test-threads=1`).
//!
//! Covers the full durable path end to end: migrations, OMS idempotency +
//! recovery, dedup first-arrival semantics, the audit hash chain (including
//! tamper detection), reconciliation queue mechanics and retention.

use std::sync::Arc;
use std::time::Duration;

use bot_core::config::DatabaseConfig;
use bot_core::db::repo::{
    AuditRepo, CheckpointRepo, ConfigVersionRepo, DedupRepo, IdempotencyRepo, IntentRecord,
    IntentRepo, OrderRepo, PositionRepo, ReconRepo, RiskEventRepo, SystemEventRepo, TradeRepo,
    TransactionRepo, WalletRepo,
};
use bot_core::db::Database;
use bot_core::models::{
    BotModule, ExecutionMode, Position, PositionSide, PositionStatus, Trade, TradeSource, Venue,
};
use bot_core::oms::{OrderDraft, OrderManager, OrderStatus};

fn url() -> Option<String> {
    std::env::var("POSTGRES_URL")
        .ok()
        .filter(|v| !v.trim().is_empty())
}

async fn setup() -> Option<Arc<Database>> {
    let Some(url) = url() else {
        eprintln!("POSTGRES_URL not set — skipping database integration tests");
        return None;
    };
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

/// Unique namespace per test run so parallel CI jobs / repeated runs never
/// collide on unique keys.
fn run_id() -> String {
    format!(
        "{}-{}",
        std::process::id(),
        chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)
    )
}

#[tokio::test]
async fn migrations_apply_and_report() {
    let Some(db) = setup().await else { return };
    let n = db
        .migration_count()
        .await
        .expect("migration table readable");
    assert!(n >= 5, "all embedded migrations applied: {n}");
    db.ping().await.expect("ping");
}

#[tokio::test]
async fn oms_idempotency_survives_a_fresh_manager() {
    let Some(db) = setup().await else { return };
    let run = run_id();
    let key = format!("intent-{run}");

    let mgr1 = OrderManager::new(Some(db.clone()), 1024);
    let order = mgr1
        .create(OrderDraft {
            idempotency_key: key.clone(),
            module: BotModule::Sniper,
            side: "buy".into(),
            symbol: "TEST".into(),
            venue: "pump.fun".into(),
            mode: ExecutionMode::Paper,
            qty: 1.5,
            price: None,
            meta: serde_json::json!({"run": run}),
        })
        .await
        .expect("create");
    mgr1.transition(&order.id, OrderStatus::Submitted, None)
        .await
        .expect("transition");
    mgr1.attach_external(&order.id, None, Some(format!("sig-{run}")))
        .await
        .expect("attach");

    // A "restart": brand-new manager over the same DB.
    let mgr2 = OrderManager::new(Some(db.clone()), 1024);
    let dup = mgr2
        .create(OrderDraft {
            idempotency_key: key.clone(),
            module: BotModule::Sniper,
            side: "buy".into(),
            symbol: "TEST".into(),
            venue: "pump.fun".into(),
            mode: ExecutionMode::Paper,
            qty: 1.5,
            price: None,
            meta: serde_json::json!({}),
        })
        .await
        .expect("duplicate create resolves to the existing order");
    assert_eq!(dup.id, order.id, "duplicate intent collapses");

    let recovered = mgr2.recover_from_db().await.expect("recovery");
    assert!(recovered >= 1, "unfinished order recovered");
    let inc = mgr2.incomplete().await;
    let mine = inc.iter().find(|o| o.id == order.id).expect("present");
    assert_eq!(
        mine.status,
        OrderStatus::Unknown,
        "recovered orders are Unknown until reconciled"
    );
    assert_eq!(
        mine.signature.as_deref(),
        Some(format!("sig-{run}").as_str())
    );
}

#[tokio::test]
async fn dedup_first_arrival_is_exactly_once_across_processes() {
    let Some(db) = setup().await else { return };
    let run = run_id();
    let repo = DedupRepo::new(db.clone());
    assert!(repo
        .mark("sig", &run, Duration::from_secs(60))
        .await
        .expect("mark"));
    assert!(
        !repo
            .mark("sig", &run, Duration::from_secs(60))
            .await
            .expect("mark2"),
        "second arrival is a duplicate"
    );
    assert!(repo.exists("sig", &run).await.expect("exists"));
    repo.forget("sig", &run).await.expect("forget");
    assert!(
        repo.mark("sig", &run, Duration::from_secs(60))
            .await
            .expect("mark3"),
        "after forget the key is new again"
    );

    let idem = IdempotencyRepo::new(db.clone());
    assert!(idem.try_consume("webhook", &run).await.expect("consume"));
    assert!(!idem.try_consume("webhook", &run).await.expect("consume2"));
}

#[tokio::test]
async fn audit_chain_verifies_and_detects_tampering() {
    let Some(db) = setup().await else { return };
    let run = run_id();
    let repo = AuditRepo::new(db.clone());
    for i in 0..5 {
        repo.append(
            "test",
            "action",
            Some(&run),
            "success",
            &serde_json::json!({"i": i}),
        )
        .await
        .expect("append");
    }
    assert_eq!(
        repo.verify_chain().await.expect("verify"),
        None,
        "chain intact after appends"
    );

    // Tamper: rewrite one row's action behind the app's back.
    sqlx::query("UPDATE audit_events SET action = 'TAMPERED' WHERE target = $1 AND id = (SELECT MIN(id) FROM audit_events WHERE target = $1)")
        .bind(&run)
        .execute(db.pool())
        .await
        .expect("direct update (test-only)");
    let broken = repo.verify_chain().await.expect("verify2");
    assert!(broken.is_some(), "tampering MUST be detected: {broken:?}");

    // Test-only cleanup: the deliberately tampered rows must not poison the
    // GLOBAL chain for subsequent runs against the same database (CI uses a
    // fresh container per job; this keeps local repeat runs green). Deleting
    // this run's rows restores the chain head to its pre-test state — the
    // app itself can never do this (audit is immutable from app APIs).
    sqlx::query("DELETE FROM audit_events WHERE target = $1")
        .bind(&run)
        .execute(db.pool())
        .await
        .expect("test-only cleanup");
    assert_eq!(
        repo.verify_chain().await.expect("verify3"),
        None,
        "chain intact again after removing the test's own rows"
    );
}

#[tokio::test]
async fn audit_chain_detects_reorder_missing_and_duplicate() {
    // Regression coverage demanded by the release audit (item: audit /
    // provenance): the hash chain must detect REORDERED, MISSING and
    // DUPLICATED rows — not just modified content (covered by
    // `audit_chain_verifies_and_detects_tampering`). Malformed records are
    // unrepresentable: `outcome` has a CHECK constraint, `detail` is typed
    // jsonb, and every content column feeds the recomputed hash, so any
    // representation change is caught as a modification.
    let Some(db) = setup().await else { return };
    let repo = AuditRepo::new(db.clone());
    let run = run_id();

    // Helper: append a deterministic scenario block, then remove it again.
    // Deleting a scenario's own rows restores the global chain head to its
    // pre-test state (test-only cleanup, exactly as in the tamper test —
    // the app itself can never delete audit rows).
    let t_reorder = format!("{run}-reorder");
    let t_missing = format!("{run}-missing");
    let t_duplicate = format!("{run}-duplicate");

    // ---- scenario 1: REORDER (swap the content of two adjacent rows) -----
    for a in ["s0", "s1", "s2", "s3"] {
        repo.append(
            "test",
            a,
            Some(&t_reorder),
            "success",
            &serde_json::json!({}),
        )
        .await
        .expect("append");
    }
    assert_eq!(repo.verify_chain().await.expect("verify"), None);
    for (from, to) in [("s1", "TMP_SWAP"), ("s2", "s1"), ("TMP_SWAP", "s2")] {
        sqlx::query("UPDATE audit_events SET action = $2 WHERE target = $1 AND action = $3")
            .bind(&t_reorder)
            .bind(to)
            .bind(from)
            .execute(db.pool())
            .await
            .expect("reorder swap (test-only)");
    }
    assert!(
        repo.verify_chain().await.expect("verify reorder").is_some(),
        "reordered rows MUST break the chain"
    );
    sqlx::query("DELETE FROM audit_events WHERE target = $1")
        .bind(&t_reorder)
        .execute(db.pool())
        .await
        .expect("cleanup");
    assert_eq!(repo.verify_chain().await.expect("verify"), None);

    // ---- scenario 2: MISSING row (delete a middle entry) ------------------
    for a in ["s0", "s1", "s2", "s3"] {
        repo.append(
            "test",
            a,
            Some(&t_missing),
            "success",
            &serde_json::json!({}),
        )
        .await
        .expect("append");
    }
    assert_eq!(repo.verify_chain().await.expect("verify"), None);
    sqlx::query("DELETE FROM audit_events WHERE target = $1 AND action = 's2'")
        .bind(&t_missing)
        .execute(db.pool())
        .await
        .expect("delete middle row (test-only)");
    assert!(
        repo.verify_chain().await.expect("verify missing").is_some(),
        "a deleted middle row MUST break the chain (prev_hash link)"
    );
    sqlx::query("DELETE FROM audit_events WHERE target = $1")
        .bind(&t_missing)
        .execute(db.pool())
        .await
        .expect("cleanup");
    assert_eq!(repo.verify_chain().await.expect("verify"), None);

    // ---- scenario 3: DUPLICATE row (re-insert a copy of an entry) ---------
    for a in ["s0", "s1"] {
        repo.append(
            "test",
            a,
            Some(&t_duplicate),
            "success",
            &serde_json::json!({}),
        )
        .await
        .expect("append");
    }
    assert_eq!(repo.verify_chain().await.expect("verify"), None);
    sqlx::query(
        "INSERT INTO audit_events (ts, actor, action, target, outcome, detail, prev_hash, hash)
         SELECT ts, actor, action, target, outcome, detail, prev_hash, hash
         FROM audit_events WHERE target = $1 ORDER BY id ASC LIMIT 1",
    )
    .bind(&t_duplicate)
    .execute(db.pool())
    .await
    .expect("duplicate insert (test-only)");
    assert!(
        repo.verify_chain()
            .await
            .expect("verify duplicate")
            .is_some(),
        "a duplicated row MUST break the chain (link + recomputed hash)"
    );
    sqlx::query("DELETE FROM audit_events WHERE target = $1")
        .bind(&t_duplicate)
        .execute(db.pool())
        .await
        .expect("cleanup");
    assert_eq!(
        repo.verify_chain().await.expect("verify final"),
        None,
        "chain intact again after removing the test's own rows"
    );
}

#[tokio::test]
async fn audit_chain_survives_concurrent_appends() {
    // Regression test for the chain-fork race: `AuditRepo::append` used to
    // take the head hash with `SELECT … ORDER BY id DESC LIMIT 1 FOR UPDATE`.
    // Under READ COMMITTED a blocked writer's snapshot never sees the row
    // the winner inserted, so it appended from a stale head and forked the
    // chain (verify then reported `broken(at_id)` on a perfectly honest
    // database — a false security incident, reproducible with two replicas
    // or two concurrent API handlers). The append is now serialized by a
    // transaction-scoped advisory lock. This test exercises the production
    // shape: N concurrent appenders over ONE shared pool.
    let Some(db) = setup().await else { return };
    let run = run_id();
    let repo = AuditRepo::new(db.clone());

    let appends = (0..8).map(|i| {
        let repo = &repo;
        let target = run.clone();
        async move {
            repo.append(
                "test",
                "concurrent_append",
                Some(&target),
                "success",
                &serde_json::json!({"i": i}),
            )
            .await
        }
    });
    for r in futures::future::join_all(appends).await {
        r.expect("concurrent append");
    }

    let count = sqlx::query("SELECT COUNT(*) AS n FROM audit_events WHERE target = $1")
        .bind(&run)
        .fetch_one(db.pool())
        .await
        .expect("count");
    use sqlx::Row as _;
    let n: i64 = count.try_get("n").expect("n");
    assert_eq!(n, 8, "all concurrent appends landed");

    assert_eq!(
        repo.verify_chain().await.expect("verify"),
        None,
        "chain must stay LINEAR under concurrent appends (advisory-lock serialization)"
    );

    // Test-only cleanup (as in the other chain tests): remove this run's
    // rows so the global chain returns to its pre-test state.
    sqlx::query("DELETE FROM audit_events WHERE target = $1")
        .bind(&run)
        .execute(db.pool())
        .await
        .expect("cleanup");
    assert_eq!(repo.verify_chain().await.expect("verify2"), None);
}

#[tokio::test]
async fn positions_and_trades_round_trip() {
    let Some(db) = setup().await else { return };
    let run = run_id();
    let pos_repo = PositionRepo::new(db.clone());
    let mut p = Position::new(
        format!("p-{run}"),
        TradeSource::Sniper,
        Venue::PumpFun,
        ExecutionMode::Live,
        format!("mint-{run}"),
        "TEST".into(),
        "SOL".into(),
    );
    p.apply_buy(100.0, 0.5, 50.0);
    p.stop_loss = Some(0.4);
    p.status = PositionStatus::Open;
    pos_repo.upsert(&p).await.expect("upsert");

    let loaded = pos_repo.get(&p.id).await.expect("get").expect("row exists");
    assert_eq!(loaded.qty, 100.0);
    assert_eq!(loaded.avg_entry, 0.5);
    assert_eq!(loaded.cost_basis, 50.0);
    assert_eq!(loaded.stop_loss, Some(0.4));
    assert_eq!(loaded.venue, Venue::PumpFun);
    assert_eq!(loaded.mode, ExecutionMode::Live);

    let open = pos_repo.list_open().await.expect("list_open");
    assert!(open.iter().any(|x| x.id == p.id));

    let trade = Trade {
        id: format!("t-{run}"),
        ts: chrono::Utc::now(),
        source: TradeSource::Sniper,
        venue: Venue::PumpFun,
        mode: ExecutionMode::Live,
        side: PositionSide::Long,
        symbol: format!("mint-{run}"),
        symbol_display: "TEST".into(),
        amount_in: 50.0,
        amount_out: 100.0,
        quote_symbol: "SOL".into(),
        price: 0.5,
        fee: 0.1,
        slippage_bps: 12,
        signature: Some(format!("sig-{run}")),
        position_id: Some(p.id.clone()),
        note: None,
        latency_ms: Some(42),
    };
    TradeRepo::new(db.clone())
        .append(&trade)
        .await
        .expect("trade append");
    // Idempotent retry.
    TradeRepo::new(db.clone())
        .append(&trade)
        .await
        .expect("trade retry append");
    let recent = TradeRepo::new(db.clone())
        .list_recent(500)
        .await
        .expect("list");
    let mine: Vec<_> = recent.iter().filter(|t| t.id == trade.id).collect();
    assert_eq!(mine.len(), 1, "retry did not duplicate the trade");
    assert_eq!(mine[0].slippage_bps, 12);
    assert_eq!(mine[0].latency_ms, Some(42));

    // Close the position; it must leave list_open.
    p.status = PositionStatus::Closed;
    p.closed_at = Some(chrono::Utc::now());
    pos_repo.upsert(&p).await.expect("close upsert");
    let open = pos_repo.list_open().await.expect("list_open2");
    assert!(
        !open.iter().any(|x| x.id == p.id),
        "closed left the open list"
    );
}

#[tokio::test]
async fn reconciliation_queue_claim_backoff_and_giveup() {
    let Some(db) = setup().await else { return };
    let run = run_id();
    let repo = ReconRepo::new(db.clone());
    repo.enqueue("transaction", &run).await.expect("enqueue");
    // Enqueue is idempotent.
    repo.enqueue("transaction", &run).await.expect("enqueue2");

    let claimed = repo.claim_due(50).await.expect("claim");
    let mine = claimed.iter().find(|i| i.subject == run);
    assert!(mine.is_some(), "due item claimed");
    // Second claim while in_progress + not due: nothing for this subject.
    let again = repo.claim_due(50).await.expect("claim2");
    assert!(!again.iter().any(|i| i.subject == run));

    repo.fail("transaction", &run, "boom").await.expect("fail");
    // Backoff pushed it into the future: not immediately claimable.
    let now = repo.claim_due(50).await.expect("claim3");
    assert!(!now.iter().any(|i| i.subject == run), "backoff respected");

    repo.give_up("transaction", &run, "permanent")
        .await
        .expect("give_up");
    let failed = repo.list_failed(100).await.expect("list_failed");
    assert!(failed
        .iter()
        .any(|v| v["subject"] == serde_json::Value::String(run.clone())));
    repo.resolve("transaction", &run).await.expect("resolve");
}

#[tokio::test]
async fn transactions_checkpoints_and_misc_repos_work() {
    let Some(db) = setup().await else { return };
    let run = run_id();

    let tx = TransactionRepo::new(db.clone());
    assert!(tx
        .record_submitted("solana", &format!("sigA-{run}"), None, None, None, 1)
        .await
        .expect("record"));
    assert!(
        !tx.record_submitted("solana", &format!("sigA-{run}"), None, None, None, 1)
            .await
            .expect("record2"),
        "duplicate submit is a no-op"
    );
    tx.set_status(&format!("sigA-{run}"), "finalized", Some(123), None)
        .await
        .expect("status");
    let unresolved = tx
        .list_unresolved_before(chrono::Utc::now(), 500)
        .await
        .expect("list");
    assert!(
        !unresolved.iter().any(|(s, _)| s == &format!("sigA-{run}")),
        "finalized left the sweep list"
    );

    let cp = CheckpointRepo::new(db.clone());
    cp.save("feed", &serde_json::json!({"slot": 42}))
        .await
        .expect("save");
    let loaded = cp.load("feed").await.expect("load");
    assert_eq!(loaded.unwrap()["slot"], 42);

    let risk = RiskEventRepo::new(db.clone());
    risk.append(
        "sniper",
        "rejected",
        Some("X"),
        "too risky",
        &serde_json::json!({}),
    )
    .await
    .expect("risk append");
    let events = risk.list_recent(50).await.expect("risk list");
    assert!(!events.is_empty());

    let sys = SystemEventRepo::new(db.clone());
    sys.append(
        "test",
        None,
        "info",
        &format!("hello {run}"),
        &serde_json::json!({}),
    )
    .await
    .expect("sys append");
    let listed = sys.list_recent(50).await.expect("sys list");
    assert!(listed
        .iter()
        .any(|v| v["message"] == serde_json::Value::String(format!("hello {run}"))));

    let cv = ConfigVersionRepo::new(db.clone());
    let sha = format!("sha-{run}");
    assert!(cv
        .record(&sha, &serde_json::json!({"a": 1}), "test", None)
        .await
        .expect("record"));
    assert!(
        !cv.record(&sha, &serde_json::json!({"a": 1}), "test", None)
            .await
            .expect("record2"),
        "same head hash not re-recorded"
    );

    let w = WalletRepo::new(db.clone());
    w.upsert("hot", "solana", &format!("addr-{run}"), "hot")
        .await
        .expect("wallet");
    let wallets = w.list().await.expect("wallets");
    assert!(wallets
        .iter()
        .any(|v| v["address"] == serde_json::Value::String(format!("addr-{run}"))));
}

#[tokio::test]
async fn order_repo_queries_by_signature_and_status_history_grows() {
    let Some(db) = setup().await else { return };
    let run = run_id();
    let repo = OrderRepo::new(db.clone());
    let now = chrono::Utc::now();
    let order = bot_core::oms::Order {
        id: format!("ord-{run}"),
        idempotency_key: format!("key-{run}"),
        module: BotModule::Copy,
        side: "buy".into(),
        symbol: "S".into(),
        venue: "pump.fun".into(),
        mode: ExecutionMode::Simulate,
        status: OrderStatus::Submitted,
        qty: 2.0,
        price: Some(0.25),
        external_id: None,
        signature: Some(format!("sig-{run}")),
        error: None,
        meta: serde_json::json!({}),
        created_at: now,
        updated_at: now,
        submitted_at: Some(now),
        finished_at: None,
    };
    assert!(repo.insert_if_absent(&order).await.expect("insert"));
    assert!(!repo.insert_if_absent(&order).await.expect("insert2"));

    let by_sig = repo
        .get_by_signature(&format!("sig-{run}"))
        .await
        .expect("by sig")
        .expect("found");
    assert_eq!(by_sig.id, order.id);
    assert_eq!(by_sig.module, BotModule::Copy);
    assert_eq!(by_sig.mode, ExecutionMode::Simulate);

    repo.set_status(&order, OrderStatus::Submitted, Some("filled"))
        .await
        .expect("set_status");
    let history: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM order_status_history WHERE order_id = $1")
            .bind(&order.id)
            .fetch_one(db.pool())
            .await
            .expect("history count");
    assert!(history >= 1, "transition wrote history");
}

// ---------------------------------------------------------------------------
// Prompt 2: reconciliation attribution, queue lifecycle, PnL replay, startup
// gate. Same POSTGRES_URL gating as everything else in this file.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn recon_attribution_and_queue_lifecycle_work() {
    let Some(db) = setup().await else { return };
    let run = run_id();

    // §E: transaction attempts persist signer/venue/attempts attribution.
    let tx = TransactionRepo::new(db.clone());
    let sig = format!("sigAttr-{run}");
    assert!(tx
        .record_submitted(
            "solana",
            &sig,
            None,
            Some("WalletPubkey111"),
            Some("pumpfun"),
            2
        )
        .await
        .expect("record"));
    assert_eq!(
        tx.get_status(&sig).await.expect("status").as_deref(),
        Some("submitted")
    );
    let row: (Option<String>, Option<String>, i32) =
        sqlx::query_as("SELECT signer, venue, attempts FROM transactions WHERE signature = $1")
            .bind(&sig)
            .fetch_one(db.pool())
            .await
            .expect("attribution row");
    assert_eq!(row.0.as_deref(), Some("WalletPubkey111"));
    assert_eq!(row.1.as_deref(), Some("pumpfun"));
    assert_eq!(row.2, 2);
    // Re-recording never overwrites ATTRIBUTION (durable history), and is not
    // reported as the first recording; the broadcast-attempt count is the one
    // cross-replica truth that moves: it MAXes (a second replica that
    // broadcast more times knows better).
    assert!(!tx
        .record_submitted("solana", &sig, None, Some("Other"), None, 9)
        .await
        .expect("record2"));
    let row2: (Option<String>, i32) =
        sqlx::query_as("SELECT signer, attempts FROM transactions WHERE signature = $1")
            .bind(&sig)
            .fetch_one(db.pool())
            .await
            .expect("attribution row 2");
    assert_eq!(row2.0.as_deref(), Some("WalletPubkey111"));
    assert_eq!(row2.1, 9);

    // §J queue mechanics: active detection, counts, resolve/reopen/park.
    let recon = ReconRepo::new(db.clone());
    let subj = format!("pos-{run}");
    recon.enqueue("position", &subj).await.expect("enqueue");
    assert!(recon.is_active("position", &subj).await.expect("active"));
    let counts = recon.unresolved_counts(true).await.expect("counts");
    assert!(counts.iter().any(|(k, n)| k == "position" && *n >= 1));
    recon.resolve("position", &subj).await.expect("resolve");
    assert!(!recon.is_active("position", &subj).await.expect("active2"));
    // Resolved claims can be re-armed for periodic re-verification...
    assert!(recon
        .reopen_resolved("position", &subj)
        .await
        .expect("reopen"));
    assert!(recon.is_active("position", &subj).await.expect("active3"));
    // ...but claims parked for operators are never silently reopened.
    recon
        .give_up("position", &subj, "test park")
        .await
        .expect("give_up");
    assert!(!recon
        .reopen_resolved("position", &subj)
        .await
        .expect("reopen2"));
    let counts_all = recon.unresolved_counts(false).await.expect("counts all");
    assert!(counts_all.iter().any(|(k, _)| k == "position"));
}

#[tokio::test]
async fn pnl_is_replayable_from_persisted_fills() {
    let Some(db) = setup().await else { return };
    let run = run_id();
    use bot_core::reconciliation::{reconstruct_pnl, FillRecord};

    let pos_id = format!("p-recon-{run}");
    let mut pos = Position::new(
        pos_id.clone(),
        TradeSource::Sniper,
        Venue::PumpFun,
        ExecutionMode::Live,
        format!("mint-{run}"),
        "TEST".into(),
        "SOL".into(),
    );
    pos.apply_buy(100.0, 0.06, 6.0);
    PositionRepo::new(db.clone())
        .upsert(&pos)
        .await
        .expect("position upsert");

    let trades = TradeRepo::new(db.clone());
    let buy = Trade {
        id: format!("t-buy-{run}"),
        ts: chrono::Utc::now(),
        source: TradeSource::Sniper,
        venue: Venue::PumpFun,
        mode: ExecutionMode::Live,
        side: PositionSide::Long,
        symbol: format!("mint-{run}"),
        symbol_display: "TEST".into(),
        amount_in: 6.0,
        amount_out: 100.0,
        quote_symbol: "SOL".into(),
        price: 0.06,
        fee: 0.0,
        slippage_bps: 0,
        signature: Some(format!("sigbuy-{run}")),
        position_id: Some(pos_id.clone()),
        note: None,
        latency_ms: None,
    };
    let mut sell = buy.clone();
    sell.id = format!("t-sell-{run}");
    sell.side = PositionSide::Short;
    sell.amount_in = 100.0; // qty sold
    sell.amount_out = 7.2; // quote received
    sell.price = 0.072;
    sell.signature = Some(format!("sigsell-{run}"));
    trades.append(&buy).await.expect("buy append");
    trades.append(&sell).await.expect("sell append");

    let to_fills = |ts: &[Trade]| -> Vec<FillRecord> {
        ts.iter()
            .map(|t| {
                if t.is_buy() {
                    FillRecord {
                        side: "buy".into(),
                        qty: t.amount_out,
                        quote: t.amount_in + t.fee,
                    }
                } else {
                    FillRecord {
                        side: "sell".into(),
                        qty: t.amount_in,
                        quote: t.amount_out - t.fee,
                    }
                }
            })
            .collect()
    };

    // §K/§V23: realized PnL reconstructed from the persisted fill history.
    let fills = trades.list_for_position(&pos_id).await.expect("fills");
    assert_eq!(fills.len(), 2, "chronological fill history");
    let r = reconstruct_pnl(&to_fills(&fills));
    assert!((r.realized - 1.2).abs() < 1e-9, "realized = {}", r.realized);
    assert_eq!(r.open_qty, 0.0);
    assert!((r.avg_entry).abs() < 1e-12);

    // Replaying the SAME persisted history (post-restart determinism) gives
    // byte-identical results.
    let fills2 = trades.list_for_position(&pos_id).await.expect("fills2");
    let r2 = reconstruct_pnl(&to_fills(&fills2));
    assert_eq!(r.realized, r2.realized);
    assert_eq!(r.open_qty, r2.open_qty);
    assert_eq!(r.buy_cost, r2.buy_cost);
    assert_eq!(r.sell_proceeds, r2.sell_proceeds);
}

#[tokio::test]
async fn startup_reconcile_reports_unresolved_claims() {
    let Some(db) = setup().await else { return };
    let run = run_id();
    use bot_core::recovery::{startup_reconcile, RecoveryWorker};

    let recon = ReconRepo::new(db.clone());
    let sig = format!("startup-{run}");
    recon.enqueue("transaction", &sig).await.expect("enqueue");

    // No truth source registered: the claim cannot resolve, must survive the
    // pass as unresolved (retried), and must appear in the report the startup
    // gate uses to block modules (§H) — no panics, no silent success.
    let worker = RecoveryWorker::new(db.clone(), bot_core::lifecycle::Shutdown::new());
    let report = startup_reconcile(&worker, 16, Duration::from_secs(5)).await;
    assert!(report.passes >= 1);
    assert!(report.retried >= 1);
    assert!(report
        .unresolved
        .iter()
        .any(|(k, n)| k == "transaction" && *n >= 1));
    assert!(report.total_unresolved() >= 1);
}

// ---------------------------------------------------------------------------
// Intent journal (write-ahead, crash point C) + cross-replica attempt truth
// ---------------------------------------------------------------------------

fn intent_rec(run: &str, id: &str, sym: &str) -> IntentRecord {
    IntentRecord {
        intent_id: format!("{id}-{run}"),
        module: "sniper".into(),
        symbol: sym.into(),
        wallet: "wallet-1".into(),
        side: "buy".into(),
        qty: "1000".into(),
        status: "pending".into(),
        signature: None,
        created_at: chrono::Utc::now(),
    }
}

#[tokio::test]
async fn intent_journal_lifecycle_and_orphan_sweep() {
    let Some(db) = setup().await else { return };
    let run = run_id();
    let repo = IntentRepo::new(db.clone());

    // pending -> submitted (linked to the signature that left the process).
    repo.record(&intent_rec(&run, "i1", "SYM1"))
        .await
        .expect("record");
    repo.link(&format!("i1-{run}"), "SIG1").await.expect("link");
    // Re-recording the same intent is a NO-OP: the journal is write-ahead
    // evidence and is never rewritten (not even by a buggy/racing producer).
    repo.record(&intent_rec(&run, "i1", "HACKED"))
        .await
        .expect("rerecord");
    let got = repo
        .get(&format!("i1-{run}"))
        .await
        .expect("get")
        .expect("row");
    assert_eq!(got.status, "submitted");
    assert_eq!(
        got.symbol, "SYM1",
        "re-record must NOT overwrite the journal"
    );
    assert_eq!(got.signature.as_deref(), Some("SIG1"));

    // pending -> abandoned (provably never broadcast); terminal rows are
    // immutable: a late link on an abandoned intent is a no-op.
    repo.record(&intent_rec(&run, "i2", "SYM2"))
        .await
        .expect("record2");
    repo.abandon(&format!("i2-{run}")).await.expect("abandon");
    repo.link(&format!("i2-{run}"), "SIG2")
        .await
        .expect("link-noop");
    let got2 = repo.get(&format!("i2-{run}")).await.unwrap().unwrap();
    assert_eq!(got2.status, "abandoned");
    assert!(got2.signature.is_none());

    // Orphans: only PENDING rows older than the cutoff. i3 stays pending.
    repo.record(&intent_rec(&run, "i3", "SYM3"))
        .await
        .expect("record3");
    let future = chrono::Utc::now() + chrono::Duration::seconds(60);
    let ancient = chrono::Utc::now() - chrono::Duration::hours(70);
    let orphans = repo.list_orphaned(future, 500).await.expect("orphans");
    assert!(orphans.iter().any(|o| o.intent_id == format!("i3-{run}")));
    assert!(
        !orphans.iter().any(|o| o.intent_id == format!("i1-{run}")),
        "submitted intents are not orphans"
    );
    assert!(
        !orphans.iter().any(|o| o.intent_id == format!("i2-{run}")),
        "abandoned intents are not orphans"
    );
    assert!(repo.list_orphaned(ancient, 500).await.unwrap().is_empty());

    // The sweep enqueues `intent` claims for orphans (never resubmits them).
    let swept = bot_core::recovery::sweep_orphan_intents(&db, Duration::from_secs(0)).await;
    assert!(swept.iter().any(|o| o.intent_id == format!("i3-{run}")));
    assert!(
        ReconRepo::new(db.clone())
            .is_active("intent", &format!("i3-{run}"))
            .await
            .expect("is_active"),
        "orphan must be queued as an intent claim"
    );
}

#[tokio::test]
async fn record_submitted_maxes_attempts_across_replicas() {
    let Some(db) = setup().await else { return };
    let run = run_id();
    let tx = TransactionRepo::new(db.clone());
    let sig = format!("sigX-{run}");

    // A real persisted order: transactions.order_id is a foreign key.
    let mgr = OrderManager::new(Some(db.clone()), 64);
    let order = mgr
        .create(OrderDraft {
            idempotency_key: format!("maxattempts-{run}"),
            module: BotModule::Sniper,
            side: "buy".into(),
            symbol: "TEST".into(),
            venue: "pump.fun".into(),
            mode: ExecutionMode::Paper,
            qty: 1.0,
            price: None,
            meta: serde_json::json!({}),
        })
        .await
        .expect("create order");

    assert!(tx
        .record_submitted("solana", &sig, None, None, None, 1)
        .await
        .expect("r1"));
    // Second replica: higher attempt count wins, missing attribution is
    // filled in, and it is NOT reported as the first recording.
    assert!(!tx
        .record_submitted(
            "solana",
            &sig,
            Some(&order.id),
            Some("SignerA"),
            Some("pump"),
            3
        )
        .await
        .expect("r2"));
    assert_eq!(tx.get_attempts(&sig).await.unwrap(), Some(3));
    assert_eq!(
        tx.get_order_id(&sig).await.unwrap().as_deref(),
        Some(order.id.as_str())
    );
    // A lower count from a lagging replica never decreases the truth.
    assert!(!tx
        .record_submitted("solana", &sig, None, None, None, 2)
        .await
        .expect("r3"));
    assert_eq!(tx.get_attempts(&sig).await.unwrap(), Some(3));
    // Terminal rows are immutable (§Y): a late replica cannot resurrect or
    // rewrite a confirmed transaction.
    tx.set_status(&sig, "confirmed", Some(42), None)
        .await
        .expect("confirm");
    assert!(!tx
        .record_submitted("solana", &sig, None, None, None, 9)
        .await
        .expect("r4"));
    assert_eq!(
        tx.get_attempts(&sig).await.unwrap(),
        Some(3),
        "terminal rows never change"
    );
    assert_eq!(
        tx.get_status(&sig).await.unwrap().as_deref(),
        Some("confirmed")
    );
}

// ---------------------------------------------------------------------------
// Prompt 3 (§D/§T/§W): distributed execution ownership on Postgres —
// `execution_claims` (migration 0009) and `runtime_flags` (migration 0010).
// These tests exercise the AUTHORITATIVE store through the same atomic
// upsert/CAS statements production uses.
// ---------------------------------------------------------------------------

use bot_core::db::claims::{PostgresClaimStore, PostgresFlags};
use bot_core::ownership::{
    ClaimOutcome, ClaimStatus, OwnershipRegistry, RuntimeFlagsReader, RuntimeFlagsWriter,
};

async fn pg_registry(
    db: &Arc<Database>,
    replica: &str,
    lease: Duration,
    grace: Duration,
) -> OwnershipRegistry {
    OwnershipRegistry::new(
        Arc::new(PostgresClaimStore::new(db.clone())),
        replica,
        lease,
        grace,
    )
}

/// Two replicas, one logical execution: exactly one owner; the loser gets a
/// deterministic rejection carrying the current holder (§G).
#[tokio::test]
async fn pg_claim_single_owner_and_loser_sees_holder() {
    let Some(db) = setup().await else { return };
    let id = format!("snipe:{}", run_id());
    let a = pg_registry(
        &db,
        "rep-A",
        Duration::from_secs(30),
        Duration::from_secs(900),
    )
    .await;
    let b = pg_registry(
        &db,
        "rep-B",
        Duration::from_secs(30),
        Duration::from_secs(900),
    )
    .await;

    match a.claim(&id, "entry", "sniper", "pump", "S").await.unwrap() {
        ClaimOutcome::Owned(g) => {
            assert_eq!(g.epoch(), 1);
            assert_eq!(g.owner_id(), "rep-A");
            g.fence().await.expect("owner must pass the fence");
        }
        _ => panic!("A must acquire"),
    }
    match b.claim(&id, "entry", "sniper", "pump", "S").await.unwrap() {
        ClaimOutcome::OwnedByOther {
            owner_id,
            status,
            epoch,
            ..
        } => {
            assert_eq!(owner_id, "rep-A");
            assert_eq!(status, ClaimStatus::Claimed);
            assert_eq!(epoch, 1);
        }
        _ => panic!("B must be rejected while A's lease runs"),
    }
}

/// Eight concurrent claimants on the SAME id: the atomic upsert elects
/// exactly one winner (§D/§W).
#[tokio::test]
async fn pg_claim_concurrent_race_exactly_one_winner() {
    let Some(db) = setup().await else { return };
    let id = format!("copy:w:{}", run_id());
    let mut racers = Vec::new();
    for i in 0..8 {
        let db = db.clone();
        let id = id.clone();
        racers.push(tokio::spawn(async move {
            let reg = pg_registry(
                &db,
                &format!("rep-{i}"),
                Duration::from_secs(30),
                Duration::from_secs(900),
            )
            .await;
            reg.claim(&id, "entry", "copy", "mirror", "S").await
        }));
    }
    let mut owned = 0;
    let mut rejected = 0;
    for r in racers {
        match r.await.unwrap().unwrap() {
            ClaimOutcome::Owned(_) => owned += 1,
            ClaimOutcome::OwnedByOther { .. } => rejected += 1,
        }
    }
    assert_eq!(owned, 1, "exactly one winner");
    assert_eq!(rejected, 7);
}

/// Lease expiry → takeover with epoch+1 and full audit lineage; every stale
/// generation is fenced on verify/renew/release (§E/§J/§T).
#[tokio::test]
async fn pg_claim_expiry_takeover_and_fencing() {
    let Some(db) = setup().await else { return };
    let id = format!("exit:p:{}", run_id());
    let a = pg_registry(
        &db,
        "rep-A",
        Duration::from_secs(1),
        Duration::from_secs(900),
    )
    .await;
    let b = pg_registry(
        &db,
        "rep-B",
        Duration::from_secs(30),
        Duration::from_secs(900),
    )
    .await;

    let mut ga = match a.claim(&id, "exit", "sniper", "sl", "S").await.unwrap() {
        ClaimOutcome::Owned(g) => g,
        _ => panic!("A must acquire"),
    };
    // A's lease (1 s) lapses; B is a fresh replica with a normal lease.
    tokio::time::sleep(Duration::from_millis(1400)).await;

    assert!(ga.fence().await.is_err(), "expired owner must be fenced");
    assert!(!ga.renew().await.unwrap(), "expired leases never resurrect");

    let gb = match b.claim(&id, "exit", "sniper", "sl", "S").await.unwrap() {
        ClaimOutcome::Owned(g) => g,
        _ => panic!("B must take over the expired lease"),
    };
    assert_eq!(gb.epoch(), 2);
    assert_eq!(gb.claim().takeover_count, 1);
    assert_eq!(gb.claim().previous_owner.as_deref(), Some("rep-A"));

    // A wakes up: every continuation is rejected; it cannot release B's claim.
    assert!(ga.fence().await.is_err());
    assert!(!ga.release().await);
    assert!(!ga.hand_off().await);
    assert!(gb.fence().await.is_ok());

    // The audit row tells the whole story (§S/§T).
    let rec = gb.claim();
    assert_eq!(rec.status, ClaimStatus::Claimed);
    let fetched = a.get(&id).await.unwrap().unwrap();
    assert_eq!(fetched.owner_id, "rep-B");
    assert_eq!(fetched.epoch, 2);
    assert_eq!(fetched.takeover_count, 1);
}

/// Clean release is immediately re-acquirable; an ambiguous handoff blocks
/// EVERYONE for the grace window (§I case E / §M).
#[tokio::test]
async fn pg_claim_release_and_handoff_grace() {
    let Some(db) = setup().await else { return };
    let a = pg_registry(
        &db,
        "rep-A",
        Duration::from_secs(30),
        Duration::from_secs(1),
    )
    .await;
    let b = pg_registry(
        &db,
        "rep-B",
        Duration::from_secs(30),
        Duration::from_secs(1),
    )
    .await;

    // Released → re-acquirable at once (re-fired exit rule = new decision).
    let id = format!("exit:r:{}", run_id());
    let mut g = match a.claim(&id, "exit", "sniper", "sl", "S").await.unwrap() {
        ClaimOutcome::Owned(g) => g,
        _ => panic!(),
    };
    assert!(g.release().await);
    assert!(matches!(
        b.claim(&id, "exit", "sniper", "sl", "S").await.unwrap(),
        ClaimOutcome::Owned(_)
    ));

    // Handed off → blocked during grace, acquirable after.
    let id2 = format!("exit:h:{}", run_id());
    let mut g2 = match a.claim(&id2, "exit", "sniper", "sl", "S").await.unwrap() {
        ClaimOutcome::Owned(g) => g,
        _ => panic!(),
    };
    assert!(g2.hand_off().await);
    match b.claim(&id2, "exit", "sniper", "sl", "S").await.unwrap() {
        ClaimOutcome::OwnedByOther { status, .. } => assert_eq!(status, ClaimStatus::HandedOff),
        _ => panic!("handoff grace must block re-acquisition"),
    }
    tokio::time::sleep(Duration::from_millis(1400)).await;
    assert!(matches!(
        b.claim(&id2, "exit", "sniper", "sl", "S").await.unwrap(),
        ClaimOutcome::Owned(_)
    ));
}

/// Renewal extends the lease past its original expiry while the owner keeps
/// fencing successfully (§N).
#[tokio::test]
async fn pg_claim_renew_extends_lease() {
    let Some(db) = setup().await else { return };
    let id = format!("snipe:renew:{}", run_id());
    let a = pg_registry(
        &db,
        "rep-A",
        Duration::from_secs(2),
        Duration::from_secs(900),
    )
    .await;
    let b = pg_registry(
        &db,
        "rep-B",
        Duration::from_secs(2),
        Duration::from_secs(900),
    )
    .await;

    let mut g = match a.claim(&id, "entry", "sniper", "pump", "S").await.unwrap() {
        ClaimOutcome::Owned(g) => g,
        _ => panic!(),
    };
    tokio::time::sleep(Duration::from_millis(1200)).await;
    assert!(
        g.renew().await.unwrap(),
        "renewal inside the lease must work"
    );
    // 2.4 s after the claim — past the ORIGINAL 2 s lease, inside the renewal.
    tokio::time::sleep(Duration::from_millis(1200)).await;
    g.fence().await.expect("renewed lease must still fence");
    assert!(
        matches!(
            b.claim(&id, "entry", "sniper", "pump", "S").await.unwrap(),
            ClaimOutcome::OwnedByOther { .. }
        ),
        "no takeover while the lease is renewed"
    );
    assert!(g.release().await);
}

/// `runtime_flags` (migration 0010) round-trips kill-switch/module values
/// with writer identity and timestamps (§Q/§T).
#[tokio::test]
async fn pg_runtime_flags_roundtrip() {
    let Some(db) = setup().await else { return };
    let flags = PostgresFlags::new(db.clone());
    let tag = run_id();

    RuntimeFlagsWriter::write(&flags, "kill_switch", true, &format!("test-{tag}"), "rep-A").await;
    RuntimeFlagsWriter::write(
        &flags,
        "module:sniper",
        false,
        &format!("test-{tag}"),
        "rep-A",
    )
    .await;

    let rows = RuntimeFlagsReader::read_all(&flags).await.unwrap();
    let kill = rows
        .iter()
        .find(|r| r.flag == "kill_switch")
        .expect("kill row");
    assert!(kill.enabled);
    assert_eq!(kill.updated_by, "rep-A");
    let sniper = rows
        .iter()
        .find(|r| r.flag == "module:sniper")
        .expect("module row");
    assert!(!sniper.enabled);

    // Upsert: the latest write wins, timestamp advances.
    let before = kill.updated_at;
    RuntimeFlagsWriter::write(&flags, "kill_switch", false, "released", "rep-B").await;
    let rows = RuntimeFlagsReader::read_all(&flags).await.unwrap();
    let kill = rows.iter().find(|r| r.flag == "kill_switch").unwrap();
    assert!(!kill.enabled);
    assert_eq!(kill.updated_by, "rep-B");
    assert!(kill.updated_at >= before);
}

/// §S/§M gap closure: the append-only events table (migration 0011) records
/// the FULL lineage — every generation of ownership, fencing rejections
/// included — not just the one-deep `previous_owner` on the claim row.
#[tokio::test]
async fn pg_claim_event_history_records_full_lineage() {
    let Some(db) = setup().await else { return };
    let id = format!("exit:hist:{}", run_id());
    let a = pg_registry(
        &db,
        "rep-A",
        Duration::from_secs(1),
        Duration::from_secs(900),
    )
    .await;
    let b = pg_registry(
        &db,
        "rep-B",
        Duration::from_secs(30),
        Duration::from_secs(900),
    )
    .await;

    // Generation 1: A acquires, then stalls past its 1 s lease.
    let ga = match a.claim(&id, "exit", "sniper", "sl", "S").await.unwrap() {
        ClaimOutcome::Owned(g) => g,
        _ => panic!("A must acquire"),
    };
    tokio::time::sleep(Duration::from_millis(1400)).await;

    // Generation 2: B takes over; the stale owner A gets fenced.
    let mut gb = match b.claim(&id, "exit", "sniper", "sl", "S").await.unwrap() {
        ClaimOutcome::Owned(g) => g,
        _ => panic!("B must take over"),
    };
    assert!(ga.fence().await.is_err(), "A must be fenced");
    assert!(gb.release().await);

    let events = PostgresClaimStore::new(db.clone())
        .events(&id)
        .await
        .expect("events readable");
    let kinds: Vec<&str> = events.iter().map(|e| e.event.as_str()).collect();
    assert_eq!(
        kinds,
        vec!["acquired", "takeover", "fenced", "released"],
        "full lineage, oldest first"
    );
    assert_eq!(events[0].owner_id, "rep-A");
    assert_eq!(events[1].owner_id, "rep-B");
    assert_eq!(events[1].previous_owner.as_deref(), Some("rep-A"));
    assert_eq!(events[1].epoch, 2);
    assert_eq!(
        events[2].owner_id, "rep-A",
        "the fenced generation is recorded"
    );
    assert!(
        events[2].detail.contains("rep-B"),
        "detail names the current holder"
    );
    assert_eq!(events[3].event, "released");
}

/// §Q gap closure: the cluster-wide risk oracle counts open positions and
/// sums today's realized PnL from the shared positions table.
#[tokio::test]
async fn pg_risk_oracle_counts_and_pnl() {
    use bot_core::db::claims::PostgresRiskOracle;
    use bot_core::risk::GlobalRiskOracle;

    let Some(db) = setup().await else { return };
    let oracle = PostgresRiskOracle::new(db.clone());
    let repo = PositionRepo::new(db.clone());

    // Non-trading modules have no position source → unknown (None).
    assert!(oracle.count_open(BotModule::Telegram).await.is_none());

    let baseline = oracle.count_open(BotModule::Sniper).await.expect("count");
    let pnl_before = oracle.realized_today().await.expect("pnl");

    let mut p = Position::new(
        format!("p-{}", run_id()),
        TradeSource::Sniper,
        Venue::PumpFun,
        ExecutionMode::Paper,
        "MINT".into(),
        "MINT".into(),
        "SOL".into(),
    );
    p.qty = 1.0;
    p.avg_entry = 1.0;
    repo.upsert(&p).await.expect("upsert open");
    assert_eq!(
        oracle.count_open(BotModule::Sniper).await.unwrap(),
        baseline + 1,
        "a position written by ANY replica counts against global capacity"
    );

    // Close it at a loss: capacity drops back, today's realized PnL includes
    // the -0.5 (single-threaded run ⇒ deterministic delta).
    p.status = PositionStatus::Closed;
    p.qty = 0.0;
    p.closed_at = Some(chrono::Utc::now());
    p.cost_basis = 1.0;
    p.realized_quote = 0.5;
    repo.upsert(&p).await.expect("upsert closed");
    assert_eq!(
        oracle.count_open(BotModule::Sniper).await.unwrap(),
        baseline
    );
    let pnl_after = oracle.realized_today().await.unwrap();
    assert!(
        (pnl_after - (pnl_before - 0.5)).abs() < 1e-9,
        "pnl {pnl_before} -> {pnl_after} must include the -0.5"
    );
}

// ---------------------------------------------------------------------------
// Copy-trading journal (migration 0013, TASK 3)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn pg_copy_journal_roundtrip() {
    let Some(db) = setup().await else { return };
    use bot_core::db::copy::{
        CopyEventRecord, CopyLinkRecord, CopyRepo, LeaderEventRecord, LeaderRecord,
    };
    let repo = CopyRepo::new(db.clone());
    let run = run_id();
    let leader = format!("leader-{run}");
    let now = chrono::Utc::now();

    // Leaders: upsert keeps the earliest followed_at, grows counters, follows
    // status; lifecycle events append.
    let rec = LeaderRecord {
        address: leader.clone(),
        label: "whale".into(),
        status: "active".into(),
        source: "config".into(),
        followed_at: now,
        status_since: now,
        events_seen: 3,
        mirrored: 1,
        rejected: 1,
        last_event_at: Some(now),
        last_slot: Some(100),
        updated_at: now,
    };
    repo.upsert_leader(&rec).await.expect("leader upsert");
    let mut again = rec.clone();
    again.status = "paused".into();
    again.events_seen = 1;
    again.last_slot = Some(90);
    again.followed_at = now - chrono::Duration::days(2);
    repo.upsert_leader(&again).await.expect("leader upsert 2");
    let got = repo.get_leader(&leader).await.expect("get").expect("row");
    assert_eq!(got.status, "paused");
    assert_eq!(got.events_seen, 3, "counters never shrink");
    assert_eq!(got.last_slot, Some(100));
    assert_eq!(got.label, "whale");
    assert!(repo
        .list_leaders()
        .await
        .unwrap()
        .iter()
        .any(|l| l.address == leader));
    repo.append_leader_event(&LeaderEventRecord {
        id: 0,
        address: leader.clone(),
        event: "paused".into(),
        reason: Some("operator".into()),
        replica_id: "r1".into(),
        ts: now,
    })
    .await
    .expect("leader event");
    let events = repo.leader_events(&leader, 10).await.unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].event, "paused");
    assert!(events[0].id > 0);

    // Events: upsert on event_id keeps created_at, overwrites the outcome.
    let event_id = format!("cev_{run}");
    let ev = CopyEventRecord {
        event_id: event_id.clone(),
        leader: leader.clone(),
        signature: format!("sig-{run}"),
        slot: 123,
        mint: format!("mint-{run}"),
        side: "buy".into(),
        venue: "pump.fun".into(),
        token_amount: 1_000.0,
        sol_amount: 0.5,
        source: "pumpportal".into(),
        source_sequence: 7,
        event_at: Some(now),
        observed_at: now,
        stage: "REJECTED".into(),
        reject_reason: Some("STALE_EVENT".into()),
        detail: Some("old".into()),
        intent_id: None,
        position_id: None,
        created_at: now,
        updated_at: now,
    };
    repo.record_event(&ev).await.expect("event");
    let mut filled = ev.clone();
    filled.stage = "FILLED".into();
    filled.reject_reason = None;
    filled.detail = None;
    filled.intent_id = Some("int_x".into());
    filled.position_id = Some(format!("p-{run}"));
    filled.created_at = now + chrono::Duration::hours(1);
    repo.record_event(&filled).await.expect("event 2");
    let got = repo.get_event(&event_id).await.unwrap().unwrap();
    assert_eq!(got.stage, "FILLED");
    assert_eq!(got.intent_id.as_deref(), Some("int_x"));
    assert!(got.reject_reason.is_none());
    assert_eq!(got.slot, 123);
    assert_eq!(got.source_sequence, 7);
    assert!(
        (got.created_at - now).num_seconds().abs() < 2,
        "created_at is first-seen"
    );
    let since = repo
        .events_since(now - chrono::Duration::minutes(1), 1000)
        .await
        .unwrap();
    assert!(since.iter().any(|e| e.event_id == event_id));
    assert!(repo
        .events_for_leader(&leader, 10)
        .await
        .unwrap()
        .iter()
        .any(|e| e.event_id == event_id));

    // Links: open → closed exactly once; state check constraint holds.
    let position_id = format!("p-{run}");
    let link = CopyLinkRecord {
        position_id: position_id.clone(),
        leader: leader.clone(),
        mint: format!("mint-{run}"),
        entry_event_id: event_id.clone(),
        entry_signature: format!("sig-{run}"),
        intent_id: Some("int_x".into()),
        leader_token_amount: 1_000.0,
        follower_qty: 50.0,
        status: "open".into(),
        opened_at: now,
        closed_at: None,
        exit_event_id: None,
        last_reconciled_at: None,
        note: None,
        updated_at: now,
    };
    repo.upsert_link(&link).await.expect("link");
    assert!(repo
        .open_links()
        .await
        .unwrap()
        .iter()
        .any(|l| l.position_id == position_id));
    let mut refreshed = link.clone();
    refreshed.follower_qty = 25.0;
    refreshed.last_reconciled_at = Some(now);
    repo.upsert_link(&refreshed).await.expect("link refresh");
    assert_eq!(
        repo.get_link(&position_id)
            .await
            .unwrap()
            .unwrap()
            .follower_qty,
        25.0
    );
    assert!(repo
        .close_link(
            &position_id,
            "closed",
            Some("cev_exit"),
            Some("leader exited")
        )
        .await
        .unwrap());
    assert!(
        !repo
            .close_link(&position_id, "orphaned", None, None)
            .await
            .unwrap(),
        "only open links transition"
    );
    let closed = repo.get_link(&position_id).await.unwrap().unwrap();
    assert_eq!(closed.status, "closed");
    assert_eq!(closed.exit_event_id.as_deref(), Some("cev_exit"));
    assert!(closed.closed_at.is_some());
    let mut bad = link.clone();
    bad.position_id = format!("p-bad-{run}");
    bad.status = "bogus".into();
    assert!(
        repo.upsert_link(&bad).await.is_err(),
        "CHECK (status) rejects unknown states"
    );
}

// ---------------------------------------------------------------------------
// Polymarket trading journal (migration 0014, TASK 4)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn pg_polymarket_journal_roundtrip() {
    let Some(db) = setup().await else { return };
    use bot_core::db::polymarket::{
        PolyFillRecord, PolyOrderRecord, PolyReconFindingRecord, PolyRepo, PolySignalRecord,
    };
    let repo = PolyRepo::new(db.clone());
    let run = run_id();
    let now = chrono::Utc::now();
    let signal_id = format!("psig_{run}");
    let venue_order_id = format!("0x{run:0>64}");
    let order_id = format!("ord_{run}");

    // Signals: upsert keeps created_at, follows stage/reason, COALESCEs ids.
    let sig = PolySignalRecord {
        signal_id: signal_id.clone(),
        condition_id: "0xcond".into(),
        token_id: format!("tok{run}"),
        outcome: "Yes".into(),
        side: "buy".into(),
        strategy: "value".into(),
        limit_price: 0.42,
        size_tokens: 10.0,
        stake_usd: 4.2,
        mode: "paper".into(),
        stage: "RISK_APPROVED".into(),
        reject_reason: None,
        detail: "first".into(),
        order_id: None,
        venue_order_id: None,
        position_id: None,
        created_at: now - chrono::Duration::minutes(1),
        updated_at: now - chrono::Duration::minutes(1),
    };
    repo.record_signal(&sig).await.expect("signal insert");
    let mut later = sig.clone();
    later.stage = "FILLED".into();
    later.detail = "second".into();
    later.order_id = Some(order_id.clone());
    later.venue_order_id = Some(venue_order_id.clone());
    later.created_at = now;
    later.updated_at = now;
    repo.record_signal(&later).await.expect("signal upsert");
    let got = repo.get_signal(&signal_id).await.unwrap().unwrap();
    assert_eq!(got.stage, "FILLED");
    assert_eq!(got.detail, "second");
    assert_eq!(got.order_id.as_deref(), Some(order_id.as_str()));
    assert!(got.created_at < now, "created_at keeps the first-seen time");
    let mut rejected = sig.clone();
    rejected.signal_id = format!("psig_rej_{run}");
    rejected.stage = "REJECTED".into();
    rejected.reject_reason = Some("SPREAD_TOO_WIDE".into());
    repo.record_signal(&rejected).await.unwrap();
    let recent = repo
        .signals_since(now - chrono::Duration::hours(1), 1000)
        .await
        .unwrap();
    assert!(recent.iter().any(|s| s.signal_id == signal_id));
    assert!(recent
        .iter()
        .any(|s| s.reject_reason.as_deref() == Some("SPREAD_TOO_WIDE")));
    let mut bad = sig.clone();
    bad.signal_id = format!("psig_bad_{run}");
    bad.side = "hold".into();
    assert!(
        repo.record_signal(&bad).await.is_err(),
        "CHECK (side) rejects unknown sides"
    );

    // Orders: matched size never regresses, closed_at sticks, open list.
    let order = PolyOrderRecord {
        venue_order_id: venue_order_id.clone(),
        order_id: order_id.clone(),
        signal_id: signal_id.clone(),
        condition_id: "0xcond".into(),
        token_id: format!("tok{run}"),
        outcome: "Yes".into(),
        side: "buy".into(),
        order_type: "GTC".into(),
        limit_price: 0.42,
        size_tokens: 10.0,
        size_matched: 4.0,
        mode: "live".into(),
        state: "partially_filled".into(),
        venue_status: "live".into(),
        expiration: 0,
        position_id: None,
        replica_id: "r1".into(),
        submitted_at: now - chrono::Duration::minutes(2),
        updated_at: now,
        closed_at: None,
    };
    repo.upsert_order(&order).await.expect("order insert");
    let open = repo.open_orders().await.unwrap();
    assert!(open.iter().any(|o| o.venue_order_id == venue_order_id));
    let mut stale = order.clone();
    stale.size_matched = 1.0;
    stale.state = "resting".into();
    repo.upsert_order(&stale).await.unwrap();
    let got = repo.get_order(&venue_order_id).await.unwrap().unwrap();
    assert_eq!(
        got.size_matched, 4.0,
        "a stale poll cannot un-fill an order"
    );
    let mut done = order.clone();
    done.size_matched = 10.0;
    done.state = "filled".into();
    done.venue_status = "matched".into();
    done.position_id = Some(format!("p{run}"));
    done.closed_at = Some(now);
    repo.upsert_order(&done).await.unwrap();
    let got = repo.get_order(&venue_order_id).await.unwrap().unwrap();
    assert_eq!(got.state, "filled");
    assert!(got.closed_at.is_some());
    assert_eq!(got.position_id.as_deref(), Some(format!("p{run}").as_str()));
    assert!(!repo
        .open_orders()
        .await
        .unwrap()
        .iter()
        .any(|o| o.venue_order_id == venue_order_id));

    // Fills: guarded insert dedups on fill_id.
    let fill = PolyFillRecord {
        fill_id: format!("trade-{run}"),
        venue_order_id: venue_order_id.clone(),
        order_id: order_id.clone(),
        token_id: format!("tok{run}"),
        side: "buy".into(),
        price: 0.42,
        size_tokens: 4.0,
        quote_usd: 1.68,
        source: "user_ws".into(),
        position_id: None,
        ts: now,
    };
    assert!(repo.record_fill(&fill).await.unwrap(), "first booking");
    assert!(
        !repo.record_fill(&fill).await.unwrap(),
        "replayed fill is not booked twice"
    );
    let fills = repo.fills_for(&venue_order_id).await.unwrap();
    assert_eq!(fills.len(), 1);
    assert_eq!(fills[0].source, "user_ws");

    // Recon findings: append-only, newest first.
    repo.append_finding(&PolyReconFindingRecord {
        id: 0,
        kind: "orphan_venue_order".into(),
        venue_order_id: Some(format!("0xorphan{run}")),
        order_id: None,
        token_id: Some(format!("tok{run}")),
        detail: "open on venue, unknown locally".into(),
        action: "reported".into(),
        replica_id: "r1".into(),
        ts: now,
    })
    .await
    .unwrap();
    let findings = repo.recent_findings(50).await.unwrap();
    assert!(findings
        .iter()
        .any(|f| f.kind == "orphan_venue_order" && f.id > 0));
}

/// TASK 5 (migration 0015): the ledger event + postings insert is one
/// transaction and idempotent on the event id; snapshots, decisions, kill
/// switches and findings round-trip; the durable-store adapters recover a
/// fresh ledger from the journal without booking anything twice.
#[tokio::test]
async fn global_ledger_repo_round_trips_and_is_idempotent() {
    use bot_core::accounting::{
        expand, fill_event, AccountingFindingKind, Applied, EventSide, GlobalLedger, LedgerStore,
        PositionBook, StoredEvent,
    };
    use bot_core::db::accounting::AccountingRepo;
    use bot_core::events::EventBus;
    use bot_core::global_risk::{
        DecisionContext, GlobalRiskRequest, KillScope, KillSwitchEvent, KillSwitchState,
    };

    let Some(db) = setup().await else { return };
    let run = run_id();
    let repo = AccountingRepo::new(db.clone());
    let wallet = format!("wallet-{run}");

    // Event + postings in one transaction; the replay inserts nothing.
    let event = fill_event(
        BotModule::Sniper,
        Venue::PumpFun,
        wallet.clone(),
        "sniper",
        format!("MINT-{run}"),
        "SOL",
        EventSide::Buy,
        100.0,
        0.01,
        1.0,
        0.0,
        ExecutionMode::Paper,
        format!("sig-{run}"),
        Some(format!("intent-{run}")),
        Some(format!("p-{run}")),
        chrono::Utc::now(),
        "db test",
    );
    let stored = StoredEvent {
        event_id: event.event_id(),
        event: event.clone(),
        recorded_at: chrono::Utc::now(),
        replica_id: "r1".into(),
    };
    let entry = expand(&event, 0.0).unwrap();
    assert!(
        repo.record_event(&stored, &entry).await.unwrap(),
        "first booking"
    );
    assert!(
        !repo.record_event(&stored, &entry).await.unwrap(),
        "replayed event is not journaled twice"
    );
    let postings = repo.postings(&stored.event_id).await.unwrap();
    assert_eq!(postings.len(), entry.postings.len());
    assert!(repo
        .load_events()
        .await
        .unwrap()
        .iter()
        .any(|e| e.event_id == stored.event_id));

    // Position snapshot upsert.
    let mut book = PositionBook::new();
    book.apply(&event, &stored.event_id);
    let snapshot = book.positions().next().unwrap().clone();
    repo.upsert_position(&snapshot).await.unwrap();
    repo.upsert_position(&snapshot).await.unwrap();
    assert!(repo
        .positions()
        .await
        .unwrap()
        .iter()
        .any(|p| p.key == snapshot.key && (p.qty - 100.0).abs() < 1e-9));

    // A fresh ledger over the durable store rebuilds from the journal and
    // treats the same fact as a duplicate.
    let store: Arc<dyn LedgerStore> = Arc::new(TestLedgerStore(repo_arc(db.clone())));
    let ledger = GlobalLedger::new(EventBus::new(16), "r2");
    ledger.attach_store(store).await;
    let report = ledger.recover(&[]).await;
    assert!(report.journal_available);
    assert!(report.rebuilt >= 1);
    assert_eq!(ledger.submit(event.clone()).await, Applied::Duplicate);

    // Decisions, kill switches and findings.
    let engine = bot_core::global_risk::GlobalRiskEngine::new(
        bot_core::config::GlobalRiskConfig::default(),
        Arc::new(GlobalLedger::new(EventBus::new(16), "r3")),
        EventBus::new(16),
        "r3",
    );
    let decision = engine
        .decide(
            &GlobalRiskRequest {
                module: BotModule::Sniper,
                venue: Venue::PumpFun,
                wallet: wallet.clone(),
                strategy: "sniper".into(),
                asset: format!("MINT-{run}"),
                quote_asset: "SOL".into(),
                requested_quote: 0.1,
                mode: ExecutionMode::Paper,
            },
            &DecisionContext::default(),
        )
        .await;
    repo.record_decision(&decision).await.unwrap();
    repo.record_decision(&decision).await.unwrap();
    assert!(repo
        .recent_decisions(50)
        .await
        .unwrap()
        .iter()
        .any(|d| d.decision_id == decision.decision_id));
    let scope = KillScope::Strategy(format!("copy:{run}"));
    let now = chrono::Utc::now();
    repo.upsert_kill_switch(&KillSwitchState {
        scope: scope.clone(),
        configured: false,
        engaged: true,
        reason: "incident".into(),
        actor: "op".into(),
        updated_at: now,
    })
    .await
    .unwrap();
    repo.append_kill_switch_event(&KillSwitchEvent {
        scope: scope.clone(),
        action: "engage".into(),
        reason: "incident".into(),
        actor: "op".into(),
        replica_id: "r1".into(),
        ts: now,
    })
    .await
    .unwrap();
    assert!(repo
        .load_kill_switches()
        .await
        .unwrap()
        .iter()
        .any(|s| s.scope == scope && s.engaged));
    let finding = bot_core::accounting::reconcile(bot_core::accounting::ReconInputs {
        orders: &[],
        positions: &[],
        trades: &[],
        book: &PositionBook::new(),
        events: &[],
        pending: &[format!("led_pending_{run}")],
        since: now,
        tolerance: bot_core::reconciliation::QuantityTolerance::default(),
        replica_id: "r1",
        now,
    })
    .remove(0);
    assert_eq!(
        finding.kind,
        AccountingFindingKind::UnresolvedFinancialEvent
    );
    repo.append_finding(&finding).await.unwrap();
    assert!(repo
        .recent_findings(50)
        .await
        .unwrap()
        .iter()
        .any(|f| f.finding_id == finding.finding_id));

    // TASK 5 §5: a finding about the ORDER layer round-trips with its
    // `order_id` (the intent layer of the reconciliation).
    let mut order_finding = finding.clone();
    order_finding.finding_id = format!("acf_order_{run}");
    order_finding.kind = AccountingFindingKind::UnresolvedFinancialEvent;
    order_finding.order_id = Some(format!("ord-{run}"));
    order_finding.detail = "filled order with no ledger event".into();
    repo.append_finding(&order_finding).await.unwrap();
    let back = repo
        .recent_findings(50)
        .await
        .unwrap()
        .into_iter()
        .find(|f| f.finding_id == order_finding.finding_id)
        .expect("order-layer finding round-trips");
    assert_eq!(back.order_id, order_finding.order_id);
}

fn repo_arc(db: Arc<Database>) -> Arc<bot_core::db::accounting::AccountingRepo> {
    Arc::new(bot_core::db::accounting::AccountingRepo::new(db))
}

/// Minimal durable-store adapter for the test above (the server's
/// `DbLedgerStore` is the production one; this mirrors its semantics).
struct TestLedgerStore(Arc<bot_core::db::accounting::AccountingRepo>);

#[async_trait::async_trait]
impl bot_core::accounting::LedgerStore for TestLedgerStore {
    async fn record_event(
        &self,
        stored: &bot_core::accounting::StoredEvent,
        entry: &bot_core::accounting::Entry,
    ) -> Option<bool> {
        self.0.record_event(stored, entry).await.ok()
    }
    async fn load_events(&self) -> Option<Vec<bot_core::accounting::StoredEvent>> {
        self.0.load_events().await.ok()
    }
    async fn upsert_position(&self, position: &bot_core::accounting::BookPosition) -> bool {
        self.0.upsert_position(position).await.is_ok()
    }
    async fn append_finding(&self, finding: &bot_core::accounting::AccountingFinding) -> bool {
        self.0.append_finding(finding).await.is_ok()
    }
    async fn recent_findings(
        &self,
        limit: usize,
    ) -> Option<Vec<bot_core::accounting::AccountingFinding>> {
        self.0.recent_findings(limit as i64).await.ok()
    }
}
