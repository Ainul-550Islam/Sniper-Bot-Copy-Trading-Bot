//! Landing providers (GAP-MAP v2, P2) — a pluggable layer for getting a
//! SIGNED transaction accepted by the network.
//!
//! The executor (`crate::execute`) already implements Rpc/Jito/JitoThenRpc/
//! Race broadcast internally; this module adds the NEXT step the gap map
//! asks for: a provider *trait* so new landing routes (Helius sender, later
//! bloXroute / Nozomi / 0slot / BlockRazor — each only after reading that
//! provider's CURRENT docs) can be added without editing the executor, plus
//! a circuit breaker so a dead provider is shed instead of being retried
//! into a timeout on the hot path.
//!
//! Design rules:
//! * Providers accept an ALREADY-SIGNED, serialized versioned transaction.
//!   Nothing in this module signs or mutates bytes — re-signing is the
//!   executor's job (blockhash refresh lives there too).
//! * Acceptance ≠ landing. A [`SendReceipt`] means the route took the
//!   transaction; landing confirmation stays with the executor's confirm
//!   loop (`Rpc::confirm...`), which is the only place slot truth lives.
//! * Deterministic error surface: every failure is a [`BotError`] with a
//!   low-cardinality message prefix (`landing:`), so ops can alert on the
//!   provider name without parsing vendor error text.
//!
//! The circuit breaker and router are pure state machines over an async
//! trait — fully unit-tested offline with the `FakeLanding` test double (no sockets).

pub mod helius_sender;
pub mod jito;

use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use tokio::sync::Mutex;
use tracing::warn;

use bot_core::error::{BotError, BotResult};

/// One accepted send. The provider's own receipt id (bundle id, http id,
/// ...) is kept when the provider exposes one — it is the attestation an
/// operator uses when chasing a transaction with the provider's support.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SendReceipt {
    /// Provider label (`"jito"`, `"helius"`, ...).
    pub provider: String,
    /// The transaction signature (same on every route for a given signed tx).
    pub signature: String,
    /// Provider-side receipt reference, when available.
    pub provider_receipt: Option<String>,
    /// When the route accepted the transaction.
    pub accepted_at: Instant,
}

/// A route that can accept a signed, serialized versioned transaction.
#[async_trait]
pub trait LandingProvider: Send + Sync {
    /// Stable, low-cardinality label (`"jito"`, `"helius"`).
    fn name(&self) -> &'static str;

    /// Submit the transaction. Implementations MUST NOT sign, mutate, or
    /// re-serialize the bytes — they send exactly what they are given.
    async fn send(&self, signed_tx_base64: &str, signature: &str) -> BotResult<SendReceipt>;
}

/// Circuit-breaker states (classic three-state machine).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BreakerState {
    /// Passing calls through.
    Closed,
    /// Shedding calls until `retry_at`.
    Open,
    /// Letting exactly one probe call through to test recovery.
    HalfOpen,
}

/// Snapshot of breaker counters for observability.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BreakerStats {
    pub state: BreakerState,
    pub consecutive_failures: u32,
    pub total_success: u64,
    pub total_failure: u64,
    pub total_shed: u64,
}

struct BreakerInner {
    state: BreakerState,
    consecutive_failures: u32,
    open_until: Option<Instant>,
    total_success: u64,
    total_failure: u64,
    total_shed: u64,
}

/// Circuit breaker around any [`LandingProvider`].
///
/// After `failure_threshold` CONSECUTIVE failures the breaker opens and
/// sheds calls for `open_for`. The first call after the cooldown goes
/// through as a half-open probe: success closes the breaker, failure
/// re-opens it for another full cooldown. A shed call fails fast with
/// [`BotError::rpc`] carrying the provider name — it never blocks
/// and never touches the network.
pub struct CircuitBreaker<P: LandingProvider> {
    provider: P,
    failure_threshold: u32,
    open_for: Duration,
    inner: Mutex<BreakerInner>,
}

impl<P: LandingProvider> CircuitBreaker<P> {
    /// Wrap a provider. `failure_threshold` ≥ 1; `open_for` > 0.
    pub fn new(provider: P, failure_threshold: u32, open_for: Duration) -> Self {
        CircuitBreaker {
            provider,
            failure_threshold: failure_threshold.max(1),
            open_for,
            inner: Mutex::new(BreakerInner {
                state: BreakerState::Closed,
                consecutive_failures: 0,
                open_until: None,
                total_success: 0,
                total_failure: 0,
                total_shed: 0,
            }),
        }
    }

    /// The wrapped provider's label.
    pub fn provider_name(&self) -> &'static str {
        self.provider.name()
    }

    /// Current counters/state (for metrics and tests).
    pub async fn stats(&self) -> BreakerStats {
        let inner = self.inner.lock().await;
        let state = match (inner.state, inner.open_until) {
            // An expired Open window is, functionally, HalfOpen already —
            // report what the NEXT call will experience.
            (BreakerState::Open, Some(until)) if Instant::now() >= until => BreakerState::HalfOpen,
            (s, _) => s,
        };
        BreakerStats {
            state,
            consecutive_failures: inner.consecutive_failures,
            total_success: inner.total_success,
            total_failure: inner.total_failure,
            total_shed: inner.total_shed,
        }
    }

    /// Decide what the next call may do, WITHOUT advancing state.
    async fn gate(&self) -> Result<BreakerState, BotError> {
        let mut inner = self.inner.lock().await;
        match inner.state {
            BreakerState::Closed => Ok(BreakerState::Closed),
            BreakerState::Open => {
                let due = inner.open_until.map(|t| Instant::now() >= t).unwrap_or(true);
                if due {
                    // Transition now so exactly ONE caller probes.
                    inner.state = BreakerState::HalfOpen;
                    Ok(BreakerState::HalfOpen)
                } else {
                    inner.total_shed += 1;
                    Err(BotError::rpc(format!(
                        "landing: {} circuit open, shedding call",
                        self.provider.name()
                    )))
                }
            }
            BreakerState::HalfOpen => {
                // A probe is already in flight: shed until it resolves so a
                // burst cannot flood a recovering provider.
                inner.total_shed += 1;
                Err(BotError::rpc(format!(
                    "landing: {} circuit half-open, probe in flight",
                    self.provider.name()
                )))
            }
        }
    }

    /// Record the outcome of one attempted call.
    async fn record(&self, ok: bool) {
        let mut inner = self.inner.lock().await;
        if ok {
            inner.total_success += 1;
            inner.consecutive_failures = 0;
            inner.state = BreakerState::Closed;
            inner.open_until = None;
        } else {
            inner.total_failure += 1;
            inner.consecutive_failures += 1;
            let trip = inner.state == BreakerState::HalfOpen
                || inner.consecutive_failures >= self.failure_threshold;
            if trip {
                inner.state = BreakerState::Open;
                inner.open_until = Some(Instant::now() + self.open_for);
            }
        }
    }
}

#[async_trait]
impl<P: LandingProvider> LandingProvider for CircuitBreaker<P> {
    fn name(&self) -> &'static str {
        self.provider.name()
    }

    async fn send(&self, signed_tx_base64: &str, signature: &str) -> BotResult<SendReceipt> {
        self.gate().await?;
        let result = self.provider.send(signed_tx_base64, signature).await;
        self.record(result.is_ok()).await;
        result
    }
}

/// Sequential failover across providers, in priority order: the first
/// ACCEPTANCE wins. This is the cross-provider analogue of the executor's
/// `JitoThenRpc` mode, but over arbitrary plugged-in routes. Providers whose
/// breakers are open fail fast and cost nothing on the hot path.
pub struct LandingRouter {
    providers: Vec<Arc<dyn LandingProvider>>,
}

impl LandingRouter {
    /// Build a router from providers in priority order (highest first).
    pub fn new(providers: Vec<Arc<dyn LandingProvider>>) -> Self {
        LandingRouter { providers }
    }

    /// Try each provider in order; return the first acceptance, or an error
    /// summarising every refusal (an empty router is a configuration error).
    pub async fn send(&self, signed_tx_base64: &str, signature: &str) -> BotResult<SendReceipt> {
        if self.providers.is_empty() {
            return Err(BotError::config(
                "landing: router has no providers configured",
            ));
        }
        let mut last: Option<BotError> = None;
        for provider in &self.providers {
            match provider.send(signed_tx_base64, signature).await {
                Ok(receipt) => return Ok(receipt),
                Err(error) => {
                    warn!(provider = provider.name(), error = %error, "landing route refused");
                    last = Some(error);
                }
            }
        }
        Err(last.unwrap_or_else(|| {
            BotError::rpc("landing: every route refused".to_string())
        }))
    }
}

// ---------------------------------------------------------------------------
// Tests — offline state-machine coverage (no sockets anywhere).
// ---------------------------------------------------------------------------
#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    /// Scriptable offline provider: fails the first `n` calls, then succeeds.
    struct FakeLanding {
        label: &'static str,
        fail_first: AtomicU32,
        calls: AtomicU32,
    }

    impl FakeLanding {
        fn new(label: &'static str, fail_first: u32) -> Self {
            FakeLanding {
                label,
                fail_first: AtomicU32::new(fail_first),
                calls: AtomicU32::new(0),
            }
        }
        fn calls(&self) -> u32 {
            self.calls.load(Ordering::SeqCst)
        }
    }

    #[async_trait]
    impl LandingProvider for FakeLanding {
        fn name(&self) -> &'static str {
            self.label
        }
        async fn send(&self, _tx: &str, sig: &str) -> BotResult<SendReceipt> {
            let call = self.calls.fetch_add(1, Ordering::SeqCst);
            if call < self.fail_first.load(Ordering::SeqCst) {
                return Err(BotError::rpc(format!("fake:{call}")));
            }
            Ok(SendReceipt {
                provider: self.label.to_string(),
                signature: sig.to_string(),
                provider_receipt: None,
                accepted_at: Instant::now(),
            })
        }
    }

    #[tokio::test]
    async fn breaker_passes_calls_while_closed() {
        let breaker = CircuitBreaker::new(FakeLanding::new("fake", 0), 3, Duration::from_secs(60));
        for _ in 0..5 {
            assert!(breaker.send("tx", "sig").await.is_ok());
        }
        let stats = breaker.stats().await;
        assert_eq!(stats.state, BreakerState::Closed);
        assert_eq!(stats.total_success, 5);
        assert_eq!(stats.consecutive_failures, 0);
    }

    #[tokio::test]
    async fn breaker_opens_after_threshold_consecutive_failures() {
        let breaker = CircuitBreaker::new(
            FakeLanding::new("fake", u32::MAX),
            3,
            Duration::from_secs(60),
        );
        for _ in 0..3 {
            assert!(breaker.send("tx", "sig").await.is_err());
        }
        assert_eq!(breaker.stats().await.state, BreakerState::Open);
        // The next call is SHED: the underlying provider sees no new call.
        let err = breaker.send("tx", "sig").await.unwrap_err();
        assert!(err.to_string().contains("circuit open"));
        assert_eq!(breaker.provider.calls(), 3, "shed call must not reach the provider");
        assert_eq!(breaker.stats().await.total_shed, 1);
    }

    #[tokio::test]
    async fn breaker_recovers_through_a_half_open_probe() {
        let provider = FakeLanding::new("fake", 2); // two failures then ok
        let breaker = CircuitBreaker::new(provider, 2, Duration::from_millis(1));
        assert!(breaker.send("tx", "sig").await.is_err());
        assert!(breaker.send("tx", "sig").await.is_err());
        assert_eq!(breaker.stats().await.state, BreakerState::Open);
        // Let the cooldown lapse; the reported state becomes HalfOpen.
        tokio::time::sleep(Duration::from_millis(5)).await;
        assert_eq!(breaker.stats().await.state, BreakerState::HalfOpen);
        // Probe succeeds -> closed again.
        assert!(breaker.send("tx", "sig").await.is_ok());
        let stats = breaker.stats().await;
        assert_eq!(stats.state, BreakerState::Closed);
        assert_eq!(stats.consecutive_failures, 0);
    }

    #[tokio::test]
    async fn failed_probe_reopens_the_breaker() {
        let provider = FakeLanding::new("fake", u32::MAX); // always fails
        let breaker = CircuitBreaker::new(provider, 2, Duration::from_millis(1));
        let _ = breaker.send("tx", "sig").await;
        let _ = breaker.send("tx", "sig").await;
        tokio::time::sleep(Duration::from_millis(5)).await;
        // Probe fails -> straight back to Open (not another 2-failure cycle).
        assert!(breaker.send("tx", "sig").await.is_err());
        assert_eq!(breaker.stats().await.state, BreakerState::Open);
    }

    #[tokio::test]
    async fn success_resets_the_failure_streak() {
        let provider = FakeLanding::new("fake", 1); // only the first call fails
        let breaker = CircuitBreaker::new(provider, 3, Duration::from_secs(60));
        assert!(breaker.send("tx", "sig").await.is_err());
        // Two successes afterwards: streak resets, breaker stays closed even
        // though total failures == 1 historically.
        assert!(breaker.send("tx", "sig").await.is_ok());
        assert!(breaker.send("tx", "sig").await.is_ok());
        assert_eq!(breaker.stats().await.state, BreakerState::Closed);
        assert_eq!(breaker.stats().await.consecutive_failures, 0);
    }

    #[tokio::test]
    async fn router_takes_the_first_acceptance_in_priority_order() {
        let bad = Arc::new(FakeLanding::new("bad", u32::MAX));
        let good = Arc::new(FakeLanding::new("good", 0));
        let router = LandingRouter::new(vec![bad.clone(), good.clone()]);
        let receipt = router.send("tx", "sig-1").await.expect("router accepts");
        assert_eq!(receipt.provider, "good");
        assert_eq!(receipt.signature, "sig-1");
        assert_eq!(bad.calls(), 1, "first route got its chance");
        assert_eq!(good.calls(), 1);
    }

    #[tokio::test]
    async fn router_reports_the_last_refusal_when_all_fail() {
        let a = Arc::new(FakeLanding::new("a", u32::MAX));
        let b = Arc::new(FakeLanding::new("b", u32::MAX));
        let router = LandingRouter::new(vec![a, b]);
        let err = router.send("tx", "sig").await.unwrap_err();
        assert!(err.to_string().contains("fake:0"), "last refusal surfaces: {err}");
    }

    #[tokio::test]
    async fn empty_router_is_a_configuration_error() {
        let router = LandingRouter::new(vec![]);
        let err = router.send("tx", "sig").await.unwrap_err();
        assert!(err.to_string().contains("no providers"));
    }

    #[tokio::test]
    async fn router_with_breakers_sheds_open_routes_cheaply() {
        let flaky = Arc::new(CircuitBreaker::new(
            FakeLanding::new("flaky", u32::MAX),
            1,
            Duration::from_secs(60),
        ));
        let steady = Arc::new(FakeLanding::new("steady", 0));
        // Trip flaky's breaker once.
        let _ = flaky.send("tx", "sig").await;
        assert_eq!(flaky.stats().await.state, BreakerState::Open);
        let router = LandingRouter::new(vec![flaky.clone(), steady.clone()]);
        let receipt = router.send("tx", "sig-2").await.expect("accepts via steady");
        assert_eq!(receipt.provider, "steady");
        // The flaky provider itself saw only the 1 original (tripping) call.
        assert_eq!(flaky.stats().await.total_shed, 1);
    }
}
