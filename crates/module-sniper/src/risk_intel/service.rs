//! Production owner for the sniper's mutable risk-intel state.
//!
//! [`RiskIntelService`] is deliberately the only production owner of the
//! [`CreatorLedger`] and [`SellProbeCache`]. Entry, market loading and the
//! exit sweeper all receive the same service handle from `Sniper`, so a
//! position close can affect the next launch decision and repeated market
//! loads for one mint can reuse the same sell-probe verdict while it is
//! fresh.
//!
//! The service never manufactures a signal:
//! * unknown creators stay unknown unless an outcome was recorded here;
//! * a sell probe is cached only after the caller's real probe completes;
//! * a position close is attributed to a creator only when the launch was
//!   observed through [`RiskIntelService::note_launch`].

use std::collections::HashMap;
use std::future::Future;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use chrono::{DateTime, Utc};
use solana_kit::token_safety::SellProbeOutcome;
use solana_sdk::pubkey::Pubkey;

use bot_core::config::RiskConfig;

use super::creator_history::{CreatorLedger, CreatorStats, LaunchOutcome, LaunchRecord};
use super::honeypot::SellProbeCache;

/// Default lifetime of a sell-probe verdict. A launch can be evaluated more
/// than once while the entry pipeline runs; a minute is long enough to avoid
/// duplicate RPC simulations without letting a stale verdict outlive the
/// market conditions that produced it.
pub const DEFAULT_SELL_PROBE_TTL: Duration = Duration::from_secs(60);

/// Return multiple at or below which a fully closed position is recorded as
/// a rugged launch for its creator. This classifies an observed close; it
/// does not introduce a gate or alter the thresholds in `gates.rs`.
const RUG_RETURN_MULTIPLE: f64 = 0.1;

#[derive(Debug)]
struct RiskIntelState {
    creator_ledger: CreatorLedger,
    sell_probe_cache: SellProbeCache,
    creators_by_mint: HashMap<Pubkey, (Pubkey, usize)>,
}

/// Cloneable handle to the shared risk-intel state.
///
/// Cloning the handle does not clone the ledger or cache. `Sniper::new`
/// creates one service and the exit-sweeper clone receives the same handle,
/// preserving the single-owner invariant across both production paths.
#[derive(Debug, Clone)]
pub struct RiskIntelService {
    state: Arc<Mutex<RiskIntelState>>,
}

impl RiskIntelService {
    /// Create a service with the production sell-probe TTL.
    pub fn new() -> Self {
        Self::with_sell_probe_ttl(DEFAULT_SELL_PROBE_TTL)
    }

    /// Create a service with an explicit sell-probe TTL.
    pub fn with_sell_probe_ttl(ttl: Duration) -> Self {
        Self {
            state: Arc::new(Mutex::new(RiskIntelState {
                creator_ledger: CreatorLedger::new(1),
                sell_probe_cache: SellProbeCache::new(ttl),
                creators_by_mint: HashMap::new(),
            })),
        }
    }

    fn lock(&self) -> MutexGuard<'_, RiskIntelState> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Remember which creator launched `mint` for one booked position
    /// when the launch source actually reported the creator. Unknown
    /// creators are left unknown. The count is the number of booked
    /// positions still awaiting a full close for this mint.
    pub fn note_launch(&self, mint: &Pubkey, creator: Option<&Pubkey>) {
        if let Some(creator) = creator {
            let mut state = self.lock();
            let entry = state.creators_by_mint.entry(*mint).or_insert((*creator, 0));
            entry.0 = *creator;
            entry.1 = entry.1.saturating_add(1);
        }
    }

    /// Creator recorded for an observed launch, if one was reported.
    pub fn creator_for_mint(&self, mint: &Pubkey) -> Option<Pubkey> {
        self.lock()
            .creators_by_mint
            .get(mint)
            .map(|(creator, _)| *creator)
    }

    /// Forget one booked-position reference without recording an outcome.
    /// Used when a booked entry is cleaned up because it provably never
    /// landed: no tokens were held, so that cleanup is not evidence about
    /// the launch or its creator.
    pub fn forget_launch(&self, mint: &Pubkey) {
        let _ = self.take_creator_for_close(mint);
    }

    /// Consume one booked-position reference for `mint` at full close.
    /// The last close removes the mint, so this attribution map tracks
    /// open positions rather than every launch the process has ever seen.
    fn take_creator_for_close(&self, mint: &Pubkey) -> Option<Pubkey> {
        let mut state = self.lock();
        let (creator, remaining) = {
            let entry = state.creators_by_mint.get_mut(mint)?;
            entry.1 = entry.1.saturating_sub(1);
            (entry.0, entry.1)
        };
        if remaining == 0 {
            state.creators_by_mint.remove(mint);
        }
        Some(creator)
    }

    /// Build the `known_bad_creators` list consumed by the existing
    /// `RiskEngine::check_launch_with_lists` implementation.
    ///
    /// This intentionally adds no second repeat-offender policy. The core
    /// `block_repeat_offender_creators` setting remains the switch; this
    /// method only supplies observed ledger data to that existing check.
    pub fn known_bad_creators(&self, creator: Option<&Pubkey>, risk: &RiskConfig) -> Vec<String> {
        if !risk.block_repeat_offender_creators {
            return Vec::new();
        }
        let Some(creator) = creator else {
            return Vec::new();
        };
        if self.lock().creator_ledger.is_serial_rugger(creator, 1) {
            vec![creator.to_string()]
        } else {
            Vec::new()
        }
    }

    /// True when the ledger contains at least one observed rug by `creator`.
    pub fn is_serial_rugger(&self, creator: &Pubkey) -> bool {
        self.lock().creator_ledger.is_serial_rugger(creator, 1)
    }

    /// Observed rug rate for a creator, or `None` when no usable history
    /// exists. Unknown history is never reported as a zero rate.
    pub fn rug_rate_bps(&self, creator: &Pubkey) -> Option<u64> {
        self.lock().creator_ledger.rug_rate_bps(creator)
    }

    /// Aggregate observed history for a creator.
    pub fn creator_stats(&self, creator: &Pubkey) -> Option<CreatorStats> {
        self.lock().creator_ledger.stats(creator)
    }

    /// Record a terminal launch outcome. `CreatorLedger` keeps the first
    /// outcome per mint, so duplicate observations cannot double-count.
    pub fn record_outcome(
        &self,
        creator: &Pubkey,
        mint: Pubkey,
        outcome: LaunchOutcome,
        observed_at: DateTime<Utc>,
    ) {
        self.lock().creator_ledger.record(
            creator,
            LaunchRecord {
                mint,
                outcome,
                observed_at,
            },
        );
    }

    /// Classify and record a full position close for `mint`.
    ///
    /// Returns `None` without recording when the creator was not observed
    /// for this mint, either price is unusable, or the close does not
    /// establish a terminal launch outcome. A close at no more than ten
    /// percent of its entry price is an observed near-total loss and is
    /// recorded as [`LaunchOutcome::Rugged`]; a smaller loss is a fade.
    /// A break-even-or-better position close proves neither migration nor
    /// any other terminal launch outcome, so nothing is invented for it.
    pub fn record_position_close(
        &self,
        mint: &Pubkey,
        entry_price: f64,
        exit_price: f64,
        closed_at: DateTime<Utc>,
    ) -> Option<LaunchOutcome> {
        let creator = self.take_creator_for_close(mint)?;
        let outcome = classify_close(entry_price, exit_price)?;
        self.record_outcome(&creator, *mint, outcome, closed_at);
        Some(outcome)
    }

    /// Fresh cached sell-probe outcome for `mint`, if one exists.
    pub fn cached_sell_probe(&self, mint: &Pubkey, now: DateTime<Utc>) -> Option<SellProbeOutcome> {
        self.lock().sell_probe_cache.get(mint, now).cloned()
    }

    /// Store the outcome of a completed sell probe. Expired verdicts are
    /// pruned on the same write so the cache cannot accumulate one entry
    /// for every mint the process has ever evaluated.
    pub fn cache_sell_probe(
        &self,
        mint: Pubkey,
        outcome: SellProbeOutcome,
        probed_at: DateTime<Utc>,
    ) {
        let mut state = self.lock();
        state.sell_probe_cache.prune(probed_at);
        state.sell_probe_cache.insert(mint, outcome, probed_at);
    }

    /// Return a fresh cache entry or run `probe` once and cache its verdict.
    ///
    /// The mutex is never held while the probe runs, so an RPC simulation
    /// cannot block unrelated creator-ledger reads.
    pub async fn sell_probe_with_cache<F, Fut>(
        &self,
        mint: &Pubkey,
        now: DateTime<Utc>,
        probe: F,
    ) -> SellProbeOutcome
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = SellProbeOutcome>,
    {
        if let Some(outcome) = self.cached_sell_probe(mint, now) {
            return outcome;
        }
        let outcome = probe().await;
        self.cache_sell_probe(*mint, outcome.clone(), now);
        outcome
    }

    /// Drop expired sell-probe entries.
    pub fn prune_sell_probe_cache(&self, now: DateTime<Utc>) {
        self.lock().sell_probe_cache.prune(now);
    }
}

impl Default for RiskIntelService {
    fn default() -> Self {
        Self::new()
    }
}

/// Pure position-close classification used by [`RiskIntelService`].
pub fn classify_close(entry_price: f64, exit_price: f64) -> Option<LaunchOutcome> {
    if !entry_price.is_finite() || !exit_price.is_finite() || entry_price <= 0.0 || exit_price < 0.0
    {
        return None;
    }
    let multiple = exit_price / entry_price;
    if multiple <= RUG_RETURN_MULTIPLE {
        Some(LaunchOutcome::Rugged)
    } else if multiple < 1.0 {
        Some(LaunchOutcome::Faded)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn at(secs: i64) -> DateTime<Utc> {
        DateTime::from_timestamp(secs, 0).unwrap()
    }

    #[test]
    fn close_classification_uses_observed_prices_only() {
        assert_eq!(classify_close(10.0, 1.0), Some(LaunchOutcome::Rugged));
        assert_eq!(classify_close(10.0, 5.0), Some(LaunchOutcome::Faded));
        assert_eq!(classify_close(10.0, 11.0), None);
        assert_eq!(classify_close(0.0, 1.0), None);
        assert_eq!(classify_close(10.0, f64::NAN), None);
    }

    #[test]
    fn close_without_observed_creator_is_not_attributed() {
        let service = RiskIntelService::new();
        let outcome = service.record_position_close(&Pubkey::new_unique(), 1.0, 0.01, at(1_000));
        assert_eq!(outcome, None);
    }

    #[test]
    fn full_close_consumes_creator_attribution() {
        let service = RiskIntelService::new();
        let mint = Pubkey::new_unique();
        let creator = Pubkey::new_unique();
        service.note_launch(&mint, Some(&creator));
        assert_eq!(
            service.record_position_close(&mint, 1.0, 0.5, at(1_000)),
            Some(LaunchOutcome::Faded)
        );
        assert_eq!(service.creator_for_mint(&mint), None);
        assert_eq!(
            service.record_position_close(&mint, 1.0, 0.01, at(2_000)),
            None
        );
    }

    #[test]
    fn forgotten_launch_records_no_creator_outcome() {
        let service = RiskIntelService::new();
        let mint = Pubkey::new_unique();
        let creator = Pubkey::new_unique();
        service.note_launch(&mint, Some(&creator));
        service.forget_launch(&mint);
        assert_eq!(service.creator_for_mint(&mint), None);
        assert_eq!(service.creator_stats(&creator), None);
    }

    #[tokio::test]
    async fn second_probe_within_ttl_uses_cache() {
        let service = RiskIntelService::new();
        let mint = Pubkey::new_unique();
        let calls = Arc::new(AtomicUsize::new(0));

        let first_calls = Arc::clone(&calls);
        let first = service
            .sell_probe_with_cache(&mint, at(1_000), || async move {
                first_calls.fetch_add(1, Ordering::SeqCst);
                SellProbeOutcome::Sellable
            })
            .await;
        let second_calls = Arc::clone(&calls);
        let second = service
            .sell_probe_with_cache(&mint, at(1_010), || async move {
                second_calls.fetch_add(1, Ordering::SeqCst);
                SellProbeOutcome::NotSellable("must not run".into())
            })
            .await;

        assert_eq!(first, SellProbeOutcome::Sellable);
        assert_eq!(second, SellProbeOutcome::Sellable);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }
}
