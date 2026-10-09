//! Layout-drift detector for the pump.fun bonding-curve program
//! (GAP-MAP P1 VERIFY: `crates/solana-kit/src/pump.rs`).
//!
//! ## Why this file exists
//!
//! A previous audit round flagged a contradiction inside `pump.rs`: the
//! module header documented **17** accounts for `buy` while the tests
//! asserted **16** (the official IDL shape). The resolution, kept in the
//! code comments, is: the IDL form is the default and the trailing-account
//! shapes that have been live in the wild (17 with `bonding_curve_v2`,
//! 18 with a trailing fee recipient, 17 with the fee recipient only) are
//! kept as explicit variants in [`solana_kit::pump::buy_layout_variants`] /
//! [`solana_kit::pump::sell_layout_variants`].
//!
//! This file closes the gap with executable evidence:
//!
//! 1. **Offline (always run)** — the variant table is exactly the shapes
//!    the module header documents, in the documented order.
//! 2. **Live (gated: `E2E_NETWORK=1` + `PUMP_DRIFT_MINT` + a MAINNET
//!    `E2E_URL`)** — for a real, still-on-the-curve pump.fun mint, every
//!    buy variant is simulated unsigned against the network and every sell
//!    variant is classified. The test prints which shape the program
//!    accepts today. It does NOT spend anything and broadcasts nothing:
//!    `simulateTransaction` only (`Rpc::simulate` forces `sig_verify=false`
//!    and `replace_recent_blockhash=true`, so a zero-hash, default-signature
//!    transaction can never land on chain). A sell probe holding zero
//!    tokens that fails with an insufficient-funds error is counted as
//!    *layout-correct* — the same classification
//!    `token_safety::classify_sell_probe` uses.
//!
//! CI never sets `E2E_NETWORK`, so the live half prints SKIP and the
//! offline half runs everywhere.

use solana_sdk::hash::Hash;
use solana_sdk::instruction::Instruction;
use solana_sdk::message::{Message, VersionedMessage};
use solana_sdk::pubkey::Pubkey;
use solana_sdk::signature::Signature;
use solana_sdk::transaction::VersionedTransaction;

use solana_kit::consts::{PUMP_DISC_BUY, PUMP_DISC_SELL, PUMP_PROGRAM_ID};
use solana_kit::pump::{self, buy_layout_variants, sell_layout_variants, PumpContext};
use solana_kit::rpc::Rpc;

fn gated(var: &str) -> bool {
    std::env::var(var).is_ok_and(|v| v == "1" || v.eq_ignore_ascii_case("true"))
}

/// Build the raw buy instruction for one layout variant, exactly the way
/// `pump::build_buy_ix` does, but pinned to the variant under test.
fn buy_ix_for_variant(ctx: &PumpContext, variant_idx: usize) -> Instruction {
    let layout = buy_layout_variants()
        .into_iter()
        .nth(variant_idx)
        .expect("variant index in range");
    let accounts = layout
        .build(&ctx.named_accounts())
        .expect("all variant slot names resolve against PumpContext");
    let mut data = Vec::with_capacity(25);
    data.extend_from_slice(&PUMP_DISC_BUY);
    data.extend_from_slice(&1u64.to_le_bytes()); // buy exactly 1 token
    data.extend_from_slice(&10_000_000_000u64.to_le_bytes()); // generous 10 SOL cap
    data.push(0u8); // track_volume = false
    Instruction {
        program_id: *PUMP_PROGRAM_ID,
        accounts,
        data,
    }
}

fn sell_ix_for_variant(ctx: &PumpContext, variant_idx: usize) -> Instruction {
    let layout = sell_layout_variants()
        .into_iter()
        .nth(variant_idx)
        .expect("variant index in range");
    let accounts = layout
        .build(&ctx.named_accounts())
        .expect("all variant slot names resolve against PumpContext");
    let mut data = Vec::with_capacity(24);
    data.extend_from_slice(&PUMP_DISC_SELL);
    data.extend_from_slice(&1u64.to_le_bytes()); // sell 1 token (we hold none)
    data.extend_from_slice(&1u64.to_le_bytes()); // min output 1 lamport
    Instruction {
        program_id: *PUMP_PROGRAM_ID,
        accounts,
        data,
    }
}

/// Unsigned simulate-only transaction: default signature + zero blockhash,
/// exactly the `market.rs::simulate_probe` pattern. `Rpc::simulate` never
/// verifies the signature and replaces the blockhash, so this can never
/// land on chain.
fn unsigned_tx(ix: Instruction, payer: &Pubkey) -> VersionedTransaction {
    let message =
        Message::new_with_blockhash(std::slice::from_ref(&ix), Some(payer), &Hash::default());
    VersionedTransaction {
        signatures: vec![Signature::default()],
        message: VersionedMessage::Legacy(message),
    }
}

/// `true` when the simulated instruction failed only because the prober
/// holds no tokens — proof the layout and sell path are sound.
fn is_insufficient_funds(err: &serde_json::Value) -> bool {
    let s = err.to_string().to_lowercase();
    s.contains("custom program error: 0x1") || s.contains("insufficient funds")
}

#[test]
fn offline_variant_table_matches_the_documented_shapes() {
    // buy: base IDL (16), +bonding_curve_v2 (17), +v2+trailing fee (18),
    // +trailing fee only (17) — exactly the documented history.
    let buy: Vec<usize> = buy_layout_variants().iter().map(|l| l.len()).collect();
    assert_eq!(buy, vec![16, 17, 18, 17], "buy variant table drifted");

    // sell: base IDL (14) with the same three trailing shapes.
    let sell: Vec<usize> = sell_layout_variants().iter().map(|l| l.len()).collect();
    assert_eq!(sell, vec![14, 15, 16, 15], "sell variant table drifted");

    // The first variant of each table must be the IDL default itself.
    assert_eq!(
        buy_layout_variants()[0].len(),
        pump::default_buy_layout().len()
    );
    assert_eq!(
        sell_layout_variants()[0].len(),
        pump::default_sell_layout().len()
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn live_layout_probe_detects_which_shape_the_program_accepts() {
    if !gated("E2E_NETWORK") {
        eprintln!("SKIP live_layout_probe: set E2E_NETWORK=1 PUMP_DRIFT_MINT=<curve-mint> [E2E_URL=<mainnet-rpc>] to run");
        return;
    }
    let mint_raw = match std::env::var("PUMP_DRIFT_MINT") {
        Ok(v) => v,
        Err(_) => {
            eprintln!("SKIP live_layout_probe: PUMP_DRIFT_MINT not set (needs a mint still on the bonding curve)");
            return;
        }
    };
    let mint: Pubkey = mint_raw
        .trim()
        .parse()
        .unwrap_or_else(|e| panic!("PUMP_DRIFT_MINT is not a pubkey: {e}"));

    // The pump.fun program is MAINNET-only: an explicit mainnet RPC URL is
    // mandatory here (the default NetworkConfig cluster cannot serve it).
    let url = match std::env::var("E2E_URL") {
        Ok(u) if !u.trim().is_empty() => u,
        _ => {
            eprintln!("SKIP live_layout_probe: E2E_URL must point at a MAINNET RPC (pump.fun is not on devnet)");
            return;
        }
    };
    let mut net = bot_core::config::NetworkConfig::default();
    net.rpc_url = url;
    let rpc = Rpc::new(&net).expect("rpc pool builds from NetworkConfig");
    let prober = Pubkey::new_unique(); // holds nothing; simulate only

    let ctx = PumpContext::load(&rpc, &mint, &prober, None)
        .await
        .unwrap_or_else(|e| panic!("PumpContext::load({mint}): {e}"));

    // ---- buy variants -------------------------------------------------
    let variants = buy_layout_variants();
    let mut buy_winners: Vec<usize> = Vec::new();
    for (idx, variant) in variants.iter().enumerate() {
        let tx = unsigned_tx(buy_ix_for_variant(&ctx, idx), &prober);
        match rpc.simulate(&tx).await {
            Ok(resp) => match resp.value.err.as_ref() {
                None => {
                    buy_winners.push(idx);
                    eprintln!(
                        "BUY variant[{idx}] ({} accounts): SIMULATED OK",
                        variant.len()
                    );
                }
                Some(err) => {
                    eprintln!(
                        "BUY variant[{idx}] ({} accounts): rejected err={:?} logs={:?}",
                        variant.len(),
                        err,
                        resp.value
                            .logs
                            .as_ref()
                            .map(|l| l.iter().rev().take(5).collect::<Vec<_>>())
                    );
                    // Conservative: only an outright simulation success
                    // counts as a winner.
                }
            },
            Err(e) => {
                eprintln!("BUY variant[{idx}]: rpc error: {e}");
            }
        }
    }
    assert!(
        !buy_winners.is_empty(),
        "no buy layout variant simulated clean against {mint} — the program \
         has drifted past every known shape; extend buy_layout_variants()"
    );
    if !buy_winners.contains(&0) {
        eprintln!(
            "DRIFT DETECTED: the 16-account IDL default no longer simulates \
             clean; accepted variant indexes = {buy_winners:?}"
        );
    } else {
        eprintln!(
            "buy layout stable: the 16-account IDL default is accepted (winners: {buy_winners:?})"
        );
    }

    // ---- sell variants ------------------------------------------------
    // The prober holds zero tokens: "insufficient funds" is a PASS for the
    // layout (identical to token_safety::classify_sell_probe). Anything
    // else is recorded for the operator.
    let sell_variants = sell_layout_variants();
    let mut sell_layout_ok: Vec<usize> = Vec::new();
    for (idx, variant) in sell_variants.iter().enumerate() {
        let tx = unsigned_tx(sell_ix_for_variant(&ctx, idx), &prober);
        match rpc.simulate(&tx).await {
            Ok(resp) => match resp.value.err.as_ref() {
                None => {
                    sell_layout_ok.push(idx);
                    eprintln!("SELL variant[{idx}] ({} accounts): simulated clean", variant.len());
                }
                Some(err) => match serde_json::to_value(err) {
                    Ok(v) if is_insufficient_funds(&v) => {
                        sell_layout_ok.push(idx);
                        eprintln!(
                            "SELL variant[{idx}] ({} accounts): layout sound (insufficient funds, expected for a zero-balance prober)",
                            variant.len()
                        );
                    }
                    Ok(v) => eprintln!("SELL variant[{idx}]: rejected err={v}"),
                    Err(_) => eprintln!("SELL variant[{idx}]: rejected err={err:?}"),
                },
            },
            Err(e) => eprintln!("SELL variant[{idx}]: rpc error: {e}"),
        }
    }
    assert!(
        !sell_layout_ok.is_empty(),
        "no sell layout variant produced a sound outcome against {mint}"
    );
}
