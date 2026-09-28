//! Async RPC wrapper: provider pool with automatic failover, classified
//! retries with jittered backoff, blockhash cache, confirmation tracking with
//! blockhash-expiry detection, and the raw JSON-RPC escape hatch used for
//! provider-specific methods.
//!
//! The transport (endpoints, health, breaker, rate limits) lives in
//! [`crate::provider`]; this file is the typed API the rest of the suite
//! talks to. Every method routes through [`Rpc::retry`], which asks the pool
//! for a provider per attempt, so a call that starts on a dying primary
//! finishes on a fallback without the caller noticing.

use std::sync::Arc;
use std::time::{Duration, Instant};

use base64::Engine;
use serde_json::json;
use solana_client::client_error::ClientError;
use solana_client::nonblocking::rpc_client::RpcClient;
use solana_client::rpc_client::GetConfirmedSignaturesForAddress2Config;
use solana_client::rpc_config::{
    RpcAccountInfoConfig, RpcBlockConfig, RpcProgramAccountsConfig, RpcSendTransactionConfig,
    RpcSignaturesForAddressConfig, RpcSimulateTransactionConfig, RpcTransactionConfig,
};
use solana_client::rpc_filter::{Memcmp, MemcmpEncodedBytes, RpcFilterType};
use solana_client::rpc_request::RpcRequest;
use solana_client::rpc_response::{Response, RpcSimulateTransactionResult};
use solana_sdk::account::Account;
use solana_sdk::commitment_config::{CommitmentConfig, CommitmentLevel};
use solana_sdk::hash::Hash;
use solana_sdk::pubkey::Pubkey;
use solana_sdk::signature::Signature;
use solana_sdk::transaction::VersionedTransaction;
use solana_transaction_status::{
    EncodedConfirmedTransactionWithStatusMeta, TransactionDetails, UiConfirmedBlock,
    UiTransactionEncoding,
};
use tokio::sync::RwLock;
use tracing::{debug, warn};

use bot_core::config::NetworkConfig;
use bot_core::error::{BotError, BotResult};

use crate::cache::AccountCache;
use crate::consts::*;
use crate::provider::{
    classify_client_error, PoolConfig, ProviderPool, ProviderStatus, RetryPolicy, RpcErrorClass,
};

/// One signature plus the metadata we need from `getSignaturesForAddress`.
#[derive(Debug, Clone)]
pub struct SignatureInfo {
    pub signature: Signature,
    pub slot: u64,
    pub err: Option<String>,
    pub block_time: Option<i64>,
    pub memo: Option<String>,
}

impl SignatureInfo {
    pub fn succeeded(&self) -> bool {
        self.err.is_none()
    }
}

/// Latest blockhash plus its expiry height.
#[derive(Debug, Clone, Copy)]
pub struct Blockhash {
    pub blockhash: Hash,
    pub last_valid_block_height: u64,
    pub fetched_at: Instant,
}

impl Blockhash {
    /// Wall-clock age of this blockhash on our side.
    pub fn age(&self) -> Duration {
        self.fetched_at.elapsed()
    }

    /// Freshness check used before signing: younger than `max_age`.
    pub fn is_fresh(&self, max_age: Duration) -> bool {
        self.age() <= max_age
    }
}

/// A cached blockhash. Sniping needs one per transaction and a fresh
/// `getLatestBlockhash` round trip costs 50–200 ms, which is most of the
/// 1-second budget, so we cache it for a short, configurable window.
struct BlockhashCache {
    value: RwLock<Option<Blockhash>>,
    max_age: Duration,
}

impl BlockhashCache {
    fn new(max_age: Duration) -> Self {
        BlockhashCache {
            value: RwLock::new(None),
            max_age,
        }
    }

    async fn get(&self) -> Option<Blockhash> {
        let guard = self.value.read().await;
        let cached = (*guard)?;
        if cached.fetched_at.elapsed() <= self.max_age {
            Some(cached)
        } else {
            None
        }
    }

    async fn set(&self, bh: Blockhash) {
        *self.value.write().await = Some(bh);
    }

    async fn invalidate(&self) {
        *self.value.write().await = None;
    }
}

/// A classified RPC failure (what the retry loop gave up on). Carries the
/// class so the executor can tell "the node rejected it" from "we never
/// heard back" without parsing text.
#[derive(Debug, Clone)]
pub struct RpcFailure {
    pub class: RpcErrorClass,
    /// Sanitised (no URLs / keys) message, prefixed with the method name.
    pub message: String,
    /// Attempts made before giving up.
    pub attempts: u32,
    /// Label of the provider that produced the final error.
    pub provider: String,
}

impl std::fmt::Display for RpcFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl From<RpcFailure> for BotError {
    fn from(e: RpcFailure) -> Self {
        BotError::rpc(e.message)
    }
}

#[derive(Clone)]
pub struct Rpc {
    /// Shared endpoint pool (health, breaker, retry policy).
    pool: Arc<ProviderPool>,
    /// Index of the provider this handle prefers. `Rpc::new` hands out
    /// origin 0; `failover()` returns a handle for the next index, which is
    /// how the executor fans a broadcast across every endpoint.
    origin: usize,
    commitment: CommitmentConfig,
    timeout: Duration,
    cache: Arc<BlockhashCache>,
    /// Warm cache for semi-static accounts (pump Global, mint owners, ATA
    /// existence). Only the `*_cached` methods consult it; price-bearing
    /// reads keep using the uncached ones. TTL 0 disables it entirely.
    account_cache: Arc<AccountCache>,
    account_cache_ttl: Duration,
}

impl Rpc {
    /// Build a client from the network config: every configured endpoint
    /// joins the pool, the retry/backoff/breaker settings come from
    /// `[network]`.
    pub fn new(cfg: &NetworkConfig) -> BotResult<Self> {
        let pool = ProviderPool::new(PoolConfig::from_network(cfg))?;
        let rpc = Self::from_pool(Arc::new(pool), 0);
        Ok(rpc.with_account_cache(
            Duration::from_millis(cfg.account_cache_ttl_ms),
            cfg.account_cache_max_entries,
        ))
    }

    /// Attach (or reconfigure) the warm account cache. `ttl == 0` disables
    /// cached reads entirely — every `*_cached` call becomes a plain fetch.
    pub fn with_account_cache(mut self, ttl: Duration, max_entries: usize) -> Self {
        self.account_cache = Arc::new(AccountCache::new(max_entries));
        self.account_cache_ttl = ttl;
        self
    }

    /// Hand-built client (tests, tools). Keeps the pre-pool retry timing
    /// (`RetryPolicy::legacy`): `max_retries` fixed-backoff retries, no
    /// jitter, breaker threshold 3 with a 5 s cooldown.
    pub fn with_urls(
        url: String,
        ws_url: String,
        fallbacks: Vec<String>,
        commitment: CommitmentConfig,
        max_retries: u32,
        timeout: Duration,
    ) -> BotResult<Self> {
        if url.trim().is_empty() {
            return Err(BotError::config("rpc_url is empty"));
        }
        let mut urls = Vec::with_capacity(1 + fallbacks.len());
        urls.push(url);
        urls.extend(fallbacks);
        let pool = ProviderPool::new(PoolConfig {
            urls,
            ws_url,
            commitment,
            timeout,
            failure_threshold: 3,
            cooldown: Duration::from_secs(5),
            retry: RetryPolicy::legacy(max_retries),
        })?;
        Ok(Self::from_pool(Arc::new(pool), 0))
    }

    /// Wrap an existing pool. `origin` is clamped to the pool size.
    pub fn from_pool(pool: Arc<ProviderPool>, origin: usize) -> Self {
        let origin = origin.min(pool.len().saturating_sub(1));
        Rpc {
            commitment: pool.commitment(),
            timeout: pool.timeout(),
            // A blockhash stays valid for ~60–90 s; refreshing every second
            // keeps us well inside that while cutting a round trip per snipe.
            cache: Arc::new(BlockhashCache::new(Duration::from_secs(1))),
            // Off unless `new(cfg)`/`with_account_cache` opts in: direct
            // `with_urls` callers (tests, tools) keep exact-fetch semantics.
            account_cache: Arc::new(AccountCache::new(0)),
            account_cache_ttl: Duration::ZERO,
            pool,
            origin,
        }
    }

    /// HTTP URL of the provider this handle prefers.
    pub fn url(&self) -> &str {
        self.pool.provider(self.origin).url()
    }

    /// Websocket URL of the provider this handle prefers.
    pub fn ws_url(&self) -> &str {
        self.pool.provider(self.origin).ws_url()
    }

    /// Safe-to-log label of the preferred provider (`"{index}:{host}"`).
    pub fn provider_label(&self) -> &str {
        self.pool.provider(self.origin).label()
    }

    pub fn commitment(&self) -> CommitmentConfig {
        self.commitment
    }

    /// Per-request timeout.
    pub fn timeout(&self) -> Duration {
        self.timeout
    }

    /// The shared endpoint pool.
    pub fn pool(&self) -> &Arc<ProviderPool> {
        &self.pool
    }

    /// Retry policy in force for this client.
    pub fn retry_policy(&self) -> RetryPolicy {
        *self.pool.retry()
    }

    /// Number of configured endpoints.
    pub fn provider_count(&self) -> usize {
        self.pool.len()
    }

    /// Endpoints currently routable (not cooling down).
    pub fn healthy_provider_count(&self) -> usize {
        self.pool.healthy_count()
    }

    /// Health of every endpoint, primary first (dashboard / health API).
    pub fn provider_status(&self) -> Vec<ProviderStatus> {
        self.pool.snapshot()
    }

    /// Direct access to the preferred provider's client for anything this
    /// wrapper does not cover. Bypasses retries and failover.
    pub fn raw(&self) -> &RpcClient {
        self.pool.provider(self.origin).client()
    }

    /// True when the preferred endpoint has failed enough times that the
    /// caller should consider switching (its breaker is open or it has hit
    /// the failure threshold). Exposed for the dashboard health widget.
    pub fn unhealthy(&self) -> bool {
        let p = self.pool.provider(self.origin);
        !p.is_healthy() || p.consecutive_failures() >= self.pool.failure_threshold()
    }

    /// Consecutive failure count of the preferred endpoint.
    pub fn failure_count(&self) -> u64 {
        self.pool.provider(self.origin).consecutive_failures() as u64
    }

    /// Outer guard around one attempt: the HTTP client has its own timeout,
    /// this catches a transport that stalls without ever timing out.
    fn attempt_timeout(&self) -> Duration {
        self.timeout + Duration::from_millis(500)
    }

    /// Run `op` with classified retries and provider failover.
    ///
    /// Per attempt the pool picks the best untried provider (origin first,
    /// then fallbacks in configured order, skipping open breakers). A
    /// retryable failure moves straight to the next healthy provider when
    /// one exists; otherwise the jittered exponential backoff applies (a
    /// 429 `Retry-After` is a floor). Permanent errors return immediately.
    async fn retry_classified<'a, T, F, Fut>(
        &'a self,
        what: &str,
        mut op: F,
    ) -> Result<T, RpcFailure>
    where
        F: FnMut(&'a RpcClient) -> Fut,
        Fut: std::future::Future<Output = Result<T, ClientError>>,
    {
        let reg = bot_core::obs::metrics::global();
        let policy = *self.pool.retry();
        let max_attempts = policy.max_attempts();
        let mut attempt = 0u32;
        let mut tried: Vec<usize> = Vec::with_capacity(2);
        loop {
            let idx = match self.pool.route(self.origin, &tried) {
                Some(i) => i,
                None => {
                    // Every provider has been tried this call; start over
                    // (the retry budget, not the provider list, bounds us).
                    tried.clear();
                    self.pool.route(self.origin, &tried).unwrap_or(self.origin)
                }
            };
            let provider = self.pool.provider(idx);
            if idx != self.origin {
                reg.counter(
                    "bot_rpc_failover_total",
                    "RPC attempts routed away from the preferred provider.",
                    &[("method", what)],
                )
                .inc();
            }
            let started = Instant::now();
            let outcome = tokio::time::timeout(self.attempt_timeout(), op(provider.client())).await;
            let elapsed = started.elapsed();
            reg.histogram(
                "bot_rpc_attempt_duration_ms",
                "Duration of a single RPC attempt in milliseconds.",
                &[("method", what)],
                bot_core::obs::metrics::LATENCY_BUCKETS_MS,
            )
            .observe(elapsed.as_millis() as u64);

            let (class, raw_msg) = match outcome {
                Ok(Ok(v)) => {
                    self.pool.record_success(idx, elapsed);
                    reg.counter(
                        "bot_rpc_requests_total",
                        "Completed RPC requests (after retries) by outcome.",
                        &[("method", what), ("outcome", "ok")],
                    )
                    .inc();
                    return Ok(v);
                }
                Ok(Err(e)) => (classify_client_error(&e), e.to_string()),
                Err(_) => (
                    RpcErrorClass::Timeout,
                    format!(
                        "attempt timed out after {} ms",
                        self.attempt_timeout().as_millis()
                    ),
                ),
            };
            self.pool.record_failure(idx, class, &raw_msg);
            let message = self.pool.sanitize(&raw_msg);
            attempt += 1;
            tried.push(idx);

            if !class.retryable() || attempt >= max_attempts {
                // outcome is a closed set: fatal = non-retryable error,
                // exhausted = retryable but the attempt budget ran out.
                reg.counter(
                    "bot_rpc_requests_total",
                    "Completed RPC requests (after retries) by outcome.",
                    &[
                        ("method", what),
                        (
                            "outcome",
                            if class.retryable() {
                                "exhausted"
                            } else {
                                "fatal"
                            },
                        ),
                    ],
                )
                .inc();
                return Err(RpcFailure {
                    class,
                    message: format!("{what}: {message}"),
                    attempts: attempt,
                    provider: provider.label().to_string(),
                });
            }
            reg.counter(
                "bot_rpc_retries_total",
                "RPC attempts retried, by failure class.",
                &[("method", what), ("class", class.label())],
            )
            .inc();
            // A provider fault with another healthy endpoint available is
            // a failover, not a wait. Otherwise back off (jittered) so a
            // flapping node does not spin the hot loop.
            let backoff =
                if class.provider_fault() && self.pool.has_healthy_untried(self.origin, &tried) {
                    Duration::ZERO
                } else {
                    policy.delay(attempt, class.retry_after())
                };
            warn!(
                what,
                attempt,
                provider = provider.label(),
                class = class.label(),
                error = %message,
                backoff_ms = backoff.as_millis() as u64,
                "transient rpc error, retrying"
            );
            if !backoff.is_zero() {
                tokio::time::sleep(backoff).await;
            }
        }
    }

    /// `retry_classified` flattened into the crate's error type.
    async fn retry<'a, T, F, Fut>(&'a self, what: &str, op: F) -> BotResult<T>
    where
        F: FnMut(&'a RpcClient) -> Fut,
        Fut: std::future::Future<Output = Result<T, ClientError>>,
    {
        self.retry_classified(what, op)
            .await
            .map_err(BotError::from)
    }

    // ------------------------------------------------------------- basics --

    pub async fn get_version(&self) -> BotResult<String> {
        let v = self.retry("getVersion", |c| c.get_version()).await?;
        Ok(v.solana_core)
    }

    pub async fn get_slot(&self) -> BotResult<u64> {
        self.retry("getSlot", |c| c.get_slot()).await
    }

    /// Current block height at the client's commitment — the unit
    /// `last_valid_block_height` is expressed in.
    pub async fn get_block_height(&self) -> BotResult<u64> {
        self.retry("getBlockHeight", |c| c.get_block_height()).await
    }

    pub async fn health(&self) -> BotResult<String> {
        // `getHealth` returns the string "ok" or an error object.
        let v: serde_json::Value = self.send_raw(RpcRequest::GetHealth, json!([])).await?;
        Ok(v.as_str().unwrap_or("unknown").to_string())
    }

    pub async fn get_balance(&self, pubkey: &Pubkey) -> BotResult<u64> {
        self.retry("getBalance", |c| c.get_balance(pubkey)).await
    }

    pub async fn get_account(&self, pubkey: &Pubkey) -> BotResult<Option<Account>> {
        match self.retry("getAccount", |c| c.get_account(pubkey)).await {
            Ok(a) => Ok(Some(a)),
            // `getAccount` errors when the account does not exist; that is a
            // normal, expected condition for us, not a failure.
            Err(e) if e.to_string().contains("getAccount") => {
                debug!(%pubkey, "account not found");
                Ok(None)
            }
            Err(e) => Err(e),
        }
    }

    pub async fn account_exists(&self, pubkey: &Pubkey) -> BotResult<bool> {
        Ok(self.get_account(pubkey).await?.is_some())
    }

    // ------------------------------------------------- warm account cache --
    //
    // Only for *semi-static* accounts (pump Global, mint owners, ATA
    // existence). Price-bearing accounts (bonding curves, pools) must keep
    // using the uncached readers above. All of these become plain fetches
    // when the cache TTL is zero.

    /// The warm account cache handle (for metrics/tests).
    pub fn account_cache(&self) -> &AccountCache {
        &self.account_cache
    }

    /// TTL configured for cached reads; `Duration::ZERO` = cache disabled.
    pub fn account_cache_ttl(&self) -> Duration {
        self.account_cache_ttl
    }

    /// `get_account` backed by the warm cache (positive entries only — a
    /// missing account always goes to the network, because accounts appear
    /// mid-flight: brand-new mints, just-created ATAs).
    pub async fn get_account_cached(&self, pubkey: &Pubkey) -> BotResult<Option<Account>> {
        if !self.account_cache_ttl.is_zero() {
            if let Some(hit) = self.account_cache.get(pubkey, self.account_cache_ttl).await {
                return Ok(Some(hit));
            }
        }
        let account = self.get_account(pubkey).await?;
        if !self.account_cache_ttl.is_zero() {
            if let Some(a) = &account {
                self.account_cache.insert(*pubkey, a.clone()).await;
            }
        }
        Ok(account)
    }

    /// Batch variant: cached keys are served locally, the rest are fetched in
    /// a single `getMultipleAccounts` round trip and written back.
    pub async fn get_multiple_accounts_cached(
        &self,
        pubkeys: &[Pubkey],
    ) -> BotResult<Vec<Option<Account>>> {
        if self.account_cache_ttl.is_zero() || pubkeys.is_empty() {
            return self.get_multiple_accounts(pubkeys).await;
        }
        let mut out: Vec<Option<Account>> = Vec::with_capacity(pubkeys.len());
        let mut misses: Vec<usize> = Vec::new();
        for (i, key) in pubkeys.iter().enumerate() {
            match self.account_cache.get(key, self.account_cache_ttl).await {
                Some(hit) => out.push(Some(hit)),
                None => {
                    out.push(None);
                    misses.push(i);
                }
            }
        }
        if misses.is_empty() {
            return Ok(out);
        }
        let keys: Vec<Pubkey> = misses.iter().map(|i| pubkeys[*i]).collect();
        let fetched = self.get_multiple_accounts(&keys).await?;
        for (slot, account) in misses.into_iter().zip(fetched) {
            if let Some(a) = &account {
                self.account_cache.insert(pubkeys[slot], a.clone()).await;
            }
            out[slot] = account;
        }
        Ok(out)
    }

    /// `account_exists` backed by the warm cache. Only `true` answers are
    /// cached (an account that does not exist yet may appear at any moment),
    /// so this is safe for "did my ATA get created" style checks.
    pub async fn account_exists_cached(&self, pubkey: &Pubkey) -> BotResult<bool> {
        Ok(self.get_account_cached(pubkey).await?.is_some())
    }

    pub async fn get_account_data(&self, pubkey: &Pubkey) -> BotResult<Vec<u8>> {
        self.retry("getAccountData", |c| c.get_account_data(pubkey))
            .await
    }

    pub async fn get_multiple_accounts(
        &self,
        pubkeys: &[Pubkey],
    ) -> BotResult<Vec<Option<Account>>> {
        if pubkeys.is_empty() {
            return Ok(Vec::new());
        }
        self.retry("getMultipleAccounts", |c| c.get_multiple_accounts(pubkeys))
            .await
    }

    /// Program accounts filtered by a discriminator prefix (Anchor style).
    pub async fn get_program_accounts_with_discriminator(
        &self,
        program_id: &Pubkey,
        discriminator: &[u8; 8],
    ) -> BotResult<Vec<(Pubkey, Account)>> {
        let config = RpcProgramAccountsConfig {
            filters: Some(vec![RpcFilterType::Memcmp(Memcmp::new(
                0,
                MemcmpEncodedBytes::Bytes(discriminator.to_vec()),
            ))]),
            account_config: RpcAccountInfoConfig {
                encoding: Some(solana_account_decoder::UiAccountEncoding::Base64),
                data_slice: None,
                commitment: Some(self.commitment),
                min_context_slot: None,
            },
            with_context: Some(false),
            sort_results: None,
        };
        self.retry("getProgramAccounts", |c| {
            c.get_program_accounts_with_config(program_id, config.clone())
        })
        .await
    }

    // ---------------------------------------------------------- blockhash --

    /// Cached latest blockhash. Pass `force_refresh` after a send failure to
    /// bypass the cache.
    pub async fn latest_blockhash(&self, force_refresh: bool) -> BotResult<Blockhash> {
        if !force_refresh {
            if let Some(cached) = self.cache.get().await {
                return Ok(cached);
            }
        }
        let (blockhash, last_valid_block_height) = self
            .retry("getLatestBlockhash", |c| {
                c.get_latest_blockhash_with_commitment(CommitmentConfig::confirmed())
            })
            .await?;
        let bh = Blockhash {
            blockhash,
            last_valid_block_height,
            fetched_at: Instant::now(),
        };
        self.cache.set(bh).await;
        Ok(bh)
    }

    /// A blockhash no older than `max_age` on our clock: serves the cached
    /// one when it qualifies, otherwise refreshes. This is the freshness
    /// gate the executor applies right before signing.
    pub async fn fresh_blockhash(&self, max_age: Duration) -> BotResult<Blockhash> {
        if let Some(cached) = self.cache.get().await {
            if cached.is_fresh(max_age) {
                return Ok(cached);
            }
        }
        self.latest_blockhash(true).await
    }

    pub async fn invalidate_blockhash(&self) {
        self.cache.invalidate().await;
    }

    pub async fn is_blockhash_valid(&self, blockhash: &Hash) -> BotResult<bool> {
        self.retry("isBlockhashValid", |c| {
            c.is_blockhash_valid(blockhash, CommitmentConfig::confirmed())
        })
        .await
    }

    // ------------------------------------------------------ priority fees --

    /// Recent per-slot prioritization fees (micro-lamports per CU) for the
    /// given writable accounts (empty = cluster-wide). Sorted ascending so
    /// callers can take a percentile directly.
    pub async fn recent_prioritization_fees(&self, accounts: &[Pubkey]) -> BotResult<Vec<u64>> {
        let fees = self
            .retry("getRecentPrioritizationFees", |c| {
                c.get_recent_prioritization_fees(accounts)
            })
            .await?;
        let mut out: Vec<u64> = fees.into_iter().map(|f| f.prioritization_fee).collect();
        out.sort_unstable();
        Ok(out)
    }

    // -------------------------------------------------------- transactions --

    /// Recent signatures for an address, newest first.
    pub async fn signatures_for_address(
        &self,
        address: &Pubkey,
        limit: usize,
        before: Option<Signature>,
    ) -> BotResult<Vec<SignatureInfo>> {
        let config = RpcSignaturesForAddressConfig {
            before: before.map(|s| s.to_string()),
            until: None,
            limit: Some(limit.clamp(1, 1000)),
            commitment: Some(self.commitment),
            min_context_slot: None,
        };
        let raw = self
            .retry("getSignaturesForAddress", |c| {
                c.get_signatures_for_address_with_config(address, to_get_confirmed(&config))
            })
            .await?;
        Ok(raw
            .into_iter()
            .filter_map(|s| {
                let signature = s.signature.parse::<Signature>().ok()?;
                Some(SignatureInfo {
                    signature,
                    slot: s.slot,
                    err: s.err.as_ref().map(|e| e.to_string()),
                    block_time: s.block_time,
                    memo: s.memo,
                })
            })
            .collect())
    }

    /// Full transaction with base64 encoding + versioned-transaction support.
    pub async fn get_transaction(
        &self,
        signature: &Signature,
    ) -> BotResult<Option<EncodedConfirmedTransactionWithStatusMeta>> {
        let config = RpcTransactionConfig {
            encoding: Some(UiTransactionEncoding::Base64),
            commitment: Some(CommitmentConfig::confirmed()),
            max_supported_transaction_version: Some(0),
        };
        // A node answers `result: null` for a signature it does not know
        // (not yet confirmed, or dropped). Fetch the raw value so that case is
        // a clean `Ok(None)` — the typed client would turn the null into a
        // deserialisation error, which would look like a transport fault to
        // the confirmation poller and hide blockhash expiry behind it.
        let params = json!([signature.to_string(), config]);
        match self
            .retry("getTransaction", |c| {
                c.send::<serde_json::Value>(RpcRequest::GetTransaction, params.clone())
            })
            .await
        {
            Ok(serde_json::Value::Null) => Ok(None),
            Ok(value) => serde_json::from_value::<EncodedConfirmedTransactionWithStatusMeta>(value)
                .map(Some)
                .map_err(|e| BotError::rpc(format!("getTransaction decode: {e}"))),
            Err(e) => {
                // Some providers reject unknown signatures with an explicit
                // error instead of null; that is still "no data".
                let msg = e.to_string();
                if msg.contains("not found") || msg.contains("Not found") {
                    Ok(None)
                } else {
                    Err(e)
                }
            }
        }
    }

    /// Fetch a whole block with full transaction bodies. Used by the copy
    /// trader's polling fallback feed.
    pub async fn get_block(&self, slot: u64) -> BotResult<UiConfirmedBlock> {
        let config = RpcBlockConfig {
            encoding: Some(UiTransactionEncoding::Base64),
            transaction_details: Some(TransactionDetails::Full),
            rewards: Some(false),
            commitment: Some(CommitmentConfig::confirmed()),
            max_supported_transaction_version: Some(0),
        };
        self.retry("getBlock", |c| c.get_block_with_config(slot, config))
            .await
    }

    /// Simulate without signature verification. This is the dry-run path used
    /// by `execution.mode = "simulate"` and by pre-flight checks.
    pub async fn simulate(
        &self,
        tx: &VersionedTransaction,
    ) -> BotResult<Response<RpcSimulateTransactionResult>> {
        let config = RpcSimulateTransactionConfig {
            sig_verify: false,
            replace_recent_blockhash: true,
            commitment: Some(self.commitment),
            encoding: Some(UiTransactionEncoding::Base64),
            accounts: None,
            min_context_slot: None,
            inner_instructions: true,
        };
        self.retry("simulateTransaction", |c| {
            c.simulate_transaction_with_config(tx, config.clone())
        })
        .await
    }

    /// Broadcast a signed transaction. `skip_preflight` is what a sniper wants:
    /// preflight costs a round trip and we simulate separately anyway.
    pub async fn send_transaction(&self, tx: &VersionedTransaction) -> BotResult<Signature> {
        self.send_transaction_classified(tx)
            .await
            .map_err(BotError::from)
    }

    /// `send_transaction` that keeps the failure class, so the executor can
    /// distinguish an ambiguous outcome (timeout / transport: the tx may
    /// still land) from a definite rejection without parsing text.
    /// Re-broadcasting the same signed bytes is idempotent on chain, so the
    /// retry loop is safe here.
    pub async fn send_transaction_classified(
        &self,
        tx: &VersionedTransaction,
    ) -> Result<Signature, RpcFailure> {
        let config = RpcSendTransactionConfig {
            skip_preflight: true,
            preflight_commitment: Some(CommitmentLevel::Processed),
            encoding: Some(UiTransactionEncoding::Base64),
            max_retries: Some(0),
            min_context_slot: None,
        };
        self.retry_classified("sendTransaction", |c| {
            c.send_transaction_with_config(tx, config)
        })
        .await
    }

    /// Poll until the signature is confirmed/finalised or the deadline passes.
    pub async fn confirm(
        &self,
        signature: &Signature,
        timeout: Duration,
        poll: Duration,
    ) -> BotResult<ConfirmOutcome> {
        self.confirm_tracked(signature, None, timeout, poll).await
    }

    /// `confirm` with blockhash-expiry detection: when the cluster's block
    /// height passes `last_valid_block_height` and the transaction is still
    /// not visible, it can never land and the outcome is `Expired` — the
    /// executor can rebuild right away instead of burning the whole
    /// confirmation timeout. Height is sampled at most every 1.5 s so the
    /// poll loop stays cheap.
    pub async fn confirm_tracked(
        &self,
        signature: &Signature,
        last_valid_block_height: Option<u64>,
        timeout: Duration,
        poll: Duration,
    ) -> BotResult<ConfirmOutcome> {
        let deadline = Instant::now() + timeout;
        let mut last_height_check: Option<Instant> = None;
        loop {
            if Instant::now() > deadline {
                return Ok(ConfirmOutcome::Timeout);
            }
            match self.get_transaction(signature).await {
                Ok(Some(tx)) => return Ok(outcome_from_transaction(&tx)),
                Ok(None) => {
                    if let Some(lvbh) = last_valid_block_height {
                        let due = last_height_check
                            .map(|t| t.elapsed() >= Duration::from_millis(1_500))
                            .unwrap_or(true);
                        if due {
                            last_height_check = Some(Instant::now());
                            if let Ok(height) = self.get_block_height().await {
                                if height > lvbh {
                                    // Double-check: the tx may have landed in
                                    // one of the last valid blocks and only
                                    // just become visible.
                                    if let Ok(Some(tx)) = self.get_transaction(signature).await {
                                        return Ok(outcome_from_transaction(&tx));
                                    }
                                    return Ok(ConfirmOutcome::Expired {
                                        last_valid_block_height: lvbh,
                                        block_height: height,
                                    });
                                }
                            }
                        }
                    }
                    tokio::time::sleep(poll).await
                }
                Err(e) => {
                    debug!(error = %e, "confirm poll error, will retry");
                    tokio::time::sleep(poll).await;
                }
            }
        }
    }

    // ------------------------------------------------------------ raw rpc --

    /// Escape hatch for provider-specific methods (`getAsset`, Helius DAS,
    /// Yellowstone, …). Same retry/failover policy as the typed methods.
    pub async fn send_raw(
        &self,
        request: RpcRequest,
        params: serde_json::Value,
    ) -> BotResult<serde_json::Value> {
        let what = request.to_string();
        self.retry(&what, |c| {
            c.send::<serde_json::Value>(request, params.clone())
        })
        .await
    }

    /// Fetch the DAS (Digital Asset Standard) metadata for a mint. Returns
    /// `None` when the RPC does not implement `getAsset`, which is the normal
    /// case on a vanilla node.
    pub async fn get_asset(&self, mint: &Pubkey) -> BotResult<Option<DasAsset>> {
        let v = match self
            .send_raw(
                RpcRequest::Custom { method: "getAsset" },
                json!([{ "id": mint.to_string() }]),
            )
            .await
        {
            Ok(v) => v,
            Err(e) => {
                debug!(error = %e, "getAsset unavailable on this rpc");
                return Ok(None);
            }
        };
        Ok(serde_json::from_value::<DasAsset>(v).ok())
    }

    /// SPL token supply + decimals, via `getMint`-equivalent account read.
    pub async fn token_decimals(&self, mint: &Pubkey) -> BotResult<u8> {
        let data = self.get_account_data(mint).await?;
        // SPL Mint layout: mint_authority(36) supply(8) decimals(1) …
        if data.len() < 45 {
            return Err(BotError::solana(format!(
                "mint account too short: {} bytes",
                data.len()
            )));
        }
        Ok(data[44])
    }

    /// Which token program owns `mint` — SPL Token or Token-2022. Pump.fun now
    /// creates Token-2022 mints, and the ATA address differs between them, so
    /// this must be read rather than assumed.
    pub async fn token_program_of(&self, mint: &Pubkey) -> BotResult<Pubkey> {
        // A mint's owner (token program) never changes, so this read is safe
        // to warm-cache; brand-new mints still miss through to the network.
        match self.get_account_cached(mint).await? {
            Some(account) => Ok(account.owner),
            None => {
                // Brand-new mints may not be visible on `confirmed` yet; retry
                // once at processed commitment before defaulting.
                let data = self
                    .send_raw(
                        RpcRequest::GetAccountInfo,
                        json!([mint.to_string(), {"encoding": "base64", "commitment": "processed"}]),
                    )
                    .await
                    .ok()
                    .and_then(|v| v.get("value").and_then(|x| x.get("owner")).and_then(|o| o.as_str()).map(String::from));
                match data {
                    Some(owner) if owner == TOKEN_2022_PROGRAM.to_string() => {
                        Ok(*TOKEN_2022_PROGRAM)
                    }
                    _ => Ok(*TOKEN_PROGRAM),
                }
            }
        }
    }

    /// Raw account read at `processed` commitment, used for freshly created
    /// accounts that `confirmed` has not caught up with yet.
    pub async fn get_account_processed(&self, pubkey: &Pubkey) -> BotResult<Option<Vec<u8>>> {
        let v: serde_json::Value = self
            .send_raw(
                RpcRequest::GetAccountInfo,
                json!([pubkey.to_string(), {"encoding": "base64", "commitment": "processed"}]),
            )
            .await?;
        let data = v
            .get("value")
            .and_then(|x| x.get("data"))
            .and_then(|d| d.as_array())
            .and_then(|a| a.first())
            .and_then(|s| s.as_str());
        Ok(data.and_then(|b64| base64::engine::general_purpose::STANDARD.decode(b64).ok()))
    }

    /// A handle preferring the next endpoint in the pool.
    ///
    /// Used by the executor to fan a broadcast across every endpoint and by
    /// the supervisor when `unhealthy()` stays true: rather than hammering a
    /// dead node, we hand out a client that starts elsewhere. The pool —
    /// health counters, breaker state, warm account cache — is shared; the
    /// blockhash cache stays per-handle because hash freshness is
    /// node-local.
    pub fn failover(&self) -> Option<Rpc> {
        let next = self.origin + 1;
        if next >= self.pool.len() {
            return None;
        }
        Some(Rpc {
            pool: Arc::clone(&self.pool),
            origin: next,
            commitment: self.commitment,
            timeout: self.timeout,
            cache: Arc::new(BlockhashCache::new(Duration::from_secs(1))),
            account_cache: Arc::clone(&self.account_cache),
            account_cache_ttl: self.account_cache_ttl,
        })
    }
}

/// Map a fetched transaction onto the confirmation outcome.
fn outcome_from_transaction(tx: &EncodedConfirmedTransactionWithStatusMeta) -> ConfirmOutcome {
    let err = tx
        .transaction
        .meta
        .as_ref()
        .and_then(|m| m.err.as_ref().map(|e| e.to_string()));
    let logs = match tx.transaction.meta.as_ref().map(|m| m.log_messages.clone()) {
        Some(solana_transaction_status::option_serializer::OptionSerializer::Some(l)) => l,
        _ => Vec::new(),
    };
    match err {
        Some(e) => ConfirmOutcome::Failed { error: e, logs },
        None => ConfirmOutcome::Confirmed {
            slot: tx.slot,
            logs,
            fee: tx.transaction.meta.as_ref().map(|m| m.fee).unwrap_or(0),
        },
    }
}

#[derive(Debug, Clone)]
pub enum ConfirmOutcome {
    Confirmed {
        slot: u64,
        logs: Vec<String>,
        fee: u64,
    },
    Failed {
        error: String,
        logs: Vec<String>,
    },
    /// The deadline passed with the transaction still unseen. It may yet
    /// land (until its blockhash expires) — an ambiguous outcome.
    Timeout,
    /// The blockhash expired before the transaction was seen: it can never
    /// land. A definite, fee-free failure that is safe to rebuild.
    Expired {
        last_valid_block_height: u64,
        block_height: u64,
    },
}

impl ConfirmOutcome {
    pub fn ok(&self) -> bool {
        matches!(self, ConfirmOutcome::Confirmed { .. })
    }

    /// True when the transaction can still land (only `Timeout`).
    pub fn is_ambiguous(&self) -> bool {
        matches!(self, ConfirmOutcome::Timeout)
    }

    pub fn summary(&self) -> String {
        match self {
            ConfirmOutcome::Confirmed { slot, fee, .. } => {
                format!("confirmed in slot {slot} (fee {fee} lamports)")
            }
            ConfirmOutcome::Failed { error, .. } => format!("FAILED on chain: {error}"),
            ConfirmOutcome::Timeout => "timed out waiting for confirmation".into(),
            ConfirmOutcome::Expired {
                last_valid_block_height,
                block_height,
            } => format!(
                "blockhash expired before landing (last valid height {last_valid_block_height}, cluster at {block_height})"
            ),
        }
    }
}

/// Minimal DAS `getAsset` response: the fields Module 1 uses for screening.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct DasAsset {
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub content: Option<DasContent>,
    #[serde(default)]
    pub authorities: Option<Vec<DasAuthority>>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct DasContent {
    #[serde(default)]
    pub metadata: Option<DasMetadata>,
    #[serde(default)]
    pub links: Option<DasLinks>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct DasMetadata {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub symbol: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct DasLinks {
    #[serde(default)]
    pub website: Option<String>,
    #[serde(default)]
    pub twitter: Option<String>,
    #[serde(default)]
    pub telegram: Option<String>,
    #[serde(default)]
    pub image: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct DasAuthority {
    #[serde(default)]
    pub address: Option<String>,
    #[serde(default)]
    pub scopes: Option<Vec<String>>,
}

impl DasAsset {
    pub fn social_count(&self) -> usize {
        let Some(links) = self.content.as_ref().and_then(|c| c.links.as_ref()) else {
            return 0;
        };
        [
            links.website.as_deref(),
            links.twitter.as_deref(),
            links.telegram.as_deref(),
        ]
        .iter()
        .filter(|v| v.is_some_and(|s| !s.trim().is_empty()))
        .count()
    }
}

/// `https://host` -> `wss://host`. The RPC client does not expose the ws url in
/// 2.x, so derive it; most providers use the same host for both.
pub fn http_to_ws(url: &str) -> String {
    if url.starts_with("wss://") || url.starts_with("ws://") {
        return url.to_string();
    }
    if let Some(rest) = url.strip_prefix("https://") {
        format!("wss://{rest}")
    } else if let Some(rest) = url.strip_prefix("http://") {
        format!("ws://{rest}")
    } else {
        url.to_string()
    }
}

pub fn parse_commitment(s: &str) -> CommitmentConfig {
    match s.trim().to_ascii_lowercase().as_str() {
        "processed" => CommitmentConfig::processed(),
        "finalized" => CommitmentConfig::finalized(),
        "confirmed" => CommitmentConfig::confirmed(),
        other => {
            warn!(
                commitment = other,
                "unknown commitment level, using confirmed"
            );
            CommitmentConfig::confirmed()
        }
    }
}

/// `get_signatures_for_address_with_config` in solana-client 2.3 takes the
/// legacy config type; translate between the two.
fn to_get_confirmed(c: &RpcSignaturesForAddressConfig) -> GetConfirmedSignaturesForAddress2Config {
    GetConfirmedSignaturesForAddress2Config {
        before: c.before.as_ref().and_then(|s| s.parse::<Signature>().ok()),
        until: c.until.as_ref().and_then(|s| s.parse::<Signature>().ok()),
        limit: c.limit,
        commitment: c.commitment,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    #[test]
    fn ws_url_is_derived_from_the_http_url() {
        assert_eq!(
            http_to_ws("https://api.mainnet-beta.solana.com"),
            "wss://api.mainnet-beta.solana.com"
        );
        assert_eq!(http_to_ws("http://localhost:8899"), "ws://localhost:8899");
        assert_eq!(http_to_ws("wss://already/ws"), "wss://already/ws");
    }

    #[test]
    fn commitment_parsing_defaults_safely() {
        assert_eq!(parse_commitment("processed"), CommitmentConfig::processed());
        assert_eq!(parse_commitment("FINALIZED"), CommitmentConfig::finalized());
        assert_eq!(parse_commitment("nonsense"), CommitmentConfig::confirmed());
    }

    #[tokio::test]
    async fn blockhash_cache_expires() {
        let cache = BlockhashCache::new(Duration::from_millis(20));
        assert!(cache.get().await.is_none());
        cache
            .set(Blockhash {
                blockhash: Hash::default(),
                last_valid_block_height: 1,
                fetched_at: Instant::now(),
            })
            .await;
        assert!(cache.get().await.is_some());
        tokio::time::sleep(Duration::from_millis(40)).await;
        assert!(
            cache.get().await.is_none(),
            "stale blockhash must not be reused"
        );
        cache.invalidate().await;
        assert!(cache.get().await.is_none());
    }

    #[test]
    fn failover_chain_advances() {
        let cfg = NetworkConfig {
            rpc_url: "https://primary.example.com".into(),
            rpc_url_fallbacks: vec![
                "https://a.example.com".into(),
                "https://b.example.com".into(),
            ],
            ..Default::default()
        };
        let rpc = Rpc::new(&cfg).unwrap();
        assert_eq!(rpc.url(), "https://primary.example.com");
        assert_eq!(rpc.ws_url(), "wss://primary.example.com");
        assert!(!rpc.unhealthy());
        assert_eq!(rpc.provider_count(), 3);
        assert_eq!(rpc.healthy_provider_count(), 3);

        let next = rpc.failover().expect("should have a fallback");
        assert_eq!(next.url(), "https://a.example.com");
        assert_eq!(next.ws_url(), "wss://a.example.com");
        let last = next.failover().expect("should have a second fallback");
        assert_eq!(last.url(), "https://b.example.com");
        assert!(last.failover().is_none());
        assert!(
            Arc::ptr_eq(rpc.pool(), last.pool()),
            "failover handles share health state"
        );
    }

    #[test]
    fn das_social_count_ignores_empty_links() {
        let asset: DasAsset = serde_json::from_value(json!({
            "id": "abc",
            "content": { "links": { "website": "https://x.com", "twitter": "", "telegram": null } }
        }))
        .unwrap();
        assert_eq!(asset.social_count(), 1);

        let none: DasAsset = serde_json::from_value(json!({})).unwrap();
        assert_eq!(none.social_count(), 0);
    }

    #[test]
    fn confirm_outcome_summary_is_human_readable() {
        assert!(ConfirmOutcome::Confirmed {
            slot: 5,
            logs: vec![],
            fee: 5000
        }
        .summary()
        .contains("slot 5"));
        assert!(ConfirmOutcome::Failed {
            error: "custom 6024".into(),
            logs: vec![]
        }
        .summary()
        .contains("6024"));
        assert!(!ConfirmOutcome::Timeout.ok());
        assert!(ConfirmOutcome::Timeout.is_ambiguous());
        let expired = ConfirmOutcome::Expired {
            last_valid_block_height: 10,
            block_height: 12,
        };
        assert!(!expired.ok() && !expired.is_ambiguous());
        assert!(expired.summary().contains("expired"));
    }

    /// Scripted JSON-RPC endpoint: answers each connection with the next
    /// canned response (last one repeats) and counts the hits.
    async fn spawn_scripted(
        responses: Vec<(&'static str, &'static str)>,
    ) -> (String, Arc<AtomicUsize>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = format!("http://{}", listener.local_addr().unwrap());
        let hits = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&hits);
        tokio::spawn(async move {
            loop {
                if let Ok((mut sock, _)) = listener.accept().await {
                    let n = counter.fetch_add(1, Ordering::SeqCst);
                    let (status, body) = responses[n.min(responses.len() - 1)];
                    let mut buf = [0u8; 8192];
                    let _ = sock.read(&mut buf).await;
                    let resp = format!(
                        "HTTP/1.1 {status}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                        body.len()
                    );
                    let _ = sock.write_all(resp.as_bytes()).await;
                    let _ = sock.shutdown().await;
                }
            }
        });
        (addr, hits)
    }

    /// Endpoint that accepts and drops every connection (transport fault).
    async fn spawn_black_hole() -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move {
            loop {
                let _ = listener.accept().await;
            }
        });
        addr
    }

    const SLOT_OK: (&str, &str) = ("200 OK", r#"{"jsonrpc":"2.0","result":777,"id":1}"#);

    #[tokio::test]
    async fn failing_primary_fails_over_to_the_fallback_within_one_call() {
        // RPC failure injection: the primary drops every connection, the
        // fallback answers. One `get_slot` must succeed and the primary's
        // health must reflect the fault.
        let dead = spawn_black_hole().await;
        let (alive, hits) = spawn_scripted(vec![SLOT_OK]).await;
        let rpc = Rpc::with_urls(
            dead,
            String::new(),
            vec![alive],
            CommitmentConfig::confirmed(),
            2,
            Duration::from_secs(2),
        )
        .unwrap();
        let slot = rpc.get_slot().await.expect("fallback serves the call");
        assert_eq!(slot, 777);
        assert_eq!(hits.load(Ordering::SeqCst), 1);
        assert_eq!(
            rpc.failure_count(),
            1,
            "primary recorded one transport failure"
        );
        let status = rpc.provider_status();
        assert_eq!(status[0].total_failures, 1);
        assert_eq!(status[0].last_failure_class.as_deref(), Some("transport"));
        assert_eq!(status[1].total_ok, 1);
    }

    #[tokio::test]
    async fn rate_limited_primary_is_cooled_down_and_skipped() {
        // Rate-limit injection: primary answers 429 with Retry-After; the
        // call must complete on the fallback immediately and later calls
        // must not touch the primary while it cools down.
        let (limited, limited_hits) = spawn_scripted(vec![("429 Too Many Requests", "{}")]).await;
        let (alive, alive_hits) = spawn_scripted(vec![SLOT_OK]).await;
        let rpc = Rpc::with_urls(
            limited,
            String::new(),
            vec![alive],
            CommitmentConfig::confirmed(),
            2,
            Duration::from_secs(2),
        )
        .unwrap();
        let started = Instant::now();
        assert_eq!(rpc.get_slot().await.unwrap(), 777);
        assert_eq!(rpc.get_slot().await.unwrap(), 777);
        assert!(
            started.elapsed() < Duration::from_millis(900),
            "no hidden 429 sleeps"
        );
        assert_eq!(
            limited_hits.load(Ordering::SeqCst),
            1,
            "primary skipped while cooling down"
        );
        assert_eq!(alive_hits.load(Ordering::SeqCst), 2);
        assert!(rpc.unhealthy(), "cooling-down primary reports unhealthy");
        assert_eq!(rpc.healthy_provider_count(), 1);
    }

    #[tokio::test]
    async fn timeout_is_classified_and_retried_then_exhausted() {
        // RPC timeout injection: a socket that never answers. With one
        // retry and no fallback the call must fail with a timeout class
        // after exactly two attempts.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = format!("http://{}", listener.local_addr().unwrap());
        let hits = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&hits);
        tokio::spawn(async move {
            let mut held = Vec::new();
            loop {
                if let Ok((sock, _)) = listener.accept().await {
                    counter.fetch_add(1, Ordering::SeqCst);
                    held.push(sock); // keep it open, never respond
                }
            }
        });
        let rpc = Rpc::with_urls(
            addr,
            String::new(),
            Vec::new(),
            CommitmentConfig::confirmed(),
            1,
            Duration::from_millis(300),
        )
        .unwrap();
        let err = rpc
            .send_transaction_classified(&VersionedTransaction::default())
            .await
            .expect_err("must time out");
        assert_eq!(err.class, RpcErrorClass::Timeout, "{err}");
        assert!(err.class.ambiguous());
        assert_eq!(err.attempts, 2);
        assert_eq!(hits.load(Ordering::SeqCst), 2);
        assert!(
            err.message.starts_with("sendTransaction:"),
            "{}",
            err.message
        );
        let bot: BotError = err.into();
        assert!(matches!(bot, BotError::Rpc(_)));
    }

    #[tokio::test]
    async fn permanent_errors_are_not_retried() {
        let (url, hits) = spawn_scripted(vec![(
            "200 OK",
            r#"{"jsonrpc":"2.0","error":{"code":-32602,"message":"Invalid param: WrongSize"},"id":1}"#,
        )])
        .await;
        let rpc = Rpc::with_urls(
            url,
            String::new(),
            Vec::new(),
            CommitmentConfig::confirmed(),
            3,
            Duration::from_secs(2),
        )
        .unwrap();
        let err = rpc.get_slot().await.expect_err("rejected");
        assert!(err.to_string().contains("getSlot"), "{err}");
        assert_eq!(
            hits.load(Ordering::SeqCst),
            1,
            "no retry on a definite rejection"
        );
        assert!(!rpc.unhealthy(), "bad params do not trip the breaker");
    }

    #[tokio::test]
    async fn transient_node_error_is_retried_on_the_same_provider_and_recovers() {
        let (url, hits) = spawn_scripted(vec![("503 Service Unavailable", "busy"), SLOT_OK]).await;
        let rpc = Rpc::with_urls(
            url,
            String::new(),
            Vec::new(),
            CommitmentConfig::confirmed(),
            2,
            Duration::from_secs(2),
        )
        .unwrap();
        assert_eq!(rpc.get_slot().await.unwrap(), 777);
        assert_eq!(hits.load(Ordering::SeqCst), 2);
        assert_eq!(rpc.failure_count(), 0, "success resets the streak");
    }

    #[tokio::test]
    async fn fresh_blockhash_refreshes_when_the_cached_one_is_too_old() {
        let (url, hits) = spawn_scripted(vec![(
            "200 OK",
            r#"{"jsonrpc":"2.0","result":{"context":{"slot":1},"value":{"blockhash":"11111111111111111111111111111111","lastValidBlockHeight":500}},"id":1}"#,
        )])
        .await;
        let rpc = Rpc::with_urls(
            url,
            String::new(),
            Vec::new(),
            CommitmentConfig::confirmed(),
            1,
            Duration::from_secs(2),
        )
        .unwrap();
        let first = rpc.latest_blockhash(false).await.unwrap();
        assert_eq!(first.last_valid_block_height, 500);
        assert!(first.is_fresh(Duration::from_secs(1)));
        // Cached: no new round trip.
        let again = rpc.fresh_blockhash(Duration::from_secs(1)).await.unwrap();
        assert_eq!(again.fetched_at, first.fetched_at);
        assert_eq!(hits.load(Ordering::SeqCst), 1);
        tokio::time::sleep(Duration::from_millis(30)).await;
        // Stricter freshness than the age of the cached hash → refresh.
        let refreshed = rpc
            .fresh_blockhash(Duration::from_millis(10))
            .await
            .unwrap();
        assert!(refreshed.fetched_at > first.fetched_at);
        assert_eq!(hits.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn confirm_tracked_reports_expiry_instead_of_burning_the_timeout() {
        // getTransaction → "not found" error; getBlockHeight → 1000, which
        // is past last_valid_block_height 900 → Expired, quickly.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move {
            loop {
                if let Ok((mut sock, _)) = listener.accept().await {
                    let mut buf = vec![0u8; 16384];
                    let n = sock.read(&mut buf).await.unwrap_or(0);
                    let req = String::from_utf8_lossy(&buf[..n]).to_string();
                    let body = if req.contains("getBlockHeight") {
                        r#"{"jsonrpc":"2.0","result":1000,"id":1}"#.to_string()
                    } else {
                        r#"{"jsonrpc":"2.0","error":{"code":-32602,"message":"Transaction not found: signature unknown"},"id":1}"#.to_string()
                    };
                    let resp = format!(
                        "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                        body.len()
                    );
                    let _ = sock.write_all(resp.as_bytes()).await;
                    let _ = sock.shutdown().await;
                }
            }
        });
        let rpc = Rpc::with_urls(
            addr,
            String::new(),
            Vec::new(),
            CommitmentConfig::confirmed(),
            0,
            Duration::from_secs(2),
        )
        .unwrap();
        let started = Instant::now();
        let outcome = rpc
            .confirm_tracked(
                &Signature::default(),
                Some(900),
                Duration::from_secs(20),
                Duration::from_millis(50),
            )
            .await
            .unwrap();
        assert!(
            matches!(
                outcome,
                ConfirmOutcome::Expired {
                    last_valid_block_height: 900,
                    block_height: 1000
                }
            ),
            "{outcome:?}"
        );
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "must not wait for the deadline"
        );

        // Without an expiry height the legacy behaviour holds: Timeout.
        let outcome = rpc
            .confirm(
                &Signature::default(),
                Duration::from_millis(200),
                Duration::from_millis(50),
            )
            .await
            .unwrap();
        assert!(matches!(outcome, ConfirmOutcome::Timeout), "{outcome:?}");
    }
}
