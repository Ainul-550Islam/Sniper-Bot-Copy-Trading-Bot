//! Customer bots surface (§H, Batch 8).
//!
//! `GET /api/tenant/bots` and `GET /api/tenant/bots/:module` answer the
//! customer question "which bot runtimes exist for MY organization and
//! what phase are they in". The data comes from the live §A
//! [`TenantModuleRegistry`] — the same registry the runtime registers
//! module launches into. When nothing is registered, the honest answer
//! is an empty list, never a synthesized one.
//!
//! Every handler runs the full [`crate::trading_data_plane::authorization_chain`]
//! guard: authenticate → organization (from the credential, never from
//! the client) → plane attachment → lifecycle → module-family
//! entitlement → registry read scoped to the authenticated organization.
//!
//! Runtime CONTROL (start/stop/pause) is deliberately not exposed here
//! yet: the module lifecycle transitions are operator/runtime-owned
//! (`module_lifecycle::authorize_start` gates them on runtime identity
//! and fencing). The customer surface reports state; it does not reach
//! around the fencing. Each bot row says exactly that in `controls`.
//!
//! Note: the chain's plane step requires the trading data plane to be
//! attached. Bots of a tenant with no trading-module entitlement are
//! refused (`module_not_entitled`) — the surface is part of the trading
//! customer API.

use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::json;

use bot_core::membership::Permission;

use crate::api::ApiState;
use crate::trading_data_plane::authorization_chain::{guard, TradingModuleFamily};

/// The machine-name of a module, as the registry reports it.
fn module_name(module: bot_core::models::BotModule) -> String {
    module.to_string()
}

/// Serialize one registry handle into the customer-facing bot row.
/// Everything shown here is non-secret runtime identity + phase.
fn bot_row(handle: &crate::module_runtime::module_handle::ModuleHandle) -> serde_json::Value {
    json!({
        "module": module_name(handle.module()),
        "runtime_id": handle.runtime_id().to_string(),
        "generation": handle.generation().to_string(),
        "phase": handle.phase().as_str(),
        "since": handle.since().to_rfc3339(),
        "controls": "runtime-owned: module lifecycle transitions are gated on runtime identity and fencing; this surface reports state only",
    })
}

/// `GET /api/tenant/bots` — the caller's bot runtimes.
pub async fn list(State(state): State<ApiState>, headers: HeaderMap) -> Response {
    let auth = match guard(
        &state,
        &headers,
        Permission::OrderRead,
        TradingModuleFamily::CoreTrading,
    )
    .await
    {
        Ok(a) => a,
        Err(response) => return response,
    };
    let org = auth.organization_id();
    let handles = state.module_registry.modules_for(org);
    let items: Vec<serde_json::Value> = handles.iter().map(bot_row).collect();
    (
        StatusCode::OK,
        Json(json!({
            "organization_id": org.to_string(),
            "items": items,
            "count": items.len(),
        })),
    )
        .into_response()
}

/// `GET /api/tenant/bots/:module` — one module family's runtime for the
/// caller's organization. An unknown module name is a `400`; a module
/// with no registered runtime for THIS organization is an empty `200`
/// (absence is not an error and leaks nothing).
pub async fn detail(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Path(module): Path<String>,
) -> Response {
    let auth = match guard(
        &state,
        &headers,
        Permission::OrderRead,
        TradingModuleFamily::CoreTrading,
    )
    .await
    {
        Ok(a) => a,
        Err(response) => return response,
    };
    let org = auth.organization_id();
    let parsed = match module.trim().parse::<bot_core::models::BotModule>() {
        Ok(m) => m,
        Err(_) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({
                    "error": "unknown_module",
                    "detail": format!("'{module}' is not a known module"),
                })),
            )
                .into_response()
        }
    };
    let handle = state.module_registry.get(org, parsed);
    match handle {
        Some(h) => (
            StatusCode::OK,
            Json(json!({
                "organization_id": org.to_string(),
                "bot": bot_row(&h),
            })),
        )
            .into_response(),
        None => (
            StatusCode::OK,
            Json(json!({
                "organization_id": org.to_string(),
                "bot": serde_json::Value::Null,
                "detail": "no runtime registered for this module and organization",
            })),
        )
            .into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::module_runtime::module_handle::ModuleHandle;
    use crate::module_runtime::module_lifecycle::ModulePhase;
    use crate::module_runtime::tenant_module_instance::TenantModuleInstance;
    use bot_core::execution::{AuthorityChecklist, ExecutionTrace, AUTHORITY_CHECK_ORDER};
    use bot_core::models::BotModule;
    use bot_core::tenant::{
        OrganizationId, RuntimeGeneration, RuntimeId, TenantSignerRef, TenantWalletRef,
    };
    use chrono::Utc;

    /// Mirror of the §A test pattern: derive the instance from a fully
    /// ISSUED execution context (the only public constructor).
    fn instance(org: OrganizationId, module: BotModule) -> TenantModuleInstance {
        use bot_core::execution::TenantExecutionContext;
        use bot_core::models::ExecutionMode;
        let now = Utc::now();
        let runtime = RuntimeId::new();
        let generation = RuntimeGeneration::first();
        let scope = bot_core::execution::ExecutionScope::new(
            org,
            runtime,
            generation,
            module,
            ExecutionMode::Paper,
        )
        .expect("scope");
        let mut checklist = AuthorityChecklist::new();
        for name in AUTHORITY_CHECK_ORDER {
            checklist.record(name, now).expect("checklist record");
        }
        let authority = checklist.finish(&scope, now).expect("authority");
        let wallet = TenantWalletRef::new(org, "9WzDXwBbmkg8ZTbNMqUxvQRAyrZzDsGYdLVL9zYtAWWM")
            .expect("wallet ref");
        let signer = TenantSignerRef::new(org, bot_core::tenant::SignerProvider::Local, "mod-key")
            .expect("signer ref");
        let context = TenantExecutionContext::issue(
            org,
            runtime,
            generation,
            module,
            ExecutionMode::Paper,
            authority,
            wallet,
            signer,
            ExecutionTrace::for_request(),
        )
        .expect("issued context");
        TenantModuleInstance::from_context(&context, true)
    }

    #[test]
    fn bot_row_reports_identity_and_phase_without_secrets() {
        let org = OrganizationId::new();
        let handle = ModuleHandle::new(
            instance(org, BotModule::Sniper),
            ModulePhase::Running,
            Utc::now(),
        );
        let row = bot_row(&handle);
        assert_eq!(row["module"], "sniper");
        assert_eq!(row["phase"], "running");
        assert!(row["runtime_id"].as_str().unwrap().len() > 10);
        assert!(row["generation"].as_str().is_some());
        assert!(row["since"].as_str().is_some());
        assert!(row["controls"].as_str().unwrap().contains("runtime-owned"));
        let s = row.to_string().to_ascii_lowercase();
        for banned in ["secret", "private", "sk-", "password", "token"] {
            assert!(!s.contains(banned), "leaked {banned}");
        }
    }

    #[test]
    fn registry_listing_is_organization_scoped() {
        use crate::module_runtime::module_registry::TenantModuleRegistry;
        let registry = TenantModuleRegistry::new();
        let a = OrganizationId::new();
        let b = OrganizationId::new();
        let _ = registry.register(ModuleHandle::new(
            instance(a, BotModule::Copy),
            ModulePhase::Running,
            Utc::now(),
        ));
        let _ = registry.register(ModuleHandle::new(
            instance(b, BotModule::Sniper),
            ModulePhase::Idle,
            Utc::now(),
        ));
        let a_handles = registry.modules_for(a);
        assert_eq!(a_handles.len(), 1);
        assert_eq!(a_handles[0].module(), BotModule::Copy);
        let b_handles = registry.modules_for(b);
        assert_eq!(b_handles.len(), 1);
        assert_eq!(b_handles[0].module(), BotModule::Sniper);
        // A third org sees nothing.
        let c = OrganizationId::new();
        assert!(registry.modules_for(c).is_empty());
    }
}
