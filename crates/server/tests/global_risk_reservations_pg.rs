//! Cross-replica global-risk admission reservations against real PostgreSQL.
//!
//! Two independent AppStates emulate separate replicas. Their in-memory
//! ledgers and locks are intentionally separate; only PostgreSQL can prevent
//! both 80-unit entries from accepting against a 100-unit portfolio cap.

use std::collections::HashMap;
use std::sync::Arc;

use bot_core::config::{AppConfig, DatabaseConfig};
use bot_core::db::Database;
use bot_core::global_risk::{RiskEntryReservation, RiskStore};
use bot_core::models::{BotModule, Venue};
use bot_core::risk::{EntryRequest, RiskCode, RiskEngine};
use bot_core::state::AppState;
use sniper_suite::accounting::DbRiskStore;

async fn setup() -> Option<Arc<Database>> {
    let url = std::env::var("POSTGRES_URL")
        .ok()
        .filter(|url| !url.trim().is_empty())?;
    let cfg = DatabaseConfig {
        enabled: true,
        auto_migrate: true,
        max_connections: 1,
        min_connections: 0,
        ..Default::default()
    };
    let db = Database::connect(&cfg, &url)
        .await
        .expect("configured PostgreSQL URL must connect");
    db.migrate().await.expect("migrations must apply");
    Some(Arc::new(db))
}

fn state() -> Arc<AppState> {
    let mut config = AppConfig::from_defaults();
    config.raw.sniper.enabled = true;
    config.raw.copy.enabled = true;
    config.raw.polymarket.enabled = true;
    config.raw.global_risk.max_portfolio_exposure_ref = 100.0;
    config.raw.global_risk.reference_rates =
        HashMap::from([("SOL".to_string(), 100.0), ("USDC".to_string(), 1.0)]);
    config.raw.risk.max_open_positions = 100;
    config.raw.risk.max_position_quote = 1_000.0;
    config.raw.risk.max_position_fraction = 1.0;
    config.raw.risk.min_sol_reserve = 0.0;
    config.raw.risk.daily_loss_limit_quote = 0.0;
    AppState::new(config)
}

fn request(module: BotModule, asset: &str) -> EntryRequest {
    EntryRequest {
        module,
        venue: Venue::PumpFun,
        symbol: asset.to_string(),
        symbol_display: asset.to_string(),
        requested_quote: 0.8,
        available_quote: 1_000.0,
        slippage_bps: 100,
        price: None,
        fair_value: None,
        liquidity: None,
        wallet: format!("wallet-{asset}"),
        strategy: module.as_str().to_string(),
    }
}

#[tokio::test]
async fn independent_replicas_share_atomic_global_exposure_reservations() {
    let Some(db) = setup().await else {
        eprintln!("NOT_RUN: global_risk_reservations_pg — POSTGRES_URL missing");
        return;
    };
    let replica_a = state();
    let replica_b = state();
    replica_a
        .global_risk()
        .attach_store(Arc::new(DbRiskStore::new(db.clone())))
        .await;
    replica_b
        .global_risk()
        .attach_store(Arc::new(DbRiskStore::new(db.clone())))
        .await;

    let risk_a = RiskEngine::new(replica_a);
    let risk_b = RiskEngine::new(replica_b);
    let req_a = request(
        BotModule::Sniper,
        &format!("reservation-a-{}", uuid::Uuid::new_v4()),
    );
    let req_b = request(
        BotModule::Copy,
        &format!("reservation-b-{}", uuid::Uuid::new_v4()),
    );

    // Individually each request is worth 80 reference units. With separate
    // process-local ledgers both would otherwise pass a 100-unit cap.
    let (mut a, mut b) = tokio::join!(risk_a.check_entry(&req_a), risk_b.check_entry(&req_b));
    assert_ne!(
        a.allowed(),
        b.allowed(),
        "the shared reservation admits at most one: A={}, B={}",
        a.reason,
        b.reason
    );
    let rejected = if a.allowed() { &b } else { &a };
    assert_eq!(rejected.code, Some(RiskCode::GlobalExposure));

    let store = DbRiskStore::new(db);
    let active: Vec<RiskEntryReservation> = store
        .active_entry_reservations()
        .await
        .expect("reservation table is readable");
    let matching = active
        .iter()
        .filter(|reservation| {
            reservation.asset == req_a.symbol || reservation.asset == req_b.symbol
        })
        .count();
    assert_eq!(matching, 1, "accepted capacity is represented durably");
    if a.allowed() {
        a.release_reservation().await;
    } else {
        b.release_reservation().await;
    }
}
