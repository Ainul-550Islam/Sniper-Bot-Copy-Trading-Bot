//! Typed Risk Limits & Kill-Switch SDK Client (THIRD.md §147).

use serde::{Deserialize, Serialize};

use crate::client::Client;
use crate::error::Result;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RiskLimitRule {
    pub id: String,
    pub name: String,
    pub scope: String,
    pub limit_usd_cents: u64,
    pub current_utilization_cents: u64,
    pub utilization_pct: f64,
    pub status: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RiskDashboardState {
    pub organization_id: String,
    pub kill_switch_active: bool,
    pub max_drawdown_limit_bps: u32,
    pub current_drawdown_bps: u32,
    pub daily_loss_limit_usd_cents: u64,
    pub current_daily_loss_cents: u64,
    pub rules: Vec<RiskLimitRule>,
    pub as_of: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToggleKillSwitchResponse {
    pub success: bool,
    pub kill_switch_active: bool,
    pub organization_id: String,
    pub updated_at: String,
}

/// Risk limits and emergency kill switch client.
#[derive(Debug, Clone)]
pub struct RiskClient {
    client: Client,
}

impl RiskClient {
    pub fn new(client: Client) -> Self {
        Self { client }
    }

    /// Fetches risk dashboard and limit rules.
    pub async fn get_dashboard(&self) -> Result<RiskDashboardState> {
        self.client.get("/api/saas/risk-dashboard").await
    }

    /// Toggles organization-level emergency kill switch.
    pub async fn toggle_kill_switch(
        &self,
        active: bool,
        reason: &str,
    ) -> Result<ToggleKillSwitchResponse> {
        self.client
            .post(
                "/api/saas/risk-dashboard/kill-switch",
                &serde_json::json!({
                    "active": active,
                    "reason": reason
                }),
            )
            .await
    }
}
