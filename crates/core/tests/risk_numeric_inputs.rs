//! Fail-closed regressions for numeric inputs at the public risk boundary.
//! These exercise real risk decisions, not order submission or market execution.

use bot_core::config::AppConfig;
use bot_core::models::{BotModule, LaunchFeed, PositionSide, TokenLaunch, Venue, WalletTrade};
use bot_core::risk::{EntryRequest, RiskCode, RiskEngine};
use bot_core::state::AppState;
use chrono::Utc;

fn engine() -> RiskEngine {
    let mut cfg = AppConfig::from_defaults();
    cfg.raw.sniper.enabled = true;
    cfg.raw.copy.enabled = true;
    cfg.raw.polymarket.enabled = true;
    RiskEngine::new(AppState::new(cfg))
}

fn request() -> EntryRequest {
    EntryRequest {
        module: BotModule::Sniper,
        venue: Venue::PumpFun,
        symbol: "numeric-regression-mint".into(),
        symbol_display: "NUMERIC".into(),
        requested_quote: 0.01,
        available_quote: 100.0,
        slippage_bps: 0,
        price: None,
        fair_value: None,
        liquidity: None,
        wallet: String::new(),
        strategy: String::new(),
    }
}

fn poly_request() -> EntryRequest {
    EntryRequest {
        module: BotModule::Polymarket,
        venue: Venue::PolymarketClob,
        price: Some(0.5),
        fair_value: Some(0.6),
        liquidity: Some(50_000.0),
        ..request()
    }
}

fn trade(sol_amount: f64) -> WalletTrade {
    WalletTrade {
        wallet: "numeric-regression-leader".into(),
        signature: "numeric-regression-signature".into(),
        slot: 1,
        block_time: None,
        side: PositionSide::Long,
        mint: "numeric-regression-mint".into(),
        symbol: None,
        token_amount: 1.0,
        sol_amount,
        venue: Venue::PumpFun,
        fee_sol: 0.0,
        discriminator: None,
        observed_at: Utc::now(),
    }
}

fn invalid_nonnegative() -> [f64; 4] {
    [f64::NAN, f64::INFINITY, f64::NEG_INFINITY, -1.0]
}

#[tokio::test]
async fn invalid_balance_never_authorizes_an_entry() {
    let e = engine();
    for available_quote in invalid_nonnegative() {
        let r = EntryRequest {
            available_quote,
            ..request()
        };
        let d = e.check_entry(&r).await;
        assert!(!d.allowed(), "balance {available_quote}: {d:?}");
        assert_eq!(d.code, Some(RiskCode::InsufficientBalance));
        assert_eq!(d.sized_quote, 0.0);
    }
}

#[tokio::test]
async fn invalid_liquidity_never_bypasses_the_venue_gate() {
    let e = engine();
    for liquidity in invalid_nonnegative() {
        let r = EntryRequest {
            liquidity: Some(liquidity),
            ..poly_request()
        };
        let d = e.check_entry(&r).await;
        assert!(!d.allowed(), "liquidity {liquidity}: {d:?}");
        assert_eq!(d.code, Some(RiskCode::VenueCheck));
    }
}

#[tokio::test]
async fn invalid_fair_value_never_bypasses_the_edge_gate() {
    let e = engine();
    for fair_value in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY, 1.01, -0.01] {
        let r = EntryRequest {
            fair_value: Some(fair_value),
            ..poly_request()
        };
        let d = e.check_entry(&r).await;
        assert!(!d.allowed(), "fair value {fair_value}: {d:?}");
        assert_eq!(d.code, Some(RiskCode::VenueCheck));
    }
}

#[tokio::test]
async fn invalid_whale_notional_is_not_a_valid_copy_signal() {
    let e = engine();
    let risk = e.state().config_snapshot().await.risk;
    for amount in invalid_nonnegative().into_iter().chain([0.0]) {
        let result = e.check_copy_coded(&trade(amount), &risk, 0.01, None).await;
        assert!(result.is_err(), "whale amount {amount} was accepted");
        assert_eq!(result.unwrap_err().0, RiskCode::InvalidSize);
    }
}

#[tokio::test]
async fn invalid_copy_request_cannot_skip_leader_exposure_checks() {
    let e = engine();
    let risk = e.state().config_snapshot().await.risk;
    for amount in invalid_nonnegative() {
        let result = e.check_copy_coded(&trade(1.0), &risk, amount, None).await;
        assert!(result.is_err(), "copy request {amount} was accepted");
        assert_eq!(result.unwrap_err().0, RiskCode::InvalidSize);
    }
}

#[tokio::test]
async fn invalid_resting_commitment_is_not_assumed_to_be_zero() {
    let e = engine();
    let risk = e.state().config_snapshot().await.risk;
    for resting in invalid_nonnegative() {
        let result = e.check_polymarket_coded(&risk, 0.01, 0, resting, 0.0).await;
        assert!(result.is_err(), "resting commitment {resting} was accepted");
        assert_eq!(result.unwrap_err().0, RiskCode::InvalidSize);
    }
}

#[tokio::test]
async fn invalid_market_commitment_is_not_assumed_to_be_zero() {
    let e = engine();
    let risk = e.state().config_snapshot().await.risk;
    for market in invalid_nonnegative() {
        let result = e.check_polymarket_coded(&risk, 0.01, 0, 0.0, market).await;
        assert!(result.is_err(), "market commitment {market} was accepted");
        assert_eq!(result.unwrap_err().0, RiskCode::InvalidSize);
    }
}

fn launch() -> TokenLaunch {
    TokenLaunch {
        mint: "numeric-regression-mint".into(),
        name: "Numeric Regression".into(),
        symbol: "NUM".into(),
        uri: None,
        creator: "numeric-regression-creator".into(),
        pool: "numeric-regression-pool".into(),
        initial_buy_sol: 1.0,
        market_cap_sol: 10.0,
        market_cap_usd: None,
        total_supply: None,
        slot: None,
        signature: None,
        tx_type: None,
        observed_at: Utc::now(),
        feed: LaunchFeed::Manual,
        socials: None,
    }
}

#[tokio::test]
async fn invalid_creator_buy_never_passes_launch_screening() {
    let e = engine();
    let mut risk = e.state().config_snapshot().await.risk;
    risk.min_socials = 0;
    risk.min_creator_buy_sol = 0.1;
    for amount in invalid_nonnegative() {
        let l = TokenLaunch {
            initial_buy_sol: amount,
            ..launch()
        };
        assert!(
            e.check_launch(&l, &risk).is_err(),
            "creator buy {amount} was accepted"
        );
    }
}

#[tokio::test]
async fn invalid_market_cap_never_passes_launch_screening() {
    let e = engine();
    let mut risk = e.state().config_snapshot().await.risk;
    risk.min_socials = 0;
    risk.max_launch_market_cap_sol = 100.0;
    for amount in invalid_nonnegative() {
        let l = TokenLaunch {
            market_cap_sol: amount,
            ..launch()
        };
        assert!(
            e.check_launch(&l, &risk).is_err(),
            "market cap {amount} was accepted"
        );
    }
}

#[tokio::test]
async fn finite_controls_and_legacy_zero_request_still_work() {
    let e = engine();
    assert!(e.check_entry(&request()).await.allowed());
    assert!(e.check_entry(&poly_request()).await.allowed());
    // None means no model estimate supplied, not an invalid numeric estimate.
    assert!(e
        .check_entry(&EntryRequest {
            fair_value: None,
            ..poly_request()
        })
        .await
        .allowed());
    let mut risk = e.state().config_snapshot().await.risk;
    assert!(e
        .check_copy_coded(&trade(1.0), &risk, 0.01, None)
        .await
        .is_ok());
    // Zero is the documented sentinel used by the legacy check_copy wrapper.
    assert_eq!(e.check_copy(&trade(1.0), &risk).await, Ok(1.0));
    assert!(e
        .check_polymarket_coded(&risk, 0.01, 0, 0.0, 0.0)
        .await
        .is_ok());
    risk.min_socials = 0;
    assert!(e.check_launch(&launch(), &risk).is_ok());
}
