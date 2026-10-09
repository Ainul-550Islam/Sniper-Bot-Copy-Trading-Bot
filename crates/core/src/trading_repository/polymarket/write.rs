//! Tenant-scoped polymarket writes (PROMPT 3/10 §G44).
//!
//! Semantics are the legacy pipeline's (`db::polymarket`) with the
//! 0030 composite arbiters: signals advance stage (linkage columns
//! fill in via COALESCE — never overwritten), mirror orders keep the
//! monotonic `size_matched` and the `closed_at`-COALESCE terminal
//! discipline, fills are insert-once per tenant.

use std::sync::Arc;

use chrono::{DateTime, Utc};

use crate::db::Database;
use crate::trading_repository::repository_error::RepositoryError;
use crate::trading_repository::write_scope::TenantWriteScope;

use super::model::{TenantPolyFill, TenantPolyOrder, TenantPolySignal};

/// Write-side tenant polymarket repository.
pub struct TenantPolyWrite {
    db: Arc<Database>,
}

impl TenantPolyWrite {
    pub fn new(db: Arc<Database>) -> Self {
        TenantPolyWrite { db }
    }

    fn check_mode(mode: &str) -> Result<(), RepositoryError> {
        if matches!(mode, "paper" | "simulate" | "live") {
            Ok(())
        } else {
            Err(RepositoryError::Validation("poly mode"))
        }
    }

    /// Record/advance one of the acting tenant's signals on the 0030
    /// composite arbiter `(organization_id, signal_id)`. Retries of
    /// THIS tenant's signal advance the stage and fill in linkage;
    /// another tenant's same signal_id is a different row.
    pub async fn record_signal(
        &self,
        write: &TenantWriteScope,
        sig: &TenantPolySignal,
    ) -> Result<(), RepositoryError> {
        if sig.organization_id != write.organization_id() {
            return Err(RepositoryError::TenantMismatch);
        }
        if sig.signal_id.trim().is_empty() {
            return Err(RepositoryError::Validation("signal_id"));
        }
        if !matches!(sig.side.as_str(), "buy" | "sell") {
            return Err(RepositoryError::Validation("side"));
        }
        Self::check_mode(&sig.mode)?;
        self.db
            .timed(
                "tenant_poly_signal_upsert",
                sqlx::query(
                    r#"INSERT INTO poly_signals
                           (organization_id, signal_id, condition_id, token_id, outcome,
                            side, strategy, limit_price, size_tokens, stake_usd, mode,
                            stage, reject_reason, detail, order_id, venue_order_id,
                            position_id, created_at, updated_at)
                       VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17,$18,$19)
                       ON CONFLICT (organization_id, signal_id) DO UPDATE SET
                           stage = EXCLUDED.stage,
                           reject_reason = EXCLUDED.reject_reason,
                           detail = EXCLUDED.detail,
                           order_id = COALESCE(EXCLUDED.order_id, poly_signals.order_id),
                           venue_order_id = COALESCE(EXCLUDED.venue_order_id, poly_signals.venue_order_id),
                           position_id = COALESCE(EXCLUDED.position_id, poly_signals.position_id),
                           updated_at = EXCLUDED.updated_at"#,
                )
                .bind(sig.organization_id.as_uuid())
                .bind(&sig.signal_id)
                .bind(&sig.condition_id)
                .bind(&sig.token_id)
                .bind(&sig.outcome)
                .bind(&sig.side)
                .bind(&sig.strategy)
                .bind(sig.limit_price)
                .bind(sig.size_tokens)
                .bind(sig.stake_usd)
                .bind(&sig.mode)
                .bind(&sig.stage)
                .bind(&sig.reject_reason)
                .bind(&sig.detail)
                .bind(&sig.order_id)
                .bind(&sig.venue_order_id)
                .bind(&sig.position_id)
                .bind(sig.created_at)
                .bind(sig.updated_at)
                .execute(self.db.pool()),
            )
            .await?;
        Ok(())
    }

    /// Upsert one of the acting tenant's mirror orders on the 0030
    /// composite arbiter `(organization_id, venue_order_id)`: the SAME
    /// venue order id mirrored by another tenant is a different row;
    /// `size_matched` only moves forward; `closed_at` never un-sets.
    pub async fn upsert_order(
        &self,
        write: &TenantWriteScope,
        ord: &TenantPolyOrder,
    ) -> Result<(), RepositoryError> {
        if ord.organization_id != write.organization_id() {
            return Err(RepositoryError::TenantMismatch);
        }
        if ord.venue_order_id.trim().is_empty() || ord.order_id.trim().is_empty() {
            return Err(RepositoryError::Validation("venue_order_id/order_id"));
        }
        if !matches!(ord.side.as_str(), "buy" | "sell") {
            return Err(RepositoryError::Validation("side"));
        }
        Self::check_mode(&ord.mode)?;
        self.db
            .timed(
                "tenant_poly_order_upsert",
                sqlx::query(
                    r#"INSERT INTO poly_orders
                           (organization_id, venue_order_id, order_id, signal_id,
                            condition_id, token_id, outcome, side, order_type,
                            limit_price, size_tokens, size_matched, mode, state,
                            venue_status, expiration, position_id, replica_id,
                            submitted_at, updated_at, closed_at)
                       VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17,$18,$19,$20,$21)
                       ON CONFLICT (organization_id, venue_order_id) DO UPDATE SET
                           order_id = EXCLUDED.order_id,
                           size_matched = GREATEST(poly_orders.size_matched, EXCLUDED.size_matched),
                           state = EXCLUDED.state,
                           venue_status = EXCLUDED.venue_status,
                           position_id = COALESCE(EXCLUDED.position_id, poly_orders.position_id),
                           replica_id = EXCLUDED.replica_id,
                           updated_at = EXCLUDED.updated_at,
                           closed_at = COALESCE(EXCLUDED.closed_at, poly_orders.closed_at)"#,
                )
                .bind(ord.organization_id.as_uuid())
                .bind(&ord.venue_order_id)
                .bind(&ord.order_id)
                .bind(&ord.signal_id)
                .bind(&ord.condition_id)
                .bind(&ord.token_id)
                .bind(&ord.outcome)
                .bind(&ord.side)
                .bind(&ord.order_type)
                .bind(ord.limit_price)
                .bind(ord.size_tokens)
                .bind(ord.size_matched)
                .bind(&ord.mode)
                .bind(&ord.state)
                .bind(&ord.venue_status)
                .bind(ord.expiration)
                .bind(&ord.position_id)
                .bind(&ord.replica_id)
                .bind(ord.submitted_at)
                .bind(ord.updated_at)
                .bind(ord.closed_at)
                .execute(self.db.pool()),
            )
            .await?;
        Ok(())
    }

    /// Terminalize one of the acting tenant's mirror orders: set the
    /// state and stamp `closed_at`. Guarded on the order still being
    /// open — a foreign or already-closed order affects zero rows.
    pub async fn close_order(
        &self,
        write: &TenantWriteScope,
        venue_order_id: &str,
        state: &str,
        venue_status: Option<&str>,
        at: DateTime<Utc>,
    ) -> Result<(), RepositoryError> {
        if state.trim().is_empty() {
            return Err(RepositoryError::Validation("state"));
        }
        let res = self
            .db
            .timed(
                "tenant_poly_order_close",
                sqlx::query(
                    r#"UPDATE poly_orders
                          SET state = $3,
                              venue_status = COALESCE($4, venue_status),
                              updated_at = $5,
                              closed_at = COALESCE(closed_at, $5)
                        WHERE organization_id = $1 AND venue_order_id = $2
                          AND closed_at IS NULL"#,
                )
                .bind(write.organization_id().as_uuid())
                .bind(venue_order_id)
                .bind(state)
                .bind(venue_status)
                .bind(at)
                .execute(self.db.pool()),
            )
            .await?;
        if res.rows_affected() == 1 {
            Ok(())
        } else {
            let row = self
                .db
                .timed(
                    "tenant_poly_order_probe",
                    sqlx::query(
                        r#"SELECT 1 FROM poly_orders
                            WHERE organization_id = $1 AND venue_order_id = $2"#,
                    )
                    .bind(write.organization_id().as_uuid())
                    .bind(venue_order_id)
                    .fetch_optional(self.db.pool()),
                )
                .await?;
            match row {
                Some(_) => Err(RepositoryError::StaleWrite("poly order closed")),
                None => Err(RepositoryError::NotFound("poly_order")),
            }
        }
    }

    /// Record one of the acting tenant's fills on the 0030 composite
    /// arbiter `(organization_id, fill_id)` — replay of the venue fill
    /// stream is idempotent per tenant.
    pub async fn record_fill(
        &self,
        write: &TenantWriteScope,
        fill: &TenantPolyFill,
    ) -> Result<(), RepositoryError> {
        if fill.organization_id != write.organization_id() {
            return Err(RepositoryError::TenantMismatch);
        }
        if fill.fill_id.trim().is_empty() {
            return Err(RepositoryError::Validation("fill_id"));
        }
        if !matches!(fill.side.as_str(), "buy" | "sell") {
            return Err(RepositoryError::Validation("side"));
        }
        self.db
            .timed(
                "tenant_poly_fill_upsert",
                sqlx::query(
                    r#"INSERT INTO poly_fills
                           (organization_id, fill_id, venue_order_id, order_id, token_id,
                            side, price, size_tokens, quote_usd, source, position_id, ts)
                       VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12)
                       ON CONFLICT (organization_id, fill_id) DO NOTHING"#,
                )
                .bind(fill.organization_id.as_uuid())
                .bind(&fill.fill_id)
                .bind(&fill.venue_order_id)
                .bind(&fill.order_id)
                .bind(&fill.token_id)
                .bind(&fill.side)
                .bind(fill.price)
                .bind(fill.size_tokens)
                .bind(fill.quote_usd)
                .bind(&fill.source)
                .bind(&fill.position_id)
                .bind(fill.ts)
                .execute(self.db.pool()),
            )
            .await?;
        Ok(())
    }
}
