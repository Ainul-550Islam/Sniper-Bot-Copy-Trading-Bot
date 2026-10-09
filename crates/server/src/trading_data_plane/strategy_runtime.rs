//! Strategy runtime (GAP-MAP P1 REWRITE — replaces the orphan 39-line
//! bridge stub).
//!
//! The contract: **activating a strategy means translating its parameters
//! into the tenant's VERSIONED module configuration**, from which the
//! module runtime rebuilds the engine through
//! [`crate::module_runtime::tenant_module_factory`] on its next build.
//! The runtime never patches a live engine in place — a config version
//! bump is the only activation signal, so activation and the engine state
//! can never disagree.
//!
//! ## Safety gates
//!
//! * `Paused` / `Archived` strategies cannot be activated.
//! * `paper` / `simulate` strategies activate FENCED: the engine may be
//!   built, but live broadcast stays off.
//! * `paper → live` is guarded by [`crate::ops::funded_mode_guard`]:
//!   activation with `mode = live` is refused unless the deployment is
//!   explicitly live-funded (allow_live_trading, configured AND funded
//!   wallet, owner authorization, risk gates). The refusal is surfaced,
//!   never silently downgraded.
//! * Strategy parameters are validated by
//!   [`bot_core::strategy::validate_strategy_params`] before anything is
//!   written.

use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};

use bot_core::db::Database;
use bot_core::error::{BotError, BotResult};
use bot_core::models::{BotModule, ExecutionMode};
use bot_core::strategy::{validate_strategy_params, StrategyId, StrategyRecord, StrategyStatus};
use bot_core::tenant::OrganizationId;

use crate::ops::funded_mode_guard::{FundedModeConfig, FundedModeGuard};

use super::config_store;

/// Why an activation/deactivation was refused.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ActivationRefusal {
    /// Strategy must be `active` to be activated.
    StatusNotActive { status: String },
    /// Parameters failed `validate_strategy_params`.
    InvalidParams { reason: String },
    /// `mode = live` but the funded-mode guard denies this deployment.
    LiveNotAllowed { reason: String },
    /// The module has no tenant engine wiring (e.g. Telegram, Contract).
    ModuleNotWired { module: String },
}

impl ActivationRefusal {
    pub fn message(&self) -> String {
        match self {
            ActivationRefusal::StatusNotActive { status } => {
                format!("strategy is {status}, only active strategies can be activated")
            }
            ActivationRefusal::InvalidParams { reason } => {
                format!("strategy parameters are invalid: {reason}")
            }
            ActivationRefusal::LiveNotAllowed { reason } => {
                format!("live activation denied by funded-mode guard: {reason}")
            }
            ActivationRefusal::ModuleNotWired { module } => {
                format!("module {module} has no tenant engine wiring")
            }
        }
    }
}

/// The versioned-config write an activation performs. Pure data — built
/// by [`plan_activation`], executed by [`apply_plan`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ActivationPlan {
    pub strategy_id: StrategyId,
    pub organization_id: OrganizationId,
    pub module: BotModule,
    pub mode: ExecutionMode,
    /// `true` = the engine may be built but must NOT broadcast live
    /// (paper / simulate). `false` only for funded-live activations.
    pub fenced: bool,
    /// The module-config key the patch targets (`sniper`, `copy`,
    /// `polymarket`).
    pub module_key: &'static str,
    /// The patch merged into the tenant's versioned configuration.
    pub config_patch: Map<String, Value>,
}

/// Deactivation: fence the module config for this strategy and mark the
/// runtime generation as retired. Same versioned-config mechanism.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DeactivationPlan {
    pub strategy_id: StrategyId,
    pub organization_id: OrganizationId,
    pub module_key: &'static str,
    pub config_patch: Map<String, Value>,
}

/// Which modules have tenant engine wiring (mirrors
/// `TenantModuleFactory::build_from_instance`).
fn module_key(module: BotModule) -> Option<&'static str> {
    match module {
        BotModule::Sniper => Some("sniper"),
        BotModule::Copy => Some("copy"),
        BotModule::Polymarket => Some("polymarket"),
        BotModule::Telegram | BotModule::Contract => None,
    }
}

/// Derive the funded-mode picture of THIS deployment from its config.
///
/// The funded flag itself cannot be proven from configuration — it is an
/// OPERATOR ATTESTATION (`EXECUTION_WALLET_FUNDED=1`). Unattested means
/// `wallet_funded = false`, so live activation is denied: the guard fails
/// safe by construction.
pub fn funded_mode_from_deployment(cfg: &bot_core::config::Config) -> FundedModeConfig {
    let execution_mode = match cfg.execution.mode {
        ExecutionMode::Live => "live",
        ExecutionMode::Simulate => "simulate",
        ExecutionMode::Paper => "paper",
    };
    FundedModeConfig {
        execution_mode: execution_mode.into(),
        allow_live_trading: cfg.execution.allow_live_trading,
        // Server mode always loads a wallet (`load_wallet`); the operator
        // trust boundary is documented in solana-kit signer.rs.
        wallet_configured: true,
        wallet_funded: std::env::var("EXECUTION_WALLET_FUNDED")
            .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
            .unwrap_or(false),
        owner_authorized: cfg.execution.allow_live_trading,
        // The kill switch or any module emergency stop counts as the
        // deployment's risk gates failing.
        risk_gates_pass: !cfg.risk.kill_switch
            && !cfg.risk.sniper_emergency_disable
            && !cfg.risk.copy_emergency_disable
            && !cfg.risk.poly_emergency_disable,
    }
}

/// Pure activation decision. `funded` describes THIS deployment; the
/// guard refuses `live` strategies unless it is explicitly live-funded.
pub fn plan_activation(
    strategy: &StrategyRecord,
    funded: &FundedModeConfig,
) -> Result<ActivationPlan, ActivationRefusal> {
    if strategy.status != StrategyStatus::Active {
        return Err(ActivationRefusal::StatusNotActive {
            status: strategy.status.as_str().to_string(),
        });
    }
    let module_key = module_key(strategy.module).ok_or_else(|| {
        ActivationRefusal::ModuleNotWired {
            module: format!("{:?}", strategy.module),
        }
    })?;
    validate_strategy_params(strategy.module, &strategy.config_json).map_err(|e| {
        ActivationRefusal::InvalidParams {
            reason: e.to_string(),
        }
    })?;

    let fenced = !strategy.mode.is_live();
    if strategy.mode.is_live() {
        // paper -> live gate: the deployment itself must be live-funded.
        match FundedModeGuard::check(funded) {
            Ok(mode) if mode.is_funded() => {}
            Ok(mode) => {
                return Err(ActivationRefusal::LiveNotAllowed {
                    reason: format!(
                        "deployment mode is {} — live activation requires live_funded",
                        mode.as_str()
                    ),
                })
            }
            Err(reason) => {
                return Err(ActivationRefusal::LiveNotAllowed { reason })
            }
        }
    }

    let mut config_patch = Map::new();
    config_patch.insert(
        "strategy".into(),
        json!({
            "id": strategy.id.to_string(),
            "strategy_version": strategy.version,
            "mode": match strategy.mode {
                ExecutionMode::Paper => "paper",
                ExecutionMode::Simulate => "simulate",
                ExecutionMode::Live => "live",
            },
            "fenced": fenced,
            "params": strategy.config_json,
        }),
    );

    Ok(ActivationPlan {
        strategy_id: strategy.id,
        organization_id: strategy.organization_id,
        module: strategy.module,
        mode: strategy.mode,
        fenced,
        module_key,
        config_patch,
    })
}

/// Pure deactivation decision: removes the strategy binding from the
/// module config (the runtime reads `strategy == null` as "no engine").
pub fn plan_deactivation(strategy: &StrategyRecord) -> Result<DeactivationPlan, ActivationRefusal> {
    let module_key = module_key(strategy.module).ok_or_else(|| {
        ActivationRefusal::ModuleNotWired {
            module: format!("{:?}", strategy.module),
        }
    })?;
    let mut config_patch = Map::new();
    config_patch.insert(
        "strategy".into(),
        json!({
            "id": strategy.id.to_string(),
            "fenced": true,
            "deactivated": true,
        }),
    );
    Ok(DeactivationPlan {
        strategy_id: strategy.id,
        organization_id: strategy.organization_id,
        module_key,
        config_patch,
    })
}

/// Execute an activation plan: one versioned-config write. The returned
/// value is the new module configuration (with its version) — the module
/// runtime rebuilds from it through the tenant module factory.
pub async fn apply_activation(
    db: &Database,
    plan: &ActivationPlan,
    actor: &str,
) -> BotResult<Value> {
    config_store::write_module(db, plan.organization_id, plan.module_key, &plan.config_patch, actor)
        .await
        .map_err(|e| BotError::db(format!("strategy activation config write: {e}")))
}

/// Execute a deactivation plan (same versioned mechanism).
pub async fn apply_deactivation(
    db: &Database,
    plan: &DeactivationPlan,
    actor: &str,
) -> BotResult<Value> {
    config_store::write_module(db, plan.organization_id, plan.module_key, &plan.config_patch, actor)
        .await
        .map_err(|e| BotError::db(format!("strategy deactivation config write: {e}")))
}

/// The active runtime bridge for an applied activation. Kept for the
/// surfaces that report runtime state; constructed from the PLAN, not
/// re-derived, so the reported state can never drift from what was
/// written.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StrategyRuntimeBridge {
    pub strategy_id: StrategyId,
    pub organization_id: OrganizationId,
    pub module_key: String,
    pub mode: String,
    pub fenced: bool,
    pub active_since: chrono::DateTime<chrono::Utc>,
}

impl StrategyRuntimeBridge {
    pub fn from_plan(plan: &ActivationPlan, now: chrono::DateTime<chrono::Utc>) -> Self {
        StrategyRuntimeBridge {
            strategy_id: plan.strategy_id,
            organization_id: plan.organization_id,
            module_key: plan.module_key.to_string(),
            mode: match plan.mode {
                ExecutionMode::Paper => "paper",
                ExecutionMode::Simulate => "simulate",
                ExecutionMode::Live => "live",
            }
            .to_string(),
            fenced: plan.fenced,
            active_since: now,
        }
    }

    pub fn to_json(&self) -> Value {
        json!({
            "strategy_id": self.strategy_id.to_string(),
            "organization_id": self.organization_id.to_string(),
            "module": self.module_key,
            "mode": self.mode,
            "fenced": self.fenced,
            "active_since": self.active_since.to_rfc3339(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    fn funded_config(live_funded: bool) -> FundedModeConfig {
        FundedModeConfig {
            execution_mode: if live_funded { "live" } else { "paper" }.into(),
            allow_live_trading: live_funded,
            wallet_configured: live_funded,
            wallet_funded: live_funded,
            owner_authorized: live_funded,
            risk_gates_pass: live_funded,
        }
    }

    fn valid_sniper_params() -> Value {
        // The full strongly-typed shape validate_strategy_params expects.
        json!({
            "min_liquidity_lamports": 5_000_000_000u64,
            "max_slippage_bps": 500u32,
            "anti_mev_protection": true,
            "priority_fee_lamports": 500_000u64,
            "entry_amount_lamports": 10_000_000u64,
            "take_profit_pct": 100u32,
            "stop_loss_pct": 20u32,
            "trailing_stop_pct": 10u32,
            "auto_sell_timeout_seconds": 300u32,
            "dry_run": false
        })
    }

    fn sniper_strategy(mode: ExecutionMode, status: StrategyStatus) -> StrategyRecord {
        let mut record = StrategyRecord::new(
            OrganizationId::new(),
            "momentum".into(),
            "test strategy".into(),
            BotModule::Sniper,
            mode,
            valid_sniper_params(),
            Utc::now(),
        );
        record.status = status;
        record
    }

    #[test]
    fn paper_strategy_activates_fenced_on_any_deployment() {
        let strategy = sniper_strategy(ExecutionMode::Paper, StrategyStatus::Active);
        let plan = plan_activation(&strategy, &funded_config(false)).expect("paper activates");
        assert!(plan.fenced);
        assert_eq!(plan.module_key, "sniper");
        assert_eq!(plan.config_patch["strategy"]["fenced"], true);
        assert_eq!(plan.config_patch["strategy"]["mode"], "paper");
        assert_eq!(
            plan.config_patch["strategy"]["params"],
            strategy.config_json
        );
    }

    #[test]
    fn live_strategy_requires_a_live_funded_deployment() {
        let strategy = sniper_strategy(ExecutionMode::Live, StrategyStatus::Active);
        let refused = plan_activation(&strategy, &funded_config(false)).unwrap_err();
        assert!(matches!(refused, ActivationRefusal::LiveNotAllowed { .. }));
        assert!(refused.message().contains("funded-mode guard"));

        let plan = plan_activation(&strategy, &funded_config(true)).expect("live funded passes");
        assert!(!plan.fenced, "funded live activation is unfenced");
        assert_eq!(plan.config_patch["strategy"]["mode"], "live");
    }

    #[test]
    fn unfenced_live_without_full_authorization_is_denied() {
        let strategy = sniper_strategy(ExecutionMode::Live, StrategyStatus::Active);
        // Live deployment but the owner did not authorize.
        let mut cfg = funded_config(true);
        cfg.owner_authorized = false;
        let refused = plan_activation(&strategy, &cfg).unwrap_err();
        assert!(matches!(refused, ActivationRefusal::LiveNotAllowed { .. }));
    }

    #[test]
    fn paused_and_archived_strategies_cannot_activate() {
        let paused = sniper_strategy(ExecutionMode::Paper, StrategyStatus::Paused);
        assert!(matches!(
            plan_activation(&paused, &funded_config(false)),
            Err(ActivationRefusal::StatusNotActive { .. })
        ));
        let archived = sniper_strategy(ExecutionMode::Paper, StrategyStatus::Archived);
        assert!(matches!(
            plan_activation(&archived, &funded_config(false)),
            Err(ActivationRefusal::StatusNotActive { .. })
        ));
    }

    #[test]
    fn invalid_params_are_refused_before_anything_is_written() {
        let mut strategy = sniper_strategy(ExecutionMode::Paper, StrategyStatus::Active);
        // Well-formed JSON but a zero entry amount: validation refuses it.
        let mut params = valid_sniper_params();
        params["entry_amount_lamports"] = json!(0u64);
        strategy.config_json = params;
        let refused = plan_activation(&strategy, &funded_config(false)).unwrap_err();
        assert!(matches!(refused, ActivationRefusal::InvalidParams { .. }));

        // And a structurally broken config is refused just the same.
        strategy.config_json = json!({"nonsense": true});
        assert!(matches!(
            plan_activation(&strategy, &funded_config(false)),
            Err(ActivationRefusal::InvalidParams { .. })
        ));
    }

    #[test]
    fn unwired_modules_are_refused() {
        let mut strategy = sniper_strategy(ExecutionMode::Paper, StrategyStatus::Active);
        strategy.module = BotModule::Telegram;
        let refused = plan_activation(&strategy, &funded_config(false)).unwrap_err();
        assert!(matches!(refused, ActivationRefusal::ModuleNotWired { .. }));
    }

    #[test]
    fn deactivation_fences_the_module_config() {
        let strategy = sniper_strategy(ExecutionMode::Live, StrategyStatus::Active);
        let plan = plan_deactivation(&strategy).expect("deactivation plans");
        assert_eq!(plan.module_key, "sniper");
        assert_eq!(plan.config_patch["strategy"]["deactivated"], true);
        assert_eq!(plan.config_patch["strategy"]["fenced"], true);
    }

    #[test]
    fn bridge_reports_the_plan_exactly() {
        let strategy = sniper_strategy(ExecutionMode::Paper, StrategyStatus::Active);
        let plan = plan_activation(&strategy, &funded_config(false)).unwrap();
        let now = Utc::now();
        let bridge = StrategyRuntimeBridge::from_plan(&plan, now);
        assert_eq!(bridge.strategy_id, strategy.id);
        assert!(bridge.fenced);
        assert_eq!(bridge.mode, "paper");
        let json = bridge.to_json();
        assert_eq!(json["module"], "sniper");
        assert_eq!(json["fenced"], true);
    }
}
