//! Gate × risk-intel integration cases (GAP-MAP v2, P2).
//!
//! The P1 work added the holder / creator / bundler / sell-simulation
//! gates to [`module_sniper::gates`]; the P2 `risk_intel` module supplies
//! the measurements that feed them. These tests pin the CONTRACT between
//! the two layers:
//!
//! * `gates::evaluate` behaves correctly for every data state of the four
//!   risk-intel gates (present/fail, present/pass, absent → Skip, and
//!   strict_gates promoting Skip to refusal);
//! * the risk-intel conversions (`bps_from_pct`, bundler ratios, honeypot
//!   verdicts, creator rug rates) AGREE with the gate arithmetic — the
//!   same threshold must mean the same thing on both sides;
//! * nothing here touches the network: every case is pure data.

use chrono::{Duration, Utc};

use bot_core::config::{AppConfig, SniperConfig};
use module_sniper::gates::{
    self as gates, BundlingSignal, GateOutcome, MarketSnapshot, TokenHolder,
    GATE_BUNDLER_DETECTION, GATE_CREATOR_CONCENTRATION, GATE_HOLDER_CONCENTRATION,
    GATE_SELL_SIMULATION,
};
use module_sniper::event::LaunchProtocol;
use module_sniper::risk_intel::{
    bundler, creator_history, holders, honeypot,
};
use solana_kit::token_safety::SellProbeOutcome;

// ---------------------------------------------------------------------------
// Fixture builders
// ---------------------------------------------------------------------------

/// A healthy pump.fun snapshot: every default gate passes.
fn healthy_snapshot() -> MarketSnapshot {
    MarketSnapshot {
        protocol: LaunchProtocol::PumpFun,
        pool: Some("curve".into()),
        quote_reserve_lamports: 30_000_000_000,
        pricing_quote_reserve_lamports: 30_000_000_000,
        base_reserve_raw: 800_000_000_000_000,
        base_decimals: 6,
        total_supply_raw: Some(1_000_000_000_000_000),
        fee_bps: 100,
        tradable: true,
        tradable_detail: String::new(),
        pool_open_time: None,
        mint_authority_revoked: Some(true),
        freeze_authority_revoked: Some(true),
        sell_probe: Some(SellProbeOutcome::Sellable),
        creator_initial_buy_sol: Some(0.5),
        top_holders: None,
        bundling: None,
        spot_price_sol: 0.00003,
        fetched_at: Utc::now(),
    }
}

/// Sniper config with the FOUR risk-intel gates enabled (defaults ship
/// them disabled = silent pass).
fn gated_config() -> SniperConfig {
    let mut cfg = AppConfig::from_defaults().raw.sniper;
    cfg.max_creator_initial_buy_sol = 2.0; // SOL
    cfg.max_top_holder_pct = 20.0; // percent
    cfg.max_bundler_ratio = 0.5; // fraction
    cfg.simulate_sell = true;
    cfg.strict_gates = false;
    cfg
}

fn gate_outcome<'a>(report: &'a gates::GateReport, gate: &'a str) -> &'a GateOutcome {
    report
        .results
        .iter()
        .find(|(g, _)| g == gate)
        .map(|(_, o)| o)
        .unwrap_or_else(|| panic!("gate {gate} missing from report"))
}

// ---------------------------------------------------------------------------
// Holder concentration
// ---------------------------------------------------------------------------

#[test]
fn holder_gate_fails_on_a_whale_passes_on_distribution() {
    let cfg = gated_config();
    let now = Utc::now();

    let mut snap = healthy_snapshot();
    snap.top_holders = Some(vec![
        TokenHolder {
            address: "CurveVault".into(),
            pct_of_supply: 80.0,
            is_infrastructure: true, // never counts
        },
        TokenHolder {
            address: "Whale111".into(),
            pct_of_supply: 35.0, // > 20% gate
            is_infrastructure: false,
        },
    ]);
    let report = gates::evaluate(&snap, &cfg, now);
    match gate_outcome(&report, GATE_HOLDER_CONCENTRATION) {
        GateOutcome::Fail(detail) => assert!(detail.contains("Whale111"), "{detail}"),
        other => panic!("expected Fail, got {other:?}"),
    }
    assert!(!report.passed(false));

    // Same whale under the limit passes; infrastructure never counts even
    // at 80%.
    snap.top_holders = Some(vec![
        TokenHolder {
            address: "CurveVault".into(),
            pct_of_supply: 80.0,
            is_infrastructure: true,
        },
        TokenHolder {
            address: "NormalGuy".into(),
            pct_of_supply: 12.0,
            is_infrastructure: false,
        },
    ]);
    let report = gates::evaluate(&snap, &cfg, now);
    assert_eq!(
        gate_outcome(&report, GATE_HOLDER_CONCENTRATION),
        &GateOutcome::Pass
    );
    assert!(report.passed(false));
}

#[test]
fn holder_gate_skip_semantics_and_strict_promotion() {
    let mut cfg = gated_config();
    let now = Utc::now();
    let snap = healthy_snapshot(); // top_holders = None

    // Non-strict: missing data is a Skip and the report still passes.
    let report = gates::evaluate(&snap, &cfg, now);
    assert!(matches!(
        gate_outcome(&report, GATE_HOLDER_CONCENTRATION),
        GateOutcome::Skip(_)
    ));
    assert!(report.passed(false));

    // Strict: the same Skip becomes a refusal.
    cfg.strict_gates = true;
    let report = gates::evaluate(&snap, &cfg, now);
    assert!(!report.passed(true));
    let (gate, _) = report.first_failure(true).unwrap();
    assert_eq!(gate, GATE_HOLDER_CONCENTRATION);

    // Disabled threshold (<= 0) passes even without data.
    cfg.max_top_holder_pct = 0.0;
    let report = gates::evaluate(&snap, &cfg, now);
    assert!(report.passed(true));
}

#[test]
fn holder_bps_conversion_agrees_with_gate_threshold() {
    // The risk-intel side speaks basis points; the gate config speaks
    // percent. The conversion must round-trip the operator's threshold.
    assert_eq!(holders::bps_from_pct(20.0), 2_000);
    assert_eq!(holders::bps_from_pct(0.5), 50);
    assert!((holders::pct_from_bps(2_000) - 20.0).abs() < 1e-12);
    // Degenerate thresholds fail closed to "gate off" (0), never panic.
    assert_eq!(holders::bps_from_pct(0.0), 0);
    assert_eq!(holders::bps_from_pct(-5.0), 0);
    assert_eq!(holders::bps_from_pct(f64::NAN), 0);
    assert_eq!(holders::bps_from_pct(250.0), 10_000, "capped at 100%");
}

// ---------------------------------------------------------------------------
// Creator concentration + history
// ---------------------------------------------------------------------------

#[test]
fn creator_gate_bounds_the_opening_buy() {
    let cfg = gated_config();
    let now = Utc::now();

    let mut snap = healthy_snapshot();
    snap.creator_initial_buy_sol = Some(5.0); // > 2 SOL limit
    let report = gates::evaluate(&snap, &cfg, now);
    assert!(matches!(
        gate_outcome(&report, GATE_CREATOR_CONCENTRATION),
        GateOutcome::Fail(_)
    ));

    snap.creator_initial_buy_sol = Some(1.0);
    let report = gates::evaluate(&snap, &cfg, now);
    assert_eq!(
        gate_outcome(&report, GATE_CREATOR_CONCENTRATION),
        &GateOutcome::Pass
    );

    // Source without the field: Skip (strict decides), never silent Pass.
    snap.creator_initial_buy_sol = None;
    let report = gates::evaluate(&snap, &cfg, now);
    assert!(matches!(
        gate_outcome(&report, GATE_CREATOR_CONCENTRATION),
        GateOutcome::Skip(_)
    ));
}

#[test]
fn creator_history_rug_rate_flags_serial_ruggers_only_with_samples() {
    use creator_history::{CreatorLedger, LaunchOutcome, LaunchRecord};
    use solana_sdk::pubkey::Pubkey;

    let mut ledger = CreatorLedger::new(3); // min 3 samples before judging
    let creator = Pubkey::new_unique();
    let now = Utc::now();
    for i in 0..2 {
        ledger.record(
            &creator,
            LaunchRecord {
                mint: Pubkey::new_unique(),
                outcome: LaunchOutcome::Rugged,
                observed_at: now - Duration::hours(i),
            },
        );
    }
    // 2/2 rugs — but below the min-sample floor, so the gate stays silent.
    assert_eq!(ledger.rug_rate_bps(&creator), None);
    assert!(!ledger.is_serial_rugger(&creator, 2));

    // Third launch tips the sample count; now the rate is reportable.
    ledger.record(
        &creator,
        LaunchRecord {
            mint: Pubkey::new_unique(),
            outcome: LaunchOutcome::Rugged,
            observed_at: now,
        },
    );
    assert_eq!(ledger.rug_rate_bps(&creator), Some(10_000));
    assert!(ledger.is_serial_rugger(&creator, 3));

    // A migrated launch dilutes but non-rugs never COUNT as rugs.
    let honest = Pubkey::new_unique();
    for outcome in [
        LaunchOutcome::Migrated,
        LaunchOutcome::Migrated,
        LaunchOutcome::Active,
    ] {
        ledger.record(
            &honest,
            LaunchRecord {
                mint: Pubkey::new_unique(),
                outcome,
                observed_at: now,
            },
        );
    }
    assert_eq!(ledger.rug_rate_bps(&honest), Some(0));
    // Unknown creator: None, never "guilty".
    assert_eq!(ledger.rug_rate_bps(&Pubkey::new_unique()), None);
}

// ---------------------------------------------------------------------------
// Bundler detection
// ---------------------------------------------------------------------------

#[test]
fn bundler_gate_ratio_matches_risk_intel_clustering() {
    let cfg = gated_config(); // max_bundler_ratio = 0.5
    let now = Utc::now();

    // 4 first-slot buyers, 3 bundled -> 0.75 >= 0.5 -> Fail.
    let mut snap = healthy_snapshot();
    snap.bundling = Some(BundlingSignal {
        first_slot_buyers: 4,
        bundled_wallets: 3,
    });
    let report = gates::evaluate(&snap, &cfg, now);
    assert!(matches!(
        gate_outcome(&report, GATE_BUNDLER_DETECTION),
        GateOutcome::Fail(_)
    ));

    // 1 of 4 bundled -> 0.25 -> Pass.
    snap.bundling = Some(BundlingSignal {
        first_slot_buyers: 4,
        bundled_wallets: 1,
    });
    let report = gates::evaluate(&snap, &cfg, now);
    assert_eq!(
        gate_outcome(&report, GATE_BUNDLER_DETECTION),
        &GateOutcome::Pass
    );

    // Zero-buyer signal = analyser misfire -> Skip, never Pass.
    snap.bundling = Some(BundlingSignal {
        first_slot_buyers: 0,
        bundled_wallets: 0,
    });
    let report = gates::evaluate(&snap, &cfg, now);
    assert!(matches!(
        gate_outcome(&report, GATE_BUNDLER_DETECTION),
        GateOutcome::Skip(_)
    ));
}

#[test]
fn bundler_clustering_agrees_with_the_gate_threshold() {
    use bundler::{analyze, exceeds, FirstSlotBuy};
    use solana_sdk::pubkey::Pubkey;

    let funder = Pubkey::new_unique();
    let solo = Pubkey::new_unique();
    let buys: Vec<FirstSlotBuy> = vec![
        FirstSlotBuy { buyer: Pubkey::new_unique(), funder },
        FirstSlotBuy { buyer: Pubkey::new_unique(), funder },
        FirstSlotBuy { buyer: Pubkey::new_unique(), funder },
        // Same funder backing ONE buyer is not a cluster.
        FirstSlotBuy { buyer: Pubkey::new_unique(), funder: solo },
    ];
    let report = analyze(&buys);
    assert_eq!(report.total_buyers, 4);
    assert_eq!(report.clustered_buyers, 3);
    assert_eq!(report.bundler_ratio_bps, 7_500);

    // The gate's 0.5 fraction and risk-intel's 7500 bps must agree.
    assert!(exceeds(&report, 0.5));
    assert!(!exceeds(&report, 0.8), "75% < 80% threshold");
    // 0 / negative / NaN thresholds disable the check (never reject).
    assert!(!exceeds(&report, 0.0));
    assert!(!exceeds(&report, -1.0));
    assert!(!exceeds(&report, f64::NAN));

    // Duplicate buyers are deduplicated before clustering.
    let dup = vec![
        FirstSlotBuy { buyer: buys[0].buyer, funder },
        FirstSlotBuy { buyer: buys[0].buyer, funder },
    ];
    assert_eq!(analyze(&dup).total_buyers, 1);
}

// ---------------------------------------------------------------------------
// Sell simulation / honeypot
// ---------------------------------------------------------------------------

#[test]
fn sell_simulation_gate_maps_probe_outcomes() {
    let cfg = gated_config(); // simulate_sell = true, strict off
    let now = Utc::now();

    let mut snap = healthy_snapshot();
    snap.sell_probe = Some(SellProbeOutcome::NotSellable(
        "transfer hook reverts".into(),
    ));
    let report = gates::evaluate(&snap, &cfg, now);
    match gate_outcome(&report, GATE_SELL_SIMULATION) {
        GateOutcome::Fail(detail) => assert!(detail.contains("transfer hook"), "{detail}"),
        other => panic!("expected Fail, got {other:?}"),
    }

    snap.sell_probe = Some(SellProbeOutcome::Sellable);
    let report = gates::evaluate(&snap, &cfg, now);
    assert_eq!(gate_outcome(&report, GATE_SELL_SIMULATION), &GateOutcome::Pass);

    snap.sell_probe = Some(SellProbeOutcome::Unknown("rpc timeout".into()));
    let report = gates::evaluate(&snap, &cfg, now);
    assert!(matches!(
        gate_outcome(&report, GATE_SELL_SIMULATION),
        GateOutcome::Skip(_)
    ));
    assert!(report.passed(false), "non-strict: unknown is a skip");

    snap.sell_probe = None;
    let report = gates::evaluate(&snap, &cfg, now);
    assert!(
        matches!(
            gate_outcome(&report, GATE_SELL_SIMULATION),
            GateOutcome::Skip(_)
        ),
        "enabled gate without a probe must skip, not pass"
    );

    // Disabled probe gate passes silently regardless of data.
    let mut cfg_off = cfg.clone();
    cfg_off.simulate_sell = false;
    snap.sell_probe = Some(SellProbeOutcome::NotSellable("x".into()));
    let report = gates::evaluate(&snap, &cfg_off, now);
    assert_eq!(gate_outcome(&report, GATE_SELL_SIMULATION), &GateOutcome::Pass);
}

#[test]
fn honeypot_verdicts_match_gate_strictness() {
    use honeypot::{decide, rejects_with, HoneypotVerdict};

    let broken = SellProbeOutcome::NotSellable("frozen".into());
    assert_eq!(decide(&broken), HoneypotVerdict::Honeypot);
    assert!(rejects_with(HoneypotVerdict::Honeypot, false));
    assert!(rejects_with(HoneypotVerdict::Honeypot, true));

    let unknown = SellProbeOutcome::Unknown("timeout".into());
    assert_eq!(decide(&unknown), HoneypotVerdict::Unknown);
    assert!(
        !rejects_with(HoneypotVerdict::Unknown, false),
        "non-strict: unknown is a skip"
    );
    assert!(
        rejects_with(HoneypotVerdict::Unknown, true),
        "strict: unknown rejects — mirrors gates.rs Skip promotion"
    );

    let ok = SellProbeOutcome::Sellable;
    assert_eq!(decide(&ok), HoneypotVerdict::Sellable);
    assert!(!rejects_with(HoneypotVerdict::Sellable, true));
}

#[test]
fn honeypot_cache_ttl_round_trips_and_expires() {
    use honeypot::SellProbeCache;
    use solana_sdk::pubkey::Pubkey;

    let mut cache = SellProbeCache::new(std::time::Duration::from_secs(60));
    let mint = Pubkey::new_unique();
    let now = Utc::now();
    cache.insert(mint, SellProbeOutcome::Sellable, now);
    assert!(cache.get(&mint, now).is_some());
    assert!(cache.get(&mint, now + Duration::seconds(59)).is_some());
    assert!(
        cache.get(&mint, now + Duration::seconds(61)).is_none(),
        "entries expire after the TTL"
    );
    // Zero TTL disables the cache entirely.
    let mut off = SellProbeCache::new(std::time::Duration::ZERO);
    off.insert(mint, SellProbeOutcome::Sellable, now);
    assert!(off.get(&mint, now).is_none());
}

// ---------------------------------------------------------------------------
// Cross-gate composition
// ---------------------------------------------------------------------------

#[test]
fn first_failure_reports_the_earliest_gate_in_order() {
    let cfg = gated_config();
    let now = Utc::now();
    let mut snap = healthy_snapshot();
    // Fail BOTH holder and bundler gates; the report must surface the
    // holder gate first (evaluation order is fixed).
    snap.top_holders = Some(vec![TokenHolder {
        address: "Whale".into(),
        pct_of_supply: 90.0,
        is_infrastructure: false,
    }]);
    snap.bundling = Some(BundlingSignal {
        first_slot_buyers: 2,
        bundled_wallets: 2,
    });
    let report = gates::evaluate(&snap, &cfg, now);
    let (gate, _) = report.first_failure(false).unwrap();
    assert_eq!(gate, GATE_HOLDER_CONCENTRATION);

    // The machine reason maps both risk-intel gates to the same reject
    // family (concentration), keeping metrics labels stable.
    assert_eq!(
        gates::reason_for_gate(GATE_HOLDER_CONCENTRATION),
        gates::reason_for_gate(GATE_BUNDLER_DETECTION)
    );
}
