//! The tenant customer-API authorization chain (§H, Batch 8).
//!
//! ONE explicit, ordered guard chain for every `/api/tenant/*` trading
//! surface. The order is fixed and every step fails closed:
//!
//! 1. **authenticate** — `authorize_request` resolves the credential
//!    (session or tenant API key) into a [`SaasContext`]; no credential,
//!    no context, no data.
//! 2. **organization** — the tenant identity comes from the authenticated
//!    context ONLY. A client-supplied `organization_id` is never trusted
//!    for authorization; it may at most name a resource and is then
//!    ownership-checked by the repositories.
//! 3. **runtime** — the trading data plane (and its database) must be
//!    attached; otherwise the whole surface answers `503`, it does not
//!    degrade to in-memory guesses.
//! 4. **lifecycle** — suspended/closed organizations are refused before
//!    any data access (`403` with the lifecycle reason).
//! 5. **entitlement** — the organization's effective entitlements must
//!    grant the module family the route belongs to.
//! 6. **module** — the module feature itself (`module.sniper`,
//!    `module.copy`, `module.polymarket`) is the entitlement being
//!    checked; there is no separate, weaker gate.
//! 7. **tenant repository scope** — the caller receives scopes bound to
//!    the authenticated organization ([`TradingQueryScope`] /
//!    [`TenantWriteScope`]); the scope is created by the plane, never
//!    assembled from client input.
//! 8. **tenant data only** — every repository predicate carries the
//!    organization id from step 2; cross-tenant reads return not-found.
//!
//! Steps 1–3 and 7–8 already existed per handler; this module makes the
//! full chain explicit, uniform, and testable, and adds the lifecycle +
//! entitlement gates the customer API must fail closed on.

use std::sync::Arc;

use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::json;

use bot_core::authorization::AccessRequest;
use bot_core::billing::EntitlementSet;
use bot_core::membership::Permission;
use bot_core::tenant::{OrganizationId, OrganizationStatus};

use crate::api::ApiState;
use crate::saas::middleware::{authorize_request, deny_response, SaasContext};
use crate::trading_data_plane::TenantTradingDataPlane;

/// The module families a customer-API route can belong to. Each family
/// maps to the entitlement feature that grants it — there is no route
/// without a family, and no family without an entitlement.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TradingModuleFamily {
    /// Core trading records (orders, executions, positions, balances,
    /// recovery) — visible to a tenant entitled to ANY trading module,
    /// because all modules write into the same tenant-scoped truth.
    CoreTrading,
    /// Copy-trading leaders/links + controls — `module.copy`.
    Copy,
    /// Polymarket orders/fills + controls — `module.polymarket`.
    Polymarket,
    /// Sniper controls/status — `module.sniper`.
    Sniper,
    /// Telegram binding/status — the CONTROL PLANE. Every authenticated
    /// tenant has it and it never trades by itself
    /// (`bot_core::tenant::feature_key` maps it to no plan feature);
    /// the entitlement step is therefore a no-op for this family, not a
    /// weaker gate.
    Telegram,
}

impl TradingModuleFamily {
    /// The entitlement feature(s) that satisfy this family. `CoreTrading`
    /// is satisfied by any one trading module entitlement.
    pub fn satisfying_features(self) -> &'static [&'static str] {
        match self {
            TradingModuleFamily::CoreTrading => &[
                bot_core::billing::plan::features::MODULE_SNIPER,
                bot_core::billing::plan::features::MODULE_COPY,
                bot_core::billing::plan::features::MODULE_POLYMARKET,
            ],
            TradingModuleFamily::Copy => &[bot_core::billing::plan::features::MODULE_COPY],
            TradingModuleFamily::Polymarket => {
                &[bot_core::billing::plan::features::MODULE_POLYMARKET]
            }
            TradingModuleFamily::Sniper => &[bot_core::billing::plan::features::MODULE_SNIPER],
            // No plan feature gates the control plane — see the variant
            // docs. `check_module_entitlement` treats the empty feature
            // list as "granted to any authenticated tenant".
            TradingModuleFamily::Telegram => &[],
        }
    }
}

/// Why the chain refused, after authentication already succeeded
/// (authentication denials keep their existing shape from
/// `authorize_request`/`deny_response`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TradingChainDenial {
    /// Step 3: the data plane is not attached.
    PlaneUnavailable,
    /// Step 4: the organization lifecycle blocks data access.
    LifecycleBlocked { status: &'static str },
    /// Steps 5–6: no entitlement grants the route's module family.
    ModuleNotEntitled { family: &'static str },
}

impl TradingChainDenial {
    /// Stable machine code for audit rows and API error bodies.
    pub fn code(&self) -> &'static str {
        match self {
            TradingChainDenial::PlaneUnavailable => "trading_data_plane_unavailable",
            TradingChainDenial::LifecycleBlocked { .. } => "tenant_lifecycle_blocked",
            TradingChainDenial::ModuleNotEntitled { .. } => "module_not_entitled",
        }
    }

    pub fn http_status(&self) -> StatusCode {
        match self {
            TradingChainDenial::PlaneUnavailable => StatusCode::SERVICE_UNAVAILABLE,
            TradingChainDenial::LifecycleBlocked { .. }
            | TradingChainDenial::ModuleNotEntitled { .. } => StatusCode::FORBIDDEN,
        }
    }

    pub fn detail(&self) -> String {
        match self {
            TradingChainDenial::PlaneUnavailable => {
                "the trading data plane is not attached to this deployment".to_string()
            }
            TradingChainDenial::LifecycleBlocked { status } => {
                format!("organization lifecycle status '{status}' blocks tenant data access")
            }
            TradingChainDenial::ModuleNotEntitled { family } => {
                format!("no entitlement grants the '{family}' module family for this organization")
            }
        }
    }

    pub fn into_response(self) -> Response {
        (
            self.http_status(),
            Json(json!({
                "error": self.code(),
                "detail": self.detail(),
            })),
        )
            .into_response()
    }
}

/// What a successfully guarded handler receives: the authenticated
/// context plus the plane, both bound to the SAME organization.
pub struct AuthorizedTradingPlane {
    pub ctx: SaasContext,
    pub plane: Arc<TenantTradingDataPlane>,
}

impl AuthorizedTradingPlane {
    /// The authenticated organization (step 2) — the only identity the
    /// handler may use to build scopes.
    pub fn organization_id(&self) -> OrganizationId {
        self.ctx.organization_id()
    }
}

/// Step 4 — lifecycle. Active, trialing and past-due tenants keep READ
/// access (past-due is a collections state, not a data freeze — the
/// dunning policy in `bot_core::tenant::policy` governs writes);
/// suspended and closed tenants are refused.
pub fn check_lifecycle(status: OrganizationStatus) -> Result<(), TradingChainDenial> {
    match status {
        OrganizationStatus::Active | OrganizationStatus::Trialing | OrganizationStatus::PastDue => {
            Ok(())
        }
        OrganizationStatus::Suspended => Err(TradingChainDenial::LifecycleBlocked {
            status: "suspended",
        }),
        OrganizationStatus::Closed => {
            Err(TradingChainDenial::LifecycleBlocked { status: "closed" })
        }
    }
}

/// Steps 5–6 — module family entitlement. The family is satisfied when
/// ANY of its features is enabled in the effective entitlement set.
pub fn check_module_entitlement(
    entitlements: &EntitlementSet,
    family: TradingModuleFamily,
) -> Result<(), TradingChainDenial> {
    // The telegram control plane is not plan-gated: any authenticated,
    // lifecycle-allowed tenant has it. This is an explicit carve-out in
    // one place, not a missing check.
    if family == TradingModuleFamily::Telegram {
        return Ok(());
    }
    let granted = family
        .satisfying_features()
        .iter()
        .any(|feature| entitlements.limit_for(feature).is_enabled());
    if granted {
        Ok(())
    } else {
        let family_name = match family {
            TradingModuleFamily::CoreTrading => "core_trading",
            TradingModuleFamily::Copy => "copy",
            TradingModuleFamily::Polymarket => "polymarket",
            TradingModuleFamily::Sniper => "sniper",
            TradingModuleFamily::Telegram => "telegram",
        };
        Err(TradingChainDenial::ModuleNotEntitled {
            family: family_name,
        })
    }
}

/// The full chain for a READ request. Every `/api/tenant/*` trading
/// read handler calls this; none of them may talk to the plane without
/// the chain.
#[allow(clippy::result_large_err)]
pub async fn guard(
    state: &ApiState,
    headers: &HeaderMap,
    permission: Permission,
    family: TradingModuleFamily,
) -> Result<AuthorizedTradingPlane, Response> {
    guard_with(state, headers, permission, family, AccessRequest::read).await
}

/// The full chain for a MANAGE request (cancel/sweep-style mutations on
/// the data plane). Same ordered steps, manage-level access request.
#[allow(clippy::result_large_err)]
pub async fn guard_manage(
    state: &ApiState,
    headers: &HeaderMap,
    permission: Permission,
    family: TradingModuleFamily,
) -> Result<AuthorizedTradingPlane, Response> {
    guard_with(state, headers, permission, family, AccessRequest::manage).await
}

#[allow(clippy::result_large_err)]
async fn guard_with(
    state: &ApiState,
    headers: &HeaderMap,
    permission: Permission,
    family: TradingModuleFamily,
    level: fn(Permission) -> AccessRequest<'static>,
) -> Result<AuthorizedTradingPlane, Response> {
    // Steps 1–2: authenticate + resolve the organization from the
    // credential. Denials here carry the existing response shape.
    let ctx = match authorize_request(state, headers, level(permission)).await {
        Ok(c) => c,
        Err(d) => return Err(deny_response(state, &d).await),
    };
    // Step 3: the runtime/data plane must be attached.
    let plane = match state.trading.as_ref() {
        Some(p) => Arc::clone(p),
        None => return Err(TradingChainDenial::PlaneUnavailable.into_response()),
    };
    // Step 4: lifecycle.
    if let Err(denial) = check_lifecycle(ctx.organization.status) {
        return Err(denial.into_response());
    }
    // Steps 5–6: module family entitlement.
    let entitlements = match state
        .saas
        .entitlements_of(ctx.organization_id(), chrono::Utc::now())
        .await
    {
        Ok(value) => value,
        Err(error) => {
            tracing::error!(error = %error, organization = %ctx.organization_id(), "trading authorization entitlements could not be loaded");
            return Err((
                StatusCode::SERVICE_UNAVAILABLE,
                Json(json!({
                    "error": "authorization_evidence_unavailable",
                    "detail": "tenant entitlements could not be loaded",
                })),
            )
                .into_response());
        }
    };
    if let Err(denial) = check_module_entitlement(&entitlements, family) {
        return Err(denial.into_response());
    }
    // Steps 7–8 are the caller's repository calls through the plane with
    // `AuthorizedTradingPlane::organization_id()` — the plane builds
    // scopes bound to that organization only.
    Ok(AuthorizedTradingPlane { ctx, plane })
}

#[cfg(test)]
mod tests {
    use super::*;
    use bot_core::billing::entitlement::{Entitlement, EntitlementSource};
    use bot_core::billing::plan::features;

    fn entitlement_set(features: &[&str]) -> EntitlementSet {
        let org = OrganizationId::new();
        let now = chrono::Utc::now();
        let stored: Vec<Entitlement> = features
            .iter()
            .map(|f| Entitlement::new(org, *f, None, EntitlementSource::Override, now))
            .collect();
        EntitlementSet::resolve(None, None, &stored, now)
    }

    #[test]
    fn lifecycle_allows_read_states_and_blocks_terminal_states() {
        for ok in [
            OrganizationStatus::Active,
            OrganizationStatus::Trialing,
            OrganizationStatus::PastDue,
        ] {
            assert!(check_lifecycle(ok).is_ok(), "{ok:?} keeps read access");
        }
        for blocked in [OrganizationStatus::Suspended, OrganizationStatus::Closed] {
            let denial = check_lifecycle(blocked).unwrap_err();
            assert_eq!(denial.code(), "tenant_lifecycle_blocked");
            assert_eq!(denial.http_status(), StatusCode::FORBIDDEN);
            assert!(denial.detail().contains(blocked.as_str()));
        }
    }

    #[test]
    fn core_trading_is_satisfied_by_any_trading_module() {
        for feature in [
            features::MODULE_SNIPER,
            features::MODULE_COPY,
            features::MODULE_POLYMARKET,
        ] {
            let set = entitlement_set(&[feature]);
            assert!(
                check_module_entitlement(&set, TradingModuleFamily::CoreTrading).is_ok(),
                "{feature} grants core trading"
            );
        }
    }

    #[test]
    fn copy_family_requires_copy_module_exactly() {
        let sniper_only = entitlement_set(&[features::MODULE_SNIPER]);
        assert!(check_module_entitlement(&sniper_only, TradingModuleFamily::Copy).is_err());
        let copy = entitlement_set(&[features::MODULE_COPY]);
        assert!(check_module_entitlement(&copy, TradingModuleFamily::Copy).is_ok());
    }

    #[test]
    fn polymarket_family_requires_polymarket_module() {
        let copy_only = entitlement_set(&[features::MODULE_COPY]);
        let denial = check_module_entitlement(&copy_only, TradingModuleFamily::Polymarket)
            .expect_err("copy does not grant polymarket");
        assert_eq!(denial.code(), "module_not_entitled");
        assert_eq!(denial.http_status(), StatusCode::FORBIDDEN);
        assert!(denial.detail().contains("polymarket"));
    }

    #[test]
    fn tenant_with_no_entitlements_is_refused_fail_closed() {
        let none = entitlement_set(&[]);
        for family in [
            TradingModuleFamily::CoreTrading,
            TradingModuleFamily::Copy,
            TradingModuleFamily::Polymarket,
        ] {
            assert!(check_module_entitlement(&none, family).is_err());
        }
    }

    #[test]
    fn denial_response_body_is_machine_readable() {
        let body = TradingChainDenial::PlaneUnavailable.into_response();
        assert_eq!(body.status(), StatusCode::SERVICE_UNAVAILABLE);
        let denial = TradingChainDenial::LifecycleBlocked {
            status: "suspended",
        };
        assert!(denial.code().contains("lifecycle"));
    }
}
