//! Shared market-data domain types and provider abstractions (SECOND.md §96).

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::models::{BotModule, Venue};

/// Canonical market summary for discovery and screener tables.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MarketTicker {
    pub id: String,
    pub symbol: String,
    pub name: String,
    pub venue: Venue,
    pub base_asset: String,
    pub quote_asset: String,
    pub price_usd_cents: u64,
    pub change_24h_bps: i32,
    pub volume_24h_usd_cents: u64,
    pub liquidity_usd_cents: u64,
    pub is_active: bool,
    pub compatible_modules: Vec<BotModule>,
    pub updated_at: DateTime<Utc>,
}

impl MarketTicker {
    pub fn new(
        id: String,
        symbol: String,
        name: String,
        venue: Venue,
        base_asset: String,
        quote_asset: String,
        price_usd_cents: u64,
        change_24h_bps: i32,
        volume_24h_usd_cents: u64,
        liquidity_usd_cents: u64,
        compatible_modules: Vec<BotModule>,
        now: DateTime<Utc>,
    ) -> Self {
        Self {
            id,
            symbol,
            name,
            venue,
            base_asset,
            quote_asset,
            price_usd_cents,
            change_24h_bps,
            volume_24h_usd_cents,
            liquidity_usd_cents,
            is_active: true,
            compatible_modules,
            updated_at: now,
        }
    }
}
