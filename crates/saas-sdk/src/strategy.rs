//! Buyer-facing typed SDK for strategy APIs (SECOND.md §97).

use serde::{Deserialize, Serialize};

use crate::client::SaasClient;
use crate::error::BotResult;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StrategyDto {
    pub id: String,
    pub organization_id: String,
    pub name: String,
    pub description: String,
    pub module: String,
    pub mode: String,
    pub status: String,
    pub version: u32,
    pub config_json: serde_json::Value,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateStrategyRequest {
    pub name: String,
    pub description: String,
    pub module: String,
    pub mode: String,
    pub config_json: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateStrategyRequest {
    pub name: Option<String>,
    pub description: Option<String>,
    pub status: Option<String>,
    pub config_json: Option<serde_json::Value>,
}

pub struct StrategyClient<'a> {
    pub(crate) client: &'a SaasClient,
}

impl<'a> StrategyClient<'a> {
    pub fn new(client: &'a SaasClient) -> Self {
        Self { client }
    }

    /// List all strategies for the authenticated tenant.
    pub async fn list(&self) -> BotResult<Vec<StrategyDto>> {
        let res: serde_json::Value = self.client.get("/api/tenant/strategies").await?;
        let items = res
            .get("items")
            .cloned()
            .unwrap_or_else(|| serde_json::json!([]));
        serde_json::from_value(items).map_err(|e| crate::error::BotError::invalid_input(e.to_string()))
    }

    /// Fetch a single strategy by ID.
    pub async fn get(&self, id: &str) -> BotResult<StrategyDto> {
        self.client
            .get(&format!("/api/tenant/strategies/{}", id))
            .await
    }

    /// Create a new strategy template.
    pub async fn create(&self, req: &CreateStrategyRequest) -> BotResult<StrategyDto> {
        self.client.post("/api/tenant/strategies", req).await
    }

    /// Update an existing strategy.
    pub async fn update(&self, id: &str, req: &UpdateStrategyRequest) -> BotResult<StrategyDto> {
        self.client
            .put(&format!("/api/tenant/strategies/{}", id), req)
            .await
    }

    /// Archive a strategy.
    pub async fn archive(&self, id: &str) -> BotResult<bool> {
        let res: serde_json::Value = self
            .client
            .delete(&format!("/api/tenant/strategies/{}", id))
            .await?;
        Ok(res.get("archived").and_then(|v| v.as_bool()).unwrap_or(true))
    }
}
