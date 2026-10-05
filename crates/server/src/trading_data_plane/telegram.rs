//! Tenant Telegram binding and status API.
//!
//! The binding is durable tenant configuration. It records only a public chat
//! identifier and an audit actor; bot tokens and delivery credentials remain in
//! deployment secret storage and are never accepted by this API.

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};

use bot_core::membership::Permission;
use bot_core::models::BotModule;

use super::authorization_chain::{guard, guard_manage, TradingModuleFamily};
use super::config_store::{clear_module, read_module, write_module};
use crate::api::ApiState;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TelegramBinding {
    pub chat_id: i64,
    pub bound_at: DateTime<Utc>,
    pub bound_by: String,
}

fn error_response(status: StatusCode, error: &'static str, detail: impl Into<String>) -> Response {
    (
        status,
        Json(json!({ "error": error, "detail": detail.into() })),
    )
        .into_response()
}

async fn binding_for(
    state: &ApiState,
    organization_id: bot_core::tenant::OrganizationId,
) -> Result<Option<TelegramBinding>, Response> {
    let Some(db) = state.db.as_deref() else {
        return Err(error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "trading_data_plane_unavailable",
            "Telegram binding requires PostgreSQL",
        ));
    };
    let value = read_module(db, organization_id, "telegram")
        .await
        .map_err(|error| {
            tracing::error!(error = %error, "failed to read tenant Telegram binding");
            error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "config_storage_error",
                "Telegram binding could not be loaded",
            )
        })?;
    value
        .map(|item| {
            serde_json::from_value(item).map_err(|error| {
                tracing::error!(error = %error, "tenant Telegram binding has invalid durable data");
                error_response(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "config_storage_error",
                    "Telegram binding data is invalid",
                )
            })
        })
        .transpose()
}

/// Validate a Telegram chat id. Telegram user/group/channel identifiers are
/// signed 64-bit integers, so negative values are valid.
fn parse_chat_id(body: &Value) -> Result<i64, Response> {
    let Some(object) = body.as_object() else {
        return Err(error_response(
            StatusCode::BAD_REQUEST,
            "invalid_chat_id",
            "request body must be a JSON object",
        ));
    };
    let Some(value) = object.get("chat_id") else {
        return Err(error_response(
            StatusCode::BAD_REQUEST,
            "invalid_chat_id",
            "chat_id is required",
        ));
    };
    let Some(chat_id) = value.as_i64() else {
        return Err(error_response(
            StatusCode::BAD_REQUEST,
            "invalid_chat_id",
            "chat_id must be an integer Telegram chat id",
        ));
    };
    Ok(chat_id)
}

/// `GET /api/tenant/telegram/status`
pub async fn status(State(state): State<ApiState>, headers: HeaderMap) -> Response {
    let auth = match guard(
        &state,
        &headers,
        Permission::BotRead,
        TradingModuleFamily::Telegram,
    )
    .await
    {
        Ok(value) => value,
        Err(response) => return response,
    };
    let organization_id = auth.organization_id();
    let binding = match binding_for(&state, organization_id).await {
        Ok(value) => value,
        Err(response) => return response,
    };
    let runtime = state
        .module_registry
        .get(organization_id, BotModule::Telegram)
        .map(|handle| {
            json!({
                "runtime_id": handle.runtime_id().to_string(),
                "generation": handle.generation().to_string(),
                "phase": handle.phase().as_str(),
                "since": handle.since().to_rfc3339(),
            })
        });
    (
        StatusCode::OK,
        Json(json!({
            "organization_id": organization_id.to_string(),
            "module": "telegram",
            "entitlement": {
                "feature": Value::Null,
                "granted": true,
                "note": "telegram is the control plane; it never trades",
            },
            "effective_state": "enabled",
            "runtime": runtime.unwrap_or(Value::Null),
            "runtime_detail": if state.module_registry.get(organization_id, BotModule::Telegram).is_some() {
                Value::Null
            } else {
                json!("no runtime registered for this module and organization")
            },
            "binding": binding,
            "binding_detail": {
                "purpose": "records this organization's declared Telegram notification chat",
                "delivery": "delivery health is not claimed by this binding endpoint",
            },
            "controls": {
                "available": true,
                "actions": ["bind", "unbind"],
            },
        })),
    )
        .into_response()
}

/// `PUT /api/tenant/telegram/binding` — body `{"chat_id": <integer>}`.
pub async fn bind(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Response {
    let chat_id = match parse_chat_id(&body) {
        Ok(value) => value,
        Err(response) => return response,
    };
    let auth = match guard_manage(
        &state,
        &headers,
        Permission::TenantUpdate,
        TradingModuleFamily::Telegram,
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
            "Telegram binding requires PostgreSQL",
        );
    };
    let mut patch = Map::new();
    patch.insert("chat_id".to_string(), json!(chat_id));
    patch.insert("bound_at".to_string(), json!(Utc::now().to_rfc3339()));
    patch.insert("bound_by".to_string(), json!(auth.ctx.actor_label()));
    match write_module(db, auth.organization_id(), "telegram", &patch, &auth.ctx.actor_label()).await {
        Ok(binding) => (StatusCode::OK, Json(json!({ "organization_id": auth.organization_id().to_string(), "binding": binding }))).into_response(),
        Err(error) => {
            tracing::error!(error = %error, "failed to persist tenant Telegram binding");
            error_response(StatusCode::INTERNAL_SERVER_ERROR, "config_storage_error", "Telegram binding could not be saved")
        }
    }
}

/// `DELETE /api/tenant/telegram/binding`
pub async fn unbind(State(state): State<ApiState>, headers: HeaderMap) -> Response {
    let auth = match guard_manage(
        &state,
        &headers,
        Permission::TenantUpdate,
        TradingModuleFamily::Telegram,
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
            "Telegram binding requires PostgreSQL",
        );
    };
    match clear_module(db, auth.organization_id(), "telegram", &auth.ctx.actor_label()).await {
        Ok(_) => (
            StatusCode::OK,
            Json(json!({ "organization_id": auth.organization_id().to_string(), "binding": Value::Null })),
        )
            .into_response(),
        Err(error) => {
            tracing::error!(error = %error, "failed to clear tenant Telegram binding");
            error_response(StatusCode::INTERNAL_SERVER_ERROR, "config_storage_error", "Telegram binding could not be cleared")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chat_ids_parse_strictly() {
        assert_eq!(
            parse_chat_id(&json!({ "chat_id": -1002003004005_i64 })).unwrap(),
            -1002003004005_i64
        );
        assert_eq!(
            parse_chat_id(&json!({ "chat_id": 42_i64 })).unwrap(),
            42_i64
        );
        assert!(parse_chat_id(&json!({ "chat_id": "123" })).is_err());
        assert!(parse_chat_id(&json!({ "chat_id": 12.5 })).is_err());
        assert!(parse_chat_id(&json!({})).is_err());
    }
}
