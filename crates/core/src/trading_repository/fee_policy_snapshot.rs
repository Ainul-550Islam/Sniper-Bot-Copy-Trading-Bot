//! Tenant-scoped, immutable policy snapshots keyed by execution intent.
//!
//! The caller must freeze the policy before submitting an order. This store
//! neither resolves a plan nor authorizes execution, posts a fee, or proves a
//! venue transfer. A replay with different immutable inputs is a conflict.

use std::sync::Arc;

use crate::billing::platform_fee::{FeeSurface, PlatformFeePolicy};
use crate::db::Database;
use crate::models::ExecutionMode;
use crate::tenant::OrganizationId;
use sqlx::Row;

use super::{RepositoryError, TenantWriteScope, TradingQueryScope};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IntentFeePolicy {
    pub organization_id: OrganizationId,
    pub intent_id: String,
    pub surface: FeeSurface,
    pub mode: ExecutionMode,
    pub asset: String,
    pub policy: PlatformFeePolicy,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FreezeOutcome {
    Inserted,
    IdenticalReplay,
}

pub struct TenantIntentFeePolicyStore {
    db: Arc<Database>,
}

fn surface_label(surface: FeeSurface) -> &'static str {
    match surface {
        FeeSurface::Sniper => "sniper",
        FeeSurface::Copy => "copy",
        FeeSurface::TelegramSwap => "telegram_swap",
        FeeSurface::Polymarket => "polymarket",
    }
}

fn mode_label(mode: ExecutionMode) -> &'static str {
    match mode {
        ExecutionMode::Paper => "paper",
        ExecutionMode::Simulate => "simulate",
        ExecutionMode::Live => "live",
    }
}

impl TenantIntentFeePolicyStore {
    pub fn new(db: Arc<Database>) -> Self {
        Self { db }
    }

    /// Persist exactly one immutable snapshot per tenant/intent. Concurrent
    /// same-payload attempts replay; changed surface/mode/asset/policy fails.
    pub async fn freeze(
        &self,
        scope: &TenantWriteScope,
        snapshot: &IntentFeePolicy,
    ) -> Result<FreezeOutcome, RepositoryError> {
        let org = scope.organization_id();
        if org != snapshot.organization_id {
            return Err(RepositoryError::TenantMismatch);
        }
        if snapshot.intent_id.trim().is_empty() || snapshot.asset.trim().is_empty() {
            return Err(RepositoryError::Validation("intent_fee_policy"));
        }
        PlatformFeePolicy::new(
            snapshot.policy.fee_bps,
            snapshot.policy.cap_micros,
            snapshot.policy.minimum_micros,
        )
        .map_err(|_| RepositoryError::Validation("platform_fee_policy"))?;
        let surface = surface_label(snapshot.surface);
        let mode = mode_label(snapshot.mode);
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
        let inserted = sqlx::query("INSERT INTO public.platform_fee_intent_policy (organization_id, intent_id, surface, mode, asset, fee_bps, cap_micros, minimum_micros) VALUES ($1,$2,$3,$4,$5,$6,$7,$8) ON CONFLICT (organization_id,intent_id) DO NOTHING RETURNING intent_id")
            .bind(org.as_uuid()).bind(&snapshot.intent_id).bind(surface).bind(mode)
            .bind(&snapshot.asset).bind(i32::from(snapshot.policy.fee_bps))
            .bind(snapshot.policy.cap_micros).bind(snapshot.policy.minimum_micros)
            .fetch_optional(&mut *tx).await.map_err(RepositoryError::from)?;
        let outcome = if inserted.is_some() {
            FreezeOutcome::Inserted
        } else {
            let row = sqlx::query("SELECT surface, mode, asset, fee_bps, cap_micros, minimum_micros FROM public.platform_fee_intent_policy WHERE organization_id = $1 AND intent_id = $2")
                .bind(org.as_uuid()).bind(&snapshot.intent_id)
                .fetch_optional(&mut *tx).await.map_err(RepositoryError::from)?
                .ok_or(RepositoryError::Conflict("intent_fee_policy"))?;
            let same = row
                .try_get::<String, _>("surface")
                .map_err(RepositoryError::from)?
                == surface
                && row
                    .try_get::<String, _>("mode")
                    .map_err(RepositoryError::from)?
                    == mode
                && row
                    .try_get::<String, _>("asset")
                    .map_err(RepositoryError::from)?
                    == snapshot.asset
                && row
                    .try_get::<i32, _>("fee_bps")
                    .map_err(RepositoryError::from)?
                    == i32::from(snapshot.policy.fee_bps)
                && row
                    .try_get::<Option<i64>, _>("cap_micros")
                    .map_err(RepositoryError::from)?
                    == snapshot.policy.cap_micros
                && row
                    .try_get::<i64, _>("minimum_micros")
                    .map_err(RepositoryError::from)?
                    == snapshot.policy.minimum_micros;
            if !same {
                return Err(RepositoryError::Conflict("intent_fee_policy"));
            }
            FreezeOutcome::IdenticalReplay
        };
        tx.commit().await.map_err(RepositoryError::from)?;
        Ok(outcome)
    }

    /// Read the exact frozen policy for the acting tenant. The caller cannot
    /// load another tenant's policy by supplying only an intent id.
    pub async fn get(
        &self,
        scope: &TradingQueryScope,
        intent_id: &str,
    ) -> Result<Option<IntentFeePolicy>, RepositoryError> {
        if intent_id.trim().is_empty() {
            return Err(RepositoryError::Validation("intent_id"));
        }
        let org = scope.organization_id();
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
        let row = sqlx::query("SELECT surface, mode, asset, fee_bps, cap_micros, minimum_micros FROM public.platform_fee_intent_policy WHERE organization_id = $1 AND intent_id = $2")
            .bind(org.as_uuid()).bind(intent_id).fetch_optional(&mut *tx).await.map_err(RepositoryError::from)?;
        let snapshot = if let Some(row) = row {
            let surface = match row
                .try_get::<String, _>("surface")
                .map_err(RepositoryError::from)?
                .as_str()
            {
                "sniper" => FeeSurface::Sniper,
                "copy" => FeeSurface::Copy,
                "telegram_swap" => FeeSurface::TelegramSwap,
                "polymarket" => FeeSurface::Polymarket,
                _ => return Err(RepositoryError::Storage("unknown fee surface".into())),
            };
            let mode = match row
                .try_get::<String, _>("mode")
                .map_err(RepositoryError::from)?
                .as_str()
            {
                "paper" => ExecutionMode::Paper,
                "simulate" => ExecutionMode::Simulate,
                "live" => ExecutionMode::Live,
                _ => return Err(RepositoryError::Storage("unknown execution mode".into())),
            };
            let fee_bps = u16::try_from(
                row.try_get::<i32, _>("fee_bps")
                    .map_err(RepositoryError::from)?,
            )
            .map_err(|_| RepositoryError::Storage("invalid fee bps".into()))?;
            let policy = PlatformFeePolicy::new(
                fee_bps,
                row.try_get("cap_micros").map_err(RepositoryError::from)?,
                row.try_get("minimum_micros")
                    .map_err(RepositoryError::from)?,
            )
            .map_err(|_| RepositoryError::Storage("invalid frozen fee policy".into()))?;
            Some(IntentFeePolicy {
                organization_id: org,
                intent_id: intent_id.to_owned(),
                surface,
                mode,
                asset: row.try_get("asset").map_err(RepositoryError::from)?,
                policy,
            })
        } else {
            None
        };
        tx.commit().await.map_err(RepositoryError::from)?;
        Ok(snapshot)
    }
}
