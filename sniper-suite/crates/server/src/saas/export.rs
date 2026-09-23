//! Deterministic, tenant-scoped data exports (TASK 7B file 15).
//!
//! `GET /api/saas/exports?kind=…` answers one tenant's own data, in a
//! byte-stable shape (BTreeMaps, sorted vectors, stable field order), so
//! the same tenant state always produces the same export. Sections:
//!
//! | kind          | requires                          | contents |
//! |---------------|-----------------------------------|----------|
//! | `profile`     | export.create                     | the caller's user record and memberships |
//! | `members`     | export.create ∧ users.read        | the tenant's member list |
//! | `api_keys`    | export.create ∧ users.read        | API-key METADATA only (prefix/label/role) — never hashes or secrets |
//! | `usage`       | export.create ∧ billing.read      | the current month's usage totals |
//! | `subscription`| export.create ∧ billing.read      | the tenant's subscription and plan reference |
//! | `wallets`     | export.create ∧ wallet.read       | the tenant's wallet bindings (public data) |
//! | `audit`       | export.create ∧ audit.read        | audit records targeting this tenant |
//!
//! Never exported: session rows, password/token/key hashes, provider
//! webhook secrets, other tenants' anything. Every export is audited.

use std::collections::BTreeMap;

use axum::extract::{Query, State};
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Response};
use axum::{Json, Router};
use chrono::Utc;
use serde_json::{json, Value};

use bot_core::authorization::AccessRequest;
use bot_core::billing::UsageMetric;
use bot_core::membership::Permission;

use crate::api::ApiState;
use crate::saas::middleware::{authorize_request, deny_response};
use crate::saas::wallet_access::WalletRegistry;

/// The export sections, with the extra permission each one needs beyond
/// `export.create`.
pub const SECTIONS: [(&str, Option<Permission>); 7] = [
    ("profile", None),
    ("members", Some(Permission::UsersRead)),
    ("api_keys", Some(Permission::UsersRead)),
    ("usage", Some(Permission::BillingRead)),
    ("subscription", Some(Permission::BillingRead)),
    ("wallets", Some(Permission::WalletRead)),
    ("audit", Some(Permission::AuditRead)),
];

/// How many audit rows one export may carry.
const AUDIT_EXPORT_LIMIT: usize = 500;

fn unknown_kind() -> Response {
    (
        axum::http::StatusCode::UNPROCESSABLE_ENTITY,
        Json(json!({ "error": "unknown_export_kind", "known": SECTIONS.map(|(k, _)| k) })),
    )
        .into_response()
}

/// One section's payload. Every map is a BTreeMap and every list is sorted,
/// so the same tenant state yields the same bytes.
async fn section(
    state: &ApiState,
    kind: &str,
    ctx_org: bot_core::tenant::OrganizationId,
    ctx_user: Option<bot_core::tenant::UserId>,
) -> Value {
    let now = Utc::now();
    match kind {
        "profile" => {
            let user = match ctx_user.map(|id| state.saas.user(id)) {
                Some(fut) => fut.await,
                None => None,
            };
            let mut memberships = match ctx_user.map(|id| state.saas.memberships_of_user(id)) {
                Some(fut) => fut.await,
                None => Vec::new(),
            };
            // An org-scoped export carries only THIS tenant's membership row.
            memberships.retain(|m| m.organization_id == ctx_org);
            memberships.sort_by_key(|m| m.created_at);
            json!({
                "user": user.as_ref().map(|u| json!({
                    "id": u.id,
                    "email": u.email,
                    "display_name": u.display_name,
                    "platform_admin": u.platform_admin,
                    "created_at": u.created_at,
                })),
                "memberships": memberships
                    .iter()
                    .map(|m| json!({
                        "organization_id": m.organization_id,
                        "role": m.role,
                        "status": m.status,
                        "created_at": m.created_at,
                    }))
                    .collect::<Vec<_>>(),
            })
        }
        "members" => {
            let mut members = state.saas.members(ctx_org).await;
            members.sort_by_key(|m| (m.created_at, m.user_id));
            json!({ "members": members
                .iter()
                .map(|m| json!({
                    "user_id": m.user_id,
                    "role": m.role,
                    "status": m.status,
                    "created_at": m.created_at,
                }))
                .collect::<Vec<_>>() })
        }
        "api_keys" => {
            let mut keys = state.saas.api_keys_of(ctx_org).await;
            keys.sort_by_key(|k| (k.created_at, k.id));
            json!({ "api_keys": keys
                .iter()
                .map(|k| json!({
                    "key_prefix": k.key_prefix,
                    "label": k.label,
                    "role": k.role,
                    "created_at": k.created_at,
                    "expires_at": k.expires_at,
                    "revoked_at": k.revoked_at,
                }))
                .collect::<Vec<_>>() })
        }
        "usage" => {
            let period = now.format("%Y-%m").to_string();
            let mut totals = BTreeMap::new();
            for metric in UsageMetric::ALL {
                let total = state.saas.usage_total(ctx_org, metric, &period).await;
                totals.insert(metric.as_str().to_string(), total);
            }
            json!({ "period": period, "totals": totals })
        }
        "subscription" => {
            let subscription = state.saas.subscription_of(ctx_org).await;
            let plan = match &subscription {
                Some(s) => state.saas.plan(s.plan_id).await.map(|p| p.code),
                None => None,
            };
            json!({
                "subscription": subscription.map(|s| json!({
                    "id": s.id,
                    "plan_id": s.plan_id,
                    "plan_code": plan,
                    "provider": s.provider,
                    "status": s.status,
                    "current_period_start": s.current_period_start,
                    "current_period_end": s.current_period_end,
                    "cancel_at_period_end": s.cancel_at_period_end,
                })),
            })
        }
        "wallets" => {
            let mut bindings = WalletRegistry::for_state(state).list_for(ctx_org).await;
            bindings.sort_by_key(|b| (b.created_at, b.id));
            json!({ "wallets": bindings.iter().map(|b| b.public_view()).collect::<Vec<_>>() })
        }
        "audit" => {
            // The audit trail is a global log; an export carries only the
            // rows that TARGET this tenant, in chronological order.
            let rows: Vec<Value> = state
                .audit
                .recent(AUDIT_EXPORT_LIMIT)
                .await
                .into_iter()
                .filter(|r| r.target.as_deref() == Some(ctx_org.to_string().as_str()))
                .map(|r| {
                    json!({
                        "id": r.id,
                        "ts": r.ts,
                        "actor": r.actor,
                        "action": r.action,
                        "outcome": r.outcome,
                        "detail": r.detail,
                        "chained": r.chained,
                    })
                })
                .collect();
            json!({ "records": rows, "limit": AUDIT_EXPORT_LIMIT })
        }
        _ => unreachable!("kind is validated against SECTIONS first"),
    }
}

/// `GET /api/saas/exports?kind=…` — one audited, deterministic section.
pub async fn export(
    State(state): State<ApiState>,
    Query(params): Query<std::collections::HashMap<String, String>>,
    headers: HeaderMap,
) -> Response {
    let Some(kind) = params.get("kind").map(|s| s.trim()) else {
        return unknown_kind();
    };
    let Some((_, extra)) = SECTIONS.iter().find(|(k, _)| *k == kind) else {
        return unknown_kind();
    };
    let ctx = match authorize_request(
        &state,
        &headers,
        AccessRequest::read(Permission::ExportCreate),
    )
    .await
    {
        Ok(c) => c,
        Err(d) => return deny_response(&state, &d).await,
    };
    if let Some(extra) = extra {
        if !ctx.authorization.permissions.contains(*extra) {
            let d = bot_core::authorization::Decision::permission(
                "this export section needs an additional permission",
            );
            return deny_response(&state, &d).await;
        }
    }
    let org = ctx.organization_id();
    let payload = section(&state, kind, org, ctx.authorization.user_id).await;
    state
        .audit
        .success(&ctx.actor_label(), "saas.export", Some(kind))
        .await;
    Json(json!({
        "organization_id": org,
        "kind": kind,
        "generated_at": Utc::now(),
        "data": payload,
    }))
    .into_response()
}

/// The export route, mounted by [`crate::saas::routes`].
pub fn routes() -> Router<ApiState> {
    Router::new().route("/api/saas/exports", axum::routing::get(export))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use bot_core::billing::PlanCode;
    use bot_core::config::AppConfig;
    use bot_core::membership::{Membership, MembershipRole};
    use bot_core::session::model::SessionRecord;
    use bot_core::session::token::{generate_token, hash_password};
    use bot_core::state::AppState;
    use bot_core::tenant::{Organization, User, UserStatus};
    use http_body_util::BodyExt;
    use std::sync::Arc;
    use tower::ServiceExt;

    use crate::saas::middleware::AUTH_HEADER;

    async fn tenant(
        state: &ApiState,
        slug: &str,
        role: MembershipRole,
        plan: PlanCode,
    ) -> (bot_core::tenant::OrganizationId, String) {
        let org = Organization::new(
            bot_core::tenant::OrganizationId::new(),
            format!("{slug}-{}", uuid::Uuid::new_v4().simple()),
            slug,
            None,
            Utc::now(),
        );
        state.saas.create_organization(&org).await.expect("org");
        state
            .saas
            .assign_plan(org.id, plan, Utc::now())
            .await
            .expect("plan");
        let user = User {
            id: bot_core::tenant::UserId::new(),
            email: format!("{slug}@example.com"),
            email_verified: true,
            display_name: slug.into(),
            password_hash: hash_password("password-123456"),
            status: UserStatus::Active,
            platform_admin: false,
            created_at: Utc::now(),
            updated_at: Utc::now(),
            last_login_at: None,
        };
        state.saas.create_user(&user).await.expect("user");
        state
            .saas
            .create_membership(&Membership::new(org.id, user.id, role, None, Utc::now()))
            .await
            .expect("membership");
        let t = generate_token("ses");
        state
            .saas
            .create_session(&SessionRecord::new(
                user.id,
                Some(org.id),
                t.hash,
                t.prefix,
                chrono::Duration::hours(1),
                Utc::now(),
            ))
            .await
            .expect("session");
        (org.id, t.plaintext)
    }

    fn test_state() -> ApiState {
        let shared = AppState::new(AppConfig::from_defaults());
        ApiState {
            audit: bot_core::audit::AuditTrail::new(None, shared.events.clone()),
            shared,
            api_key: None,
            auth: None,
            limiter: bot_core::auth::RateLimiter::new(0),
            db: None,
            journal: None,
            serve_dashboard: false,
            health: Arc::new(bot_core::obs::health::HealthRegistry::new()),
            metrics_enabled: false,
            saas: crate::saas::SaasStore::shared(),
        }
    }

    async fn get_export(
        app: axum::Router,
        token: &str,
        org: &bot_core::tenant::OrganizationId,
        kind: &str,
    ) -> (StatusCode, Value) {
        let res = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri(format!("/api/saas/exports?kind={kind}"))
                    .header(AUTH_HEADER, format!("Bearer {token}"))
                    .header("x-organization", org.to_string())
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = res.status();
        let bytes = BodyExt::collect(res.into_body()).await.unwrap().to_bytes();
        (status, serde_json::from_slice(&bytes).unwrap_or(json!({})))
    }

    #[tokio::test]
    async fn exports_are_tenant_scoped_and_never_leak_other_tenants() {
        let state = test_state();
        let (org_a, token_a) = tenant(
            &state,
            "exp-a",
            MembershipRole::OrgOwner,
            PlanCode::Business,
        )
        .await;
        let (org_b, _token_b) = tenant(
            &state,
            "exp-b",
            MembershipRole::OrgOwner,
            PlanCode::Business,
        )
        .await;
        let app = crate::api::router(state);

        for kind in [
            "profile",
            "members",
            "api_keys",
            "usage",
            "subscription",
            "wallets",
        ] {
            let (status, body) = get_export(app.clone(), &token_a, &org_a, kind).await;
            assert_eq!(status, StatusCode::OK, "{kind}: {body}");
            assert_eq!(body["organization_id"], json!(org_a.to_string()), "{kind}");
            assert_eq!(body["kind"], json!(kind));
            let text = body.to_string();
            assert!(!text.contains(&org_b.to_string()), "{kind} leaked tenant B");
            assert!(
                !text.contains("password_hash"),
                "{kind} leaked a password hash"
            );
            assert!(!text.contains("token_hash"), "{kind} leaked a session hash");
            assert!(!text.contains("secret_hash"), "{kind} leaked a key hash");
        }

        // Unknown kinds are refused deterministically.
        let (status, body) = get_export(app.clone(), &token_a, &org_a, "sessions").await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(body["error"], json!("unknown_export_kind"));

        // A viewer holds NO export.create at all: even `profile` is refused.
        let state = test_state();
        let (org_c, token_c) =
            tenant(&state, "exp-c", MembershipRole::Viewer, PlanCode::Starter).await;
        let app = crate::api::router(state);
        let (status, body) = get_export(app.clone(), &token_c, &org_c, "profile").await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        assert!(
            body["reason"]
                .as_str()
                .map(|s| !s.is_empty())
                .unwrap_or(false),
            "a stable, human reason accompanies the refusal: {body}"
        );
        // …while an auditor (read surface + export.create + audit.read) may
        // take everything it can read, including the audit section.
        let state = test_state();
        let (org_d, token_d) =
            tenant(&state, "exp-d", MembershipRole::Auditor, PlanCode::Business).await;
        let app = crate::api::router(state);
        for kind in [
            "profile",
            "members",
            "api_keys",
            "usage",
            "subscription",
            "wallets",
            "audit",
        ] {
            let (status, body) = get_export(app.clone(), &token_d, &org_d, kind).await;
            assert_eq!(status, StatusCode::OK, "auditor {kind}: {body}");
        }
    }

    #[tokio::test]
    async fn exports_are_deterministic_for_the_same_state() {
        let state = test_state();
        let (org, token) = tenant(
            &state,
            "exp-det",
            MembershipRole::OrgOwner,
            PlanCode::Business,
        )
        .await;
        let app = crate::api::router(state);
        let (_, one) = get_export(app.clone(), &token, &org, "api_keys").await;
        let (_, two) = get_export(app, &token, &org, "api_keys").await;
        // generated_at differs; the DATA section must be identical bytes.
        assert_eq!(one["data"], two["data"]);
        assert_eq!(one["data"].to_string(), two["data"].to_string());
    }
}
