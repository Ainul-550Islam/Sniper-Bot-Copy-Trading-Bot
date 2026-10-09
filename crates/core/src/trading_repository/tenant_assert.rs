//! Runtime row-ownership assertions (PROMPT 3/10 §A7).
//!
//! The SQL predicates already constrain every query by
//! `organization_id`; these helpers are the SECOND layer — they prove
//! that rows RETURNED by a query carry the acting tenant's id, so a
//! future query regression (a forgotten predicate, a bad join) fails
//! loudly as [`RepositoryError::TenantMismatch`] instead of silently
//! leaking another tenant's data to the caller.
//!
//! Every repository `get`/`list` funnels its rows through
//! [`assert_row_org`] / [`assert_rows_org`].

use crate::tenant::OrganizationId;

use super::repository_error::RepositoryError;

/// The ownership contract a returned row must satisfy.
pub trait OwnedRow {
    /// The organization the row belongs to (the persisted
    /// `organization_id` column).
    fn row_organization_id(&self) -> OrganizationId;
}

/// Assert one returned row belongs to the acting tenant.
pub fn assert_row_org<R: OwnedRow>(acting: OrganizationId, row: &R) -> Result<(), RepositoryError> {
    if row.row_organization_id() == acting {
        Ok(())
    } else {
        Err(RepositoryError::TenantMismatch)
    }
}

/// Assert EVERY returned row belongs to the acting tenant (list paths).
pub fn assert_rows_org<R: OwnedRow>(
    acting: OrganizationId,
    rows: &[R],
) -> Result<(), RepositoryError> {
    for row in rows {
        assert_row_org(acting, row)?;
    }
    Ok(())
}

/// Filter-free verification for `Option` results: `None` passes (the
/// caller maps it through `not_found`), `Some(row)` must be owned.
pub fn assert_optional_row_org<R: OwnedRow>(
    acting: OrganizationId,
    row: &Option<R>,
) -> Result<(), RepositoryError> {
    if let Some(r) = row {
        assert_row_org(acting, r)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Row(OrganizationId);

    impl OwnedRow for Row {
        fn row_organization_id(&self) -> OrganizationId {
            self.0
        }
    }

    #[test]
    fn owned_rows_pass() {
        let org = OrganizationId::new();
        assert!(assert_row_org(org, &Row(org)).is_ok());
        assert!(assert_rows_org(org, &[Row(org), Row(org)]).is_ok());
        assert!(assert_optional_row_org(org, &Some(Row(org))).is_ok());
        assert!(assert_optional_row_org(org, &None::<Row>).is_ok());
    }

    #[test]
    fn foreign_rows_fail_as_tenant_mismatch() {
        let org = OrganizationId::new();
        let other = OrganizationId::new();
        let err = assert_row_org(org, &Row(other)).unwrap_err();
        assert!(matches!(err, RepositoryError::TenantMismatch));
        // A single foreign row in a list poisons the whole list.
        let err = assert_rows_org(org, &[Row(org), Row(other)]).unwrap_err();
        assert!(matches!(err, RepositoryError::TenantMismatch));
        let err = assert_optional_row_org(org, &Some(Row(other))).unwrap_err();
        assert!(matches!(err, RepositoryError::TenantMismatch));
    }
}
