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
//! | `postgres.rs` | PostgreSQL runtime-record repository and atomic plan assignment |
//! | `store.rs` | PostgreSQL-authoritative production store with an in-memory test mode |
//!
//! # What this boundary must never do
//!
//! It decides WHO may ask. It cannot approve a trade: after it allows a
//! request, the TASK 5 global risk engine still runs and the TASK 6 lease
//! fencing still applies. It also never becomes a second source of truth
//! for orders, fills, positions, risk, the ledger, HA leases or feed
//! cursors — those stay in TASK 1–6.

pub mod activity;
pub mod alerts;
pub mod api_keys;
pub mod audit_export;
pub mod backup_status;
pub mod billing;
pub mod billing_reconciliation;
pub mod billing_status;
pub mod billing_view;
pub mod billing_webhook;
pub mod checkout;
pub mod commercial_state;
pub mod custody;
pub mod custody_health;
pub mod custody_rotation;
pub mod custody_rotation_store;
pub mod data_lifecycle;
pub mod export;
pub mod feature_catalog;
pub mod invoices;
pub mod middleware;
pub mod notifications;
pub mod openapi;
pub mod organizations;
pub mod payment_webhooks;
pub mod portfolio;
pub mod postgres;
pub mod pricing;
pub mod provider;
pub mod readiness;
pub mod reports;
pub mod risk_dashboard;
pub mod security;
pub mod security_summary;
pub mod status;
pub mod store;
pub mod support;
pub mod team;
pub mod tenant_lifecycle;
pub mod usage_limits;
pub mod users;
pub mod wallet_access;
pub mod webhooks;
pub mod websocket_auth;
pub mod websocket_replay_store;

#[allow(unused_imports)]
pub use middleware::{authorize_request, deny_response, SaasContext, DEPLOYMENT_ORG_SLUG};
pub use store::SaasStore;

use axum::routing::{delete, get, patch, post};
use axum::Router;
use chrono::Utc;

use bot_core::billing::PlanCode;
use bot_core::error::BotResult;
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
        .route("/api/saas/api-keys/:prefix", delete(api_keys::revoke_key))
        // --- TASK 7B: the public contract --------------------------------
        .route("/api/saas/openapi.json", get(openapi::serve))
        // --- TASK 7B: verified, idempotent provider webhooks -------------
        .merge(billing_webhook::routes())
        // --- TASK 7B: the tenant→wallet→strategy boundary ----------------
        .merge(wallet_access::routes())
        // --- TASK 7B: deterministic tenant-scoped exports ----------------
        .merge(export::routes())
        // --- TASK 7B: the authenticated tenant-scoped event stream -------
        .merge(crate::security::websocket::routes())
        // --- BATCH: billing, checkout, invoices, custody, lifecycle -----
        .merge(checkout::routes())
        .merge(invoices::routes())
        .merge(payment_webhooks::routes())
        .merge(custody::routes())
        .merge(tenant_lifecycle::routes())
        // --- BATCH 2: reconciliation, health, audit export, data lifecycle, WS auth ---
        .merge(billing_reconciliation::routes())
        .merge(custody_health::routes())
        .merge(audit_export::routes())
        .merge(data_lifecycle::routes())
        // --- BATCH 3: commercial, readiness, usage, rotation ---
        .merge(billing_status::routes())
        .merge(usage_limits::routes())
        .merge(custody_rotation::routes())
        .merge(readiness::routes())
        .merge(commercial_state::routes())
        // --- BATCH 4: security summary + backup status ---
        .merge(security_summary::routes())
        .merge(backup_status::routes())
        // --- SECOND.md §85-§89: team, security, webhooks, reports, support ---
        .merge(team::routes())
        .merge(security::routes())
        .merge(webhooks::routes())
        .merge(reports::routes())
        .merge(support::routes())
        // --- THIRD.md §133-§141: portfolio, risk, alerts, status, pricing, notifications, activity ---
        .merge(portfolio::routes())
        .merge(risk_dashboard::routes())
        .merge(alerts::routes())
        .merge(status::routes())
        .merge(pricing::routes())
        .merge(notifications::routes())
        .merge(activity::routes())
}

/// Resolve the user behind a presented session token, without requiring a
/// tenant context.
///
/// Used by the two endpoints that exist BEFORE a tenant does: creating the
/// first organization, and reading one's own profile right after signup.
/// Everything else goes through [`middleware::authorize_request`].
pub async fn session_user(
    state: &ApiState,
    headers: &axum::http::HeaderMap,
) -> BotResult<Option<User>> {
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
        .filter(|v| !v.is_empty());
    let Some(presented) = presented else {
        return Ok(None);
    };

    let Some(record) = state.saas.session_by_hash(&hash_token(&presented)).await? else {
        return Ok(None);
    };
    let now = Utc::now();
    let Ok(validated) = bot_core::session::validate(Some(&record), None, now) else {
        return Ok(None);
    };
    let Some(user) = state.saas.user(validated.user_id).await? else {
        return Ok(None);
    };
    Ok(user.can_authenticate().then_some(user))
}

/// Ensure the deployment organization exists.
///
/// A single-tenant install has no signup flow, but the SaaS API still needs
/// a tenant to resolve to. This creates (once) the organization a legacy
/// deployment key maps to, on the Business plan so every module the
/// operator already runs stays enabled. Idempotent: calling it twice
/// returns the existing row.
///
/// The Business plan is ensured for a PRE-EXISTING organization too:
/// migration 0024 backfills a legacy database by creating the deployment
/// organization from SQL (same slug `deployment`, same name `Deployment`,
/// active), and a deployment organization without a subscription would
/// fail every plan-entitlement check once the runtime entitlement gates
/// land. An operator's later plan change is never clobbered: the plan is
/// only assigned when the organization has no subscription at all.
pub async fn ensure_deployment_organization(store: &SaasStore) -> BotResult<Option<Organization>> {
    let org = match store.organization_by_slug(DEPLOYMENT_ORG_SLUG).await? {
        Some(existing) => existing,
        None => {
            let now = Utc::now();
            let org = Organization::new(
                OrganizationId::new(),
                DEPLOYMENT_ORG_SLUG,
                "Deployment",
                None,
                now,
            );
            store.create_organization(&org).await?;
            org
        }
    };
    // Business tier: the deployment operator already had every module.
    if store.subscription_of(org.id).await?.is_none() {
        store
            .assign_plan(org.id, PlanCode::Business, Utc::now())
            .await?;
    }
    Ok(Some(org))
}

/// Attach an owner membership for a bootstrap user (used by tests and by
/// operators seeding the first account).
pub async fn attach_owner(
    store: &SaasStore,
    organization_id: OrganizationId,
    user: &User,
) -> BotResult<Option<Membership>> {
    if let Some(existing) = store.membership(organization_id, user.id).await? {
        return Ok(Some(existing));
    }
    let m = Membership::new(
        organization_id,
        user.id,
        MembershipRole::OrgOwner,
        None,
        Utc::now(),
    );
    store.create_membership(&m).await?;
    Ok(Some(m))
}
