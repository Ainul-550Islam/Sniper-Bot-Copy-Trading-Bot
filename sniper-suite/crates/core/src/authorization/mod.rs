//! Centralized SaaS authorization (TASK 7A file 18).
//!
//! ONE function decides every tenant-scoped request: [`authorize`]. It runs
//! the gates in a fixed order and returns a typed [`Decision`]; there is no
//! way for a handler to skip a gate, reorder them, or invent an outcome.
//!
//! ```text
//! authenticated?      → DENY_UNAUTHENTICATED   (no context at all)
//! tenant resolved?    → DENY_TENANT
//! resource ownership  → DENY_RESOURCE          (cross-tenant, checked BEFORE role)
//! tenant state        → DENY_SUSPENDED
//! role / permission   → DENY_PERMISSION
//! entitlement         → DENY_ENTITLEMENT
//! otherwise           → ALLOW
//! ```
//!
//! Ownership is checked before the role on purpose: a cross-tenant request
//! must be refused even when the caller is an owner in their own tenant,
//! and the answer must not depend on what role they happen to hold.
//!
//! | file | concern |
//! |---|---|
//! | `context.rs` | [`AuthorizationContext`]: principal + tenant + role + effective permissions |
//! | `decision.rs` | the 8-value [`DecisionKind`] vocabulary and [`Decision`] |
//!
//! # What this layer must never do
//!
//! It gates the CONTROL PLANE. It cannot approve a trade: after it allows a
//! request, the TASK 5 global risk engine and the module risk checks still
//! run, and the TASK 6 lease fencing still applies. An `ALLOW` here means
//! "this caller may ask", never "the system will comply".

pub mod context;
pub mod decision;

pub use context::{AuthorizationContext, Principal};
pub use decision::{Decision, DecisionKind};

use crate::billing::{EntitlementSet, EntitlementVerdict};
use crate::membership::Permission;
use crate::tenant::{
    policy::{self, TenantAction, TenantVerdict},
    OrganizationId,
};

/// What a request wants to do. Everything the gates need, in one value.
#[derive(Debug, Clone)]
pub struct AccessRequest<'a> {
    /// The permission the handler requires.
    pub permission: Permission,
    /// The tenant-level action class (read / manage / trade / …).
    pub action: TenantAction,
    /// The tenant that owns the resource being touched, when the request
    /// names one. `None` for collection endpoints, which are implicitly
    /// scoped to the caller's own tenant.
    pub resource_owner: Option<OrganizationId>,
    /// The feature the request consumes, when it is gated by the plan.
    pub feature: Option<&'a str>,
    /// Current usage of that feature (for counted limits).
    pub current_usage: f64,
    /// How much this request would add.
    pub requested_usage: f64,
}

impl<'a> AccessRequest<'a> {
    /// A read of the caller's own tenant.
    pub fn read(permission: Permission) -> Self {
        AccessRequest {
            permission,
            action: TenantAction::Read,
            resource_owner: None,
            feature: None,
            current_usage: 0.0,
            requested_usage: 0.0,
        }
    }

    /// A control-plane change.
    pub fn manage(permission: Permission) -> Self {
        AccessRequest {
            permission,
            action: TenantAction::Manage,
            resource_owner: None,
            feature: None,
            current_usage: 0.0,
            requested_usage: 0.0,
        }
    }

    /// An action that starts new trading activity.
    pub fn trade(permission: Permission) -> Self {
        AccessRequest {
            permission,
            action: TenantAction::Trade,
            resource_owner: None,
            feature: None,
            current_usage: 0.0,
            requested_usage: 0.0,
        }
    }

    /// An action that only reduces exposure.
    pub fn reduce_risk(permission: Permission) -> Self {
        AccessRequest {
            permission,
            action: TenantAction::ReduceRisk,
            resource_owner: None,
            feature: None,
            current_usage: 0.0,
            requested_usage: 0.0,
        }
    }

    /// A billing action.
    pub fn billing(permission: Permission) -> Self {
        AccessRequest {
            permission,
            action: TenantAction::Billing,
            resource_owner: None,
            feature: None,
            current_usage: 0.0,
            requested_usage: 0.0,
        }
    }

    /// Name the tenant that owns the resource being touched.
    pub fn on_resource(mut self, owner: OrganizationId) -> Self {
        self.resource_owner = Some(owner);
        self
    }

    /// Require a plan feature (on/off).
    pub fn requiring(mut self, feature: &'a str) -> Self {
        self.feature = Some(feature);
        self
    }

    /// Require a counted plan limit.
    pub fn consuming(mut self, feature: &'a str, current: f64, requested: f64) -> Self {
        self.feature = Some(feature);
        self.current_usage = current;
        self.requested_usage = requested;
        self
    }
}

/// THE authorization decision. Deterministic and side-effect free.
///
/// `context = None` means the request was not authenticated at all.
/// `entitlements = None` skips the plan gate (used by endpoints that must
/// stay reachable even when billing data cannot be loaded, e.g. reading
/// one's own profile).
pub fn authorize(
    context: Option<&AuthorizationContext>,
    request: &AccessRequest<'_>,
    entitlements: Option<&EntitlementSet>,
) -> Decision {
    // 1. Authenticated?
    let Some(ctx) = context else {
        return Decision::unauthenticated("no authenticated principal on the request");
    };

    // 2. Resource ownership — before role, so a cross-tenant attempt is
    //    refused identically whatever role the caller holds elsewhere.
    if let Some(owner) = request.resource_owner {
        if !ctx.owns(owner) {
            return Decision::resource(format!(
                "resource belongs to another organization (acting as {})",
                ctx.organization_id
            ));
        }
    }

    // 3. Tenant state.
    if let TenantVerdict::Deny(reason) =
        policy::check_status(ctx.organization_status, request.action)
    {
        // A cross-tenant answer already returned above; everything here is
        // about the caller's own tenant being unable to act.
        return Decision::suspended(format!("{} ({})", reason.detail(), reason.as_str()));
    }

    // 4. Permission (which subsumes the role: the role IS its permissions).
    if !ctx.has(request.permission) {
        return Decision::permission(format!(
            "role {} does not grant {}",
            ctx.role, request.permission
        ));
    }

    // 5. Entitlement.
    if let (Some(feature), Some(set)) = (request.feature, entitlements) {
        match set.check(feature, request.current_usage, request.requested_usage) {
            EntitlementVerdict::Allow(_) => {}
            EntitlementVerdict::Deny(reason) => {
                return Decision::entitlement(format!("{feature}: {}", reason.as_str()));
            }
        }
    }

    Decision::allow()
}

/// Convenience: authorize and map the refusal to `Err`.
pub fn require(
    context: Option<&AuthorizationContext>,
    request: &AccessRequest<'_>,
    entitlements: Option<&EntitlementSet>,
) -> Result<(), Decision> {
    let d = authorize(context, request, entitlements);
    if d.is_allowed() {
        Ok(())
    } else {
        Err(d)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::accounting::GlobalLedger;
    use crate::billing::plan::{features, FeatureLimit, Plan, PlanCode};
    use crate::billing::subscription::Subscription;
    use crate::config::GlobalRiskConfig;
    use crate::events::EventBus;
    use crate::global_risk::{
        DecisionContext, GlobalRejectReason, GlobalRiskEngine, GlobalRiskRequest,
    };
    use crate::ha::{HaStore, LeaseRequest, LeaseRole, MemoryHaStore};
    use crate::membership::{Membership, MembershipRole};
    use crate::models::{BotModule, ExecutionMode, Venue};
    use crate::tenant::{Organization, OrganizationStatus, UserId};
    use chrono::Utc;
    use std::collections::HashMap;
    use std::sync::Arc;

    fn org(status: OrganizationStatus) -> Organization {
        let mut o = Organization::new(
            OrganizationId::new(),
            "acme",
            "Acme",
            Some(UserId::new()),
            Utc::now(),
        );
        o.status = status;
        o
    }

    fn ctx(o: &Organization, role: MembershipRole) -> AuthorizationContext {
        let m = Membership::new(o.id, UserId::new(), role, None, Utc::now());
        AuthorizationContext::from_membership(
            Principal::UserSession {
                session_id: "s".into(),
            },
            o,
            &m,
            None,
            false,
            Utc::now(),
        )
    }

    fn entitlements(o: &Organization, live_trading: bool) -> EntitlementSet {
        let now = Utc::now();
        let plan = Plan::new(PlanCode::Pro, "Pro", now)
            .with_limit(
                features::LIVE_TRADING,
                if live_trading {
                    FeatureLimit::Unlimited
                } else {
                    FeatureLimit::Disabled
                },
            )
            .with_limit(features::MAX_API_KEYS, FeatureLimit::Limited(2.0));
        let sub = Subscription::manual(o.id, plan.id, now);
        EntitlementSet::resolve(Some(&plan), Some(&sub), &[], now)
    }

    #[test]
    fn unauthenticated_requests_are_denied_first() {
        let d = authorize(None, &AccessRequest::read(Permission::BotRead), None);
        assert_eq!(d.kind, DecisionKind::DenyUnauthenticated);
        assert_eq!(d.http_status(), 401);
    }

    #[test]
    fn a_permitted_request_in_a_healthy_tenant_is_allowed() {
        let o = org(OrganizationStatus::Active);
        let c = ctx(&o, MembershipRole::Trader);
        let d = authorize(Some(&c), &AccessRequest::read(Permission::BotRead), None);
        assert!(d.is_allowed(), "{d}");
        assert!(require(Some(&c), &AccessRequest::trade(Permission::BotStart), None).is_ok());
    }

    #[test]
    fn cross_tenant_is_refused_before_the_role_is_considered() {
        let mine = org(OrganizationStatus::Active);
        let theirs = OrganizationId::new();
        // Even an owner cannot reach another tenant.
        let c = ctx(&mine, MembershipRole::OrgOwner);
        let d = authorize(
            Some(&c),
            &AccessRequest::read(Permission::OrderRead).on_resource(theirs),
            None,
        );
        assert_eq!(d.kind, DecisionKind::DenyResource);
        assert_eq!(
            d.http_status(),
            403,
            "must not 404 — that would confirm existence"
        );

        // A caller who also lacks the permission still gets DENY_RESOURCE,
        // so the answer does not depend on their role.
        let v = ctx(&mine, MembershipRole::Viewer);
        let d = authorize(
            Some(&v),
            &AccessRequest::manage(Permission::RiskManage).on_resource(theirs),
            None,
        );
        assert_eq!(d.kind, DecisionKind::DenyResource);

        // Own resource is fine.
        assert!(authorize(
            Some(&c),
            &AccessRequest::read(Permission::OrderRead).on_resource(mine.id),
            None
        )
        .is_allowed());
    }

    #[test]
    fn a_suspended_tenant_cannot_trade_or_manage_but_can_read_and_reduce() {
        let o = org(OrganizationStatus::Suspended);
        let c = ctx(&o, MembershipRole::OrgOwner);
        assert_eq!(
            authorize(Some(&c), &AccessRequest::trade(Permission::BotStart), None).kind,
            DecisionKind::DenySuspended
        );
        assert_eq!(
            authorize(
                Some(&c),
                &AccessRequest::manage(Permission::TenantUpdate),
                None
            )
            .kind,
            DecisionKind::DenySuspended
        );
        assert!(authorize(Some(&c), &AccessRequest::read(Permission::BotRead), None).is_allowed());
        assert!(
            authorize(
                Some(&c),
                &AccessRequest::reduce_risk(Permission::BotStop),
                None
            )
            .is_allowed(),
            "a suspended customer must still be able to stop a bot"
        );
        assert!(
            authorize(
                Some(&c),
                &AccessRequest::billing(Permission::BillingManage),
                None
            )
            .is_allowed(),
            "…and to pay the invoice"
        );
    }

    #[test]
    fn a_role_without_the_permission_is_denied() {
        let o = org(OrganizationStatus::Active);
        let v = ctx(&o, MembershipRole::Viewer);
        let d = authorize(Some(&v), &AccessRequest::trade(Permission::BotStart), None);
        assert_eq!(d.kind, DecisionKind::DenyPermission);
        assert!(d.reason.contains("viewer"));
        assert!(d.reason.contains("bot.start"));

        // The same caller may read.
        assert!(authorize(Some(&v), &AccessRequest::read(Permission::BotRead), None).is_allowed());

        // A trader cannot administer keys.
        let t = ctx(&o, MembershipRole::Trader);
        assert_eq!(
            authorize(
                Some(&t),
                &AccessRequest::manage(Permission::ApiKeyCreate),
                None
            )
            .kind,
            DecisionKind::DenyPermission
        );
    }

    #[test]
    fn the_entitlement_gate_runs_last() {
        let o = org(OrganizationStatus::Active);
        let c = ctx(&o, MembershipRole::OrgOwner);

        // Plan lacks live trading.
        let ents = entitlements(&o, false);
        let d = authorize(
            Some(&c),
            &AccessRequest::trade(Permission::BotStart).requiring(features::LIVE_TRADING),
            Some(&ents),
        );
        assert_eq!(d.kind, DecisionKind::DenyEntitlement);
        assert_eq!(d.http_status(), 402);

        // Plan includes it.
        let ents = entitlements(&o, true);
        assert!(authorize(
            Some(&c),
            &AccessRequest::trade(Permission::BotStart).requiring(features::LIVE_TRADING),
            Some(&ents)
        )
        .is_allowed());

        // Counted limit.
        let d = authorize(
            Some(&c),
            &AccessRequest::manage(Permission::ApiKeyCreate).consuming(
                features::MAX_API_KEYS,
                2.0,
                1.0,
            ),
            Some(&ents),
        );
        assert_eq!(d.kind, DecisionKind::DenyEntitlement);
        assert!(authorize(
            Some(&c),
            &AccessRequest::manage(Permission::ApiKeyCreate).consuming(
                features::MAX_API_KEYS,
                1.0,
                1.0
            ),
            Some(&ents)
        )
        .is_allowed());
    }

    #[test]
    fn gate_order_is_stable() {
        // A request that fails several gates always reports the earliest.
        let theirs = OrganizationId::new();
        let suspended = org(OrganizationStatus::Suspended);
        let v = ctx(&suspended, MembershipRole::Viewer);
        let ents = entitlements(&suspended, false);
        let d = authorize(
            Some(&v),
            &AccessRequest::trade(Permission::BotStart)
                .on_resource(theirs)
                .requiring(features::LIVE_TRADING),
            Some(&ents),
        );
        assert_eq!(
            d.kind,
            DecisionKind::DenyResource,
            "ownership is the first gate after authentication"
        );

        // Remove the cross-tenant problem: the tenant state is next.
        let d = authorize(
            Some(&v),
            &AccessRequest::trade(Permission::BotStart)
                .on_resource(suspended.id)
                .requiring(features::LIVE_TRADING),
            Some(&ents),
        );
        assert_eq!(d.kind, DecisionKind::DenySuspended);

        // Healthy tenant, weak role: permission.
        let active = org(OrganizationStatus::Active);
        let v = ctx(&active, MembershipRole::Viewer);
        let ents = entitlements(&active, false);
        let d = authorize(
            Some(&v),
            &AccessRequest::trade(Permission::BotStart).requiring(features::LIVE_TRADING),
            Some(&ents),
        );
        assert_eq!(d.kind, DecisionKind::DenyPermission);
    }

    #[test]
    fn platform_admin_may_cross_tenants_with_both_flags() {
        let mine = org(OrganizationStatus::Active);
        let theirs = OrganizationId::new();
        let m = Membership::new(
            mine.id,
            UserId::new(),
            MembershipRole::PlatformAdmin,
            None,
            Utc::now(),
        );
        let staff = AuthorizationContext::from_membership(
            Principal::UserSession {
                session_id: "s".into(),
            },
            &mine,
            &m,
            None,
            true,
            Utc::now(),
        );
        assert!(authorize(
            Some(&staff),
            &AccessRequest::read(Permission::OrderRead).on_resource(theirs),
            None
        )
        .is_allowed());

        // Without the user flag it is an ordinary cross-tenant refusal.
        let not_staff = AuthorizationContext::from_membership(
            Principal::UserSession {
                session_id: "s".into(),
            },
            &mine,
            &m,
            None,
            false,
            Utc::now(),
        );
        assert_eq!(
            authorize(
                Some(&not_staff),
                &AccessRequest::read(Permission::OrderRead).on_resource(theirs),
                None
            )
            .kind,
            DecisionKind::DenyResource
        );
    }

    // ------------------------------------------------------ task boundaries --
    // The module doc promises: an ALLOW here means "this caller may ask",
    // never "the system will comply". These two tests pin that promise
    // against the two authorities the control plane must never displace:
    // the TASK 5 global-risk engine and the TASK 6 lease fencing.

    fn trade_request() -> GlobalRiskRequest {
        GlobalRiskRequest {
            module: BotModule::Sniper,
            venue: Venue::PumpFun,
            wallet: "w1".into(),
            strategy: "sniper".into(),
            asset: "MINT-K".into(),
            quote_asset: "SOL".into(),
            requested_quote: 0.1,
            mode: ExecutionMode::Paper,
        }
    }

    /// K: a SaaS `ALLOW` cannot bypass TASK 5. The global-risk engine owns
    /// the trade verdict and decides from its own state alone — an ALLOW
    /// from this layer does not move it, and a DENY here is not a risk
    /// control either. Both layers must pass in the real stack.
    #[tokio::test]
    async fn an_allow_here_cannot_bypass_the_task5_global_risk_engine() {
        let o = org(OrganizationStatus::Active);
        let owner = ctx(&o, MembershipRole::OrgOwner);
        let saas = authorize(
            Some(&owner),
            &AccessRequest::trade(Permission::BotStart),
            None,
        );
        assert!(saas.is_allowed(), "{saas}");

        let engine = GlobalRiskEngine::new(
            GlobalRiskConfig::default(),
            Arc::new(GlobalLedger::new(EventBus::new(64), "authz-boundary")),
            EventBus::new(64),
            "authz-boundary",
        );

        // SaaS says yes … TASK 5 still says no under its own kill switch.
        let killed = engine
            .decide(
                &trade_request(),
                &DecisionContext {
                    global_kill: true,
                    marks: HashMap::new(),
                },
            )
            .await;
        assert!(!killed.accepted(), "{killed:?}");
        assert_eq!(killed.reason, Some(GlobalRejectReason::GlobalKillSwitch));

        // … and the mirror direction: with the SaaS layer refusing outright,
        // the TASK 5 engine still evaluates on its own inputs (it would
        // allow this small paper request). Neither layer substitutes for
        // the other; neither can silence the other.
        let refused_here = authorize(None, &AccessRequest::trade(Permission::BotStart), None);
        assert!(!refused_here.is_allowed());
        let open = engine
            .decide(
                &trade_request(),
                &DecisionContext {
                    global_kill: false,
                    marks: HashMap::new(),
                },
            )
            .await;
        assert!(open.accepted(), "{open:?}");
    }

    /// L: a SaaS `ALLOW` cannot bypass TASK 6 fencing. Lease mutations are
    /// answered by the HaStore from `(role, holder, generation)` alone —
    /// there is no API path that lets an authorization decision renew,
    /// verify, or release a lease it does not hold, and a stale generation
    /// stays fenced after takeover.
    #[tokio::test]
    async fn an_allow_here_cannot_bypass_the_task6_lease_fencing() {
        let store = MemoryHaStore::new();
        let ttl = chrono::Duration::seconds(60);

        // Worker 1 legitimately holds the reconciliation lease.
        let first = store
            .acquire_lease(&LeaseRequest {
                role: LeaseRole::Reconciliation,
                holder: "w1".into(),
                ttl,
            })
            .await
            .expect("store answers");
        let lease = first.acquired().expect("first worker wins").clone();
        assert_eq!(lease.generation, 1);
        assert_eq!(lease.holder, "w1");

        // The SaaS layer fully trusts this caller — a platform administrator
        // whose cross-tenant requests are allowed everywhere.
        let mine = org(OrganizationStatus::Active);
        let m = crate::membership::Membership::new(
            mine.id,
            UserId::new(),
            MembershipRole::PlatformAdmin,
            None,
            Utc::now(),
        );
        let staff = AuthorizationContext::from_membership(
            Principal::UserSession {
                session_id: "s".into(),
            },
            &mine,
            &m,
            None,
            true,
            Utc::now(),
        );
        let saas = authorize(
            Some(&staff),
            &AccessRequest::read(Permission::RiskManage),
            None,
        );
        assert!(saas.is_allowed(), "{saas}");

        // Even so, the allowed caller cannot renew, verify, or release a
        // lease it does not hold: the store answers from the lease record,
        // never from the authorization decision.
        assert!(!store
            .renew_lease(&LeaseRole::Reconciliation, "w2", 1, ttl)
            .await
            .expect("store answers"));
        assert!(!store
            .verify_lease(&LeaseRole::Reconciliation, "w2", 1)
            .await
            .expect("store answers"));
        assert!(!store
            .release_lease(&LeaseRole::Reconciliation, "w2", 1)
            .await
            .expect("store answers"));
        let still = store
            .get_lease(&LeaseRole::Reconciliation)
            .await
            .expect("store answers")
            .expect("lease is live");
        assert_eq!(still.holder, "w1");
        assert_eq!(still.generation, 1);

        // The rightful holder still succeeds with its own generation.
        assert!(store
            .renew_lease(&LeaseRole::Reconciliation, "w1", 1, ttl)
            .await
            .expect("store answers"));

        // After expiry + takeover by w2, a delayed w1 write under the OLD
        // generation is fenced — even though w1 was also SaaS-allowed.
        store.advance_clock(chrono::Duration::seconds(120));
        let takeover = store
            .acquire_lease(&LeaseRequest {
                role: LeaseRole::Reconciliation,
                holder: "w2".into(),
                ttl,
            })
            .await
            .expect("store answers");
        assert!(takeover.is_acquired());
        assert!(!store
            .renew_lease(&LeaseRole::Reconciliation, "w1", 1, ttl)
            .await
            .expect("store answers"));
        let now_held = store
            .get_lease(&LeaseRole::Reconciliation)
            .await
            .expect("store answers")
            .expect("lease is live");
        assert_eq!(now_held.holder, "w2");
        assert_eq!(now_held.generation, 2);
    }
}
