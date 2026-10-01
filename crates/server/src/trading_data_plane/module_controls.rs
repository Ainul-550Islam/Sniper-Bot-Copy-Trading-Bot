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
//! * the store is process-local (the same contract as the custody
//!   store in `saas/custody.rs`); the DB-backed path is a migration away
//!   and the API surface will not change.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use bot_core::models::BotModule;
use bot_core::tenant::OrganizationId;
use chrono::{DateTime, Utc};
use serde_json::json;

use crate::api::ApiState;
use crate::trading_data_plane::authorization_chain::AuthorizedTradingPlane;

/// A tenant-level module control override.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModuleControlOverride {
    /// The override state: `false` = tenant paused the module.
    pub enabled: bool,
    /// Why (for disables; empty for enables).
    pub reason: String,
    pub updated_at: DateTime<Utc>,
    /// Non-secret actor label from the authenticated context.
    pub updated_by: String,
}

fn controls_store() -> &'static Mutex<HashMap<(OrganizationId, BotModule), ModuleControlOverride>> {
    static S: OnceLock<Mutex<HashMap<(OrganizationId, BotModule), ModuleControlOverride>>> =
        OnceLock::new();
    S.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Read THIS organization's override for a module (tenant-scoped).
pub fn override_for(org: OrganizationId, module: BotModule) -> Option<ModuleControlOverride> {
    controls_store()
        .lock()
        .expect("controls store mutex")
        .get(&(org, module))
        .cloned()
}

/// Apply a control override for THIS organization's module.
pub fn apply_override(
    org: OrganizationId,
    module: BotModule,
    enabled: bool,
    reason: &str,
    updated_by: &str,
    now: DateTime<Utc>,
) -> ModuleControlOverride {
    let entry = ModuleControlOverride {
        enabled,
        reason: reason.to_string(),
        updated_at: now,
        updated_by: updated_by.to_string(),
    };
    controls_store()
        .lock()
        .expect("controls store mutex")
        .insert((org, module), entry.clone());
    entry
}

/// Clear the override entirely (used by tests and by "re-enable" when
/// the tenant wants to return to entitlement-governed default).
pub fn clear_override(org: OrganizationId, module: BotModule) -> bool {
    controls_store()
        .lock()
        .expect("controls store mutex")
        .remove(&(org, module))
        .is_some()
}

/// The effective module state from the tenant's perspective: a disable
/// override wins; everything else is the entitlement default (which the
/// guard has already established as granted before a handler runs).
pub fn effective_state(
    org: OrganizationId,
    module: BotModule,
) -> (&'static str, Option<ModuleControlOverride>) {
    match override_for(org, module) {
        Some(o) if !o.enabled => ("disabled", Some(o)),
        Some(o) => ("enabled", Some(o)),
        None => ("enabled", None),
    }
}

/// Assemble the module status payload shared by every per-module status
/// handler. `feature` is the entitlement feature the family maps to
/// (reported as granted — the guard refused otherwise).
pub fn status_payload(
    state: &ApiState,
    auth: &AuthorizedTradingPlane,
    module: BotModule,
    feature: Option<&'static str>,
) -> serde_json::Value {
    let org = auth.organization_id();
    let (effective, override_entry) = effective_state(org, module);
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
pub fn apply_control(
    state: &ApiState,
    auth: &AuthorizedTradingPlane,
    module: BotModule,
    action: ControlAction,
) -> Response {
    let org = auth.organization_id();
    let actor = auth.ctx.actor_label();
    let now = Utc::now();
    let applied = match action {
        ControlAction::Enable => {
            // Enable clears any disable override — the module returns to
            // its entitlement-governed default.
            clear_override(org, module);
            json!({
                "action": "enable",
                "applied": true,
                "detail": "tenant override cleared; module follows the entitlement-governed default",
                "updated_at": now.to_rfc3339(),
                "updated_by": actor,
            })
        }
        ControlAction::Disable { reason } => {
            let entry = apply_override(org, module, false, &reason, &actor, now);
            json!({
                "action": "disable",
                "applied": true,
                "detail": "tenant-level disable recorded; the runtime honors it at its next module evaluation (runtime lifecycle stays runtime-owned)",
                "reason": entry.reason,
                "updated_at": entry.updated_at.to_rfc3339(),
                "updated_by": entry.updated_by,
            })
        }
    };
    let mut status = status_payload(state, auth, module, feature_key_for(module));
    if let Some(obj) = status.as_object_mut() {
        obj.insert("control_result".to_string(), applied);
    }
    (StatusCode::OK, Json(status)).into_response()
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

    #[test]
    fn overrides_are_tenant_scoped() {
        let a = org();
        let b = org();
        apply_override(a, BotModule::Sniper, false, "paused", "user:a", Utc::now());
        // Org B sees no override and an enabled default.
        assert!(override_for(b, BotModule::Sniper).is_none());
        assert_eq!(effective_state(b, BotModule::Sniper).0, "enabled");
        // Org A sees its own.
        let (state, entry) = effective_state(a, BotModule::Sniper);
        assert_eq!(state, "disabled");
        assert_eq!(entry.unwrap().reason, "paused");
        clear_override(a, BotModule::Sniper);
        assert_eq!(effective_state(a, BotModule::Sniper).0, "enabled");
    }

    #[test]
    fn overrides_are_per_module() {
        let a = org();
        apply_override(
            a,
            BotModule::Copy,
            false,
            "paused copy",
            "user:a",
            Utc::now(),
        );
        assert_eq!(effective_state(a, BotModule::Sniper).0, "enabled");
        assert_eq!(effective_state(a, BotModule::Copy).0, "disabled");
        clear_override(a, BotModule::Copy);
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
