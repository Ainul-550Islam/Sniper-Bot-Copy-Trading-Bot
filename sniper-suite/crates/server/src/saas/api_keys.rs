//! Tenant-scoped API keys (TASK 7A file 25).
//!
//! # The gap this closes
//!
//! Before TASK 7A the control plane had ONE kind of credential: a
//! deployment key in [`bot_core::auth::Authenticator`], built from
//! configuration at startup and held in memory. Two consequences:
//!
//! 1. **Keys created at runtime did not survive a restart.** `POST
//!    /api/keys` added the hash to the in-memory map (and to `api_keys`
//!    when a database was attached), but the authenticator was rebuilt from
//!    configuration on the next boot, so the key silently stopped working.
//! 2. **Keys were not tenant-scoped.** They carried a role, not an owner,
//!    which is exactly what a multi-tenant product cannot allow.
//!
//! This module adds the SaaS credential: owned by an organization, carrying
//! a SaaS role and optional narrowing scopes, and stored as a hash. Production
//! startup connects [`super::store::SaasStore`] to PostgreSQL, and every
//! authentication lookup reads the durable projection. Revocation therefore
//! takes effect across replicas and runtime-created tenant keys survive a
//! restart. Database-disabled test fixtures retain the in-memory adapter.
//!
//! The existing deployment keys are untouched: they keep their own
//! registry, their own `[auth]` configuration and their own role gate. A
//! deployment key is mapped into the SaaS model as
//! [`bot_core::authorization::Principal::LegacyDeploymentKey`] so a
//! single-tenant operator keeps working unchanged.
//!
//! # Security rules enforced here
//!
//! * the plaintext secret is returned exactly once, at creation;
//! * only `secret_hash` is stored — a database copy cannot authenticate;
//! * `key_prefix` is the public identifier used in listings and logs;
//! * lookups are by hash and then checked against the tenant, so a key from
//!   another organization can never resolve;
//! * revocation and expiry are explicit, durable and checked on every use.

use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use serde_json::json;
use uuid::Uuid;

use bot_core::authorization::{AccessRequest, Principal};
use bot_core::billing::features;
use bot_core::membership::{MembershipRole, Permission, PermissionSet};
use bot_core::session::token::{generate_token, hash_token};
use bot_core::tenant::{OrganizationId, UserId};

use super::middleware::{authorize_request, deny_response, SaasContext};
use crate::api::ApiState;

/// One tenant API key. Contains no secret material.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SaasApiKey {
    /// Row identity.
    pub id: Uuid,
    /// The owning tenant. Every lookup is checked against it.
    pub organization_id: OrganizationId,
    /// Public identifier (`sk_ab12cd34`) — safe in listings and logs.
    pub key_prefix: String,
    /// SHA-256 of the secret. NEVER the secret.
    pub secret_hash: String,
    /// Operator label.
    pub label: String,
    /// The SaaS role the key acts with.
    pub role: MembershipRole,
    /// Optional narrowing of the role's permissions (strings; unknown
    /// entries are ignored, never granted).
    pub scopes: Vec<String>,
    /// Who created it.
    pub created_by: Option<UserId>,
    /// Creation time.
    pub created_at: DateTime<Utc>,
    /// Last successful authentication.
    pub last_used_at: Option<DateTime<Utc>>,
    /// Optional expiry.
    pub expires_at: Option<DateTime<Utc>>,
    /// When it was revoked.
    pub revoked_at: Option<DateTime<Utc>>,
    /// Why it was revoked.
    pub revoke_reason: String,
}

impl SaasApiKey {
    /// Is the key usable at `now`?
    pub fn is_usable(&self, now: DateTime<Utc>) -> bool {
        self.revoked_at.is_none() && self.expires_at.map(|e| e > now).unwrap_or(true)
    }

    /// Why it is not usable (stable label for audit / metrics).
    pub fn rejection(&self, now: DateTime<Utc>) -> Option<&'static str> {
        if self.revoked_at.is_some() {
            Some("api_key_revoked")
        } else if self.expires_at.map(|e| e <= now).unwrap_or(false) {
            Some("api_key_expired")
        } else {
            None
        }
    }

    /// Revoke it. Idempotent.
    pub fn revoke(&mut self, reason: impl Into<String>, now: DateTime<Utc>) -> bool {
        if self.revoked_at.is_some() {
            return false;
        }
        self.revoked_at = Some(now);
        self.revoke_reason = reason.into();
        true
    }

    /// The listing view (already secret-free, kept explicit so a future
    /// field addition cannot leak by accident).
    pub fn metadata(&self) -> serde_json::Value {
        json!({
            "id": self.id,
            "organization_id": self.organization_id,
            "key_prefix": self.key_prefix,
            "label": self.label,
            "role": self.role.as_str(),
            "scopes": self.scopes,
            "created_by": self.created_by,
            "created_at": self.created_at,
            "last_used_at": self.last_used_at,
            "expires_at": self.expires_at,
            "revoked_at": self.revoked_at,
            "revoke_reason": self.revoke_reason,
            "usable": self.is_usable(Utc::now()),
        })
    }

    /// Single-line audit text — prefix only.
    pub fn summary(&self) -> String {
        format!(
            "api_key={} organization={} role={} scopes={} usable={}",
            self.key_prefix,
            self.organization_id,
            self.role,
            self.scopes.len(),
            self.is_usable(Utc::now())
        )
    }
}

/// A freshly created key plus the one-time plaintext.
pub struct CreatedApiKey {
    /// The durable record.
    pub key: SaasApiKey,
    /// The plaintext secret. Show once, never store.
    pub plaintext: String,
}

/// Build a new tenant API key. The caller persists `key` and returns
/// `plaintext` to the client exactly once.
pub fn build_api_key(
    organization_id: OrganizationId,
    label: &str,
    role: MembershipRole,
    scopes: Vec<String>,
    created_by: Option<UserId>,
    expires_in_days: Option<i64>,
    now: DateTime<Utc>,
) -> CreatedApiKey {
    let token = generate_token("sk");
    let key = SaasApiKey {
        id: Uuid::new_v4(),
        organization_id,
        key_prefix: token.prefix.clone(),
        secret_hash: token.hash.clone(),
        label: label.trim().to_string(),
        role,
        scopes,
        created_by,
        created_at: now,
        last_used_at: None,
        expires_at: expires_in_days.map(|d| now + Duration::days(d.max(1))),
        revoked_at: None,
        revoke_reason: String::new(),
    };
    CreatedApiKey {
        key,
        plaintext: token.plaintext,
    }
}

/// Resolve a presented secret to its key record.
///
/// Hash-only lookup: the plaintext is hashed and compared, so the store
/// never sees the secret. Returns `None` for an unknown hash — the caller
/// must not distinguish "unknown" from "revoked" to a client.
pub async fn resolve_key(state: &ApiState, presented: &str) -> Option<SaasApiKey> {
    if presented.trim().is_empty() {
        return None;
    }
    state.saas.api_key_by_hash(&hash_token(presented)).await
}

// ---------------------------------------------------------------- routes --

/// Request body for key creation.
#[derive(Debug, Deserialize)]
pub struct CreateKeyBody {
    /// Operator label.
    pub label: String,
    /// SaaS role the key acts with.
    pub role: String,
    /// Optional narrowing scopes (permission strings).
    #[serde(default)]
    pub scopes: Vec<String>,
    /// Optional expiry in days.
    #[serde(default)]
    pub expires_in_days: Option<i64>,
}

/// `POST /api/saas/api-keys` — create a tenant key. The plaintext appears
/// in this response and nowhere else, ever.
pub async fn create_key(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Json(body): Json<CreateKeyBody>,
) -> Response {
    let existing_keys = |ctx: &SaasContext| ctx.organization_id();
    let ctx = match authorize_request(
        &state,
        &headers,
        AccessRequest::manage(Permission::ApiKeyCreate),
    )
    .await
    {
        Ok(c) => c,
        Err(d) => return deny_response(&state, &d).await,
    };

    // Plan limit: how many keys may exist at once.
    let current = state
        .saas
        .api_keys_of(existing_keys(&ctx))
        .await
        .iter()
        .filter(|k| k.is_usable(Utc::now()))
        .count() as f64;
    let entitlements = state
        .saas
        .entitlements_of(ctx.organization.id, Utc::now())
        .await;
    let request = AccessRequest::manage(Permission::ApiKeyCreate).consuming(
        features::MAX_API_KEYS,
        current,
        1.0,
    );
    let decision =
        bot_core::authorization::authorize(Some(&ctx.authorization), &request, Some(&entitlements));
    if !decision.is_allowed() {
        return deny_response(&state, &decision).await;
    }

    let Some(role) = MembershipRole::parse(&body.role) else {
        return (
            StatusCode::BAD_REQUEST,
            format!("unknown role '{}'", body.role),
        )
            .into_response();
    };
    if body.label.trim().is_empty() {
        return (StatusCode::BAD_REQUEST, "label must not be empty").into_response();
    }
    if let Some(unknown) = body
        .scopes
        .iter()
        .find(|scope| Permission::parse(scope).is_none())
    {
        return (
            StatusCode::BAD_REQUEST,
            format!("unknown permission scope '{unknown}'"),
        )
            .into_response();
    }
    if let Err(decision) = super::middleware::require_role_at_least(&ctx, role) {
        return deny_response(&state, &decision).await;
    }
    // A key may never be stronger than the effective permissions of the
    // credential creating it. Comparing set sizes is insufficient because
    // two roles can have the same number of different permissions.
    let requested_permissions = if body.scopes.is_empty() {
        role.permissions()
    } else {
        role.permissions()
            .intersect(&PermissionSet::parse_list(&body.scopes))
    };
    if !ctx
        .authorization
        .permissions
        .contains_all(requested_permissions.as_slice())
    {
        return (
            StatusCode::FORBIDDEN,
            "a key may not grant permissions its creator does not hold",
        )
            .into_response();
    }
    if role.is_platform_scope() {
        return (
            StatusCode::FORBIDDEN,
            "an API key may not hold platform scope",
        )
            .into_response();
    }

    let created = build_api_key(
        ctx.organization.id,
        &body.label,
        role,
        body.scopes.clone(),
        ctx.authorization.user_id,
        body.expires_in_days,
        Utc::now(),
    );
    if let Err(e) = state.saas.create_api_key(&created.key).await {
        return (StatusCode::CONFLICT, e.to_string()).into_response();
    }
    state
        .audit
        .success(
            &ctx.actor_label(),
            "saas.api_key.created",
            Some(&created.key.key_prefix),
        )
        .await;

    (
        StatusCode::CREATED,
        Json(json!({
            "key": created.key.metadata(),
            // The ONLY time the secret is ever returned.
            "secret": created.plaintext,
            "warning": "store this secret now — it cannot be retrieved again",
        })),
    )
        .into_response()
}

/// `GET /api/saas/api-keys` — list the tenant's keys (metadata only).
pub async fn list_keys(State(state): State<ApiState>, headers: HeaderMap) -> Response {
    let ctx = match authorize_request(
        &state,
        &headers,
        AccessRequest::read(Permission::TenantRead),
    )
    .await
    {
        Ok(c) => c,
        Err(d) => return deny_response(&state, &d).await,
    };
    let keys = state.saas.api_keys_of(ctx.organization.id).await;
    let list: Vec<serde_json::Value> = keys.iter().map(|k| k.metadata()).collect();
    Json(json!({ "count": list.len(), "keys": list })).into_response()
}

/// `DELETE /api/saas/api-keys/:prefix` — revoke a key by its public prefix.
pub async fn revoke_key(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Path(prefix): Path<String>,
) -> Response {
    let ctx = match authorize_request(
        &state,
        &headers,
        AccessRequest::manage(Permission::ApiKeyRevoke),
    )
    .await
    {
        Ok(c) => c,
        Err(d) => return deny_response(&state, &d).await,
    };
    // Tenant-scoped lookup: a prefix from another organization is simply
    // not in this list, so cross-tenant revocation is impossible.
    let keys = state.saas.api_keys_of(ctx.organization.id).await;
    let Some(mut key) = keys.into_iter().find(|k| k.key_prefix == prefix) else {
        return (StatusCode::NOT_FOUND, "no such key in this organization").into_response();
    };
    if !key.revoke("revoked via api", Utc::now()) {
        return Json(json!({ "ok": true, "already_revoked": true })).into_response();
    }
    if let Err(e) = state.saas.update_api_key(&key).await {
        return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response();
    }
    state
        .audit
        .success(
            &ctx.actor_label(),
            "saas.api_key.revoked",
            Some(&key.key_prefix),
        )
        .await;
    Json(json!({ "ok": true, "key": key.metadata() })).into_response()
}

/// The principal an authenticated key produces.
pub fn principal_for(key: &SaasApiKey) -> Principal {
    Principal::ApiKey {
        key_id: key.id.to_string(),
        key_prefix: key.key_prefix.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(now: DateTime<Utc>) -> CreatedApiKey {
        build_api_key(
            OrganizationId::new(),
            "ci",
            MembershipRole::Trader,
            vec![],
            Some(UserId::new()),
            None,
            now,
        )
    }

    #[test]
    fn the_record_never_carries_the_secret() {
        let now = Utc::now();
        let created = key(now);
        let json = serde_json::to_string(&created.key).unwrap();
        assert!(
            !json.contains(&created.plaintext),
            "the plaintext must never be serialisable"
        );
        assert!(json.contains(&created.key.secret_hash));
        let meta = created.key.metadata().to_string();
        assert!(!meta.contains(&created.plaintext));
        assert!(
            !meta.contains(&created.key.secret_hash),
            "not even the hash leaks to clients"
        );
        assert!(meta.contains(&created.key.key_prefix));
        assert!(!created.key.summary().contains(&created.plaintext));
    }

    #[test]
    fn lookup_hash_matches_the_presented_secret() {
        let created = key(Utc::now());
        assert_eq!(hash_token(&created.plaintext), created.key.secret_hash);
        assert_ne!(hash_token("wrong"), created.key.secret_hash);
        // The public prefix alone never authenticates.
        assert_ne!(hash_token(&created.key.key_prefix), created.key.secret_hash);
    }

    #[test]
    fn revocation_and_expiry_stop_a_key() {
        let now = Utc::now();
        let mut created = key(now);
        assert!(created.key.is_usable(now));
        assert_eq!(created.key.rejection(now), None);

        // Expiry.
        let mut expiring = build_api_key(
            created.key.organization_id,
            "short",
            MembershipRole::Viewer,
            vec![],
            None,
            Some(1),
            now,
        );
        assert!(expiring.key.is_usable(now));
        assert!(!expiring.key.is_usable(now + Duration::days(2)));
        assert_eq!(
            expiring.key.rejection(now + Duration::days(2)),
            Some("api_key_expired")
        );
        expiring.key.expires_at = None;

        // Revocation wins and is idempotent.
        assert!(created.key.revoke("compromised", now));
        assert!(!created.key.revoke("again", now));
        assert!(!created.key.is_usable(now));
        assert_eq!(created.key.rejection(now), Some("api_key_revoked"));
        assert_eq!(created.key.revoke_reason, "compromised");
    }

    #[test]
    fn keys_are_tenant_owned() {
        let now = Utc::now();
        let a = key(now);
        let b = key(now);
        assert_ne!(a.key.organization_id, b.key.organization_id);
        assert_ne!(a.key.secret_hash, b.key.secret_hash);
        assert_ne!(a.key.key_prefix, b.key.key_prefix);
        let p = principal_for(&a.key);
        assert_eq!(p.kind(), "api_key");
        assert!(p.identifier().contains(&a.key.key_prefix));
    }
}
