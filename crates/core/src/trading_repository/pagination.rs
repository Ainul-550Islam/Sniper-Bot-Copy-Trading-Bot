//! Tenant-scoped pagination primitives (PROMPT 3/10 §A8).
//!
//! Re-exports the STEP 3 keyset types ([`TenantPageCursor`],
//! [`TenantPageRequest`], [`TenantPage`]) — the cursor carries the
//! organization and [`TenantPageRequest::new`] REFUSES a cursor minted
//! for another tenant — and adds the one SQL fragment every
//! repository list query shares: the (organization_id, sort-key, ts)
//! keyset `WHERE` clause builder [`page_where_clause`].
//!
//! Keyset contract used by every list query in this module:
//!
//! ```sql
//! WHERE organization_id = $1
//!   AND (updated_at, id) < ($2, $3)   -- keyset page, opaque to callers
//! ORDER BY updated_at DESC, id DESC
//! LIMIT $4 + 1                          -- continuation proof
//! ```
//!
//! The cursor is opaque (base64 JSON), tenant-bound and carries no ids
//! of other tenants.

pub use crate::db::tenant_pagination::{
    TenantPage, TenantPageCursor, TenantPageRequest, DEFAULT_PAGE_LIMIT, MAX_PAGE_LIMIT,
};

/// The keyset continuation predicate shared by the list queries.
///
/// Bind order for a first page (no cursor):
/// `$1 = organization_id`. For a continuation page:
/// `$1 = organization_id, $2 = cursor.at, $3 = cursor.sort_key`.
/// The caller appends its own predicates AFTER this fragment and binds
/// in the documented order.
pub const PAGE_KEYSET_WHERE: &str = "(updated_at, id) < ($2, $3)";

/// The full first-page predicate: the tenant constraint only.
///
/// `bind_index` is the position `organization_id` occupies in the
/// caller's bind list (always 1 in this module's queries).
pub fn page_where_clause(bind_index: usize) -> String {
    format!("organization_id = ${bind_index}")
}

/// Clamp a caller-supplied limit into the validated page range
/// (1..=[`MAX_PAGE_LIMIT`]) without needing a full page request.
pub fn clamp_limit(limit: i64) -> i64 {
    limit.clamp(1, MAX_PAGE_LIMIT as i64)
}

/// Fetch window: one extra row is the has-more proof consumed by
/// [`TenantPage::from_fetched`].
pub fn fetch_window(limit: i64) -> i64 {
    clamp_limit(limit) + 1
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tenant::OrganizationId;

    #[test]
    fn fragments_name_the_tenant_column() {
        assert_eq!(page_where_clause(1), "organization_id = $1");
        assert!(PAGE_KEYSET_WHERE.starts_with("(updated_at, id) <"));
    }

    #[test]
    fn limits_clamp_into_the_valid_window() {
        assert_eq!(clamp_limit(0), 1);
        assert_eq!(clamp_limit(-5), 1);
        assert_eq!(clamp_limit(25), 25);
        assert_eq!(clamp_limit(10_000), MAX_PAGE_LIMIT as i64);
        assert_eq!(fetch_window(25), 26);
    }

    #[test]
    fn cursors_never_cross_tenants() {
        let a = OrganizationId::new();
        let b = OrganizationId::new();
        let req = TenantPageRequest::new(a, Some(10), None).unwrap();
        assert_eq!(req.limit, 10);
        let cursor = TenantPageCursor {
            organization_id: a,
            sort_key: "ord-1".into(),
            at: chrono::Utc::now(),
        };
        let encoded = cursor.encode();
        // Tenant A can resume from its own cursor…
        assert!(TenantPageRequest::new(a, None, Some(&encoded)).is_ok());
        // …tenant B cannot (fail closed, no cross-tenant pagination).
        let err = TenantPageRequest::new(b, None, Some(&encoded)).unwrap_err();
        assert!(err.to_string().contains("another organization"));
    }
}
