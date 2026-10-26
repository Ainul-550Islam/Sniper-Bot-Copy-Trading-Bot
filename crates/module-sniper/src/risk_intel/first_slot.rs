//! First-slot buy producer (PART 03 K1): the real data behind
//! `LaunchEvent::first_slot_buys`.
//!
//! [`crate::gates`]' `bundler_detection` gate and
//! [`crate::risk_intel::bundler::analyze`] consume a list of first-slot buys
//! attributed to a launch event. Until now that list was only ever
//! `Vec::new()`, so an enabled `sniper.max_bundler_ratio` gate had nothing to
//! measure and (correctly) failed closed. This module is the one producer of
//! that list; nothing else in the workspace fills it.
//!
//! # What "first slot" means here
//! The creating transaction occupies one slot. Every transaction in that SAME
//! slot that buys this mint on the launch venue is a first-slot buy: that is
//! the window a bundle occupies, because a bundle is built to land in the
//! creation slot. Buys in later slots are ordinary snipers and are not this
//! signal.
//!
//! # Attribution — `bundler.rs`' definition, not a new one
//! `bundler.rs` defines one first-slot buy as `(buyer, funder)` where the
//! funder is "the wallet that FUNDED it (the fee payer / SOL source)".
//! Therefore:
//!
//! * `funder` = the transaction's fee payer = `static_account_keys()[0]` of
//!   the message that carried the buy. In Solana's message layout key 0 *is*
//!   the fee payer, so this is a read, not an inference;
//! * `buyer` = the `user` account of the buy instruction, taken from the
//!   venue's own default account layout ([`solana_kit::pump`] /
//!   [`solana_kit::pumpswap`]). A bundler therefore shows up as N distinct
//!   buyers behind 1 funder — exactly the cluster `analyze()` counts.
//!
//! No second decoder is written here: instruction bodies come from
//! [`solana_kit::decode::decode_instructions`] (which already understands both
//! transaction encodings) and the discriminators from [`solana_kit::consts`].
//!
//! # Fail-closed contract
//! Nothing is ever guessed:
//!
//! * gate disabled (`sniper.max_bundler_ratio <= 0`) → **no RPC call at all**,
//!   so the default deployment pays zero cost and behaves exactly as before;
//! * no slot resolvable, a node error, a timeout, an undecodable transaction,
//!   or a transaction whose meta says it failed → the event's list stays
//!   **empty**, which is the gate's existing "no bundler signal" refusal;
//! * a launch on a venue whose swap instruction this module does not decode
//!   (Raydium AMM v4/CLMM/CPMM, Jupiter-routed buys) contributes no entries.
//!   That is reported as *unknown* (empty list → refusal), never as *clean*.
//!
//! # Known limit of the signal
//! Only *top-level* instructions are attributed (the same granularity
//! `decode_instructions` offers). A buy wrapped inside another program is not
//! visible here, so clustering can be under-reported — it can never be
//! invented. That is the honest ceiling of a first-order on-chain clustering
//! signal, and `max_bundler_ratio`' documentation says the threshold, not
//! this module, is what turns it into a rejection.
//!
//! # Cost and latency
//! With the gate on, one launch costs a `getBlock` (plus one `getTransaction`
//! when the feed reported a signature but no slot, i.e. PumpPortal). Both are
//! bounded by the RPC client's own configured timeout
//! ([`solana_kit::rpc::Rpc::timeout`]), so a stalling provider cannot silently
//! push the entry path past `sniper.max_entry_latency_ms`: a launch we could
//! not check in time is refused by the gate, which is exactly what an enabled
//! risk gate with no data means.

use solana_sdk::pubkey::Pubkey;
use solana_sdk::signature::Signature;
use tracing::debug;

use bot_core::config::SniperConfig;
use solana_kit::consts::{
    PUMPSWAP_DISC_BUY, PUMPSWAP_DISC_BUY_EXACT_QUOTE_IN, PUMPSWAP_DISC_SELL,
    PUMPSWAP_DISC_SELL_EXACT_BASE_IN, PUMPSWAP_PROGRAM_ID, PUMP_DISC_BUY,
    PUMP_DISC_BUY_EXACT_SOL_IN, PUMP_DISC_BUY_V2, PUMP_DISC_SELL, PUMP_DISC_SELL_V2,
    PUMP_PROGRAM_ID,
};
use solana_kit::decode::{decode_instructions, DecodedInstruction};
use solana_kit::layout::{AccountLayout, Slot};
use solana_kit::rpc::Rpc;

use crate::event::LaunchEvent;
use crate::risk_intel::bundler::FirstSlotBuy;

/// Length of an Anchor instruction discriminator: the first 8 bytes of
/// `sha256("global:<name>")`, the same convention as the `*_DISC_*` constants
/// in [`solana_kit::consts`].
const DISCRIMINATOR_LEN: usize = 8;

/// The account-layout positions a swap instruction needs. Both indices are
/// looked up by NAME in the venue's own default layout, so a pump.fun
/// account-list change is absorbed by the `layout` module (which exists for
/// exactly that) instead of by a second hardcoded index here.
#[derive(Debug, Clone, Copy)]
struct SwapShape {
    layout: fn() -> AccountLayout,
    mint_slot: &'static str,
    user_slot: &'static str,
}

impl SwapShape {
    /// The bonding-curve v1 account list, per side.
    fn pump(buy: bool) -> Self {
        Self {
            mint_slot: "mint",
            user_slot: "user",
            layout: if buy {
                solana_kit::pump::default_buy_layout
            } else {
                solana_kit::pump::default_sell_layout
            },
        }
    }

    /// The multi-quote v2 account list, per side.
    fn pump_v2(buy: bool) -> Self {
        Self {
            mint_slot: "base_mint",
            user_slot: "user",
            layout: if buy {
                solana_kit::pump::default_buy_v2_layout
            } else {
                solana_kit::pump::default_sell_v2_layout
            },
        }
    }

    /// The PumpSwap account list, per side. Both buy kinds (`buy` and
    /// `buy_exact_quote_in`) and both sell kinds share one account list.
    fn pumpswap(buy: bool) -> Self {
        Self {
            mint_slot: "base_mint",
            user_slot: "user",
            layout: if buy {
                solana_kit::pumpswap::default_buy_layout
            } else {
                solana_kit::pumpswap::default_sell_layout
            },
        }
    }

    /// Resolve `(mint, user)` from the layout positions.
    fn resolve(&self, ix: &DecodedInstruction) -> Option<(Pubkey, Pubkey)> {
        let layout = (self.layout)();
        let mint_at = slot_index(&layout, self.mint_slot)?;
        let user_at = slot_index(&layout, self.user_slot)?;
        if ix.accounts.len() <= mint_at.max(user_at) {
            // Fewer accounts than the layout we know: we cannot say who traded
            // what, so this instruction is not evidence either way.
            return None;
        }
        Some((ix.accounts[mint_at], ix.accounts[user_at]))
    }
}

/// Which swap a decoded instruction performed.
///
/// Crate-internal on purpose: [`crate::risk_intel::dev_sell`] reads the very
/// same instruction shapes for sells, so the venue classification has ONE
/// owner instead of two decoders.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct VenueSwap {
    /// `true` for a buy, `false` for a sell.
    pub is_buy: bool,
    /// The trader the venue's account list attributes the swap to.
    pub user: Pubkey,
    /// The base mint swapped.
    pub mint: Pubkey,
}

/// `true` when the operator switched the bundler gate on.
///
/// Mirrors the gate's own enable test (`sniper.max_bundler_ratio` is a
/// positive finite fraction) and is the only place the producer decides
/// whether to spend RPC calls.
pub fn enabled(cfg: &SniperConfig) -> bool {
    cfg.max_bundler_ratio.is_finite() && cfg.max_bundler_ratio > 0.0
}

/// The swap shape and side of one instruction, or `None` when it is not a
/// swap of the two launch venues.
fn shape_for(ix: &DecodedInstruction, disc: [u8; DISCRIMINATOR_LEN]) -> Option<(SwapShape, bool)> {
    if ix.program_id == *PUMP_PROGRAM_ID {
        if disc == PUMP_DISC_BUY || disc == PUMP_DISC_BUY_EXACT_SOL_IN {
            Some((SwapShape::pump(true), true))
        } else if disc == PUMP_DISC_BUY_V2 {
            Some((SwapShape::pump_v2(true), true))
        } else if disc == PUMP_DISC_SELL {
            Some((SwapShape::pump(false), false))
        } else if disc == PUMP_DISC_SELL_V2 {
            Some((SwapShape::pump_v2(false), false))
        } else {
            None
        }
    } else if ix.program_id == *PUMPSWAP_PROGRAM_ID {
        if disc == PUMPSWAP_DISC_BUY || disc == PUMPSWAP_DISC_BUY_EXACT_QUOTE_IN {
            Some((SwapShape::pumpswap(true), true))
        } else if disc == PUMPSWAP_DISC_SELL || disc == PUMPSWAP_DISC_SELL_EXACT_BASE_IN {
            Some((SwapShape::pumpswap(false), false))
        } else {
            None
        }
    } else {
        None
    }
}

/// Classify one decoded instruction as a pump.fun or PumpSwap swap.
///
/// `None` for anything else: another program, a discriminator we do not
/// model, or instruction data too short to carry a discriminator.
pub(crate) fn classify_swap(ix: &DecodedInstruction) -> Option<VenueSwap> {
    let data = &ix.data;
    if data.len() < DISCRIMINATOR_LEN {
        return None;
    }
    let disc: [u8; DISCRIMINATOR_LEN] = data[..DISCRIMINATOR_LEN].try_into().ok()?;
    let (shape, is_buy) = shape_for(ix, disc)?;
    let (mint, user) = shape.resolve(ix)?;
    Some(VenueSwap { is_buy, user, mint })
}

/// Index of a named slot in an account layout.
///
/// Only `Named` slots count: a `Fixed` slot is a learned trailing addition and
/// its `name()` is the pubkey string, never a layout account name.
fn slot_index(layout: &AccountLayout, want: &str) -> Option<usize> {
    layout
        .slots
        .iter()
        .position(|slot| matches!(slot, Slot::Named { name, .. } if name == want))
}

/// Reduce one transaction's instructions to attributed first-slot buys.
///
/// Pure: `funder` is whatever the caller read off the message, never derived
/// from the buyer. Duplicate buyers are left in place — `bundler::analyze`
/// deduplicates by buyer, which is the single owner of that rule.
pub fn buys_of_instructions(
    mint: &Pubkey,
    instructions: &[DecodedInstruction],
    funder: Pubkey,
) -> Vec<FirstSlotBuy> {
    let mut out = Vec::new();
    for ix in instructions {
        let Some(swap) = classify_swap(ix) else {
            continue;
        };
        if swap.is_buy && swap.mint == *mint {
            out.push(FirstSlotBuy {
                buyer: swap.user,
                funder,
            });
        }
    }
    out
}

/// The single wiring point used by the detection feeds (see `crate::detect`).
///
/// Leaves `event.first_slot_buys` untouched when the gate is off, when the
/// launch source already supplied attributions, or when the chain data could
/// not be read; fills it with exactly what was observed otherwise. Returns the
/// number of attributed buys on the event afterwards.
pub async fn attach_if_enabled(rpc: &Rpc, cfg: &SniperConfig, event: &mut LaunchEvent) -> usize {
    if !enabled(cfg) {
        // Gate off: not one RPC request for this signal.
        return 0;
    }
    if !event.first_slot_buys.is_empty() {
        // The source itself attributed the buys — that observation wins; a
        // second read would only add cost and a second owner of the truth.
        return event.first_slot_buys.len();
    }
    let Some(mint) = event.mint_pubkey() else {
        debug!(mint = %event.mint, "first-slot fetch skipped: mint is not a pubkey");
        count("skipped");
        return 0;
    };
    let Some(slot) = launch_slot(rpc, event).await else {
        debug!(
            mint = %event.mint,
            "first-slot fetch skipped: no slot and no readable launch signature"
        );
        count("skipped");
        return 0;
    };
    match fetch(rpc, &mint, slot).await {
        Some(buys) => {
            let n = buys.len();
            event.first_slot_buys = buys;
            debug!(mint = %event.mint, slot, buys = n, "first-slot buys attributed");
            n
        }
        None => 0,
    }
}

/// Fetch one slot and reduce it to this mint's first-slot buys.
///
/// `None` = the datum is unavailable (node error or timeout), which the caller
/// must report as *unknown*. `Some(vec![])` means the block was read and held
/// no buy of this mint — a different, and equally honest, statement.
pub async fn fetch(rpc: &Rpc, mint: &Pubkey, slot: u64) -> Option<Vec<FirstSlotBuy>> {
    let budget = rpc.timeout();
    let block = match tokio::time::timeout(budget, rpc.get_block(slot)).await {
        Err(_) => {
            debug!(
                %mint,
                slot,
                timeout_ms = budget.as_millis() as u64,
                "first-slot fetch timed out; the bundler gate stays without a signal"
            );
            count("timeout");
            return None;
        }
        Ok(Err(error)) => {
            debug!(%mint, slot, %error, "first-slot fetch failed; the bundler gate stays without a signal");
            count("error");
            return None;
        }
        Ok(Ok(block)) => block,
    };

    let mut buys: Vec<FirstSlotBuy> = Vec::new();
    // The transaction/meta model lives in `solana-transaction-status`, which
    // this crate deliberately does not depend on: every access below goes
    // through inferred field/method access and the `solana-kit` decoders, so
    // no type of that crate is named here.
    for tx in block.transactions.as_ref().into_iter().flatten() {
        // A failed transaction bought nothing.
        let Some(meta) = tx.meta.as_ref() else {
            continue;
        };
        if meta.err.is_some() {
            continue;
        }
        // The fee payer — the coordination key. No decodable message, no
        // attribution (never a fallback guess).
        let Some(decoded) = tx.transaction.decode() else {
            continue;
        };
        let Some(funder) = decoded.message.static_account_keys().first().copied() else {
            continue;
        };
        let Ok(instructions) = decode_instructions(&tx.transaction, meta) else {
            debug!("first-slot fetch: a block transaction could not be decoded; skipping it");
            continue;
        };
        buys.extend(buys_of_instructions(mint, &instructions, funder));
    }
    count(if buys.is_empty() { "empty" } else { "ok" });
    Some(buys)
}

/// The launch slot to scan: the one the feed reported, else the slot of the
/// launch transaction (PumpPortal reports a signature but no slot).
async fn launch_slot(rpc: &Rpc, event: &LaunchEvent) -> Option<u64> {
    if let Some(slot) = event.slot {
        if slot > 0 {
            return Some(slot);
        }
    }
    let signature = event
        .signature
        .as_deref()
        .and_then(|s| s.parse::<Signature>().ok())?;
    let budget = rpc.timeout();
    match tokio::time::timeout(budget, rpc.get_transaction(&signature)).await {
        Ok(Ok(Some(confirmed))) => (confirmed.slot > 0).then_some(confirmed.slot),
        Ok(Ok(None)) => None,
        Ok(Err(error)) => {
            debug!(%error, "launch transaction read failed; no slot to scan for first-slot buys");
            count("error");
            None
        }
        Err(_) => {
            count("timeout");
            None
        }
    }
}

/// Low-cardinality telemetry for the producer, so an operator can tell
/// "the gate has data" from "the gate is refusing because the read failed"
/// without debug logs.
fn count(outcome: &'static str) {
    bot_core::obs::metrics::global()
        .counter(
            "sniper_first_slot_fetch_total",
            "First-slot buy fetches by outcome (ok/empty/error/timeout/skipped).",
            &[("outcome", outcome)],
        )
        .inc();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg_with(ratio: f64) -> SniperConfig {
        let mut cfg = bot_core::config::AppConfig::from_defaults().raw.sniper;
        cfg.max_bundler_ratio = ratio;
        cfg
    }

    /// An instruction whose accounts follow `layout`, with `mint`/`user`
    /// placed at the names the shape looks up.
    fn swap_ix(
        program: Pubkey,
        layout: &AccountLayout,
        mint: Pubkey,
        user: Pubkey,
        data: Vec<u8>,
    ) -> DecodedInstruction {
        let accounts: Vec<Pubkey> = layout
            .slots
            .iter()
            .map(|slot| match slot.name() {
                "mint" | "base_mint" => mint,
                "user" => user,
                _ => Pubkey::new_unique(),
            })
            .collect();
        DecodedInstruction {
            program_id: program,
            accounts,
            data,
        }
    }

    fn buy_data(disc: [u8; 8]) -> Vec<u8> {
        let mut data = disc.to_vec();
        data.extend_from_slice(&1_000u64.to_le_bytes());
        data.extend_from_slice(&900u64.to_le_bytes());
        data.push(0);
        data
    }

    #[test]
    fn the_producer_is_gated_by_the_same_switch_as_the_gate() {
        assert!(!enabled(&cfg_with(0.0)), "off by default: zero RPC cost");
        assert!(!enabled(&cfg_with(-1.0)));
        assert!(!enabled(&cfg_with(f64::NAN)));
        assert!(enabled(&cfg_with(0.5)));
        assert!(enabled(&cfg_with(1.0)));
    }

    #[test]
    fn a_pump_buy_is_classified_from_the_venues_own_layout() {
        let mint = Pubkey::new_from_array([7; 32]);
        let user = Pubkey::new_from_array([3; 32]);
        let layout = solana_kit::pump::default_buy_layout();
        let ix = swap_ix(
            *PUMP_PROGRAM_ID,
            &layout,
            mint,
            user,
            buy_data(PUMP_DISC_BUY),
        );
        assert_eq!(
            classify_swap(&ix),
            Some(VenueSwap {
                is_buy: true,
                user,
                mint
            })
        );
    }

    #[test]
    fn a_pumpswap_sell_is_classified_as_a_sell_of_its_base_mint() {
        let mint = Pubkey::new_from_array([11; 32]);
        let user = Pubkey::new_from_array([12; 32]);
        let layout = solana_kit::pumpswap::default_sell_layout();
        let ix = swap_ix(
            *PUMPSWAP_PROGRAM_ID,
            &layout,
            mint,
            user,
            buy_data(PUMPSWAP_DISC_SELL),
        );
        assert_eq!(
            classify_swap(&ix),
            Some(VenueSwap {
                is_buy: false,
                user,
                mint
            })
        );
    }

    #[test]
    fn an_unmodelled_program_or_discriminator_is_not_a_swap() {
        let layout = solana_kit::pump::default_buy_layout();
        let mint = Pubkey::new_unique();
        let user = Pubkey::new_unique();
        let foreign = swap_ix(
            Pubkey::new_from_array([42; 32]),
            &layout,
            mint,
            user,
            buy_data(PUMP_DISC_BUY),
        );
        assert_eq!(classify_swap(&foreign), None, "wrong program");

        let unknown_disc = swap_ix(*PUMP_PROGRAM_ID, &layout, mint, user, vec![9; 8]);
        assert_eq!(classify_swap(&unknown_disc), None, "unknown discriminator");

        let short = swap_ix(*PUMP_PROGRAM_ID, &layout, mint, user, vec![1, 2, 3]);
        assert_eq!(
            classify_swap(&short),
            None,
            "data shorter than a discriminator"
        );
    }

    #[test]
    fn a_truncated_account_list_is_not_attributed() {
        let mut data = PUMPSWAP_DISC_SELL.to_vec();
        data.extend_from_slice(&1u64.to_le_bytes());
        let ix = DecodedInstruction {
            program_id: *PUMPSWAP_PROGRAM_ID,
            accounts: vec![Pubkey::new_unique(); 2],
            data,
        };
        assert_eq!(classify_swap(&ix), None);
    }

    #[test]
    fn only_buys_of_the_requested_mint_are_attributed() {
        let mint = Pubkey::new_from_array([5; 32]);
        let other = Pubkey::new_from_array([6; 32]);
        let funder = Pubkey::new_from_array([9; 32]);
        let layout = solana_kit::pump::default_buy_layout();
        let ours = swap_ix(
            *PUMP_PROGRAM_ID,
            &layout,
            mint,
            Pubkey::new_from_array([1; 32]),
            buy_data(PUMP_DISC_BUY),
        );
        let theirs = swap_ix(
            *PUMP_PROGRAM_ID,
            &layout,
            other,
            Pubkey::new_from_array([2; 32]),
            buy_data(PUMP_DISC_BUY),
        );
        let sell = swap_ix(
            *PUMP_PROGRAM_ID,
            &layout,
            mint,
            Pubkey::new_from_array([1; 32]),
            buy_data(PUMP_DISC_SELL),
        );
        let buys = buys_of_instructions(&mint, &[theirs, sell, ours], funder);
        assert_eq!(
            buys,
            vec![FirstSlotBuy {
                buyer: Pubkey::new_from_array([1; 32]),
                funder
            }],
            "a foreign mint and a sell of ours are both not first-slot buys"
        );
    }

    #[test]
    fn every_buy_of_one_funder_keeps_that_funder() {
        // A bundle: two buys, same fee payer, different buyers.
        let mint = Pubkey::new_from_array([5; 32]);
        let funder = Pubkey::new_from_array([9; 32]);
        let layout = solana_kit::pump::default_buy_layout();
        let a = swap_ix(
            *PUMP_PROGRAM_ID,
            &layout,
            mint,
            Pubkey::new_from_array([1; 32]),
            buy_data(PUMP_DISC_BUY),
        );
        let b = swap_ix(
            *PUMP_PROGRAM_ID,
            &layout,
            mint,
            Pubkey::new_from_array([2; 32]),
            buy_data(PUMP_DISC_BUY_EXACT_SOL_IN),
        );
        let buys = buys_of_instructions(&mint, &[a, b], funder);
        assert_eq!(buys.len(), 2);
        assert!(buys.iter().all(|buy| buy.funder == funder));
        let report = crate::risk_intel::bundler::analyze(&buys);
        assert_eq!(report.clustered_buyers, 2);
        assert_eq!(report.bundler_ratio_bps, crate::risk_intel::bundler::BPS);
    }

    #[test]
    fn layout_names_resolve_to_the_documented_positions() {
        let buy = solana_kit::pump::default_buy_layout();
        assert_eq!(slot_index(&buy, "mint"), Some(2));
        assert_eq!(slot_index(&buy, "user"), Some(6));
        let sell = solana_kit::pump::default_sell_layout();
        assert_eq!(slot_index(&sell, "mint"), Some(2));
        assert_eq!(slot_index(&sell, "user"), Some(6));
        let swap_buy = solana_kit::pumpswap::default_buy_layout();
        assert_eq!(slot_index(&swap_buy, "base_mint"), Some(3));
        assert_eq!(slot_index(&swap_buy, "user"), Some(1));
        let swap_sell = solana_kit::pumpswap::default_sell_layout();
        assert_eq!(slot_index(&swap_sell, "base_mint"), Some(3));
        assert_eq!(slot_index(&swap_sell, "user"), Some(1));
        let v2 = solana_kit::pump::default_buy_v2_layout();
        assert_eq!(slot_index(&v2, "base_mint"), Some(1));
        assert_eq!(slot_index(&v2, "user"), Some(13));
        // `sell_v2` drops a trailing account only: both positions we need are
        // identical, which is what makes reusing the buy layout safe.
        let v2_sell = solana_kit::pump::default_sell_v2_layout();
        assert_eq!(slot_index(&v2_sell, "base_mint"), Some(1));
        assert_eq!(slot_index(&v2_sell, "user"), Some(13));
        assert_eq!(slot_index(&buy, "no-such-account"), None);
    }
}
