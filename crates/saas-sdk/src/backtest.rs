//! Buyer-facing typed SDK for backtest APIs (SECOND.md §98).

use serde::{Deserialize, Serialize};

use crate::client::SaasClient;
use crate::error::BotResult;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BacktestDto {
    pub id: String,
    pub organization_id: String,
    pub strategy_id: String,
    pub venue: String,
    pub period_start: String,
    pub period_end: String,
    pub initial_balance_usd_cents: u64,
    pub fee_rate_bps: u32,
    pub slippage_bps: u32,
    pub status: String,
    pub result: Option<serde_json::Value>,
    pub error: Option<String>,
    pub created_at: String,
    pub completed_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateBacktestRequest {
    pub strategy_id: String,
    pub venue: String,
    pub period_start: String,
    pub period_end: String,
    pub initial_balance_usd_cents: u64,
    pub fee_rate_bps: u32,
    pub slippage_bps: u32,
}

pub struct BacktestClient<'a> {
    pub(crate) client: &'a SaasClient,
}

impl<'a> BacktestClient<'a> {
    pub fn new(client: &'a SaasClient) -> Self {
        Self { client }
    }

    /// List backtests for the authenticated tenant.
    pub async fn list(&self) -> BotResult<Vec<BacktestDto>> {
        let res: serde_json::Value = self.client.get("/api/tenant/backtests").await?;
        let items = res
            .get("items")
            .cloned()
            .unwrap_or_else(|| serde_json::json!([]));
        serde_json::from_value(items)
            .map_err(|e| crate::error::BotError::invalid_input(e.to_string()))
    }

    /// Get backtest detail.
    pub async fn get(&self, id: &str) -> BotResult<BacktestDto> {
        self.client
            .get(&format!("/api/tenant/backtests/{}", id))
            .await
    }

    /// Queue a new backtest simulation.
    pub async fn create(&self, req: &CreateBacktestRequest) -> BotResult<BacktestDto> {
        self.client.post("/api/tenant/backtests", req).await
    }
}
