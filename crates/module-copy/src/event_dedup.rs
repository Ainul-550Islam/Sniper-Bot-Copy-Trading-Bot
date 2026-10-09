//! The one authoritative event dedup (TASK 3 §03).
//!
//! A leader trade can reach the pipeline more than once: two feeds decode
//! the same transaction, a websocket reconnect replays its backlog, a
//! restart re-polls recent signatures, or a token-for-token route yields
//! one event per mint. Exactly one mechanism decides whether an event is
//! new: [`claim`], which marks the event's dedup key
//! (`copy:{signature}:{leader}:{mint}:{side}`) in
//! [`bot_core::state::AppState::mark_copy_event_seen`]. That call routes
//! through the durable dedup facade (Redis / Postgres, namespace
//! `copy_event`) when one is attached and falls back to the bounded
//! in-memory set otherwise.
//!
//! What this module deliberately is **not**:
//!
//! * the feeds' `mark_signature_seen` — that is fetch suppression inside
//!   `feeds.rs` (don't decode the same signature twice) and shares no
//!   namespace with the pipeline. Before TASK 3 the pipeline re-marked the
//!   same `sig` key the feeds had already marked, so every event from the
//!   polling and Geyser feeds was dropped as a duplicate; the pipeline now
//!   never touches the `sig` namespace;
//! * the execution ledger's intent-id duplicate protection — that is the
//!   last line of defence at broadcast time and stays untouched;
//! * the cross-replica ownership claim (`copy:{wallet}:{mint}`) — that
//!   elects one executor among replicas that all saw the event; dedup is
//!   per event, ownership is per logical trade.
//!
//! Claiming happens once, early, and is never released: an event that was
//! decided (mirrored OR refused) is not reconsidered when redelivered. The
//! restart recovery re-seeds the keys of recently journaled events
//! ([`seed`]) so an in-memory deployment does not re-mirror after a crash.

use bot_core::state::Shared;

use crate::event::LeaderTradeEvent;
use crate::metrics;

/// Result of a dedup claim.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DedupOutcome {
    /// First time the pipeline sees this event — proceed.
    Fresh,
    /// Already decided — drop.
    Duplicate,
}

impl DedupOutcome {
    /// Metric label.
    pub fn as_str(&self) -> &'static str {
        match self {
            DedupOutcome::Fresh => "fresh",
            DedupOutcome::Duplicate => "duplicate",
        }
    }
}

/// Claim `event` in the authoritative dedup. `Fresh` exactly once per key,
/// on every replica that shares the durable facade; process-lifetime when
/// no durable backend is attached.
pub async fn claim(state: &Shared, event: &LeaderTradeEvent) -> DedupOutcome {
    let key = event.dedup_key();
    let outcome = if state.mark_copy_event_seen(&key).await {
        DedupOutcome::Fresh
    } else {
        DedupOutcome::Duplicate
    };
    metrics::count_dedup(outcome.as_str());
    outcome
}

/// Read-only check (never marks).
pub async fn already_seen(state: &Shared, event: &LeaderTradeEvent) -> bool {
    state.copy_event_seen(&event.dedup_key()).await
}

/// Pre-mark `keys` (restart recovery from the durable journal). Returns how
/// many were newly marked.
pub async fn seed<I, S>(state: &Shared, keys: I) -> usize
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let mut added = 0;
    for key in keys {
        if state.mark_copy_event_seen(key.as_ref()).await {
            added += 1;
            metrics::count_dedup("seeded");
        }
    }
    added
}

/// Rebuild a dedup key from journaled parts (the journal stores the
/// components, not the key).
pub fn key_from_parts(signature: &str, leader: &str, mint: &str, side: &str) -> String {
    format!(
        "copy:{}:{}:{}:{}",
        signature.trim(),
        leader.trim(),
        mint.trim(),
        side.trim()
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use bot_core::config::{AppConfig, Config};
    use bot_core::models::{PositionSide, Venue, WalletTrade};
    use bot_core::state::AppState;
    use chrono::Utc;

    fn state() -> Shared {
        AppState::new(AppConfig {
            raw: Config::default(),
            source_path: None,
            warnings: Vec::new(),
        })
    }

    fn event(sig: &str, mint: &str, side: PositionSide) -> LeaderTradeEvent {
        LeaderTradeEvent::from_wallet_trade(
            &WalletTrade {
                wallet: "whale".into(),
                signature: sig.into(),
                slot: 1,
                block_time: None,
                side,
                mint: mint.into(),
                symbol: None,
                token_amount: 1.0,
                sol_amount: 0.1,
                venue: Venue::PumpFun,
                fee_sol: 0.0,
                discriminator: None,
                observed_at: Utc::now(),
            },
            crate::event::EventSource::Manual,
            0,
        )
    }

    #[tokio::test]
    async fn claim_is_fresh_once_then_duplicate() {
        let st = state();
        let e = event("sig1", "mintA", PositionSide::Long);
        assert!(!already_seen(&st, &e).await);
        assert_eq!(claim(&st, &e).await, DedupOutcome::Fresh);
        assert_eq!(claim(&st, &e).await, DedupOutcome::Duplicate);
        assert!(already_seen(&st, &e).await);
        // Same signature, other mint / side → distinct events.
        assert_eq!(
            claim(&st, &event("sig1", "mintB", PositionSide::Long)).await,
            DedupOutcome::Fresh
        );
        assert_eq!(
            claim(&st, &event("sig1", "mintA", PositionSide::Short)).await,
            DedupOutcome::Fresh
        );
        assert_eq!(st.seen_copy_event_count().await, 3);
    }

    #[tokio::test]
    async fn pipeline_namespace_is_independent_of_feed_signature_marks() {
        let st = state();
        let e = event("sigX", "mint", PositionSide::Long);
        // A feed marked the raw signature (fetch suppression)…
        assert!(st.mark_signature_seen("sigX").await);
        // …which must not make the pipeline drop the event.
        assert_eq!(claim(&st, &e).await, DedupOutcome::Fresh);
        // And the pipeline's claim must not consume the feed namespace.
        assert!(!st.mark_signature_seen("sigX").await);
        assert!(st.mark_signature_seen("sigY").await);
    }

    #[tokio::test]
    async fn seed_marks_journaled_keys() {
        let st = state();
        let e = event("sigS", "mint", PositionSide::Long);
        let key = key_from_parts(&e.signature, &e.leader, &e.mint, "buy");
        assert_eq!(key, e.dedup_key());
        assert_eq!(seed(&st, [key.clone(), key.clone()]).await, 1);
        assert_eq!(claim(&st, &e).await, DedupOutcome::Duplicate);
    }
}
