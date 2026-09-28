//! Global risk engine (TASK 5 §1).
//!
//! ```text
//! signal -> venue / strategy
//!        -> GlobalRiskEngine::decide      ONE portfolio-level decision (this module)
//!        -> RiskEngine module checks      TASK 1–4 limits, unchanged
//!        -> OMS / execution -> fill -> GlobalLedger -> book -> reconciliation
//! ```
//!
//! | file | concern |
//! |---|---|
//! | `decision.rs` | [`GlobalRiskRequest`], [`GlobalRejectReason`] (14 reasons), [`GlobalRiskDecision`] + snapshot |
//! | `engine.rs` | [`GlobalRiskEngine`]: the ordered checks over the ledger's book |
//! | `kill_switch.rs` | [`KillSwitches`] per venue / strategy, config-pinned or operator-engaged |
//! | `store.rs` | [`RiskStore`] (decision journal, switch state / events) + [`MemoryRiskStore`] |
//! | `metrics.rs` / `audit.rs` | `global_risk_*` series, `global.risk.*` / `global.kill_switch.*` audit actions |
//!
//! Configuration lives in `[global_risk]` ([`crate::config::GlobalRiskConfig`]).

pub mod audit;
pub mod decision;
pub mod engine;
pub mod kill_switch;
pub mod metrics;
pub mod store;

pub use audit::AUDIT_ACTOR;
pub use decision::{
    DecisionSnapshot, GlobalRejectReason, GlobalRiskDecision, GlobalRiskRequest, GlobalVerdict,
};
pub use engine::{quote_asset_for, strategy_label, DecisionContext, GlobalRiskEngine};
pub use kill_switch::{KillScope, KillSwitchEvent, KillSwitchState, KillSwitches, SwitchOutcome};
pub use store::{MemoryRiskStore, RiskStore};
