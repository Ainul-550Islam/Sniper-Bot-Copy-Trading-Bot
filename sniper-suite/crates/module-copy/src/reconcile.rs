//! Leader ↔ follower reconciliation (TASK 3 §08).
//!
//! Mirroring drifts: a leader sells while we were paused or while
//! `mirror_exits` was off, our sweeper closes a position on TP/SL, a
//! partial exit changes our quantity, an entry lands ambiguously, a leader is
//! unfollowed while we still hold its mint. Reconciliation compares three
//! views — the **links** (`copy_links`: what we opened, from whom), the
//! **position book** (what we still hold) and the **leader activity
//! journal** (`copy_events`: what the leader did since) — and produces
//! findings with a suggested action. The pure part is [`reconcile`];
//! [`crate::CopyBot::reconcile_once`] gathers inputs and applies actions.
//!
//! | finding | evidence | action |
//! |---|---|---|
//! | `leader_exited_we_hold` | open link, leader SELL on the mint after the link opened and no later BUY | `MirrorExit` when `copy.reconcile_auto_exit` (through the normal exit path), else `Flag` |
//! | `leader_removed_we_hold` | open link whose leader is `removed` | `Flag` |
//! | `link_without_position` | open link, position closed or missing | `CloseLink` |
//! | `quantity_mismatch` | link `follower_qty` ≠ position `qty` | `UpdateLink` |
//! | `orphan_position` | open copy position with no link and no `copied_wallet` | `Flag` |
//! | `ambiguous_entry` | position whose entry is still live in the ledger | `Flag` (the ledger owns it) |
//!
//! This module compares our own records with each other; it does not read
//! the chain. On-chain truth for our own transactions is the execution
//! ledger's job (`bot_core::reconciliation`), which stays authoritative.

use std::collections::HashMap;

use bot_core::db::copy::{CopyEventRecord, CopyLinkRecord};
use bot_core::execution::ExecutionRecord;
use bot_core::models::{Position, PositionStatus};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::leader::LeaderStatus;
use crate::recovery::{entry_fate, EntryFate};

/// Kind of finding.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FindingKind {
    /// The leader sold; we still hold.
    LeaderExitedWeHold,
    /// The leader was unfollowed; we still hold.
    LeaderRemovedWeHold,
    /// The link is open but the position is not.
    LinkWithoutPosition,
    /// Our quantity changed since the link was written.
    QuantityMismatch,
    /// A copy position that cannot be attributed to a leader.
    OrphanPosition,
    /// The entry is still ambiguous in the execution ledger.
    AmbiguousEntry,
}

impl FindingKind {
    /// Metric / audit label.
    pub fn as_str(&self) -> &'static str {
        match self {
            FindingKind::LeaderExitedWeHold => "leader_exited_we_hold",
            FindingKind::LeaderRemovedWeHold => "leader_removed_we_hold",
            FindingKind::LinkWithoutPosition => "link_without_position",
            FindingKind::QuantityMismatch => "quantity_mismatch",
            FindingKind::OrphanPosition => "orphan_position",
            FindingKind::AmbiguousEntry => "ambiguous_entry",
        }
    }
}

impl std::fmt::Display for FindingKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// What to do about a finding.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum ReconAction {
    /// Surface only (audit + metric).
    Flag,
    /// Sell the position through the normal mirrored-exit path.
    MirrorExit {
        /// Position to sell.
        position_id: String,
        /// Fraction to sell (`1.0` = full).
        fraction: f64,
    },
    /// Close the link.
    CloseLink {
        /// Link key.
        position_id: String,
        /// `closed` | `orphaned`.
        status: String,
    },
    /// Refresh the link's follower quantity.
    UpdateLink {
        /// Link key.
        position_id: String,
        /// Current quantity.
        follower_qty: f64,
    },
}

impl ReconAction {
    /// Label.
    pub fn as_str(&self) -> &'static str {
        match self {
            ReconAction::Flag => "flag",
            ReconAction::MirrorExit { .. } => "mirror_exit",
            ReconAction::CloseLink { .. } => "close_link",
            ReconAction::UpdateLink { .. } => "update_link",
        }
    }
}

/// One finding.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Finding {
    /// Kind.
    pub kind: FindingKind,
    /// Leader (empty for orphans).
    pub leader: String,
    /// Mint.
    pub mint: String,
    /// Position involved, when any.
    pub position_id: Option<String>,
    /// Human detail.
    pub detail: String,
    /// Suggested action.
    pub action: ReconAction,
}

/// Inputs to [`reconcile`].
#[derive(Debug, Clone, Default)]
pub struct ReconInputs {
    /// Open links.
    pub links: Vec<CopyLinkRecord>,
    /// Copy positions, open and closed.
    pub positions: Vec<Position>,
    /// Journaled leader events (any stage) inside the lookback window.
    pub activity: Vec<CopyEventRecord>,
    /// Leader status by address.
    pub leader_status: HashMap<String, LeaderStatus>,
    /// Execution-ledger records for module `copy`.
    pub ledger: Vec<ExecutionRecord>,
    /// `copy.reconcile_auto_exit && copy.mirror_exits`.
    pub auto_exit: bool,
}

/// Result of one pass.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ReconReport {
    /// Findings, in detection order.
    pub findings: Vec<Finding>,
    /// Links examined.
    pub links_checked: usize,
    /// Open positions examined.
    pub positions_checked: usize,
    /// When the pass ran.
    pub at: DateTime<Utc>,
}

impl ReconReport {
    /// Findings of one kind.
    pub fn count(&self, kind: FindingKind) -> usize {
        self.findings.iter().filter(|f| f.kind == kind).count()
    }

    /// Whether nothing was found.
    pub fn is_clean(&self) -> bool {
        self.findings.is_empty()
    }
}

/// Relative quantity difference above which a link is refreshed.
pub const QTY_TOLERANCE: f64 = 1e-9;

/// Pure comparison. Deterministic for the same inputs.
pub fn reconcile(inputs: &ReconInputs) -> ReconReport {
    let now = Utc::now();
    let mut findings = Vec::new();
    let is_open =
        |p: &Position| p.status == PositionStatus::Open || p.status == PositionStatus::Closing;
    let open_positions: Vec<&Position> = inputs.positions.iter().filter(|p| is_open(p)).collect();
    let by_id: HashMap<&str, &Position> = inputs
        .positions
        .iter()
        .map(|p| (p.id.as_str(), p))
        .collect();
    let ledger_by_sig: HashMap<&str, &ExecutionRecord> = inputs
        .ledger
        .iter()
        .filter_map(|r| r.signature.as_deref().map(|s| (s, r)))
        .collect();

    for link in inputs.links.iter().filter(|l| l.status == "open") {
        let position = by_id.get(link.position_id.as_str()).copied();
        let Some(position) = position.filter(|p| is_open(p)) else {
            let (status, detail) = match position {
                Some(p) => ("closed", format!("position {} is {:?}", p.id, p.status)),
                None => ("orphaned", "position not found in the book".to_string()),
            };
            findings.push(Finding {
                kind: FindingKind::LinkWithoutPosition,
                leader: link.leader.clone(),
                mint: link.mint.clone(),
                position_id: Some(link.position_id.clone()),
                detail,
                action: ReconAction::CloseLink {
                    position_id: link.position_id.clone(),
                    status: status.into(),
                },
            });
            continue;
        };

        // Leader activity on this mint since we opened.
        let mut last_buy: Option<DateTime<Utc>> = None;
        let mut last_sell: Option<(DateTime<Utc>, String, f64)> = None;
        for e in inputs
            .activity
            .iter()
            .filter(|e| e.leader == link.leader && e.mint == link.mint)
            .filter(|e| e.observed_at > link.opened_at && e.event_id != link.entry_event_id)
        {
            let newer_buy = last_buy.map(|t| e.observed_at > t).unwrap_or(true);
            let newer_sell = last_sell
                .as_ref()
                .map(|(t, _, _)| e.observed_at > *t)
                .unwrap_or(true);
            match e.side.as_str() {
                "buy" if newer_buy => last_buy = Some(e.observed_at),
                "sell" if newer_sell => {
                    last_sell = Some((e.observed_at, e.event_id.clone(), e.token_amount));
                }
                _ => {}
            }
        }
        if let Some((sold_at, event_id, tokens)) = last_sell {
            let re_bought = last_buy.map(|t| t > sold_at).unwrap_or(false);
            if !re_bought {
                let action = if inputs.auto_exit {
                    ReconAction::MirrorExit {
                        position_id: position.id.clone(),
                        fraction: 1.0,
                    }
                } else {
                    ReconAction::Flag
                };
                findings.push(Finding {
                    kind: FindingKind::LeaderExitedWeHold,
                    leader: link.leader.clone(),
                    mint: link.mint.clone(),
                    position_id: Some(position.id.clone()),
                    detail: format!(
                        "leader sold {tokens:.4} tokens at {} (event {event_id}); we hold {:.4}",
                        sold_at.to_rfc3339(),
                        position.qty
                    ),
                    action,
                });
            }
        }

        if inputs.leader_status.get(&link.leader) == Some(&LeaderStatus::Removed) {
            findings.push(Finding {
                kind: FindingKind::LeaderRemovedWeHold,
                leader: link.leader.clone(),
                mint: link.mint.clone(),
                position_id: Some(position.id.clone()),
                detail: format!(
                    "leader unfollowed; position {} stays under the sweeper's TP/SL management",
                    position.id
                ),
                action: ReconAction::Flag,
            });
        }

        let diff = (link.follower_qty - position.qty).abs();
        let scale = link.follower_qty.abs().max(position.qty.abs()).max(1e-12);
        if diff / scale > QTY_TOLERANCE {
            findings.push(Finding {
                kind: FindingKind::QuantityMismatch,
                leader: link.leader.clone(),
                mint: link.mint.clone(),
                position_id: Some(position.id.clone()),
                detail: format!(
                    "link records {:.6}, position holds {:.6}",
                    link.follower_qty, position.qty
                ),
                action: ReconAction::UpdateLink {
                    position_id: position.id.clone(),
                    follower_qty: position.qty,
                },
            });
        }
    }

    let linked: std::collections::HashSet<&str> = inputs
        .links
        .iter()
        .filter(|l| l.status == "open")
        .map(|l| l.position_id.as_str())
        .collect();
    for p in &open_positions {
        if !linked.contains(p.id.as_str()) && p.copied_wallet.as_deref().unwrap_or("").is_empty() {
            findings.push(Finding {
                kind: FindingKind::OrphanPosition,
                leader: String::new(),
                mint: p.symbol.clone(),
                position_id: Some(p.id.clone()),
                detail: "copy position without a link or a copied_wallet".into(),
                action: ReconAction::Flag,
            });
        }
        if let Some(sig) = p.entry_signature.as_deref().filter(|s| !s.is_empty()) {
            if entry_fate(ledger_by_sig.get(sig).map(|r| r.state)) == EntryFate::Unknown {
                findings.push(Finding {
                    kind: FindingKind::AmbiguousEntry,
                    leader: p.copied_wallet.clone().unwrap_or_default(),
                    mint: p.symbol.clone(),
                    position_id: Some(p.id.clone()),
                    detail: format!("entry {sig} is still live in the execution ledger"),
                    action: ReconAction::Flag,
                });
            }
        }
    }

    ReconReport {
        findings,
        links_checked: inputs.links.iter().filter(|l| l.status == "open").count(),
        positions_checked: open_positions.len(),
        at: now,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::recovery::link_for;
    use bot_core::execution::ExecutionState;
    use bot_core::models::{ExecutionMode, TradeSource, Venue};
    use chrono::Duration;

    fn position(
        id: &str,
        mint: &str,
        qty: f64,
        leader: Option<&str>,
        sig: Option<&str>,
    ) -> Position {
        let mut p = Position::new(
            id.to_string(),
            TradeSource::Copy,
            Venue::PumpFun,
            ExecutionMode::Paper,
            mint.to_string(),
            mint.to_string(),
            "SOL".into(),
        );
        p.apply_buy(qty, 0.001, qty * 0.001);
        p.copied_wallet = leader.map(|s| s.to_string());
        p.entry_signature = sig.map(|s| s.to_string());
        p.opened_at = Utc::now() - Duration::minutes(10);
        p
    }

    fn activity(
        id: &str,
        leader: &str,
        mint: &str,
        side: &str,
        ago_secs: i64,
        tokens: f64,
    ) -> CopyEventRecord {
        let at = Utc::now() - Duration::seconds(ago_secs);
        CopyEventRecord {
            event_id: id.into(),
            leader: leader.into(),
            signature: format!("sig-{id}"),
            slot: 1,
            mint: mint.into(),
            side: side.into(),
            venue: "pump.fun".into(),
            token_amount: tokens,
            sol_amount: 0.1,
            source: "pumpportal".into(),
            source_sequence: 0,
            event_at: Some(at),
            observed_at: at,
            stage: "REJECTED".into(),
            reject_reason: Some("NO_POSITION_TO_EXIT".into()),
            detail: None,
            intent_id: None,
            position_id: None,
            created_at: at,
            updated_at: at,
        }
    }

    fn open_link(pos: &str, leader: &str, mint: &str, qty: f64) -> CopyLinkRecord {
        let mut l = link_for(
            pos,
            leader,
            mint,
            "entry-ev",
            "entry-sig",
            None,
            1000.0,
            qty,
        );
        l.opened_at = Utc::now() - Duration::minutes(10);
        l
    }

    #[test]
    fn clean_book_has_no_findings() {
        let inputs = ReconInputs {
            links: vec![open_link("p1", "A", "m1", 100.0)],
            positions: vec![position("p1", "m1", 100.0, Some("A"), Some("s1"))],
            leader_status: HashMap::from([("A".to_string(), LeaderStatus::Active)]),
            ..Default::default()
        };
        let r = reconcile(&inputs);
        assert!(r.is_clean(), "{:?}", r.findings);
        assert_eq!(r.links_checked, 1);
        assert_eq!(r.positions_checked, 1);
    }

    #[test]
    fn leader_exit_is_flagged_or_mirrored() {
        let mut inputs = ReconInputs {
            links: vec![open_link("p1", "A", "m1", 100.0)],
            positions: vec![position("p1", "m1", 100.0, Some("A"), None)],
            activity: vec![activity("e-sell", "A", "m1", "sell", 60, 5000.0)],
            leader_status: HashMap::from([("A".to_string(), LeaderStatus::Active)]),
            ..Default::default()
        };
        let r = reconcile(&inputs);
        assert_eq!(r.count(FindingKind::LeaderExitedWeHold), 1);
        assert_eq!(r.findings[0].action, ReconAction::Flag);
        inputs.auto_exit = true;
        let r = reconcile(&inputs);
        assert_eq!(
            r.findings[0].action,
            ReconAction::MirrorExit {
                position_id: "p1".into(),
                fraction: 1.0
            }
        );
        // A later re-buy cancels the exit finding.
        inputs
            .activity
            .push(activity("e-rebuy", "A", "m1", "buy", 30, 100.0));
        assert_eq!(reconcile(&inputs).count(FindingKind::LeaderExitedWeHold), 0);
        // Activity before the link opened does not count.
        inputs.activity = vec![activity("e-old", "A", "m1", "sell", 3600, 1.0)];
        assert!(reconcile(&inputs).is_clean());
        // The entry event itself is never "activity since".
        let mut own = activity("entry-ev", "A", "m1", "buy", 1, 1.0);
        own.side = "sell".into();
        inputs.activity = vec![own];
        assert!(reconcile(&inputs).is_clean());
    }

    #[test]
    fn link_and_quantity_drift() {
        let mut closed = position("p-closed", "m2", 10.0, Some("A"), None);
        closed.status = PositionStatus::Closed;
        let inputs = ReconInputs {
            links: vec![
                open_link("p-closed", "A", "m2", 10.0),
                open_link("p-gone", "A", "m3", 10.0),
                open_link("p-drift", "A", "m4", 10.0),
                {
                    let mut l = open_link("p-done", "A", "m5", 1.0);
                    l.status = "closed".into();
                    l
                },
            ],
            positions: vec![closed, position("p-drift", "m4", 4.0, Some("A"), None)],
            leader_status: HashMap::from([("A".to_string(), LeaderStatus::Removed)]),
            ..Default::default()
        };
        let r = reconcile(&inputs);
        assert_eq!(r.links_checked, 3, "closed links are skipped");
        assert_eq!(r.count(FindingKind::LinkWithoutPosition), 2);
        assert!(r.findings.iter().any(|f| f.action
            == ReconAction::CloseLink {
                position_id: "p-closed".into(),
                status: "closed".into()
            }));
        assert!(r.findings.iter().any(|f| f.action
            == ReconAction::CloseLink {
                position_id: "p-gone".into(),
                status: "orphaned".into()
            }));
        assert_eq!(r.count(FindingKind::QuantityMismatch), 1);
        assert!(r.findings.iter().any(|f| f.action
            == ReconAction::UpdateLink {
                position_id: "p-drift".into(),
                follower_qty: 4.0
            }));
        assert_eq!(r.count(FindingKind::LeaderRemovedWeHold), 1);
    }

    #[test]
    fn orphans_and_ambiguous_entries() {
        let json = serde_json::json!({
            "intent_id": "i1", "module": "copy", "label": "copy-m1", "wallet": "w", "symbol": "m1",
            "state": "submitted", "attempts": 1, "signature": "s-amb", "blockhash": null,
            "last_valid_block_height": null, "priority_fee_micro_lamports": 0, "failure": null,
            "error": null, "created_at": Utc::now(), "updated_at": Utc::now(),
        });
        let rec: ExecutionRecord = serde_json::from_value(json).unwrap();
        let inputs = ReconInputs {
            positions: vec![
                position("p-orphan", "m9", 1.0, None, None),
                position("p-amb", "m1", 1.0, Some("A"), Some("s-amb")),
            ],
            ledger: vec![rec],
            ..Default::default()
        };
        let r = reconcile(&inputs);
        assert_eq!(r.count(FindingKind::OrphanPosition), 1);
        assert_eq!(r.count(FindingKind::AmbiguousEntry), 1);
        assert_eq!(
            r.findings
                .iter()
                .filter(|f| f.action == ReconAction::Flag)
                .count(),
            2
        );
        assert_eq!(ExecutionState::Submitted.as_str(), "submitted");
    }
}
