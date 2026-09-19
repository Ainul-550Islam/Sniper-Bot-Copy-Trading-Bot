//! Async RPC wrapper: retries, fallback endpoints, blockhash cache and the
//! raw JSON-RPC escape hatch used for provider-specific methods.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use base64::Engine;
use serde_json::json;
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

#[derive(Clone)]
pub struct Rpc {
    inner: Arc<RpcClient>,
    /// Failover endpoints, tried in order when the primary keeps failing.
    fallbacks: Vec<String>,
    commitment: CommitmentConfig,
    max_retries: u32,
    timeout: Duration,
    cache: Arc<BlockhashCache>,
    /// Count of consecutive primary failures, used to decide on failover.
    failures: Arc<AtomicU64>,
    url: String,
    ws_url: String,
    /// Warm cache for semi-static accounts (pump Global, mint owners, ATA
    /// existence). Only the `*_cached` methods consult it; price-bearing
    /// reads keep using the uncached ones. TTL 0 disables it entirely.
    account_cache: Arc<AccountCache>,
    account_cache_ttl: Duration,
}

impl Rpc {
    /// Build a client from the network config.
    pub fn new(cfg: &NetworkConfig) -> BotResult<Self> {
        let rpc = Self::with_urls(
            cfg.rpc_url.clone(),
            cfg.ws_url.clone(),
            cfg.rpc_url_fallbacks.clone(),
            parse_commitment(&cfg.commitment),
            cfg.max_retries,
            Duration::from_millis(cfg.request_timeout_ms),
        )?;
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
        let ws_url = if ws_url.trim().is_empty() {
            http_to_ws(&url)
        } else {
            ws_url
        };
        let inner = RpcClient::new_with_timeout_and_commitment(url.clone(), timeout, commitment);
        Ok(Rpc {
            inner: Arc::new(inner),
            fallbacks,
            commitment,
            max_retries,
            timeout,
            // A blockhash stays valid for ~60–90 s; refreshing every second
            // keeps us well inside that while cutting a round trip per snipe.
            cache: Arc::new(BlockhashCache::new(Duration::from_secs(1))),
            failures: Arc::new(AtomicU64::new(0)),
            url,
            ws_url,
            // Off unless `new(cfg)`/`with_account_cache` opts in: direct
            // `with_urls` callers (tests, tools) keep exact-fetch semantics.
            account_cache: Arc::new(AccountCache::new(0)),
            account_cache_ttl: Duration::ZERO,
        })
    }

    pub fn url(&self) -> &str {
        &self.url
    }

    pub fn ws_url(&self) -> &str {
        &self.ws_url
    }

    pub fn commitment(&self) -> CommitmentConfig {
        self.commitment
    }

    /// Direct access for anything this wrapper does not cover.
    pub fn raw(&self) -> &RpcClient {
        &self.inner
    }

    /// Run `op` with retries. Transient client errors are retried; a permanent
    /// one (bad pubkey, account not found) returns immediately.
    async fn retry<T, F, Fut>(&self, what: &str, mut op: F) -> BotResult<T>
    where
        F: FnMut() -> Fut,
        Fut: std::future::Future<Output = Result<T, solana_client::client_error::ClientError>>,
    {
        let reg = bot_core::obs::metrics::global();
        let mut attempt = 0u32;
        loop {
            let started = Instant::now();
            match op().await {
                Ok(v) => {
                    reg.histogram(
                        "bot_rpc_attempt_duration_ms",
                        "Duration of a single RPC attempt in milliseconds.",
                        &[("method", what)],
                        bot_core::obs::metrics::LATENCY_BUCKETS_MS,
                    )
                    .observe(started.elapsed().as_millis() as u64);
                    reg.counter(
                        "bot_rpc_requests_total",
                        "Completed RPC requests (after retries) by outcome.",
                        &[("method", what), ("outcome", "ok")],
                    )
                    .inc();
                    self.failures.store(0, Ordering::Relaxed);
                    return Ok(v);
                }
                Err(e) => {
                    reg.histogram(
                        "bot_rpc_attempt_duration_ms",
                        "Duration of a single RPC attempt in milliseconds.",
                        &[("method", what)],
                        bot_core::obs::metrics::LATENCY_BUCKETS_MS,
                    )
                    .observe(started.elapsed().as_millis() as u64);
                    attempt += 1;
                    let transient = is_transient(&e);
                    if !transient || attempt > self.max_retries {
                        // outcome is a closed set: fatal = non-transient error,
                        // exhausted = transient but retries ran out.
                        reg.counter(
                            "bot_rpc_requests_total",
                            "Completed RPC requests (after retries) by outcome.",
                            &[
                                ("method", what),
                                ("outcome", if transient { "exhausted" } else { "fatal" }),
                            ],
                        )
                        .inc();
                        self.failures.fetch_add(1, Ordering::Relaxed);
                        return Err(BotError::rpc(format!("{what}: {e}")));
                    }
                    // Exponential backoff with a cap, so a flapping node does
                    // not spin the hot loop.
                    let backoff = Duration::from_millis(50 * 2u64.pow(attempt.min(5)));
                    warn!(
                        what,
                        attempt,
                        error = %e,
                        backoff_ms = backoff.as_millis() as u64,
                        "transient rpc error, retrying"
                    );
                    tokio::time::sleep(backoff).await;
                }
            }
        }
    }

    /// True when the primary endpoint has failed enough times that the caller
    /// should consider switching. Exposed for the dashboard health widget.
    pub fn unhealthy(&self) -> bool {
        self.failures.load(Ordering::Relaxed) >= 3
    }

    /// Consecutive failure count.
    pub fn failure_count(&self) -> u64 {
        self.failures.load(Ordering::Relaxed)
    }

    // ------------------------------------------------------------- basics --

    pub async fn get_version(&self) -> BotResult<String> {
        let v = self
            .retry("getVersion", || self.inner.get_version())
            .await?;
        Ok(v.solana_core)
    }

    pub async fn get_slot(&self) -> BotResult<u64> {
        self.retry("getSlot", || self.inner.get_slot()).await
    }

    pub async fn health(&self) -> BotResult<String> {
        // `getHealth` returns the string "ok" or an error object.
        let v: serde_json::Value = self.send_raw(RpcRequest::GetHealth, json!([])).await?;
        Ok(v.as_str().unwrap_or("unknown").to_string())
    }

    pub async fn get_balance(&self, pubkey: &Pubkey) -> BotResult<u64> {
        self.retry("getBalance", || self.inner.get_balance(pubkey))
            .await
    }

    pub async fn get_account(&self, pubkey: &Pubkey) -> BotResult<Option<Account>> {
        match self
            .retry("getAccount", || self.inner.get_account(pubkey))
            .await
        {
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
        self.retry("getAccountData", || self.inner.get_account_data(pubkey))
            .await
    }

    pub async fn get_multiple_accounts(
        &self,
        pubkeys: &[Pubkey],
    ) -> BotResult<Vec<Option<Account>>> {
        if pubkeys.is_empty() {
            return Ok(Vec::new());
        }
        self.retry("getMultipleAccounts", || {
            self.inner.get_multiple_accounts(pubkeys)
        })
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
        self.retry("getProgramAccounts", || {
            self.inner
                .get_program_accounts_with_config(program_id, config.clone())
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
            .retry("getLatestBlockhash", || {
                self.inner
                    .get_latest_blockhash_with_commitment(CommitmentConfig::confirmed())
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

    pub async fn invalidate_blockhash(&self) {
        self.cache.invalidate().await;
    }

    pub async fn is_blockhash_valid(&self, blockhash: &Hash) -> BotResult<bool> {
        self.retry("isBlockhashValid", || {
            self.inner
                .is_blockhash_valid(blockhash, CommitmentConfig::confirmed())
        })
        .await
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
            .retry("getSignaturesForAddress", || {
                self.inner
                    .get_signatures_for_address_with_config(address, to_get_confirmed(&config))
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
        match self
            .retry("getTransaction", || {
                self.inner.get_transaction_with_config(signature, config)
            })
            .await
        {
            Ok(tx) => Ok(Some(tx)),
            Err(e) => {
                // Not-yet-confirmed and dropped transactions both surface as
                // errors; treat them as "no data" so callers can poll.
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
        self.retry("getBlock", || {
            self.inner.get_block_with_config(slot, config)
        })
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
        self.retry("simulateTransaction", || {
            self.inner
                .simulate_transaction_with_config(tx, config.clone())
        })
        .await
    }

    /// Broadcast a signed transaction. `skip_preflight` is what a sniper wants:
    /// preflight costs a round trip and we simulate separately anyway.
    pub async fn send_transaction(&self, tx: &VersionedTransaction) -> BotResult<Signature> {
        let config = RpcSendTransactionConfig {
            skip_preflight: true,
            preflight_commitment: Some(CommitmentLevel::Processed),
            encoding: Some(UiTransactionEncoding::Base64),
            max_retries: Some(0),
            min_context_slot: None,
        };
        self.retry("sendTransaction", || {
            self.inner.send_transaction_with_config(tx, config)
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
        let deadline = Instant::now() + timeout;
        loop {
            if Instant::now() > deadline {
                return Ok(ConfirmOutcome::Timeout);
            }
            match self.get_transaction(signature).await {
                Ok(Some(tx)) => {
                    let err = tx
                        .transaction
                        .meta
                        .as_ref()
                        .and_then(|m| m.err.as_ref().map(|e| e.to_string()));
                    let logs = match tx.transaction.meta.as_ref().map(|m| m.log_messages.clone()) {
                        Some(
                            solana_transaction_status::option_serializer::OptionSerializer::Some(l),
                        ) => l,
                        _ => Vec::new(),
                    };
                    return Ok(match err {
                        Some(e) => ConfirmOutcome::Failed { error: e, logs },
                        None => ConfirmOutcome::Confirmed {
                            slot: tx.slot,
                            logs,
                            fee: tx.transaction.meta.as_ref().map(|m| m.fee).unwrap_or(0),
                        },
                    });
                }
                Ok(None) => tokio::time::sleep(poll).await,
                Err(e) => {
                    debug!(error = %e, "confirm poll error, will retry");
                    tokio::time::sleep(poll).await;
                }
            }
        }
    }

    // ------------------------------------------------------------ raw rpc --

    /// Escape hatch for provider-specific methods (`getAsset`, Helius DAS,
    /// Yellowstone, …).
    pub async fn send_raw(
        &self,
        request: RpcRequest,
        params: serde_json::Value,
    ) -> BotResult<serde_json::Value> {
        self.retry_raw("send", request, params).await
    }

    async fn retry_raw(
        &self,
        what: &str,
        request: RpcRequest,
        params: serde_json::Value,
    ) -> BotResult<serde_json::Value> {
        let reg = bot_core::obs::metrics::global();
        let mut attempt = 0u32;
        loop {
            let started = Instant::now();
            match self.inner.send(request, params.clone()).await {
                Ok(v) => {
                    reg.histogram(
                        "bot_rpc_attempt_duration_ms",
                        "Duration of a single RPC attempt in milliseconds.",
                        &[("method", what)],
                        bot_core::obs::metrics::LATENCY_BUCKETS_MS,
                    )
                    .observe(started.elapsed().as_millis() as u64);
                    reg.counter(
                        "bot_rpc_requests_total",
                        "Completed RPC requests (after retries) by outcome.",
                        &[("method", what), ("outcome", "ok")],
                    )
                    .inc();
                    self.failures.store(0, Ordering::Relaxed);
                    return Ok(v);
                }
                Err(e) => {
                    reg.histogram(
                        "bot_rpc_attempt_duration_ms",
                        "Duration of a single RPC attempt in milliseconds.",
                        &[("method", what)],
                        bot_core::obs::metrics::LATENCY_BUCKETS_MS,
                    )
                    .observe(started.elapsed().as_millis() as u64);
                    attempt += 1;
                    let transient = is_transient(&e);
                    if !transient || attempt > self.max_retries {
                        reg.counter(
                            "bot_rpc_requests_total",
                            "Completed RPC requests (after retries) by outcome.",
                            &[
                                ("method", what),
                                ("outcome", if transient { "exhausted" } else { "fatal" }),
                            ],
                        )
                        .inc();
                        return Err(BotError::rpc(format!("{what}: {e}")));
                    }
                    tokio::time::sleep(Duration::from_millis(50 * 2u64.pow(attempt.min(5)))).await;
                }
            }
        }
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

    /// Build a client pointed at the first healthy fallback endpoint.
    ///
    /// Used by the supervisor when `unhealthy()` stays true: rather than
    /// hammering a dead node, we hand out a client for another endpoint.
    pub fn failover(&self) -> Option<Rpc> {
        self.fallbacks.first().map(|url| Rpc {
            inner: Arc::new(RpcClient::new_with_timeout_and_commitment(
                url.clone(),
                self.timeout,
                self.commitment,
            )),
            fallbacks: self.fallbacks[1..].to_vec(),
            commitment: self.commitment,
            max_retries: self.max_retries,
            timeout: self.timeout,
            cache: Arc::new(BlockhashCache::new(Duration::from_secs(1))),
            failures: Arc::new(AtomicU64::new(0)),
            url: url.clone(),
            ws_url: http_to_ws(url),
            // Same cluster, same accounts: share the warm cache across the
            // failover chain (the blockhash cache stays per-endpoint because
            // hashes are node-local in terms of freshness).
            account_cache: Arc::clone(&self.account_cache),
            account_cache_ttl: self.account_cache_ttl,
        })
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
    Timeout,
}

impl ConfirmOutcome {
    pub fn ok(&self) -> bool {
        matches!(self, ConfirmOutcome::Confirmed { .. })
    }

    pub fn summary(&self) -> String {
        match self {
            ConfirmOutcome::Confirmed { slot, fee, .. } => {
                format!("confirmed in slot {slot} (fee {fee} lamports)")
            }
            ConfirmOutcome::Failed { error, .. } => format!("FAILED on chain: {error}"),
            ConfirmOutcome::Timeout => "timed out waiting for confirmation".into(),
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

/// Network/timeout errors are worth retrying; a bad pubkey or a missing
/// account is not.
fn is_transient(e: &solana_client::client_error::ClientError) -> bool {
    use solana_client::client_error::ClientErrorKind;
    match &e.kind {
        ClientErrorKind::Reqwest(_) => true,
        ClientErrorKind::RpcError(re) => matches!(
            re,
            solana_client::rpc_request::RpcError::ForUser(_)
                | solana_client::rpc_request::RpcError::ParseError(_)
        ),
        ClientErrorKind::Io(_) => true,
        ClientErrorKind::SigningError(_) => false,
        ClientErrorKind::Custom(msg) => {
            let m = msg.to_ascii_lowercase();
            m.contains("timeout")
                || m.contains("429")
                || m.contains("502")
                || m.contains("503")
                || m.contains("504")
                || m.contains("connection")
                || m.contains("blockhash")
        }
        _ => false,
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

        let next = rpc.failover().expect("should have a fallback");
        assert_eq!(next.url(), "https://a.example.com");
        assert_eq!(next.ws_url(), "wss://a.example.com");
        let last = next.failover().expect("should have a second fallback");
        assert_eq!(last.url(), "https://b.example.com");
        assert!(last.failover().is_none());
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
    }
}
