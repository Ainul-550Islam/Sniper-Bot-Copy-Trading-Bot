//! `solana-kit` — the Solana half of the suite.
//!
//! Contains everything that touches the chain:
//!   * [`consts`]   — program ids, PDA seeds and instruction discriminators
//!   * [`rpc`]      — thin async RPC wrapper with retries and fallbacks
//!   * [`ws`]       — reconnecting websocket client (`logsSubscribe`, …)
//!   * [`tokens`]   — keypair loading, balances, ATA management
//!   * [`pump`]     — Pump.fun bonding curve accounts + buy/sell instructions
//!   * [`pumpswap`] — PumpSwap (graduated) AMM buy/sell
//!   * [`raydium`]  — Raydium AMM v4 pool parsing + `swapBaseIn`
//!   * [`jupiter`]  — aggregator fallback
//!   * [`tx`]       — v0 transaction assembly, priority fees, Jito tips
//!   * [`execute`]  — simulate / broadcast / confirm
//!   * [`decode`]   — parse confirmed transactions back into `WalletTrade`s
//!   * [`pumpportal`] — PumpPortal websocket feed (fastest launch detection)
//!   * [`layout`]   — self-healing account-layout template store
//!
//! IMPORTANT: Pump.fun has changed its required account list several times
//! without a migration window (volume accumulators, fee config, and most
//! recently `bonding-curve-v2`). [`layout`] exists so a failing build can be
//! repaired from a *known-good* transaction instead of by editing code.

pub mod cache;
pub mod consts;
pub mod decode;
pub mod events;
pub mod execute;
pub mod jupiter;
pub mod layout;
pub mod pump;
pub mod pumpportal;
pub mod pumpswap;
pub mod raydium;
pub mod rpc;
pub mod signer;
pub mod tokens;
pub mod tx;
pub mod ws;

pub use cache::AccountCache;
pub use consts::*;
pub use decode::{DecodedSwap, Side, SwapVenue};
pub use events::{find_graduation, find_launch, parse_logs, PumpEvent};
pub use execute::{ExecutionResult, Executor};
pub use layout::{AccountLayout, LayoutStore};
pub use pump::{BondingCurveState, GlobalState, PumpContext};
pub use rpc::Rpc;
pub use signer::{
    build_signer_registry, LocalKeypairSigner, SignerRegistry, TransactionSigner,
    COPY_TRADING_IDENTITY, PRIMARY_SIGNER_IDENTITY, SNIPER_IDENTITY, STAKING_ADMIN_IDENTITY,
    TREASURY_IDENTITY,
};
pub use tokens::Wallet;
pub use tx::{TxBuilder, TxRequest};
pub use ws::{SolanaWs, WsMessage};
