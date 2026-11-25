//! Shared journal regression: a stale replica must not post a sell at zero cost.
use bot_core::accounting::{fill_event, Account, EventSide, GlobalLedger, MemoryLedgerStore};
use bot_core::events::EventBus;
use bot_core::models::{BotModule, ExecutionMode, Venue};
use std::sync::Arc;

fn fill(id: &str, side: EventSide, qty: f64, quote: f64) -> bot_core::accounting::AccountingEvent {
    fill_event(
        BotModule::Sniper,
        Venue::PumpFun,
        "wallet",
        "sniper",
        "mint",
        "SOL",
        side,
        qty,
        quote / qty,
        quote,
        0.0,
        ExecutionMode::Paper,
        id,
        None,
        Some("position".into()),
        chrono::Utc::now(),
        "",
    )
}

/// A replica that never saw the opening buy must still post the sell at the
/// COMMITTED cost — the journal is the shared truth, not the local book.
///
/// This used to be the only `#[ignore]` in `crates/` because the defect was
/// real and unfixed. It is neither now: the cost comes from a replay of the
/// committed postings (`PositionBook::apply_committed`), so a stale replica
/// books the same cost as the replica that saw the buy.
#[tokio::test]
async fn stale_replica_sell_posting_uses_committed_inventory_cost() {
    let store = Arc::new(MemoryLedgerStore::new());
    let a = GlobalLedger::new(EventBus::new(64), "A");
    let b = GlobalLedger::new(EventBus::new(64), "B");
    a.attach_store(store.clone()).await;
    b.attach_store(store.clone()).await;
    let buy = fill("buy", EventSide::Buy, 10.0, 1.0);
    let sell = fill("sell", EventSide::Sell, 5.0, 1.0);
    assert!(a.submit(buy).await.is_new());
    assert!(b.submit(sell.clone()).await.is_new());
    let posted = store.entry(&sell.event_id()).await.unwrap();
    assert!(
        (posted.net(Account::Inventory) + 0.5).abs() < 1e-9,
        "a shared-journal sell must credit the committed 0.5 SOL cost, got {}",
        posted.net(Account::Inventory)
    );
}
