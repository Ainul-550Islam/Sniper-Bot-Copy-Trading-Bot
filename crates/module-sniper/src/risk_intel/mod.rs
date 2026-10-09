//! Risk intelligence layer (GAP-MAP v2, P2).
//!
//! Aggregates the non-price risk signals that the sniper's entry gates
//! consume, and publishes the dev-sell signal that the advanced exit policy
//! (`crate::exit_policy`) acts on:
//!
//! | submodule        | concern                                            |
//! |------------------|----------------------------------------------------|
//! | `holders`        | holder concentration (solana-kit bridge)           |
//! | `creator_history`| creator wallet track record across launches        |
//! | `bundler`        | same-funder / coordinated first-slot buyer clusters|
//! | `honeypot`       | sell-path probe verdicts with TTL cache            |
//! | `external`       | OPTIONAL third-party risk APIs (off by default)    |
//!
//! Wiring status: the holder/bundler gates in `crate::gates` already expose
//! `max_top_holder_pct` / `max_bundler_ratio` (P1). This module supplies the
//! DATA those gates evaluate. The dev-sell bus connects to
//! `Sniper::with_dev_sell_bus`. Everything here is additive — nothing
//! changes when the features are unconfigured.

pub mod bundler;
pub mod creator_history;
pub mod external;
pub mod holders;
pub mod honeypot;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use solana_sdk::pubkey::Pubkey;

use crate::exit_policy::{DevSellBus, DevSellSignal};

/// One dev-sell observation, ready to publish on the bus.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DevSellObservation {
    /// The mint whose creator/dev wallet sold.
    pub mint: Pubkey,
    /// When the sale was observed (feed timestamp, not detection time).
    pub observed_at: DateTime<Utc>,
    /// The wallet that sold (for audit/telemetry).
    pub seller: Pubkey,
}

/// The coordinator: owns the dev-sell bus and forwards observations.
///
/// Deliberately thin — each signal family keeps its own state machine in its
/// submodule; this type exists so callers have ONE handle to publish
/// observations without reaching into channel plumbing.
#[derive(Clone)]
pub struct RiskIntel {
    dev_sell_tx: DevSellBus,
}

impl RiskIntel {
    /// Create the coordinator with a fresh dev-sell bus.
    pub fn new() -> Self {
        // Capacity: dev sells are rare, high-value events; a small ring is
        // enough, and a lagged subscriber MUST miss signals rather than
        // block the detector (money-path independence).
        let (tx, _rx) = tokio::sync::broadcast::channel::<DevSellSignal>(64);
        RiskIntel { dev_sell_tx: tx }
    }

    /// Wrap an existing bus (e.g. shared across modules).
    pub fn with_bus(tx: DevSellBus) -> Self {
        RiskIntel { dev_sell_tx: tx }
    }

    /// The bus sender — hand this to `Sniper::with_dev_sell_bus`.
    pub fn bus(&self) -> DevSellBus {
        self.dev_sell_tx.clone()
    }

    /// Publish a dev-sell observation. Returns false when NO subscriber
    /// exists (the sweeper never attached) — the observation is dropped
    /// rather than buffered forever.
    pub fn publish_dev_sell(&self, obs: DevSellObservation) -> bool {
        self.dev_sell_tx
            .send((obs.mint, obs.observed_at))
            .is_ok()
    }
}

impl Default for RiskIntel {
    fn default() -> Self {
        RiskIntel::new()
    }
}

/// Aggregated risk picture for one launch candidate, as seen at gate time.
/// Each field is `None` when that signal family could not be evaluated —
/// gates decide (via `strict_gates`) whether a missing signal is a skip or
/// a reject. No fabricated verdicts.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct RiskIntelReport {
    /// Holder concentration, in bps of float (holders.rs).
    pub holder_top1_bps: Option<u64>,
    /// Creator rug-rate basis: launches seen / rugs seen (creator_history).
    pub creator_rug_rate_bps: Option<u64>,
    /// Fraction of first-slot buyers in same-funder clusters, bps (bundler).
    pub bundler_ratio_bps: Option<u64>,
    /// Sell-probe verdict (honeypot) — true = sellable.
    pub sell_probe_ok: Option<bool>,
    /// External provider verdict, when enabled and reachable.
    pub external_flagged: Option<bool>,
}

impl RiskIntelReport {
    /// True when every requested signal produced a verdict. `required`
    /// mirrors which families the operator's config switched on.
    pub fn complete(&self, required: &RequiredSignals) -> bool {
        (!required.holders || self.holder_top1_bps.is_some())
            && (!required.creator_history || self.creator_rug_rate_bps.is_some())
            && (!required.bundler || self.bundler_ratio_bps.is_some())
            && (!required.honeypot || self.sell_probe_ok.is_some())
            && (!required.external || self.external_flagged.is_some())
    }
}

/// Which signal families must be present for a report to count as complete.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RequiredSignals {
    pub holders: bool,
    pub creator_history: bool,
    pub bundler: bool,
    pub honeypot: bool,
    pub external: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn dev_sell_observations_reach_subscribers() {
        let intel = RiskIntel::new();
        let mut rx = intel.bus().subscribe();
        let mint = Pubkey::new_unique();
        let now = Utc::now();
        assert!(intel.publish_dev_sell(DevSellObservation {
            mint,
            observed_at: now,
            seller: Pubkey::new_unique(),
        }));
        let (got_mint, got_ts) = rx.try_recv().expect("signal delivered");
        assert_eq!(got_mint, mint);
        assert_eq!(got_ts, now);
    }

    #[tokio::test]
    async fn publish_without_subscribers_is_dropped_not_buffered() {
        let intel = RiskIntel::new();
        // No subscriber attached: send reports the absence.
        let delivered = intel.publish_dev_sell(DevSellObservation {
            mint: Pubkey::new_unique(),
            observed_at: Utc::now(),
            seller: Pubkey::new_unique(),
        });
        assert!(!delivered);
    }

    #[test]
    fn report_completeness_tracks_required_families() {
        let mut report = RiskIntelReport::default();
        let required = RequiredSignals {
            holders: true,
            creator_history: true,
            bundler: false,
            honeypot: false,
            external: false,
        };
        assert!(!report.complete(&required));
        report.holder_top1_bps = Some(1_500);
        assert!(!report.complete(&required), "creator history still missing");
        report.creator_rug_rate_bps = Some(0);
        assert!(report.complete(&required));
    }

    #[test]
    fn empty_requirements_are_always_complete() {
        assert!(RiskIntelReport::default().complete(&RequiredSignals::default()));
    }
}
