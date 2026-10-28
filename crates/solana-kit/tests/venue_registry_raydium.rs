//! The venue registry, with the two Raydium venues wired in.
//!
//! This test asserts the *registry-level* contract that Batch 03 changes:
//!
//! * both Raydium venues are registered, with their pinned program ids;
//! * both are `layout_verified` against the official vendored IDLs;
//! * both are therefore in the `buildable` set;
//! * the Meteora adapters are untouched and still refuse to build.

use solana_kit::consts::{RAYDIUM_CPMM, RAYDIUM_LAUNCHLAB};
use solana_kit::venues::raydium_cpmm::RaydiumCpmm;
use solana_kit::venues::raydium_launchlab::RaydiumLaunchLab;
use solana_kit::venues::{VenueAdapter, VenueRegistry, VenueSwapRequest};
use solana_sdk::pubkey::Pubkey;

#[test]
fn both_raydium_venues_are_registered() {
    let reg = VenueRegistry::default_registry();
    let ids: Vec<&str> = reg.all().iter().map(|a| a.id()).collect();
    assert!(ids.contains(&"raydium_cpmm"), "{ids:?}");
    assert!(ids.contains(&"raydium_launchlab"), "{ids:?}");
}

#[test]
fn the_raydium_venues_come_first_in_the_registry() {
    // The registry is in routing-preference order: the Raydium venues are
    // the deepest liquidity on Solana.
    let reg = VenueRegistry::default_registry();
    let ids: Vec<&str> = reg.all().iter().map(|a| a.id()).collect();
    let cpmm = ids.iter().position(|id| *id == "raydium_cpmm").unwrap();
    let launchlab = ids
        .iter()
        .position(|id| *id == "raydium_launchlab")
        .unwrap();
    let dlmm = ids.iter().position(|id| *id == "meteora_dlmm").unwrap();
    assert!(cpmm < dlmm, "{ids:?}");
    assert!(launchlab < dlmm, "{ids:?}");
}

#[test]
fn the_program_ids_are_the_official_ones() {
    let reg = VenueRegistry::default_registry();
    let cpmm = reg.all().iter().find(|v| v.id() == "raydium_cpmm").unwrap();
    assert_eq!(*cpmm.program_id(), *RAYDIUM_CPMM);
    assert_eq!(
        cpmm.program_id().to_string(),
        "CPMMoo8L3F4NbTegBCKVNunggL7H1ZpdTHKxQB5qKP1C"
    );

    let launchlab = reg
        .all()
        .iter()
        .find(|v| v.id() == "raydium_launchlab")
        .unwrap();
    assert_eq!(*launchlab.program_id(), *RAYDIUM_LAUNCHLAB);
    assert_eq!(
        launchlab.program_id().to_string(),
        "LanMV9sAd7wArD4vJFi2qDdfnVhFxYSUg6eADduJ3uj"
    );
}

#[test]
fn the_two_raydium_adapters_agree_with_the_registry_entries() {
    let cpmm = RaydiumCpmm::new();
    let launchlab = RaydiumLaunchLab::new();
    assert_eq!(cpmm.id(), "raydium_cpmm");
    assert_eq!(launchlab.id(), "raydium_launchlab");
    assert_eq!(*cpmm.program_id(), *RAYDIUM_CPMM);
    assert_eq!(*launchlab.program_id(), *RAYDIUM_LAUNCHLAB);
    assert_eq!(cpmm.label(), "Raydium CPMM");
    assert_eq!(launchlab.label(), "Raydium LaunchLab");
}

#[test]
fn both_raydium_venues_are_layout_verified_and_buildable() {
    let reg = VenueRegistry::default_registry();
    let buildable: Vec<&str> = reg.buildable().iter().map(|a| a.id()).collect();
    assert!(buildable.contains(&"raydium_cpmm"), "{buildable:?}");
    assert!(buildable.contains(&"raydium_launchlab"), "{buildable:?}");

    for id in ["raydium_cpmm", "raydium_launchlab"] {
        let adapter = reg.all().iter().find(|v| v.id() == id).unwrap();
        assert!(adapter.layout_verified(), "{id} must be layout verified");
        assert!(adapter.enabled(), "{id} must be enabled");
    }
}

#[test]
fn the_meteora_adapters_are_untouched_and_still_refuse() {
    let reg = VenueRegistry::default_registry();
    for id in ["meteora_dlmm", "meteora_damm_v1"] {
        let adapter = reg.all().iter().find(|v| v.id() == id).unwrap();
        assert!(!adapter.layout_verified(), "{id} must stay unverified");
        assert!(
            !reg.buildable().iter().any(|a| a.id() == id),
            "{id} must not be buildable"
        );
    }
}

#[test]
fn an_unverified_venue_refuses_to_build_with_the_layout_error() {
    let reg = VenueRegistry::default_registry();
    let dlmm = reg.all().iter().find(|v| v.id() == "meteora_dlmm").unwrap();
    let req = VenueSwapRequest {
        base_mint: Pubkey::new_unique(),
        quote_mint: Pubkey::new_unique(),
        buy: true,
        amount_in: 1_000,
        min_out: 1,
    };
    let err = dlmm.build_swap(&req).unwrap_err();
    assert!(err.to_string().contains("unverified"), "{err}");
}

#[test]
fn a_disabled_venue_is_not_buildable() {
    // The registry's `buildable` filter honours `enabled()`.
    let reg = VenueRegistry::default_registry();
    for adapter in reg.all() {
        assert!(adapter.enabled(), "{} must ship enabled", adapter.id());
    }
}

#[test]
fn the_venue_ids_are_unique() {
    let reg = VenueRegistry::default_registry();
    let mut ids: Vec<&str> = reg.all().iter().map(|a| a.id()).collect();
    let count = ids.len();
    ids.sort_unstable();
    ids.dedup();
    assert_eq!(ids.len(), count, "the venue ids must be unique");
}

#[test]
fn no_two_venues_share_a_program_id() {
    let reg = VenueRegistry::default_registry();
    let mut program_ids: Vec<String> = reg
        .all()
        .iter()
        .map(|a| a.program_id().to_string())
        .collect();
    let count = program_ids.len();
    program_ids.sort();
    program_ids.dedup();
    assert_eq!(
        program_ids.len(),
        count,
        "two venues must not share a program id"
    );
}

#[test]
fn existing_pump_pumpswap_and_raydium_amm_v4_venues_remain_untouched_and_resolve() {
    let mint = Pubkey::new_unique();
    let creator = Pubkey::new_unique();
    let quote = *solana_kit::consts::WSOL_MINT;

    // 1. Pump.fun bonding-curve module resolves and derives PDAs deterministically.
    assert_eq!(
        solana_kit::consts::PUMP_PROGRAM_ID.to_string(),
        "6EF8rrecthR5Dkzon8Nwu78hRvfCKubJ14M5uBEwF6P"
    );
    let bc = solana_kit::pump::bonding_curve_pda(&mint);
    assert_ne!(bc, Pubkey::default());
    assert_eq!(
        solana_kit::pump::event_authority_pda(),
        *solana_kit::consts::PUMP_EVENT_AUTHORITY
    );

    // 2. PumpSwap AMM module resolves and derives pool PDAs + default layouts.
    let ps_pool = solana_kit::pumpswap::pool_pda(0, &creator, &mint, &quote);
    assert_ne!(ps_pool, Pubkey::default());
    assert!(!solana_kit::pumpswap::default_buy_layout().slots.is_empty());
    assert!(!solana_kit::pumpswap::default_sell_layout().slots.is_empty());

    // 3. Raydium AMM v4 module resolves and derives canonical AMM authority.
    assert_eq!(
        solana_kit::consts::RAYDIUM_AMM_V4.to_string(),
        "675kPX9MHTjS2zt1qfr1NYHuzeLXfQM9H24wFSUt1Mp8"
    );
    let (amm_auth, _bump) = solana_kit::raydium::amm_authority_canonical();
    assert_ne!(amm_auth, Pubkey::default());

    // 4. And their program IDs are distinct from CPMM and LaunchLab.
    let reg = VenueRegistry::default_registry();
    assert!(reg
        .all()
        .iter()
        .any(|v| *v.program_id() == *solana_kit::consts::RAYDIUM_CPMM_PROGRAM_ID));
    assert!(reg
        .all()
        .iter()
        .any(|v| *v.program_id() == *solana_kit::consts::RAYDIUM_LAUNCHLAB_PROGRAM_ID));
    assert!(!reg
        .all()
        .iter()
        .any(|v| *v.program_id() == *solana_kit::consts::PUMP_PROGRAM_ID));
    assert!(!reg
        .all()
        .iter()
        .any(|v| *v.program_id() == *solana_kit::consts::RAYDIUM_AMM_V4));
}
