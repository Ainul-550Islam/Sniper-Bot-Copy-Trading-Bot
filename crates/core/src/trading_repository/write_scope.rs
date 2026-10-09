//! Write-side tenant scope (PROMPT 3/10 §A3).
//!
//! [`TenantWriteScope`] is REQUIRED for every tenant repository WRITE.
//! It exists so a mutation can never be tenant-blind:
//!
//! * the acting [`OrganizationId`] — the value the SQL predicate AND the
//!   inserted `organization_id` column bind;
//! * the ACTOR — the authenticated principal (user id, api-key label,
//!   telegram identity, `worker:<runtime>` for background jobs) that
//!   performed the mutation, for the audit trail;
//! * the ORIGIN — where the mutation entered the system.
//!
//! The scope is constructed by the server data plane AFTER
//! authentication; a blank actor is rejected fail-closed (an
//! unattributable write is a bug, not a convenience).

use crate::tenant::OrganizationId;

/// Where a tenant write entered the system.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum WriteOrigin {
    /// An authenticated HTTP control-plane request.
    Http,
    /// An authenticated WebSocket command.
    Stream,
    /// A tenant background job running under a fenced runtime.
    Job,
    /// A recovery/reconciliation sweep for the tenant.
    Recovery,
    /// An operator-authorized explicit action on behalf of the tenant.
    Operator,
}

impl WriteOrigin {
    /// Stable machine-readable label (metrics/audit).
    pub fn as_str(&self) -> &'static str {
        match self {
            WriteOrigin::Http => "http",
            WriteOrigin::Stream => "stream",
            WriteOrigin::Job => "job",
            WriteOrigin::Recovery => "recovery",
            WriteOrigin::Operator => "operator",
        }
    }
}

/// Why a write scope was rejected. Closed vocabulary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WriteScopeError {
    /// The acting principal is blank (unattributable write).
    BlankActor,
}

impl WriteScopeError {
    pub fn as_str(&self) -> &'static str {
        match self {
            WriteScopeError::BlankActor => "blank_actor",
        }
    }
}

impl std::fmt::Display for WriteScopeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            WriteScopeError::BlankActor => {
                write!(f, "tenant write scope: the acting principal is blank")
            }
        }
    }
}

impl std::error::Error for WriteScopeError {}

/// The scope a tenant trading WRITE executes under.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TenantWriteScope {
    organization_id: OrganizationId,
    actor: String,
    origin: WriteOrigin,
}

impl TenantWriteScope {
    /// Build and validate. Fails closed on a blank actor.
    pub fn new(
        organization_id: OrganizationId,
        actor: &str,
        origin: WriteOrigin,
    ) -> Result<Self, WriteScopeError> {
        if actor.trim().is_empty() {
            return Err(WriteScopeError::BlankActor);
        }
        Ok(TenantWriteScope {
            organization_id,
            actor: actor.trim().to_string(),
            origin,
        })
    }

    /// The acting organization — the value the write's SQL predicate and
    /// its inserted `organization_id` column bind.
    pub fn organization_id(&self) -> OrganizationId {
        self.organization_id
    }

    /// The authenticated principal that performs the write.
    pub fn actor(&self) -> &str {
        &self.actor
    }

    /// Where the write entered the system.
    pub fn origin(&self) -> WriteOrigin {
        self.origin
    }

    /// The read scope for the same tenant (writes may need to read).
    pub fn query_scope(&self) -> super::TradingQueryScope {
        super::TradingQueryScope::new(self.organization_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blank_actors_are_rejected_fail_closed() {
        let org = OrganizationId::new();
        for blank in ["", "   ", "\t"] {
            let err = TenantWriteScope::new(org, blank, WriteOrigin::Http).unwrap_err();
            assert_eq!(err.as_str(), "blank_actor");
        }
    }

    #[test]
    fn scope_carries_tenant_actor_and_origin() {
        let org = OrganizationId::new();
        let scope = TenantWriteScope::new(org, "user:1  ", WriteOrigin::Job).expect("valid actor");
        assert_eq!(scope.organization_id(), org);
        assert_eq!(scope.actor(), "user:1");
        assert_eq!(scope.origin().as_str(), "job");
        assert_eq!(scope.query_scope().organization_id(), org);
    }
}
