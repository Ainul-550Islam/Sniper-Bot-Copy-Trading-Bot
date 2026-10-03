//! Tenant module controls — the shared engine behind the per-module
//! controls/status surfaces (§J, spec files 74–77).
//!
//! What a customer "controls" here is the TENANT-LEVEL enablement
//! override for THEIR OWN organization: a deliberate disable (pause) or
//! re-enable of a module they are entitled to. It is NOT runtime
//! lifecycle control — module start/stop transitions remain runtime-owned
//! and fenced (see `bots.rs`); this surface records tenant intent that
//! the entitlement chain and the runtime honor at their next evaluation.
//!
//! Invariants:
//!
//! * the organization is ALWAYS taken from the authenticated context —
//!   a client can never name another tenant's controls (the store is
//!   keyed by `(OrganizationId, ModuleKind)` and every read/write goes
//!   through the guard-resolved org);
//! * absence of an override is the honest "no tenant override": the
//!   effective state is then governed by the deployment entitlement
//!   (which the authorization chain has already checked);
//! * a disable carries a reason; an enable clears the override — both
//!   record who did it (actor label, never a secret);
//! * the state is DURABLE and shared across replicas: PostgreSQL
//!   (`tenant_module_controls`, migration 0036) is the authority and
//!   [`crate::trading_data_plane::module_control_store`] owns the access.
//!   Before 0036 this map was process-local, which meant a tenant pausing
//!   a module paused it on ONE replica and a restart silently re-enabled
//!   it. A deployment with no database attached still runs the in-process
//!   mode, and `status_payload` reports that honestly via `durable`.

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use bot_core::models::BotModule;
use bot_core::tenant::OrganizationId;
use chrono::Utc;
use serde_json::json;

use crate::api::ApiState;
use crate::trading_data_plane::authorization_chain::AuthorizedTradingPlane;

pub use crate::trading_data_plane::module_control_store::{
    ModuleControlOverride, ModuleControlStore,
};

/// Read THIS organization's override for a module (tenant-scoped).
pub async fn override_for(
    state: &ApiState,
    org: OrganizationId,
    module: BotModule,
) -> Option<ModuleControlOverride> {
    state.module_controls().override_for(org, module).await
}

/// The effective module state from the tenant's perspective: a disable
/// override wins; everything else is the entitlement default (which the
/// guard has already established as granted before a handler runs).
pub async fn effective_state(
    state: &ApiState,
    org: OrganizationId,
    module: BotModule,
) -> (&'static str, Option<ModuleControlOverride>) {
    state.module_controls().effective_state(org, module).await
}

/// Assemble the module status payload shared by every per-module status
/// handler. `feature` is the entitlement feature the family maps to
/// (reported as granted — the guard refused otherwise).
pub async fn status_payload(
    state: &ApiState,
    auth: &AuthorizedTradingPlane,
    module: BotModule,
    feature: Option<&'static str>,
) -> serde_json::Value {
    let org = auth.organization_id();
    let controls = state.module_controls();
    let (effective, override_entry) = controls.effective_state(org, module).await;
    let runtime = state.module_registry.get(org, module).map(|h| {
        json!({
            "runtime_id": h.runtime_id().to_string(),
            "generation": h.generation().to_string(),
            "phase": h.phase().as_str(),
            "since": h.since().to_rfc3339(),
        })
    });
    json!({
        "organization_id": org.to_string(),
        "module": module.to_string(),
        "entitlement": {
            "feature": feature,
            "granted": true,
            "note": "verified by the customer-API authorization chain for this request",
        },
        "tenant_override": override_entry.map(|o| json!({
            "state": if o.enabled { "enabled" } else { "disabled" },
            "reason": o.reason,
            "updated_at": o.updated_at.to_rfc3339(),
            "updated_by": o.updated_by,
            "version": o.version,
        })).unwrap_or(serde_json::Value::Null),
        "effective_state": effective,
        "runtime": runtime.unwrap_or(serde_json::Value::Null),
        "runtime_detail": if state.module_registry.get(org, module).is_some() {
            serde_json::Value::Null
        } else {
            json!("no runtime registered for this module and organization (absence is not an error)")
        },
        "controls": {
            "available": true,
            "actions": ["enable", "disable"],
            // Honest durability reporting: `false` means this deployment
            // has no database attached, so the override is process-local
            // and does NOT survive a restart or reach another replica.
            "durable": controls.is_durable(),
            "scope": "tenant-level enablement of your own organization; runtime lifecycle transitions remain runtime-owned and fenced",
        },
    })
}

/// The request body of a control action.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ControlAction {
    Enable,
    Disable { reason: String },
}

impl ControlAction {
    /// Parse and validate the action JSON. Strict: unknown actions,
    /// non-string reasons, or a reason on an enable are refused.
    #[allow(clippy::result_large_err)]
    pub fn parse(body: &serde_json::Value) -> Result<Self, Response> {
        let action = body
            .get("action")
            .and_then(|v| v.as_str())
            .map(|s| s.trim().to_ascii_lowercase());
        let reason = body
            .get("reason")
            .and_then(|v| v.as_str())
            .map(|s| s.trim().to_string());
        match (action.as_deref(), reason) {
            (Some("enable"), None) => Ok(ControlAction::Enable),
            (Some("enable"), Some(r)) if r.is_empty() => Ok(ControlAction::Enable),
            (Some("disable"), reason) => {
                let reason = reason.unwrap_or_default();
                if reason.len() > 280 {
                    return Err((
                        StatusCode::BAD_REQUEST,
                        Json(json!({"error": "reason_too_long", "max": 280})),
                    )
                        .into_response());
                }
                Ok(ControlAction::Disable { reason })
            }
            (Some(other), _) => Err((
                StatusCode::BAD_REQUEST,
                Json(json!({
                    "error": "unknown_action",
                    "detail": format!("'{other}' is not a valid action; use 'enable' or 'disable'"),
                })),
            )
                .into_response()),
            (None, _) => Err((
                StatusCode::BAD_REQUEST,
                Json(json!({"error": "missing_action", "detail": "body must carry {\"action\": \"enable\"|\"disable\"}"})),
            )
                .into_response()),
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            ControlAction::Enable => "enable",
            ControlAction::Disable { .. } => "disable",
        }
    }
}

/// Apply a parsed control action for the authenticated organization.
/// The response body is the new status of the module.
pub async fn apply_control(
    state: &ApiState,
    auth: &AuthorizedTradingPlane,
    module: BotModule,
    action: ControlAction,
) -> Response {
    let org = auth.organization_id();
    let actor = auth.ctx.actor_label();
    let correlation = auth.ctx.correlation_label();
    let now = Utc::now();
    let controls = state.module_controls();
    // Both arms FAIL CLOSED. A tenant must never be told "module paused"
    // when the pause did not reach the authoritative store — that is the
    // exact failure mode the in-memory kill-switch had on every replica
    // but one.
    let applied = match action {
        ControlAction::Enable => {
            // Enable clears any disable override — the module returns to
            // its entitlement-governed default.
            if let Err(e) = controls.clear_override(org, module).await {
                return control_store_unavailable(module, "enable", &e);
            }
            json!({
                "action": "enable",
                "applied": true,
                "detail": "tenant override cleared; module follows the entitlement-governed default",
                "updated_at": now.to_rfc3339(),
                "updated_by": actor,
            })
        }
        ControlAction::Disable { reason } => {
            let entry = match controls
                .apply_override(org, module, false, &reason, &actor, &correlation, now)
                .await
            {
                Ok(entry) => entry,
                Err(e) => return control_store_unavailable(module, "disable", &e),
            };
            json!({
                "action": "disable",
                "applied": true,
                "detail": "tenant-level disable recorded; the runtime honors it at its next module evaluation (runtime lifecycle stays runtime-owned)",
                "reason": entry.reason,
                "updated_at": entry.updated_at.to_rfc3339(),
                "updated_by": entry.updated_by,
                "version": entry.version,
            })
        }
    };
    let mut status = status_payload(state, auth, module, feature_key_for(module)).await;
    if let Some(obj) = status.as_object_mut() {
        obj.insert("control_result".to_string(), applied);
    }
    (StatusCode::OK, Json(status)).into_response()
}

/// The refusal returned when the authoritative control store could not
/// be written. 503, never 200: the caller must not believe a control
/// action took effect when it did not.
fn control_store_unavailable(
    module: BotModule,
    action: &'static str,
    error: &crate::trading_data_plane::module_control_store::ControlStoreError,
) -> Response {
    tracing::error!(
        module = module.as_str(),
        action,
        error = %error,
        "module control action refused: authoritative store unavailable"
    );
    (
        StatusCode::SERVICE_UNAVAILABLE,
        Json(json!({
            "error": "module_control_store_unavailable",
            "module": module.to_string(),
            "action": action,
            "applied": false,
            "detail": "the control action was NOT applied: the authoritative control store could not be written. Retry; the module's previous state is unchanged.",
        })),
    )
        .into_response()
}

/// The plan feature key gating a module (from the core tenancy mapping —
/// `None` for the telegram control plane, which every authenticated
/// tenant has).
pub fn feature_key_for(module: BotModule) -> Option<&'static str> {
    bot_core::tenant::feature_key(module)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn org() -> OrganizationId {
        OrganizationId::new()
    }

    /// Tenant scoping and per-module scoping of overrides now live with
    /// the authoritative store; these assert the SAME invariants through
    /// the module's own re-exported surface so a refactor of either side
    /// cannot quietly drop them.
    #[tokio::test]
    async fn overrides_are_tenant_and_module_scoped() {
        crate::trading_data_plane::module_control_store::reset_cache();
        let store = ModuleControlStore::new(None);
        let a = org();
        let b = org();

        store
            .apply_override(
                a,
                BotModule::Sniper,
                false,
                "paused",
                "user:a",
                "corr",
                Utc::now(),
            )
            .await
            .expect("memory write");

        // Org B sees no override and an enabled default.
        assert!(store.override_for(b, BotModule::Sniper).await.is_none());
        assert_eq!(
            store.effective_state(b, BotModule::Sniper).await.0,
            "enabled"
        );
        // Org A sees its own, and only on the module it paused.
        let (state, entry) = store.effective_state(a, BotModule::Sniper).await;
        assert_eq!(state, "disabled");
        assert_eq!(entry.expect("entry").reason, "paused");
        assert_eq!(store.effective_state(a, BotModule::Copy).await.0, "enabled");

        assert!(store
            .clear_override(a, BotModule::Sniper)
            .await
            .expect("clear"));
        assert_eq!(
            store.effective_state(a, BotModule::Sniper).await.0,
            "enabled"
        );
    }

    #[test]
    fn control_actions_parse_strictly() {
        assert_eq!(
            ControlAction::parse(&serde_json::json!({"action": "enable"})).ok(),
            Some(ControlAction::Enable)
        );
        assert_eq!(
            ControlAction::parse(
                &serde_json::json!({"action": " disable ", "reason": "risk review"})
            )
            .ok(),
            Some(ControlAction::Disable {
                reason: "risk review".to_string()
            })
        );
        // Unknown / missing actions are 400s.
        assert!(ControlAction::parse(&serde_json::json!({"action": "detonate"})).is_err());
        assert!(ControlAction::parse(&serde_json::json!({})).is_err());
        assert!(ControlAction::parse(&serde_json::json!({"action": 42})).is_err());
        // Oversized reason is refused.
        let long = "x".repeat(281);
        assert!(
            ControlAction::parse(&serde_json::json!({"action": "disable", "reason": long}))
                .is_err()
        );
    }

    #[test]
    fn feature_keys_come_from_the_core_mapping() {
        assert_eq!(feature_key_for(BotModule::Sniper), Some("module.sniper"));
        assert_eq!(feature_key_for(BotModule::Copy), Some("module.copy"));
        assert_eq!(
            feature_key_for(BotModule::Polymarket),
            Some("module.polymarket")
        );
        assert_eq!(feature_key_for(BotModule::Telegram), None);
    }
}
