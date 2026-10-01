//! Tenant-scoped pagination (STEP 3 file 48).
//!
//! Keyset pagination for tenant listings, with two isolation rules the
//! generic cursor types of the world do not have:
//!
//! 1. The cursor embeds the organization id, and a cursor from tenant A
//!    can never be replayed against tenant B's request
//!    ([`TenantPageCursor::require_same_tenant`]).
//! 2. The page result exposes the NEXT cursor only — a client can walk
//!    forward inside one tenant, never sideways into another.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::error::{BotError, BotResult};
use crate::tenant::OrganizationId;

/// The hard page-size cap. Listings are paged to keep worst-case query
/// cost bounded no matter how large a tenant grows.
pub const MAX_PAGE_LIMIT: u32 = 200;
/// The default page size when the caller does not specify one.
pub const DEFAULT_PAGE_LIMIT: u32 = 50;

/// An opaque, tenant-bound keyset cursor.
///
/// Encoded form: the canonical JSON of `(organization_id, sort_key, at)`.
/// It is opaque to clients (they treat it as a string) but verifiable
/// server-side: the tenant is part of the signed-by-construction payload.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TenantPageCursor {
    /// The tenant whose listing this cursor walks. A cursor replayed
    /// against another tenant is rejected.
    pub organization_id: OrganizationId,
    /// The last sort key seen (its meaning is defined by the listing
    /// query: created_at, ts, id — stable and total per listing).
    pub sort_key: String,
    /// The tie-break timestamp the listing sorts by.
    pub at: DateTime<Utc>,
}

impl TenantPageCursor {
    /// Encode to the opaque client-facing string.
    pub fn encode(&self) -> String {
        serde_json::to_string(self).unwrap_or_default()
    }

    /// Decode from the opaque client-facing string. Malformed input is
    /// rejected (never guessed at).
    pub fn decode(encoded: &str) -> BotResult<Self> {
        serde_json::from_str(encoded).map_err(|e| BotError::db(format!("invalid page cursor: {e}")))
    }

    /// Fail-closed tenant check: this cursor may only walk the acting
    /// tenant's listing.
    pub fn require_same_tenant(&self, organization_id: OrganizationId) -> BotResult<()> {
        if self.organization_id == organization_id {
            Ok(())
        } else {
            Err(BotError::db(
                "page cursor rejected: it belongs to another organization",
            ))
        }
    }
}

/// A validated page request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TenantPageRequest {
    /// 1..=MAX_PAGE_LIMIT.
    pub limit: u32,
    /// Where to resume, if this is not the first page.
    pub cursor: Option<TenantPageCursor>,
}

impl TenantPageRequest {
    /// Validate a raw (limit, cursor) pair from a caller.
    pub fn new(
        organization_id: OrganizationId,
        limit: Option<u32>,
        cursor: Option<&str>,
    ) -> BotResult<Self> {
        let limit = limit.unwrap_or(DEFAULT_PAGE_LIMIT).clamp(1, MAX_PAGE_LIMIT);
        let cursor = match cursor {
            None => None,
            Some(encoded) => {
                let c = TenantPageCursor::decode(encoded)?;
                c.require_same_tenant(organization_id)?;
                Some(c)
            }
        };
        Ok(TenantPageRequest { limit, cursor })
    }
}

impl Default for TenantPageRequest {
    fn default() -> Self {
        TenantPageRequest {
            limit: DEFAULT_PAGE_LIMIT,
            cursor: None,
        }
    }
}

/// One page of results plus the continuation cursor.
#[derive(Debug, Clone, PartialEq)]
pub struct TenantPage<T> {
    /// The rows of this page (already tenant-verified by the repository).
    pub items: Vec<T>,
    /// The cursor to request the next page, `None` when exhausted.
    pub next: Option<TenantPageCursor>,
}

impl<T> TenantPage<T> {
    /// Build a page: when the query fetched `limit + 1` rows, the extra
    /// row is the continuation proof and is dropped from the items.
    pub fn from_fetched(
        organization_id: OrganizationId,
        mut items: Vec<T>,
        limit: u32,
        last_sort_key: impl Fn(&T) -> String,
        last_at: impl Fn(&T) -> DateTime<Utc>,
    ) -> Self {
        let has_more = items.len() as u32 > limit;
        if has_more {
            items.truncate(limit as usize);
        }
        let next = if has_more {
            items.last().map(|last| TenantPageCursor {
                organization_id,
                sort_key: last_sort_key(last),
                at: last_at(last),
            })
        } else {
            None
        };
        TenantPage { items, next }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn org() -> OrganizationId {
        OrganizationId::new()
    }

    #[test]
    fn cursor_round_trips_opaquely() {
        let c = TenantPageCursor {
            organization_id: org(),
            sort_key: "ord-123".into(),
            at: Utc::now(),
        };
        let encoded = c.encode();
        assert!(encoded.starts_with('{'), "encoded form is opaque JSON");
        assert_eq!(TenantPageCursor::decode(&encoded).unwrap(), c);
        assert!(TenantPageCursor::decode("not-json").is_err());
    }

    #[test]
    fn cursor_cannot_cross_tenants() {
        let a = org();
        let b = org();
        let c = TenantPageCursor {
            organization_id: a,
            sort_key: "k".into(),
            at: Utc::now(),
        };
        assert!(c.require_same_tenant(a).is_ok());
        assert!(c.require_same_tenant(b).is_err());
        // And the request builder enforces it too.
        let req = TenantPageRequest::new(b, None, Some(&c.encode()));
        assert!(req.is_err());
    }

    #[test]
    fn limits_are_clamped_into_range() {
        let o = org();
        assert_eq!(
            TenantPageRequest::new(o, None, None).unwrap().limit,
            DEFAULT_PAGE_LIMIT
        );
        assert_eq!(TenantPageRequest::new(o, Some(0), None).unwrap().limit, 1);
        assert_eq!(
            TenantPageRequest::new(o, Some(10_000), None).unwrap().limit,
            MAX_PAGE_LIMIT
        );
    }

    #[test]
    fn page_truncates_and_emits_next_only_when_more_exists() {
        let o = org();
        let rows = vec![("a", 1), ("b", 2), ("c", 3)];
        let page = TenantPage::from_fetched(
            o,
            rows.clone(),
            2,
            |r| r.0.to_string(),
            |r| Utc::now() + chrono::Duration::seconds(r.1),
        );
        assert_eq!(page.items.len(), 2);
        assert!(page.next.is_some());
        assert_eq!(page.next.unwrap().sort_key, "b");

        let small = vec![("a", 1)];
        let page = TenantPage::from_fetched(o, small, 2, |r| r.0.to_string(), |_| Utc::now());
        assert_eq!(page.items.len(), 1);
        assert!(page.next.is_none());
    }
}
