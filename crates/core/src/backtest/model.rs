//! Backtest domain model and deterministic run state (SECOND.md §95).
//!
//! Strongly typed backtesting definitions: job identity, dataset provenance,
//! financial assumptions (integer BPS fees/slippage), and exact result metrics.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::fmt;
use uuid::Uuid;

use crate::strategy::StrategyId;
use crate::tenant::OrganizationId;

/// Unique identifier for a backtest execution run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct BacktestRunId(Uuid);

impl BacktestRunId {
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

impl Default for BacktestRunId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for BacktestRunId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Lifecycle status of a backtest run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BacktestStatus {
    Queued,
    Running,
    Completed,
    Failed,
}

impl BacktestStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Running => "running",
            Self::Completed => "completed",
            Self::Failed => "failed",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "queued" => Some(Self::Queued),
            "running" => Some(Self::Running),
            "completed" => Some(Self::Completed),
            "failed" => Some(Self::Failed),
            _ => None,
        }
    }
}

/// Configuration parameters for a backtest simulation run.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BacktestConfig {
    pub strategy_id: StrategyId,
    pub period_start: DateTime<Utc>,
    pub period_end: DateTime<Utc>,
    pub venue: String,
    pub initial_balance_usd_cents: u64,
    pub fee_rate_bps: u32,
    pub slippage_bps: u32,
}

/// Exact financial result of a completed backtest.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BacktestResult {
    pub final_balance_usd_cents: u64,
    pub net_pnl_usd_cents: i64,
    pub net_roi_bps: i32,
    pub max_drawdown_bps: u32,
    pub total_trades: u32,
    pub winning_trades: u32,
    pub win_rate_bps: u32,
    pub sharpe_ratio_scaled: i32, // Scaled by 100 (e.g. 234 = 2.34)
}

/// Canonical backtest record with provenance.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BacktestRecord {
    pub id: BacktestRunId,
    pub organization_id: OrganizationId,
    pub config: BacktestConfig,
    pub status: BacktestStatus,
    pub result: Option<BacktestResult>,
    pub error: Option<String>,
    pub created_at: DateTime<Utc>,
    pub completed_at: Option<DateTime<Utc>>,
}

impl BacktestRecord {
    pub fn new(
        organization_id: OrganizationId,
        config: BacktestConfig,
        now: DateTime<Utc>,
    ) -> Self {
        Self {
            id: BacktestRunId::new(),
            organization_id,
            config,
            status: BacktestStatus::Queued,
            result: None,
            error: None,
            created_at: now,
            completed_at: None,
        }
    }

    pub fn complete(&mut self, result: BacktestResult, now: DateTime<Utc>) {
        self.status = BacktestStatus::Completed;
        self.result = Some(result);
        self.completed_at = Some(now);
    }

    pub fn fail(&mut self, error: String, now: DateTime<Utc>) {
        self.status = BacktestStatus::Failed;
        self.error = Some(error);
        self.completed_at = Some(now);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backtest_id_parsing() {
        let id = BacktestRunId::new();
        assert_eq!(BacktestRunId::parse(&id.to_string()), Some(id));
    }
}
