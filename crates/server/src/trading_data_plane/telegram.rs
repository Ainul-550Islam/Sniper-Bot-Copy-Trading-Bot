//! Tenant Telegram binding/status API (§J, spec file 77).
//!
//! Telegram is the CONTROL PLANE: an authenticated tenant always has it
//! and it never trades by itself (`bot_core::tenant::feature_key` maps
//! it to no plan feature). This surface gives the tenant:
//!
//! * `GET /api/tenant/telegram/status` — module status (control plane)
//!   plus the organization's notification binding;
//! * `PUT /api/tenant/telegram/binding` — bind/replace the
//!   organization's notification chat (a chat id, never a secret);
//! * `DELETE /api/tenant/telegram/binding` — clear the binding.
//!
//! HONESTY NOTE (stated in the status payload too): the deployment-level
//! alert forwarder (`module-telegram`) currently routes alerts to the
//! deployment's configured alert chat; per-tenant routing that consumes
//! this binding is a deployment-side integration step. The binding API
//! records the tenant's declared notification target — it does not
//! claim per-tenant delivery is already active.
//!
//! The binding store follows the established tenant-scoped store pattern
//! (`saas/custody.rs`): keyed by organization, process-local, with every
//! access resolved from the authenticated context — a tenant can only
//! ever read or write THEIR OWN binding.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use chrono::{DateTime, Utc};
use serde_json::json;

use bot_core::membership::Permission;
use bot_core::models::BotModule;
use bot_core::tenant::OrganizationId;

use super::authorization_chain::{guard, guard_manage, TradingModuleFamily};
use crate::api::ApiState;

/// A tenant's Telegram notification binding. The chat id is a public
/// Telegram identifier, not a secret; the bot token (the actual secret)
/// stays in the operator environment and NEVER appears here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TelegramBinding {
    pub chat_id: i64,
    pub bound_at: DateTime<Utc>,
    /// Non-secret actor label from the authenticated context.
    pub bound_by: String,
}

fn bindings_store() -> &'static Mutex<HashMap<OrganizationId, TelegramBinding>> {
    static S: OnceLock<Mutex<HashMap<OrganizationId, TelegramBinding>>> = OnceLock::new();
    S.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Read THIS organization's binding (tenant-scoped).
pub fn binding_for(org: OrganizationId) -> Option<TelegramBinding> {
    bindings_store()
        .lock()
        .expect("bindings mutex")
        .get(&org)
        .cloned()
}

/// Set THIS organization's binding (upsert).
pub fn set_binding(
    org: OrganizationId,
    chat_id: i64,
    bound_by: &str,
    now: DateTime<Utc>,
) -> TelegramBinding {
    let binding = TelegramBinding {
        chat_id,
        bound_at: now,
        bound_by: bound_by.to_string(),
    };
    bindings_store()
        .lock()
        .expect("bindings mutex")
        .insert(org, binding.clone());
    binding
}

/// Clear THIS organization's binding. Returns true when one existed.
pub fn clear_binding(org: OrganizationId) -> bool {
    bindings_store()
        .lock()
        .expect("bindings mutex")
        .remove(&org)
        .is_some()
}

/// Validate a Telegram chat id (Telegram chat ids are i64; positive for
/// users/groups, negative for supergroups/channel style ids — both are
/// legitimate, so the only invalid value is a non-number).
#[allow(clippy::result_large_err)]
fn parse_chat_id(body: &serde_json::Value) -> Result<i64, Response> {
    let raw = match body.get("chat_id").and_then(|v| v.as_i64()) {
        Some(v) => v,
        None => {
            return Err((
                StatusCode::BAD_REQUEST,
                Json(json!({
                    "error": "invalid_chat_id",
                    "detail": "chat_id must be an integer Telegram chat id",
                })),
            )
                .into_response())
        }
    };
    Ok(raw)
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
        Ok(a) => a,
        Err(response) => return response,
    };
    let org = auth.organization_id();
    let runtime = state
        .module_registry
        .get(org, BotModule::Telegram)
        .map(|h| {
            json!({
                "runtime_id": h.runtime_id().to_string(),
                "generation": h.generation().to_string(),
                "phase": h.phase().as_str(),
                "since": h.since().to_rfc3339(),
            })
        });
    let binding = binding_for(org);
    (
        StatusCode::OK,
        Json(json!({
            "organization_id": org.to_string(),
            "module": "telegram",
            "entitlement": {
                "feature": serde_json::Value::Null,
                "granted": true,
                "note": "telegram is the control plane — every authenticated tenant has it; it never trades",
            },
            "effective_state": "enabled",
            "runtime": runtime.unwrap_or(serde_json::Value::Null),
            "runtime_detail": if state.module_registry.get(org, BotModule::Telegram).is_some() {
                serde_json::Value::Null
            } else {
                json!("no runtime registered for this module and organization (absence is not an error)")
            },
            "binding": binding.map(|b| json!({
                "chat_id": b.chat_id,
                "bound_at": b.bound_at.to_rfc3339(),
                "bound_by": b.bound_by,
            })).unwrap_or(serde_json::Value::Null),
            "binding_detail": json!({
                "purpose": "records this organization's declared Telegram notification chat",
                "delivery": "the deployment-level alert forwarder currently routes to the deployment alert chat; per-tenant routing is a deployment-side integration step",
            }),
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
    Json(body): Json<serde_json::Value>,
) -> Response {
    let chat_id = match parse_chat_id(&body) {
        Ok(v) => v,
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
        Ok(a) => a,
        Err(response) => return response,
    };
    let org = auth.organization_id();
    let actor = auth.ctx.actor_label();
    let binding = set_binding(org, chat_id, &actor, Utc::now());
    (
        StatusCode::OK,
        Json(json!({
            "organization_id": org.to_string(),
            "binding": {
                "chat_id": binding.chat_id,
                "bound_at": binding.bound_at.to_rfc3339(),
                "bound_by": binding.bound_by,
            },
            "detail": "binding recorded for your organization; the chat id is a public Telegram identifier and no token material is stored",
        })),
    )
        .into_response()
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
        Ok(a) => a,
        Err(response) => return response,
    };
    let org = auth.organization_id();
    if clear_binding(org) {
        (
            StatusCode::OK,
            Json(json!({
                "organization_id": org.to_string(),
                "binding": serde_json::Value::Null,
                "detail": "binding cleared",
            })),
        )
            .into_response()
    } else {
        (
            StatusCode::OK,
            Json(json!({
                "organization_id": org.to_string(),
                "binding": serde_json::Value::Null,
                "detail": "no binding was present",
            })),
        )
            .into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bindings_are_tenant_scoped() {
        let a = OrganizationId::new();
        let b = OrganizationId::new();
        set_binding(a, 123456, "user:a", Utc::now());
        assert!(binding_for(b).is_none());
        assert_eq!(binding_for(a).unwrap().chat_id, 123456);
        // Upsert replaces.
        set_binding(a, 987654, "user:a2", Utc::now());
        assert_eq!(binding_for(a).unwrap().chat_id, 987654);
        assert_eq!(binding_for(a).unwrap().bound_by, "user:a2");
        // Clear is scoped and idempotent.
        assert!(clear_binding(a));
        assert!(!clear_binding(a));
        assert!(binding_for(a).is_none());
        assert!(binding_for(b).is_none());
    }

    #[test]
    fn chat_ids_parse_strictly() {
        assert_eq!(
            parse_chat_id(&serde_json::json!({"chat_id": -1002003004005i64})).ok(),
            Some(-1002003004005i64)
        );
        assert_eq!(
            parse_chat_id(&serde_json::json!({"chat_id": 42i64})).ok(),
            Some(42)
        );
        assert!(parse_chat_id(&serde_json::json!({"chat_id": "123"})).is_err());
        assert!(parse_chat_id(&serde_json::json!({"chat_id": 12.5})).is_err());
        assert!(parse_chat_id(&serde_json::json!({})).is_err());
    }

    #[test]
    fn telegram_family_is_the_control_plane() {
        // No plan feature gates telegram — it is satisfied by
        // authentication itself (the chain's control-plane carve-out).
        assert_eq!(
            TradingModuleFamily::Telegram.satisfying_features(),
            &[] as &[&str]
        );
        assert_eq!(bot_core::tenant::feature_key(BotModule::Telegram), None);
    }
}
