//! Tenant row mapping/validation helpers (STEP 3 file 47).
//!
//! Rows that carry `organization_id` implement [`TenantRow`], and every
//! read path that hands a row to a caller verifies it against the acting
//! [`TenantQueryScope`] via [`require_same_row_ownership`] — a cross-tenant
//! row can then never leave the repository layer, even if a query
//! predicate were wrong.

use crate::error::{BotError, BotResult};
use crate::tenant::OrganizationId;

use super::tenant_query::TenantQueryScope;

/// A row that belongs to a tenant.
pub trait TenantRow {
    /// The owning organization of this row.
    fn organization_id(&self) -> OrganizationId;
}

/// Fail-closed ownership check: the row must belong to the query scope's
/// tenant. The error message is deliberately identical for "other
/// tenant's row" and "no row at all" situations the caller cannot
/// distinguish — existence is not leaked.
pub fn require_same_row_ownership(scope: &TenantQueryScope, row: &impl TenantRow) -> BotResult<()> {
    if row.organization_id() == scope.organization_id() {
        Ok(())
    } else {
        Err(BotError::db(
            "row not visible: it belongs to another organization",
        ))
    }
}

/// Fail-closed ownership check on a raw organization id (for rows not
/// yet mapped into a [`TenantRow`] impl, e.g. inside a `sqlx` map
/// closure).
pub fn require_same_org(
    scope: &TenantQueryScope,
    row_organization_id: OrganizationId,
) -> BotResult<()> {
    if row_organization_id == scope.organization_id() {
        Ok(())
    } else {
        Err(BotError::db(
            "row not visible: it belongs to another organization",
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A representative trading row: a primary key plus the owning
    /// organization. The `id` is not part of the tenancy surface under
    /// test — it exists to keep the fixture shaped like a real row.
    #[allow(dead_code)]
    struct OrderRow {
        id: u64,
        organization_id: OrganizationId,
    }

    impl TenantRow for OrderRow {
        fn organization_id(&self) -> OrganizationId {
            self.organization_id
        }
    }

    #[test]
    fn own_rows_pass() {
        let org = OrganizationId::new();
        let scope = TenantQueryScope::new(org);
        let row = OrderRow {
            id: 1,
            organization_id: org,
        };
        assert!(require_same_row_ownership(&scope, &row).is_ok());
        assert!(require_same_org(&scope, org).is_ok());
    }

    #[test]
    fn foreign_rows_fail_closed_with_no_existence_leak() {
        let scope = TenantQueryScope::new(OrganizationId::new());
        let row = OrderRow {
            id: 2,
            organization_id: OrganizationId::new(),
        };
        let a = require_same_row_ownership(&scope, &row)
            .unwrap_err()
            .to_string();
        let b = require_same_org(&scope, row.organization_id)
            .unwrap_err()
            .to_string();
        assert_eq!(a, b, "the message must not depend on which check ran");
        assert!(
            !a.contains("another organization id"),
            "no ids in errors: {a}"
        );
    }
}
