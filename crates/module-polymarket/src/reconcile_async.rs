//! Reconciliation of async commitments (PROMPT 4/10 §D).
//!
//! The base reconciler (`reconcile.rs`) compares local order state
//! with the venue. This module adds the async-commit dimension: for
//! every acceptance the async pipeline is still holding open
//! (matched-without-hashes or `delayed`), ask the venue ONCE per
//! reconciliation pass, classify the answer, journal a finding, and
//! close or keep the pending record.
//!
//! Findings use the same `poly_recon_findings` journal with kinds
//! prefixed `async_` so the existing dashboards/metrics (keyed by
//! `kind`) separate them cleanly:
//!
//! | kind | meaning | action |
//! |---|---|---|
//! | `async_settlement_settled` | all trades terminal; fills carried hashes | `resolved_settled` |
//! | `async_settlement_failed` | terminal FAILED trade(s) among the fills | `reported_failed` |
//! | `async_still_pending` | settlement/match still in flight | `keep_pending` |
//! | `async_order_gone` | venue does not know the order (404/cancelled) | `dropped_pending` |
//! | `async_stale_pending` | pending beyond its staleness budget | `reported_stale` |
//!
//! Nothing here fabricates: findings carry exactly what the venue
//! said, and a pending record is only dropped on a definitive venue
//! answer (`OrderGoneOnVenue`) or terminal trades.

use chrono::{DateTime, Utc};

use crate::backfill::{poll_once, AsyncPendingRegistry, BackfillVerdict, PendingAcceptance};
use crate::clob::ClobClient;
use crate::error::PolyResult;
use crate::store::{PolyReconFindingRecord, PolyStore};

/// Async-commit reconciliation finding kinds (journal `kind` values).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AsyncReconKind {
    /// Every trade reached a terminal state and fills are confirmed.
    Settled,
    /// At least one trade terminal-FAILED (never book those as fills).
    SettlementFailed,
    /// Still settling / still delayed — keep waiting.
    StillPending,
    /// The venue no longer knows the order.
    OrderGone,
    /// Pending beyond its staleness budget — needs operator eyes.
    StalePending,
}

impl AsyncReconKind {
    /// Journal `kind` label.
    pub fn as_str(&self) -> &'static str {
        match self {
            AsyncReconKind::Settled => "async_settlement_settled",
            AsyncReconKind::SettlementFailed => "async_settlement_failed",
            AsyncReconKind::StillPending => "async_still_pending",
            AsyncReconKind::OrderGone => "async_order_gone",
            AsyncReconKind::StalePending => "async_stale_pending",
        }
    }
}

/// One async reconciliation finding (journal-shaped).
#[derive(Debug, Clone, PartialEq)]
pub struct AsyncReconFinding {
    /// Which kind diverged/resolved.
    pub kind: AsyncReconKind,
    /// The venue order id.
    pub venue_order_id: String,
    /// Human detail carrying the venue's own words.
    pub detail: String,
    /// `resolved_settled` | `reported_failed` | `keep_pending` |
    /// `dropped_pending` | `reported_stale`.
    pub action: String,
}

/// Reconcile every pending async acceptance once.
///
/// For each registry entry: one venue poll (`poll_once` — no
/// waiting), classify, journal through the store, and update the
/// registry (drop on `OrderGoneOnVenue` or terminal verdicts; keep
/// on `StillPending`). Returns the findings in registry order.
///
/// `replica_id` identifies this process in the journal (same
/// convention as the base reconciler).
pub async fn reconcile_async_commitments(
    client: &ClobClient,
    registry: &AsyncPendingRegistry,
    store: &dyn PolyStore,
    replica_id: &str,
    now: DateTime<Utc>,
) -> PolyResult<Vec<AsyncReconFinding>> {
    let mut findings = Vec::new();
    for pending in registry.pending() {
        // A pending record past its staleness budget gets its own
        // finding even if the venue poll would keep it pending —
        // "still pending" and "pending for too long" are different
        // operational facts.
        let stale = pending.is_stale(now);
        let verdict = poll_once(client, &pending).await?;
        let finding = match &verdict {
            BackfillVerdict::Settled { fills, failed } => {
                let kind = if failed.is_empty() {
                    AsyncReconKind::Settled
                } else {
                    AsyncReconKind::SettlementFailed
                };
                let detail = if failed.is_empty() {
                    format!(
                        "all {} trade(s) terminal; {} confirmed fill(s) with settlement hashes",
                        fills.len() + failed.len(),
                        fills.len()
                    )
                } else {
                    let failed_list: Vec<String> = failed
                        .iter()
                        .map(|(id, status)| format!("{id}={status}"))
                        .collect();
                    format!(
                        "terminal failures among the fills: {} ({} confirmed)",
                        failed_list.join(", "),
                        fills.len()
                    )
                };
                AsyncReconFinding {
                    kind,
                    venue_order_id: pending.order_id.clone(),
                    detail,
                    action: if failed.is_empty() {
                        "resolved_settled".to_string()
                    } else {
                        "reported_failed".to_string()
                    },
                }
            }
            BackfillVerdict::StillPending { trades } => {
                let described: Vec<String> = trades
                    .iter()
                    .map(|t| format!("{}={}", t.trade_id, t.settlement.as_str()))
                    .collect();
                AsyncReconFinding {
                    kind: if stale {
                        AsyncReconKind::StalePending
                    } else {
                        AsyncReconKind::StillPending
                    },
                    venue_order_id: pending.order_id.clone(),
                    detail: format!(
                        "settlement still open ({}): [{}]",
                        pending.commit_state.as_str(),
                        described.join(", ")
                    ),
                    action: if stale {
                        "reported_stale".to_string()
                    } else {
                        "keep_pending".to_string()
                    },
                }
            }
            BackfillVerdict::OrderGoneOnVenue => AsyncReconFinding {
                kind: AsyncReconKind::OrderGone,
                venue_order_id: pending.order_id.clone(),
                detail: "venue does not know the order (not found / terminal cancelled)"
                    .to_string(),
                action: "dropped_pending".to_string(),
            },
        };
        // Registry bookkeeping: drop on definitive answers.
        match &verdict {
            BackfillVerdict::OrderGoneOnVenue => registry.remove(&pending.order_id),
            BackfillVerdict::Settled { .. } => registry.remove(&pending.order_id),
            BackfillVerdict::StillPending { .. } => {}
        }
        findings.push(finding);
    }
    // Journal every finding (same contract as the base reconciler:
    // a journal failure is counted, never swallowed).
    for f in &findings {
        let rec = PolyReconFindingRecord {
            id: 0,
            kind: f.kind.as_str().to_string(),
            venue_order_id: Some(f.venue_order_id.clone()),
            order_id: None,
            token_id: None,
            detail: f.detail.clone(),
            action: f.action.clone(),
            replica_id: replica_id.to_string(),
            ts: now,
        };
        if !store.append_finding(rec).await {
            crate::metrics::count_journal_error("append_finding");
        }
    }
    Ok(findings)
}

/// The pending entries that are stale right now, as findings (used by
/// callers that only want the operator-facing view).
pub fn stale_findings(
    registry: &AsyncPendingRegistry,
    now: DateTime<Utc>,
) -> Vec<AsyncReconFinding> {
    registry
        .stale(now)
        .into_iter()
        .map(|p: PendingAcceptance| AsyncReconFinding {
            kind: AsyncReconKind::StalePending,
            venue_order_id: p.order_id,
            detail: format!(
                "pending since {} (state {})",
                p.submitted_at,
                p.commit_state.as_str()
            ),
            action: "reported_stale".to_string(),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::MemoryPolyStore;

    #[test]
    fn kinds_are_prefixed_and_distinct() {
        assert_eq!(AsyncReconKind::Settled.as_str(), "async_settlement_settled");
        assert_eq!(
            AsyncReconKind::SettlementFailed.as_str(),
            "async_settlement_failed"
        );
        assert_eq!(AsyncReconKind::StillPending.as_str(), "async_still_pending");
        assert_eq!(AsyncReconKind::OrderGone.as_str(), "async_order_gone");
        assert_eq!(AsyncReconKind::StalePending.as_str(), "async_stale_pending");
    }

    #[test]
    fn stale_findings_reflect_the_registry() {
        let reg = AsyncPendingRegistry::new();
        assert!(stale_findings(&reg, Utc::now()).is_empty());
        let mut a = crate::async_commit::AsyncOrderAcceptance {
            order_id: Some("0xstale".into()),
            status: crate::async_commit::AcceptanceStatus::Delayed,
            success: Some(true),
            error_msg: None,
            making_amount: Some(0),
            taking_amount: Some(0),
            trade_ids: vec![],
            transactions_hashes: vec![],
        };
        a.order_id = Some("0xstale".into());
        reg.record(
            &a,
            Utc::now() - chrono::Duration::seconds(9_000),
            chrono::Duration::seconds(600),
        )
        .unwrap();
        let findings = stale_findings(&reg, Utc::now());
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].kind, AsyncReconKind::StalePending);
        assert_eq!(findings[0].action, "reported_stale");
    }

    #[tokio::test]
    async fn findings_journal_through_the_store() {
        // The journal path is exercised with a store that always
        // accepts; the venue poll path needs a server and is covered
        // by the async_commit_pipeline integration test.
        let store = MemoryPolyStore::new();
        let rec = PolyReconFindingRecord {
            id: 0,
            kind: AsyncReconKind::StillPending.as_str().to_string(),
            venue_order_id: Some("0xo".into()),
            order_id: None,
            token_id: None,
            detail: "settlement still open".into(),
            action: "keep_pending".into(),
            replica_id: "test".into(),
            ts: Utc::now(),
        };
        assert!(store.append_finding(rec).await);
    }
}
