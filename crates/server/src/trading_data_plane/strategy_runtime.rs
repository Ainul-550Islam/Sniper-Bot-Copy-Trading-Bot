//! Strategy runtime bridge linking strategy catalog to autonomous execution engine (THIRD.md §142).

use chrono::Utc;
use serde_json::json;

use bot_core::strategy::{StrategyId, StrategyRecord, StrategyStatus};
use bot_core::tenant::OrganizationId;

/// Represents the active runtime bridge for an activated tenant strategy.
#[derive(Debug, Clone)]
pub struct StrategyRuntimeBridge {
    pub strategy_id: StrategyId,
    pub organization_id: OrganizationId,
    pub generation: u64,
    pub is_fenced: bool,
    pub active_since: chrono::DateTime<Utc>,
}

impl StrategyRuntimeBridge {
    pub fn new(strategy: &StrategyRecord) -> Self {
        Self {
            strategy_id: strategy.id,
            organization_id: strategy.organization_id,
            generation: 1,
            is_fenced: strategy.status != StrategyStatus::Active,
            active_since: Utc::now(),
        }
    }

    pub fn to_json(&self) -> serde_json::Value {
        json!({
            "strategy_id": self.strategy_id.to_string(),
            "organization_id": self.organization_id.to_string(),
            "generation": self.generation,
            "is_fenced": self.is_fenced,
            "active_since": self.active_since.to_rfc3339()
        })
    }
}
