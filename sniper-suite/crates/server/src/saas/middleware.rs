//! Mandatory tenant authorization (TASK 7A file 22).
//!
//! Every tenant-owned request goes through [`authorize_request`]:
//!
//! ```text
//! request
//!   → authenticate        (session cookie/header | tenant API key | legacy key)
//!   → resolve user
//!   → resolve organization
//!   → resolve membership
//!   → resolve role + effective permissions (role ∩ key scopes)
//!   → bot_core::authorization::authorize  (ownership → tenant state → permission → entitlement)
//!   → handler
//! ```
//!
//! Three rules are absolute:
//!
//! * **Missing tenant context is a DENY.** There is no default
//!   organization, and a handler cannot run without a [`SaasContext`].
//! * **A caller-supplied organization id never widens access.** The
//!   requested tenant must match the credential's tenant (or the caller
//!   must be a genuine platform admin), otherwise `DENY_RESOURCE`.
//! * **A suspended tenant cannot trade or manage**, whatever its role.
//!
//! This is server-side enforcement in the request path — not UI filtering.

use axum::http::HeaderMap;
use axum::response::{IntoResponse, Response};
use axum::Json;
use chrono::Utc;
use serde_json::json;
use tracing::warn;

use bot_core::authorization::{
    authorize, AccessRequest, AuthorizationContext, Decision, Principal,
};
use bot_core::membership::{Membership, MembershipRole};
use bot_core::session::{self, hash_token};
use bot_core::tenant::{Organization, OrganizationId, User};

use super::api_keys::{principal_for, SaasApiKey};
use super::store::SaasStore;
use crate::api::ApiState;

/// Header carrying a tenant API key or a session token.
pub const AUTH_HEADER: &str = "authorization";

/// Legacy header the existing deployment API already accepts.
pub const API_KEY_HEADER: &str = "x-api-key";

/// Optional header naming the tenant a multi-organization user is acting
/// for. It can only ever SELECT among the caller's own memberships.
pub const ORG_HEADER: &str = "x-organization";

/// Everything a SaaS handler needs after the middleware ran.
#[derive(Debug, Clone)]
pub struct SaasContext {
    /// The authorization input (principal, tenant, role, permissions).
    pub authorization: AuthorizationContext,
    /// The resolved tenant record.
    pub organization: Organization,
    /// The human, when the credential has one.
    pub user: Option<User>,
    /// The API key, when the credential was one.
    pub api_key: Option<SaasApiKey>,
}

impl SaasContext {
    /// Non-secret actor label for the audit trail.
    pub fn actor_label(&self) -> String {
        if let Some(key) = &self.api_key {
            return key.summary();
        }
        format!(
            "{}:{}",
            self.authorization.principal.kind(),
            self.authorization.principal.identifier()
        )
    }

    /// The tenant this request acts for.
    pub fn organization_id(&self) -> OrganizationId {
        self.organization.id
    }
}

/// Extract the presented credential: `Authorization: Bearer …`, or the
/// legacy `x-api-key` header.
fn presented_credential(headers: &HeaderMap) -> Option<String> {
    if let Some(v) = headers.get(AUTH_HEADER).and_then(|v| v.to_str().ok()) {
        let v = v.trim();
        if let Some(rest) = v
            .strip_prefix("Bearer ")
            .or_else(|| v.strip_prefix("bearer "))
        {
            if !rest.trim().is_empty() {
                return Some(rest.trim().to_string());
            }
        }
    }
    headers
        .get(API_KEY_HEADER)
        .and_then(|v| v.to_str().ok())
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

/// The organization the caller asked to act for, if any.
fn requested_organization(headers: &HeaderMap) -> Option<OrganizationId> {
    headers
        .get(ORG_HEADER)
        .and_then(|v| v.to_str().ok())
        .and_then(OrganizationId::parse)
}

/// Authenticate and build the tenant context, or return the refusal.
///
/// The `request` is evaluated here too, so a handler cannot forget to call
/// the decision function after obtaining the context.
pub async fn authorize_request(
    state: &ApiState,
    headers: &HeaderMap,
    request: AccessRequest<'_>,
) -> Result<SaasContext, Decision> {
    let now = Utc::now();
    let Some(presented) = presented_credential(headers) else {
        return Err(Decision::unauthenticated("no credential presented"));
    };
    let requested_org = requested_organization(headers);

    // ---- 1. A tenant API key? ------------------------------------------
    if let Some(key) = super::api_keys::resolve_key(state, &presented).await {
        if let Some(reason) = key.rejection(now) {
            return Err(Decision::unauthenticated(reason));
        }
        let Some(org) = state.saas.organization(key.organization_id).await else {
            return Err(Decision::tenant("the key's organization no longer exists"));
        };
        if !bot_core::tenant::can_authenticate(org.status) {
            return Err(Decision::tenant("the organization is closed"));
        }
        // A caller-supplied organization may only CONFIRM the key's tenant.
        if let Some(req) = requested_org {
            if req != org.id {
                return Err(Decision::resource(
                    "the key does not belong to the requested organization",
                ));
            }
        }
        let scopes = SaasStore::scopes_of(&key);
        let ctx = AuthorizationContext::from_api_key(
            principal_for(&key),
            &org,
            key.role,
            scopes.as_ref(),
            key.created_by,
            now,
        );
        let entitlements = state.saas.entitlements_of(org.id, now).await;
        let decision = authorize(Some(&ctx), &request, Some(&entitlements));
        if !decision.is_allowed() {
            return Err(decision);
        }
        // Record last use (best effort; never blocks the request).
        let mut used = key.clone();
        used.last_used_at = Some(now);
        let _ = state.saas.update_api_key(&used).await;
        let user = match key.created_by {
            Some(u) => state.saas.user(u).await,
            None => None,
        };
        return Ok(SaasContext {
            authorization: ctx,
            organization: org,
            user,
            api_key: Some(key),
        });
    }

    // ---- 2. A user session? --------------------------------------------
    if let Some(record) = state.saas.session_by_hash(&hash_token(&presented)).await {
        let validated = session::validate(Some(&record), None, now)
            .map_err(|r| Decision::unauthenticated(r.as_str()))?;
        let Some(user) = state.saas.user(validated.user_id).await else {
            return Err(Decision::unauthenticated("session user no longer exists"));
        };
        if !user.can_authenticate() {
            return Err(Decision::unauthenticated("the account is not active"));
        }

        // The explicit header wins; otherwise a path/resource owner supplied
        // by the handler wins; finally use the session's selected tenant.
        // This order lets platform staff operate on a path-named tenant while
        // ordinary users still have to prove membership in that exact tenant.
        let target = requested_org
            .or(request.resource_owner)
            .or(validated.organization_id);
        let Some(target) = target else {
            return Err(Decision::tenant(
                "no organization context: select an organization first",
            ));
        };
        let Some(org) = state.saas.organization(target).await else {
            // Keep the response indistinguishable from a missing membership
            // for ordinary callers; platform staff may receive the same safe
            // resource refusal without confirming existence.
            return Err(Decision::resource(
                "no access to the requested organization",
            ));
        };
        if !bot_core::tenant::can_authenticate(org.status) {
            return Err(Decision::tenant("the organization is closed"));
        }
        let principal = Principal::UserSession {
            session_id: validated.id.to_string(),
        };
        let ctx = if user.platform_admin {
            AuthorizationContext::from_platform_admin(principal, &org, user.id, now)
        } else {
            let Some(membership) = state.saas.membership(target, user.id).await else {
                // No membership: this is a cross-tenant attempt, and the
                // answer must not reveal whether the organization exists.
                return Err(Decision::resource(
                    "no membership in the requested organization",
                ));
            };
            build_context(principal, &org, &membership, &user, now)
        };
        let entitlements = state.saas.entitlements_of(org.id, now).await;
        let decision = authorize(Some(&ctx), &request, Some(&entitlements));
        if !decision.is_allowed() {
            return Err(decision);
        }
        let mut touched = record.clone();
        touched.touch(now);
        let _ = state.saas.update_session(&touched).await;
        return Ok(SaasContext {
            authorization: ctx,
            organization: org,
            user: Some(user),
            api_key: None,
        });
    }

    // ---- 3. A legacy deployment key? -----------------------------------
    // Single-tenant operators keep working: the deployment credential acts
    // for the deployment's own organization when one exists. It never
    // reaches another tenant, because it resolves to exactly one.
    if let Some(auth) = &state.auth {
        if let Some(principal) = auth.authenticate(&presented).await {
            let Some(org) = state.saas.organization_by_slug(DEPLOYMENT_ORG_SLUG).await else {
                return Err(Decision::tenant(
                    "this deployment has no organization; create one to use the SaaS API",
                ));
            };
            if let Some(req) = requested_org {
                if req != org.id {
                    return Err(Decision::resource(
                        "the deployment key does not belong to the requested organization",
                    ));
                }
            }
            let role = SaasStore::legacy_role(principal.role);
            let ctx = AuthorizationContext::from_api_key(
                Principal::LegacyDeploymentKey {
                    label: principal.label.clone(),
                },
                &org,
                role,
                None,
                None,
                now,
            );
            let entitlements = state.saas.entitlements_of(org.id, now).await;
            let decision = authorize(Some(&ctx), &request, Some(&entitlements));
            if !decision.is_allowed() {
                return Err(decision);
            }
            return Ok(SaasContext {
                authorization: ctx,
                organization: org,
                user: None,
                api_key: None,
            });
        }
    }

    Err(Decision::unauthenticated("credential not recognised"))
}

/// The slug the single-tenant deployment organization uses. A legacy
/// deployment key maps to this organization and no other.
pub const DEPLOYMENT_ORG_SLUG: &str = "deployment";

/// Build the context for a human membership.
fn build_context(
    principal: Principal,
    org: &Organization,
    membership: &Membership,
    user: &User,
    now: chrono::DateTime<Utc>,
) -> AuthorizationContext {
    AuthorizationContext::from_membership(
        principal,
        org,
        membership,
        None,
        user.platform_admin,
        now,
    )
}

/// Turn a refusal into an HTTP response, auditing the security-relevant
/// ones. The body never says whether a resource exists in another tenant.
pub async fn deny_response(state: &ApiState, decision: &Decision) -> Response {
    if decision.kind.is_security_relevant() {
        warn!(decision = %decision.summary(), "saas authorization denied");
        state
            .audit
            .record(
                "saas",
                "saas.authorization.denied",
                Some(decision.kind.as_str()),
                bot_core::audit::AuditOutcome::Denied,
                json!({ "reason": decision.reason }),
            )
            .await;
    }
    let status = axum::http::StatusCode::from_u16(decision.http_status())
        .unwrap_or(axum::http::StatusCode::FORBIDDEN);
    (
        status,
        Json(json!({
            "error": decision.kind.as_str(),
            "reason": decision.reason,
        })),
    )
        .into_response()
}

/// Require a role at least as strong as `minimum` — used by the few
/// endpoints that gate on seniority rather than on a single permission
/// (e.g. changing another member's role).
pub fn require_role_at_least(ctx: &SaasContext, minimum: MembershipRole) -> Result<(), Decision> {
    // `MembershipRole` is ordered strongest-first, so "at least" means a
    // lower or equal ordinal.
    if ctx.authorization.role <= minimum {
        Ok(())
    } else {
        Err(Decision::role(format!(
            "role {} is weaker than the required {}",
            ctx.authorization.role, minimum
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    fn headers(pairs: &[(&str, &str)]) -> HeaderMap {
        let mut h = HeaderMap::new();
        for (k, v) in pairs {
            h.insert(
                axum::http::HeaderName::from_bytes(k.as_bytes()).unwrap(),
                HeaderValue::from_str(v).unwrap(),
            );
        }
        h
    }

    #[test]
    fn credentials_are_read_from_both_header_styles() {
        assert_eq!(
            presented_credential(&headers(&[("authorization", "Bearer sk_abc")])),
            Some("sk_abc".to_string())
        );
        assert_eq!(
            presented_credential(&headers(&[("authorization", "bearer sk_abc")])),
            Some("sk_abc".to_string())
        );
        assert_eq!(
            presented_credential(&headers(&[("x-api-key", "legacy-key")])),
            Some("legacy-key".to_string())
        );
        // A bearer header wins when both are present.
        assert_eq!(
            presented_credential(&headers(&[
                ("authorization", "Bearer sk_abc"),
                ("x-api-key", "legacy")
            ])),
            Some("sk_abc".to_string())
        );
        assert_eq!(presented_credential(&HeaderMap::new()), None);
        assert_eq!(
            presented_credential(&headers(&[("authorization", "Bearer   ")])),
            None
        );
        assert_eq!(presented_credential(&headers(&[("x-api-key", "  ")])), None);
        // A non-bearer Authorization scheme is not a credential here.
        assert_eq!(
            presented_credential(&headers(&[("authorization", "Basic abc")])),
            None
        );
    }

    #[test]
    fn the_organization_header_is_parsed_strictly() {
        let id = OrganizationId::new();
        assert_eq!(
            requested_organization(&headers(&[("x-organization", &id.to_string())])),
            Some(id)
        );
        assert_eq!(
            requested_organization(&headers(&[("x-organization", "not-a-uuid")])),
            None
        );
        assert_eq!(requested_organization(&HeaderMap::new()), None);
    }
}
