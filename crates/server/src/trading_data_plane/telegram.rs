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
    /// The Telegram user allowed to act in this chat (Batch 07). A chat id
    /// alone is not an identity: without a user id the binding stays a
    /// notification address and the trade console never links the chat.
    /// `#[serde(default)]` keeps pre-Batch-07 durable documents readable.
    #[serde(default)]
    pub telegram_user_id: Option<i64>,
    /// Whether the linked chat may use the consent-gated trade console
    /// (Batch 07). Only meaningful together with `telegram_user_id`; the
    /// bind handler refuses `true` without one.
    #[serde(default)]
    pub trading_enabled: bool,
}

fn error_response(status: StatusCode, error: &'static str, detail: impl Into<String>) -> Response {
    (
        status,
        Json(json!({ "error": error, "detail": detail.into() })),
    )
        .into_response()
}

/// Load the durable Telegram binding for one organization.
///
/// Shared by the HTTP handlers and by the server boot path, which mirrors
/// the deployment organization's binding into the trade console's chat
/// link (Batch 07). A malformed durable document is a hard error, never a
/// silently absent binding.
pub async fn load_binding(
    db: &bot_core::db::Database,
    organization_id: bot_core::tenant::OrganizationId,
) -> Result<Option<TelegramBinding>, sqlx::Error> {
    let value = read_module(db, organization_id, "telegram").await?;
    value
        .map(|item| {
            serde_json::from_value(item).map_err(|error| {
                sqlx::Error::Protocol(format!("telegram binding document is invalid: {error}"))
            })
        })
        .transpose()
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
    load_binding(db, organization_id).await.map_err(|error| {
        tracing::error!(error = %error, "failed to read tenant Telegram binding");
        error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "config_storage_error",
            "Telegram binding could not be loaded",
        )
    })
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

/// Parse the optional Batch 07 trading-link fields of a bind request.
///
/// Fail-closed: a present but malformed field is a 400, and
/// `trading_enabled: true` without a `telegram_user_id` is refused — the
/// console must never link a trading chat without knowing WHICH Telegram
/// user may act in it.
fn parse_trading_link(body: &Value) -> Result<(Option<i64>, bool), Response> {
    let Some(object) = body.as_object() else {
        return Err(error_response(
            StatusCode::BAD_REQUEST,
            "invalid_trading_link",
            "request body must be a JSON object",
        ));
    };
    let telegram_user_id = match object.get("telegram_user_id") {
        None | Some(Value::Null) => None,
        Some(value) => match value.as_i64() {
            Some(id) if id > 0 => Some(id),
            _ => {
                return Err(error_response(
                    StatusCode::BAD_REQUEST,
                    "invalid_telegram_user_id",
                    "telegram_user_id must be a positive integer Telegram user id",
                ))
            }
        },
    };
    let trading_enabled = match object.get("trading_enabled") {
        None | Some(Value::Null) => false,
        Some(value) => match value.as_bool() {
            Some(flag) => flag,
            None => {
                return Err(error_response(
                    StatusCode::BAD_REQUEST,
                    "invalid_trading_enabled",
                    "trading_enabled must be a boolean",
                ))
            }
        },
    };
    if trading_enabled && telegram_user_id.is_none() {
        return Err(error_response(
            StatusCode::BAD_REQUEST,
            "trading_requires_user",
            "trading_enabled requires telegram_user_id: a chat id alone is not an identity",
        ));
    }
    Ok((telegram_user_id, trading_enabled))
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
                "note": "telegram is the control plane; chat trading exists only for the deployment organization, behind in-chat consent and operator caps",
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
                "purpose": "records this organization's declared Telegram notification chat and, when telegram_user_id is set, the user the trade console may link to it",
                "delivery": "delivery health is not claimed by this binding endpoint",
                "trading": "trading_enabled on the binding only allows the deployment organization's console to link the chat; every order still needs an in-chat consent that this API can view and revoke but never grant",
            },
            "controls": {
                "available": true,
                "actions": ["bind", "unbind", "consent_view", "consent_revoke"],
            },
        })),
    )
        .into_response()
}

/// `PUT /api/tenant/telegram/binding` — body `{"chat_id": <integer>}` plus
/// the optional Batch 07 trading link (`telegram_user_id`,
/// `trading_enabled`). A rebind always rewrites the trading-link fields
/// (absent = cleared), so rebinding to a new chat can never inherit the
/// previous chat's trading permission.
pub async fn bind(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Response {
    let chat_id = match parse_chat_id(&body) {
        Ok(value) => value,
        Err(response) => return response,
    };
    let (telegram_user_id, trading_enabled) = match parse_trading_link(&body) {
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
    patch.insert("telegram_user_id".to_string(), json!(telegram_user_id));
    patch.insert("trading_enabled".to_string(), json!(trading_enabled));
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

/// Public projection of a consent row for the admin surface.
///
/// Deliberately excludes `phrase_sha256`: the digest proves the operator
/// echoed the phrase in chat, and nothing on an HTTP response needs it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ConsentView {
    pub consent_id: String,
    pub chat_id: i64,
    pub user_id: i64,
    pub scope: Vec<String>,
    pub max_order_lamports: u64,
    pub daily_cap_lamports: u64,
    pub status: String,
    pub granted_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub revoked_at: Option<DateTime<Utc>>,
}

fn consent_view(consent: &module_telegram::Consent, now: DateTime<Utc>) -> ConsentView {
    ConsentView {
        consent_id: consent.consent_id.to_string(),
        chat_id: consent.chat_id,
        user_id: consent.user_id,
        scope: consent
            .scope
            .labels()
            .into_iter()
            .map(str::to_string)
            .collect(),
        max_order_lamports: consent.max_order_lamports,
        daily_cap_lamports: consent.daily_cap_lamports,
        status: consent.status(now).as_str().to_string(),
        granted_at: consent.granted_at,
        expires_at: consent.expires_at,
        revoked_at: consent.revoked_at,
    }
}

fn consent_store_error(error: &module_telegram::consent::store::StoreError) -> Response {
    use module_telegram::consent::store::StoreError;
    tracing::error!(error = %error.detail(), code = error.code(), "telegram consent store failed");
    match error {
        StoreError::Unavailable(_) => error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "consent_store_unavailable",
            "the consent store is unavailable (database or trade tables missing)",
        ),
        StoreError::Conflict(_) => error_response(
            StatusCode::CONFLICT,
            "consent_store_conflict",
            "the consent store reported a consistency conflict",
        ),
    }
}

/// The bound chat of the caller's organization, or the response to return.
/// Consent administration is meaningless without a binding: consents are
/// per-chat, and this surface only ever addresses the caller's own chat.
async fn bound_chat(
    state: &ApiState,
    organization_id: bot_core::tenant::OrganizationId,
) -> Result<i64, Response> {
    match binding_for(state, organization_id).await? {
        Some(binding) => Ok(binding.chat_id),
        None => Err(error_response(
            StatusCode::NOT_FOUND,
            "telegram_not_bound",
            "this organization has no Telegram chat binding",
        )),
    }
}

/// `GET /api/tenant/telegram/consent` — view the live consent, if any.
///
/// Read-only by design. Granting stays in chat (the phrase echo is the
/// proof of intent); the control plane can only look and revoke.
pub async fn consent(State(state): State<ApiState>, headers: HeaderMap) -> Response {
    use module_telegram::consent::store::ConsentStore as _;
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
    let chat_id = match bound_chat(&state, organization_id).await {
        Ok(value) => value,
        Err(response) => return response,
    };
    let Some(db) = state.db.as_deref() else {
        return error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "trading_data_plane_unavailable",
            "Telegram consent requires PostgreSQL",
        );
    };
    let now = Utc::now();
    let store = crate::telegram_trade_store_pg::PgTradeStore::new(db.pool().clone());
    match store
        .consents()
        .active(&organization_id.to_string(), chat_id, now)
        .await
    {
        Ok(consent) => (
            StatusCode::OK,
            Json(json!({
                "organization_id": organization_id.to_string(),
                "chat_id": chat_id,
                "consent": consent.as_ref().map(|c| consent_view(c, now)),
                "grant": "in-chat only: use /consent in the bound Telegram chat",
            })),
        )
            .into_response(),
        Err(error) => consent_store_error(&error),
    }
}

/// `POST /api/tenant/telegram/consent/revoke` — revoke the live consent
/// and cancel every pending command of the chat.
///
/// Revocation is the one mutation this surface offers, and it only ever
/// subtracts authority. Cancelling pending commands closes the race where
/// a confirmation issued before the revoke could still execute after it.
pub async fn consent_revoke(State(state): State<ApiState>, headers: HeaderMap) -> Response {
    use module_telegram::commands::pending::CommandStore as _;
    use module_telegram::consent::store::ConsentStore as _;
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
    let organization_id = auth.organization_id();
    let chat_id = match bound_chat(&state, organization_id).await {
        Ok(value) => value,
        Err(response) => return response,
    };
    let Some(db) = state.db.as_deref() else {
        return error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "trading_data_plane_unavailable",
            "Telegram consent requires PostgreSQL",
        );
    };
    let now = Utc::now();
    let org = organization_id.to_string();
    let store = crate::telegram_trade_store_pg::PgTradeStore::new(db.pool().clone());
    let revoked = match store.consents().revoke(&org, chat_id, now).await {
        Ok(value) => value,
        Err(error) => return consent_store_error(&error),
    };
    let cancelled = match store.commands().cancel_pending(&org, chat_id, now).await {
        Ok(value) => value,
        Err(error) => return consent_store_error(&error),
    };
    (
        StatusCode::OK,
        Json(json!({
            "organization_id": org,
            "chat_id": chat_id,
            "revoked": revoked.as_ref().map(|c| consent_view(c, now)),
            "cancelled_pending": cancelled.iter().map(|id| id.to_string()).collect::<Vec<_>>(),
        })),
    )
        .into_response()
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

    #[test]
    fn trading_link_defaults_to_alerts_only_without_user() {
        assert_eq!(parse_trading_link(&json!({})).unwrap(), (None, false));
        assert_eq!(
            parse_trading_link(&json!({ "telegram_user_id": 777_i64 })).unwrap(),
            (Some(777), false)
        );
        assert_eq!(
            parse_trading_link(&json!({
                "telegram_user_id": 777_i64,
                "trading_enabled": true,
            }))
            .unwrap(),
            (Some(777), true)
        );
    }

    #[test]
    fn trading_link_rejects_malformed_and_userless_trading() {
        // Trading without an identified user is the one combination that
        // must never persist: a chat id alone is not an identity.
        assert!(parse_trading_link(&json!({ "trading_enabled": true })).is_err());
        assert!(parse_trading_link(&json!({ "telegram_user_id": 0_i64 })).is_err());
        assert!(parse_trading_link(&json!({ "telegram_user_id": -5_i64 })).is_err());
        assert!(parse_trading_link(&json!({ "telegram_user_id": "777" })).is_err());
        assert!(parse_trading_link(&json!({
            "telegram_user_id": 777_i64,
            "trading_enabled": "yes",
        }))
        .is_err());
    }

    #[test]
    fn legacy_binding_documents_still_decode() {
        // Bindings written before Batch 07 have no trading-link fields;
        // serde defaults must keep them loadable (and non-trading).
        let binding: TelegramBinding = serde_json::from_value(json!({
            "chat_id": -99_i64,
            "bound_at": "2026-01-01T00:00:00Z",
            "bound_by": "operator",
        }))
        .unwrap();
        assert_eq!(binding.chat_id, -99);
        assert_eq!(binding.telegram_user_id, None);
        assert!(!binding.trading_enabled);
    }

    #[test]
    fn consent_view_hides_the_phrase_digest_and_derives_status() {
        let granted = DateTime::parse_from_rfc3339("2026-01-01T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let consent = module_telegram::Consent {
            consent_id: uuid::Uuid::nil(),
            organization_id: "org".to_string(),
            user_id: 777,
            chat_id: -99,
            scope: module_telegram::ConsentScope::both(),
            max_order_lamports: 5_000,
            daily_cap_lamports: 50_000,
            granted_at: granted,
            expires_at: granted + chrono::Duration::hours(1),
            revoked_at: None,
            phrase_sha256: [7_u8; 32],
        };
        let view = consent_view(&consent, granted + chrono::Duration::minutes(5));
        assert_eq!(view.status, "active");
        assert_eq!(view.scope, vec!["buy".to_string(), "sell".to_string()]);
        let serialized = serde_json::to_value(&view).unwrap();
        let object = serialized.as_object().unwrap();
        assert!(
            !object.contains_key("phrase_sha256"),
            "the admin surface must never expose the phrase digest"
        );
        let expired = consent_view(&consent, granted + chrono::Duration::hours(2));
        assert_eq!(expired.status, "expired");
    }
}
