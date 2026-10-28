//! CPMM event decoding.
//!
//! The `SwapEvent` layout is the official program's
//! `programs/cp-swap/src/states/events.rs` → `SwapEvent`, and the field
//! order and types are cross-checked against the vendored official IDL's
//! `SwapEvent` type.

use serde_json::Value;
use solana_kit::consts::{
    RAYDIUM_CPMM_DISC_INITIALIZE, RAYDIUM_CPMM_DISC_INITIALIZE_WITH_PERMISSION,
    RAYDIUM_CPMM_DISC_SWAP_BASE_INPUT, RAYDIUM_CPMM_DISC_SWAP_BASE_OUTPUT, RAYDIUM_CPMM_EV_SWAP,
    RAYDIUM_CPMM_PROGRAM_ID,
};
use solana_kit::decode::{Side, SwapVenue};
use solana_kit::venues::raydium_cpmm::decode::{
    to_decoded_swap, to_decoded_swap_from_instruction, to_pool_creation_event, CpmmPoolInitEvent,
    CpmmSwapEvent, CpmmSwapInstruction, CpmmTradeSide,
};
use solana_sdk::pubkey::Pubkey;

const IDL: &str = include_str!("../idl/raydium_cp_swap.json");

fn idl() -> Value {
    serde_json::from_str(IDL).expect("the vendored IDL is valid JSON")
}

/// The IDL's `SwapEvent` field names, in order.
fn idl_event_fields() -> Vec<String> {
    idl()["types"]
        .as_array()
        .expect("the IDL has types")
        .iter()
        .find(|t| t["name"] == "SwapEvent")
        .unwrap_or_else(|| panic!("the IDL has a SwapEvent type"))["type"]["fields"]
        .as_array()
        .expect("SwapEvent has fields")
        .iter()
        .map(|f| {
            f["name"]
                .as_str()
                .expect("the field has a name")
                .to_string()
        })
        .collect()
}

/// Build a synthetic `SwapEvent` payload in the official field order.
fn encode_swap_event(base_input: bool) -> Vec<u8> {
    let mut b = Vec::new();
    b.extend_from_slice(&RAYDIUM_CPMM_EV_SWAP);
    b.extend_from_slice(&[1u8; 32]); // pool_id
    b.extend_from_slice(&1_000u64.to_le_bytes()); // input_vault_before
    b.extend_from_slice(&2_000u64.to_le_bytes()); // output_vault_before
    b.extend_from_slice(&100u64.to_le_bytes()); // input_amount
    b.extend_from_slice(&200u64.to_le_bytes()); // output_amount
    b.extend_from_slice(&3u64.to_le_bytes()); // input_transfer_fee
    b.extend_from_slice(&4u64.to_le_bytes()); // output_transfer_fee
    b.push(u8::from(base_input)); // base_input
    b.extend_from_slice(&[2u8; 32]); // input_mint
    b.extend_from_slice(&[3u8; 32]); // output_mint
    b.extend_from_slice(&5u64.to_le_bytes()); // trade_fee
    b.extend_from_slice(&6u64.to_le_bytes()); // creator_fee
    b.push(0); // creator_fee_on_input
    b
}

#[test]
fn the_idl_event_fields_match_the_order_the_decoder_uses() {
    assert_eq!(
        idl_event_fields(),
        vec![
            "pool_id",
            "input_vault_before",
            "output_vault_before",
            "input_amount",
            "output_amount",
            "input_transfer_fee",
            "output_transfer_fee",
            "base_input",
            "input_mint",
            "output_mint",
            "trade_fee",
            "creator_fee",
            "creator_fee_on_input",
        ]
    );
}

#[test]
fn the_event_discriminator_is_the_anchor_sighash() {
    // `sha256("event:SwapEvent")[..8]`.
    let expected = {
        use sha2::{Digest, Sha256};
        let digest = Sha256::digest(b"event:SwapEvent");
        let mut out = [0u8; 8];
        out.copy_from_slice(&digest[..8]);
        out
    };
    assert_eq!(RAYDIUM_CPMM_EV_SWAP, expected);
}

#[test]
fn the_event_decodes_every_field() {
    let ev = CpmmSwapEvent::decode(&encode_swap_event(true)).expect("the event decodes");
    assert_eq!(ev.pool_id, Pubkey::new_from_array([1u8; 32]));
    assert_eq!(ev.input_vault_before, 1_000);
    assert_eq!(ev.output_vault_before, 2_000);
    assert_eq!(ev.input_amount, 100);
    assert_eq!(ev.output_amount, 200);
    assert_eq!(ev.input_transfer_fee, 3);
    assert_eq!(ev.output_transfer_fee, 4);
    assert!(ev.base_input);
    assert_eq!(ev.input_mint, Pubkey::new_from_array([2u8; 32]));
    assert_eq!(ev.output_mint, Pubkey::new_from_array([3u8; 32]));
    assert_eq!(ev.trade_fee, 5);
    assert_eq!(ev.creator_fee, 6);
    assert!(!ev.creator_fee_on_input);
}

#[test]
fn a_wrong_discriminator_is_refused() {
    let mut bytes = encode_swap_event(true);
    bytes[0] ^= 0xff;
    let err = CpmmSwapEvent::decode(&bytes).unwrap_err();
    assert!(err.to_string().contains("discriminator"), "{err}");
}

#[test]
fn a_truncated_body_is_refused() {
    let bytes = encode_swap_event(true);
    assert!(CpmmSwapEvent::decode(&bytes[..bytes.len() - 1]).is_err());
    assert!(CpmmSwapEvent::decode(&bytes[..4]).is_err());
    assert!(CpmmSwapEvent::decode(&[]).is_err());
}

#[test]
fn the_event_decodes_from_a_base64_log_payload() {
    use base64::Engine;
    let bytes = encode_swap_event(false);
    let payload = base64::engine::general_purpose::STANDARD.encode(&bytes);
    let ev = CpmmSwapEvent::decode_base64(&payload).expect("the base64 payload decodes");
    assert!(!ev.base_input);
    assert_eq!(ev.input_amount, 100);
}

#[test]
fn the_trade_side_follows_which_mint_is_the_base() {
    let ev = CpmmSwapEvent::decode(&encode_swap_event(true)).unwrap();
    // The base mint is the one being traded. When it is the *input* the
    // trader sold the base; when it is the *output* the trader bought it.
    assert_eq!(
        CpmmTradeSide::from_event(&ev, &ev.input_mint),
        Some(CpmmTradeSide::Sell)
    );
    assert_eq!(
        CpmmTradeSide::from_event(&ev, &ev.output_mint),
        Some(CpmmTradeSide::Buy)
    );
    // A mint that is neither the input nor the output is refused.
    assert_eq!(CpmmTradeSide::from_event(&ev, &Pubkey::new_unique()), None);
}

#[test]
fn the_neutral_conversion_reports_the_cpmm_venue() {
    let ev = CpmmSwapEvent::decode(&encode_swap_event(true)).unwrap();
    // The base mint is the output mint, so the trader bought the base.
    let swap = to_decoded_swap(
        &ev,
        "sig",
        42,
        Some(1_700_000_000),
        &Pubkey::new_from_array([9u8; 32]),
        &ev.output_mint,
        6,
        9,
    )
    .expect("the conversion succeeds");
    assert_eq!(swap.venue, SwapVenue::RaydiumCpmm);
    assert_eq!(swap.side, Side::Buy);
    assert_eq!(swap.base_amount, 200);
    assert_eq!(swap.quote_amount, 100);
    assert_eq!(swap.slot, 42);
    assert_eq!(swap.signature, "sig");
    assert!(swap.succeeded);
    assert!(swap
        .programs
        .contains(&solana_kit::consts::RAYDIUM_CPMM.to_string()));
}

#[test]
fn a_sell_is_reported_as_a_sell() {
    let ev = CpmmSwapEvent::decode(&encode_swap_event(false)).unwrap();
    let swap = to_decoded_swap(
        &ev,
        "sig",
        42,
        None,
        &Pubkey::new_from_array([9u8; 32]),
        &ev.input_mint,
        6,
        9,
    )
    .expect("the conversion succeeds");
    assert_eq!(swap.side, Side::Sell);
    assert_eq!(swap.venue, SwapVenue::RaydiumCpmm);
}

#[test]
fn the_pool_creation_event_is_produced_from_the_initialize_instruction() {
    let init = CpmmPoolInitEvent {
        pool_state: Pubkey::new_from_array([1u8; 32]),
        amm_config: Pubkey::new_from_array([2u8; 32]),
        creator: Pubkey::new_from_array([3u8; 32]),
        token_0_mint: Pubkey::new_from_array([4u8; 32]),
        token_1_mint: Pubkey::new_from_array([5u8; 32]),
        lp_mint: Pubkey::new_from_array([6u8; 32]),
        token_0_vault: Pubkey::new_from_array([7u8; 32]),
        token_1_vault: Pubkey::new_from_array([8u8; 32]),
        observation_state: Pubkey::new_from_array([9u8; 32]),
        init_amount_0: 1_000,
        init_amount_1: 2_000,
        open_time: 1_700_000_000,
    };
    let event = to_pool_creation_event(&init);
    // The neutral event keeps the mints, the creator and the deposits.
    let text = format!("{event:?}");
    assert!(text.contains(&init.token_0_mint.to_string()), "{text}");
    assert!(text.contains(&init.token_1_mint.to_string()), "{text}");
    assert!(text.contains(&init.creator.to_string()), "{text}");
    assert!(text.contains("1000"), "{text}");
}

fn idl_accounts_for(ix_name: &str) -> (Vec<Pubkey>, std::collections::HashMap<String, Pubkey>) {
    let accounts_json = idl()["instructions"]
        .as_array()
        .expect("instructions array")
        .iter()
        .find(|ix| ix["name"] == ix_name)
        .unwrap_or_else(|| panic!("missing instruction {ix_name}"))["accounts"]
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
fn initialize_and_permissioned_init_account_indices_match_vendored_idl() {
    let (accounts, by_name) = idl_accounts_for("initialize");
    let mut data = RAYDIUM_CPMM_DISC_INITIALIZE.to_vec();
    data.extend_from_slice(&10_000u64.to_le_bytes());
    data.extend_from_slice(&20_000u64.to_le_bytes());
    data.extend_from_slice(&1_700_000_000u64.to_le_bytes());
    let decoded = CpmmPoolInitEvent::from_instruction(&RAYDIUM_CPMM_PROGRAM_ID, &accounts, &data)
        .unwrap()
        .expect("CPMM initialize decodes");
    assert_eq!(decoded.creator, by_name["creator"]);
    assert_eq!(decoded.amm_config, by_name["amm_config"]);
    assert_eq!(decoded.pool_state, by_name["pool_state"]);
    assert_eq!(decoded.token_0_mint, by_name["token_0_mint"]);
    assert_eq!(decoded.token_1_mint, by_name["token_1_mint"]);
    assert_eq!(decoded.lp_mint, by_name["lp_mint"]);
    assert_eq!(decoded.token_0_vault, by_name["token_0_vault"]);
    assert_eq!(decoded.token_1_vault, by_name["token_1_vault"]);
    assert_eq!(decoded.observation_state, by_name["observation_state"]);

    let (perm_accounts, perm_by_name) = idl_accounts_for("initialize_with_permission");
    let mut perm_data = RAYDIUM_CPMM_DISC_INITIALIZE_WITH_PERMISSION.to_vec();
    perm_data.extend_from_slice(&30_000u64.to_le_bytes());
    perm_data.extend_from_slice(&40_000u64.to_le_bytes());
    perm_data.extend_from_slice(&1_700_000_500u64.to_le_bytes());
    perm_data.push(0);
    let perm_decoded =
        CpmmPoolInitEvent::from_instruction(&RAYDIUM_CPMM_PROGRAM_ID, &perm_accounts, &perm_data)
            .unwrap()
            .expect("CPMM initialize_with_permission decodes");
    assert_eq!(perm_decoded.creator, perm_by_name["creator"]);
    assert_eq!(perm_decoded.amm_config, perm_by_name["amm_config"]);
    assert_eq!(perm_decoded.pool_state, perm_by_name["pool_state"]);
    assert_eq!(perm_decoded.token_0_vault, perm_by_name["token_0_vault"]);
    assert_eq!(perm_decoded.token_1_vault, perm_by_name["token_1_vault"]);
    assert_eq!(
        perm_decoded.observation_state,
        perm_by_name["observation_state"]
    );
}

#[test]
fn swap_instruction_account_indices_match_vendored_idl() {
    for (ix_name, disc, expected_base_in) in [
        ("swap_base_input", RAYDIUM_CPMM_DISC_SWAP_BASE_INPUT, true),
        (
            "swap_base_output",
            RAYDIUM_CPMM_DISC_SWAP_BASE_OUTPUT,
            false,
        ),
    ] {
        let (accounts, by_name) = idl_accounts_for(ix_name);
        let mut data = disc.to_vec();
        data.extend_from_slice(&500_000u64.to_le_bytes());
        data.extend_from_slice(&250_000u64.to_le_bytes());
        let decoded =
            CpmmSwapInstruction::from_instruction(&RAYDIUM_CPMM_PROGRAM_ID, &accounts, &data)
                .unwrap()
                .expect("CPMM swap instruction decodes");
        assert_eq!(decoded.is_base_input, expected_base_in);
        assert_eq!(decoded.payer, by_name["payer"]);
        assert_eq!(decoded.amm_config, by_name["amm_config"]);
        assert_eq!(decoded.pool_state, by_name["pool_state"]);
        assert_eq!(decoded.input_vault, by_name["input_vault"]);
        assert_eq!(decoded.output_vault, by_name["output_vault"]);
        assert_eq!(decoded.input_mint, by_name["input_token_mint"]);
        assert_eq!(decoded.output_mint, by_name["output_token_mint"]);

        let swap = to_decoded_swap_from_instruction(
            &decoded,
            "sig-ix",
            99,
            Some(1_700_000_000),
            &by_name["output_token_mint"],
            6,
            9,
        )
        .unwrap();
        assert_eq!(swap.venue, SwapVenue::RaydiumCpmm);
        assert_eq!(swap.side, Side::Buy);
    }
}
