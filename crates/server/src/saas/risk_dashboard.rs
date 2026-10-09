//! Tenant risk posture and an organization-scoped emergency kill switch.
//!
//! The control is implemented with the same durable tenant module-control
//! store used by the trading data plane. It never reads or mutates the
//! deployment-wide risk engine, because that engine cannot safely represent
//! one organization's state in a multi-tenant request.

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::{Json, Router};
use chrono::Utc;
use serde::Deserialize;
use serde_json::{json, Value};
use sqlx::Row;

use bot_core::authorization::AccessRequest;
use bot_core::membership::Permission;
use bot_core::models::BotModule;

use crate::api::ApiState;
use crate::saas::middleware::{authorize_request, deny_response};

pub fn routes() -> Router<ApiState> {
    Router::new()
        .route(
            "/api/saas/risk-dashboard",
            axum::routing::get(get_risk_dashboard),
        )
        .route(
            "/api/saas/risk-dashboard/kill-switch",
            axum::routing::post(toggle_kill_switch),
        )
}

#[derive(Debug, Deserialize)]
pub struct KillSwitchRequest {
    pub active: bool,
    pub reason: String,
}

async fn trading_module_state(
    state: &ApiState,
    organization_id: bot_core::tenant::OrganizationId,
) -> Result<(bool, Vec<Value>), Response> {
    let mut all_disabled = true;
    let mut modules = Vec::with_capacity(BotModule::TRADING.len());
    for module in BotModule::TRADING {
        let (effective_state, override_entry) = state
            .module_controls()
            .effective_state(organization_id, module)
            .await
            .map_err(|error| {
                tracing::error!(
                    error = %error,
                    organization = %organization_id,
                    module = module.as_str(),
                    "tenant kill-switch state could not be read"
                );
                (
                    StatusCode::SERVICE_UNAVAILABLE,
                    Json(json!({
                        "error": "risk_storage_unavailable",
                        "reason": "tenant module control state could not be loaded"
                    })),
                )
                    .into_response()
            })?;
        all_disabled &= effective_state == "disabled";
        modules.push(json!({
            "module": module.as_str(),
            "effective_state": effective_state,
            "override": override_entry.map(|entry| json!({
                "reason": entry.reason,
                "updated_at": entry.updated_at.to_rfc3339(),
                "updated_by": entry.updated_by,
                "version": entry.version,
            })),
        }));
    }
    Ok((all_disabled, modules))
}

async fn tenant_risk_rules(
    state: &ApiState,
    organization_id: bot_core::tenant::OrganizationId,
) -> Result<Vec<Value>, Response> {
    let Some(db) = state.db.as_deref() else {
        return Ok(Vec::new());
    };
    let row = sqlx::query("SELECT config FROM tenant_configs WHERE organization_id = $1")
        .bind(organization_id.as_uuid())
        .fetch_optional(db.pool())
        .await
        .map_err(|error| {
            tracing::error!(error = %error, organization = %organization_id, "tenant risk configuration could not be loaded");
            (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(json!({
                    "error": "risk_storage_unavailable",
                    "reason": "tenant risk configuration could not be loaded"
                })),
            )
                .into_response()
        })?;
    let Some(row) = row else {
        return Ok(Vec::new());
    };
    let document: Value = row.try_get("config").map_err(|error| {
        tracing::error!(error = %error, organization = %organization_id, "tenant risk configuration is invalid");
        (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({
                "error": "risk_storage_unavailable",
                "reason": "tenant risk configuration is invalid"
            })),
        )
            .into_response()
    })?;
    let risk = document.get("risk").and_then(Value::as_object);
    let mut rules = Vec::new();
    let fields = [
        ("max_position_usd", "maximum position", "USD"),
        ("daily_loss_usd_cap", "daily realized loss", "USD"),
        ("max_slippage_bps", "maximum slippage", "basis points"),
    ];
    for (field, name, unit) in fields {
        let Some(value) = risk.and_then(|object| object.get(field)) else {
            continue;
        };
        let Some(limit) = value.as_f64() else {
            continue;
        };
        if !limit.is_finite() || limit < 0.0 {
            continue;
        }
        rules.push(json!({
            "id": field,
            "name": name,
            "scope": "tenant",
            "unit": unit,
            "limit_ref": limit,
            "current_utilization_ref": Value::Null,
            "utilization_pct": Value::Null,
            "status": "normal",
        }));
    }
    Ok(rules)
}

pub async fn get_risk_dashboard(State(state): State<ApiState>, headers: HeaderMap) -> Response {
    let ctx = match authorize_request(
        &state,
        &headers,
        AccessRequest::read_only(Permission::RiskRead),
    )
    .await
    {
        Ok(c) => c,
        Err(d) => return deny_response(&state, &d).await,
    };
    let (kill_switch_active, modules) =
        match trading_module_state(&state, ctx.organization.id).await {
            Ok(value) => value,
            Err(response) => return response,
        };
    let rules = match tenant_risk_rules(&state, ctx.organization.id).await {
        Ok(value) => value,
        Err(response) => return response,
    };

    (
        StatusCode::OK,
        Json(json!({
            "organization_id": ctx.organization.id.to_string(),
            "kill_switch_active": kill_switch_active,
            "modules": modules,
            "durable": state.module_controls().is_durable(),
            "reference_asset": "tenant configuration units",
            "max_drawdown_limit_ref": Value::Null,
            "current_drawdown_ref": Value::Null,
            "daily_loss_limit_ref": rules.iter().find(|rule| rule.get("id").and_then(Value::as_str) == Some("daily_loss_usd_cap")).and_then(|rule| rule.get("limit_ref")).cloned().unwrap_or(Value::Null),
            "current_daily_loss_ref": Value::Null,
            "rules": rules,
            "as_of": Utc::now().to_rfc3339()
        })),
    )
        .into_response()
}

pub async fn toggle_kill_switch(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Json(body): Json<KillSwitchRequest>,
) -> Response {
    let ctx = match authorize_request(
        &state,
        &headers,
        AccessRequest::manage(Permission::RiskManage),
    )
    .await
    {
        Ok(c) => c,
        Err(d) => return deny_response(&state, &d).await,
    };
    let reason = body.reason.trim();
    if reason.is_empty() || reason.len() > 280 {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({
                "error": "invalid_kill_switch_reason",
                "reason": "reason must contain 1 to 280 characters"
            })),
        )
            .into_response();
    }

    if let Err(error) = state
        .module_controls()
        .set_trading_kill_switch(
            ctx.organization.id,
            body.active,
            reason,
            &ctx.actor_label(),
            &ctx.correlation_label(),
            Utc::now(),
        )
        .await
    {
        tracing::error!(
            error = %error,
            organization = %ctx.organization.id,
            active = body.active,
            "tenant kill switch update failed"
        );
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({
                "error": "risk_storage_unavailable",
                "reason": "tenant kill switch was not changed because the authoritative module-control store was unavailable"
            })),
        )
            .into_response();
    }

    state
        .audit
        .record(
            &ctx.actor_label(),
            if body.active {
                "risk.kill_switch.activated"
            } else {
                "risk.kill_switch.deactivated"
            },
            Some(&ctx.organization.id.to_string()),
            bot_core::audit::AuditOutcome::Success,
            json!({ "reason": reason, "scope": "tenant", "modules": BotModule::TRADING.iter().map(|module| module.as_str()).collect::<Vec<_>>() }),
        )
        .await;

    (
        StatusCode::OK,
        Json(json!({
            "success": true,
            "kill_switch_active": body.active,
            "organization_id": ctx.organization.id.to_string(),
            "durable": state.module_controls().is_durable(),
            "updated_at": Utc::now().to_rfc3339()
        })),
    )
        .into_response()
}
