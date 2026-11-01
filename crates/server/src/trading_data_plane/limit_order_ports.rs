//! Production implementations of the limit-order worker's two ports.
//!
//! The worker (`limit_order_worker`) was written against `MarkSource` and
//! `TriggerSink`, but only test doubles ever implemented them, so the worker
//! could not be started without inventing prices or fabricating fills. This
//! module supplies both ports from real infrastructure:
//!
//! * [`JupiterMarkSource`] — a *live* mark, from Jupiter's public price API.
//!   SOL-per-whole-token is `usdPrice(mint) / usdPrice(WSOL)`, both read from
//!   the same response so the ratio is internally consistent. A missing mint,
//!   a non-200, a timeout, a non-finite or non-positive price all return
//!   `None` — and the worker's rule is that a missing mark never fires an
//!   order, so the order simply keeps waiting. Nothing is estimated,
//!   interpolated or cached beyond the request.
//!
//! * [`IntentJournalSink`] — a durable, idempotent hand-off into the project's
//!   own write-ahead execution journal (`execution_intents`, migration 0007).
//!   The insert is `ON CONFLICT (organization_id, intent_id) DO NOTHING` on
//!   0031's composite arbiter and carries the order id as the idempotency key,
//!   so a crash-and-retry cannot double-fire. **It journals an intent, not a
//!   fill**: no signature, balance or venue state is fabricated here, and the
//!   order is reported as `triggered` (intent journaled), never as filled.
//!   Broadcasting the intent is the engine/outbox step, exactly as the DCA
//!   scheduler documents for `dca_runs`.

use std::time::Duration;

use async_trait::async_trait;
use sqlx::PgPool;

use bot_core::tenant::rls_context::with_tenant_scope;
use module_sniper::limit_orders::OrderSide;

use crate::trading_data_plane::limit_order_worker::{ClaimedOrder, MarkSource, TriggerSink};

/// Jupiter's public price endpoint (no key required).
pub const JUPITER_PRICE_API: &str = "https://lite-api.jup.ag/price/v3";
/// Wrapped SOL — the quotation currency for every mark in this system.
pub const WSOL_MINT: &str = "So11111111111111111111111111111111111111112";

/// SOL per whole token, from two USD prices read in the same response.
///
/// Pure and total: any non-finite or non-positive input yields `None` rather
/// than a bogus mark, because a bogus mark could fire a real order.
pub fn sol_per_token(token_usd: f64, sol_usd: f64) -> Option<f64> {
    if !token_usd.is_finite() || !sol_usd.is_finite() || token_usd <= 0.0 || sol_usd <= 0.0 {
        return None;
    }
    let ratio = token_usd / sol_usd;
    if ratio.is_finite() && ratio > 0.0 {
        Some(ratio)
    } else {
        None
    }
}

/// Live `MarkSource` backed by Jupiter's price API.
#[derive(Debug, Clone)]
pub struct JupiterMarkSource {
    http: reqwest::Client,
    price_url: String,
}

impl Default for JupiterMarkSource {
    fn default() -> Self {
        Self::new()
    }
}

impl JupiterMarkSource {
    pub fn new() -> Self {
        Self::with_url(JUPITER_PRICE_API)
    }

    /// Injectable URL — used by tests to point at a local server.
    pub fn with_url(price_url: impl Into<String>) -> Self {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(8))
            .build()
            .unwrap_or_default();
        Self {
            http,
            price_url: price_url.into(),
        }
    }

    pub fn price_url(&self) -> &str {
        &self.price_url
    }
}

#[async_trait]
impl MarkSource for JupiterMarkSource {
    async fn mark_sol(&self, mint: &str) -> Option<f64> {
        let mint = mint.trim();
        if mint.is_empty() {
            return None;
        }
        // One request for both legs: the ratio can never mix two different
        // snapshots, and a partial answer is not a mark.
        let response = self
            .http
            .get(&self.price_url)
            .query(&[("ids", format!("{mint},{WSOL_MINT}"))])
            .send()
            .await
            .ok()?;
        if !response.status().is_success() {
            tracing::warn!(status = %response.status(), %mint, "mark source: jupiter price http error");
            return None;
        }
        let body: serde_json::Value = response.json().await.ok()?;
        let token_usd = body.get(mint)?.get("usdPrice")?.as_f64()?;
        let sol_usd = body.get(WSOL_MINT)?.get("usdPrice")?.as_f64()?;
        let mark = sol_per_token(token_usd, sol_usd);
        if mark.is_none() {
            tracing::warn!(token_usd, sol_usd, %mint, "mark source: non-positive price, no mark");
        }
        mark
    }
}

/// Durable hand-off into `execution_intents` (write-ahead journal).
///
/// Runs in its own tenant scope: the worker calls this from inside a tenant
/// scope, but the sink must not depend on a connection it does not own, so it
/// opens a scoped transaction for the order's organization and lets RLS
/// enforce the boundary a second time.
pub struct IntentJournalSink {
    pool: PgPool,
}

impl IntentJournalSink {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl TriggerSink for IntentJournalSink {
    async fn hand_off(&self, order: &ClaimedOrder, fired_at_sol: f64) -> Result<(), String> {
        let side = match order.side {
            OrderSide::Buy => "buy",
            OrderSide::Sell => "sell",
        };
        // The journal row name is stable per order+side+mark-bucket: the order
        // id is the idempotency key the worker contract demands.
        let intent_id = format!("limit-order:{}", order.id);
        let qty = format!("{:.9}", order.amount);
        let symbol = order.mint.clone();
        let symbol_for_log = symbol.clone();
        let org = order.organization_id;

        let result = with_tenant_scope(&self.pool, org, |connection| {
            Box::pin(async move {
                sqlx::query(
                    r#"INSERT INTO execution_intents
                           (organization_id, intent_id, module, symbol, wallet, side, qty)
                       VALUES ($1, $2, 'limit_order', $3, '', $4, $5)
                       ON CONFLICT (organization_id, intent_id) DO NOTHING"#,
                )
                .bind(org)
                .bind(&intent_id)
                .bind(&symbol)
                .bind(side)
                .bind(&qty)
                .execute(&mut *connection)
                .await
            })
        })
        .await
        .map_err(|error| format!("intent journal write failed: {error}"))?;

        // Observability only — the persisted state is the journal row.
        let _ = result;
        tracing::info!(
            order_id = %order.id,
            symbol = %symbol_for_log,
            side,
            fired_at_sol,
            "limit order triggered; execution intent journaled (no fill claimed)"
        );
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ratio_is_sol_per_whole_token() {
        // 2 USD token against a 100 USD SOL is 0.02 SOL per token.
        assert_eq!(sol_per_token(2.0, 100.0), Some(0.02));
    }

    #[test]
    fn refuses_to_produce_a_mark_from_bad_input() {
        assert_eq!(sol_per_token(0.0, 100.0), None);
        assert_eq!(sol_per_token(2.0, 0.0), None);
        assert_eq!(sol_per_token(-1.0, 100.0), None);
        assert_eq!(sol_per_token(f64::NAN, 100.0), None);
        assert_eq!(sol_per_token(2.0, f64::INFINITY), None);
    }
}
