//! Common tenant-aware query scope (PROMPT 3/10 §A2).
//!
//! [`TradingQueryScope`] is the READ-side scope every tenant repository
//! query runs under. It wraps the STEP 3 [`TenantQueryScope`] (the
//! organization constraint) and adds the optional runtime fencing
//! identity: repository reads that belong to one runtime instance (a
//! worker's recovery sweep, a stream's continuation) carry the
//! [`RuntimeId`] so the caller can prove WHICH runtime of the tenant is
//! reading — the repository still constrains by organization first and
//! only, the runtime is carried for attribution and for the caller's own
//! fencing checks.
//!
//! Construction is only meaningful from an authenticated context; the
//! server data plane builds it after `authorize_request`. There is no
//! `TradingQueryScope::from_header`.

use crate::db::tenant_query::TenantQueryScope;
use crate::tenant::{OrganizationId, RuntimeId};

/// The scope a tenant trading READ executes under.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TradingQueryScope {
    inner: TenantQueryScope,
    runtime: Option<RuntimeId>,
}

impl TradingQueryScope {
    /// Build from the acting organization.
    pub fn new(organization_id: OrganizationId) -> Self {
        TradingQueryScope {
            inner: TenantQueryScope::new(organization_id),
            runtime: None,
        }
    }

    /// Attach the runtime identity the read is attributed to (optional;
    /// the organization constraint is unaffected).
    pub fn for_runtime(mut self, runtime: RuntimeId) -> Self {
        self.runtime = Some(runtime);
        self
    }

    /// The acting organization — the value every SQL predicate binds.
    pub fn organization_id(&self) -> OrganizationId {
        self.inner.organization_id()
    }

    /// The underlying STEP 3 scope (transaction/lock helpers take it).
    pub fn tenant_scope(&self) -> &TenantQueryScope {
        &self.inner
    }

    /// The runtime identity, when the read is runtime-attributed.
    pub fn runtime_id(&self) -> Option<RuntimeId> {
        self.runtime
    }

    /// The exact predicate fragment every scoped query embeds at bind
    /// position `bind_index` (value bound from [`Self::organization_id`]).
    pub fn org_predicate(&self, bind_index: usize) -> String {
        self.inner.org_predicate(bind_index)
    }
}

impl From<TenantQueryScope> for TradingQueryScope {
    fn from(inner: TenantQueryScope) -> Self {
        TradingQueryScope {
            inner,
            runtime: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scope_carries_the_tenant_and_optional_runtime() {
        let org = OrganizationId::new();
        let scope = TradingQueryScope::new(org);
        assert_eq!(scope.organization_id(), org);
        assert!(scope.runtime_id().is_none());

        let runtime = RuntimeId::new();
        let scoped = TradingQueryScope::new(org).for_runtime(runtime);
        assert_eq!(scoped.runtime_id(), Some(runtime));
        // The runtime never weakens the organization constraint.
        assert_eq!(scoped.org_predicate(1), "organization_id = $1");
    }
}
