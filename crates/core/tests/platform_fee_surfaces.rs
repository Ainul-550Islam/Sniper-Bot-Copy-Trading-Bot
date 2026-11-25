//! Surface attribution and fee arithmetic, without implying durable posting.
use bot_core::billing::platform_fee::{
    compute_platform_fee, FeeSurface, PlatformFeeCharge, PlatformFeePolicy,
};
use std::collections::HashMap;

#[test]
fn surface_posting_projections_reconcile_with_assessed_amounts() {
    let policy = PlatformFeePolicy::new(100, Some(10_000), 0).unwrap();
    let surfaces = [
        FeeSurface::Sniper,
        FeeSurface::Copy,
        FeeSurface::TelegramSwap,
        FeeSurface::Polymarket,
    ];
    let mut totals = HashMap::new();
    for (index, surface) in surfaces.into_iter().enumerate() {
        let charge = PlatformFeeCharge::assess(
            format!("fee-{index}"),
            "org",
            "venue",
            "SOL",
            1_000_000,
            policy,
            "tenant",
            "platform",
        )
        .unwrap()
        .with_surface(surface);
        let tenant = charge.entry();
        assert!(tenant.is_balanced());
        let revenue = charge.platform_revenue_posting().unwrap();
        assert!((tenant.postings[0].amount - revenue.amount).abs() < 1e-12);
        *totals.entry(charge.surface.unwrap()).or_insert(0i64) += charge.fee_micros;
    }
    assert_eq!(totals.len(), 4);
    for surface in surfaces {
        assert_eq!(totals[&surface], 10_000);
    }
}

#[test]
fn valid_policies_never_produce_negative_fees_or_exceed_cap() {
    for rate in [0, 1, 25, 100, 500] {
        for cap in [0, 1, 500, 10_000, i64::MAX] {
            let p = PlatformFeePolicy::new(rate, Some(cap), 0).unwrap();
            for n in [0, 1, 10, 1_000_000, i64::MAX / 500, i64::MAX] {
                let fee = compute_platform_fee(n, &p);
                assert!(
                    fee >= 0 && fee <= cap,
                    "rate={rate}, cap={cap}, n={n}, fee={fee}"
                );
            }
        }
    }
}

#[test]
fn old_serialized_charges_remain_unattributed_instead_of_guessing() {
    let charge = PlatformFeeCharge::assess(
        "id",
        "org",
        "venue",
        "SOL",
        10,
        PlatformFeePolicy::new(10, None, 0).unwrap(),
        "tenant",
        "platform",
    )
    .unwrap();
    let mut json = serde_json::to_value(charge).unwrap();
    json.as_object_mut().unwrap().remove("surface");
    let restored: PlatformFeeCharge = serde_json::from_value(json).unwrap();
    assert_eq!(restored.surface, None);
}

#[test]
fn paper_and_simulation_project_no_cash_movement() {
    use bot_core::accounting::Account;
    use bot_core::models::ExecutionMode;
    let charge = PlatformFeeCharge::assess(
        "mode-test",
        "org",
        "venue",
        "SOL",
        1_000_000,
        PlatformFeePolicy::new(100, None, 0).unwrap(),
        "tenant",
        "platform",
    )
    .unwrap()
    .with_surface(FeeSurface::Copy);
    for mode in [ExecutionMode::Paper, ExecutionMode::Simulate] {
        let entry = charge.entry_for_mode(mode);
        assert!(entry.is_balanced());
        assert!(entry.postings.iter().all(|p| p.account != Account::Cash));
        assert_eq!(
            charge.revenue_for_mode(mode).unwrap().account,
            Account::Adjustments
        );
    }
    let live = charge.entry_for_mode(ExecutionMode::Live);
    assert!(live.postings.iter().any(|p| p.account == Account::Cash));
    assert_eq!(
        charge
            .revenue_for_mode(ExecutionMode::Live)
            .unwrap()
            .account,
        Account::Cash
    );
}

#[test]
fn randomized_valid_policy_properties_and_invalid_assessments() {
    use rand::{Rng, SeedableRng};
    let mut rng = rand::rngs::StdRng::seed_from_u64(0xfee5_2026);
    for _ in 0..50_000 {
        let rate = rng.gen_range(0..=500);
        let n = rng.gen_range(0..=i64::MAX);
        let cap = rng.gen_range(0..=i64::MAX);
        let min = rng.gen_range(0..=cap);
        let policy = PlatformFeePolicy::new(rate, Some(cap), min).unwrap();
        let fee = compute_platform_fee(n, &policy);
        assert!(fee >= 0 && fee <= cap);
        if n == 0 || rate == 0 {
            assert_eq!(fee, 0);
        } else {
            assert!(fee >= min);
        }
    }
    for policy in [
        PlatformFeePolicy {
            fee_bps: 501,
            cap_micros: None,
            minimum_micros: 0,
        },
        PlatformFeePolicy {
            fee_bps: 1,
            cap_micros: Some(1),
            minimum_micros: 2,
        },
        PlatformFeePolicy {
            fee_bps: 1,
            cap_micros: None,
            minimum_micros: -1,
        },
    ] {
        assert!(PlatformFeeCharge::assess(
            "id", "org", "venue", "SOL", 100, policy, "wallet", "platform"
        )
        .is_err());
    }
    // Even a manually constructed invalid bps must not wrap into a negative fee.
    let malformed = PlatformFeePolicy {
        fee_bps: u16::MAX,
        cap_micros: None,
        minimum_micros: 0,
    };
    assert_eq!(compute_platform_fee(i64::MAX, &malformed), i64::MAX);
}
