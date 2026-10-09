//! Tenant query scope helpers (STEP 3 file 46).
//!
//! Every tenant repository query MUST constrain `organization_id`. This
//! module is the shared vocabulary for that constraint:
//!
//! * [`TenantQueryScope`] — the scope a query runs under;
//! * [`TenantQueryScope::org_predicate`] — the exact predicate fragment
//!   every scoped query embeds;
//! * [`require_org_predicate`] — a static assertion helper our query
//!   builders call so a query that forgot the predicate cannot ship.
//!
//! This is deliberately NOT a query builder: the existing repositories
//! (`db::repo`, `db::claims`, …) keep their hand-written SQL — these
//! helpers make the tenant constraint uniform, cheap to review and
//! impossible to forget in the new tenant paths.

use crate::error::{BotError, BotResult};
use crate::tenant::OrganizationId;

/// The scope a tenant query executes under.
///
/// Built from an authenticated tenant context (or the deployment context
/// for legacy/operator paths) — never from user input alone.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TenantQueryScope {
    organization_id: OrganizationId,
}

impl TenantQueryScope {
    /// Build from the acting tenant.
    pub fn new(organization_id: OrganizationId) -> Self {
        TenantQueryScope { organization_id }
    }

    /// The acting tenant.
    pub fn organization_id(&self) -> OrganizationId {
        self.organization_id
    }

    /// The column every scoped query filters on.
    pub fn org_column(&self) -> &'static str {
        "organization_id"
    }

    /// The exact predicate fragment (bind position supplied by the
    /// caller, value bound from [`TenantQueryScope::organization_id`]).
    pub fn org_predicate(&self, bind_index: usize) -> String {
        format!("organization_id = ${bind_index}")
    }

    /// Append the org predicate to an existing `WHERE` clause list.
    ///
    /// `parts` are the predicates already collected; this returns the
    /// next bind index so callers can chain safely.
    pub fn push_org_predicate(&self, parts: &mut Vec<String>, next_bind: usize) -> usize {
        parts.push(self.org_predicate(next_bind));
        next_bind + 1
    }
}

/// Static guard for our own query builders: the SQL text must mention the
/// tenant column. This is a belt-and-braces check for NEW tenant queries
/// (it cannot parse SQL and does not try to); its job is to make a
/// forgotten predicate a loud, immediate error in review and tests
/// rather than a silent cross-tenant read.
///
/// Note: a query that merely MENTIONS the column without constraining it
/// would still pass — the authoritative isolation is the guard chain plus
/// the per-query predicates written against [`TenantQueryScope`].
pub fn require_org_predicate(sql: &str) -> BotResult<()> {
    let lowered = sql.to_ascii_lowercase();
    if lowered.contains("organization_id") {
        Ok(())
    } else {
        Err(BotError::db(
            "tenant query rejected: the SQL text does not constrain organization_id",
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scope_carries_the_tenant() {
        let org = OrganizationId::new();
        let s = TenantQueryScope::new(org);
        assert_eq!(s.organization_id(), org);
        assert_eq!(s.org_column(), "organization_id");
    }

    #[test]
    fn predicate_uses_caller_supplied_bind_position() {
        let s = TenantQueryScope::new(OrganizationId::new());
        assert_eq!(s.org_predicate(1), "organization_id = $1");
        assert_eq!(s.org_predicate(4), "organization_id = $4");
    }

    #[test]
    fn push_appends_and_advances_the_bind_index() {
        let s = TenantQueryScope::new(OrganizationId::new());
        let mut parts = vec!["status = $1".to_string()];
        let next = s.push_org_predicate(&mut parts, 2);
        assert_eq!(next, 3);
        assert_eq!(parts, vec!["status = $1", "organization_id = $2"]);
    }

    #[test]
    fn require_org_predicate_accepts_and_rejects() {
        assert!(require_org_predicate("SELECT * FROM orders WHERE organization_id = $1").is_ok());
        assert!(require_org_predicate("SELECT * FROM orders WHERE id = $1").is_err());
        // Case-insensitive on purpose.
        assert!(require_org_predicate("... ORGANIZATION_ID IS NOT NULL ...").is_ok());
    }
}
