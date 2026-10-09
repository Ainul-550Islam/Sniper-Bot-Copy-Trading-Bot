//! Reconciliation — local order state versus the venue.
//!
//! [`PolyBot::reconcile_once`] compares the tracker with the venue's open
//! list and per-order status (live) and with the position book (any mode),
//! acts only on venue-confirmed facts and reports everything else. Every
//! finding is journaled (`poly_recon_findings`), metered
//! (`poly_recon_findings_total{kind}`) and audited (`poly.recon.<kind>`).
//! Cancelling orphan venue orders is opt-in (`reconcile_cancel_orphans`) and
//! counts as done only when the venue's `canceled` list names the order.
//!
//! Chain truth for *positions* (CTF ERC-1155 balances) is the server's
//! `polymarket_position` reconciliation task, which uses the shared engine in
//! `bot_core::reconciliation`; this module reconciles *orders*.

use std::collections::{HashMap, HashSet};

use chrono::Utc;
use tracing::{debug, info, warn};

use bot_core::config::PolymarketConfig;
use bot_core::models::{BotModule, ExecutionMode};

use crate::clob::CancelOutcome;
use crate::error::PolyResult;
use crate::metrics;
use crate::orders::{FillSource, LocalOrderState, TrackedOrder, VenueObservation};
use crate::store::PolyReconFindingRecord;
use crate::PolyBot;

/// Kinds of local-vs-venue divergence the reconciler reports.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ReconKind {
    /// The venue holds an open order for our API key that we do not track.
    OrphanVenueOrder,
    /// We track a live order the venue does not have (open list + status).
    LocalOrderMissingOnVenue,
    /// The venue reports a different matched size than we booked.
    MatchedSizeMismatch,
    /// A submit-unknown order was resolved by asking the venue.
    AmbiguousSubmitResolved,
    /// An open Polymarket position has no order behind it.
    PositionWithoutOrder,
    /// A resting order outlived its TTL / GTD expiry locally.
    StaleOrder,
}

impl ReconKind {
    /// Stable label (journal `kind`, metric label, audit action).
    pub fn as_str(&self) -> &'static str {
        match self {
            ReconKind::OrphanVenueOrder => "orphan_venue_order",
            ReconKind::LocalOrderMissingOnVenue => "local_order_missing_on_venue",
            ReconKind::MatchedSizeMismatch => "matched_size_mismatch",
            ReconKind::AmbiguousSubmitResolved => "ambiguous_submit_resolved",
            ReconKind::PositionWithoutOrder => "position_without_order",
            ReconKind::StaleOrder => "stale_order",
        }
    }
}

/// One reconciliation finding and what was done about it.
#[derive(Debug, Clone, PartialEq)]
pub struct ReconFinding {
    /// What diverged.
    pub kind: ReconKind,
    /// Venue order id involved (if any).
    pub venue_order_id: Option<String>,
    /// OMS order id involved (if any).
    pub order_id: Option<String>,
    /// Outcome token involved (if any).
    pub token_id: Option<String>,
    /// Human detail.
    pub detail: String,
    /// `reported` | `cancelled` | `resolved_filled` | `resolved_cancelled` |
    /// `resolved_expired` | `marked_unknown` | `marked_failed` | `adopted`.
    pub action: String,
}

impl PolyBot {
    /// Compare local order state with the venue (live) or with the position
    /// book (paper) and act on divergences. Every finding is journaled,
    /// metered and audited; cancelling orphans is opt-in
    /// (`reconcile_cancel_orphans`).
    pub async fn reconcile_once(&self, poly: &PolymarketConfig) -> PolyResult<Vec<ReconFinding>> {
        let mut findings = Vec::new();
        let now = Utc::now();
        let mode = self.state.execution_mode().await;
        let live_client = if mode == ExecutionMode::Live {
            self.authed_client().await.ok()
        } else {
            None
        };

        if let Some(client) = &live_client {
            let venue_open = client.open_orders(None, None).await?;
            let tracked: HashMap<String, TrackedOrder> = self
                .tracked
                .read()
                .await
                .iter()
                .map(|(k, v)| (k.to_ascii_lowercase(), v.clone()))
                .collect();

            // 1. Venue orders we do not know.
            for v in &venue_open {
                if tracked.contains_key(&v.id.to_ascii_lowercase()) {
                    continue;
                }
                let mut action = "reported".to_string();
                if poly.reconcile_cancel_orphans {
                    metrics::count_cancel("recon");
                    match client.cancel_order(&v.id).await {
                        Ok(resp) => {
                            let out = CancelOutcome::from_value(&resp);
                            // Only the venue's `canceled` list is a
                            // confirmation; a refusal or an unrecognised
                            // answer leaves the orphan on the venue and is
                            // reported as such (retried next run).
                            action = if out.confirmed(&v.id) {
                                "cancelled".into()
                            } else if out
                                .not_canceled
                                .iter()
                                .any(|(id, _)| id.eq_ignore_ascii_case(&v.id))
                            {
                                "cancel_refused".into()
                            } else {
                                "cancel_unconfirmed".into()
                            };
                        }
                        Err(e) => {
                            warn!(order = %v.id, error = %e, "orphan cancel failed");
                            action = "cancel_failed".into();
                        }
                    }
                }
                findings.push(ReconFinding {
                    kind: ReconKind::OrphanVenueOrder,
                    venue_order_id: Some(v.id.clone()),
                    order_id: None,
                    token_id: Some(v.asset_id.clone()),
                    detail: format!(
                        "open on venue, unknown locally: {} {} @ {} matched {}",
                        v.side, v.original_size, v.price, v.size_matched
                    ),
                    action,
                });
            }

            // 2. Local live orders vs the venue.
            for (_, mut t) in tracked {
                if t.state.is_terminal() || t.mode != ExecutionMode::Live {
                    continue;
                }
                let on_venue = venue_open
                    .iter()
                    .find(|v| v.id.eq_ignore_ascii_case(&t.venue_order_id));
                match on_venue {
                    Some(v) => {
                        let venue_matched_opt = v.size_matched_opt();
                        let venue_matched = venue_matched_opt.unwrap_or(t.size_matched);
                        if venue_matched_opt.is_some()
                            && (venue_matched - t.size_matched).abs() > 1e-6
                        {
                            let detail = format!(
                                "venue matched {venue_matched:.4} vs local {:.4}",
                                t.size_matched
                            );
                            let obs = VenueObservation {
                                state: v.state(),
                                raw_status: v.status.clone(),
                                size_matched: venue_matched_opt,
                                fill_delta: None,
                                trade_id: None,
                                price: None,
                                source: FillSource::Recon,
                                at: now,
                                associate_trades: v.associate_trades.clone(),
                            };
                            let action = match self.apply_observation(&mut t, &obs).await {
                                Ok(_) => "resolved_booked".to_string(),
                                Err(e) => format!("reported ({e})"),
                            };
                            findings.push(ReconFinding {
                                kind: ReconKind::MatchedSizeMismatch,
                                venue_order_id: Some(t.venue_order_id.clone()),
                                order_id: Some(t.order_id.clone()),
                                token_id: Some(t.token_id.clone()),
                                detail,
                                action,
                            });
                        }
                        if t.state == LocalOrderState::Unknown
                            || t.state == LocalOrderState::Submitted
                        {
                            // An ambiguous submit that IS on the book.
                            let obs = VenueObservation {
                                state: v.state(),
                                raw_status: v.status.clone(),
                                size_matched: venue_matched_opt,
                                fill_delta: None,
                                trade_id: None,
                                price: None,
                                source: FillSource::Recon,
                                at: now,
                                associate_trades: v.associate_trades.clone(),
                            };
                            let _ = self.apply_observation(&mut t, &obs).await;
                            findings.push(ReconFinding {
                                kind: ReconKind::AmbiguousSubmitResolved,
                                venue_order_id: Some(t.venue_order_id.clone()),
                                order_id: Some(t.order_id.clone()),
                                token_id: Some(t.token_id.clone()),
                                detail: "resting on venue".into(),
                                action: format!("resolved_{}", t.state.as_str()),
                            });
                        }
                    }
                    None => match client.order(&t.venue_order_id).await {
                        Ok(Some(v)) => {
                            let was_ambiguous = matches!(
                                t.state,
                                LocalOrderState::Unknown | LocalOrderState::Submitted
                            );
                            let obs = VenueObservation {
                                state: v.state(),
                                raw_status: v.status.clone(),
                                size_matched: v.size_matched_opt(),
                                fill_delta: None,
                                trade_id: None,
                                price: None,
                                source: FillSource::Recon,
                                at: now,
                                associate_trades: v.associate_trades.clone(),
                            };
                            let action = match self.apply_observation(&mut t, &obs).await {
                                Ok(_) => format!("resolved_{}", t.state.as_str()),
                                Err(e) => format!("reported ({e})"),
                            };
                            findings.push(ReconFinding {
                                kind: if was_ambiguous {
                                    ReconKind::AmbiguousSubmitResolved
                                } else {
                                    ReconKind::LocalOrderMissingOnVenue
                                },
                                venue_order_id: Some(t.venue_order_id.clone()),
                                order_id: Some(t.order_id.clone()),
                                token_id: Some(t.token_id.clone()),
                                detail: format!("not in open list; venue status {}", v.status),
                                action,
                            });
                        }
                        Ok(None) => {
                            // Never acknowledged by the venue → it never
                            // existed there: definite failure. Anything the
                            // venue once acknowledged (resting, partially
                            // filled, a `matched` FAK with an unread
                            // quantity) is held as unknown — never a
                            // fabricated fill or cancel.
                            let never_acknowledged = matches!(
                                t.state,
                                LocalOrderState::Submitted | LocalOrderState::Unknown
                            ) && !t.venue_acknowledged();
                            let action = if never_acknowledged {
                                self.finish_locally(
                                    &mut t,
                                    LocalOrderState::Failed,
                                    "not on venue",
                                )
                                .await;
                                "marked_failed"
                            } else {
                                self.finish_locally(
                                    &mut t,
                                    LocalOrderState::Unknown,
                                    "vanished from venue",
                                )
                                .await;
                                "marked_unknown"
                            };
                            findings.push(ReconFinding {
                                kind: ReconKind::LocalOrderMissingOnVenue,
                                venue_order_id: Some(t.venue_order_id.clone()),
                                order_id: Some(t.order_id.clone()),
                                token_id: Some(t.token_id.clone()),
                                detail: "venue has no record of this order".into(),
                                action: action.into(),
                            });
                        }
                        Err(e) => {
                            debug!(order = %t.venue_order_id, error = %e, "recon status lookup failed");
                        }
                    },
                }
            }
        }

        // 3. Positions without an order behind them (any mode).
        let positions = self.state.open_positions_for(BotModule::Polymarket).await;
        if !positions.is_empty() {
            let tracked = self.tracked.read().await;
            let journaled: HashSet<String> = tracked
                .values()
                .filter_map(|t| t.position_id.clone())
                .collect();
            for p in positions {
                if journaled.contains(&p.id) {
                    continue;
                }
                let has_order = tracked.values().any(|t| t.token_id == p.symbol);
                if !has_order {
                    findings.push(ReconFinding {
                        kind: ReconKind::PositionWithoutOrder,
                        venue_order_id: None,
                        order_id: None,
                        token_id: Some(p.symbol.clone()),
                        detail: format!(
                            "open position {} qty {:.2} has no tracked order (pre-engine or restored)",
                            p.id, p.qty
                        ),
                        action: "reported".into(),
                    });
                }
            }
        }

        // 4. Stale local orders (TTL / expiry) that polling could not cancel.
        {
            let stale: Vec<TrackedOrder> = self
                .tracked
                .read()
                .await
                .values()
                .filter(|t| {
                    !t.state.is_terminal()
                        && (t.ttl_elapsed(now, poly.order_ttl_secs) || t.expiry_passed(now))
                })
                .cloned()
                .collect();
            for t in stale {
                findings.push(ReconFinding {
                    kind: ReconKind::StaleOrder,
                    venue_order_id: Some(t.venue_order_id.clone()),
                    order_id: Some(t.order_id.clone()),
                    token_id: Some(t.token_id.clone()),
                    detail: format!(
                        "age {}s ttl {}s expiration {}",
                        t.age_secs(now),
                        poly.order_ttl_secs,
                        t.expiration
                    ),
                    action: "reported".into(),
                });
            }
        }

        for f in &findings {
            metrics::count_recon_finding(f.kind.as_str());
            self.audit(
                &format!("poly.recon.{}", f.kind.as_str()),
                f.venue_order_id.as_deref().unwrap_or("-"),
                &format!("{} action={} {}", f.kind.as_str(), f.action, f.detail),
            );
            let ok = self
                .store
                .append_finding(PolyReconFindingRecord {
                    id: 0,
                    kind: f.kind.as_str().into(),
                    venue_order_id: f.venue_order_id.clone(),
                    order_id: f.order_id.clone(),
                    token_id: f.token_id.clone(),
                    detail: f.detail.clone(),
                    action: f.action.clone(),
                    replica_id: self.state.replica_id().to_string(),
                    ts: now,
                })
                .await;
            if !ok {
                metrics::count_journal_error("append_finding");
            }
        }
        if !findings.is_empty() {
            info!(count = findings.len(), "polymarket reconciliation findings");
        }
        Ok(findings)
    }
}
