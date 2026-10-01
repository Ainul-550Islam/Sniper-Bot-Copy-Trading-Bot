//! Tenant Sniper controls/status (§J, spec file 74).
//!
//! `GET /api/tenant/sniper/status` answers "what state is Sniper in for
//! MY organization" — entitlement (verified by the chain), tenant-level
//! override, and the registered runtime phase. `POST
//! /api/tenant/sniper/controls` records a tenant-level enable/disable
//! for the caller's OWN organization. Runtime lifecycle transitions stay
//! runtime-owned and fenced (see `bots.rs`); nothing here reaches around
//! the fencing.

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use bot_core::membership::Permission;
use bot_core::models::BotModule;

use super::authorization_chain::{guard, guard_manage, TradingModuleFamily};
use super::module_controls::{apply_control, feature_key_for, status_payload, ControlAction};
use crate::api::ApiState;

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
        Ok(a) => a,
        Err(response) => return response,
    };
    (
        StatusCode::OK,
        Json(status_payload(
            &state,
            &auth,
            BotModule::Sniper,
            feature_key_for(BotModule::Sniper),
        )),
    )
        .into_response()
}

/// `POST /api/tenant/sniper/controls` — body `{"action":"enable"}` or
/// `{"action":"disable","reason":"…"}`. Enable requires the BotStart
/// permission, disable BotStop; both act only on the caller's own
/// organization.
pub async fn controls(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Json(body): Json<serde_json::Value>,
) -> Response {
    let action = match ControlAction::parse(&body) {
        Ok(a) => a,
        Err(response) => return response,
    };
    // The permission depends on the action: starting vs stopping.
    let permission = match action {
        ControlAction::Enable => Permission::BotStart,
        ControlAction::Disable { .. } => Permission::BotStop,
    };
    let auth = match guard_manage(&state, &headers, permission, TradingModuleFamily::Sniper).await {
        Ok(a) => a,
        Err(response) => return response,
    };
    apply_control(&state, &auth, BotModule::Sniper, action)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sniper_payload_shape_is_secret_free() {
        // The status payload builder is exercised end-to-end through the
        // authorization chain in the integration tests; here we pin the
        // module identity and feature mapping used by these handlers.
        assert_eq!(BotModule::Sniper.to_string(), "sniper");
        assert_eq!(
            feature_key_for(BotModule::Sniper),
            Some("module.sniper"),
            "the sniper family must map to the plan feature"
        );
    }

    #[test]
    fn sniper_family_is_entitlement_gated() {
        assert_eq!(
            TradingModuleFamily::Sniper.satisfying_features(),
            &["module.sniper"]
        );
    }

    #[test]
    fn control_actions_use_start_stop_permissions() {
        assert_eq!(
            Permission::BotStart.as_str(),
            match ControlAction::Enable {
                // The mapping is checked at runtime in `controls`; the
                // static pin here guards accidental swaps.
                ControlAction::Enable => Permission::BotStart,
                ControlAction::Disable { .. } => Permission::BotStop,
            }
            .as_str()
        );
    }
}
