//! Tenant onboarding state derived from authoritative tenant and trading records.
//!
//! Onboarding is a projection, not a second source of truth. A tenant cannot
//! advance a checkbox by sending a request: custody, wallet funding, module,
//! strategy, and paper-trade status are read from their durable systems.

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::Row;

use bot_core::membership::Permission;

use super::authorization_chain::{guard, guard_manage, TradingModuleFamily};
use crate::api::ApiState;
use bot_core::tenant::OrganizationId;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OnboardingStepPayload {
    pub step: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TenantOnboardingState {
    pub organization_id: String,
    pub current_step: u32,
    pub steps_total: u32,
    pub completed: bool,
    pub steps: OnboardingSteps,
    pub details: OnboardingDetails,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OnboardingSteps {
    pub org_created: bool,
    pub custody_configured: bool,
    pub module_enabled: bool,
    pub strategy_configured: bool,
    pub paper_trade_executed: bool,
    pub live_prerequisites_met: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OnboardingDetails {
    pub has_active_signer: bool,
    pub has_funded_wallet: bool,
    pub has_enabled_module: bool,
    pub has_saved_strategy: bool,
}

fn error_response(status: StatusCode, error: &'static str, detail: impl Into<String>) -> Response {
    (
        status,
        Json(json!({
            "error": error,
            "detail": detail.into(),
        })),
    )
        .into_response()
}

fn current_step(steps: &OnboardingSteps) -> u32 {
    if !steps.org_created {
        1
    } else if !steps.custody_configured {
        2
    } else if !steps.module_enabled {
        3
    } else if !steps.strategy_configured {
        4
    } else if !steps.paper_trade_executed {
        5
    } else {
        6
    }
}

fn state_from_facts(
    organization_id: OrganizationId,
    has_active_signer: bool,
    has_funded_wallet: bool,
    has_enabled_module: bool,
    has_saved_strategy: bool,
    has_paper_trade: bool,
) -> TenantOnboardingState {
    let steps = OnboardingSteps {
        org_created: true,
        custody_configured: has_active_signer,
        module_enabled: has_enabled_module,
        strategy_configured: has_saved_strategy,
        paper_trade_executed: has_paper_trade,
        live_prerequisites_met: has_active_signer
            && has_funded_wallet
            && has_enabled_module
            && has_saved_strategy
            && has_paper_trade,
    };
    let completed = steps.org_created
        && steps.custody_configured
        && steps.module_enabled
        && steps.strategy_configured
        && steps.paper_trade_executed
        && steps.live_prerequisites_met;

    TenantOnboardingState {
        organization_id: organization_id.to_string(),
        current_step: current_step(&steps),
        steps_total: 6,
        completed,
        steps,
        details: OnboardingDetails {
            has_active_signer,
            has_funded_wallet,
            has_enabled_module,
            has_saved_strategy,
        },
    }
}

async fn load_state(
    state: &ApiState,
    organization_id: OrganizationId,
) -> Result<TenantOnboardingState, Response> {
    let Some(db) = state.db.as_deref() else {
        return Err(error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "trading_data_plane_unavailable",
            "onboarding requires an attached PostgreSQL database",
        ));
    };

    let active_signer = sqlx::query(
        "SELECT EXISTS (
             SELECT 1
               FROM custody_signers cs
               JOIN custody_profiles cp ON cp.id = cs.custody_profile_id
              WHERE cs.organization_id = $1
                AND cp.organization_id = $1
                AND cs.status = 'active'
                AND cp.status = 'active'
         ) AS value",
    )
    .bind(organization_id.as_uuid())
    .fetch_one(db.pool())
    .await;
    let has_active_signer = match active_signer {
        Ok(row) => row.get::<bool, _>("value"),
        Err(error) => {
            tracing::error!(error = %error, "failed to load onboarding signer state");
            return Err(error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "onboarding_storage_error",
                "custody readiness could not be loaded",
            ));
        }
    };

    let funded_wallet = sqlx::query(
        "SELECT EXISTS (
             SELECT 1
               FROM tenant_bindings b
              WHERE b.organization_id = $1
                AND b.kind = 'wallet'
                AND b.active = true
                AND EXISTS (
                    SELECT 1
                      FROM balance_snapshots bs
                     WHERE bs.organization_id = $1
                       AND bs.address = b.reference
                       AND bs.ts = (
                           SELECT MAX(latest.ts)
                             FROM balance_snapshots latest
                            WHERE latest.organization_id = $1
                              AND latest.address = b.reference
                       )
                       AND COALESCE(bs.usd_value_exact::double precision, bs.usd_value, 0.0) > 0.0
                )
         ) AS value",
    )
    .bind(organization_id.as_uuid())
    .fetch_one(db.pool())
    .await;
    let has_funded_wallet = match funded_wallet {
        Ok(row) => row.get::<bool, _>("value"),
        Err(error) => {
            tracing::error!(error = %error, "failed to load onboarding wallet state");
            return Err(error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "onboarding_storage_error",
                "wallet funding state could not be loaded",
            ));
        }
    };

    let enabled_module = sqlx::query(
        "SELECT EXISTS (
             SELECT 1
               FROM tenant_module_controls
              WHERE organization_id = $1
                AND enabled = true
         ) AS value",
    )
    .bind(organization_id.as_uuid())
    .fetch_one(db.pool())
    .await;
    let has_enabled_module = match enabled_module {
        Ok(row) => row.get::<bool, _>("value"),
        Err(error) => {
            tracing::error!(error = %error, "failed to load onboarding module state");
            return Err(error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "onboarding_storage_error",
                "module readiness could not be loaded",
            ));
        }
    };

    let saved_strategy = sqlx::query(
        "SELECT EXISTS (
             SELECT 1
               FROM tenant_strategies
              WHERE organization_id = $1
                AND status NOT IN ('archived', 'deleted')
         ) AS value",
    )
    .bind(organization_id.as_uuid())
    .fetch_one(db.pool())
    .await;
    let has_saved_strategy = match saved_strategy {
        Ok(row) => row.get::<bool, _>("value"),
        Err(error) => {
            tracing::error!(error = %error, "failed to load onboarding strategy state");
            return Err(error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "onboarding_storage_error",
                "strategy readiness could not be loaded",
            ));
        }
    };

    let paper_trade = sqlx::query(
        "SELECT EXISTS (
             SELECT 1
               FROM trades
              WHERE organization_id = $1
                AND mode = 'paper'
                AND amount_in > 0
                AND amount_out >= 0
         ) AS value",
    )
    .bind(organization_id.as_uuid())
    .fetch_one(db.pool())
    .await;
    let has_paper_trade = match paper_trade {
        Ok(row) => row.get::<bool, _>("value"),
        Err(error) => {
            tracing::error!(error = %error, "failed to load onboarding paper-trade state");
            return Err(error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "onboarding_storage_error",
                "paper-trading readiness could not be loaded",
            ));
        }
    };

    Ok(state_from_facts(
        organization_id,
        has_active_signer,
        has_funded_wallet,
        has_enabled_module,
        has_saved_strategy,
        has_paper_trade,
    ))
}

/// `GET /api/tenant/onboarding`.
pub async fn get_state(State(state): State<ApiState>, headers: HeaderMap) -> Response {
    let auth = match guard(
        &state,
        &headers,
        Permission::BotRead,
        TradingModuleFamily::Sniper,
    )
    .await
    {
        Ok(a) => a,
        Err(r) => return r,
    };

    match load_state(&state, auth.organization_id()).await {
        Ok(value) => (StatusCode::OK, Json(value)).into_response(),
        Err(response) => response,
    }
}

/// `POST /api/tenant/onboarding/complete`.
///
/// Completion is intentionally an acknowledgement endpoint only. It never
/// mutates the authoritative records behind the checklist. A request succeeds
/// only when the requested step is already evidenced by those records.
pub async fn complete_step(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Json(body): Json<OnboardingStepPayload>,
) -> Response {
    let auth = match guard_manage(
        &state,
        &headers,
        Permission::BotConfigure,
        TradingModuleFamily::Sniper,
    )
    .await
    {
        Ok(a) => a,
        Err(r) => return r,
    };

    if !(1..=6).contains(&body.step) {
        return error_response(
            StatusCode::BAD_REQUEST,
            "invalid_onboarding_step",
            "step must be between 1 and 6",
        );
    }

    let current = match load_state(&state, auth.organization_id()).await {
        Ok(value) => value,
        Err(response) => return response,
    };
    let completed = match body.step {
        1 => current.steps.org_created,
        2 => current.steps.custody_configured,
        3 => current.steps.module_enabled,
        4 => current.steps.strategy_configured,
        5 => current.steps.paper_trade_executed,
        6 => current.steps.live_prerequisites_met,
        _ => false,
    };

    if !completed {
        return (
            StatusCode::CONFLICT,
            Json(json!({
                "error": "onboarding_step_not_complete",
                "detail": "the requested step has not been evidenced by the tenant records",
                "state": current,
            })),
        )
            .into_response();
    }

    (StatusCode::OK, Json(current)).into_response()
}
