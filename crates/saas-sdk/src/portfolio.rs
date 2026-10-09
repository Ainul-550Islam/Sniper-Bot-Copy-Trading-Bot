//! Typed Portfolio & Exposure SDK Client (THIRD.md §146).

use serde::{Deserialize, Serialize};

use crate::client::Client;
use crate::error::Result;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AssetExposure {
    pub asset_symbol: String,
    pub amount_lamports: Option<u64>,
    pub amount_units: f64,
    pub value_usd_cents: u64,
    pub percentage_bps: u32,
    pub venue: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PortfolioSummary {
    pub organization_id: String,
    pub total_equity_usd_cents: u64,
    pub available_cash_usd_cents: u64,
    pub allocated_margin_usd_cents: u64,
    pub unrealized_pnl_usd_cents: i64,
    pub realized_pnl_30d_usd_cents: i64,
    pub max_drawdown_bps: u32,
    pub exposures: Vec<AssetExposure>,
    pub as_of: String,
    pub is_stale: bool,
}

/// Portfolio and exposure API client.
#[derive(Debug, Clone)]
pub struct PortfolioClient {
    client: Client,
}

impl PortfolioClient {
    pub fn new(client: Client) -> Self {
        Self { client }
    }

    /// Fetches tenant portfolio summary.
    pub async fn get_summary(&self) -> Result<PortfolioSummary> {
        self.client.get("/api/saas/portfolio").await
    }
}
