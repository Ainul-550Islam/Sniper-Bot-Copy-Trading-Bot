//! A minimal Yellowstone Geyser gRPC server used to prove the bot's ingest path.
//!
//! It is NOT a sandbox validator: it serves a synthetic stream so the wiring in
//! `sniper_suite::solana::geyser_ingest` can be exercised end-to-end without a
//! paid provider endpoint or mainnet credentials.
//!
//!   cargo run -p solana-kit --example geyser_mock_server -- 127.0.0.1:50077
//!
//! It emits one non-vote transaction every 250 ms with an incrementing slot and
//! answers `Ping` with a pong, which is enough for `GET /api/status`-style
//! counters and for the shutdown phase to report a non-zero transaction count.

use std::collections::HashMap;
use std::net::SocketAddr;

use tokio::sync::mpsc;
use tokio_stream::wrappers::{ReceiverStream, TcpListenerStream};
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
    SubscribeUpdate, SubscribeUpdateDeshred, SubscribeUpdateGossip, SubscribeUpdateTransaction,
    SubscribeUpdateTransactionInfo,
};

#[derive(Default)]
struct MockGeyser;

#[tonic::async_trait]
impl Geyser for MockGeyser {
    type SubscribeStream = ReceiverStream<Result<SubscribeUpdate, Status>>;

    async fn subscribe(
        &self,
        request: Request<tonic::Streaming<SubscribeRequest>>,
    ) -> Result<Response<Self::SubscribeStream>, Status> {
        let mut stream = request.into_inner();
        let first: SubscribeRequest = stream
            .message()
            .await?
            .ok_or_else(|| Status::invalid_argument("empty subscribe stream"))?;
        let filter: &SubscribeRequestFilterTransactions = first
            .transactions
            .get("sniper-suite")
            .ok_or_else(|| Status::invalid_argument("missing sniper-suite transaction filter"))?;
        eprintln!(
            "[mock-geyser] subscribe: commitment={:?} vote={:?} failed={:?} account_include={:?}",
            first.commitment, filter.vote, filter.failed, filter.account_include
        );
        let _ = HashMap::<String, String>::new(); // keep the import honest

        let (tx, rx) = mpsc::channel(64);
        tokio::spawn(async move {
            let mut slot: u64 = 1_000;
            loop {
                slot += 1;
                let mut signature = vec![0u8; 64];
                signature[..8].copy_from_slice(&slot.to_le_bytes());
                let update = SubscribeUpdate {
                    filters: vec!["sniper-suite".to_string()],
                    update_oneof: Some(UpdateOneof::Transaction(SubscribeUpdateTransaction {
                        slot,
                        bank_id: 0,
                        transaction: Some(SubscribeUpdateTransactionInfo {
                            signature,
                            is_vote: false,
                            index: slot % 64,
                            ..Default::default()
                        }),
                    })),
                    created_at: None,
                };
                if tx.send(Ok(update)).await.is_err() {
                    return; // client went away
                }
                tokio::time::sleep(std::time::Duration::from_millis(250)).await;
            }
        });
        Ok(Response::new(ReceiverStream::new(rx)))
    }

    async fn ping(&self, _r: Request<PingRequest>) -> Result<Response<PongResponse>, Status> {
        Ok(Response::new(PongResponse { count: 1 }))
    }

    type SubscribeDeshredStream = tokio_stream::Empty<Result<SubscribeUpdateDeshred, Status>>;
    async fn subscribe_deshred(
        &self,
        _r: Request<tonic::Streaming<SubscribeDeshredRequest>>,
    ) -> Result<Response<Self::SubscribeDeshredStream>, Status> {
        Err(Status::unimplemented("mock"))
    }

    type SubscribeGossipStream = tokio_stream::Empty<Result<SubscribeUpdateGossip, Status>>;
    async fn subscribe_gossip(
        &self,
        _r: Request<SubscribeGossipRequest>,
    ) -> Result<Response<Self::SubscribeGossipStream>, Status> {
        Err(Status::unimplemented("mock"))
    }

    async fn subscribe_replay_info(
        &self,
        _r: Request<SubscribeReplayInfoRequest>,
    ) -> Result<Response<SubscribeReplayInfoResponse>, Status> {
        Err(Status::unimplemented("mock"))
    }

    async fn get_latest_blockhash(
        &self,
        _r: Request<GetLatestBlockhashRequest>,
    ) -> Result<Response<GetLatestBlockhashResponse>, Status> {
        Err(Status::unimplemented("mock"))
    }

    async fn get_block_height(
        &self,
        _r: Request<GetBlockHeightRequest>,
    ) -> Result<Response<GetBlockHeightResponse>, Status> {
        Err(Status::unimplemented("mock"))
    }

    async fn get_slot(
        &self,
        _r: Request<GetSlotRequest>,
    ) -> Result<Response<GetSlotResponse>, Status> {
        Err(Status::unimplemented("mock"))
    }

    async fn is_blockhash_valid(
        &self,
        _r: Request<IsBlockhashValidRequest>,
    ) -> Result<Response<IsBlockhashValidResponse>, Status> {
        Err(Status::unimplemented("mock"))
    }

    async fn get_version(
        &self,
        _r: Request<GetVersionRequest>,
    ) -> Result<Response<GetVersionResponse>, Status> {
        Err(Status::unimplemented("mock"))
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let addr: SocketAddr = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "127.0.0.1:50077".to_string())
        .parse()?;
    let listener = tokio::net::TcpListener::bind(addr).await?;
    eprintln!("[mock-geyser] listening on {addr}");
    Server::builder()
        .add_service(GeyserServer::new(MockGeyser))
        .serve_with_incoming(TcpListenerStream::new(listener))
        .await?;
    Ok(())
}

// Silence "unused" for the helper import in builds where it is not used.
#[allow(dead_code)]
fn _keep(_: subscribe_update::UpdateOneof) {}
