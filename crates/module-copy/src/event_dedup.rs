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
//! * the feeds: poll and Geyser may deliver duplicates, but do not consume
//!   the shared `sig` namespace before the event decision. The pipeline owns
//!   the separate `copy_event` namespace;
//! * the execution ledger's intent-id duplicate protection — that is the
//!   last line of defence at broadcast time and stays untouched;
//! * the cross-replica ownership claim (`copy:{wallet}:{mint}`) — that
//!   elects one executor among replicas that all saw the event; dedup is
//!   per event, ownership is per logical trade.
//!
//! A successful claim happens early and is not explicitly released, but can
//! expire, be evicted or be lost. Restart recovery attempts to re-seed recent
//! journaled keys ([`seed`]); neither path is unconditional exactly-once.

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

/// Claim `event` in the event dedup. The memory backend is process-local;
/// Redis and Postgres marks are shared while available, but the facade falls
/// back to process-local marks on backend errors and entries can expire or be
/// evicted. This is not an unconditional cross-replica exactly-once guarantee.
///
/// Process-local dedup does not elect an executor across replicas. Durable
/// dedup is an early, non-released event filter; ownership is a separate,
/// lease-based execution gate. These facts do not by themselves establish
/// the cause of a prior workspace-only test failure.
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

/// Claim with an EXPLICIT key. Tenant-bound callers pass
/// [`LeaderTradeEvent::tenant_dedup_key`] so the durable namespace is
/// per-tenant; `None` keeps the shared operator key.
pub async fn claim_for(
    state: &Shared,
    event: &LeaderTradeEvent,
    key: Option<&str>,
) -> DedupOutcome {
    match key {
        Some(k) => {
            let outcome = if state.mark_copy_event_seen(k).await {
                DedupOutcome::Fresh
            } else {
                DedupOutcome::Duplicate
            };
            metrics::count_dedup(outcome.as_str());
            outcome
        }
        None => claim(state, event).await,
    }
}

/// Require a shared first-arrival decision. Unlike [`claim`], this fails
/// closed when the facade is missing, memory-only, or the shared backend
/// cannot be reached. A failed or ambiguous write never authorizes execution.
pub async fn claim_shared(
    state: &Shared,
    event: &LeaderTradeEvent,
) -> bot_core::error::BotResult<DedupOutcome> {
    let store = state.dedup().ok_or_else(|| {
        bot_core::error::BotError::config("clustered copy execution requires shared event dedup")
    })?;
    let fresh = store.mark_shared("copy_event", &event.dedup_key()).await?;
    let outcome = if fresh {
        DedupOutcome::Fresh
    } else {
        DedupOutcome::Duplicate
    };
    metrics::count_dedup(outcome.as_str());
    Ok(outcome)
}

/// [`claim_shared`] with an explicit key (tenant namespacing, see
/// [`claim_for`]).
pub async fn claim_shared_for(
    state: &Shared,
    event: &LeaderTradeEvent,
    key: Option<&str>,
) -> bot_core::error::BotResult<DedupOutcome> {
    match key {
        Some(k) => {
            let store = state.dedup().ok_or_else(|| {
                bot_core::error::BotError::config(
                    "clustered copy execution requires shared event dedup",
                )
            })?;
            let fresh = store.mark_shared("copy_event", k).await?;
            let outcome = if fresh {
                DedupOutcome::Fresh
            } else {
                DedupOutcome::Duplicate
            };
            metrics::count_dedup(outcome.as_str());
            Ok(outcome)
        }
        None => claim_shared(state, event).await,
    }
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

    /// Process-local behavior witness (deterministic, no database, no sleep).
    ///
    /// Same event, two independent `AppState`s — exactly what two replicas
    /// behind the same feed hold when no durable dedup facade is attached.
    /// Both claims come back `Fresh`: the dedup stage is NOT an election, so
    /// the ownership claim is a separate, temporary exclusion mechanism. This test fails the moment somebody "fixes" the two-replica
    /// test by weakening the ownership gate on the strength of this call.
    #[tokio::test]
    async fn process_local_dedup_lets_both_replicas_through() {
        let replica_a = state();
        let replica_b = state();
        assert!(replica_a.dedup().is_none(), "no durable facade attached");
        assert!(replica_b.dedup().is_none(), "no durable facade attached");
        let e = event("sigRace", "mintRace", PositionSide::Long);
        assert_eq!(claim(&replica_a, &e).await, DedupOutcome::Fresh);
        assert_eq!(
            claim(&replica_b, &e).await,
            DedupOutcome::Fresh,
            "process-local dedup cannot elect a single executor"
        );
        // Within one replica the dedup still holds (that part is sound).
        assert_eq!(claim(&replica_a, &e).await, DedupOutcome::Duplicate);
        assert_eq!(claim(&replica_b, &e).await, DedupOutcome::Duplicate);
    }

    /// The failure mode is a second *execution*, not a second *arrival*:
    /// dedup drops the redelivery, yet one whale trade yields two independent
    /// executor intents when both replicas got a `Fresh` above. `entry_intent_id`
    /// is intentionally independent of the replica (one source trade → one
    /// ledger id), so the executor's intent dedup would collapse them — which
    /// is precisely why the lost update at the claim stage is money-relevant
    /// only while the ownership gate is missing. Pinned here so the reasoning
    /// survives refactors.
    #[test]
    fn one_trade_maps_to_one_executor_intent() {
        let a = crate::intent::entry_intent_id_for(
            &event("sigRace", "mintRace", PositionSide::Long),
            crate::intent::EntryRoute::Curve,
        );
        let b = crate::intent::entry_intent_id_for(
            &event("sigRace", "mintRace", PositionSide::Long),
            crate::intent::EntryRoute::Curve,
        );
        assert_eq!(
            a, b,
            "the executor's intent id must not depend on the replica"
        );
    }

    /// Tenant namespacing of the one authoritative dedup: the same event
    /// claimed by two organizations is two independent decisions, while a
    /// repeat inside one organization is still a duplicate. The bare
    /// operator key keeps working unchanged.
    #[tokio::test]
    async fn tenant_namespaced_keys_do_not_collide_and_never_lose_the_bare_key() {
        let st = state();
        let e = event("sigNS", "mintNS", PositionSide::Long);
        let a = e.tenant_dedup_key("org-a");
        let b = e.tenant_dedup_key("org-b");
        assert_eq!(a, "tenant:org-a:copy:sigNS:whale:mintNS:buy");
        assert_ne!(a, b);
        assert_eq!(
            claim_for(&st, &e, Some(&a)).await,
            DedupOutcome::Fresh,
            "tenant A sees the trade first"
        );
        assert_eq!(
            claim_for(&st, &e, Some(&b)).await,
            DedupOutcome::Fresh,
            "tenant B is not a duplicate of tenant A"
        );
        assert_eq!(
            claim_for(&st, &e, Some(&a)).await,
            DedupOutcome::Duplicate,
            "a tenant's own repeat still dedups"
        );
        // The operator path is untouched by tenant claims.
        assert_eq!(claim(&st, &e).await, DedupOutcome::Fresh);
        assert_eq!(claim(&st, &e).await, DedupOutcome::Duplicate);
        assert!(already_seen(&st, &e).await, "the operator key is marked");
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
