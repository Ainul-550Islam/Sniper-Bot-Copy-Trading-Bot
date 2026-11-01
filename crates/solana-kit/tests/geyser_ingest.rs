//! End-to-end Geyser ingestion test.
//!
//! A real Yellowstone gRPC server (`geyser.Geyser`) is started on a loopback
//! TCP port with `tonic`. It records the `SubscribeRequest` and the metadata it
//! received, then pushes a fixture stream: one vote transaction, one
//! non-transaction update (ping), and one real transaction. The client under
//! test must:
//!
//!   1. connect over TCP and authenticate with `x-token`,
//!   2. send the documented filter (`vote=false`, `failed=false`,
//!      `account_include=[OUR PROGRAM]`) at `Confirmed`,
//!   3. skip the vote and the ping and surface exactly one transaction,
//!   4. return `Ok(None)` when the server closes the stream,
//!   5. answer `Ping` with a pong.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use solana_kit::geyser::{Commitment, GeyserConfig, GeyserSubscription};
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use tonic::transport::Server;
use tonic::{Request, Response, Status};

use yellowstone_grpc_proto::geyser::geyser_server::{Geyser, GeyserServer};
use yellowstone_grpc_proto::geyser::subscribe_update::UpdateOneof;
use yellowstone_grpc_proto::geyser::{
    subscribe_update, GetBlockHeightRequest, GetBlockHeightResponse, GetLatestBlockhashRequest,
    GetLatestBlockhashResponse, GetSlotRequest, GetSlotResponse, GetVersionRequest,
    GetVersionResponse, IsBlockhashValidRequest, IsBlockhashValidResponse, PingRequest,
    PongResponse, SubscribeDeshredRequest, SubscribeGossipRequest, SubscribeReplayInfoRequest,
    SubscribeReplayInfoResponse, SubscribeRequest, SubscribeRequestFilterTransactions,
    SubscribeUpdate, SubscribeUpdateDeshred, SubscribeUpdateGossip, SubscribeUpdatePing,
    SubscribeUpdateTransaction, SubscribeUpdateTransactionInfo,
};

const PROGRAM: &str = "6EF8rrecthR5Dkzon8Nwu78hRvfCKubJ14M5uBEwF6P"; // pump.fun
const TX_SIG: [u8; 4] = [0xde, 0xad, 0xbe, 0xef];

#[derive(Clone, Default)]
struct RecordingGeyser {
    seen: Arc<Mutex<Option<Observed>>>,
}

#[derive(Debug, Clone)]
struct Observed {
    request: SubscribeRequest,
    x_token: Option<String>,
}

fn tx_update(slot: u64, is_vote: bool, signature: Vec<u8>, index: u64) -> SubscribeUpdate {
    SubscribeUpdate {
        filters: vec!["sniper-suite".to_string()],
        update_oneof: Some(UpdateOneof::Transaction(SubscribeUpdateTransaction {
            slot,
            bank_id: 0,
            transaction: Some(SubscribeUpdateTransactionInfo {
                signature,
                is_vote,
                index,
                ..Default::default()
            }),
        })),
        created_at: None,
    }
}

#[tonic::async_trait]
impl Geyser for RecordingGeyser {
    type SubscribeStream = ReceiverStream<Result<SubscribeUpdate, Status>>;

    async fn subscribe(
        &self,
        request: Request<tonic::Streaming<SubscribeRequest>>,
    ) -> Result<Response<Self::SubscribeStream>, Status> {
        let x_token = request
            .metadata()
            .get("x-token")
            .map(|v| v.to_str().unwrap_or("<non-ascii>").to_string());
        // `Subscribe` is a bidi stream: read the first (and only) request.
        let mut request = request.into_inner();
        let first = request
            .message()
            .await?
            .ok_or_else(|| Status::invalid_argument("empty subscribe stream"))?;
        *self.seen.lock().unwrap() = Some(Observed {
            request: first,
            x_token,
        });

        let (tx, rx) = mpsc::channel(8);
        tokio::spawn(async move {
            // 1. a vote transaction — the client must drop it
            let _ = tx.send(Ok(tx_update(10, true, vec![1, 2, 3], 0))).await;
            // 2. a non-transaction update — the client must skip it
            let ping = SubscribeUpdate {
                filters: vec![],
                update_oneof: Some(UpdateOneof::Ping(SubscribeUpdatePing {})),
                created_at: None,
            };
            let _ = tx.send(Ok(ping)).await;
            // 3. the transaction we care about
            let _ = tx.send(Ok(tx_update(11, false, TX_SIG.to_vec(), 7))).await;
            // 4. clean end of stream
        });
        Ok(Response::new(ReceiverStream::new(rx)))
    }

    async fn ping(&self, _request: Request<PingRequest>) -> Result<Response<PongResponse>, Status> {
        Ok(Response::new(PongResponse { count: 42 }))
    }

    // The remaining RPCs are not exercised by this suite; a mock that returns
    // UNIMPLEMENTED is exactly what a partial provider looks like.
    type SubscribeDeshredStream = tokio_stream::Empty<Result<SubscribeUpdateDeshred, Status>>;
    async fn subscribe_deshred(
        &self,
        _request: Request<tonic::Streaming<SubscribeDeshredRequest>>,
    ) -> Result<Response<Self::SubscribeDeshredStream>, Status> {
        Err(Status::unimplemented("deshred not served by this mock"))
    }

    type SubscribeGossipStream = tokio_stream::Empty<Result<SubscribeUpdateGossip, Status>>;
    async fn subscribe_gossip(
        &self,
        _request: Request<SubscribeGossipRequest>,
    ) -> Result<Response<Self::SubscribeGossipStream>, Status> {
        Err(Status::unimplemented("gossip not served by this mock"))
    }

    async fn subscribe_replay_info(
        &self,
        _request: Request<SubscribeReplayInfoRequest>,
    ) -> Result<Response<SubscribeReplayInfoResponse>, Status> {
        Err(Status::unimplemented("replay info not served by this mock"))
    }

    async fn get_latest_blockhash(
        &self,
        _request: Request<GetLatestBlockhashRequest>,
    ) -> Result<Response<GetLatestBlockhashResponse>, Status> {
        Err(Status::unimplemented("not served by this mock"))
    }

    async fn get_block_height(
        &self,
        _request: Request<GetBlockHeightRequest>,
    ) -> Result<Response<GetBlockHeightResponse>, Status> {
        Err(Status::unimplemented("not served by this mock"))
    }

    async fn get_slot(
        &self,
        _request: Request<GetSlotRequest>,
    ) -> Result<Response<GetSlotResponse>, Status> {
        Err(Status::unimplemented("not served by this mock"))
    }

    async fn is_blockhash_valid(
        &self,
        _request: Request<IsBlockhashValidRequest>,
    ) -> Result<Response<IsBlockhashValidResponse>, Status> {
        Err(Status::unimplemented("not served by this mock"))
    }

    async fn get_version(
        &self,
        _request: Request<GetVersionRequest>,
    ) -> Result<Response<GetVersionResponse>, Status> {
        Err(Status::unimplemented("not served by this mock"))
    }
}

async fn serve(mock: RecordingGeyser) -> SocketAddr {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        Server::builder()
            .add_service(GeyserServer::new(mock))
            .serve_with_incoming(tokio_stream::wrappers::TcpListenerStream::new(listener))
            .await
            .unwrap();
    });
    // give the server a moment to accept
    tokio::time::sleep(Duration::from_millis(100)).await;
    addr
}

#[tokio::test]
async fn streams_transactions_and_drops_votes_over_real_grpc() {
    let mock = RecordingGeyser::default();
    let seen = mock.seen.clone();
    let addr = serve(mock).await;

    let mut cfg = GeyserConfig::new(format!("http://{addr}"), vec![PROGRAM.to_string()])
        .with_x_token("test-token-123");
    cfg.commitment = Commitment::Confirmed;
    let mut sub = GeyserSubscription::connect(&cfg).await.expect("connect");

    // The filter the server actually received.
    let observed = seen.lock().unwrap().clone().expect("server saw a request");
    assert_eq!(observed.x_token.as_deref(), Some("test-token-123"));
    assert_eq!(
        observed.request.commitment,
        Some(yellowstone_grpc_proto::geyser::CommitmentLevel::Confirmed as i32)
    );
    let filter: &SubscribeRequestFilterTransactions = observed
        .request
        .transactions
        .get("sniper-suite")
        .expect("filter");
    assert_eq!(filter.vote, Some(false));
    assert_eq!(filter.failed, Some(false));
    assert_eq!(filter.account_include, vec![PROGRAM.to_string()]);
    assert!(
        observed.request.accounts.is_empty(),
        "must not subscribe to accounts"
    );

    // Vote dropped, ping skipped, our transaction surfaced.
    let first = sub
        .next_transaction()
        .await
        .expect("no error")
        .expect("one tx");
    assert_eq!(first.slot, 11);
    assert!(!first.is_vote);
    assert_eq!(first.index, 7);
    assert_eq!(first.signature, bs58::encode(TX_SIG).into_string());

    // Server closed the stream.
    assert!(sub.next_transaction().await.expect("clean close").is_none());

    // Keep-alive works on the same client.
    assert_eq!(sub.ping().await.expect("pong"), 42);
}

#[tokio::test]
async fn connect_error_is_reported_not_panicked() {
    // Nothing listens on this port: connect must fail with a transport error.
    let cfg = GeyserConfig::new("http://127.0.0.1:1", vec![]);
    let err = match GeyserSubscription::connect(&cfg).await {
        Ok(_) => panic!("connection to port 1 must fail"),
        Err(e) => e,
    };
    let msg = err.to_string();
    assert!(msg.contains("geyser"), "unexpected error text: {msg}");
}

#[tokio::test]
async fn retry_gives_up_after_configured_attempts() {
    let mut cfg = GeyserConfig::new("http://127.0.0.1:1", vec![]);
    cfg.max_reconnects = 2;
    cfg.connect_timeout = Duration::from_millis(200);
    let started = std::time::Instant::now();
    assert!(GeyserSubscription::connect_with_retry(&cfg).await.is_err());
    // 250ms + 500ms of backoff must have elapsed.
    assert!(started.elapsed() >= Duration::from_millis(700));
}

// Keeps the unused-import lint happy in builds where the helper is not used.
#[allow(dead_code)]
fn _unused(
    _: HashMap<String, SubscribeRequestFilterTransactions>,
    _: subscribe_update::UpdateOneof,
) {
}
