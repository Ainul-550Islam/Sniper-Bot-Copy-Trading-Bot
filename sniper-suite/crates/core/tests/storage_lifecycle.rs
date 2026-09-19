//! Persistence lifecycle integration test: append → load → simulated restart
//! (a fresh [`Store`] over the same directory) → rotate → truncate.
//!
//! This is the durability contract the JSONL tier promises: everything
//! appended before a crash/restart is still readable afterwards, and rotation
//! archives without losing data.

use std::io::Write as _;
use std::time::Duration;

use bot_core::config::StorageConfig;
use bot_core::events::AppEvent;
use bot_core::models::{
    ExecutionMode, Position, PositionSide, PositionStatus, Trade, TradeSource, Venue,
};
use bot_core::storage::{JournalKind, Store};

fn temp_dir(tag: &str) -> std::path::PathBuf {
    let mut dir = std::env::temp_dir();
    let unique = format!(
        "botcore-storage-{tag}-{}-{:?}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or(Duration::ZERO)
            .as_nanos()
    );
    dir.push(unique);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn sample_trade(id: &str) -> Trade {
    Trade {
        id: id.into(),
        ts: chrono::Utc::now(),
        source: TradeSource::Sniper,
        venue: Venue::PumpFun,
        mode: ExecutionMode::Paper,
        side: PositionSide::Long,
        symbol: "MINT123".into(),
        symbol_display: "Mock Token".into(),
        amount_in: 0.25,
        amount_out: 12_345.0,
        quote_symbol: "SOL".into(),
        price: 0.0000202,
        fee: 0.001,
        slippage_bps: 42,
        signature: Some("5xyZ...".into()),
        position_id: Some("pos-1".into()),
        note: None,
        latency_ms: Some(137),
    }
}

fn sample_position(id: &str) -> Position {
    let mut p = Position::new(
        id.into(),
        TradeSource::Sniper,
        Venue::PumpFun,
        ExecutionMode::Paper,
        "MINT123".into(),
        "Mock Token".into(),
        "SOL".into(),
    );
    p.qty = 12_345.0;
    p.avg_entry = 0.0000202;
    p.cost_basis = 0.25;
    p.status = PositionStatus::Open;
    p
}

#[tokio::test]
async fn appended_records_survive_a_restarted_store() {
    let dir = temp_dir("restart");
    let cfg = StorageConfig {
        data_dir: dir.to_string_lossy().to_string(),
        ..Default::default()
    };

    // --- run 1: append -----------------------------------------------------
    let store = Store::open(&cfg).await.expect("store opens");
    store.append_trade(&sample_trade("t1")).await.unwrap();
    store.append_trade(&sample_trade("t2")).await.unwrap();
    store
        .append_position(&sample_position("pos-1"))
        .await
        .unwrap();
    store
        .append_event(&AppEvent::Lifecycle {
            ts: chrono::Utc::now(),
            message: "hello".into(),
        })
        .await
        .unwrap();

    // Same instance reads back what it wrote.
    assert_eq!(store.load_trades().await.unwrap().len(), 2);
    assert_eq!(store.load_positions().await.unwrap().len(), 1);
    assert_eq!(store.load_events().await.unwrap().len(), 1);
    drop(store);

    // --- run 2: "restart" — a brand new Store over the same directory ------
    let reopened = Store::open(&cfg).await.expect("store reopens");
    let trades = reopened.load_trades().await.unwrap();
    assert_eq!(trades.len(), 2, "trades must survive the restart");
    assert_eq!(trades[0].id, "t1");
    assert_eq!(trades[1].id, "t2");
    // Field-level fidelity through the JSONL round trip.
    assert_eq!(trades[0].amount_in, 0.25);
    assert_eq!(trades[0].slippage_bps, 42);
    assert_eq!(trades[0].latency_ms, Some(137));
    assert_eq!(trades[0].signature.as_deref(), Some("5xyZ..."));

    let positions = reopened.load_positions().await.unwrap();
    assert_eq!(positions.len(), 1);
    assert_eq!(positions[0].id, "pos-1");
    assert_eq!(positions[0].qty, 12_345.0);
    assert!(matches!(positions[0].status, PositionStatus::Open));

    let events = reopened.load_events().await.unwrap();
    assert_eq!(events.len(), 1);
    assert!(matches!(&events[0], AppEvent::Lifecycle { message, .. } if message == "hello"));

    // --- corrupt-line tolerance: a torn final line (crash mid-write) must
    // not poison the whole journal.
    std::fs::OpenOptions::new()
        .append(true)
        .open(reopened.trades_path())
        .unwrap()
        .write_all(b"{\"id\": \"t3\", \"ts\": \"broken")
        .unwrap();
    let trades = reopened.load_trades().await.unwrap();
    assert_eq!(
        trades.len(),
        2,
        "a torn trailing line is skipped, not fatal (or is reported elsewhere)"
    );

    std::fs::remove_dir_all(&dir).ok();
}

#[tokio::test]
async fn rotate_archives_and_truncate_clears() {
    let dir = temp_dir("rotate");
    let cfg = StorageConfig {
        data_dir: dir.to_string_lossy().to_string(),
        ..Default::default()
    };
    let store = Store::open(&cfg).await.unwrap();
    store.append_trade(&sample_trade("t1")).await.unwrap();
    store.append_trade(&sample_trade("t2")).await.unwrap();
    assert_eq!(store.load_trades().await.unwrap().len(), 2);

    // Rotate: the journal is archived and the live file restarts empty.
    let archived = store
        .rotate(JournalKind::Trades)
        .await
        .unwrap()
        .expect("rotate returns the archive path");
    assert!(archived.exists(), "archive file must exist: {archived:?}");
    assert!(
        std::fs::metadata(&archived).unwrap().len() > 0,
        "archive must contain the old records"
    );
    assert_eq!(
        store.load_trades().await.unwrap().len(),
        0,
        "live journal is empty after rotation"
    );

    // New writes after rotation accumulate again.
    store.append_trade(&sample_trade("t3")).await.unwrap();
    assert_eq!(store.load_trades().await.unwrap().len(), 1);

    // Truncate: wipe the live journal in place.
    store.truncate(JournalKind::Trades).await.unwrap();
    assert_eq!(store.load_trades().await.unwrap().len(), 0);
    assert_eq!(std::fs::metadata(store.trades_path()).unwrap().len(), 0);

    // JournalKind::parse drives the Telegram/REST maintenance commands.
    assert_eq!(JournalKind::parse("Trades"), Some(JournalKind::Trades));
    assert_eq!(
        JournalKind::parse(" positions "),
        Some(JournalKind::Positions)
    );
    assert_eq!(JournalKind::parse("events"), Some(JournalKind::Events));
    assert_eq!(JournalKind::parse("nope"), None);

    std::fs::remove_dir_all(&dir).ok();
}
