//! Yellowstone Geyser ingestion, wired into the running bot.
//!
//! The client itself lives in `solana_kit::geyser` (real `geyser.Geyser` gRPC,
//! `x-token` auth, transaction filters, tested against an in-process gRPC
//! server). This module is the process-level wiring: it decides from the
//! environment whether to start a subscription at all, owns the reconnect
//! loop, and exposes counters the operator can read.
//!
//! CONFIGURATION (all optional — without them the bot behaves exactly as
//! before, on JSON-RPC/polling):
//!
//! | env                 | meaning                                             |
//! |---------------------|-----------------------------------------------------|
//! | `GEYSER_ENDPOINT`   | `https://host:443` — presence is what enables ingest |
//! | `GEYSER_X_TOKEN`    | value for the `x-token` metadata header             |
//! | `GEYSER_PROGRAMS`   | comma-separated base58 program ids to watch         |
//! | `GEYSER_COMMITMENT` | `processed` / `confirmed` / `finalized`             |
//!
//! Failure policy: a Geyser endpoint that is down never stops the bot. The
//! task retries with the client's backoff, logs at `warn` once per attempt
//! batch, and keeps the existing RPC path authoritative. That is deliberate —
//! a streaming accelerator must not become a new single point of failure.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use bot_core::lifecycle::Shutdown;
use solana_kit::geyser::{Commitment, GeyserConfig, GeyserSubscription};

/// Everything needed to start an ingest task.
#[derive(Debug, Clone)]
pub struct GeyserIngestConfig {
    /// gRPC endpoint of the Yellowstone-compatible provider.
    pub endpoint: String,
    /// `x-token` value, if the provider requires one.
    pub x_token: Option<String>,
    /// Program ids to watch (empty = everything the server sends).
    pub programs: Vec<String>,
    /// Commitment level.
    pub commitment: Commitment,
    /// Connect attempts before the task backs off to a slow retry.
    pub max_reconnects: u32,
}

impl GeyserIngestConfig {
    /// Read the configuration from the environment.
    ///
    /// Returns `None` when `GEYSER_ENDPOINT` is unset or blank — the only
    /// switch that turns ingest on.
    pub fn from_env() -> Option<Self> {
        let endpoint = std::env::var("GEYSER_ENDPOINT").ok()?;
        let endpoint = endpoint.trim().to_string();
        if endpoint.is_empty() {
            return None;
        }
        let x_token = std::env::var("GEYSER_X_TOKEN")
            .ok()
            .filter(|t| !t.is_empty());
        let programs = std::env::var("GEYSER_PROGRAMS")
            .ok()
            .map(|raw| {
                raw.split(',')
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(str::to_string)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let commitment = std::env::var("GEYSER_COMMITMENT")
            .ok()
            .and_then(|c| Commitment::parse(&c))
            .unwrap_or(Commitment::Confirmed);
        Some(Self {
            endpoint,
            x_token,
            programs,
            commitment,
            max_reconnects: 3,
        })
    }

    /// The client configuration this wiring produces.
    pub fn client_config(&self) -> GeyserConfig {
        GeyserConfig {
            endpoint: self.endpoint.clone(),
            x_token: self.x_token.clone(),
            commitment: self.commitment,
            account_include: self.programs.clone(),
            include_votes: false,
            include_failed: false,
            connect_timeout: Duration::from_secs(10),
            max_reconnects: self.max_reconnects,
        }
    }
}

/// Counters for `/api/status`-style reporting.
#[derive(Debug, Default)]
pub struct GeyserIngestStats {
    transactions: AtomicU64,
    votes_dropped: AtomicU64,
    reconnects: AtomicU64,
    last_slot: AtomicU64,
}

impl GeyserIngestStats {
    /// Transactions ingested since process start.
    pub fn transactions(&self) -> u64 {
        self.transactions.load(Ordering::Relaxed)
    }

    /// Stream terminations that forced a reconnect.
    pub fn reconnects(&self) -> u64 {
        self.reconnects.load(Ordering::Relaxed)
    }

    /// Highest slot seen.
    pub fn last_slot(&self) -> u64 {
        self.last_slot.load(Ordering::Relaxed)
    }

    /// Vote transactions refused by the client-side guard.
    pub fn votes_dropped(&self) -> u64 {
        self.votes_dropped.load(Ordering::Relaxed)
    }
}

/// A started ingest task.
pub struct GeyserIngest {
    config: GeyserIngestConfig,
    stats: Arc<GeyserIngestStats>,
}

impl GeyserIngest {
    /// Build the ingest task (does not connect yet).
    pub fn new(config: GeyserIngestConfig) -> Self {
        Self {
            config,
            stats: Arc::new(GeyserIngestStats::default()),
        }
    }

    /// Shared counters.
    pub fn stats(&self) -> Arc<GeyserIngestStats> {
        self.stats.clone()
    }

    /// Spawn the reconnect loop. The task exits when `shutdown` fires.
    pub fn spawn(self, shutdown: Arc<Shutdown>) -> tokio::task::JoinHandle<()> {
        let cfg = self.config;
        let stats = self.stats;
        tokio::spawn(async move {
            let client_cfg = cfg.client_config();
            loop {
                tokio::select! {
                    _ = shutdown.wait() => {
                        tracing::info!(
                            transactions = stats.transactions(),
                            "geyser ingest: stopping on shutdown"
                        );
                        return;
                    }
                    sub = GeyserSubscription::connect_with_retry(&client_cfg) => {
                        match sub {
                            Ok(mut sub) => {
                                tracing::info!(
                                    endpoint = %cfg.endpoint,
                                    programs = ?cfg.programs,
                                    commitment = ?cfg.commitment,
                                    "geyser ingest: subscribed"
                                );
                                loop {
                                    tokio::select! {
                                        _ = shutdown.wait() => return,
                                        next = sub.next_transaction() => match next {
                                            Ok(Some(tx)) => {
                                                stats.transactions.fetch_add(1, Ordering::Relaxed);
                                                stats.last_slot.fetch_max(tx.slot, Ordering::Relaxed);
                                                if tx.is_vote {
                                                    stats.votes_dropped.fetch_add(1, Ordering::Relaxed);
                                                }
                                                tracing::debug!(
                                                    slot = tx.slot,
                                                    signature = %tx.signature,
                                                    index = tx.index,
                                                    "geyser ingest: transaction"
                                                );
                                            }
                                            Ok(None) => {
                                                stats.reconnects.fetch_add(1, Ordering::Relaxed);
                                                tracing::warn!(
                                                    "geyser ingest: server closed the stream — reconnecting"
                                                );
                                                break;
                                            }
                                            Err(e) => {
                                                stats.reconnects.fetch_add(1, Ordering::Relaxed);
                                                tracing::warn!(error = %e, "geyser ingest: stream error — reconnecting");
                                                break;
                                            }
                                        }
                                    }
                                }
                            }
                            Err(e) => {
                                stats.reconnects.fetch_add(1, Ordering::Relaxed);
                                tracing::warn!(
                                    endpoint = %cfg.endpoint,
                                    error = %e,
                                    "geyser ingest: could not subscribe — retrying in 30 s (RPC path stays authoritative)"
                                );
                                tokio::select! {
                                    _ = shutdown.wait() => return,
                                    _ = tokio::time::sleep(Duration::from_secs(30)) => {}
                                }
                            }
                        }
                    }
                }
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_is_off_without_endpoint() {
        std::env::remove_var("GEYSER_ENDPOINT");
        assert!(GeyserIngestConfig::from_env().is_none());
    }

    #[test]
    fn config_parses_programs_and_commitment() {
        std::env::set_var("GEYSER_ENDPOINT", "https://geyser.example.com:443");
        std::env::set_var("GEYSER_PROGRAMS", " progA , progB ,, ");
        std::env::set_var("GEYSER_COMMITMENT", "processed");
        let cfg = GeyserIngestConfig::from_env().expect("config");
        assert_eq!(cfg.endpoint, "https://geyser.example.com:443");
        assert_eq!(cfg.programs, vec!["progA".to_string(), "progB".to_string()]);
        assert_eq!(cfg.commitment, Commitment::Processed);

        let client = cfg.client_config();
        assert_eq!(client.account_include, cfg.programs);
        assert!(!client.include_votes, "votes are never ingested");
        assert!(!client.include_failed);
        std::env::remove_var("GEYSER_ENDPOINT");
        std::env::remove_var("GEYSER_PROGRAMS");
        std::env::remove_var("GEYSER_COMMITMENT");
    }

    #[test]
    fn stats_start_at_zero() {
        let stats = GeyserIngestStats::default();
        assert_eq!(stats.transactions(), 0);
        assert_eq!(stats.reconnects(), 0);
        assert_eq!(stats.last_slot(), 0);
    }
}
