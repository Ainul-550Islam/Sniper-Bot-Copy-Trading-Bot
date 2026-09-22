//! Venue session and kill-switch operations.
//!
//! L1 → L2 credential derivation ([`PolyBot::ensure_api_key`]), the
//! authenticated CLOB client every lifecycle / reconciliation call uses
//! ([`PolyBot::authed_client`]), the dead-man's-switch heartbeat, the
//! provider-side status read the server's order reconciliation relies on
//! ([`PolyBot::order_status`]) and the kill-switch paths
//! ([`PolyBot::cancel_all`] — closed locally only for orders the venue's
//! `canceled` list names — and [`PolyBot::flatten`]).

use tracing::{debug, info, warn};

use bot_core::models::{BotModule, ExecutionMode, PositionStatus};

use crate::clob::{CancelOutcome, ClobClient};
use crate::error::{PolyError, PolyResult};
use crate::metrics;
use crate::orders::{LocalOrderState, TrackedOrder};
use crate::PolyBot;

impl PolyBot {
    /// Provider-side status of one order (reconciliation truth source).
    /// `Ok(None)` when this bot cannot authenticate (no signer / no key) —
    /// the caller treats that as "retry later", not as an answer.
    pub async fn order_status(&self, order_id: &str) -> PolyResult<Option<serde_json::Value>> {
        if self.ensure_api_key().await.is_err() {
            return Ok(None);
        }
        let Some(ak) = self.api_key.read().await.clone() else {
            return Ok(None);
        };
        let client = self
            .clob
            .clone()
            .with_auth(self.address.clone().unwrap_or_default(), ak);
        Ok(Some(client.order_status(order_id).await?))
    }

    /// Derive API credentials from the signer (L1 auth) if not already present.
    pub(crate) async fn ensure_api_key(&self) -> PolyResult<()> {
        if self.api_key.read().await.is_some() {
            return Ok(());
        }
        let key = self
            .signer
            .as_ref()
            .ok_or_else(|| PolyError::not_configured("no signer"))?;
        let address = self
            .address
            .clone()
            .ok_or_else(|| PolyError::not_configured("no address"))?;
        let cfg = self.state.config_snapshot().await;
        let creds = ClobClient::derive_api_key(
            &cfg.polymarket.clob_url,
            cfg.polymarket.chain_id,
            key,
            &address,
        )
        .await?;
        info!(credentials = %creds.redacted(), "derived CLOB api credentials");
        *self.api_key.write().await = Some(creds);
        Ok(())
    }

    pub(crate) async fn authed_client(&self) -> PolyResult<ClobClient> {
        let Some(ak) = self.api_key.read().await.clone() else {
            return Err(PolyError::not_configured("no api key"));
        };
        Ok(self
            .clob
            .clone()
            .with_auth(self.address.clone().unwrap_or_default(), ak))
    }

    /// Spawn the heartbeat task (dead-man's switch).
    pub(crate) fn spawn_heartbeat(&self, interval_secs: u64) {
        let clob = self.clob.clone();
        let address = self.address.clone().unwrap_or_default();
        let api_key = self.api_key.clone();
        let state = self.state.clone();
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(std::time::Duration::from_secs(interval_secs));
            loop {
                tokio::select! {
                    _ = ticker.tick() => {}
                    _ = state.wait_shutdown() => break,
                }
                let Some(ak) = api_key.read().await.clone() else {
                    continue;
                };
                let client = clob.clone().with_auth(address.clone(), ak);
                if let Err(e) = client.heartbeat().await {
                    debug!(error = %e, "polymarket heartbeat failed");
                    state
                        .record_error(BotModule::Polymarket, &format!("heartbeat: {e}"))
                        .await;
                }
            }
        });
    }

    /// Cancel all open CLOB orders (used by the kill switch path). Tracked
    /// orders are closed locally only when the venue's answer names them in
    /// its `canceled` list; anything it refused or did not mention stays
    /// open locally and is resolved by the next poll / reconciliation
    /// against venue truth rather than by assumption.
    pub async fn cancel_all(&self) -> PolyResult<()> {
        let Some(ak) = self.api_key.read().await.clone() else {
            return Err(PolyError::not_configured("no api key"));
        };
        let client = self
            .clob
            .clone()
            .with_auth(self.address.clone().unwrap_or_default(), ak);
        let resp = client.cancel_all().await?;
        let outcome = CancelOutcome::from_value(&resp);
        metrics::count_cancel("cancel_all");
        let open: Vec<TrackedOrder> = self
            .tracked
            .read()
            .await
            .values()
            .filter(|t| !t.state.is_terminal() && t.mode == ExecutionMode::Live)
            .cloned()
            .collect();
        let mut unconfirmed = 0usize;
        for mut t in open {
            if outcome.confirmed(&t.venue_order_id) {
                self.finish_locally(&mut t, LocalOrderState::Cancelled, "cancel_all")
                    .await;
            } else {
                unconfirmed += 1;
                debug!(
                    order = %t.venue_order_id,
                    "cancel_all: venue did not confirm this order; left to poll / reconciliation"
                );
            }
        }
        if unconfirmed > 0 {
            warn!(
                unconfirmed,
                confirmed = outcome.canceled.len(),
                refused = outcome.not_canceled.len(),
                "polymarket cancel_all: some tracked orders were not confirmed cancelled"
            );
        }
        Ok(())
    }

    /// Mark all open polymarket positions as stopped (kill switch flatten).
    pub async fn flatten(&self, reason: &str) {
        let positions = self.state.open_positions_for(BotModule::Polymarket).await;
        for p in positions {
            self.state
                .close_position(&p.id, PositionStatus::StoppedOut, reason)
                .await;
        }
    }
}
