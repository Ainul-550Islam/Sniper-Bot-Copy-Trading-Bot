//! The tenant → wallet → strategy authorization boundary (TASK 7B file 14).
//!
//! The trading engines keep absolute execution authority: this layer never
//! signs, never holds keys and never launches anything by itself. What it
//! owns is the tenant-side answer to one question —
//!
//! ```text
//! tenant → authorized wallet → authorized module → existing execution system
//! ```
//!
//! * A wallet binding is PUBLIC data only: a label, a public address and the
//!   modules it may serve. The SaaS layer cannot receive signing material:
//!   there is no field for it, and the keypairs stay exactly where TASK 1–4
//!   and the signer registry keep them.
//! * Every answer is `ownership ∧ binding ∧ permission ∧ entitlement`:
//!   the binding must belong to the caller's tenant, the caller must hold
//!   the wallet permission, and the plan entitlement for the module must be
//!   enabled. A suspended tenant is refused earlier, by
//!   [`crate::saas::middleware`].
//! * Cross-tenant access is impossible by construction: the list endpoint
//!   reads only the caller's organization, and the id-keyed endpoints
//!   compare the binding's organization against the context before
//!   anything else.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, OnceLock};

use axum::extract::{Path, State};
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Response};
use axum::{Json, Router};
use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde_json::json;
use uuid::Uuid;

use bot_core::authorization::AccessRequest;
use bot_core::billing::features;
use bot_core::membership::Permission;
use bot_core::tenant::OrganizationId;

use crate::api::ApiState;
use crate::saas::middleware::{authorize_request, deny_response, SaasContext};
use crate::saas::postgres::PostgresSaasRepo;
use crate::security::websocket as saas_stream;

/// Runtime-record kind for durable wallet bindings.
pub const WALLET_KIND: &str = "wallet_access";

/// One tenant wallet binding. Public data only.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct WalletBinding {
    /// Row identity.
    pub id: Uuid,
    /// The owning tenant.
    pub organization_id: OrganizationId,
    /// Operator label.
    pub label: String,
    /// The PUBLIC chain address. Never a key, seed, mnemonic or secret.
    pub public_address: String,
    /// The module features this binding may serve (`module.sniper`, …).
    pub modules: Vec<String>,
    /// Who bound it.
    pub created_by: Option<bot_core::tenant::UserId>,
    /// Creation time.
    pub created_at: DateTime<Utc>,
    /// When it was revoked.
    pub revoked_at: Option<DateTime<Utc>>,
    /// Why it was revoked.
    pub revoke_reason: String,
}

impl WalletBinding {
    /// Is the binding live?
    pub fn is_active(&self) -> bool {
        self.revoked_at.is_none()
    }

    /// Revoke. Idempotent: the first reason is kept.
    pub fn revoke(&mut self, reason: impl Into<String>, now: DateTime<Utc>) -> bool {
        if self.revoked_at.is_some() {
            return false;
        }
        self.revoked_at = Some(now);
        self.revoke_reason = reason.into();
        true
    }

    /// The API view. Built here so a future field addition cannot leak
    /// anything beyond the public surface by accident.
    pub fn public_view(&self) -> serde_json::Value {
        json!({
            "id": self.id,
            "organization_id": self.organization_id,
            "label": self.label,
            "public_address": self.public_address,
            "modules": self.modules,
            "created_at": self.created_at,
            "revoked_at": self.revoked_at,
            "active": self.is_active(),
        })
    }
}

/// Validate a public address shape: printable, no whitespace, bounded
/// length. Chain-agnostic on purpose (Solana base58, EVM hex, …).
pub fn valid_public_address(value: &str) -> bool {
    let v = value.trim();
    (8..=100).contains(&v.len())
        && v.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, ':' | '_' | '-'))
}

/// The module features a binding may declare.
pub const BINDABLE_MODULES: [&str; 3] = [
    features::MODULE_SNIPER,
    features::MODULE_COPY,
    features::MODULE_POLYMARKET,
];

/// Where wallet bindings live: the durable runtime-record store when the
/// database is attached, otherwise a process-local map for single-process
/// runs and tests. Both paths carry public data only.
#[derive(Clone)]
pub enum WalletRegistry {
    Memory(Arc<Mutex<BTreeMap<Uuid, WalletBinding>>>),
    Durable(Arc<PostgresSaasRepo>),
}

fn memory_registry() -> WalletRegistry {
    static MEMORY: OnceLock<Arc<Mutex<BTreeMap<Uuid, WalletBinding>>>> = OnceLock::new();
    WalletRegistry::Memory(
        MEMORY
            .get_or_init(|| Arc::new(Mutex::new(BTreeMap::new())))
            .clone(),
    )
}

impl WalletRegistry {
    /// Pick the registry for this process from the API state.
    pub fn for_state(state: &ApiState) -> Self {
        match &state.db {
            Some(db) => WalletRegistry::Durable(Arc::new(PostgresSaasRepo::new(db.clone()))),
            None => memory_registry(),
        }
    }

    /// Insert a binding. `Ok(true)` = stored, `Ok(false)` = id already taken.
    async fn insert(&self, binding: &WalletBinding) -> Result<bool, String> {
        match self {
            WalletRegistry::Memory(map) => {
                let mut map = map.lock().expect("wallet map");
                if map.contains_key(&binding.id) {
                    return Ok(false);
                }
                map.insert(binding.id, binding.clone());
                Ok(true)
            }
            WalletRegistry::Durable(repo) => repo
                .insert(
                    WALLET_KIND,
                    &binding.id.to_string(),
                    Some(binding.organization_id),
                    binding.created_by,
                    None,
                    binding,
                )
                .await
                .map_err(|e| e.to_string()),
        }
    }

    /// Persist a changed binding.
    async fn update(&self, binding: &WalletBinding) -> Result<(), String> {
        match self {
            WalletRegistry::Memory(map) => {
                map.lock()
                    .expect("wallet map")
                    .insert(binding.id, binding.clone());
                Ok(())
            }
            WalletRegistry::Durable(repo) => repo
                .update(
                    WALLET_KIND,
                    &binding.id.to_string(),
                    Some(binding.organization_id),
                    binding.created_by,
                    None,
                    binding,
                )
                .await
                .map(|_| ())
                .map_err(|e| e.to_string()),
        }
    }

    /// One binding by id (any tenant — callers must scope before use).
    async fn get(&self, id: Uuid) -> Option<WalletBinding> {
        match self {
            WalletRegistry::Memory(map) => map.lock().expect("wallet map").get(&id).cloned(),
            WalletRegistry::Durable(repo) => repo
                .by_id::<WalletBinding>(WALLET_KIND, &id.to_string())
                .await
                .ok()
                .flatten(),
        }
    }

    /// Every binding of ONE tenant.
    pub async fn list_for(&self, organization_id: OrganizationId) -> Vec<WalletBinding> {
        match self {
            WalletRegistry::Memory(map) => map
                .lock()
                .expect("wallet map")
                .values()
                .filter(|b| b.organization_id == organization_id)
                .cloned()
                .collect(),
            WalletRegistry::Durable(repo) => repo
                .by_organization::<WalletBinding>(WALLET_KIND, organization_id)
                .await
                .unwrap_or_default(),
        }
    }
}

/// The pure boundary decision. Exported for tests and for any future
/// backend caller that wants the same answer the API gives.
pub fn decide(
    binding: &WalletBinding,
    ctx: &SaasContext,
    module_feature: &str,
    module_entitled: bool,
) -> Result<(), &'static str> {
    if binding.organization_id != ctx.organization_id() {
        return Err("the wallet belongs to a different organization");
    }
    if !binding.is_active() {
        return Err("the wallet binding is revoked");
    }
    if !binding.modules.iter().any(|m| m == module_feature) {
        return Err("the wallet binding does not serve this module");
    }
    if !ctx
        .authorization
        .permissions
        .contains(Permission::WalletManage)
    {
        return Err("the credential does not hold wallet.manage");
    }
    if !module_entitled {
        return Err("the plan does not include this module");
    }
    Ok(())
}

#[derive(Deserialize)]
pub struct BindBody {
    label: String,
    public_address: String,
    #[serde(default)]
    modules: Vec<String>,
}

/// `POST /api/saas/wallet-access` — bind a wallet to the caller's tenant.
pub async fn create_binding(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Json(body): Json<BindBody>,
) -> Response {
    let ctx = match authorize_request(
        &state,
        &headers,
        AccessRequest::manage(Permission::WalletManage),
    )
    .await
    {
        Ok(c) => c,
        Err(d) => return deny_response(&state, &d).await,
    };
    let label = body.label.trim();
    if label.is_empty() || label.len() > 120 {
        return (
            axum::http::StatusCode::UNPROCESSABLE_ENTITY,
            Json(json!({ "error": "invalid_label" })),
        )
            .into_response();
    }
    if !valid_public_address(&body.public_address) {
        return (
            axum::http::StatusCode::UNPROCESSABLE_ENTITY,
            Json(json!({ "error": "invalid_public_address" })),
        )
            .into_response();
    }
    let mut modules = body.modules.clone();
    modules.sort();
    modules.dedup();
    if modules
        .iter()
        .any(|m| !BINDABLE_MODULES.contains(&m.as_str()))
    {
        return (
            axum::http::StatusCode::UNPROCESSABLE_ENTITY,
            Json(json!({ "error": "unknown_module", "known": BINDABLE_MODULES })),
        )
            .into_response();
    }
    let binding = WalletBinding {
        id: Uuid::new_v4(),
        organization_id: ctx.organization_id(),
        label: label.to_string(),
        public_address: body.public_address.trim().to_string(),
        modules,
        created_by: ctx.authorization.user_id,
        created_at: Utc::now(),
        revoked_at: None,
        revoke_reason: String::new(),
    };
    let stored = WalletRegistry::for_state(&state);
    match stored.insert(&binding).await {
        Ok(true) => {
            state
                .audit
                .success(
                    "saas",
                    "saas.wallet.bound",
                    Some(&binding.organization_id.to_string()),
                )
                .await;
            saas_stream::publish(json!({
                "kind": "saas.wallets",
                "organization": ctx.organization_id().to_string(),
                "action": "bound",
                "wallet_id": binding.id.to_string(),
            }));
            (axum::http::StatusCode::CREATED, Json(binding.public_view())).into_response()
        }
        Ok(false) => (
            axum::http::StatusCode::CONFLICT,
            Json(json!({ "error": "binding_id_conflict" })),
        )
            .into_response(),
        Err(e) => (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": "storage_failed", "detail": e })),
        )
            .into_response(),
    }
}

/// `GET /api/saas/wallet-access` — the caller's OWN tenant's bindings.
pub async fn list_bindings(State(state): State<ApiState>, headers: HeaderMap) -> Response {
    let ctx = match authorize_request(
        &state,
        &headers,
        AccessRequest::read(Permission::WalletRead),
    )
    .await
    {
        Ok(c) => c,
        Err(d) => return deny_response(&state, &d).await,
    };
    let mut bindings = WalletRegistry::for_state(&state)
        .list_for(ctx.organization_id())
        .await;
    bindings.sort_by_key(|b| (b.created_at, b.id));
    Json(json!({
        "organization_id": ctx.organization_id(),
        "wallets": bindings.iter().map(|b| b.public_view()).collect::<Vec<_>>(),
    }))
    .into_response()
}

/// Load the binding and refuse cross-tenant access BEFORE anything else.
async fn own_binding(
    state: &ApiState,
    ctx: &SaasContext,
    id: Uuid,
) -> Result<WalletBinding, Box<Response>> {
    let binding = WalletRegistry::for_state(state).get(id).await;
    let Some(binding) = binding else {
        // Same shape as a cross-tenant refusal: existence is not disclosed.
        let d = bot_core::authorization::Decision::resource("no such wallet in this organization");
        return Err(Box::new(deny_response(state, &d).await));
    };
    if binding.organization_id != ctx.organization_id() {
        let d = bot_core::authorization::Decision::resource("no such wallet in this organization");
        return Err(Box::new(deny_response(state, &d).await));
    }
    Ok(binding)
}

/// `DELETE /api/saas/wallet-access/:id` — revoke one of the tenant's bindings.
pub async fn revoke_binding(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Response {
    let ctx = match authorize_request(
        &state,
        &headers,
        AccessRequest::manage(Permission::WalletManage),
    )
    .await
    {
        Ok(c) => c,
        Err(d) => return deny_response(&state, &d).await,
    };
    let mut binding = match own_binding(&state, &ctx, id).await {
        Ok(b) => b,
        Err(resp) => return *resp,
    };
    if !binding.revoke("revoked by operator", Utc::now()) {
        return Json(json!({ "ok": true, "already_revoked": true })).into_response();
    }
    let stored = WalletRegistry::for_state(&state);
    if let Err(e) = stored.update(&binding).await {
        return (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": "storage_failed", "detail": e })),
        )
            .into_response();
    }
    state
        .audit
        .success(
            "saas",
            "saas.wallet.revoked",
            Some(&binding.organization_id.to_string()),
        )
        .await;
    saas_stream::publish(json!({
        "kind": "saas.wallets",
        "organization": ctx.organization_id().to_string(),
        "action": "revoked",
        "wallet_id": binding.id.to_string(),
    }));
    Json(json!({ "ok": true, "wallet": binding.public_view() })).into_response()
}

#[derive(Deserialize)]
pub struct AuthorizeBody {
    module: String,
}

/// `POST /api/saas/wallet-access/:id/authorize` — the boundary question:
/// may THIS tenant run THIS module against THIS wallet right now?
pub async fn authorize_module(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Json(body): Json<AuthorizeBody>,
) -> Response {
    let ctx = match authorize_request(
        &state,
        &headers,
        AccessRequest::manage(Permission::WalletManage),
    )
    .await
    {
        Ok(c) => c,
        Err(d) => return deny_response(&state, &d).await,
    };
    let binding = match own_binding(&state, &ctx, id).await {
        Ok(b) => b,
        Err(resp) => return *resp,
    };
    let entitled = state
        .saas
        .entitlements_of(ctx.organization_id(), Utc::now())
        .await
        .allows(&body.module);
    match decide(&binding, &ctx, &body.module, entitled) {
        Ok(()) => Json(json!({
            "allowed": true,
            "binding": binding.public_view(),
            "module": body.module,
        }))
        .into_response(),
        Err(reason) => {
            state
                .audit
                .denied(
                    &ctx.actor_label(),
                    "saas.wallet.authorize",
                    Some(&binding.organization_id.to_string()),
                    reason,
                )
                .await;
            (
                axum::http::StatusCode::FORBIDDEN,
                Json(json!({ "allowed": false, "reason": reason })),
            )
                .into_response()
        }
    }
}

/// The wallet-access routes, mounted by [`crate::saas::routes`].
pub fn routes() -> Router<ApiState> {
    Router::new()
        .route(
            "/api/saas/wallet-access",
            axum::routing::post(create_binding).get(list_bindings),
        )
        .route(
            "/api/saas/wallet-access/:id",
            axum::routing::delete(revoke_binding),
        )
        .route(
            "/api/saas/wallet-access/:id/authorize",
            axum::routing::post(authorize_module),
        )
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use bot_core::billing::PlanCode;
    use bot_core::config::AppConfig;
    use bot_core::membership::Membership;
    use bot_core::session::model::SessionRecord;
    use bot_core::session::token::{generate_token, hash_password};
    use bot_core::state::AppState;
    use bot_core::tenant::{Organization, User, UserStatus};
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    use crate::saas::middleware::AUTH_HEADER;

    async fn tenant(
        state: &ApiState,
        slug: &str,
        role: bot_core::membership::MembershipRole,
        plan: PlanCode,
    ) -> (OrganizationId, String) {
        let org = Organization::new(
            OrganizationId::new(),
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

    async fn post_json(
        app: axum::Router,
        uri: &str,
        token: &str,
        org: &OrganizationId,
        body: serde_json::Value,
    ) -> (StatusCode, serde_json::Value) {
        let res = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(uri)
                    .header(AUTH_HEADER, format!("Bearer {token}"))
                    .header("x-organization", org.to_string())
                    .header("content-type", "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = res.status();
        let bytes = BodyExt::collect(res.into_body()).await.unwrap().to_bytes();
        (status, serde_json::from_slice(&bytes).unwrap_or(json!({})))
    }

    #[tokio::test]
    async fn tenants_see_only_their_own_bindings_and_never_cross_tenant_ids() {
        let state = test_state();
        let (org_a, token_a) = tenant(
            &state,
            "alpha",
            bot_core::membership::MembershipRole::OrgOwner,
            PlanCode::Business,
        )
        .await;
        let (org_b, token_b) = tenant(
            &state,
            "beta",
            bot_core::membership::MembershipRole::OrgOwner,
            PlanCode::Business,
        )
        .await;

        let app = crate::api::router(state);
        let (status, created) = post_json(
            app.clone(),
            "/api/saas/wallet-access",
            &token_a,
            &org_a,
            json!({
                "label": "hot wallet",
                "public_address": "So1anaWalletAddr1",
                "modules": ["module.sniper", "module.copy"],
            }),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "{created}");
        assert_eq!(created["organization_id"], json!(org_a.to_string()));
        // The view is public data only.
        assert!(!created.to_string().to_lowercase().contains("secret"));
        let binding_id = created["id"].as_str().unwrap().to_string();

        // Tenant B's list must NOT contain A's binding…
        let res = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/api/saas/wallet-access")
                    .header(AUTH_HEADER, format!("Bearer {token_b}"))
                    .header("x-organization", org_b.to_string())
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let bytes = BodyExt::collect(res.into_body()).await.unwrap().to_bytes();
        let listed_b: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(listed_b["organization_id"], json!(org_b.to_string()));
        assert_eq!(listed_b["wallets"].as_array().unwrap().len(), 0);
        assert!(!listed_b.to_string().contains(&binding_id));

        // …and B cannot revoke or authorize against A's binding id.
        let res = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri(format!("/api/saas/wallet-access/{binding_id}"))
                    .header(AUTH_HEADER, format!("Bearer {token_b}"))
                    .header("x-organization", org_b.to_string())
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::FORBIDDEN);
        let res = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/api/saas/wallet-access/{binding_id}/authorize"))
                    .header(AUTH_HEADER, format!("Bearer {token_b}"))
                    .header("x-organization", org_b.to_string())
                    .header("content-type", "application/json")
                    .body(Body::from(json!({ "module": "module.sniper" }).to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn the_entitlement_gate_stops_unplanned_modules() {
        let state = test_state();
        // Starter: module.polymarket is Disabled in the catalogue.
        let (org, token) = tenant(
            &state,
            "starter-co",
            bot_core::membership::MembershipRole::OrgOwner,
            PlanCode::Starter,
        )
        .await;
        let app = crate::api::router(state);
        let (_, created) = post_json(
            app.clone(),
            "/api/saas/wallet-access",
            &token,
            &org,
            json!({
                "label": "pm wallet",
                "public_address": "0xpolywallet01",
                "modules": ["module.polymarket"],
            }),
        )
        .await;
        let binding_id = created["id"].as_str().unwrap().to_string();

        let (status, decision) = post_json(
            app,
            &format!("/api/saas/wallet-access/{binding_id}/authorize"),
            &token,
            &org,
            json!({ "module": "module.polymarket" }),
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        assert_eq!(decision["allowed"], json!(false));
        assert_eq!(
            decision["reason"],
            json!("the plan does not include this module")
        );
    }

    #[tokio::test]
    async fn a_revoked_binding_stops_serving_and_unknown_modules_are_refused() {
        let state = test_state();
        let (org, token) = tenant(
            &state,
            "gamma",
            bot_core::membership::MembershipRole::OrgOwner,
            PlanCode::Business,
        )
        .await;
        let app = crate::api::router(state);
        let (_, created) = post_json(
            app.clone(),
            "/api/saas/wallet-access",
            &token,
            &org,
            json!({
                "label": "ops",
                "public_address": "Base58AddrForGamma1",
                "modules": ["module.sniper"],
            }),
        )
        .await;
        let binding_id = created["id"].as_str().unwrap().to_string();

        // A module the binding does not serve is refused even on an entitled plan.
        let (status, decision) = post_json(
            app.clone(),
            &format!("/api/saas/wallet-access/{binding_id}/authorize"),
            &token,
            &org,
            json!({ "module": "module.polymarket" }),
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        assert_eq!(
            decision["reason"],
            json!("the wallet binding does not serve this module")
        );

        let res = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri(format!("/api/saas/wallet-access/{binding_id}"))
                    .header(AUTH_HEADER, format!("Bearer {token}"))
                    .header("x-organization", org.to_string())
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);

        let (status, decision) = post_json(
            app,
            &format!("/api/saas/wallet-access/{binding_id}/authorize"),
            &token,
            &org,
            json!({ "module": "module.sniper" }),
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        assert_eq!(decision["reason"], json!("the wallet binding is revoked"));
    }

    #[test]
    fn address_validation_and_the_pure_decision_agree() {
        use bot_core::authorization::{AuthorizationContext, Principal};

        assert!(valid_public_address("So1anaWalletAddr1"));
        assert!(valid_public_address("0xABCDEF0123456789"));
        assert!(!valid_public_address("short"));
        assert!(!valid_public_address("has space inside"));
        assert!(!valid_public_address(&"x".repeat(101)));

        // A pure decision, driven by a real authorization context: an
        // OrgOwner of `org` holding wallet.manage.
        let org = Organization::new(
            OrganizationId::new(),
            "pure-decision",
            "Pure Decision Org",
            None,
            Utc::now(),
        );
        let ctx = SaasContext {
            authorization: AuthorizationContext::from_membership(
                Principal::UserSession {
                    session_id: "sess_test".to_string(),
                },
                &org,
                &bot_core::membership::Membership::new(
                    org.id,
                    bot_core::tenant::UserId::new(),
                    bot_core::membership::MembershipRole::OrgOwner,
                    None,
                    Utc::now(),
                ),
                None,
                false,
                Utc::now(),
            ),
            organization: org.clone(),
            user: None,
            api_key: None,
        };

        let mut binding = WalletBinding {
            id: Uuid::new_v4(),
            organization_id: org.id,
            label: "l".into(),
            public_address: "Base58AddrOk123".into(),
            modules: vec![features::MODULE_SNIPER.to_string()],
            created_by: None,
            created_at: Utc::now(),
            revoked_at: None,
            revoke_reason: String::new(),
        };
        // All four gates hold for the entitled, bound module.
        assert_eq!(
            decide(&binding, &ctx, features::MODULE_SNIPER, true),
            Ok(())
        );
        // Entitlement missing → refused.
        assert_eq!(
            decide(&binding, &ctx, features::MODULE_SNIPER, false),
            Err("the plan does not include this module")
        );
        // A module the binding does not serve is refused.
        assert_eq!(
            decide(&binding, &ctx, features::MODULE_POLYMARKET, true),
            Err("the wallet binding does not serve this module")
        );
        // A different organization's binding is refused by ownership first.
        let foreign = WalletBinding {
            organization_id: OrganizationId::new(),
            ..binding.clone()
        };
        assert_eq!(
            decide(&foreign, &ctx, features::MODULE_SNIPER, true),
            Err("the wallet belongs to a different organization")
        );
        // Revocation is the binding gate.
        assert!(binding.revoke("test", Utc::now()));
        assert_eq!(
            decide(&binding, &ctx, features::MODULE_SNIPER, true),
            Err("the wallet binding is revoked")
        );
    }
}
