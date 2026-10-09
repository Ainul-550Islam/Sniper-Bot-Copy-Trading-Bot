//! OPTIONAL external risk-intelligence providers (GAP-MAP v2, P2).
//!
//! Third-party token-risk APIs (RugCheck, Birdeye, GoPlus, ...) behind one
//! trait, with a circuit breaker and a TTL cache. **Off by default** — the
//! GAP item is explicit that these are optional, and this module refuses to
//! fabricate verdicts:
//!
//! * no provider configured          -> `evaluate` returns `None` (skip);
//! * every provider failed / breaker open -> `None` (skip), never a guess;
//! * at least one provider answered  -> aggregate flag.
//!
//! Provider-specific response parsing is deliberately INJECTED, not baked
//! in: each vendor's JSON schema changes without notice, and shipping a
//! hard-coded parser written against yesterday's docs would silently produce
//! wrong verdicts. Wire a parser when you have read the CURRENT docs for the
//! provider you enable (same discipline as `solana-kit/src/venues/`).

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use solana_sdk::pubkey::Pubkey;
use tokio::sync::Mutex;

use bot_core::error::{BotError, BotResult};

/// One external risk verdict.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExternalSignal {
    /// True when the provider considers the token dangerous.
    pub flagged: bool,
    /// Provider label ("rugcheck", "goplus", ...).
    pub source: String,
    /// Human-readable detail (shown in gate summaries, never secrets).
    pub detail: String,
}

/// A provider that can assess one mint.
#[async_trait]
pub trait ExternalRiskProvider: Send + Sync {
    /// Stable label.
    fn name(&self) -> &'static str;

    /// Assess one mint. Implementations must fail with an Err (not a
    /// fabricated signal) when they cannot reach their backend.
    async fn assess(&self, mint: &Pubkey) -> BotResult<ExternalSignal>;
}

/// Per-provider breaker state (a small, self-contained breaker — the
/// landing breaker is typed for `LandingProvider`, so it is not reused).
#[derive(Debug)]
struct ProviderBreaker {
    consecutive_failures: u32,
    open_until: Option<Instant>,
}

impl ProviderBreaker {
    fn new() -> Self {
        ProviderBreaker {
            consecutive_failures: 0,
            open_until: None,
        }
    }

    fn available(&self, now: Instant) -> bool {
        self.open_until.map(|t| now >= t).unwrap_or(true)
    }

    fn record(&mut self, ok: bool, threshold: u32, cooldown: Duration, now: Instant) {
        if ok {
            self.consecutive_failures = 0;
            self.open_until = None;
        } else {
            self.consecutive_failures += 1;
            if self.consecutive_failures >= threshold {
                self.open_until = Some(now + cooldown);
            }
        }
    }
}

struct CachedSignal {
    signal: ExternalSignal,
    fetched_at: DateTime<Utc>,
}

/// The client: aggregates providers, breaks on repeated failure, caches.
pub struct ExternalRiskClient {
    enabled: bool,
    providers: Vec<Arc<dyn ExternalRiskProvider>>,
    breakers: Mutex<HashMap<&'static str, ProviderBreaker>>,
    cache: Mutex<HashMap<Pubkey, CachedSignal>>,
    cache_ttl: Duration,
    breaker_threshold: u32,
    breaker_cooldown: Duration,
}

impl ExternalRiskClient {
    /// A DISABLED client (the default): `evaluate` is always `None` and no
    /// network call is ever made.
    pub fn disabled() -> Self {
        ExternalRiskClient {
            enabled: false,
            providers: Vec::new(),
            breakers: Mutex::new(HashMap::new()),
            cache: Mutex::new(HashMap::new()),
            cache_ttl: Duration::from_secs(300),
            breaker_threshold: 3,
            breaker_cooldown: Duration::from_secs(60),
        }
    }

    /// Enable with a provider set. `cache_ttl` bounds how long a verdict is
    /// trusted; the breaker parameters bound retry storms against a dying
    /// backend.
    pub fn enabled_with(
        providers: Vec<Arc<dyn ExternalRiskProvider>>,
        cache_ttl: Duration,
        breaker_threshold: u32,
        breaker_cooldown: Duration,
    ) -> Self {
        ExternalRiskClient {
            enabled: !providers.is_empty(),
            providers,
            breakers: Mutex::new(HashMap::new()),
            cache: Mutex::new(HashMap::new()),
            cache_ttl,
            breaker_threshold: breaker_threshold.max(1),
            breaker_cooldown,
        }
    }

    /// Whether this client can produce verdicts at all.
    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    /// Evaluate one mint at `now`:
    /// * disabled / no providers            -> `None`;
    /// * fresh cached verdict               -> cached flag;
    /// * providers queried until one answers -> aggregate, cached;
    /// * every provider failed/broken-open  -> `None` (skip).
    pub async fn evaluate(&self, mint: &Pubkey, now: DateTime<Utc>) -> Option<bool> {
        if !self.enabled {
            return None;
        }
        // Cache first.
        {
            let cache = self.cache.lock().await;
            if let Some(entry) = cache.get(mint) {
                let fresh = now
                    .signed_duration_since(entry.fetched_at)
                    .to_std()
                    .map(|age| age <= self.cache_ttl)
                    .unwrap_or(false);
                if fresh {
                    return Some(entry.signal.flagged);
                }
            }
        }
        // Providers in order; the FIRST answer wins (they are a redundant
        // safety net, not a voting quorum — a quorum of unreachable
        // providers must not delay the hot path).
        for provider in &self.providers {
            let open = {
                let breakers = self.breakers.lock().await;
                breakers
                    .get(provider.name())
                    .map(|b| !b.available(Instant::now()))
                    .unwrap_or(false)
            };
            if open {
                continue;
            }
            let result = provider.assess(mint).await;
            {
                let mut breakers = self.breakers.lock().await;
                let breaker = breakers
                    .entry(provider.name())
                    .or_insert_with(ProviderBreaker::new);
                breaker.record(
                    result.is_ok(),
                    self.breaker_threshold,
                    self.breaker_cooldown,
                    Instant::now(),
                );
            }
            if let Ok(signal) = result {
                let flagged = signal.flagged;
                let mut cache = self.cache.lock().await;
                cache.insert(*mint, CachedSignal { signal, fetched_at: now });
                return Some(flagged);
            }
        }
        None
    }
}

/// A generic HTTP-JSON provider. The URL template receives the mint
/// (`{mint}` is replaced), and the PARSER is injected — see the module docs
/// for why parsing is never hard-coded here.
pub struct HttpJsonProvider {
    label: &'static str,
    http: reqwest::Client,
    url_template: String,
    parse: fn(&serde_json::Value) -> BotResult<ExternalSignal>,
}

impl HttpJsonProvider {
    /// Build a provider. `url_template` must contain `{mint}`.
    pub fn new(
        label: &'static str,
        url_template: impl Into<String>,
        parse: fn(&serde_json::Value) -> BotResult<ExternalSignal>,
    ) -> BotResult<Self> {
        let url_template = url_template.into();
        if !url_template.contains("{mint}") {
            return Err(BotError::config(format!(
                "external provider {label}: url_template must contain {{mint}}"
            )));
        }
        Ok(HttpJsonProvider {
            label,
            http: reqwest::Client::new(),
            url_template,
            parse,
        })
    }
}

#[async_trait]
impl ExternalRiskProvider for HttpJsonProvider {
    fn name(&self) -> &'static str {
        self.label
    }

    async fn assess(&self, mint: &Pubkey) -> BotResult<ExternalSignal> {
        let url = self.url_template.replace("{mint}", &mint.to_string());
        let response = self
            .http
            .get(&url)
            .send()
            .await
            .map_err(|e| BotError::http(format!("external {}: {e}", self.label)))?;
        let status = response.status();
        let body = response
            .text()
            .await
            .map_err(|e| BotError::http(format!("external {} body: {e}", self.label)))?;
        if !status.is_success() {
            return Err(BotError::http(format!(
                "external {} http {status}",
                self.label
            )));
        }
        let value: serde_json::Value = serde_json::from_str(&body)
            .map_err(|e| BotError::encoding(format!("external {} json: {e}", self.label)))?;
        let mut signal = (self.parse)(&value)?;
        signal.source = self.label.to_string();
        Ok(signal)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    struct FakeProvider {
        label: &'static str,
        fail_first: AtomicU32,
        flagged: bool,
        calls: AtomicU32,
    }

    impl FakeProvider {
        fn ok(label: &'static str, flagged: bool) -> Arc<Self> {
            Arc::new(FakeProvider {
                label,
                fail_first: AtomicU32::new(0),
                flagged,
                calls: AtomicU32::new(0),
            })
        }
        fn failing(label: &'static str, fail_first: u32) -> Arc<Self> {
            Arc::new(FakeProvider {
                label,
                fail_first: AtomicU32::new(fail_first),
                flagged: false,
                calls: AtomicU32::new(0),
            })
        }
        fn calls(&self) -> u32 {
            self.calls.load(Ordering::SeqCst)
        }
    }

    #[async_trait]
    impl ExternalRiskProvider for FakeProvider {
        fn name(&self) -> &'static str {
            self.label
        }
        async fn assess(&self, mint: &Pubkey) -> BotResult<ExternalSignal> {
            let call = self.calls.fetch_add(1, Ordering::SeqCst);
            if call < self.fail_first.load(Ordering::SeqCst) {
                return Err(BotError::http(format!("fake {} down", self.label)));
            }
            Ok(ExternalSignal {
                flagged: self.flagged,
                source: self.label.to_string(),
                detail: format!("mint {mint}"),
            })
        }
    }

    fn now() -> DateTime<Utc> {
        Utc::now()
    }

    #[tokio::test]
    async fn disabled_client_never_queries() {
        let provider = FakeProvider::ok("p", true);
        let _ = provider.clone(); // keep the Arc typed
        let client = ExternalRiskClient::disabled();
        assert_eq!(client.evaluate(&Pubkey::new_unique(), now()).await, None);
        assert_eq!(provider.calls(), 0);
    }

    #[tokio::test]
    async fn first_answer_wins_and_is_cached() {
        let a = FakeProvider::ok("a", false);
        let b = FakeProvider::ok("b", true);
        let client = ExternalRiskClient::enabled_with(
            vec![a.clone(), b.clone()],
            Duration::from_secs(300),
            3,
            Duration::from_secs(60),
        );
        let mint = Pubkey::new_unique();
        assert_eq!(client.evaluate(&mint, now()).await, Some(false));
        assert_eq!(a.calls(), 1);
        assert_eq!(b.calls(), 0, "second provider not consulted");
        // Cached: no new calls.
        assert_eq!(client.evaluate(&mint, now()).await, Some(false));
        assert_eq!(a.calls(), 1);
    }

    #[tokio::test]
    async fn failing_provider_falls_through_to_the_next() {
        let a = FakeProvider::failing("a", u32::MAX);
        let b = FakeProvider::ok("b", true);
        let client = ExternalRiskClient::enabled_with(
            vec![a.clone(), b.clone()],
            Duration::from_secs(300),
            3,
            Duration::from_secs(60),
        );
        assert_eq!(
            client.evaluate(&Pubkey::new_unique(), now()).await,
            Some(true)
        );
    }

    #[tokio::test]
    async fn every_provider_failing_is_a_skip_not_a_verdict() {
        let a = FakeProvider::failing("a", u32::MAX);
        let b = FakeProvider::failing("b", u32::MAX);
        let client = ExternalRiskClient::enabled_with(
            vec![a, b],
            Duration::from_secs(300),
            3,
            Duration::from_secs(60),
        );
        assert_eq!(client.evaluate(&Pubkey::new_unique(), now()).await, None);
    }

    #[tokio::test]
    async fn breaker_sheds_a_dead_provider_after_threshold() {
        let a = FakeProvider::failing("a", u32::MAX);
        let client = ExternalRiskClient::enabled_with(
            vec![a.clone()],
            Duration::from_secs(300),
            2,
            Duration::from_secs(3_600),
        );
        // Two failures trip the breaker.
        assert_eq!(client.evaluate(&Pubkey::new_unique(), now()).await, None);
        assert_eq!(client.evaluate(&Pubkey::new_unique(), now()).await, None);
        assert_eq!(a.calls(), 2);
        // Third evaluation: provider is shed (no call), still a skip.
        assert_eq!(client.evaluate(&Pubkey::new_unique(), now()).await, None);
        assert_eq!(a.calls(), 2, "breaker shed the call");
    }

    #[tokio::test]
    async fn cache_expires_after_ttl() {
        let a = FakeProvider::ok("a", true);
        let client = ExternalRiskClient::enabled_with(
            vec![a.clone()],
            Duration::from_secs(60),
            3,
            Duration::from_secs(60),
        );
        let mint = Pubkey::new_unique();
        let t0 = now();
        assert_eq!(client.evaluate(&mint, t0).await, Some(true));
        let t1 = t0 + chrono::Duration::seconds(120);
        assert_eq!(client.evaluate(&mint, t1).await, Some(true));
        assert_eq!(a.calls(), 2, "expired cache re-queries");
    }

    #[test]
    fn http_provider_requires_a_mint_placeholder() {
        let parse: fn(&serde_json::Value) -> BotResult<ExternalSignal> = |_| {
            Ok(ExternalSignal {
                flagged: false,
                source: "x".into(),
                detail: String::new(),
            })
        };
        assert!(HttpJsonProvider::new("x", "https://api.example/{mint}", parse).is_ok());
        assert!(HttpJsonProvider::new("x", "https://api.example/static", parse).is_err());
    }
}
