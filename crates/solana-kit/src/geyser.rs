//! Yellowstone (Solana Geyser) gRPC streaming ingestion.
//!
//! WHY THIS EXISTS
//!   The rest of the workspace learns about on-chain activity either by polling
//!   JSON-RPC (`provider.rs`) or by a plain WebSocket `transactionSubscribe`
//!   (`ws.rs`). Both are lossy and slow compared with a Geyser plugin: the RPC
//!   node never tells you about a transaction it has not indexed yet, and the
//!   public WS endpoint drops subscriptions under load.
//!
//!   This module is a real Yellowstone client: it speaks the `geyser.Geyser`
//!   gRPC service (`Subscribe` server-streaming), sets the `x-token` metadata
//!   header used by every hosted provider, asks for transactions filtered by
//!   program id / vote / failure, and turns the resulting stream into
//!   [`GeyserTransaction`] values the ingestion pipeline can consume.
//!
//! SCOPE
//!   * transactions stream only (`transactions`, not `transactions_status`,
//!     slots, blocks, accounts). That is what the copy-trading pipeline needs:
//!     "which program executed, in which slot, with which signature".
//!   * reconnect/backoff is the caller's policy (see [`GeyserConfig::max_reconnects`]
//!     and [`GeyserSubscription::connect_with_retry`]); the stream itself is
//!     push-based and ends when the server closes it.
//!
//! NO CREDENTIALS ARE REQUIRED TO COMPILE OR TO RUN THE TESTS
//!   `tests/geyser_ingest.rs` starts an in-process Yellowstone gRPC server,
//!   serves real protobuf over a real TCP socket, and proves the client
//!   connects, sends the documented filter, receives transactions and drops
//!   vote transactions. A live endpoint is only needed to ingest mainnet data.

use std::collections::HashMap;
use std::time::Duration;

use tonic::metadata::MetadataValue;
use tonic::service::Interceptor;
use tonic::transport::Channel;
use tonic::Request;
use yellowstone_grpc_proto::geyser::geyser_client::GeyserClient;
use yellowstone_grpc_proto::geyser::subscribe_update::UpdateOneof;
use yellowstone_grpc_proto::geyser::{
    CommitmentLevel, SubscribeRequest, SubscribeRequestFilterTransactions, SubscribeUpdate,
};

/// Errors raised by the Geyser client.
#[derive(Debug, thiserror::Error)]
pub enum GeyserError {
    /// The transport could not be established (DNS, TLS, refused, timeout).
    #[error("geyser transport error: {0}")]
    Transport(String),
    /// The server rejected the subscription or the stream failed mid-flight.
    #[error("geyser status error: {0}")]
    Status(String),
    /// An update arrived in a shape the client does not understand.
    #[error("geyser protocol error: {0}")]
    Protocol(String),
}

/// Configuration for a single Geyser subscription.
#[derive(Debug, Clone)]
pub struct GeyserConfig {
    /// gRPC endpoint, e.g. `https://grpc.example.com:443` or `http://127.0.0.1:10000`.
    pub endpoint: String,
    /// Value for the `x-token` metadata header (most hosted providers require it).
    pub x_token: Option<String>,
    /// Commitment level requested from the server.
    pub commitment: Commitment,
    /// Program ids to watch. Empty means "every transaction the server sends",
    /// which is almost never what a sniper wants — always set this in prod.
    pub account_include: Vec<String>,
    /// Include vote transactions (default: false — they are 70%+ of mainnet traffic).
    pub include_votes: bool,
    /// Include failed transactions (default: false).
    pub include_failed: bool,
    /// Per-connect timeout.
    pub connect_timeout: Duration,
    /// How many consecutive connect attempts `connect_with_retry` makes.
    pub max_reconnects: u32,
}

/// Commitment level of a Geyser subscription.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Commitment {
    /// Processed (fastest, can be rolled back).
    Processed,
    /// Confirmed (supermajority).
    Confirmed,
    /// Finalized (irreversible).
    Finalized,
}

impl Commitment {
    /// The protobuf enum value sent on the wire.
    pub fn as_i32(self) -> i32 {
        match self {
            Commitment::Processed => CommitmentLevel::Processed as i32,
            Commitment::Confirmed => CommitmentLevel::Confirmed as i32,
            Commitment::Finalized => CommitmentLevel::Finalized as i32,
        }
    }

    /// Parse the names used in `config.toml` / environment variables.
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "processed" => Some(Commitment::Processed),
            "confirmed" => Some(Commitment::Confirmed),
            "finalized" => Some(Commitment::Finalized),
            _ => None,
        }
    }
}

impl Default for GeyserConfig {
    fn default() -> Self {
        Self {
            endpoint: String::new(),
            x_token: None,
            commitment: Commitment::Confirmed,
            account_include: Vec::new(),
            include_votes: false,
            include_failed: false,
            connect_timeout: Duration::from_secs(10),
            max_reconnects: 3,
        }
    }
}

impl GeyserConfig {
    /// A config for `endpoint` watching the given program ids.
    pub fn new(endpoint: impl Into<String>, account_include: Vec<String>) -> Self {
        Self {
            endpoint: endpoint.into(),
            account_include,
            ..Self::default()
        }
    }

    /// Set the `x-token` header value.
    pub fn with_x_token(mut self, token: impl Into<String>) -> Self {
        self.x_token = Some(token.into());
        self
    }

    /// The `SubscribeRequest` this config produces.
    ///
    /// Exposed (and unit-tested) because the filter shape *is* the contract with
    /// the server: getting `vote`/`failed`/`account_include` wrong silently
    /// floods the pipeline instead of failing loudly.
    pub fn subscribe_request(&self) -> SubscribeRequest {
        let mut transactions = HashMap::new();
        transactions.insert(
            "sniper-suite".to_string(),
            SubscribeRequestFilterTransactions {
                vote: Some(self.include_votes),
                failed: Some(self.include_failed),
                account_include: self.account_include.clone(),
                ..Default::default()
            },
        );

        SubscribeRequest {
            transactions,
            commitment: Some(self.commitment.as_i32()),
            ..Default::default()
        }
    }
}

/// One transaction notification from the Geyser stream.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GeyserTransaction {
    /// Slot the transaction was executed in.
    pub slot: u64,
    /// Base58 transaction signature.
    pub signature: String,
    /// Whether this is a vote transaction (always false when
    /// [`GeyserConfig::include_votes`] is false, asserted client-side too).
    pub is_vote: bool,
    /// Index of the transaction within the block.
    pub index: u64,
}

/// A live Geyser subscription.
pub struct GeyserSubscription {
    stream: tonic::Streaming<SubscribeUpdate>,
    client: GeyserClient<tonic::service::interceptor::InterceptedService<Channel, XToken>>,
}

/// Adds the `x-token` header to every request when one is configured.
#[derive(Clone)]
pub struct XToken(Option<MetadataValue<tonic::metadata::Ascii>>);

impl Interceptor for XToken {
    fn call(&mut self, mut req: Request<()>) -> Result<Request<()>, tonic::Status> {
        if let Some(value) = &self.0 {
            req.metadata_mut().insert("x-token", value.clone());
        }
        Ok(req)
    }
}

impl XToken {
    /// Build an interceptor from an optional raw token.
    pub fn new(token: Option<&str>) -> Result<Self, GeyserError> {
        match token {
            None => Ok(Self(None)),
            Some(raw) => {
                let value = MetadataValue::try_from(raw).map_err(|e| {
                    GeyserError::Protocol(format!("x-token is not valid metadata: {e}"))
                })?;
                Ok(Self(Some(value)))
            }
        }
    }
}

impl GeyserSubscription {
    /// Connect and subscribe once.
    pub async fn connect(cfg: &GeyserConfig) -> Result<Self, GeyserError> {
        let channel = Channel::from_shared(cfg.endpoint.clone())
            .map_err(|e| GeyserError::Transport(format!("invalid endpoint: {e}")))?
            .connect_timeout(cfg.connect_timeout)
            .connect()
            .await
            .map_err(|e| GeyserError::Transport(e.to_string()))?;

        let mut client =
            GeyserClient::with_interceptor(channel, XToken::new(cfg.x_token.as_deref())?);
        // yellowstone 13 declares `Subscribe` as a bidi stream, so the request
        // is a one-item stream rather than a unary message.
        let stream = client
            .subscribe(tokio_stream::once(cfg.subscribe_request()))
            .await
            .map_err(|e| GeyserError::Status(e.to_string()))?
            .into_inner();

        Ok(Self { stream, client })
    }

    /// Connect, retrying up to [`GeyserConfig::max_reconnects`] times with a
    /// linear backoff (250 ms × attempt). The last error is returned.
    pub async fn connect_with_retry(cfg: &GeyserConfig) -> Result<Self, GeyserError> {
        let attempts = cfg.max_reconnects.max(1);
        let mut last: Option<GeyserError> = None;
        for attempt in 1..=attempts {
            match Self::connect(cfg).await {
                Ok(sub) => return Ok(sub),
                Err(e) => {
                    tracing::warn!(attempt, error = %e, "geyser connect failed");
                    last = Some(e);
                    tokio::time::sleep(Duration::from_millis(250 * u64::from(attempt))).await;
                }
            }
        }
        Err(last.unwrap_or(GeyserError::Transport("no attempt made".into())))
    }

    /// The next transaction notification.
    ///
    /// `Ok(None)` means the server closed the stream cleanly. Ping/pong and any
    /// other non-transaction update is consumed and skipped rather than being
    /// surfaced, so callers only ever see transactions.
    pub async fn next_transaction(&mut self) -> Result<Option<GeyserTransaction>, GeyserError> {
        loop {
            let update = match self.stream.message().await {
                Ok(Some(u)) => u,
                Ok(None) => return Ok(None),
                Err(e) => return Err(GeyserError::Status(e.to_string())),
            };
            match update.update_oneof {
                Some(UpdateOneof::Transaction(tx)) => {
                    let info = tx.transaction.ok_or_else(|| {
                        GeyserError::Protocol("transaction update without payload".into())
                    })?;
                    // Defence in depth: the filter asks the server not to send
                    // votes, but a non-conforming server must not poison the
                    // pipeline.
                    if info.is_vote {
                        continue;
                    }
                    return Ok(Some(GeyserTransaction {
                        slot: tx.slot,
                        signature: bs58::encode(&info.signature).into_string(),
                        is_vote: info.is_vote,
                        index: info.index,
                    }));
                }
                Some(UpdateOneof::Ping(_)) => continue,
                Some(_) => continue,
                None => continue,
            }
        }
    }

    /// Send a keep-alive ping and return the server's pong count.
    pub async fn ping(&mut self) -> Result<i32, GeyserError> {
        let resp = self
            .client
            .ping(yellowstone_grpc_proto::geyser::PingRequest { count: 1 })
            .await
            .map_err(|e| GeyserError::Status(e.to_string()))?;
        Ok(resp.into_inner().count)
    }

    /// Mutable access to the raw stream, for callers that need updates other
    /// than transactions.
    pub fn raw_stream(&mut self) -> &mut tonic::Streaming<SubscribeUpdate> {
        &mut self.stream
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn subscribe_request_sets_filter_and_commitment() {
        let mut cfg = GeyserConfig::new("http://127.0.0.1:1", vec!["pumpAmid".into()]);
        cfg.commitment = Commitment::Processed;
        let req = cfg.subscribe_request();
        assert_eq!(req.commitment, Some(CommitmentLevel::Processed as i32));
        let filter = req.transactions.get("sniper-suite").expect("filter");
        assert_eq!(filter.vote, Some(false));
        assert_eq!(filter.failed, Some(false));
        assert_eq!(filter.account_include, vec!["pumpAmid".to_string()]);
        assert!(req.accounts.is_empty());
        assert!(req.slots.is_empty());
        assert!(req.blocks.is_empty());
    }

    #[test]
    fn votes_and_failures_can_be_requested() {
        let cfg = GeyserConfig {
            include_votes: true,
            include_failed: true,
            ..GeyserConfig::default()
        };
        let filter = cfg
            .subscribe_request()
            .transactions
            .remove("sniper-suite")
            .unwrap();
        assert_eq!(filter.vote, Some(true));
        assert_eq!(filter.failed, Some(true));
    }

    #[test]
    fn commitment_parsing_matches_config_names() {
        assert_eq!(Commitment::parse("processed"), Some(Commitment::Processed));
        assert_eq!(
            Commitment::parse(" CONFIRMED "),
            Some(Commitment::Confirmed)
        );
        assert_eq!(Commitment::parse("finalized"), Some(Commitment::Finalized));
        assert_eq!(Commitment::parse("nope"), None);
    }

    #[test]
    fn x_token_validation_matches_http_metadata_rules() {
        // Visible ASCII and obs-text are legal header values …
        assert!(XToken::new(Some("ok-token")).is_ok());
        assert!(XToken::new(None).is_ok());
        // … control characters are not: a token with a newline would otherwise
        // be rejected only at request time, deep inside tonic.
        assert!(XToken::new(Some("bad\ntoken")).is_err());
        assert!(XToken::new(Some("bad\0token")).is_err());
    }
}
