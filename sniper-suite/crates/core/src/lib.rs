//! `bot-core` — shared kernel for every module in the sniper suite.
//!
//! Contains:
//!   * configuration (`config`)
//!   * domain models for positions / trades / PnL (`models`)
//!   * the order management system: lifecycle, idempotency, recovery (`oms`)
//!   * the process-wide shared state + event bus (`state`, `events`)
//!   * the risk engine that every execution path must pass through (`risk`)
//!   * JSONL persistence (`storage`)
//!   * optional PostgreSQL persistence: pool, migrations, repositories (`db`)
//!   * the execution lifecycle state machine + duplicate guard (`execution`)
//!   * integer-safe maths helpers (`maths`)
//!   * observability primitives: metrics registry + health registry (`obs`)
//!   * the global ledger: typed accounting events, double-entry postings,
//!     position aggregation, reconciliation and recovery (`accounting`)
//!   * the global risk engine every module's entry passes first (`global_risk`)
//!   * high availability: worker identity, leases with fencing, durable feed
//!     cursors, the crash-recovery matrix (`ha`)
//!   * the SaaS control plane: tenants, memberships/RBAC, sessions, billing
//!     foundation, provisioning and the one authorization decision
//!     (`tenant`, `membership`, `session`, `billing`, `provisioning`,
//!     `authorization`)
//!
//! The trading/execution logic itself performs no I/O; the only network
//! clients living here are the OPTIONAL persistence backends (`db`, and the
//! Redis coordination store), both of which default to disabled so the core
//! keeps its deterministic, I/O-free test surface.

pub mod accounting;
pub mod audit;
pub mod auth;
pub mod authorization;
pub mod billing;
pub mod config;
pub mod db;
pub mod dedup;
pub mod error;
pub mod events;
pub mod execution;
pub mod global_risk;
pub mod ha;
pub mod lifecycle;
pub mod maths;
pub mod membership;
pub mod models;
pub mod obs;
pub mod oms;
pub mod ownership;
pub mod provisioning;
pub mod reconciliation;
pub mod recovery;
pub mod redis_kv;
pub mod redis_ownership;
pub mod risk;
pub mod session;
pub mod state;
pub mod storage;
pub mod tenant;

pub use config::{AppConfig, Config};
pub use error::{BotError, BotResult};
pub use events::{AppEvent, EventBus};
pub use execution::{ExecutionLedger, ExecutionState, FailureClass};
pub use models::{
    BotModule, Chain, ExecutionMode, ModuleState, ModuleStatus, Position, PositionSide,
    PositionStatus, Trade, TradeSource, Venue,
};
pub use oms::{Order, OrderDraft, OrderManager, OrderStatus};
pub use state::{AppState, Shared};

/// Convenience prelude: `use bot_core::prelude::*;`
pub mod prelude {
    pub use crate::config::{AppConfig, Config};
    pub use crate::error::{BotError, BotResult};
    pub use crate::events::{AppEvent, EventBus};
    pub use crate::execution::{ExecutionLedger, ExecutionState, FailureClass};
    pub use crate::maths;
    pub use crate::models::*;
    pub use crate::obs::{health, metrics};
    pub use crate::oms::{Order, OrderDraft, OrderManager, OrderStatus};
    pub use crate::ownership::{
        ClaimOutcome, ClaimStatus, ClaimStore, ExecutionClaim, MemoryClaimStore, MemoryFlags,
        OwnershipRegistry, Permit, RuntimeFlag, RuntimeFlagsReader, RuntimeFlagsWriter,
    };
    pub use crate::risk::{RiskDecision, RiskEngine, RiskVerdict};
    pub use crate::state::{AppState, Shared};
    pub use crate::storage::Store;
}
