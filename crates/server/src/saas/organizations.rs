//! Organization (tenant) lifecycle (TASK 7A file 24).
//!
//! Creating a tenant, reading it, updating it, listing its members, and the
//! provisioning entry point that turns a signup into a fully configured
//! organization.
//!
//! Every read and write is scoped by the AUTHENTICATED context, never by an
//! organization id taken from the request body or path. A caller may name
//! their own organization; naming somebody else's produces
//! `DENY_RESOURCE`, the same answer whatever role they hold, so the API
//! does not reveal whether that organization exists.

use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use chrono::Utc;
use serde::Deserialize;
use serde_json::json;

use bot_core::authorization::AccessRequest;
use bot_core::billing::{features, PlanCode};
use bot_core::membership::Permission;
use bot_core::provisioning::{plan_next, ProvisioningAction, ProvisioningJob, ProvisioningStep};
use bot_core::tenant::{Organization, OrganizationId, OrganizationStatus};

use super::middleware::{authorize_request, deny_response};
use crate::api::ApiState;

/// Turn a display name into a URL-safe slug.
pub fn slugify(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    let mut last_dash = true;
    for ch in name.trim().chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
            last_dash = false;
        } else if !last_dash {
            out.push('-');
            last_dash = true;
        }
    }
    let s = out.trim_matches('-').to_string();
    if s.is_empty() {
        "org".into()
    } else {
        s.chars().take(48).collect()
    }
}

/// Creation body.
#[derive(Debug, Deserialize)]
pub struct CreateOrganizationBody {
    /// Display name.
    pub name: String,
    /// Optional explicit slug.
    #[serde(default)]
    pub slug: Option<String>,
    /// Plan to start on (defaults to `starter`).
    #[serde(default)]
    pub plan: Option<String>,
}

/// `POST /api/saas/organizations` — create a tenant for the authenticated
/// user, who becomes its owner.
///
/// This runs the provisioning state machine, so a crash halfway leaves a
/// resumable job rather than a half-built tenant.
pub async fn create_organization(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Json(body): Json<CreateOrganizationBody>,
) -> Response {
    // Creating an organization needs an authenticated USER, but not an
    // existing tenant — so this endpoint authenticates the session directly
    // rather than through the tenant-scoped middleware.
    let user = match super::session_user(&state, &headers).await {
        Ok(Some(value)) => value,
        Ok(None) => {
            return (
                StatusCode::UNAUTHORIZED,
                Json(json!({ "error": "deny_unauthenticated" })),
            )
                .into_response();
        }
        Err(error) => {
            tracing::error!(error = %error, "organization creation identity lookup failed");
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(json!({
                    "error": "identity_storage_unavailable",
                    "reason": "identity records could not be loaded",
                })),
            )
                .into_response();
        }
    };

    let plan = body
        .plan
        .as_deref()
        .and_then(PlanCode::parse)
        .unwrap_or(PlanCode::Starter);
    let slug = body
        .slug
        .as_deref()
        .map(slugify)
        .unwrap_or_else(|| slugify(&body.name));
    if body.name.trim().is_empty() {
        return (StatusCode::BAD_REQUEST, "name must not be empty").into_response();
    }

    let now = Utc::now();
    let request_key = ProvisioningJob::request_key_for(&user.email, &slug);
    let mut job = match state
        .saas
        .upsert_job(&ProvisioningJob::new(
            request_key.clone(),
            plan.as_str(),
            now,
        ))
        .await
    {
        Ok(job) => job,
        Err(error) => {
            return (StatusCode::SERVICE_UNAVAILABLE, error.to_string()).into_response();
        }
    };
    // The user already exists (they are authenticated), so that step is done.
    job.user_id = Some(user.id);
    if job.step == ProvisioningStep::Signup {
        job.complete_step(ProvisioningStep::UserCreated, now);
    }

    let organization = match run_provisioning(&state, &mut job, &body.name, &slug, plan, now).await
    {
        Ok(o) => o,
        Err(resp) => return resp,
    };

    state
        .audit
        .success(
            &format!("user:{}", user.id),
            "saas.organization.created",
            Some(&organization.id.to_string()),
        )
        .await;
    (
        StatusCode::CREATED,
        Json(json!({
            "organization": organization,
            "provisioning": {
                "state": job.state.as_str(),
                "step": job.step.as_str(),
                "ready": job.is_ready(),
            },
        })),
    )
        .into_response()
}

/// Execute the remaining provisioning steps. Each step is idempotent, so a
/// resumed job converges instead of duplicating.
#[allow(clippy::result_large_err)]
async fn run_provisioning(
    state: &ApiState,
    job: &mut ProvisioningJob,
    name: &str,
    slug: &str,
    plan: PlanCode,
    now: chrono::DateTime<Utc>,
) -> Result<Organization, Response> {
    let user_id = job
        .user_id
        .ok_or_else(|| (StatusCode::BAD_REQUEST, "provisioning has no user").into_response())?;
    job.begin(now);

    while let ProvisioningAction::Execute(step) = plan_next(job) {
        match step {
            ProvisioningStep::UserCreated => {
                // Already true at this entry point.
                job.complete_step(step, now);
            }
            ProvisioningStep::OrganizationCreated => {
                // Idempotent: reuse the row if a previous attempt created it.
                let org = match state.saas.organization_by_slug(slug).await {
                    Ok(Some(o)) => o,
                    Ok(None) => {
                        let o = Organization::new(
                            OrganizationId::new(),
                            slug,
                            name,
                            Some(user_id),
                            now,
                        );
                        if let Err(e) = state.saas.create_organization(&o).await {
                            job.record_failure(e.to_string(), now);
                            let _ = state.saas.update_job(job).await;
                            return Err((StatusCode::CONFLICT, e.to_string()).into_response());
                        }
                        o
                    }
                    Err(error) => {
                        tracing::error!(error = %error, slug, "provisioning organization lookup failed");
                        job.record_failure("organization lookup unavailable", now);
                        let _ = state.saas.update_job(job).await;
                        return Err((
                            StatusCode::SERVICE_UNAVAILABLE,
                            "organization lookup unavailable",
                        )
                            .into_response());
                    }
                };
                job.organization_id = Some(org.id);
                job.complete_step(step, now);
            }
            ProvisioningStep::MembershipCreated => {
                let org_id = job.organization_id.expect("set by the previous step");
                let membership = match state.saas.membership(org_id, user_id).await {
                    Ok(value) => value,
                    Err(error) => {
                        tracing::error!(error = %error, organization = %org_id, user = %user_id, "provisioning membership lookup failed");
                        job.record_failure("membership lookup unavailable", now);
                        let _ = state.saas.update_job(job).await;
                        return Err((
                            StatusCode::SERVICE_UNAVAILABLE,
                            "membership lookup unavailable",
                        )
                            .into_response());
                    }
                };
                if membership.is_none() {
                    let user = match state.saas.user(user_id).await {
                        Ok(Some(value)) => value,
                        Ok(None) => {
                            job.record_failure("provisioning user vanished", now);
                            let _ = state.saas.update_job(job).await;
                            return Err((
                                StatusCode::INTERNAL_SERVER_ERROR,
                                "provisioning user vanished",
                            )
                                .into_response());
                        }
                        Err(error) => {
                            tracing::error!(error = %error, user = %user_id, "provisioning user lookup failed");
                            job.record_failure("user lookup unavailable", now);
                            let _ = state.saas.update_job(job).await;
                            return Err((
                                StatusCode::SERVICE_UNAVAILABLE,
                                "user lookup unavailable",
                            )
                                .into_response());
                        }
                    };
                    match super::attach_owner(&state.saas, org_id, &user).await {
                        Ok(Some(_)) => {}
                        Ok(None) => {
                            job.record_failure("could not create owner membership", now);
                            let _ = state.saas.update_job(job).await;
                            return Err((
                                StatusCode::CONFLICT,
                                "could not create owner membership",
                            )
                                .into_response());
                        }
                        Err(error) => {
                            tracing::error!(error = %error, organization = %org_id, user = %user.id, "owner membership persistence failed");
                            job.record_failure("owner membership storage unavailable", now);
                            let _ = state.saas.update_job(job).await;
                            return Err((
                                StatusCode::SERVICE_UNAVAILABLE,
                                "owner membership storage unavailable",
                            )
                                .into_response());
                        }
                    }
                }
                job.complete_step(step, now);
            }
            ProvisioningStep::PlanAssigned => {
                let org_id = job.organization_id.expect("set by an earlier step");
                let subscription = match state.saas.subscription_of(org_id).await {
                    Ok(value) => value,
                    Err(error) => {
                        tracing::error!(error = %error, organization = %org_id, "provisioning subscription lookup failed");
                        job.record_failure("subscription lookup unavailable", now);
                        let _ = state.saas.update_job(job).await;
                        return Err((
                            StatusCode::SERVICE_UNAVAILABLE,
                            "subscription lookup unavailable",
                        )
                            .into_response());
                    }
                };
                if subscription.is_none() {
                    if let Err(e) = state.saas.assign_plan(org_id, plan, now).await {
                        job.record_failure(e.to_string(), now);
                        let _ = state.saas.update_job(job).await;
                        return Err(
                            (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response()
                        );
                    }
                }
                job.complete_step(step, now);
            }
            ProvisioningStep::DefaultConfiguration => {
                // Entitlements were written with the plan; nothing else to
                // do in TASK 7A. The step still exists so the sequence is
                // complete and a future default-config action has a home.
                job.complete_step(step, now);
            }
            ProvisioningStep::Ready => {
                job.complete_step(step, now);
            }
            ProvisioningStep::Signup => {
                job.complete_step(step, now);
            }
        }
        let _ = state.saas.update_job(job).await;
    }

    let org_id = job.organization_id.ok_or_else(|| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            "provisioning produced no organization",
        )
            .into_response()
    })?;
    match state.saas.organization(org_id).await {
        Ok(Some(value)) => Ok(value),
        Ok(None) => {
            Err((StatusCode::INTERNAL_SERVER_ERROR, "organization vanished").into_response())
        }
        Err(error) => {
            tracing::error!(error = %error, organization = %org_id, "provisioned organization could not be loaded");
            Err((
                StatusCode::SERVICE_UNAVAILABLE,
                "organization lookup unavailable",
            )
                .into_response())
        }
    }
}

/// `GET /api/saas/organizations/:id` — read one tenant. The id must be the
/// caller's own (or they must be platform staff).
pub async fn get_organization(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    let Some(requested) = OrganizationId::parse(&id) else {
        return (StatusCode::BAD_REQUEST, "invalid organization id").into_response();
    };
    let ctx = match authorize_request(
        &state,
        &headers,
        AccessRequest::read(Permission::TenantRead).on_resource(requested),
    )
    .await
    {
        Ok(c) => c,
        Err(d) => return deny_response(&state, &d).await,
    };
    // Platform staff may read another tenant; everyone else only their own.
    let org = if requested == ctx.organization.id {
        ctx.organization.clone()
    } else {
        match state.saas.organization(requested).await {
            Ok(Some(o)) => o,
            Ok(None) => return (StatusCode::NOT_FOUND, "no such organization").into_response(),
            Err(error) => {
                tracing::error!(error = %error, organization = %requested, "organization could not be loaded");
                return (
                    StatusCode::SERVICE_UNAVAILABLE,
                    "organization lookup unavailable",
                )
                    .into_response();
            }
        }
    };
    let subscription = match state.saas.subscription_of(org.id).await {
        Ok(value) => value,
        Err(error) => {
            tracing::error!(error = %error, organization = %org.id, "organization subscription could not be loaded");
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(json!({
                    "error": "billing_storage_unavailable",
                    "reason": "authoritative subscription could not be loaded",
                })),
            )
                .into_response();
        }
    };
    let entitlements = match state.saas.entitlements_of(org.id, Utc::now()).await {
        Ok(value) => value,
        Err(error) => {
            tracing::error!(error = %error, organization = %org.id, "organization entitlements could not be loaded");
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(json!({
                    "error": "billing_storage_unavailable",
                    "reason": "authoritative entitlements could not be loaded",
                })),
            )
                .into_response();
        }
    };
    Json(json!({
        "organization": org,
        "subscription": subscription,
        "entitlements": entitlements.rows().collect::<Vec<_>>(),
        "features": {
            "live_trading": entitlements.allows(features::LIVE_TRADING),
            "api_keys": entitlements.allows(features::API_KEYS),
            "exports": entitlements.allows(features::EXPORTS),
        },
    }))
    .into_response()
}

/// Update body. Only the display name is self-service; status changes are
/// a platform action (see [`suspend_organization`]).
#[derive(Debug, Deserialize)]
pub struct UpdateOrganizationBody {
    /// New display name.
    pub name: String,
}

/// `PATCH /api/saas/organizations/:id` — rename a tenant.
pub async fn update_organization(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(body): Json<UpdateOrganizationBody>,
) -> Response {
    let Some(requested) = OrganizationId::parse(&id) else {
        return (StatusCode::BAD_REQUEST, "invalid organization id").into_response();
    };
    let ctx = match authorize_request(
        &state,
        &headers,
        AccessRequest::manage(Permission::TenantUpdate).on_resource(requested),
    )
    .await
    {
        Ok(c) => c,
        Err(d) => return deny_response(&state, &d).await,
    };
    if body.name.trim().is_empty() {
        return (StatusCode::BAD_REQUEST, "name must not be empty").into_response();
    }
    let mut org = match state.saas.organization(requested).await {
        Ok(Some(o)) => o,
        Ok(None) => return (StatusCode::NOT_FOUND, "no such organization").into_response(),
        Err(error) => {
            tracing::error!(error = %error, organization = %requested, "organization could not be loaded for update");
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                "organization lookup unavailable",
            )
                .into_response();
        }
    };
    org.name = body.name.trim().to_string();
    org.updated_at = Utc::now();
    if let Err(e) = state.saas.update_organization(&org).await {
        return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response();
    }
    state
        .audit
        .success(
            &ctx.actor_label(),
            "saas.organization.updated",
            Some(&org.id.to_string()),
        )
        .await;
    Json(json!({ "organization": org })).into_response()
}

/// `GET /api/saas/organizations/:id/members` — list members.
pub async fn list_members(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    let requested = if id == "current" {
        let ctx = match authorize_request(
            &state,
            &headers,
            AccessRequest::read_only(Permission::UsersRead),
        )
        .await
        {
            Ok(c) => c,
            Err(d) => return deny_response(&state, &d).await,
        };
        ctx.organization.id
    } else {
        let Some(req) = OrganizationId::parse(&id) else {
            return (StatusCode::BAD_REQUEST, "invalid organization id").into_response();
        };
        let ctx = match authorize_request(
            &state,
            &headers,
            AccessRequest::read(Permission::UsersRead).on_resource(req),
        )
        .await
        {
            Ok(c) => c,
            Err(d) => return deny_response(&state, &d).await,
        };
        let _ = &ctx;
        req
    };

    let members = match state.saas.members(requested).await {
        Ok(value) => value,
        Err(error) => {
            tracing::error!(error = %error, organization = %requested, "organization members could not be loaded");
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(json!({
                    "error": "membership_storage_unavailable",
                    "reason": "authoritative membership records could not be loaded",
                })),
            )
                .into_response();
        }
    };
    let mut out = Vec::new();
    for m in members {
        let profile = match state.saas.user(m.user_id).await {
            Ok(value) => value.map(|u| u.profile()),
            Err(error) => {
                tracing::error!(error = %error, user = %m.user_id, "organization member profile could not be loaded");
                return (
                    StatusCode::SERVICE_UNAVAILABLE,
                    Json(json!({
                        "error": "user_storage_unavailable",
                        "reason": "authoritative member profiles could not be loaded",
                    })),
                )
                    .into_response();
            }
        };
        out.push(json!({
            "membership_id": m.id,
            "user": profile,
            "role": m.role.as_str(),
            "status": m.status.as_str(),
            "created_at": m.created_at,
        }));
    }
    Json(json!({ "count": out.len(), "members": out, "items": out })).into_response()
}

/// Suspension body.
#[derive(Debug, Deserialize)]
pub struct SuspendBody {
    /// `true` suspends, `false` reactivates.
    pub suspended: bool,
    /// Operator reason.
    #[serde(default)]
    pub reason: Option<String>,
}

/// `POST /api/saas/organizations/:id/suspension` — platform staff only.
///
/// A tenant cannot suspend itself out of trouble, and cannot un-suspend
/// itself: the endpoint requires genuine platform scope.
pub async fn suspend_organization(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(body): Json<SuspendBody>,
) -> Response {
    let Some(requested) = OrganizationId::parse(&id) else {
        return (StatusCode::BAD_REQUEST, "invalid organization id").into_response();
    };
    let ctx = match authorize_request(
        &state,
        &headers,
        // Reading is enough at the permission level; platform scope is the
        // real gate below. Naming the resource here also lets the middleware
        // resolve the path tenant without trusting a separate header.
        AccessRequest::read(Permission::TenantRead).on_resource(requested),
    )
    .await
    {
        Ok(c) => c,
        Err(d) => return deny_response(&state, &d).await,
    };
    if !ctx.authorization.is_platform_scope() {
        return deny_response(
            &state,
            &bot_core::authorization::Decision::role(
                "changing a tenant's suspension requires platform scope",
            ),
        )
        .await;
    }
    let mut org = match state.saas.organization(requested).await {
        Ok(Some(o)) => o,
        Ok(None) => return (StatusCode::NOT_FOUND, "no such organization").into_response(),
        Err(error) => {
            tracing::error!(error = %error, organization = %requested, "organization could not be loaded for suspension");
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                "organization lookup unavailable",
            )
                .into_response();
        }
    };
    let now = Utc::now();
    if body.suspended {
        org.status = OrganizationStatus::Suspended;
        org.suspended_at = Some(now);
        org.suspend_reason = body.reason.clone().unwrap_or_else(|| "suspended".into());
    } else {
        org.status = OrganizationStatus::Active;
        org.suspended_at = None;
        org.suspend_reason.clear();
    }
    org.updated_at = now;
    if let Err(e) = state.saas.update_organization(&org).await {
        return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response();
    }
    state
        .audit
        .success(
            &ctx.actor_label(),
            if body.suspended {
                "saas.organization.suspended"
            } else {
                "saas.organization.reactivated"
            },
            Some(&org.id.to_string()),
        )
        .await;
    Json(json!({ "organization": org })).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slugs_are_url_safe_and_stable() {
        assert_eq!(slugify("Acme Capital"), "acme-capital");
        assert_eq!(slugify("  ACME  Capital  "), "acme-capital");
        assert_eq!(slugify("Acme, Inc."), "acme-inc");
        assert_eq!(slugify("...."), "org");
        assert_eq!(slugify(""), "org");
        assert_eq!(slugify("a"), "a");
        assert!(slugify(&"x".repeat(100)).len() <= 48);
        // Deterministic.
        assert_eq!(slugify("Acme Capital"), slugify("Acme Capital"));
    }
}
