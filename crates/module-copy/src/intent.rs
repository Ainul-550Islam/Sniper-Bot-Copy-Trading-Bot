//! Execution-intent identity for mirrored trades (TASK 3 §07).
//!
//! Every transaction the copy engine hands to the execution engine carries a
//! **deterministic intent id** so the process-wide `ExecutionLedger` can
//! refuse a second attempt at the same logical trade — a replayed feed
//! event, a second replica, a retry after a crash while the first attempt
//! was live. The identity of a mirrored **entry** is the leader's
//! transaction: `copy | leader signature | leader | buy | route | mint`.
//! The identity of a mirrored **exit** is our position — id **and** mint
//! **and** open time — plus the exact sell (route, raw quantity sold,
//! quantity held when the decision was made): [`exit_intent_id`].
//!
//! * **Entry ids are unchanged.** The parts are the same ones `mirror.rs`
//!   used before TASK 3, so a leader trade mirrored by the old code and
//!   replayed into the new one maps onto the same ledger record.
//! * **Exit ids are hardened.** Position ids are process-local counters
//!   (`p-N`) that restart with the process; before TASK 3 two lives of the
//!   book could mint the same `p-7` and their exits would have collided in
//!   the ledger (the second sell refused as a duplicate). Mint + open time
//!   make the identity unique per position lifetime. `exit.rs` consumes
//!   these helpers for both the sweeper and the mirrored-exit path.
//!
//! This module also builds the **write-ahead journal record**
//! (`bot_core::db::repo::IntentRecord`) that `bot_core::recovery::with_intent`
//! persists before broadcast and links to the signature after.

use std::sync::Arc;

use bot_core::db::repo::IntentRecord;
use bot_core::models::Position;
use bot_core::state::Shared;
use serde::{Deserialize, Serialize};
use solana_kit::tokens::Wallet;

use crate::event::LeaderTradeEvent;

/// Which venue path a mirrored entry takes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EntryRoute {
    /// pump.fun bonding curve (token not graduated).
    Curve,
    /// Jupiter swap (graduated; routes across PumpSwap / Raydium / …).
    Jupiter,
}

impl EntryRoute {
    /// Intent-id part.
    pub fn as_str(&self) -> &'static str {
        match self {
            EntryRoute::Curve => "curve",
            EntryRoute::Jupiter => "jupiter",
        }
    }
}

/// Deterministic intent id of a mirrored entry (same parts as the pre-TASK-3
/// `mirror::copy_intent_id`).
pub fn entry_intent_id(leader: &str, signature: &str, mint: &str, route: EntryRoute) -> String {
    bot_core::execution::intent_id(&["copy", signature, leader, "buy", route.as_str(), mint])
}

/// [`entry_intent_id`] for an event.
pub fn entry_intent_id_for(event: &LeaderTradeEvent, route: EntryRoute) -> String {
    entry_intent_id(&event.leader, &event.signature, &event.mint, route)
}

/// Executor label of a mirrored entry: `copy-<mint8>` on the curve,
/// `copy-jup-<mint8>` via Jupiter (unchanged; the risk engine's pending cap
/// counts labels starting with `copy-` that are not `copy-exit`).
pub fn entry_label(mint: &str, route: EntryRoute) -> String {
    let short: String = mint.chars().take(8).collect();
    match route {
        EntryRoute::Curve => format!("copy-{short}"),
        EntryRoute::Jupiter => format!("copy-jup-{short}"),
    }
}

/// Whether an executor label belongs to a mirrored ENTRY.
pub fn is_entry_label(label: &str) -> bool {
    label.starts_with("copy-") && !label.starts_with("copy-exit")
}

/// Which venue path a copy exit takes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExitRoute {
    /// pump.fun bonding curve (token not graduated).
    Curve,
    /// Jupiter swap (graduated).
    Jupiter,
}

impl ExitRoute {
    /// Intent-id part (same vocabulary as [`EntryRoute`]).
    pub fn as_str(&self) -> &'static str {
        match self {
            ExitRoute::Curve => "curve",
            ExitRoute::Jupiter => "jupiter",
        }
    }
}

/// Deterministic execution-intent id for one exit decision (hardened, TASK
/// 3 §07). The parts are, in order: `copy`, the position id, the position's
/// **mint**, its **open time** (unix ms), `sell`, the route, the raw
/// quantity sold and the quantity held when the decision was made.
///
/// * a retry of the same exit maps onto the same ledger record (the ledger
///   refuses it as a duplicate — no double sell);
/// * the next partial exit, taken from a smaller position, gets a fresh id;
/// * two positions that share an id string (counters restart with the
///   process) never share an exit identity — mint + open time differ.
pub fn exit_intent_id(position: &Position, sell_raw: u64, route: ExitRoute) -> String {
    bot_core::execution::intent_id(&[
        "copy",
        &position.id,
        &position.symbol,
        &position.opened_at.timestamp_millis().to_string(),
        "sell",
        route.as_str(),
        &sell_raw.to_string(),
        &format!("{:.9}", position.qty),
    ])
}

/// Executor label of a copy exit: `copy-exit-<mint>` on the curve,
/// `copy-exit-jup-<mint>` via Jupiter (unchanged; the risk engine's pending
/// cap ignores labels starting with `copy-exit`).
pub fn exit_label(mint: &str, route: ExitRoute) -> String {
    match route {
        ExitRoute::Curve => format!("copy-exit-{mint}"),
        ExitRoute::Jupiter => format!("copy-exit-jup-{mint}"),
    }
}

/// Whether an executor label belongs to a copy EXIT.
pub fn is_exit_label(label: &str) -> bool {
    label.starts_with("copy-exit")
}

/// Fresh write-ahead intent record for one copy-trade broadcast (§I crash
/// point C). The journal id is process-unique (`intent-N`) on purpose — it
/// identifies the *attempt* in the durable journal, while the ledger intent
/// id identifies the *logical trade*.
pub fn journal_record(
    state: &Shared,
    wallet: &Arc<Wallet>,
    symbol: &str,
    side: &str,
    qty: &str,
) -> IntentRecord {
    IntentRecord {
        intent_id: state.next_id("intent"),
        module: "copy".into(),
        symbol: symbol.to_string(),
        wallet: wallet.pubkey.to_string(),
        side: side.into(),
        qty: qty.into(),
        status: "pending".into(),
        signature: None,
        created_at: chrono::Utc::now(),
    }
}

/// Descriptor of one mirrored-entry intent, for logs / audit / journal rows.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CopyIntent {
    /// Ledger intent id.
    pub intent_id: String,
    /// Executor label.
    pub label: String,
    /// Route taken.
    pub route: EntryRoute,
    /// Event that triggered it.
    pub event_id: String,
    /// Leader mirrored.
    pub leader: String,
    /// Token bought.
    pub mint: String,
    /// Lamports we spend.
    pub lamports_in: u64,
}

impl CopyIntent {
    /// Build the descriptor for `event` on `route`.
    pub fn for_entry(event: &LeaderTradeEvent, route: EntryRoute, lamports_in: u64) -> Self {
        CopyIntent {
            intent_id: entry_intent_id_for(event, route),
            label: entry_label(&event.mint, route),
            route,
            event_id: event.event_id.clone(),
            leader: event.leader.clone(),
            mint: event.mint.clone(),
            lamports_in,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::EventSource;
    use bot_core::models::{PositionSide, Venue, WalletTrade};
    use chrono::Utc;

    fn event() -> LeaderTradeEvent {
        LeaderTradeEvent::from_wallet_trade(
            &WalletTrade {
                wallet: "whale".into(),
                signature: "sig".into(),
                slot: 1,
                block_time: None,
                side: PositionSide::Long,
                mint: "MintAddress12345".into(),
                symbol: None,
                token_amount: 1.0,
                sol_amount: 0.5,
                venue: Venue::PumpFun,
                fee_sol: 0.0,
                discriminator: None,
                observed_at: Utc::now(),
            },
            EventSource::PumpPortal,
            1,
        )
    }

    #[test]
    fn intent_ids_are_deterministic_and_route_specific() {
        let e = event();
        let curve = entry_intent_id_for(&e, EntryRoute::Curve);
        let jup = entry_intent_id_for(&e, EntryRoute::Jupiter);
        assert!(curve.starts_with("int_"));
        assert_ne!(curve, jup);
        assert_eq!(
            curve,
            entry_intent_id("whale", "sig", "MintAddress12345", EntryRoute::Curve)
        );
        // Same parts as the legacy helper: `copy|sig|wallet|buy|route|mint`.
        assert_eq!(
            curve,
            bot_core::execution::intent_id(&[
                "copy",
                "sig",
                "whale",
                "buy",
                "curve",
                "MintAddress12345"
            ])
        );
        let mut other = e.clone();
        other.source = EventSource::LogsPoll;
        other.observed_at = Utc::now();
        other.slot = 999;
        assert_eq!(
            entry_intent_id_for(&other, EntryRoute::Curve),
            curve,
            "source, slot and observation time never change the identity"
        );
    }

    #[test]
    fn labels_follow_the_legacy_scheme() {
        assert_eq!(
            entry_label("MintAddress12345", EntryRoute::Curve),
            "copy-MintAddr"
        );
        assert_eq!(
            entry_label("MintAddress12345", EntryRoute::Jupiter),
            "copy-jup-MintAddr"
        );
        assert!(is_entry_label("copy-MintAddr"));
        assert!(is_entry_label("copy-jup-MintAddr"));
        assert!(!is_entry_label("copy-exit-p1"));
        assert!(!is_entry_label("snipe-x"));
        let d = CopyIntent::for_entry(&event(), EntryRoute::Curve, 5_000);
        assert_eq!(d.label, "copy-MintAddr");
        assert_eq!(d.lamports_in, 5_000);
        assert_eq!(d.leader, "whale");
        assert_eq!(exit_label("MintX", ExitRoute::Curve), "copy-exit-MintX");
        assert_eq!(
            exit_label("MintX", ExitRoute::Jupiter),
            "copy-exit-jup-MintX"
        );
        assert!(is_exit_label("copy-exit-MintX"));
        assert!(is_exit_label("copy-exit-jup-MintX"));
        assert!(!is_exit_label("copy-MintX"));
        assert!(!is_entry_label(&exit_label("MintX", ExitRoute::Curve)));
    }

    fn position(id: &str, mint: &str, qty: f64) -> Position {
        let mut p = Position::new(
            id.into(),
            bot_core::models::TradeSource::Copy,
            Venue::PumpFun,
            bot_core::models::ExecutionMode::Paper,
            mint.into(),
            "M".into(),
            "SOL".into(),
        );
        p.apply_buy(qty, 0.001, qty * 0.001);
        p
    }

    #[test]
    fn exit_ids_are_deterministic_per_decision_and_unique_per_position_lifetime() {
        let p = position("p-7", "MintA", 1_000.0);
        let a = exit_intent_id(&p, 500_000_000, ExitRoute::Curve);
        assert!(a.starts_with("int_"));
        // A retry of the same decision maps onto the same ledger record.
        assert_eq!(a, exit_intent_id(&p, 500_000_000, ExitRoute::Curve));
        // Exact parts, in order (mint + open time included).
        assert_eq!(
            a,
            bot_core::execution::intent_id(&[
                "copy",
                "p-7",
                "MintA",
                &p.opened_at.timestamp_millis().to_string(),
                "sell",
                "curve",
                "500000000",
                &format!("{:.9}", p.qty),
            ])
        );
        // Route, quantity sold and quantity held all change the identity.
        assert_ne!(a, exit_intent_id(&p, 500_000_000, ExitRoute::Jupiter));
        assert_ne!(a, exit_intent_id(&p, 250_000_000, ExitRoute::Curve));
        let mut smaller = p.clone();
        smaller.apply_sell(500.0, 0.001, 0.5);
        assert_ne!(
            a,
            exit_intent_id(&smaller, 500_000_000, ExitRoute::Curve),
            "the next partial exit gets a fresh id"
        );
        // Same id string, other mint → other position → other identity.
        assert_ne!(
            a,
            exit_intent_id(
                &position("p-7", "MintB", 1_000.0),
                500_000_000,
                ExitRoute::Curve
            )
        );
        // Same id string and mint, opened at another time → other identity.
        let mut later = position("p-7", "MintA", 1_000.0);
        later.opened_at = p.opened_at + chrono::Duration::milliseconds(1);
        assert_ne!(a, exit_intent_id(&later, 500_000_000, ExitRoute::Curve));
    }
}
