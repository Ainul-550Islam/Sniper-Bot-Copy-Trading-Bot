//! Tenant-scoped subscription identity (tenant-isolation file 56).
//!
//! [`SubscriptionScope`] is the identity a WS/stream consumer
//! presents: WHICH tenant it subscribes for (mandatory — there is no
//! global scope), a stable subscription id, and the narrowed
//! [`TenantStreamFilter`] behind the same mandatory-org rule. Two
//! tenants can never share a scope, and a scope cannot widen to a
//! second tenant after construction: `accepts` delegates to the
//! filter, which checks the organization FIRST.
//!
//! This composes with [`super::hub::TenantStreamHub`]: the hub routes
//! per organization, the scope decides per event.

use bot_core::tenant::OrganizationId;

use super::events::TenantEvent;
use super::filter::TenantStreamFilter;

/// One consumer's tenant-scoped subscription identity.
#[derive(Debug, Clone, PartialEq)]
pub struct SubscriptionScope {
    /// The ONLY organization this subscription may receive.
    organization_id: OrganizationId,
    /// A stable, non-secret subscription identifier (correlates
    /// connection logs and delivery metrics).
    subscription_id: String,
    /// The delivery filter (mandatory org scoping + optional
    /// refinements that only narrow).
    filter: TenantStreamFilter,
}

impl SubscriptionScope {
    /// A subscription for exactly one tenant. There is deliberately
    /// no constructor without an organization id.
    pub fn new(organization_id: OrganizationId, subscription_id: impl Into<String>) -> Self {
        SubscriptionScope {
            organization_id,
            subscription_id: subscription_id.into(),
            filter: TenantStreamFilter::everything_for(organization_id),
        }
    }

    /// The subscribed tenant.
    pub fn organization_id(&self) -> OrganizationId {
        self.organization_id
    }

    /// The subscription id (non-secret, for correlation only).
    pub fn subscription_id(&self) -> &str {
        &self.subscription_id
    }

    /// The delivery filter.
    pub fn filter(&self) -> &TenantStreamFilter {
        &self.filter
    }

    /// Narrow delivery to specific event kinds (refinements only
    /// narrow — the organization scope is immutable).
    pub fn with_kinds(mut self, kinds: Vec<&'static str>) -> Self {
        self.filter = self.filter.clone().with_kinds(kinds);
        self
    }

    /// Narrow delivery to specific modules.
    pub fn with_modules(mut self, modules: Vec<&'static str>) -> Self {
        self.filter = self.filter.clone().with_modules(modules);
        self
    }

    /// May this event be delivered to this subscription? The tenant
    /// check comes first and can never be widened.
    pub fn accepts(&self, event: &TenantEvent) -> bool {
        // Defense in depth: the filter already enforces this, but the
        // scope answers the question on its own identity too.
        if event.organization_id() != self.organization_id {
            return false;
        }
        self.filter.accepts(event)
    }

    /// Does this subscription belong to this tenant?
    pub fn belongs_to(&self, organization_id: OrganizationId) -> bool {
        self.organization_id == organization_id
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bot_core::tenant::OrganizationId;

    fn decision_for(organization_id: OrganizationId) -> TenantEvent {
        TenantEvent::Decision {
            organization_id,
            decision: "allow",
            module: "copy",
            mode: "paper",
            origin: "http",
        }
    }

    #[test]
    fn a_scope_receives_only_its_own_tenants_events() {
        let a = OrganizationId::new();
        let b = OrganizationId::new();
        let scope = SubscriptionScope::new(a, "sub-1");
        assert!(scope.accepts(&decision_for(a)));
        assert!(!scope.accepts(&decision_for(b)));
        assert!(scope.belongs_to(a));
        assert!(!scope.belongs_to(b));
    }

    #[test]
    fn refinements_only_narrow_never_widen() {
        let a = OrganizationId::new();
        let b = OrganizationId::new();
        let scope = SubscriptionScope::new(a, "sub-1").with_kinds(vec!["decision"]);
        // Own-tenant matching-kind events still pass.
        assert!(scope.accepts(&decision_for(a)));
        // A kind refinement drops non-matching kinds.
        let config_change = TenantEvent::ConfigChanged {
            organization_id: a,
            version: 2,
            changes: 1,
        };
        assert!(!scope.accepts(&config_change));
        // No refinement can ever admit another tenant's event.
        let scope_all_kinds = SubscriptionScope::new(a, "sub-2");
        assert!(!scope_all_kinds.accepts(&decision_for(b)));
    }

    #[test]
    fn the_filter_and_the_scope_agree_on_the_organization() {
        let a = OrganizationId::new();
        let scope = SubscriptionScope::new(a, "sub-1");
        assert_eq!(scope.filter().organization_id, a);
        assert_eq!(scope.organization_id(), a);
        assert_eq!(scope.subscription_id(), "sub-1");
    }
}
