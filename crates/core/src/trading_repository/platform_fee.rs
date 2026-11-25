//! Immutable, tenant-scoped fee projection journal. An inserted projection is
//! not evidence of a confirmed chain transfer or an externally settled fee.
use std::sync::Arc;

use super::{RepositoryError, TenantWriteScope, TradingQueryScope};
use crate::billing::platform_fee::{
    compute_platform_fee, FeeSurface, PlatformFeeCharge, PlatformFeePolicy,
};
use crate::db::Database;
use crate::models::ExecutionMode;
use sqlx::Row;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FeeProjectionOutcome {
    Inserted,
    IdenticalReplay,
}

pub struct TenantFeeProjectionStore {
    db: Arc<Database>,
}

impl TenantFeeProjectionStore {
    pub fn new(db: Arc<Database>) -> Self {
        Self { db }
    }

    /// Per-surface projected fee amounts, scoped to tenant, mode and asset.
    /// These are journal projections, not confirmed revenue or cash receipts.
    pub async fn totals_by_surface(
        &self,
        scope: &TradingQueryScope,
        mode: ExecutionMode,
        asset: &str,
    ) -> Result<Vec<(FeeSurface, i128)>, RepositoryError> {
        if asset.trim().is_empty() {
            return Err(RepositoryError::Validation("asset"));
        }
        let mode = match mode {
            ExecutionMode::Paper => "paper",
            ExecutionMode::Simulate => "simulate",
            ExecutionMode::Live => "live",
        };
        let org = scope.organization_id().as_uuid();
        let mut tx = self
            .db
            .pool()
            .begin()
            .await
            .map_err(RepositoryError::from)?;
        sqlx::query("SELECT set_config('app.org_id', $1, true)")
            .bind(org.to_string())
            .execute(&mut *tx)
            .await
            .map_err(RepositoryError::from)?;
        let rows = sqlx::query("SELECT surface, SUM(fee_micros)::text AS total FROM public.platform_fee_projections WHERE organization_id = $1 AND mode = $2 AND asset = $3 GROUP BY surface ORDER BY surface")
            .bind(org).bind(mode).bind(asset).fetch_all(&mut *tx).await.map_err(RepositoryError::from)?;
        let mut totals = Vec::with_capacity(rows.len());
        for row in rows {
            let surface: String = row.try_get("surface").map_err(RepositoryError::from)?;
            let surface = match surface.as_str() {
                "sniper" => FeeSurface::Sniper,
                "copy" => FeeSurface::Copy,
                "telegram_swap" => FeeSurface::TelegramSwap,
                "polymarket" => FeeSurface::Polymarket,
                _ => {
                    return Err(RepositoryError::Storage(
                        "unknown fee surface in journal".into(),
                    ))
                }
            };
            let total: String = row.try_get("total").map_err(RepositoryError::from)?;
            let total = total.parse::<i128>().map_err(|_| {
                RepositoryError::Storage("fee total exceeds supported range".into())
            })?;
            totals.push((surface, total));
        }
        tx.commit().await.map_err(RepositoryError::from)?;
        Ok(totals)
    }

    /// Record all projections as one transaction. Same tenant/key/payload is
    /// idempotent; changed payload for the same key is a conflict. Callers
    /// are responsible for ensuring live facts have actually been confirmed.
    pub async fn record(
        &self,
        scope: &TenantWriteScope,
        charge: &PlatformFeeCharge,
        mode: ExecutionMode,
    ) -> Result<FeeProjectionOutcome, RepositoryError> {
        self.record_inner(scope, charge, mode, None).await
    }

    /// Record a projection linked to a previously frozen tenant/intent policy.
    /// Matching the snapshot does not establish a confirmed fill or payment;
    /// callers must independently establish the facts before using it.
    pub async fn record_for_intent(
        &self,
        scope: &TenantWriteScope,
        intent_id: &str,
        charge: &PlatformFeeCharge,
        mode: ExecutionMode,
    ) -> Result<FeeProjectionOutcome, RepositoryError> {
        if intent_id.trim().is_empty() {
            return Err(RepositoryError::Validation("intent_id"));
        }
        self.record_inner(scope, charge, mode, Some(intent_id))
            .await
    }

    async fn record_inner(
        &self,
        scope: &TenantWriteScope,
        charge: &PlatformFeeCharge,
        mode: ExecutionMode,
        intent_id: Option<&str>,
    ) -> Result<FeeProjectionOutcome, RepositoryError> {
        let org = scope.organization_id().as_uuid();
        let charge_org = uuid::Uuid::parse_str(&charge.organization_id)
            .map_err(|_| RepositoryError::Validation("organization_id"))?;
        if charge_org != org {
            return Err(RepositoryError::TenantMismatch);
        }
        if charge.charge_id.trim().is_empty()
            || charge.asset.trim().is_empty()
            || charge.venue.trim().is_empty()
            || charge.tenant_wallet.trim().is_empty()
            || charge.platform_wallet.trim().is_empty()
            || charge.notional_micros < 0
        {
            return Err(RepositoryError::Validation("platform_fee_charge"));
        }
        PlatformFeePolicy::new(
            charge.policy.fee_bps,
            charge.policy.cap_micros,
            charge.policy.minimum_micros,
        )
        .map_err(|_| RepositoryError::Validation("platform_fee_policy"))?;
        if charge.fee_micros != compute_platform_fee(charge.notional_micros, &charge.policy) {
            return Err(RepositoryError::Validation("platform_fee_amount"));
        }
        let surface = match charge
            .surface
            .ok_or(RepositoryError::Validation("fee_surface"))?
        {
            FeeSurface::Sniper => "sniper",
            FeeSurface::Copy => "copy",
            FeeSurface::TelegramSwap => "telegram_swap",
            FeeSurface::Polymarket => "polymarket",
        };
        let mode_label = match mode {
            ExecutionMode::Paper => "paper",
            ExecutionMode::Simulate => "simulate",
            ExecutionMode::Live => "live",
        };
        let charge_json =
            serde_json::to_value(charge).map_err(|e| RepositoryError::Storage(e.to_string()))?;
        let tenant_json = serde_json::to_value(charge.entry_for_mode(mode))
            .map_err(|e| RepositoryError::Storage(e.to_string()))?;
        let platform_json = charge
            .revenue_for_mode(mode)
            .map(serde_json::to_value)
            .transpose()
            .map_err(|e| RepositoryError::Storage(e.to_string()))?;
        let mut tx = self
            .db
            .pool()
            .begin()
            .await
            .map_err(RepositoryError::from)?;
        sqlx::query("SELECT set_config('app.org_id', $1, true)")
            .bind(org.to_string())
            .execute(&mut *tx)
            .await
            .map_err(RepositoryError::from)?;
        if let Some(intent_id) = intent_id {
            let frozen = sqlx::query("SELECT surface, mode, asset, fee_bps, cap_micros, minimum_micros FROM public.platform_fee_intent_policy WHERE organization_id=$1 AND intent_id=$2")
                .bind(org).bind(intent_id).fetch_optional(&mut *tx).await.map_err(RepositoryError::from)?
                .ok_or(RepositoryError::NotFound("intent_fee_policy"))?;
            let same = frozen
                .try_get::<String, _>("surface")
                .map_err(RepositoryError::from)?
                == surface
                && frozen
                    .try_get::<String, _>("mode")
                    .map_err(RepositoryError::from)?
                    == mode_label
                && frozen
                    .try_get::<String, _>("asset")
                    .map_err(RepositoryError::from)?
                    == charge.asset
                && frozen
                    .try_get::<i32, _>("fee_bps")
                    .map_err(RepositoryError::from)?
                    == i32::from(charge.policy.fee_bps)
                && frozen
                    .try_get::<Option<i64>, _>("cap_micros")
                    .map_err(RepositoryError::from)?
                    == charge.policy.cap_micros
                && frozen
                    .try_get::<i64, _>("minimum_micros")
                    .map_err(RepositoryError::from)?
                    == charge.policy.minimum_micros;
            if !same {
                return Err(RepositoryError::Conflict("intent_fee_policy"));
            }
        }
        let inserted = sqlx::query("INSERT INTO public.platform_fee_projections (organization_id, charge_id, surface, mode, asset, fee_micros, charge, tenant_entry, platform_projection, intent_id) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10) ON CONFLICT (organization_id, charge_id) DO NOTHING RETURNING charge_id")
            .bind(org).bind(&charge.charge_id).bind(surface).bind(mode_label).bind(&charge.asset).bind(charge.fee_micros).bind(&charge_json).bind(&tenant_json).bind(&platform_json).bind(intent_id)
            .fetch_optional(&mut *tx).await.map_err(RepositoryError::from)?;
        let outcome = if inserted.is_some() {
            FeeProjectionOutcome::Inserted
        } else {
            let existing = sqlx::query("SELECT charge, mode, tenant_entry, platform_projection, intent_id FROM public.platform_fee_projections WHERE organization_id = $1 AND charge_id = $2")
                .bind(org).bind(&charge.charge_id).fetch_optional(&mut *tx).await.map_err(RepositoryError::from)?
                .ok_or(RepositoryError::Conflict("platform_fee_charge"))?;
            let old_charge: serde_json::Value =
                existing.try_get("charge").map_err(RepositoryError::from)?;
            let old_mode: String = existing.try_get("mode").map_err(RepositoryError::from)?;
            let old_entry: serde_json::Value = existing
                .try_get("tenant_entry")
                .map_err(RepositoryError::from)?;
            let old_platform: Option<serde_json::Value> = existing
                .try_get("platform_projection")
                .map_err(RepositoryError::from)?;
            let old_intent: Option<String> = existing
                .try_get("intent_id")
                .map_err(RepositoryError::from)?;
            if old_intent.as_deref() != intent_id
                || old_charge != charge_json
                || old_mode != mode_label
                || old_entry != tenant_json
                || old_platform != platform_json
            {
                return Err(RepositoryError::Conflict("platform_fee_charge"));
            }
            FeeProjectionOutcome::IdenticalReplay
        };
        tx.commit().await.map_err(RepositoryError::from)?;
        Ok(outcome)
    }
}
