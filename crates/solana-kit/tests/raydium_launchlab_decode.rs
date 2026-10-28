//! Integration tests for the Raydium LaunchLab event decoders.
//!
//! Every payload in this file is **synthetic**: it is built byte-by-byte in
//! the test from the field order recorded in the vendored official IDL and
//! the official program source. No network access, no real keys, no mainnet
//! data.

use base64::Engine;
use serde_json::Value;
use solana_sdk::pubkey::Pubkey;

use solana_kit::consts::*;
use solana_kit::decode::{Side, SwapVenue};
use solana_kit::events::PumpEvent;
use solana_kit::venues::raydium_launchlab::accounts::LaunchLabPoolStatus;
use solana_kit::venues::raydium_launchlab::decode::{
    to_decoded_swap, to_decoded_swap_from_instruction, to_migration_event, to_pool_creation_event,
    LaunchLabMigrationEvent, LaunchLabPoolCreateEvent, LaunchLabPoolInitEvent,
    LaunchLabSwapInstruction, LaunchLabTradeDirection, LaunchLabTradeEvent,
};

const LAUNCHLAB_IDL: &str = include_str!("../idl/raydium_launchpad.json");

fn u64le(v: u64) -> Vec<u8> {
    v.to_le_bytes().to_vec()
}

/// Build a synthetic `TradeEvent` payload: discriminator + borsh body.
///
/// Field order follows the program's `TradeEvent` struct (and the vendored
/// IDL's `TradeEvent` type): pool state, 13 `u64`s, the direction enum, the
/// pool status and the `exact_in` bool.
fn trade_payload(
    pool_state: &Pubkey,
    amount_in: u64,
    amount_out: u64,
    direction: LaunchLabTradeDirection,
    pool_status: u8,
    exact_in: bool,
) -> Vec<u8> {
    let mut b = Vec::new();
    b.extend_from_slice(&RAYDIUM_LAUNCHLAB_EV_TRADE);
    b.extend_from_slice(pool_state.as_ref());
    b.extend(u64le(100)); // total_base_sell
    b.extend(u64le(200)); // virtual_base
    b.extend(u64le(300)); // virtual_quote
    b.extend(u64le(400)); // real_base_before
    b.extend(u64le(500)); // real_quote_before
    b.extend(u64le(600)); // real_base_after
    b.extend(u64le(700)); // real_quote_after
    b.extend(u64le(amount_in));
    b.extend(u64le(amount_out));
    b.extend(u64le(1)); // protocol_fee
    b.extend(u64le(2)); // platform_fee
    b.extend(u64le(3)); // creator_fee
    b.extend(u64le(4)); // share_fee
    b.push(direction.to_u8());
    b.push(pool_status);
    b.push(u8::from(exact_in));
    b
}

#[test]
fn the_trade_event_discriminator_is_the_anchor_sighash() {
    // sha256("event:TradeEvent")[..8].
    let expected = {
        use sha2::{Digest, Sha256};
        let digest = Sha256::digest(b"event:TradeEvent");
        let mut out = [0u8; 8];
        out.copy_from_slice(&digest[..8]);
        out
    };
    assert_eq!(RAYDIUM_LAUNCHLAB_EV_TRADE, expected);
}

#[test]
fn the_pool_create_event_discriminator_is_the_anchor_sighash() {
    // sha256("event:PoolCreateEvent")[..8].
    let expected = {
        use sha2::{Digest, Sha256};
        let digest = Sha256::digest(b"event:PoolCreateEvent");
        let mut out = [0u8; 8];
        out.copy_from_slice(&digest[..8]);
        out
    };
    assert_eq!(RAYDIUM_LAUNCHLAB_EV_POOL_CREATE, expected);
}

#[test]
fn a_trade_event_decodes_every_field() {
    let pool = Pubkey::new_unique();
    let payload = trade_payload(
        &pool,
        1_000,
        2_000,
        LaunchLabTradeDirection::Buy,
        RAYDIUM_LAUNCHLAB_STATUS_FUND,
        true,
    );
    let ev = LaunchLabTradeEvent::decode(&payload).expect("trade event decodes");
    assert_eq!(ev.pool_state, pool);
    assert_eq!(ev.total_base_sell, 100);
    assert_eq!(ev.virtual_base, 200);
    assert_eq!(ev.virtual_quote, 300);
    assert_eq!(ev.real_base_before, 400);
    assert_eq!(ev.real_quote_before, 500);
    assert_eq!(ev.real_base_after, 600);
    assert_eq!(ev.real_quote_after, 700);
    assert_eq!(ev.amount_in, 1_000);
    assert_eq!(ev.amount_out, 2_000);
    assert_eq!(ev.protocol_fee, 1);
    assert_eq!(ev.platform_fee, 2);
    assert_eq!(ev.creator_fee, 3);
    assert_eq!(ev.share_fee, 4);
    assert_eq!(ev.trade_direction, LaunchLabTradeDirection::Buy);
    assert_eq!(ev.pool_status, RAYDIUM_LAUNCHLAB_STATUS_FUND);
    assert!(ev.exact_in);
}

#[test]
fn a_trade_event_decodes_from_a_base64_log_payload() {
    let pool = Pubkey::new_unique();
    let payload = trade_payload(
        &pool,
        5,
        6,
        LaunchLabTradeDirection::Sell,
        RAYDIUM_LAUNCHLAB_STATUS_TRADE,
        false,
    );
    let b64 = base64::engine::general_purpose::STANDARD.encode(&payload);
    let ev = LaunchLabTradeEvent::decode_base64(&b64).expect("base64 decodes");
    assert_eq!(ev.pool_state, pool);
    assert_eq!(ev.trade_direction, LaunchLabTradeDirection::Sell);
    assert!(!ev.exact_in);
}

#[test]
fn a_trade_event_with_a_wrong_discriminator_fails_closed() {
    let mut payload = trade_payload(
        &Pubkey::new_unique(),
        1,
        2,
        LaunchLabTradeDirection::Buy,
        RAYDIUM_LAUNCHLAB_STATUS_FUND,
        true,
    );
    payload[0] ^= 0xff;
    let err = LaunchLabTradeEvent::decode(&payload).expect_err("wrong discriminator");
    let msg = err.to_string();
    assert!(msg.contains("discriminator"), "{msg}");
}

#[test]
fn a_truncated_trade_event_fails_closed() {
    let payload = trade_payload(
        &Pubkey::new_unique(),
        1,
        2,
        LaunchLabTradeDirection::Buy,
        RAYDIUM_LAUNCHLAB_STATUS_FUND,
        true,
    );
    // Cut the body short: the payload has a discriminator but not enough of
    // the borsh body.
    let err = LaunchLabTradeEvent::decode(&payload[..40]).expect_err("truncated");
    let msg = err.to_string();
    assert!(
        msg.contains("expected at least") || msg.contains("truncated"),
        "{msg}"
    );

    // Too short even for a discriminator.
    let err = LaunchLabTradeEvent::decode(&payload[..4]).expect_err("truncated");
    assert!(err.to_string().contains("too short"), "{err}");
}

#[test]
fn a_trade_event_with_an_unknown_direction_fails_closed() {
    let mut payload = trade_payload(
        &Pubkey::new_unique(),
        1,
        2,
        LaunchLabTradeDirection::Buy,
        RAYDIUM_LAUNCHLAB_STATUS_FUND,
        true,
    );
    // The direction byte sits after the discriminator, the pool key and the
    // 13 `u64`s.
    let dir_at = 8 + 32 + 13 * 8;
    payload[dir_at] = 7;
    let err = LaunchLabTradeEvent::decode(&payload).expect_err("bad direction");
    assert!(err.to_string().contains("trade_direction"), "{err}");
}

#[test]
fn a_trade_event_with_an_unknown_status_fails_closed() {
    let mut payload = trade_payload(
        &Pubkey::new_unique(),
        1,
        2,
        LaunchLabTradeDirection::Buy,
        RAYDIUM_LAUNCHLAB_STATUS_FUND,
        true,
    );
    let status_at = 8 + 32 + 13 * 8 + 1;
    payload[status_at] = 42;
    let err = LaunchLabTradeEvent::decode(&payload).expect_err("bad status");
    assert!(err.to_string().contains("status"), "{err}");
}

#[test]
fn the_graduation_signal_is_the_transition_out_of_fund() {
    let pool = Pubkey::new_unique();
    let funding = LaunchLabTradeEvent::decode(&trade_payload(
        &pool,
        1,
        2,
        LaunchLabTradeDirection::Buy,
        RAYDIUM_LAUNCHLAB_STATUS_FUND,
        true,
    ))
    .unwrap();
    assert!(!funding.is_graduated());
    assert_eq!(funding.status().unwrap(), LaunchLabPoolStatus::Fund);

    let migrating = LaunchLabTradeEvent::decode(&trade_payload(
        &pool,
        1,
        2,
        LaunchLabTradeDirection::Buy,
        RAYDIUM_LAUNCHLAB_STATUS_MIGRATE,
        true,
    ))
    .unwrap();
    assert!(migrating.is_graduated());

    let trading = LaunchLabTradeEvent::decode(&trade_payload(
        &pool,
        1,
        2,
        LaunchLabTradeDirection::Buy,
        RAYDIUM_LAUNCHLAB_STATUS_TRADE,
        true,
    ))
    .unwrap();
    assert!(trading.is_graduated());
}

/// Build a synthetic `PoolCreateEvent` payload.
fn pool_create_payload() -> Vec<u8> {
    let mut b = Vec::new();
    b.extend_from_slice(&RAYDIUM_LAUNCHLAB_EV_POOL_CREATE);
    b.extend_from_slice(Pubkey::new_unique().as_ref()); // pool_state
    b.extend_from_slice(Pubkey::new_unique().as_ref()); // creator
    b.extend_from_slice(Pubkey::new_unique().as_ref()); // config
    b.push(6); // base_decimals
    for s in ["Test Token", "TEST", "https://example.invalid/meta.json"] {
        b.extend_from_slice(&(s.len() as u32).to_le_bytes());
        b.extend_from_slice(s.as_bytes());
    }
    // CurveParams: Constant { supply, total_base_sell, total_quote, migrate }
    b.push(RAYDIUM_LAUNCHLAB_CURVE_CONSTANT);
    b.extend(u64le(1_000_000_000_000_000));
    b.extend(u64le(793_100_000_000_000));
    b.extend(u64le(85_000_000_000));
    b.push(RAYDIUM_LAUNCHLAB_MIGRATE_TO_CPSWAP);
    // VestingParams
    b.extend(u64le(12_000_000));
    b.extend(u64le(10));
    b.extend(u64le(20));
    // AmmCreatorFeeOn
    b.push(0);
    b
}

#[test]
fn a_pool_create_event_decodes_every_field() {
    let payload = pool_create_payload();
    let ev = LaunchLabPoolCreateEvent::decode(&payload).expect("pool create decodes");
    assert_eq!(ev.base_decimals, 6);
    assert_eq!(ev.base_name, "Test Token");
    assert_eq!(ev.base_symbol, "TEST");
    assert_eq!(ev.base_uri, "https://example.invalid/meta.json");
    assert_eq!(ev.curve_type, RAYDIUM_LAUNCHLAB_CURVE_CONSTANT);
    assert_eq!(ev.curve_supply, 1_000_000_000_000_000);
    assert_eq!(ev.curve_total_base_sell, 793_100_000_000_000);
    assert_eq!(ev.curve_total_quote_fund_raising, 85_000_000_000);
    assert_eq!(ev.curve_migrate_type, RAYDIUM_LAUNCHLAB_MIGRATE_TO_CPSWAP);
    assert_eq!(ev.vesting_total_locked_amount, 12_000_000);
    assert_eq!(ev.vesting_cliff_period, 10);
    assert_eq!(ev.vesting_unlock_period, 20);
}

#[test]
fn a_pool_create_event_with_a_wrong_discriminator_fails_closed() {
    let mut payload = pool_create_payload();
    payload[0] ^= 0xff;
    let err = LaunchLabPoolCreateEvent::decode(&payload).expect_err("wrong discriminator");
    assert!(err.to_string().contains("discriminator"), "{err}");
}

#[test]
fn a_pool_create_event_with_an_unknown_curve_type_fails_closed() {
    let mut payload = pool_create_payload();
    // Find the curve discriminant: after the discriminator, three pubkeys, the
    // decimals byte and the three length-prefixed strings.
    let strings_at = 8 + 32 * 3 + 1;
    let mut pos = strings_at;
    for _ in 0..3 {
        let mut l = [0u8; 4];
        l.copy_from_slice(&payload[pos..pos + 4]);
        pos += 4 + u32::from_le_bytes(l) as usize;
    }
    payload[pos] = 9;
    let err = LaunchLabPoolCreateEvent::decode(&payload).expect_err("bad curve");
    assert!(err.to_string().contains("curve type"), "{err}");
}

/// Build a synthetic `initialize_v2` instruction.
///
/// Layout: discriminator, `MintParams` (decimals + three strings),
/// `CurveParams` (u8 discriminant + fields), `VestingParams` (three `u64`s),
/// `AmmCreatorFeeOn` (u8).
fn initialize_v2_data(curve_type: u8) -> Vec<u8> {
    let mut d = Vec::new();
    d.extend_from_slice(&RAYDIUM_LAUNCHLAB_DISC_INITIALIZE_V2);
    d.push(6); // decimals
    for s in ["Test Token", "TEST", "https://example.invalid/meta.json"] {
        d.extend_from_slice(&(s.len() as u32).to_le_bytes());
        d.extend_from_slice(s.as_bytes());
    }
    d.push(curve_type);
    d.extend(u64le(1_000_000_000_000_000)); // supply
    if curve_type == RAYDIUM_LAUNCHLAB_CURVE_CONSTANT {
        d.extend(u64le(793_100_000_000_000)); // total_base_sell
    }
    d.extend(u64le(85_000_000_000)); // total_quote_fund_raising
    d.push(RAYDIUM_LAUNCHLAB_MIGRATE_TO_CPSWAP); // migrate_type
    d.extend(u64le(12_000_000)); // vesting total_locked_amount
    d.extend(u64le(10)); // cliff_period
    d.extend(u64le(20)); // unlock_period
    d.push(0); // amm_creator_fee_on
    d
}

#[test]
fn an_initialize_v2_instruction_decodes_into_a_pool_init_event() {
    let accounts: Vec<Pubkey> = (0..18).map(|_| Pubkey::new_unique()).collect();
    let data = initialize_v2_data(RAYDIUM_LAUNCHLAB_CURVE_CONSTANT);
    let ev = LaunchLabPoolInitEvent::from_instruction(&RAYDIUM_LAUNCHLAB, &accounts, &data)
        .expect("no error")
        .expect("recognized as initialize_v2");

    assert_eq!(ev.pool_state, accounts[5]);
    assert_eq!(ev.creator, accounts[1]);
    assert_eq!(ev.global_config, accounts[2]);
    assert_eq!(ev.platform_config, accounts[3]);
    assert_eq!(ev.base_mint, accounts[6]);
    assert_eq!(ev.quote_mint, accounts[7]);
    assert_eq!(ev.base_vault, accounts[8]);
    assert_eq!(ev.quote_vault, accounts[9]);
    assert_eq!(ev.curve_type, RAYDIUM_LAUNCHLAB_CURVE_CONSTANT);
    assert_eq!(ev.supply, 1_000_000_000_000_000);
    assert_eq!(ev.total_base_sell, 793_100_000_000_000);
    assert_eq!(ev.total_quote_fund_raising, 85_000_000_000);
    assert_eq!(ev.migrate_type, RAYDIUM_LAUNCHLAB_MIGRATE_TO_CPSWAP);
    assert_eq!(ev.decimals, 6);
}

#[test]
fn a_fixed_curve_initialize_v2_has_no_total_base_sell() {
    let accounts: Vec<Pubkey> = (0..18).map(|_| Pubkey::new_unique()).collect();
    let data = initialize_v2_data(RAYDIUM_LAUNCHLAB_CURVE_FIXED);
    let ev = LaunchLabPoolInitEvent::from_instruction(&RAYDIUM_LAUNCHLAB, &accounts, &data)
        .expect("no error")
        .expect("recognized as initialize_v2");
    assert_eq!(ev.curve_type, RAYDIUM_LAUNCHLAB_CURVE_FIXED);
    assert_eq!(ev.total_base_sell, 0);
    assert_eq!(ev.total_quote_fund_raising, 85_000_000_000);
}

#[test]
fn a_non_launchlab_instruction_is_not_recognised() {
    let accounts: Vec<Pubkey> = (0..18).map(|_| Pubkey::new_unique()).collect();
    let data = initialize_v2_data(RAYDIUM_LAUNCHLAB_CURVE_CONSTANT);

    // Wrong program.
    assert!(
        LaunchLabPoolInitEvent::from_instruction(&Pubkey::new_unique(), &accounts, &data)
            .expect("no error")
            .is_none()
    );

    // Right program, wrong instruction.
    assert!(LaunchLabPoolInitEvent::from_instruction(
        &RAYDIUM_LAUNCHLAB,
        &accounts,
        &RAYDIUM_LAUNCHLAB_DISC_BUY_EXACT_IN,
    )
    .expect("no error")
    .is_none());
}

#[test]
fn a_truncated_initialize_v2_instruction_fails_closed() {
    let accounts: Vec<Pubkey> = (0..18).map(|_| Pubkey::new_unique()).collect();
    let data = initialize_v2_data(RAYDIUM_LAUNCHLAB_CURVE_CONSTANT);

    // Too few accounts.
    let err = LaunchLabPoolInitEvent::from_instruction(&RAYDIUM_LAUNCHLAB, &accounts[..5], &data)
        .expect_err("too few accounts");
    assert!(err.to_string().contains("accounts"), "{err}");

    // Truncated data.
    let err = LaunchLabPoolInitEvent::from_instruction(&RAYDIUM_LAUNCHLAB, &accounts, &data[..20])
        .expect_err("truncated data");
    assert!(err.to_string().contains("truncated"), "{err}");
}

#[test]
fn a_buy_trade_converts_into_the_neutral_swap_with_the_other_venue() {
    let pool = Pubkey::new_unique();
    let ev = LaunchLabTradeEvent::decode(&trade_payload(
        &pool,
        1_000,
        2_000,
        LaunchLabTradeDirection::Buy,
        RAYDIUM_LAUNCHLAB_STATUS_FUND,
        true,
    ))
    .unwrap();

    let base_mint = Pubkey::new_unique();
    let quote_mint = Pubkey::new_unique();
    let wallet = Pubkey::new_unique();
    let swap = to_decoded_swap(
        &ev,
        "5signature",
        123,
        Some(1_700_000_000),
        &wallet,
        &base_mint,
        6,
        &quote_mint,
        9,
    )
    .expect("conversion succeeds");

    assert_eq!(swap.side, Side::Buy);
    assert_eq!(swap.base_amount, 2_000);
    assert_eq!(swap.quote_amount, 1_000);
    assert_eq!(swap.base_decimals, 6);
    assert_eq!(swap.quote_decimals, 9);
    assert_eq!(swap.base_mint, base_mint.to_string());
    assert_eq!(swap.quote_mint, quote_mint.to_string());
    assert_eq!(swap.wallet, wallet.to_string());
    assert_eq!(swap.slot, 123);
    assert_eq!(swap.block_time, Some(1_700_000_000));
    assert!(swap.succeeded);
    assert_eq!(swap.programs, vec![RAYDIUM_LAUNCHLAB.to_string()]);
    // `SwapVenue` is a pre-existing public enum matched exhaustively by
    // `module-copy`, which is owned by another batch. The LaunchLab venue
    // therefore reports the generic `Other` venue; the LaunchLab-specific
    // detail stays in `LaunchLabTradeEvent`.
    assert_eq!(swap.venue, SwapVenue::Other);
}

#[test]
fn a_sell_trade_converts_into_the_neutral_swap() {
    let ev = LaunchLabTradeEvent::decode(&trade_payload(
        &Pubkey::new_unique(),
        1_000,
        2_000,
        LaunchLabTradeDirection::Sell,
        RAYDIUM_LAUNCHLAB_STATUS_FUND,
        true,
    ))
    .unwrap();

    let swap = to_decoded_swap(
        &ev,
        "sig",
        1,
        None,
        &Pubkey::new_unique(),
        &Pubkey::new_unique(),
        6,
        &Pubkey::new_unique(),
        9,
    )
    .expect("conversion succeeds");

    assert_eq!(swap.side, Side::Sell);
    assert_eq!(swap.base_amount, 1_000);
    assert_eq!(swap.quote_amount, 2_000);
    assert_eq!(swap.block_time, None);
}

#[test]
fn a_pool_init_converts_into_the_neutral_pool_creation_event() {
    let accounts: Vec<Pubkey> = (0..18).map(|_| Pubkey::new_unique()).collect();
    let data = initialize_v2_data(RAYDIUM_LAUNCHLAB_CURVE_CONSTANT);
    let init = LaunchLabPoolInitEvent::from_instruction(&RAYDIUM_LAUNCHLAB, &accounts, &data)
        .expect("no error")
        .expect("recognized");

    let ev = to_pool_creation_event(&init);
    match ev {
        PumpEvent::CreatePool {
            creator,
            base_mint,
            quote_mint,
            base_mint_decimals,
            pool_base_amount,
            pool_quote_amount,
            pool,
            coin_creator,
            ..
        } => {
            assert_eq!(creator, init.creator);
            assert_eq!(coin_creator, init.creator);
            assert_eq!(base_mint, init.base_mint);
            assert_eq!(quote_mint, init.quote_mint);
            assert_eq!(base_mint_decimals, 6);
            assert_eq!(pool_base_amount, 793_100_000_000_000);
            assert_eq!(pool_quote_amount, 85_000_000_000);
            assert_eq!(pool, init.pool_state);
        }
        other => panic!("unexpected event: {other:?}"),
    }
}

fn idl_accounts_for(ix_name: &str) -> (Vec<Pubkey>, std::collections::HashMap<String, Pubkey>) {
    let parsed: Value = serde_json::from_str(LAUNCHLAB_IDL).expect("valid vendored LaunchLab IDL");
    let accounts_json = parsed["instructions"]
        .as_array()
        .expect("instructions array")
        .iter()
        .find(|ix| ix["name"] == ix_name)
        .unwrap_or_else(|| panic!("missing LaunchLab instruction {ix_name}"))["accounts"]
        .as_array()
        .expect("accounts array")
        .clone();
    let mut keys = Vec::with_capacity(accounts_json.len());
    let mut by_name = std::collections::HashMap::new();
    for (idx, acc) in accounts_json.iter().enumerate() {
        let mut raw = [0u8; 32];
        raw[0] = (idx as u8).wrapping_add(1);
        let pk = Pubkey::new_from_array(raw);
        keys.push(pk);
        by_name.insert(acc["name"].as_str().unwrap().to_string(), pk);
    }
    (keys, by_name)
}

#[test]
fn all_eight_instruction_decoders_match_vendored_launchpad_idl_account_indices() {
    // 1. initialize & initialize_v2
    for (ix_name, disc) in [
        ("initialize", RAYDIUM_LAUNCHLAB_DISC_INITIALIZE),
        ("initialize_v2", RAYDIUM_LAUNCHLAB_DISC_INITIALIZE_V2),
    ] {
        let (accounts, by_name) = idl_accounts_for(ix_name);
        let mut data = initialize_v2_data(RAYDIUM_LAUNCHLAB_CURVE_CONSTANT);
        data[..8].copy_from_slice(&disc);
        if ix_name == "initialize" {
            data.pop();
        }
        let decoded = LaunchLabPoolInitEvent::from_instruction(
            &RAYDIUM_LAUNCHLAB_PROGRAM_ID,
            &accounts,
            &data,
        )
        .unwrap()
        .expect("init decodes");
        assert_eq!(decoded.creator, by_name["creator"]);
        assert_eq!(decoded.global_config, by_name["global_config"]);
        assert_eq!(decoded.platform_config, by_name["platform_config"]);
        assert_eq!(decoded.pool_state, by_name["pool_state"]);
        assert_eq!(decoded.base_mint, by_name["base_mint"]);
        assert_eq!(decoded.quote_mint, by_name["quote_mint"]);
        assert_eq!(decoded.base_vault, by_name["base_vault"]);
        assert_eq!(decoded.quote_vault, by_name["quote_vault"]);
    }

    // 2. buy_exact_in, buy_exact_out, sell_exact_in, sell_exact_out
    for (ix_name, disc, expected_side) in [
        (
            "buy_exact_in",
            RAYDIUM_LAUNCHLAB_DISC_BUY_EXACT_IN,
            Side::Buy,
        ),
        (
            "buy_exact_out",
            RAYDIUM_LAUNCHLAB_DISC_BUY_EXACT_OUT,
            Side::Buy,
        ),
        (
            "sell_exact_in",
            RAYDIUM_LAUNCHLAB_DISC_SELL_EXACT_IN,
            Side::Sell,
        ),
        (
            "sell_exact_out",
            RAYDIUM_LAUNCHLAB_DISC_SELL_EXACT_OUT,
            Side::Sell,
        ),
    ] {
        let (accounts, by_name) = idl_accounts_for(ix_name);
        let mut data = disc.to_vec();
        data.extend(u64le(1_000_000));
        data.extend(u64le(500_000));
        data.extend(u64le(100));
        let decoded = LaunchLabSwapInstruction::from_instruction(
            &RAYDIUM_LAUNCHLAB_PROGRAM_ID,
            &accounts,
            &data,
        )
        .unwrap()
        .expect("trade ix decodes");
        assert_eq!(decoded.payer, by_name["payer"]);
        assert_eq!(decoded.global_config, by_name["global_config"]);
        assert_eq!(decoded.platform_config, by_name["platform_config"]);
        assert_eq!(decoded.pool_state, by_name["pool_state"]);
        assert_eq!(decoded.base_mint, by_name["base_token_mint"]);
        assert_eq!(decoded.quote_mint, by_name["quote_token_mint"]);
        let swap = to_decoded_swap_from_instruction(&decoded, "sig-ll", 77, None, 6, 9).unwrap();
        assert_eq!(swap.side, expected_side);
        assert_eq!(swap.venue, SwapVenue::Other);
    }

    // 3. migrate_to_amm & migrate_to_cpswap
    let (amm_accounts, amm_by_name) = idl_accounts_for("migrate_to_amm");
    let mut amm_data = RAYDIUM_LAUNCHLAB_DISC_MIGRATE_TO_AMM.to_vec();
    amm_data.extend(u64le(1_000));
    amm_data.extend(u64le(1_000));
    amm_data.push(3);
    let mig_amm = LaunchLabMigrationEvent::from_instruction(
        &RAYDIUM_LAUNCHLAB_PROGRAM_ID,
        &amm_accounts,
        &amm_data,
    )
    .unwrap()
    .expect("migrate_to_amm decodes");
    assert_eq!(mig_amm.payer, amm_by_name["payer"]);
    assert_eq!(mig_amm.base_mint, amm_by_name["base_mint"]);
    assert_eq!(mig_amm.quote_mint, amm_by_name["quote_mint"]);
    assert_eq!(mig_amm.destination_pool, amm_by_name["amm_pool"]);
    assert_eq!(mig_amm.pool_state, amm_by_name["pool_state"]);
    assert_eq!(mig_amm.global_config, amm_by_name["global_config"]);

    let (cp_accounts, cp_by_name) = idl_accounts_for("migrate_to_cpswap");
    let mig_cp = LaunchLabMigrationEvent::from_instruction(
        &RAYDIUM_LAUNCHLAB_PROGRAM_ID,
        &cp_accounts,
        &RAYDIUM_LAUNCHLAB_DISC_MIGRATE_TO_CPSWAP,
    )
    .unwrap()
    .expect("migrate_to_cpswap decodes");
    assert_eq!(mig_cp.payer, cp_by_name["payer"]);
    assert_eq!(mig_cp.base_mint, cp_by_name["base_mint"]);
    assert_eq!(mig_cp.quote_mint, cp_by_name["quote_mint"]);
    assert_eq!(mig_cp.destination_pool, cp_by_name["cpswap_pool"]);
    assert_eq!(mig_cp.pool_state, cp_by_name["pool_state"]);
    assert_eq!(mig_cp.global_config, cp_by_name["global_config"]);
    let pump_mig = to_migration_event(&mig_cp);
    match pump_mig {
        PumpEvent::Migration {
            bonding_curve,
            pool,
            mint,
            ..
        } => {
            assert_eq!(pool, cp_by_name["cpswap_pool"]);
            assert_eq!(bonding_curve, cp_by_name["pool_state"]);
            assert_eq!(mint, cp_by_name["base_mint"]);
        }
        other => panic!("expected PumpEvent::Migration, got {other:?}"),
    }
}
