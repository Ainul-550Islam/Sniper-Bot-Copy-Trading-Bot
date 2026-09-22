//! The SaaS control-plane boundary (TASK 7A file 21).
//!
//! Everything multi-tenant lives under `/api/saas/*` and behind
//! [`middleware::authorize_request`]. The existing trading API
//! (`/api/status`, `/api/orders`, `/api/ha`, …) is untouched: it keeps its
//! deployment-key gate, so an existing single-operator install upgrades
//! without changing a single call.
//!
//! | file | concern |
//! |---|---|
//! | `middleware.rs` | authenticate → resolve tenant → role → permission → entitlement; the DENY rules |
//! | `users.rs` | register, login, current user, safe profile updates, logout |
//! | `organizations.rs` | create (through the provisioning state machine), read, update, members, suspension |
//! | `api_keys.rs` | tenant-scoped keys: create (secret shown once), list, revoke, restart-safe lookup |
//! | `store.rs` | the in-process control-plane store with the durable semantics of migration 0017 |
//!
//! # What this boundary must never do
//!
//! It decides WHO may ask. It cannot approve a trade: after it allows a
//! request, the TASK 5 global risk engine still runs and the TASK 6 lease
//! fencing still applies. It also never becomes a second source of truth
//! for orders, fills, positions, risk, the ledger, HA leases or feed
//! cursors — those stay in TASK 1–6.

pub mod api_keys;
pub mod middleware;
pub mod organizations;
pub mod store;
pub mod users;

#[allow(unused_imports)]
pub use middleware::{authorize_request, deny_response, SaasContext, DEPLOYMENT_ORG_SLUG};
pub use store::SaasStore;

use axum::routing::{delete, get, patch, post};
use axum::Router;
use chrono::Utc;

use bot_core::billing::PlanCode;
use bot_core::membership::{Membership, MembershipRole};
use bot_core::session::hash_token;
use bot_core::tenant::{Organization, OrganizationId, User};

use crate::api::ApiState;

/// Mount the SaaS routes. Called by [`crate::api::router`], so the control
/// plane is part of the same server, rate limiter and audit trail.
pub fn routes() -> Router<ApiState> {
    Router::new()
        // --- identity (public entry points) -----------------------------
        .route("/api/saas/users", post(users::register))
        .route("/api/saas/sessions", post(users::login))
        // --- authenticated user -----------------------------------------
        .route("/api/saas/users/me", get(users::current_user))
        .route("/api/saas/users/me", patch(users::update_profile))
        .route("/api/saas/users/me/logout", post(users::logout))
        // --- organizations ----------------------------------------------
        .route(
            "/api/saas/organizations",
            post(organizations::create_organization),
        )
        .route(
            "/api/saas/organizations/:id",
            get(organizations::get_organization),
        )
        .route(
            "/api/saas/organizations/:id",
            patch(organizations::update_organization),
        )
        .route(
            "/api/saas/organizations/:id/members",
            get(organizations::list_members),
        )
        .route(
            "/api/saas/organizations/:id/suspension",
            post(organizations::suspend_organization),
        )
        // --- tenant API keys ---------------------------------------------
        .route("/api/saas/api-keys", post(api_keys::create_key))
        .route("/api/saas/api-keys", get(api_keys::list_keys))
        .route(
            "/api/saas/api-keys/:prefix",
            delete(api_keys::revoke_key),
        )
}

/// Resolve the user behind a presented session token, without requiring a
/// tenant context.
///
/// Used by the two endpoints that exist BEFORE a tenant does: creating the
/// first organization, and reading one's own profile right after signup.
/// Everything else goes through [`middleware::authorize_request`].
pub async fn session_user(state: &ApiState, headers: &axum::http::HeaderMap) -> Option<User> {
    let presented = headers
        .get(middleware::AUTH_HEADER)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| {
            let v = v.trim();
            v.strip_prefix("Bearer ")
                .or_else(|| v.strip_prefix("bearer "))
                .map(|s| s.trim().to_string())
        })
        .or_else(|| {
            headers
                .get(middleware::API_KEY_HEADER)
                .and_then(|v| v.to_str().ok())
                .map(|v| v.trim().to_string())
        })
        .filter(|v| !v.is_empty())?;

    let record = state.saas.session_by_hash(&hash_token(&presented)).await?;
    let now = Utc::now();
    let validated = bot_core::session::validate(Some(&record), None, now).ok()?;
    let user = state.saas.user(validated.user_id).await?;
    user.can_authenticate().then_some(user)
}

/// Ensure the deployment organization exists.
///
/// A single-tenant install has no signup flow, but the SaaS API still needs
/// a tenant to resolve to. This creates (once) the organization a legacy
/// deployment key maps to, on the Business plan so every module the
/// operator already runs stays enabled. Idempotent: calling it twice
/// returns the existing row.
pub async fn ensure_deployment_organization(state: &ApiState) -> Option<Organization> {
    if let Some(existing) = state.saas.organization_by_slug(DEPLOYMENT_ORG_SLUG).await {
        return Some(existing);
    }
    let now = Utc::now();
    let org = Organization::new(
        OrganizationId::new(),
        DEPLOYMENT_ORG_SLUG,
        "Deployment",
        None,
        now,
    );
    state.saas.create_organization(&org).await.ok()?;
    // Business tier: the deployment operator already had every module.
    let _ = state
        .saas
        .assign_plan(org.id, PlanCode::Business, now)
        .await;
    Some(org)
}

/// Attach an owner membership for a bootstrap user (used by tests and by
/// operators seeding the first account).
pub async fn attach_owner(
    state: &ApiState,
    organization_id: OrganizationId,
    user: &User,
) -> Option<Membership> {
    if let Some(existing) = state.saas.membership(organization_id, user.id).await {
        return Some(existing);
    }
    let m = Membership::new(
        organization_id,
        user.id,
        MembershipRole::OrgOwner,
        None,
        Utc::now(),
    );
    state.saas.create_membership(&m).await.ok()?;
    Some(m)
}
