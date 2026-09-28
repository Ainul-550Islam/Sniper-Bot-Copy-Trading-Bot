//! RPC provider abstraction: many endpoints, per-provider health, automatic
//! failover, a configurable retry policy (exponential backoff with full
//! jitter), retry classification and explicit rate-limit / timeout handling.
//!
//! The pool is the transport layer under [`crate::rpc::Rpc`]. Every RPC call
//! asks the pool for a provider (`route`), reports the outcome back
//! (`record_success` / `record_failure`) and lets the retry loop decide —
//! from the error *class*, not the error text — whether to retry, whether to
//! move to another provider, and how long to wait.
//!
//! Design notes
//! * Each provider owns a `solana_client` `RpcClient` built on a custom
//!   [`RpcSender`]. The stock HTTP sender silently sleeps on HTTP 429 (up to
//!   five times, up to `Retry-After` = 120 s **each**) — invisible to the
//!   caller and fatal for a sniper. Ours returns 429 immediately, carrying
//!   the `Retry-After` hint, so the pool can cool that provider down and the
//!   very next attempt goes elsewhere.
//! * Health is a circuit breaker per provider: `failure_threshold`
//!   consecutive failures trip it for `cooldown`; after the cooldown the
//!   provider is half-open (the next call is a probe; one more failure
//!   re-trips it immediately, one success closes it).
//! * Labels never contain the URL (API keys live in paths and query
//!   strings): metrics and logs use `"{index}:{host}"`.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use reqwest::header::{CONTENT_TYPE, RETRY_AFTER};
use reqwest::StatusCode;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use solana_client::client_error::{ClientError, ClientErrorKind, Result as ClientResult};
use solana_client::nonblocking::rpc_client::RpcClient;
use solana_client::rpc_client::RpcClientConfig;
use solana_client::rpc_custom_error;
use solana_client::rpc_request::{RpcError, RpcRequest, RpcResponseErrorData};
use solana_client::rpc_response::RpcSimulateTransactionResult;
use solana_client::rpc_sender::{RpcSender, RpcTransportStats};
use solana_sdk::commitment_config::CommitmentConfig;
use tracing::{debug, warn};

use bot_core::config::NetworkConfig;
use bot_core::error::{BotError, BotResult};

use crate::rpc::http_to_ws;

// ------------------------------------------------------------------------
// Retry policy
// ------------------------------------------------------------------------

/// How the RPC layer retries: attempt budget, exponential backoff bounds,
/// jitter and the pause applied to a rate-limited provider.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RetryPolicy {
    /// Retries *after* the first attempt (total attempts = `max_retries + 1`).
    pub max_retries: u32,
    /// Delay before the first retry; doubles every retry.
    pub base: Duration,
    /// Upper bound for a single delay.
    pub max: Duration,
    /// Full jitter: the actual delay is uniform in `[0, computed]`.
    pub jitter: bool,
    /// Minimum cooldown for a provider that answered HTTP 429.
    pub rate_limit_cooldown: Duration,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        RetryPolicy {
            max_retries: 3,
            base: Duration::from_millis(50),
            max: Duration::from_millis(2_000),
            jitter: true,
            rate_limit_cooldown: Duration::from_millis(1_000),
        }
    }
}

impl RetryPolicy {
    /// Policy from the network section of the config.
    pub fn from_network(cfg: &NetworkConfig) -> Self {
        RetryPolicy {
            max_retries: cfg.max_retries,
            base: Duration::from_millis(cfg.retry_base_backoff_ms.max(1)),
            max: Duration::from_millis(
                cfg.retry_max_backoff_ms
                    .max(cfg.retry_base_backoff_ms.max(1)),
            ),
            jitter: cfg.retry_jitter,
            rate_limit_cooldown: Duration::from_millis(cfg.rate_limit_cooldown_ms),
        }
    }

    /// The pre-pool behaviour (`50 ms * 2^attempt`, capped at 1.6 s, no
    /// jitter). Used by [`crate::rpc::Rpc::with_urls`] so tests and tools
    /// that construct clients by hand keep their exact timing.
    pub fn legacy(max_retries: u32) -> Self {
        RetryPolicy {
            max_retries,
            base: Duration::from_millis(50),
            max: Duration::from_millis(1_600),
            jitter: false,
            rate_limit_cooldown: Duration::from_millis(1_000),
        }
    }

    /// Total attempts allowed (first try + retries).
    pub fn max_attempts(&self) -> u32 {
        self.max_retries.saturating_add(1).max(1)
    }

    /// Delay before retry number `attempt` (1-based: the delay after the
    /// first failure is `attempt = 1`). Exponential, capped, optionally
    /// jittered. `retry_after` (from a 429) is a floor.
    pub fn delay(&self, attempt: u32, retry_after: Option<Duration>) -> Duration {
        let exp = attempt.saturating_sub(1).min(20);
        let raw = self
            .base
            .checked_mul(1u32 << exp)
            .unwrap_or(self.max)
            .min(self.max);
        let mut delay = if self.jitter {
            let raw_us = raw.as_micros() as u64;
            if raw_us == 0 {
                Duration::ZERO
            } else {
                // Full jitter: uniform in [0, raw]. Keep a small floor so a
                // retry never fires in the same scheduler tick.
                let sample = rand::random::<u64>() % (raw_us + 1);
                Duration::from_micros(sample.max(raw_us / 8))
            }
        } else {
            raw
        };
        if let Some(floor) = retry_after {
            delay = delay.max(floor);
        }
        delay
    }
}

// ------------------------------------------------------------------------
// Error classification
// ------------------------------------------------------------------------

/// What an RPC failure *means* for the retry loop. Classification looks at
/// the error kind / JSON-RPC code first and the message text only as a last
/// resort, so provider-specific wording cannot flip a decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RpcErrorClass {
    /// No answer inside the deadline. Retry on another provider; for
    /// `sendTransaction` the outcome is ambiguous (the tx may have landed).
    Timeout,
    /// HTTP 429 / quota exhausted. Cool the provider down, go elsewhere.
    RateLimited { retry_after: Option<Duration> },
    /// Connection refused/reset, DNS, TLS, EOF — the request never got a
    /// full answer. Retry on another provider.
    Transport,
    /// The node answered but cannot serve right now (5xx, node unhealthy /
    /// behind, block not yet available, min context slot not reached).
    Unavailable,
    /// The request referenced a blockhash the node does not know
    /// (expired or from another fork). Retrying the *same* payload is
    /// pointless; the caller must rebuild with a fresh blockhash.
    Blockhash,
    /// Definite rejection: bad params, unknown account, signature or
    /// preflight failure, insufficient funds. Never retried.
    Permanent,
}

impl RpcErrorClass {
    /// Whether the same request may simply be re-issued. A `Blockhash`
    /// failure is not: the payload must be rebuilt first (see
    /// [`RpcErrorClass::needs_rebuild`]).
    pub fn retryable(&self) -> bool {
        matches!(
            self,
            RpcErrorClass::Timeout
                | RpcErrorClass::RateLimited { .. }
                | RpcErrorClass::Transport
                | RpcErrorClass::Unavailable
        )
    }

    /// Whether the failure is fixed by rebuilding the transaction with a
    /// fresh blockhash (the executor's retry-with-rebuild path).
    pub fn needs_rebuild(&self) -> bool {
        matches!(self, RpcErrorClass::Blockhash)
    }

    /// Whether the failure says something about the *provider* (and the
    /// next attempt should prefer a different one).
    pub fn provider_fault(&self) -> bool {
        matches!(
            self,
            RpcErrorClass::Timeout
                | RpcErrorClass::RateLimited { .. }
                | RpcErrorClass::Transport
                | RpcErrorClass::Unavailable
        )
    }

    /// Whether the request may already have been executed by the node
    /// (relevant for `sendTransaction`: a duplicate broadcast is safe, but
    /// the lifecycle must treat the outcome as ambiguous).
    pub fn ambiguous(&self) -> bool {
        matches!(self, RpcErrorClass::Timeout | RpcErrorClass::Transport)
    }

    /// `Retry-After` floor carried by a 429.
    pub fn retry_after(&self) -> Option<Duration> {
        match self {
            RpcErrorClass::RateLimited { retry_after } => *retry_after,
            _ => None,
        }
    }

    /// Closed-set label for metrics.
    pub fn label(&self) -> &'static str {
        match self {
            RpcErrorClass::Timeout => "timeout",
            RpcErrorClass::RateLimited { .. } => "rate_limited",
            RpcErrorClass::Transport => "transport",
            RpcErrorClass::Unavailable => "unavailable",
            RpcErrorClass::Blockhash => "blockhash",
            RpcErrorClass::Permanent => "permanent",
        }
    }
}

/// Classify a `solana_client` error.
pub fn classify_client_error(e: &ClientError) -> RpcErrorClass {
    match &e.kind {
        ClientErrorKind::Reqwest(err) => {
            if err.is_timeout() {
                return RpcErrorClass::Timeout;
            }
            match err.status() {
                Some(StatusCode::TOO_MANY_REQUESTS) => {
                    RpcErrorClass::RateLimited { retry_after: None }
                }
                Some(s) if s.is_server_error() => RpcErrorClass::Unavailable,
                Some(s) if s == StatusCode::REQUEST_TIMEOUT => RpcErrorClass::Timeout,
                Some(s) if s.is_client_error() => {
                    // 401/403 = bad key/plan; 400/404 = bad request. All
                    // definite for this provider — but another provider may
                    // still work, so classify by message when it hints at it.
                    classify_message(&err.to_string()).max_permanent()
                }
                _ => RpcErrorClass::Transport,
            }
        }
        ClientErrorKind::Io(_) => RpcErrorClass::Transport,
        ClientErrorKind::Middleware(_) => RpcErrorClass::Transport,
        ClientErrorKind::SerdeJson(_) => RpcErrorClass::Unavailable,
        ClientErrorKind::SigningError(_) => RpcErrorClass::Permanent,
        ClientErrorKind::TransactionError(_) => RpcErrorClass::Permanent,
        ClientErrorKind::Custom(msg) => classify_message(msg),
        ClientErrorKind::RpcError(re) => match re {
            RpcError::RpcResponseError { code, message, .. } => classify_rpc_code(*code, message),
            // "unable to confirm transaction", "failed to get recent
            // blockhash" — client-side wrappers around transient trouble.
            RpcError::ForUser(msg) => match classify_message(msg) {
                RpcErrorClass::Permanent => RpcErrorClass::Unavailable,
                other => other,
            },
            // The node returned something we could not parse: try another.
            RpcError::ParseError(_) => RpcErrorClass::Unavailable,
            RpcError::RpcRequestError(msg) => match classify_message(msg) {
                RpcErrorClass::Permanent => RpcErrorClass::Unavailable,
                other => other,
            },
        },
    }
}

/// JSON-RPC error codes, per `solana_client::rpc_custom_error`.
pub fn classify_rpc_code(code: i64, message: &str) -> RpcErrorClass {
    use rpc_custom_error::*;
    match code {
        JSON_RPC_SERVER_ERROR_NODE_UNHEALTHY
        | JSON_RPC_SERVER_ERROR_BLOCK_NOT_AVAILABLE
        | JSON_RPC_SERVER_ERROR_BLOCK_STATUS_NOT_AVAILABLE_YET
        | JSON_RPC_SERVER_ERROR_MIN_CONTEXT_SLOT_NOT_REACHED
        | JSON_RPC_SERVER_ERROR_TRANSACTION_HISTORY_NOT_AVAILABLE
        | JSON_RPC_SERVER_ERROR_LONG_TERM_STORAGE_UNREACHABLE
        | JSON_RPC_SERVER_ERROR_NO_SNAPSHOT
        | JSON_RPC_SCAN_ERROR => RpcErrorClass::Unavailable,
        JSON_RPC_SERVER_ERROR_SEND_TRANSACTION_PREFLIGHT_FAILURE => {
            // Preflight failure carries the simulation error; a blockhash
            // problem is the one case worth a rebuild.
            if message_mentions_blockhash(message) {
                RpcErrorClass::Blockhash
            } else {
                RpcErrorClass::Permanent
            }
        }
        JSON_RPC_SERVER_ERROR_TRANSACTION_SIGNATURE_VERIFICATION_FAILURE
        | JSON_RPC_SERVER_ERROR_TRANSACTION_PRECOMPILE_VERIFICATION_FAILURE
        | JSON_RPC_SERVER_ERROR_TRANSACTION_SIGNATURE_LEN_MISMATCH
        | JSON_RPC_SERVER_ERROR_UNSUPPORTED_TRANSACTION_VERSION
        | JSON_RPC_SERVER_ERROR_BLOCK_CLEANED_UP
        | JSON_RPC_SERVER_ERROR_SLOT_SKIPPED
        | JSON_RPC_SERVER_ERROR_LONG_TERM_STORAGE_SLOT_SKIPPED
        | JSON_RPC_SERVER_ERROR_KEY_EXCLUDED_FROM_SECONDARY_INDEX
        | JSON_RPC_SERVER_ERROR_EPOCH_REWARDS_PERIOD_ACTIVE
        | JSON_RPC_SERVER_ERROR_SLOT_NOT_EPOCH_BOUNDARY => RpcErrorClass::Permanent,
        // Standard JSON-RPC: -32600 invalid request, -32601 method not
        // found, -32602 invalid params, -32700 parse error → permanent;
        // -32603 internal error → the node choked, try another.
        -32603 => RpcErrorClass::Unavailable,
        -32600 | -32601 | -32602 | -32700 => RpcErrorClass::Permanent,
        // Providers use private codes for quotas (Helius -32429, QuickNode
        // -32009-ish text). Fall back to the message.
        _ => classify_message(message),
    }
}

/// Text-only classification (last resort, also used for `Custom` errors
/// produced by our own sender and for Jito/HTTP wrappers).
pub fn classify_message(msg: &str) -> RpcErrorClass {
    let m = msg.to_ascii_lowercase();
    if m.contains("429")
        || m.contains("too many requests")
        || m.contains("rate limit")
        || m.contains("rate-limit")
        || m.contains("quota")
        || m.contains("throttl")
    {
        return RpcErrorClass::RateLimited {
            retry_after: parse_retry_after(&m),
        };
    }
    if message_mentions_blockhash(&m) {
        return RpcErrorClass::Blockhash;
    }
    if m.contains("timed out")
        || m.contains("timeout")
        || m.contains("deadline")
        || m.contains("operation timed out")
    {
        return RpcErrorClass::Timeout;
    }
    if m.contains("502")
        || m.contains("503")
        || m.contains("504")
        || m.contains("bad gateway")
        || m.contains("service unavailable")
        || m.contains("gateway timeout")
        || m.contains("unavailable")
        || m.contains("overloaded")
        || m.contains("unhealthy")
        || m.contains("slots behind")
        || m.contains("node is behind")
        || m.contains("internal error")
        || m.contains("not yet available")
    {
        return RpcErrorClass::Unavailable;
    }
    if m.contains("connection")
        || m.contains("reset by peer")
        || m.contains("refused")
        || m.contains("broken pipe")
        || m.contains("dns error")
        || m.contains("failed to lookup")
        || m.contains("tls")
        || m.contains("certificate")
        || m.contains("unexpected eof")
        || m.contains("channel closed")
        || m.contains("incomplete message")
        || m.contains("error sending request")
        || m.contains("hyper::error")
    {
        return RpcErrorClass::Transport;
    }
    RpcErrorClass::Permanent
}

impl RpcErrorClass {
    /// For 4xx HTTP statuses: keep a rate-limit / blockhash verdict, turn
    /// anything else into `Permanent`.
    fn max_permanent(self) -> RpcErrorClass {
        match self {
            RpcErrorClass::RateLimited { .. } | RpcErrorClass::Blockhash => self,
            _ => RpcErrorClass::Permanent,
        }
    }
}

fn message_mentions_blockhash(m: &str) -> bool {
    let m = m.to_ascii_lowercase();
    m.contains("blockhash")
        || m.contains("block height exceeded")
        || m.contains("blockhashnotfound")
}

/// `retry-after=<secs>` or `retry-after: <secs>` embedded in a message.
fn parse_retry_after(m: &str) -> Option<Duration> {
    let idx = m.find("retry-after")?;
    let rest = &m[idx + "retry-after".len()..];
    let digits: String = rest
        .trim_start_matches([' ', '=', ':'])
        .chars()
        .take_while(|c| c.is_ascii_digit())
        .collect();
    let secs: u64 = digits.parse().ok()?;
    // Guard against a hostile/buggy header parking us for an hour.
    Some(Duration::from_secs(secs.min(120)))
}

// ------------------------------------------------------------------------
// Metered sender
// ------------------------------------------------------------------------

/// JSON-RPC transport that never sleeps on the caller's behalf. Identical
/// wire behaviour to `solana_client`'s HTTP sender except that HTTP 429 is
/// returned immediately as `ClientErrorKind::Custom("http 429 … retry-after=N")`.
pub struct MeteredSender {
    client: reqwest::Client,
    url: String,
    request_id: AtomicU64,
    stats: RwLock<RpcTransportStats>,
}

impl MeteredSender {
    pub fn new(client: reqwest::Client, url: String) -> Self {
        MeteredSender {
            client,
            url,
            request_id: AtomicU64::new(1),
            stats: RwLock::new(RpcTransportStats::default()),
        }
    }

    /// Build the shared HTTP client used by every provider in a pool.
    pub fn http_client(timeout: Duration) -> BotResult<reqwest::Client> {
        reqwest::Client::builder()
            .timeout(timeout)
            .connect_timeout(timeout.min(Duration::from_secs(5)))
            .pool_idle_timeout(Duration::from_secs(90))
            .pool_max_idle_per_host(8)
            .tcp_nodelay(true)
            .user_agent(concat!("sniper-suite/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|e| BotError::config(format!("rpc http client: {e}")))
    }

    // `ClientError` is the upstream client's own error type; boxing it here
    // would only move the allocation into every caller's `map_err`.
    #[allow(clippy::result_large_err)]
    async fn send_inner(&self, request: RpcRequest, body: String) -> ClientResult<Value> {
        let response = self
            .client
            .post(&self.url)
            .header(CONTENT_TYPE, "application/json")
            .body(body)
            .send()
            .await
            .map_err(|e| ClientError::new_with_request(ClientErrorKind::Reqwest(e), request))?;

        let status = response.status();
        if status == StatusCode::TOO_MANY_REQUESTS {
            let retry_after = response
                .headers()
                .get(RETRY_AFTER)
                .and_then(|v| v.to_str().ok())
                .and_then(|s| s.trim().parse::<u64>().ok());
            let msg = match retry_after {
                Some(secs) => format!("http 429 too many requests; retry-after={secs}"),
                None => "http 429 too many requests".to_string(),
            };
            return Err(ClientError::new_with_request(
                ClientErrorKind::Custom(msg),
                request,
            ));
        }
        if !status.is_success() {
            // Keep the reqwest error so classification can read the status.
            let err = response.error_for_status().unwrap_err();
            return Err(ClientError::new_with_request(
                ClientErrorKind::Reqwest(err),
                request,
            ));
        }

        let mut json: Value = response
            .json()
            .await
            .map_err(|e| ClientError::new_with_request(ClientErrorKind::Reqwest(e), request))?;

        if json["error"].is_object() {
            #[derive(Deserialize)]
            struct ErrorObject {
                code: i64,
                message: String,
            }
            return match serde_json::from_value::<ErrorObject>(json["error"].clone()) {
                Ok(obj) => {
                    let data = match obj.code {
                        rpc_custom_error::JSON_RPC_SERVER_ERROR_SEND_TRANSACTION_PREFLIGHT_FAILURE => {
                            match serde_json::from_value::<RpcSimulateTransactionResult>(
                                json["error"]["data"].clone(),
                            ) {
                                Ok(d) => RpcResponseErrorData::SendTransactionPreflightFailure(d),
                                Err(_) => RpcResponseErrorData::Empty,
                            }
                        }
                        rpc_custom_error::JSON_RPC_SERVER_ERROR_NODE_UNHEALTHY => {
                            match serde_json::from_value::<rpc_custom_error::NodeUnhealthyErrorData>(
                                json["error"]["data"].clone(),
                            ) {
                                Ok(rpc_custom_error::NodeUnhealthyErrorData { num_slots_behind }) => {
                                    RpcResponseErrorData::NodeUnhealthy { num_slots_behind }
                                }
                                Err(_) => RpcResponseErrorData::Empty,
                            }
                        }
                        _ => RpcResponseErrorData::Empty,
                    };
                    Err(ClientError::new_with_request(
                        ClientErrorKind::RpcError(RpcError::RpcResponseError {
                            code: obj.code,
                            message: obj.message,
                            data,
                        }),
                        request,
                    ))
                }
                Err(err) => Err(ClientError::new_with_request(
                    ClientErrorKind::RpcError(RpcError::RpcRequestError(format!(
                        "Failed to deserialize RPC error response: {} [{}]",
                        json["error"], err
                    ))),
                    request,
                )),
            };
        }
        Ok(json["result"].take())
    }
}

#[async_trait]
impl RpcSender for MeteredSender {
    async fn send(&self, request: RpcRequest, params: Value) -> ClientResult<Value> {
        let started = Instant::now();
        let id = self.request_id.fetch_add(1, Ordering::Relaxed);
        let body = request.build_request_json(id, params).to_string();
        let result = self.send_inner(request, body).await;
        if let Ok(mut s) = self.stats.write() {
            s.request_count += 1;
            s.elapsed_time += started.elapsed();
        }
        result
    }

    fn get_transport_stats(&self) -> RpcTransportStats {
        self.stats.read().map(|s| s.clone()).unwrap_or_default()
    }

    fn url(&self) -> String {
        self.url.clone()
    }
}

// ------------------------------------------------------------------------
// Provider + pool
// ------------------------------------------------------------------------

/// One endpoint with its health counters.
pub struct Provider {
    index: usize,
    url: String,
    ws_url: String,
    label: String,
    client: Arc<RpcClient>,
    consecutive_failures: AtomicU32,
    total_failures: AtomicU64,
    total_ok: AtomicU64,
    last_latency_ms: AtomicU64,
    cooldown_until: Mutex<Option<Instant>>,
    last_error: Mutex<Option<String>>,
    last_failure_class: Mutex<Option<&'static str>>,
}

impl Provider {
    pub fn index(&self) -> usize {
        self.index
    }

    pub fn url(&self) -> &str {
        &self.url
    }

    pub fn ws_url(&self) -> &str {
        &self.ws_url
    }

    /// Safe-to-log identifier (`"{index}:{host}"`, never the URL).
    pub fn label(&self) -> &str {
        &self.label
    }

    pub fn client(&self) -> &RpcClient {
        &self.client
    }

    pub fn consecutive_failures(&self) -> u32 {
        self.consecutive_failures.load(Ordering::Relaxed)
    }

    /// Time left in the current cooldown (zero when not cooling down).
    pub fn cooldown_remaining(&self) -> Duration {
        let guard = self
            .cooldown_until
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        match *guard {
            Some(until) => until.saturating_duration_since(Instant::now()),
            None => Duration::ZERO,
        }
    }

    /// Not in cooldown. (A provider whose cooldown just ended is "half
    /// open": healthy for routing purposes, re-tripped by one more failure.)
    pub fn is_healthy(&self) -> bool {
        self.cooldown_remaining().is_zero()
    }

    fn set_cooldown(&self, d: Duration) {
        let mut guard = self
            .cooldown_until
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let until = Instant::now() + d;
        // Never shorten an existing cooldown.
        *guard = Some(match *guard {
            Some(cur) if cur > until => cur,
            _ => until,
        });
    }

    fn clear_cooldown(&self) {
        *self
            .cooldown_until
            .lock()
            .unwrap_or_else(|p| p.into_inner()) = None;
    }

    fn status(&self) -> ProviderStatus {
        ProviderStatus {
            index: self.index,
            label: self.label.clone(),
            healthy: self.is_healthy(),
            consecutive_failures: self.consecutive_failures(),
            total_failures: self.total_failures.load(Ordering::Relaxed),
            total_ok: self.total_ok.load(Ordering::Relaxed),
            cooldown_remaining_ms: self.cooldown_remaining().as_millis() as u64,
            last_latency_ms: self.last_latency_ms.load(Ordering::Relaxed),
            last_failure_class: self
                .last_failure_class
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .map(str::to_string),
            last_error: self
                .last_error
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .clone(),
        }
    }
}

/// Point-in-time provider health (dashboard / `/api/health`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProviderStatus {
    pub index: usize,
    pub label: String,
    pub healthy: bool,
    pub consecutive_failures: u32,
    pub total_failures: u64,
    pub total_ok: u64,
    pub cooldown_remaining_ms: u64,
    pub last_latency_ms: u64,
    #[serde(default)]
    pub last_failure_class: Option<String>,
    #[serde(default)]
    pub last_error: Option<String>,
}

/// Pool construction parameters.
#[derive(Debug, Clone)]
pub struct PoolConfig {
    /// Endpoints, primary first. Blanks/duplicates are dropped.
    pub urls: Vec<String>,
    /// Websocket URL for the primary; blank = derived from the HTTP URL.
    /// Fallbacks always derive theirs.
    pub ws_url: String,
    pub commitment: CommitmentConfig,
    pub timeout: Duration,
    pub failure_threshold: u32,
    pub cooldown: Duration,
    pub retry: RetryPolicy,
}

impl PoolConfig {
    pub fn from_network(cfg: &NetworkConfig) -> Self {
        PoolConfig {
            urls: cfg.rpc_endpoints(),
            ws_url: cfg.ws_url.clone(),
            commitment: crate::rpc::parse_commitment(&cfg.commitment),
            timeout: Duration::from_millis(cfg.request_timeout_ms.max(1)),
            failure_threshold: cfg.provider_failure_threshold.max(1),
            cooldown: Duration::from_millis(cfg.provider_cooldown_ms),
            retry: RetryPolicy::from_network(cfg),
        }
    }
}

/// Endpoint pool with health tracking and failover routing.
pub struct ProviderPool {
    providers: Vec<Arc<Provider>>,
    failure_threshold: u32,
    cooldown: Duration,
    retry: RetryPolicy,
    commitment: CommitmentConfig,
    timeout: Duration,
}

impl ProviderPool {
    pub fn new(cfg: PoolConfig) -> BotResult<Self> {
        let mut urls: Vec<String> = Vec::new();
        for u in cfg.urls {
            let t = u.trim().to_string();
            if !t.is_empty() && !urls.contains(&t) {
                urls.push(t);
            }
        }
        if urls.is_empty() {
            return Err(BotError::config("rpc_url is empty"));
        }
        let http = MeteredSender::http_client(cfg.timeout)?;
        let mut seen_hosts: HashMap<String, usize> = HashMap::new();
        let mut providers = Vec::with_capacity(urls.len());
        for (index, url) in urls.into_iter().enumerate() {
            let host = url_host(&url);
            let n = seen_hosts.entry(host.clone()).or_insert(0);
            *n += 1;
            let label = format!("{index}:{host}");
            let ws_url = if index == 0 && !cfg.ws_url.trim().is_empty() {
                cfg.ws_url.trim().to_string()
            } else {
                http_to_ws(&url)
            };
            let client = RpcClient::new_sender(
                MeteredSender::new(http.clone(), url.clone()),
                RpcClientConfig::with_commitment(cfg.commitment),
            );
            providers.push(Arc::new(Provider {
                index,
                url,
                ws_url,
                label,
                client: Arc::new(client),
                consecutive_failures: AtomicU32::new(0),
                total_failures: AtomicU64::new(0),
                total_ok: AtomicU64::new(0),
                last_latency_ms: AtomicU64::new(0),
                cooldown_until: Mutex::new(None),
                last_error: Mutex::new(None),
                last_failure_class: Mutex::new(None),
            }));
        }
        let pool = ProviderPool {
            providers,
            failure_threshold: cfg.failure_threshold.max(1),
            cooldown: cfg.cooldown,
            retry: cfg.retry,
            commitment: cfg.commitment,
            timeout: cfg.timeout,
        };
        for p in &pool.providers {
            pool.publish_health(p);
        }
        Ok(pool)
    }

    pub fn len(&self) -> usize {
        self.providers.len()
    }

    pub fn is_empty(&self) -> bool {
        self.providers.is_empty()
    }

    pub fn providers(&self) -> &[Arc<Provider>] {
        &self.providers
    }

    /// Provider by index (panics only on an out-of-range index, which the
    /// `Rpc` wrapper never produces).
    pub fn provider(&self, index: usize) -> &Provider {
        &self.providers[index.min(self.providers.len() - 1)]
    }

    pub fn retry(&self) -> &RetryPolicy {
        &self.retry
    }

    pub fn failure_threshold(&self) -> u32 {
        self.failure_threshold
    }

    pub fn cooldown(&self) -> Duration {
        self.cooldown
    }

    pub fn commitment(&self) -> CommitmentConfig {
        self.commitment
    }

    pub fn timeout(&self) -> Duration {
        self.timeout
    }

    /// Providers not currently cooling down.
    pub fn healthy_count(&self) -> usize {
        self.providers.iter().filter(|p| p.is_healthy()).count()
    }

    /// Visit order for a call that prefers `origin`: origin first, then the
    /// rest in configured order (fallback priority is positional).
    fn order_from(&self, origin: usize) -> impl Iterator<Item = &Arc<Provider>> {
        let origin = origin.min(self.providers.len().saturating_sub(1));
        std::iter::once(&self.providers[origin]).chain(
            self.providers
                .iter()
                .enumerate()
                .filter(move |(i, _)| *i != origin)
                .map(|(_, p)| p),
        )
    }

    /// Pick the provider for the next attempt: the first healthy one (in
    /// priority order from `origin`) that has not been tried in this call;
    /// if every untried provider is cooling down, the one that recovers
    /// soonest (so a fully-tripped pool still probes instead of giving up).
    /// `None` only when every provider has already been tried.
    pub fn route(&self, origin: usize, tried: &[usize]) -> Option<usize> {
        if let Some(p) = self
            .order_from(origin)
            .find(|p| !tried.contains(&p.index) && p.is_healthy())
        {
            return Some(p.index);
        }
        self.order_from(origin)
            .filter(|p| !tried.contains(&p.index))
            .min_by_key(|p| p.cooldown_remaining())
            .map(|p| p.index)
    }

    /// `true` when a retry can go to a healthy provider that has not been
    /// tried yet (then the retry loop fails over without sleeping).
    pub fn has_healthy_untried(&self, origin: usize, tried: &[usize]) -> bool {
        self.order_from(origin)
            .any(|p| !tried.contains(&p.index) && p.is_healthy())
    }

    pub fn record_success(&self, index: usize, latency: Duration) {
        let p = self.provider(index);
        let was_tripped = !p.is_healthy() || p.consecutive_failures() >= self.failure_threshold;
        p.consecutive_failures.store(0, Ordering::Relaxed);
        p.total_ok.fetch_add(1, Ordering::Relaxed);
        p.last_latency_ms
            .store(latency.as_millis() as u64, Ordering::Relaxed);
        p.clear_cooldown();
        if was_tripped {
            debug!(provider = p.label(), "rpc provider recovered");
        }
        let reg = bot_core::obs::metrics::global();
        reg.counter(
            "bot_rpc_provider_requests_total",
            "RPC attempts per provider by outcome.",
            &[("provider", p.label()), ("outcome", "ok")],
        )
        .inc();
        self.publish_health(p);
    }

    /// Record a failed attempt. Trips the breaker after
    /// `failure_threshold` consecutive failures; a rate limit cools the
    /// provider down immediately for `max(retry_after, rate_limit_cooldown)`.
    pub fn record_failure(&self, index: usize, class: RpcErrorClass, error: &str) {
        let p = self.provider(index);
        let consecutive = p.consecutive_failures.fetch_add(1, Ordering::Relaxed) + 1;
        p.total_failures.fetch_add(1, Ordering::Relaxed);
        *p.last_error.lock().unwrap_or_else(|e| e.into_inner()) = Some(self.sanitize(error));
        *p.last_failure_class
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = Some(class.label());

        let reg = bot_core::obs::metrics::global();
        reg.counter(
            "bot_rpc_provider_requests_total",
            "RPC attempts per provider by outcome.",
            &[("provider", p.label()), ("outcome", "error")],
        )
        .inc();
        reg.counter(
            "bot_rpc_errors_total",
            "RPC attempt failures by class.",
            &[("provider", p.label()), ("class", class.label())],
        )
        .inc();

        if let RpcErrorClass::RateLimited { retry_after } = class {
            let pause = retry_after
                .unwrap_or(Duration::ZERO)
                .max(self.retry.rate_limit_cooldown);
            p.set_cooldown(pause);
            reg.counter(
                "bot_rpc_rate_limited_total",
                "HTTP 429 / quota responses per provider.",
                &[("provider", p.label())],
            )
            .inc();
            warn!(
                provider = p.label(),
                pause_ms = pause.as_millis() as u64,
                "rpc provider rate-limited, cooling down"
            );
        } else if class.provider_fault() && consecutive >= self.failure_threshold {
            // Only trip on provider faults: a permanent error (bad pubkey)
            // says nothing about the node.
            let fresh = consecutive == self.failure_threshold || p.is_healthy();
            p.set_cooldown(self.cooldown);
            if fresh {
                reg.counter(
                    "bot_rpc_provider_tripped_total",
                    "Provider circuit-breaker trips.",
                    &[("provider", p.label())],
                )
                .inc();
                warn!(
                    provider = p.label(),
                    consecutive,
                    cooldown_ms = self.cooldown.as_millis() as u64,
                    class = class.label(),
                    "rpc provider unhealthy, failing over"
                );
            }
        }
        self.publish_health(p);
    }

    fn publish_health(&self, p: &Provider) {
        bot_core::obs::metrics::global()
            .gauge(
                "bot_rpc_provider_healthy",
                "1 when the provider is routable, 0 while it cools down.",
                &[("provider", p.label())],
            )
            .set(if p.is_healthy() { 1 } else { 0 });
    }

    /// Health of every provider, primary first.
    pub fn snapshot(&self) -> Vec<ProviderStatus> {
        self.providers.iter().map(|p| p.status()).collect()
    }

    /// Strip every provider URL (API keys!) from an error message and cap
    /// its length before it reaches logs, metrics or the dashboard.
    pub fn sanitize(&self, msg: &str) -> String {
        let mut out = msg.to_string();
        for p in &self.providers {
            if out.contains(&p.url) {
                out = out.replace(&p.url, &p.label);
            }
            let ws = p.ws_url();
            if !ws.is_empty() && out.contains(ws) {
                out = out.replace(ws, &p.label);
            }
        }
        out = redact_url_secrets(&out);
        if out.len() > 240 {
            let mut cut = 240;
            while !out.is_char_boundary(cut) {
                cut -= 1;
            }
            out.truncate(cut);
            out.push('…');
        }
        out
    }
}

/// `host[:port]` of a URL, without scheme, path, query or credentials.
pub fn url_host(url: &str) -> String {
    let no_scheme = url.split_once("://").map(|(_, rest)| rest).unwrap_or(url);
    let authority = no_scheme.split(['/', '?', '#']).next().unwrap_or(no_scheme);
    let host = authority.rsplit('@').next().unwrap_or(authority);
    if host.is_empty() {
        "unknown".to_string()
    } else {
        host.to_string()
    }
}

/// Blank out the query string and any path segment that looks like a key
/// in every `http(s)://` / `ws(s)://` occurrence inside `msg`.
pub fn redact_url_secrets(msg: &str) -> String {
    let mut out = String::with_capacity(msg.len());
    let mut rest = msg;
    loop {
        let Some(pos) = ["https://", "http://", "wss://", "ws://"]
            .iter()
            .filter_map(|p| rest.find(p))
            .min()
        else {
            out.push_str(rest);
            return out;
        };
        out.push_str(&rest[..pos]);
        let url_end = rest[pos..]
            .find(|c: char| c.is_whitespace() || c == '"' || c == '\'' || c == ')' || c == ']')
            .map(|i| pos + i)
            .unwrap_or(rest.len());
        let url = &rest[pos..url_end];
        out.push_str(&format!("<{}>", url_host(url)));
        rest = &rest[url_end..];
    }
}

// ------------------------------------------------------------------------
// tests
// ------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    fn pool(urls: &[&str], threshold: u32, cooldown_ms: u64) -> ProviderPool {
        ProviderPool::new(PoolConfig {
            urls: urls.iter().map(|s| s.to_string()).collect(),
            ws_url: String::new(),
            commitment: CommitmentConfig::confirmed(),
            timeout: Duration::from_millis(500),
            failure_threshold: threshold,
            cooldown: Duration::from_millis(cooldown_ms),
            retry: RetryPolicy::legacy(1),
        })
        .expect("pool builds")
    }

    #[test]
    fn legacy_policy_reproduces_the_old_fixed_backoff() {
        let p = RetryPolicy::legacy(3);
        assert_eq!(p.max_attempts(), 4);
        assert_eq!(p.delay(1, None), Duration::from_millis(50));
        assert_eq!(p.delay(2, None), Duration::from_millis(100));
        assert_eq!(p.delay(3, None), Duration::from_millis(200));
        assert_eq!(p.delay(6, None), Duration::from_millis(1_600));
        assert_eq!(p.delay(40, None), Duration::from_millis(1_600), "capped");
        assert_eq!(
            p.delay(1, Some(Duration::from_secs(2))),
            Duration::from_secs(2),
            "retry-after is a floor"
        );
    }

    #[test]
    fn jittered_delays_stay_inside_the_exponential_envelope() {
        let p = RetryPolicy {
            max_retries: 5,
            base: Duration::from_millis(100),
            max: Duration::from_millis(800),
            jitter: true,
            rate_limit_cooldown: Duration::from_millis(1),
        };
        for attempt in 1..=6 {
            let cap = Duration::from_millis((100u64 << (attempt - 1)).min(800));
            for _ in 0..50 {
                let d = p.delay(attempt, None);
                assert!(d <= cap, "attempt {attempt}: {d:?} > {cap:?}");
                assert!(
                    d >= cap / 8,
                    "attempt {attempt}: floor keeps retries off the same tick"
                );
            }
        }
        let mut distinct = std::collections::HashSet::new();
        for _ in 0..40 {
            distinct.insert(p.delay(4, None).as_millis());
        }
        assert!(distinct.len() > 1, "jitter must actually vary the delay");
    }

    #[test]
    fn policy_from_network_config_uses_the_new_fields() {
        let cfg = NetworkConfig {
            max_retries: 7,
            retry_base_backoff_ms: 20,
            retry_max_backoff_ms: 300,
            retry_jitter: false,
            rate_limit_cooldown_ms: 750,
            ..Default::default()
        };
        let p = RetryPolicy::from_network(&cfg);
        assert_eq!(p.max_attempts(), 8);
        assert_eq!(p.delay(1, None), Duration::from_millis(20));
        assert_eq!(p.delay(10, None), Duration::from_millis(300));
        assert_eq!(p.rate_limit_cooldown, Duration::from_millis(750));
    }

    #[test]
    fn message_classification_matrix() {
        use RpcErrorClass::*;
        let cases: &[(&str, RpcErrorClass)] = &[
            (
                "http 429 too many requests; retry-after=3",
                RateLimited {
                    retry_after: Some(Duration::from_secs(3)),
                },
            ),
            ("Too Many Requests", RateLimited { retry_after: None }),
            (
                "rate limit exceeded for key",
                RateLimited { retry_after: None },
            ),
            ("operation timed out", Timeout),
            ("request deadline exceeded", Timeout),
            (
                "HTTP status server error (503 Service Unavailable)",
                Unavailable,
            ),
            ("Node is unhealthy", Unavailable),
            ("connection refused (os error 111)", Transport),
            ("error sending request for url", Transport),
            ("dns error: failed to lookup host", Transport),
            ("Blockhash not found", Blockhash),
            (
                "Transaction precompile verification failure BlockhashNotFound",
                Blockhash,
            ),
            ("block height exceeded", Blockhash),
            ("Invalid param: WrongSize", Permanent),
            ("insufficient funds for rent", Permanent),
            ("something nobody has seen", Permanent),
        ];
        for (msg, want) in cases {
            assert_eq!(&classify_message(msg), want, "{msg}");
        }
        assert!(Timeout.retryable() && Timeout.ambiguous() && Timeout.provider_fault());
        assert!(!Blockhash.retryable() && Blockhash.needs_rebuild());
        assert!(!Blockhash.provider_fault() && !Blockhash.ambiguous());
        assert!(!Permanent.retryable());
        assert_eq!(
            parse_retry_after("retry-after: 99999"),
            Some(Duration::from_secs(120)),
            "hostile header is capped"
        );
    }

    #[test]
    fn rpc_code_classification_follows_the_custom_error_table() {
        use rpc_custom_error::*;
        assert_eq!(
            classify_rpc_code(
                JSON_RPC_SERVER_ERROR_NODE_UNHEALTHY,
                "Node is behind by 50 slots"
            ),
            RpcErrorClass::Unavailable
        );
        assert_eq!(
            classify_rpc_code(JSON_RPC_SERVER_ERROR_MIN_CONTEXT_SLOT_NOT_REACHED, ""),
            RpcErrorClass::Unavailable
        );
        assert_eq!(
            classify_rpc_code(
                JSON_RPC_SERVER_ERROR_SEND_TRANSACTION_PREFLIGHT_FAILURE,
                "Transaction simulation failed: Blockhash not found"
            ),
            RpcErrorClass::Blockhash
        );
        assert_eq!(
            classify_rpc_code(
                JSON_RPC_SERVER_ERROR_SEND_TRANSACTION_PREFLIGHT_FAILURE,
                "Transaction simulation failed: custom program error 0x1771"
            ),
            RpcErrorClass::Permanent
        );
        assert_eq!(
            classify_rpc_code(
                JSON_RPC_SERVER_ERROR_TRANSACTION_SIGNATURE_VERIFICATION_FAILURE,
                ""
            ),
            RpcErrorClass::Permanent
        );
        assert_eq!(
            classify_rpc_code(-32602, "Invalid params"),
            RpcErrorClass::Permanent
        );
        assert_eq!(
            classify_rpc_code(-32603, "Internal error"),
            RpcErrorClass::Unavailable
        );
        assert_eq!(
            classify_rpc_code(-32429, "Too many requests for this key"),
            RpcErrorClass::RateLimited { retry_after: None }
        );
    }

    #[test]
    fn client_error_kinds_map_to_classes() {
        let io = ClientError::from(ClientErrorKind::Io(std::io::Error::new(
            std::io::ErrorKind::ConnectionReset,
            "reset",
        )));
        assert_eq!(classify_client_error(&io), RpcErrorClass::Transport);
        let custom = ClientError::from(ClientErrorKind::Custom(
            "http 429 too many requests; retry-after=1".into(),
        ));
        assert_eq!(
            classify_client_error(&custom),
            RpcErrorClass::RateLimited {
                retry_after: Some(Duration::from_secs(1))
            }
        );
        let for_user = ClientError::from(ClientErrorKind::RpcError(RpcError::ForUser(
            "unable to confirm transaction".into(),
        )));
        assert_eq!(classify_client_error(&for_user), RpcErrorClass::Unavailable);
        let response = ClientError::from(ClientErrorKind::RpcError(RpcError::RpcResponseError {
            code: -32602,
            message: "Invalid param".into(),
            data: RpcResponseErrorData::Empty,
        }));
        assert_eq!(classify_client_error(&response), RpcErrorClass::Permanent);
    }

    #[test]
    fn labels_and_redaction_never_leak_keys() {
        assert_eq!(
            url_host("https://mainnet.helius-rpc.com/?api-key=SECRET"),
            "mainnet.helius-rpc.com"
        );
        assert_eq!(
            url_host("https://user:pw@rpc.example.com:8899/v1/SECRET"),
            "rpc.example.com:8899"
        );
        assert_eq!(url_host("http://127.0.0.1:8899"), "127.0.0.1:8899");
        let msg =
            "error sending request for url (https://x.quiknode.pro/abc123/): connection reset";
        let red = redact_url_secrets(msg);
        assert!(!red.contains("abc123"), "{red}");
        assert!(red.contains("<x.quiknode.pro>"), "{red}");

        let p = pool(
            &[
                "https://a.example.com/?api-key=K1",
                "https://b.example.com/K2",
            ],
            3,
            1000,
        );
        assert_eq!(p.provider(0).label(), "0:a.example.com");
        assert_eq!(p.provider(1).label(), "1:b.example.com");
        assert_eq!(p.provider(0).ws_url(), "wss://a.example.com/?api-key=K1");
        let s = p.sanitize(
            "failed https://a.example.com/?api-key=K1 and https://b.example.com/K2 badly",
        );
        assert!(!s.contains("K1") && !s.contains("K2"), "{s}");
        let long = "x".repeat(1000);
        assert!(p.sanitize(&long).chars().count() <= 241);
    }

    #[test]
    fn pool_dedups_and_requires_an_endpoint() {
        let p = pool(&["https://a", " https://a ", "", "https://b"], 3, 1000);
        assert_eq!(p.len(), 2);
        assert!(ProviderPool::new(PoolConfig {
            urls: vec!["".into(), "  ".into()],
            ws_url: String::new(),
            commitment: CommitmentConfig::confirmed(),
            timeout: Duration::from_millis(10),
            failure_threshold: 1,
            cooldown: Duration::from_millis(1),
            retry: RetryPolicy::default(),
        })
        .is_err());
        let with_ws = ProviderPool::new(PoolConfig {
            urls: vec!["https://a".into(), "https://b".into()],
            ws_url: "wss://custom/ws".into(),
            commitment: CommitmentConfig::confirmed(),
            timeout: Duration::from_millis(10),
            failure_threshold: 1,
            cooldown: Duration::from_millis(1),
            retry: RetryPolicy::default(),
        })
        .unwrap();
        assert_eq!(with_ws.provider(0).ws_url(), "wss://custom/ws");
        assert_eq!(with_ws.provider(1).ws_url(), "wss://b");
    }

    #[test]
    fn breaker_trips_after_threshold_and_routes_around_it() {
        let p = pool(&["https://p", "https://f1", "https://f2"], 2, 60_000);
        assert_eq!(p.route(0, &[]), Some(0));
        assert_eq!(p.route(0, &[0]), Some(1), "priority order after the origin");
        assert_eq!(
            p.route(2, &[]),
            Some(2),
            "a failover handle starts at its own origin"
        );
        assert_eq!(
            p.route(2, &[2]),
            Some(0),
            "…then the rest in configured order"
        );
        assert_eq!(p.route(0, &[0, 1, 2]), None);

        p.record_failure(0, RpcErrorClass::Transport, "reset");
        assert!(
            p.provider(0).is_healthy(),
            "one failure is below the threshold"
        );
        p.record_failure(0, RpcErrorClass::Transport, "reset");
        assert!(
            !p.provider(0).is_healthy(),
            "second consecutive failure trips"
        );
        assert_eq!(p.healthy_count(), 2);
        assert_eq!(p.route(0, &[]), Some(1), "tripped primary is skipped");
        assert!(p.has_healthy_untried(0, &[]));

        p.record_success(0, Duration::from_millis(3));
        assert!(p.provider(0).is_healthy(), "success closes the breaker");
        assert_eq!(p.provider(0).consecutive_failures(), 0);
        assert_eq!(p.snapshot()[0].total_failures, 2);
        assert_eq!(p.snapshot()[0].total_ok, 1);
        assert_eq!(
            p.snapshot()[0].last_failure_class.as_deref(),
            Some("transport")
        );
    }

    #[test]
    fn permanent_errors_never_trip_the_breaker() {
        let p = pool(&["https://p", "https://f1"], 1, 60_000);
        for _ in 0..5 {
            p.record_failure(0, RpcErrorClass::Permanent, "Invalid param");
        }
        assert!(
            p.provider(0).is_healthy(),
            "bad pubkeys say nothing about the node"
        );
        assert_eq!(p.route(0, &[]), Some(0));
    }

    #[test]
    fn rate_limit_cools_down_immediately_and_honours_retry_after() {
        let p = pool(&["https://p", "https://f1"], 5, 10);
        p.record_failure(
            0,
            RpcErrorClass::RateLimited {
                retry_after: Some(Duration::from_secs(30)),
            },
            "429",
        );
        assert!(!p.provider(0).is_healthy(), "one 429 is enough");
        let remaining = p.provider(0).cooldown_remaining();
        assert!(remaining > Duration::from_secs(25), "{remaining:?}");
        assert_eq!(p.route(0, &[]), Some(1));
        // Without a hint the configured rate-limit cooldown applies.
        p.record_failure(1, RpcErrorClass::RateLimited { retry_after: None }, "429");
        let r1 = p.provider(1).cooldown_remaining();
        assert!(
            r1 <= Duration::from_secs(1) && r1 > Duration::ZERO,
            "{r1:?}"
        );
    }

    #[test]
    fn fully_tripped_pool_probes_the_soonest_recovering_provider() {
        let p = pool(&["https://p", "https://f1"], 1, 50);
        p.record_failure(0, RpcErrorClass::Unavailable, "503");
        std::thread::sleep(Duration::from_millis(20));
        p.record_failure(1, RpcErrorClass::Unavailable, "503");
        assert_eq!(p.healthy_count(), 0);
        assert_eq!(p.route(0, &[]), Some(0), "primary recovers first");
        assert!(!p.has_healthy_untried(0, &[]));
        std::thread::sleep(Duration::from_millis(60));
        assert!(
            p.provider(0).is_healthy() && p.provider(1).is_healthy(),
            "cooldown expires"
        );
        // Half-open: one more failure re-trips without waiting for the
        // threshold to be reached again.
        p.record_failure(0, RpcErrorClass::Timeout, "slow");
        assert!(!p.provider(0).is_healthy());
    }

    /// Minimal HTTP/1.1 responder used to exercise the metered sender.
    async fn spawn_http(
        status_line: &'static str,
        extra_headers: &'static str,
        body: &'static str,
    ) -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move {
            loop {
                if let Ok((mut sock, _)) = listener.accept().await {
                    let mut buf = [0u8; 8192];
                    let _ = sock.read(&mut buf).await;
                    let resp = format!(
                        "HTTP/1.1 {status_line}\r\ncontent-type: application/json\r\ncontent-length: {}\r\n{extra_headers}connection: close\r\n\r\n{body}",
                        body.len()
                    );
                    let _ = sock.write_all(resp.as_bytes()).await;
                    let _ = sock.shutdown().await;
                }
            }
        });
        addr
    }

    #[tokio::test]
    async fn metered_sender_returns_429_immediately_with_retry_after() {
        let url = spawn_http("429 Too Many Requests", "retry-after: 2\r\n", "{}").await;
        let p = pool(&[&url], 3, 1000);
        let started = Instant::now();
        let err = p
            .provider(0)
            .client()
            .get_slot()
            .await
            .expect_err("429 is an error");
        assert!(
            started.elapsed() < Duration::from_millis(1500),
            "the stock sender would have slept 2 s here: {:?}",
            started.elapsed()
        );
        assert_eq!(
            classify_client_error(&err),
            RpcErrorClass::RateLimited {
                retry_after: Some(Duration::from_secs(2))
            },
            "{err}"
        );
    }

    #[tokio::test]
    async fn metered_sender_surfaces_json_rpc_errors_and_results() {
        let url = spawn_http(
            "200 OK",
            "",
            r#"{"jsonrpc":"2.0","error":{"code":-32005,"message":"Node is unhealthy","data":{"numSlotsBehind":42}},"id":1}"#,
        )
        .await;
        let p = pool(&[&url], 3, 1000);
        let err = p
            .provider(0)
            .client()
            .get_slot()
            .await
            .expect_err("rpc error");
        match &err.kind {
            ClientErrorKind::RpcError(RpcError::RpcResponseError { code, data, .. }) => {
                assert_eq!(*code, -32005);
                assert!(matches!(
                    data,
                    RpcResponseErrorData::NodeUnhealthy {
                        num_slots_behind: Some(42)
                    }
                ));
            }
            other => panic!("unexpected kind: {other:?}"),
        }
        assert_eq!(classify_client_error(&err), RpcErrorClass::Unavailable);

        let ok = spawn_http("200 OK", "", r#"{"jsonrpc":"2.0","result":12345,"id":1}"#).await;
        let p = pool(&[&ok], 3, 1000);
        assert_eq!(p.provider(0).client().get_slot().await.unwrap(), 12345);
        assert_eq!(
            p.provider(0).client().get_transport_stats().request_count,
            1
        );

        let five = spawn_http("503 Service Unavailable", "", "busy").await;
        let p = pool(&[&five], 3, 1000);
        let err = p.provider(0).client().get_slot().await.expect_err("503");
        assert_eq!(
            classify_client_error(&err),
            RpcErrorClass::Unavailable,
            "{err}"
        );
    }
}
