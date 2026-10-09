//! Tenant-scoped position writes and open/close lifecycle
//! (PROMPT 3/10 §D25).

use std::sync::Arc;

use chrono::{DateTime, Utc};

use crate::db::Database;
use crate::trading_repository::repository_error::RepositoryError;
use crate::trading_repository::write_scope::TenantWriteScope;

use super::model::TenantPosition;

/// Write-side tenant position repository (`positions`).
pub struct TenantPositionWrite {
    db: Arc<Database>,
}

impl TenantPositionWrite {
    pub fn new(db: Arc<Database>) -> Self {
        TenantPositionWrite { db }
    }

    /// Full-row upsert of one of the acting tenant's positions
    /// (`positions.id` stays the GLOBAL app-assigned primary key — the
    /// conflict update leg is constrained to the acting tenant's row so
    /// a cross-tenant id collision can never mutate another tenant's
    /// position).
    pub async fn upsert(
        &self,
        write: &TenantWriteScope,
        position: &TenantPosition,
    ) -> Result<(), RepositoryError> {
        if position.organization_id != write.organization_id() {
            return Err(RepositoryError::TenantMismatch);
        }
        if position.id.trim().is_empty() {
            return Err(RepositoryError::Validation("position_id"));
        }
        self.db
            .timed(
                "tenant_position_upsert",
                sqlx::query(
                    r#"INSERT INTO positions
                        (organization_id, id, source, venue, mode, status, symbol,
                         symbol_display, quote_symbol, qty, avg_entry, cost_basis,
                         realized_quote, last_mark, stop_loss, take_profit,
                         trailing_stop, trailing_high_water, max_hold_secs,
                         entry_signature, exit_signature, entry_latency_ms,
                         copied_wallet, market_id, outcome, reason_closed,
                         opened_at, updated_at, closed_at)
                       VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17,
                               $18,$19,$20,$21,$22,$23,$24,$25,$26,$27,$28,$29)
                       ON CONFLICT (id) DO UPDATE SET
                         status = EXCLUDED.status, qty = EXCLUDED.qty,
                         avg_entry = EXCLUDED.avg_entry, cost_basis = EXCLUDED.cost_basis,
                         realized_quote = EXCLUDED.realized_quote,
                         last_mark = EXCLUDED.last_mark, stop_loss = EXCLUDED.stop_loss,
                         take_profit = EXCLUDED.take_profit,
                         trailing_stop = EXCLUDED.trailing_stop,
                         trailing_high_water = EXCLUDED.trailing_high_water,
                         max_hold_secs = EXCLUDED.max_hold_secs,
                         exit_signature = COALESCE(EXCLUDED.exit_signature, positions.exit_signature),
                         reason_closed = EXCLUDED.reason_closed,
                         updated_at = EXCLUDED.updated_at,
                         closed_at = COALESCE(EXCLUDED.closed_at, positions.closed_at)
                       WHERE positions.organization_id = $1"#,
                )
                .bind(position.organization_id.as_uuid())
                .bind(&position.id)
                .bind(&position.source)
                .bind(&position.venue)
                .bind(&position.mode)
                .bind(&position.status)
                .bind(&position.symbol)
                .bind(&position.symbol_display)
                .bind(&position.quote_symbol)
                .bind(position.qty)
                .bind(position.avg_entry)
                .bind(position.cost_basis)
                .bind(position.realized_quote)
                .bind(position.last_mark)
                .bind(position.stop_loss)
                .bind(position.take_profit)
                .bind(position.trailing_stop)
                .bind(position.trailing_high_water)
                .bind(position.max_hold_secs)
                .bind(&position.entry_signature)
                .bind(&position.exit_signature)
                .bind(position.entry_latency_ms)
                .bind(&position.copied_wallet)
                .bind(&position.market_id)
                .bind(&position.outcome)
                .bind(&position.reason_closed)
                .bind(position.opened_at)
                .bind(position.updated_at)
                .bind(position.closed_at)
                .execute(self.db.pool()),
            )
            .await?;
        Ok(())
    }

    /// Guarded close: transition the acting tenant's position from a
    /// live status to `closed` with the terminal fields. The tenant
    /// condition and the live-status guard live INSIDE the UPDATE.
    /// `Ok(false)` = the position is absent, foreign, or not live.
    #[allow(clippy::too_many_arguments)]
    pub async fn close(
        &self,
        write: &TenantWriteScope,
        position_id: &str,
        exit_signature: Option<&str>,
        reason_closed: Option<&str>,
        realized_quote: f64,
        last_mark: f64,
        at: DateTime<Utc>,
    ) -> Result<(), RepositoryError> {
        if position_id.trim().is_empty() {
            return Err(RepositoryError::Validation("position_id"));
        }
        let res = self
            .db
            .timed(
                "tenant_position_close",
                sqlx::query(
                    r#"UPDATE positions
                          SET status = 'closed',
                              realized_quote = $3,
                              last_mark = $4,
                              exit_signature = COALESCE($5, exit_signature),
                              reason_closed = $6,
                              updated_at = $7,
                              closed_at = $7
                        WHERE organization_id = $1 AND id = $2
                          AND status IN ('open', 'closing')"#,
                )
                .bind(write.organization_id().as_uuid())
                .bind(position_id)
                .bind(realized_quote)
                .bind(last_mark)
                .bind(exit_signature)
                .bind(reason_closed)
                .bind(at)
                .execute(self.db.pool()),
            )
            .await?;
        if res.rows_affected() == 1 {
            Ok(())
        } else {
            // Distinguish "not ours" from "ours but already terminal"
            // without ever leaking a foreign row.
            let row = self
                .db
                .timed(
                    "tenant_position_close_probe",
                    sqlx::query(
                        r#"SELECT 1 FROM positions
                            WHERE organization_id = $1 AND id = $2"#,
                    )
                    .bind(write.organization_id().as_uuid())
                    .bind(position_id)
                    .fetch_optional(self.db.pool()),
                )
                .await?;
            match row {
                Some(_) => Err(RepositoryError::StaleWrite("position status")),
                None => Err(RepositoryError::NotFound("position")),
            }
        }
    }

    /// Mark-only update (stop-loss / take-profit / trailing adjustments)
    /// on the acting tenant's LIVE positions. Zero rows for a foreign
    /// or terminal position.
    #[allow(clippy::too_many_arguments)]
    pub async fn update_risk_marks(
        &self,
        write: &TenantWriteScope,
        position_id: &str,
        stop_loss: Option<f64>,
        take_profit: Option<f64>,
        trailing_stop: Option<f64>,
        trailing_high_water: Option<f64>,
        last_mark: Option<f64>,
        at: DateTime<Utc>,
    ) -> Result<(), RepositoryError> {
        let res = self
            .db
            .timed(
                "tenant_position_risk_marks",
                sqlx::query(
                    r#"UPDATE positions
                          SET stop_loss = COALESCE($3, stop_loss),
                              take_profit = COALESCE($4, take_profit),
                              trailing_stop = COALESCE($5, trailing_stop),
                              trailing_high_water = COALESCE($6, trailing_high_water),
                              last_mark = COALESCE($7, last_mark),
                              updated_at = $8
                        WHERE organization_id = $1 AND id = $2
                          AND status IN ('open', 'closing')"#,
                )
                .bind(write.organization_id().as_uuid())
                .bind(position_id)
                .bind(stop_loss)
                .bind(take_profit)
                .bind(trailing_stop)
                .bind(trailing_high_water)
                .bind(last_mark)
                .bind(at)
                .execute(self.db.pool()),
            )
            .await?;
        if res.rows_affected() == 1 {
            Ok(())
        } else {
            Err(RepositoryError::NotFound("position"))
        }
    }
}
