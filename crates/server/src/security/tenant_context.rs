//! Central reusable tenant-context extraction/validation (BATCH file 18).
//!
//! Resolve authenticated user/session/API-key -> organization -> membership -> authorization context.
//! Eliminate repeated ad-hoc organization parsing. Never trust arbitrary organization header without auth.
//! Ensure closed/suspended tenants handled consistently.

use axum::http::HeaderMap;

use bot_core::authorization::{AuthorizationContext, Decision};
use bot_core::membership::Permission;
use bot_core::tenant::{Organization, OrganizationId};

use crate::api::ApiState;
use crate::saas::middleware::{authorize_request, ORG_HEADER};

/// The resolved tenant context for a request — HTTP or WebSocket.
#[derive(Debug, Clone)]
pub struct TenantContext {
    pub organization: Organization,
    pub organization_id: OrganizationId,
    pub authorization: AuthorizationContext,
    pub is_platform_admin: bool,
}

impl TenantContext {
    pub fn organization_id(&self) -> OrganizationId {
        self.organization_id
    }
    pub fn is_closed(&self) -> bool {
        self.organization.status == bot_core::tenant::OrganizationStatus::Closed
    }
    pub fn is_suspended(&self) -> bool {
        self.organization.status == bot_core::tenant::OrganizationStatus::Suspended
    }
}

/// Extract and validate tenant context from headers.
/// Uses existing middleware::authorize_request internally — this is the single reusable entrypoint.
/// `permission` is the permission required for the endpoint; use TenantRead for read endpoints etc.
/// Never trusts x-organization header without membership verification (enforced by authorize_request).
pub async fn resolve_tenant_context(
    state: &ApiState,
    headers: &HeaderMap,
    permission: Permission,
) -> Result<TenantContext, Decision> {
    // Use a read permission by default for context extraction; the caller can re-check with stricter AccessRequest if needed.
    let request = bot_core::authorization::AccessRequest::read(permission);
    let ctx = authorize_request(state, headers, request).await?;
    Ok(TenantContext {
        organization: ctx.organization.clone(),
        organization_id: ctx.organization.id,
        authorization: ctx.authorization.clone(),
        is_platform_admin: ctx.authorization.is_platform_scope(),
    })
}

/// Validate that the organization header, if present, matches the resolved context or is allowed via platform admin.
/// Returns error if header tries to override to another org without membership.
pub fn validate_organization_header(
    headers: &HeaderMap,
    resolved: &TenantContext,
) -> Result<(), Decision> {
    if let Some(raw) = headers.get(ORG_HEADER).and_then(|v| v.to_str().ok()) {
        if let Some(requested) = OrganizationId::parse(raw) {
            if requested != resolved.organization_id && !resolved.is_platform_admin {
                return Err(Decision::resource(
                    "x-organization header does not match authenticated tenant",
                ));
            }
        } else {
            // Malformed header — ignore rather than allow bypass; but we could also deny
            // For strictness, deny malformed header when it was explicitly supplied
            return Err(Decision::resource("invalid x-organization header"));
        }
    }
    Ok(())
}

/// Is the tenant allowed to trade? Closed/suspended cannot.
pub fn ensure_trading_allowed(ctx: &TenantContext) -> Result<(), Decision> {
    if ctx.is_closed() {
        return Err(Decision::suspended(
            "organization is closed; trading not allowed",
        ));
    }
    if ctx.is_suspended() {
        return Err(Decision::suspended(
            "organization is suspended; trading not allowed",
        ));
    }
    Ok(())
}

/// Cross-tenant object check: does the object's organization_id match the resolved context?
/// Fails closed if mismatch — never leaks existence, just denies.
pub fn ensure_same_tenant(
    ctx: &TenantContext,
    object_organization_id: OrganizationId,
) -> Result<(), Decision> {
    if object_organization_id != ctx.organization_id && !ctx.is_platform_admin {
        return Err(Decision::resource(
            "resource belongs to another organization",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use bot_core::authorization::{AuthorizationContext, Principal};
    use bot_core::membership::MembershipRole;
    use bot_core::tenant::{Organization, OrganizationId, OrganizationStatus, UserId};
    use chrono::Utc;

    use axum::http::{HeaderMap, HeaderValue};

    fn make_context(
        org_id: OrganizationId,
        status: OrganizationStatus,
        is_admin: bool,
    ) -> TenantContext {
        let org = {
            let mut o = Organization::new(org_id, "test", "Test", Some(UserId::new()), Utc::now());
            o.status = status;
            o
        };
        let principal = Principal::UserSession {
            session_id: "sess".into(),
        };
        let auth = if is_admin {
            AuthorizationContext::from_platform_admin(principal, &org, UserId::new(), Utc::now())
        } else {
            // create a mock membership
            let m = bot_core::membership::Membership::new(
                org_id,
                UserId::new(),
                MembershipRole::OrgOwner,
                None,
                Utc::now(),
            );
            AuthorizationContext::from_membership(principal, &org, &m, None, false, Utc::now())
        };
        TenantContext {
            organization: org,
            organization_id: org_id,
            authorization: auth,
            is_platform_admin: is_admin,
        }
    }

    #[test]
    fn untrusted_header_cannot_override_tenant() {
        let org_a = OrganizationId::new();
        let org_b = OrganizationId::new();
        let ctx = make_context(org_a, OrganizationStatus::Active, false);
        let mut headers = HeaderMap::new();
        headers.insert(
            ORG_HEADER,
            HeaderValue::from_str(&org_b.to_string()).unwrap(),
        );
        let err = validate_organization_header(&headers, &ctx).unwrap_err();
        assert_eq!(
            err.kind,
            bot_core::authorization::DecisionKind::DenyResource
        );
    }

    #[test]
    fn platform_admin_may_use_header() {
        let org_a = OrganizationId::new();
        let org_b = OrganizationId::new();
        let ctx = make_context(org_a, OrganizationStatus::Active, true);
        let mut headers = HeaderMap::new();
        headers.insert(
            ORG_HEADER,
            HeaderValue::from_str(&org_b.to_string()).unwrap(),
        );
        assert!(
            validate_organization_header(&headers, &ctx).is_ok(),
            "platform admin may act for another org"
        );
    }

    #[test]
    fn closed_tenant_blocked_for_trading() {
        let org = OrganizationId::new();
        let ctx = make_context(org, OrganizationStatus::Closed, false);
        assert!(ensure_trading_allowed(&ctx).is_err());
        let ctx2 = make_context(org, OrganizationStatus::Active, false);
        assert!(ensure_trading_allowed(&ctx2).is_ok());
    }

    #[test]
    fn cross_tenant_object_fails_closed() {
        let org_a = OrganizationId::new();
        let org_b = OrganizationId::new();
        let ctx = make_context(org_a, OrganizationStatus::Active, false);
        // Object belongs to B, context is A → deny
        let err = ensure_same_tenant(&ctx, org_b).unwrap_err();
        assert_eq!(
            err.kind,
            bot_core::authorization::DecisionKind::DenyResource
        );
        // Same tenant → allow
        assert!(ensure_same_tenant(&ctx, org_a).is_ok());
        // Platform admin may access cross-tenant
        let admin_ctx = make_context(org_a, OrganizationStatus::Active, true);
        assert!(ensure_same_tenant(&admin_ctx, org_b).is_ok());
    }

    #[test]
    fn malformed_header_is_rejected() {
        let org = OrganizationId::new();
        let ctx = make_context(org, OrganizationStatus::Active, false);
        let mut headers = HeaderMap::new();
        headers.insert(ORG_HEADER, HeaderValue::from_str("not-a-uuid").unwrap());
        assert!(validate_organization_header(&headers, &ctx).is_err());
    }

    #[test]
    fn no_header_is_ok() {
        let org = OrganizationId::new();
        let ctx = make_context(org, OrganizationStatus::Active, false);
        let headers = HeaderMap::new();
        assert!(validate_organization_header(&headers, &ctx).is_ok());
    }
}
