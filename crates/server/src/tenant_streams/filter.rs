//! Subscription filters with mandatory tenant scoping (STEP 3 file 48).
//!
//! The filter is the security boundary of the stream layer: a receiver
//! NEVER sees an event outside its organization, regardless of what
//! optional refinements it asked for. Optional filters (event kind,
//! module) can only NARROW the delivery.

use bot_core::tenant::OrganizationId;

use super::events::TenantEvent;

/// What a consumer wants to receive.
#[derive(Debug, Clone, PartialEq)]
pub struct TenantStreamFilter {
    /// The ONLY organization this consumer may receive. Mandatory —
    /// there is deliberately no "all tenants" scope.
    pub organization_id: OrganizationId,
    /// Only these event kinds (None = all kinds).
    pub kinds: Option<Vec<&'static str>>,
    /// Only these modules (None = all modules; ignored for events
    /// without a module field).
    pub modules: Option<Vec<&'static str>>,
}

impl TenantStreamFilter {
    /// Everything for one tenant.
    pub fn everything_for(organization_id: OrganizationId) -> Self {
        TenantStreamFilter {
            organization_id,
            kinds: None,
            modules: None,
        }
    }

    /// Narrow to specific event kinds.
    pub fn with_kinds(mut self, kinds: Vec<&'static str>) -> Self {
        self.kinds = Some(kinds);
        self
    }

    /// Narrow to specific modules.
    pub fn with_modules(mut self, modules: Vec<&'static str>) -> Self {
        self.modules = Some(modules);
        self
    }

    /// May this event be delivered under this filter? The organization
    /// check comes FIRST and cannot be widened.
    pub fn accepts(&self, event: &TenantEvent) -> bool {
        // Mandatory scope: hard equality, no exceptions, no admin bypass
        // (platform admins read through the observability module, not
        // through a tenant stream).
        if event.organization_id() != self.organization_id {
            return false;
        }
        if let Some(kinds) = &self.kinds {
            if !kinds.contains(&event.kind()) {
                return false;
            }
        }
        if let Some(modules) = &self.modules {
            if let Some(module) = event_module(event) {
                if !modules.contains(&module) {
                    return false;
                }
            }
        }
        true
    }
}

fn event_module(event: &TenantEvent) -> Option<&'static str> {
    match event {
        TenantEvent::Decision { module, .. } | TenantEvent::ExecutionStarted { module, .. } => {
            Some(module)
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decision(org: OrganizationId, module: &'static str) -> TenantEvent {
        TenantEvent::Decision {
            organization_id: org,
            decision: "allow",
            module,
            mode: "paper",
            origin: "req",
        }
    }

    #[test]
    fn another_tenants_event_is_never_accepted() {
        let mine = OrganizationId::new();
        let theirs = OrganizationId::new();
        let filter = TenantStreamFilter::everything_for(mine);
        assert!(!filter.accepts(&decision(theirs, "copy")));
        assert!(filter.accepts(&decision(mine, "copy")));
    }

    #[test]
    fn kind_and_module_filters_only_narrow() {
        let org = OrganizationId::new();
        let filter = TenantStreamFilter::everything_for(org)
            .with_kinds(vec!["decision"])
            .with_modules(vec!["copy"]);

        assert!(filter.accepts(&decision(org, "copy")));
        assert!(!filter.accepts(&decision(org, "sniper")));
        assert!(!filter.accepts(&TenantEvent::ConfigChanged {
            organization_id: org,
            version: 1,
            changes: 0,
        }));
    }

    #[test]
    fn events_without_a_module_pass_module_filters() {
        let org = OrganizationId::new();
        let filter = TenantStreamFilter::everything_for(org).with_modules(vec!["copy"]);
        // RuntimeChanged has no module: the module filter cannot apply.
        assert!(filter.accepts(&TenantEvent::RuntimeChanged {
            organization_id: org,
            change: "rotated",
            generation: 2,
        }));
    }
}
