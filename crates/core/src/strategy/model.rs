//! Strategy domain models and validation rules (SECOND.md §93).
//!
//! Strongly typed strategy representations: identity, versioning, status,
//! configuration per trading module, and tenant isolation primitives.
//! All monetary values use exact integer atomic units (lamports, basis points).

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::fmt;
use uuid::Uuid;

use crate::models::{BotModule, ExecutionMode};
use crate::tenant::OrganizationId;

/// Unique identifier for a trading strategy template.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct StrategyId(Uuid);

impl StrategyId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }

    pub fn from_uuid(u: Uuid) -> Self {
        Self(u)
    }

    pub fn parse(s: &str) -> Option<Self> {
        Uuid::parse_str(s).ok().map(Self)
    }

    pub fn as_uuid(&self) -> &Uuid {
        &self.0
    }
}

impl Default for StrategyId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for StrategyId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Lifecycle status of a strategy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StrategyStatus {
    Active,
    Paused,
    Archived,
}

impl StrategyStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Paused => "paused",
            Self::Archived => "archived",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "active" => Some(Self::Active),
            "paused" => Some(Self::Paused),
            "archived" => Some(Self::Archived),
            _ => None,
        }
    }
}

/// Canonical strategy record.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StrategyRecord {
    pub id: StrategyId,
    pub organization_id: OrganizationId,
    pub name: String,
    pub description: String,
    pub module: BotModule,
    pub mode: ExecutionMode,
    pub status: StrategyStatus,
    pub version: u32,
    pub config_json: serde_json::Value,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl StrategyRecord {
    pub fn new(
        organization_id: OrganizationId,
        name: String,
        description: String,
        module: BotModule,
        mode: ExecutionMode,
        config_json: serde_json::Value,
        now: DateTime<Utc>,
    ) -> Self {
        Self {
            id: StrategyId::new(),
            organization_id,
            name,
            description,
            module,
            mode,
            status: StrategyStatus::Active,
            version: 1,
            config_json,
            created_at: now,
            updated_at: now,
        }
    }

    /// Increment version on modification.
    pub fn bump_version(&mut self, new_config: serde_json::Value, now: DateTime<Utc>) {
        self.version += 1;
        self.config_json = new_config;
        self.updated_at = now;
    }
}

/// Strongly typed Sniper parameters.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SniperStrategyParams {
    pub min_liquidity_lamports: u64,
    pub max_slippage_bps: u32,
    pub anti_mev_protection: bool,
    pub priority_fee_lamports: u64,
    pub entry_amount_lamports: u64,
    pub take_profit_pct: u32,
    pub stop_loss_pct: u32,
    pub trailing_stop_pct: u32,
    pub auto_sell_timeout_seconds: u32,
    pub dry_run: bool,
}

impl Default for SniperStrategyParams {
    fn default() -> Self {
        Self {
            min_liquidity_lamports: 5_000_000_000, // 5 SOL
            max_slippage_bps: 150,                 // 1.5%
            anti_mev_protection: true,
            priority_fee_lamports: 500_000,
            entry_amount_lamports: 500_000_000,    // 0.5 SOL
            take_profit_pct: 100,
            stop_loss_pct: 20,
            trailing_stop_pct: 10,
            auto_sell_timeout_seconds: 300,
            dry_run: true,
        }
    }
}

/// Strongly typed Copy Trading parameters.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CopyStrategyParams {
    pub max_exposure_usd_cents: u64,
    pub allocation_per_trade_lamports: u64,
    pub max_slippage_bps: u32,
    pub mirror_buys: bool,
    pub mirror_sells: bool,
    pub stale_event_timeout_seconds: u32,
    pub copy_ratio_pct: u32,
    pub dry_run: bool,
}

impl Default for CopyStrategyParams {
    fn default() -> Self {
        Self {
            max_exposure_usd_cents: 100_000,              // $1,000.00
            allocation_per_trade_lamports: 250_000_000,  // 0.25 SOL
            max_slippage_bps: 100,
            mirror_buys: true,
            mirror_sells: true,
            stale_event_timeout_seconds: 15,
            copy_ratio_pct: 100,
            dry_run: true,
        }
    }
}

/// Strongly typed Polymarket CLOB parameters.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PolymarketStrategyParams {
    pub active_condition_ids: Vec<String>,
    pub max_position_size_usdc_units: u64,
    pub max_market_exposure_usdc_units: u64,
    pub spread_threshold_bps: u32,
    pub reprice_interval_seconds: u32,
    pub cancel_stale_orders: bool,
    pub dry_run: bool,
    pub order_type: String,
}

impl Default for PolymarketStrategyParams {
    fn default() -> Self {
        Self {
            active_condition_ids: Vec::new(),
            max_position_size_usdc_units: 500_000_000,     // 500 USDC (6 decimals)
            max_market_exposure_usdc_units: 2_500_000_000, // 2500 USDC
            spread_threshold_bps: 50,
            reprice_interval_seconds: 5,
            cancel_stale_orders: true,
            dry_run: true,
            order_type: "limit".into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strategy_id_round_trip() {
        let id = StrategyId::new();
        let s = id.to_string();
        let parsed = StrategyId::parse(&s).expect("must parse uuid");
        assert_eq!(id, parsed);
    }

    #[test]
    fn version_bumping() {
        let org = OrganizationId::new();
        let now = Utc::now();
        let mut st = StrategyRecord::new(
            org,
            "Alpha Sniper".into(),
            "Fast launches".into(),
            BotModule::Sniper,
            ExecutionMode::Paper,
            serde_json::json!({"entry_sol": 1.0}),
            now,
        );
        assert_eq!(st.version, 1);
        st.bump_version(serde_json::json!({"entry_sol": 2.0}), now);
        assert_eq!(st.version, 2);
    }
}
