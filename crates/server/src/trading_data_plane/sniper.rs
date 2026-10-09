//! Tenant Sniper controls and durable configuration.
//!
//! Status and controls use the tenant authorization chain. Configuration is
//! stored in the shared durable tenant configuration document; this handler
//! never creates a process-local config or supplies trading defaults.

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::{json, Map, Value};

use bot_core::membership::Permission;
use bot_core::models::BotModule;

use super::authorization_chain::{guard, guard_manage, TradingModuleFamily};
use super::config_store::{read_module, write_module};
use super::module_controls::{apply_control, feature_key_for, status_payload, ControlAction};
use crate::api::ApiState;

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

fn validate_patch(body: Value) -> Result<Map<String, Value>, Response> {
    let Some(object) = body.as_object() else {
        return Err(error_response(
            StatusCode::BAD_REQUEST,
            "invalid_config",
            "configuration body must be a JSON object",
        ));
    };
    const ALLOWED: &[&str] = &[
        "min_liquidity_sol",
        "max_slippage_bps",
        "anti_mev_protection",
        "priority_fee_lamports",
        "entry_amount_sol",
        "take_profit_pct",
        "stop_loss_pct",
        "trailing_stop_pct",
        "auto_sell_timeout_seconds",
        "dry_run",
        "blacklisted_tokens",
        "dex_routing",
    ];
    if let Some(unknown) = object.keys().find(|key| !ALLOWED.contains(&key.as_str())) {
        return Err(error_response(
            StatusCode::BAD_REQUEST,
            "unknown_config_field",
            format!("unsupported sniper configuration field: {unknown}"),
        ));
    }

    let finite_number = |key: &str, minimum: f64, maximum: f64| -> Result<(), Response> {
        if let Some(value) = object.get(key) {
            let Some(number) = value.as_f64() else {
                return Err(error_response(
                    StatusCode::BAD_REQUEST,
                    "invalid_config_field",
                    format!("{key} must be a number"),
                ));
            };
            if !number.is_finite() || number < minimum || number > maximum {
                return Err(error_response(
                    StatusCode::BAD_REQUEST,
                    "invalid_config_field",
                    format!("{key} must be between {minimum} and {maximum}"),
                ));
            }
        }
        Ok(())
    };
    finite_number("min_liquidity_sol", 0.0, 10_000_000.0)?;
    finite_number("entry_amount_sol", 0.0, 10_000_000.0)?;
    finite_number("take_profit_pct", 0.0, 100_000.0)?;
    finite_number("stop_loss_pct", 0.0, 100.0)?;
    finite_number("trailing_stop_pct", 0.0, 100.0)?;

    if let Some(value) = object.get("max_slippage_bps") {
        if value.as_u64().is_none_or(|number| number > 10_000) {
            return Err(error_response(
                StatusCode::BAD_REQUEST,
                "invalid_config_field",
                "max_slippage_bps must be an integer between 0 and 10000",
            ));
        }
    }
    if let Some(value) = object.get("priority_fee_lamports") {
        if value.as_u64().is_none_or(|number| number > 10_000_000_000) {
            return Err(error_response(
                StatusCode::BAD_REQUEST,
                "invalid_config_field",
                "priority_fee_lamports must be an integer between 0 and 10000000000",
            ));
        }
    }
    if let Some(value) = object.get("auto_sell_timeout_seconds") {
        if value.as_u64().is_none_or(|number| number > 31_536_000) {
            return Err(error_response(
                StatusCode::BAD_REQUEST,
                "invalid_config_field",
                "auto_sell_timeout_seconds must be an integer between 0 and 31536000",
            ));
        }
    }
    if let Some(value) = object.get("anti_mev_protection") {
        if !value.is_boolean() {
            return Err(error_response(
                StatusCode::BAD_REQUEST,
                "invalid_config_field",
                "anti_mev_protection must be boolean",
            ));
        }
    }
    if let Some(value) = object.get("dry_run") {
        if !value.is_boolean() {
            return Err(error_response(
                StatusCode::BAD_REQUEST,
                "invalid_config_field",
                "dry_run must be boolean",
            ));
        }
    }
    if let Some(value) = object.get("blacklisted_tokens") {
        let Some(items) = value.as_array() else {
            return Err(error_response(
                StatusCode::BAD_REQUEST,
                "invalid_config_field",
                "blacklisted_tokens must be an array of strings",
            ));
        };
        if items.len() > 10_000
            || items.iter().any(|item| {
                item.as_str()
                    .is_none_or(|text| text.is_empty() || text.len() > 128)
            })
        {
            return Err(error_response(
                StatusCode::BAD_REQUEST,
                "invalid_config_field",
                "blacklisted_tokens contains an invalid item",
            ));
        }
    }
    if let Some(value) = object.get("dex_routing") {
        if !matches!(
            value.as_str(),
            Some("auto" | "raydium_v4" | "pumpfun" | "pumpswap")
        ) {
            return Err(error_response(
                StatusCode::BAD_REQUEST,
                "invalid_config_field",
                "dex_routing is not supported",
            ));
        }
    }
    Ok(object.clone())
}

/// `GET /api/tenant/sniper/status`
pub async fn status(State(state): State<ApiState>, headers: HeaderMap) -> Response {
    let auth = match guard(
        &state,
        &headers,
        Permission::BotRead,
        TradingModuleFamily::Sniper,
    )
    .await
    {
        Ok(value) => value,
        Err(response) => return response,
    };
    (
        StatusCode::OK,
        Json(
            status_payload(
                &state,
                &auth,
                BotModule::Sniper,
                feature_key_for(BotModule::Sniper),
            )
            .await,
        ),
    )
        .into_response()
}

/// `POST /api/tenant/sniper/controls`
pub async fn controls(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Response {
    let action = match ControlAction::parse(&body) {
        Ok(value) => value,
        Err(response) => return response,
    };
    let permission = match action {
        ControlAction::Enable => Permission::BotStart,
        ControlAction::Disable { .. } => Permission::BotStop,
    };
    let auth = match guard_manage(&state, &headers, permission, TradingModuleFamily::Sniper).await {
        Ok(value) => value,
        Err(response) => return response,
    };
    apply_control(&state, &auth, BotModule::Sniper, action).await
}

/// `GET /api/tenant/sniper/config`
pub async fn get_config(State(state): State<ApiState>, headers: HeaderMap) -> Response {
    let auth = match guard(
        &state,
        &headers,
        Permission::BotRead,
        TradingModuleFamily::Sniper,
    )
    .await
    {
        Ok(value) => value,
        Err(response) => return response,
    };
    let Some(db) = state.db.as_deref() else {
        return error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "trading_data_plane_unavailable",
            "sniper configuration requires PostgreSQL",
        );
    };
    match read_module(db, auth.organization_id(), "sniper").await {
        Ok(value) => (StatusCode::OK, Json(value)).into_response(),
        Err(error) => {
            tracing::error!(error = %error, "failed to read tenant sniper configuration");
            error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "config_storage_error",
                "sniper configuration could not be loaded",
            )
        }
    }
}

/// `PUT /api/tenant/sniper/config`
pub async fn update_config(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Response {
    let auth = match guard_manage(
        &state,
        &headers,
        Permission::BotConfigure,
        TradingModuleFamily::Sniper,
    )
    .await
    {
        Ok(value) => value,
        Err(response) => return response,
    };
    let patch = match validate_patch(body) {
        Ok(value) => value,
        Err(response) => return response,
    };
    let Some(db) = state.db.as_deref() else {
        return error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "trading_data_plane_unavailable",
            "sniper configuration requires PostgreSQL",
        );
    };
    match write_module(
        db,
        auth.organization_id(),
        "sniper",
        &patch,
        &auth.ctx.actor_label(),
    )
    .await
    {
        Ok(value) => (StatusCode::OK, Json(value)).into_response(),
        Err(error) => {
            tracing::error!(error = %error, "failed to write tenant sniper configuration");
            error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "config_storage_error",
                "sniper configuration could not be saved",
            )
        }
    }
}
