//! Tenant custody management application layer (BATCH file 15).
//!
//! Provides APIs for create custody profile, activate, revoke, attach module capability,
//! resolve active signer, inspect public address/status.
//! Integrates with existing wallet_access.rs and TransactionSigner/SignerRegistry.
//! Never exposes private key material. Fail closed on provider failure.

use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::{Json, Router};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::Row;

use bot_core::authorization::AccessRequest;
use bot_core::custody::model::{
    CustodyProfile, CustodyProfileId, CustodyStatus, ProviderType, SignerId, SignerRecord,
};
use bot_core::custody::policy::{check as check_custody_policy, CustodyRequest};
use bot_core::db::Database;
use bot_core::membership::Permission;
use bot_core::tenant::{Organization, OrganizationId};
use tracing::warn;

use crate::api::ApiState;
use crate::saas::middleware::{authorize_request, deny_response};

/// Create custody profile request.
#[derive(Debug, Deserialize)]
pub struct CreateProfileRequest {
    pub name: String,
    pub provider_type: String,
    pub description: Option<String>,
}

/// Attach capability request.
#[derive(Debug, Deserialize)]
pub struct AttachCapabilityRequest {
    pub capability: String,
}

/// Custody profile view — public metadata only, no secrets.
#[derive(Debug, Serialize)]
pub struct CustodyProfileView {
    pub id: String,
    pub organization_id: String,
    pub name: String,
    pub provider_type: String,
    pub status: String,
    pub public_signers: Vec<SignerView>,
    pub created_at: String,
}

#[derive(Debug, Serialize)]
pub struct SignerView {
    pub id: String,
    pub logical_identity: String,
    pub public_address: String,
    pub provider_type: String,
    pub status: String,
    pub capabilities: Vec<String>,
}

impl From<SignerRecord> for SignerView {
    fn from(s: SignerRecord) -> Self {
        Self {
            id: s.id.to_string(),
            logical_identity: s.logical_identity,
            public_address: s.public_address,
            provider_type: s.provider_type.as_str().into(),
            status: s.status.as_str().into(),
            capabilities: s.capabilities,
        }
    }
}

pub fn routes() -> Router<ApiState> {
    Router::new()
        .route(
            "/api/saas/custody/profiles",
            axum::routing::post(create_profile).get(list_profiles),
        )
        .route(
            "/api/saas/custody/profiles/:id/activate",
            axum::routing::post(activate_profile),
        )
        .route(
            "/api/saas/custody/profiles/:id/revoke",
            axum::routing::post(revoke_profile),
        )
        .route(
            "/api/saas/custody/signers",
            axum::routing::post(create_signer).get(list_signers),
        )
        .route(
            "/api/saas/custody/profiles/:id/signers",
            axum::routing::get(list_profile_signers),
        )
        .route(
            "/api/saas/custody/signers/:id/activate",
            axum::routing::post(activate_signer),
        )
        .route(
            "/api/saas/custody/signers/:id/revoke",
            axum::routing::post(revoke_signer),
        )
        .route(
            "/api/saas/custody/signers/:id/capabilities",
            axum::routing::post(attach_capability),
        )
        .route(
            "/api/saas/custody/signers/:id/resolve",
            axum::routing::get(resolve_signer),
        )
        .route(
            "/api/saas/custody/signers/:id",
            axum::routing::get(get_signer),
        )
}

/// Compatibility cache for custody domain records. PostgreSQL is authoritative whenever attached; this cache is used only for the no-database test/development mode and is refreshed before database-backed reads or mutations.
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

fn profiles_store() -> &'static Mutex<HashMap<CustodyProfileId, CustodyProfile>> {
    static S: OnceLock<Mutex<HashMap<CustodyProfileId, CustodyProfile>>> = OnceLock::new();
    S.get_or_init(|| Mutex::new(HashMap::new()))
}
fn signers_store() -> &'static Mutex<HashMap<SignerId, SignerRecord>> {
    static S: OnceLock<Mutex<HashMap<SignerId, SignerRecord>>> = OnceLock::new();
    S.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Tenant-scoped profile lookup, shared with the rotation API (§F/P0:
/// rotation must resolve the REAL profile, never invent one).
pub(crate) fn find_profile(org: OrganizationId, id: CustodyProfileId) -> Option<CustodyProfile> {
    let map = profiles_store().lock().expect("mutex").clone();
    map.get(&id).filter(|p| p.organization_id == org).cloned()
}

/// Tenant-scoped signer lookup, shared with the rotation API.
pub(crate) fn find_signer(org: OrganizationId, id: SignerId) -> Option<SignerRecord> {
    let map = signers_store().lock().expect("mutex").clone();
    map.get(&id).filter(|s| s.organization_id == org).cloned()
}

/// The relational custody tables foreign-key onto `organizations(id)`,
/// while the SaaS control plane persists organizations as generic
/// runtime records. When a database is attached, the custody write path
/// therefore first ensures the tenant's relational row exists —
/// idempotently, from the AUTHENTICATED organization context (never
/// from client input, never fabricated).
async fn ensure_organization_row(db: &std::sync::Arc<Database>, org: &Organization) {
    if let Err(error) = sqlx::query(
        "INSERT INTO organizations (id, slug, name, status, created_at, updated_at) VALUES ($1,$2,$3,$4,$5,$6) ON CONFLICT DO NOTHING",
    )
    .bind(org.id.as_uuid())
    .bind(&org.slug)
    .bind(&org.name)
    .bind(org.status.as_str())
    .bind(org.created_at)
    .bind(org.updated_at)
    .execute(db.pool())
    .await
    {
        warn!(%error, organization = %org.id, "custody organization row ensure failed");
    }
}

fn custody_storage_error(detail: &'static str) -> Response {
    (
        StatusCode::SERVICE_UNAVAILABLE,
        Json(json!({
            "error": "custody_storage_error",
            "reason": detail,
        })),
    )
        .into_response()
}

/// Persist the custody lifecycle event in the dedicated compliance table.
/// The general application audit stream remains useful for operational
/// observability, but `custody_audit` is the tenant-scoped report source for
/// custody actions. In no-database test/development mode the existing
/// process-local application audit remains the only available sink.
pub(crate) async fn record_durable_custody_audit(
    state: &ApiState,
    ctx: &crate::saas::middleware::SaasContext,
    profile_id: Option<CustodyProfileId>,
    signer_id: Option<SignerId>,
    action: &'static str,
    from_status: Option<&str>,
    to_status: Option<&str>,
    from_provider: Option<&str>,
    to_provider: Option<&str>,
    reason: &str,
) -> Result<(), String> {
    let Some(db) = state.db.as_deref() else {
        return Ok(());
    };
    let actor = ctx.actor_label();
    let result = sqlx::query(
        "INSERT INTO custody_audit
             (id, organization_id, custody_profile_id, signer_id, action,
              from_status, to_status, from_provider, to_provider, reason,
              actor_user_id, actor, created_at)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, now())",
    )
    .bind(uuid::Uuid::new_v4())
    .bind(ctx.organization.id.as_uuid())
    .bind(profile_id.map(|value| value.as_uuid()))
    .bind(signer_id.map(|value| value.as_uuid()))
    .bind(action)
    .bind(from_status)
    .bind(to_status)
    .bind(from_provider)
    .bind(to_provider)
    .bind(reason)
    .bind(ctx.authorization.user_id.map(|value| value.as_uuid()))
    .bind(actor)
    .execute(db.pool())
    .await
    .map_err(|error| {
        tracing::error!(error = %error, action, organization = %ctx.organization.id, "durable custody audit write failed");
        "custody action was applied but its durable audit record could not be written".to_string()
    })?;
    if result.rows_affected() != 1 {
        return Err(
            "custody action was applied but its durable audit record was not inserted".to_string(),
        );
    }
    Ok(())
}

/// Refresh the process cache from the durable custody tables before any
/// handler relies on the legacy domain-model lookup helpers. The cache is
/// retained for the no-database test/development mode, but an attached
/// PostgreSQL database is authoritative across restarts and replicas.
pub(crate) async fn hydrate_from_database(
    state: &ApiState,
    organization_id: OrganizationId,
) -> Result<(), Response> {
    let Some(db) = state.db.as_deref() else {
        return Ok(());
    };

    macro_rules! required_column {
        ($row:expr, $column:literal) => {{
            $row.try_get($column).map_err(|error| {
                tracing::error!(error = %error, column = $column, "custody row column could not be decoded");
                custody_storage_error("stored custody data is invalid")
            })?
        }};
    }

    let profile_rows = sqlx::query(
        "SELECT id, organization_id, name, description, provider_type, status,
                created_by, created_at, updated_at, activated_at, revoked_at, revoke_reason
           FROM custody_profiles
          WHERE organization_id = $1",
    )
    .bind(organization_id.as_uuid())
    .fetch_all(db.pool())
    .await
    .map_err(|error| {
        tracing::error!(error = %error, organization_id = %organization_id, "failed to hydrate custody profiles");
        custody_storage_error("custody profiles could not be loaded")
    })?;

    let signer_rows = sqlx::query(
        "SELECT id, organization_id, custody_profile_id, logical_identity,
                provider_type, public_address, provider_ref, capabilities, status,
                created_by, created_at, updated_at, activated_at, revoked_at,
                revoke_reason, last_used_at
           FROM custody_signers
          WHERE organization_id = $1",
    )
    .bind(organization_id.as_uuid())
    .fetch_all(db.pool())
    .await
    .map_err(|error| {
        tracing::error!(error = %error, organization_id = %organization_id, "failed to hydrate custody signers");
        custody_storage_error("custody signers could not be loaded")
    })?;

    let mut profiles = Vec::with_capacity(profile_rows.len());
    for row in profile_rows {
        let provider_raw: String = required_column!(row, "provider_type");
        let status_raw: String = required_column!(row, "status");
        let Some(provider_type) = ProviderType::parse(&provider_raw) else {
            return Err(custody_storage_error(
                "stored custody provider type is invalid",
            ));
        };
        let Some(status) = CustodyStatus::parse(&status_raw) else {
            return Err(custody_storage_error(
                "stored custody profile status is invalid",
            ));
        };
        let organization_uuid: uuid::Uuid = required_column!(row, "organization_id");
        profiles.push(CustodyProfile {
            id: CustodyProfileId(required_column!(row, "id")),
            organization_id: OrganizationId::from(organization_uuid),
            name: required_column!(row, "name"),
            description: required_column!(row, "description"),
            provider_type,
            status,
            created_by: required_column!(row, "created_by"),
            created_at: required_column!(row, "created_at"),
            updated_at: required_column!(row, "updated_at"),
            activated_at: required_column!(row, "activated_at"),
            revoked_at: required_column!(row, "revoked_at"),
            revoke_reason: required_column!(row, "revoke_reason"),
        });
    }

    let mut signers = Vec::with_capacity(signer_rows.len());
    for row in signer_rows {
        let provider_raw: String = required_column!(row, "provider_type");
        let status_raw: String = required_column!(row, "status");
        let Some(provider_type) = ProviderType::parse(&provider_raw) else {
            return Err(custody_storage_error(
                "stored signer provider type is invalid",
            ));
        };
        let Some(status) = CustodyStatus::parse(&status_raw) else {
            return Err(custody_storage_error("stored signer status is invalid"));
        };
        let capabilities_value: serde_json::Value = required_column!(row, "capabilities");
        let capabilities = serde_json::from_value::<Vec<String>>(capabilities_value)
            .map_err(|_| custody_storage_error("stored signer capabilities are invalid"))?;
        let organization_uuid: uuid::Uuid = required_column!(row, "organization_id");
        signers.push(SignerRecord {
            id: SignerId(required_column!(row, "id")),
            organization_id: OrganizationId::from(organization_uuid),
            custody_profile_id: CustodyProfileId(required_column!(row, "custody_profile_id")),
            logical_identity: required_column!(row, "logical_identity"),
            provider_type,
            public_address: required_column!(row, "public_address"),
            provider_ref: required_column!(row, "provider_ref"),
            capabilities,
            status,
            created_by: required_column!(row, "created_by"),
            created_at: required_column!(row, "created_at"),
            updated_at: required_column!(row, "updated_at"),
            activated_at: required_column!(row, "activated_at"),
            revoked_at: required_column!(row, "revoked_at"),
            revoke_reason: required_column!(row, "revoke_reason"),
            last_used_at: required_column!(row, "last_used_at"),
        });
    }

    {
        let mut map = profiles_store().lock().expect("mutex");
        map.retain(|_, profile| profile.organization_id != organization_id);
        for profile in profiles {
            map.insert(profile.id, profile);
        }
    }
    {
        let mut map = signers_store().lock().expect("mutex");
        map.retain(|_, signer| signer.organization_id != organization_id);
        for signer in signers {
            map.insert(signer.id, signer);
        }
    }
    Ok(())
}

async fn create_profile(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Json(body): Json<CreateProfileRequest>,
) -> Response {
    let ctx = match authorize_request(
        &state,
        &headers,
        AccessRequest::manage(Permission::WalletManage),
    )
    .await
    {
        Ok(ctx) => ctx,
        Err(d) => return deny_response(&state, &d).await,
    };

    // Closed tenants cannot create custody profiles
    if ctx.organization.status == bot_core::tenant::OrganizationStatus::Closed {
        return (axum::http::StatusCode::FORBIDDEN, Json(json!({"error":"tenant_closed","reason":"closed organization cannot create custody profiles"}))).into_response();
    }

    let provider = match ProviderType::parse(&body.provider_type) {
        Some(p) => p,
        None => return (
            axum::http::StatusCode::BAD_REQUEST,
            Json(
                json!({"error":"invalid_provider","reason":"provider must be local|vault|kms|hsm"}),
            ),
        )
            .into_response(),
    };

    if body.name.trim().is_empty() {
        return (
            axum::http::StatusCode::BAD_REQUEST,
            Json(json!({"error":"invalid_name","reason":"name must not be empty"})),
        )
            .into_response();
    }

    let now = Utc::now();
    let mut profile = CustodyProfile::new(ctx.organization.id, body.name.trim(), provider, now);
    profile.created_by = ctx.authorization.user_id.map(|user_id| user_id.as_uuid());
    if let Some(desc) = body.description {
        profile.description = desc.trim().to_string();
    }

    // Persist to DB first when configured (durable & fail-closed).
    if let Some(db) = &state.db {
        ensure_organization_row(db, &ctx.organization).await;
        let insert = sqlx::query("INSERT INTO custody_profiles (id, organization_id, name, description, provider_type, status, created_by, created_at, updated_at) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9) ON CONFLICT DO NOTHING")
            .bind(profile.id.as_uuid()).bind(ctx.organization.id.as_uuid()).bind(&profile.name).bind(&profile.description).bind(provider.as_str()).bind(profile.status.as_str()).bind(profile.created_by).bind(profile.created_at).bind(profile.updated_at)
            .execute(db.pool()).await;
        match insert {
            Ok(result) if result.rows_affected() == 1 => {}
            Ok(_) => {
                return (
                    axum::http::StatusCode::CONFLICT,
                    Json(json!({"error":"already_exists","reason":"a custody profile with this name already exists"})),
                ).into_response();
            }
            Err(error) => {
                warn!(%error, profile = %profile.id, "custody profile durable insert failed");
                return (
                    axum::http::StatusCode::SERVICE_UNAVAILABLE,
                    Json(json!({"error":"persistence_failed","reason":"database write failed for custody profile"})),
                ).into_response();
            }
        }
    }
    {
        let mut map = profiles_store().lock().expect("mutex");
        map.insert(profile.id, profile.clone());
    }
    if let Err(reason) = record_durable_custody_audit(
        &state,
        &ctx,
        Some(profile.id),
        None,
        "profile_created",
        None,
        Some(profile.status.as_str()),
        None,
        Some(profile.provider_type.as_str()),
        "",
    )
    .await
    {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({"error":"custody_audit_unavailable","reason": reason})),
        )
            .into_response();
    }

    state.audit.record("saas", "saas.custody.profile.created", Some(&profile.id.to_string()),
        bot_core::audit::AuditOutcome::Success,
        json!({"organization": ctx.organization.id.to_string(), "provider": provider.as_str(), "name": profile.name})).await;

    let view = CustodyProfileView {
        id: profile.id.to_string(),
        organization_id: profile.organization_id.to_string(),
        name: profile.name,
        provider_type: profile.provider_type.as_str().into(),
        status: profile.status.as_str().into(),
        public_signers: Vec::new(),
        created_at: profile.created_at.to_rfc3339(),
    };
    (axum::http::StatusCode::CREATED, Json(json!(view))).into_response()
}

async fn list_profiles(State(state): State<ApiState>, headers: HeaderMap) -> Response {
    let ctx = match authorize_request(
        &state,
        &headers,
        AccessRequest::read(Permission::WalletRead),
    )
    .await
    {
        Ok(ctx) => ctx,
        Err(d) => return deny_response(&state, &d).await,
    };
    if let Err(response) = hydrate_from_database(&state, ctx.organization.id).await {
        return response;
    }
    let map = profiles_store().lock().expect("mutex");
    let profiles: Vec<CustodyProfileView> = map
        .values()
        .filter(|p| p.organization_id == ctx.organization.id)
        .map(|p| {
            let signers: Vec<SignerView> = {
                let s_map = signers_store().lock().expect("mutex");
                s_map
                    .values()
                    .filter(|s| s.custody_profile_id == p.id)
                    .cloned()
                    .map(SignerView::from)
                    .collect()
            };
            CustodyProfileView {
                id: p.id.to_string(),
                organization_id: p.organization_id.to_string(),
                name: p.name.clone(),
                provider_type: p.provider_type.as_str().into(),
                status: p.status.as_str().into(),
                public_signers: signers,
                created_at: p.created_at.to_rfc3339(),
            }
        })
        .collect();
    (
        axum::http::StatusCode::OK,
        Json(json!({"organization_id": ctx.organization.id.to_string(), "profiles": profiles})),
    )
        .into_response()
}

async fn activate_profile(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    let ctx = match authorize_request(
        &state,
        &headers,
        AccessRequest::manage(Permission::WalletManage),
    )
    .await
    {
        Ok(ctx) => ctx,
        Err(d) => return deny_response(&state, &d).await,
    };
    if let Err(response) = hydrate_from_database(&state, ctx.organization.id).await {
        return response;
    }
    let pid = match CustodyProfileId::parse(&id) {
        Some(v) => v,
        None => {
            return (
                axum::http::StatusCode::BAD_REQUEST,
                Json(json!({"error":"invalid_id","reason":"profile id must be uuid"})),
            )
                .into_response()
        }
    };
    if ctx.organization.status == bot_core::tenant::OrganizationStatus::Closed {
        return (axum::http::StatusCode::FORBIDDEN, Json(json!({"error":"tenant_closed","reason":"closed organization cannot activate custody"}))).into_response();
    }
    let mut profile = match find_profile(ctx.organization.id, pid) {
        Some(p) => p,
        None => {
            return (
                axum::http::StatusCode::NOT_FOUND,
                Json(json!({"error":"not_found","reason":"custody profile not found"})),
            )
                .into_response()
        }
    };
    // Check transition
    if !profile.status.can_transition_to(CustodyStatus::Active) {
        return (axum::http::StatusCode::CONFLICT, Json(json!({"error":"invalid_transition","reason": format!("cannot activate from {}", profile.status.as_str())}))).into_response();
    }
    let from_status = profile.status.as_str().to_string();
    profile.status = CustodyStatus::Active;
    profile.activated_at = Some(Utc::now());
    profile.updated_at = Utc::now();

    // Durable status write (fail-closed: DB first).
    if let Some(db) = &state.db {
        let update = sqlx::query(
            "UPDATE custody_profiles SET status = $1, updated_at = $2, activated_at = $3 WHERE id = $4 AND organization_id = $5",
        )
        .bind(profile.status.as_str())
        .bind(profile.updated_at)
        .bind(profile.activated_at)
        .bind(pid.as_uuid())
        .bind(ctx.organization.id.as_uuid())
        .execute(db.pool())
        .await;
        match update {
            Ok(result) if result.rows_affected() == 1 => {}
            Ok(_) => {
                return (
                    axum::http::StatusCode::NOT_FOUND,
                    Json(json!({"error":"not_found","reason":"custody profile disappeared before activation"})),
                ).into_response();
            }
            Err(error) => {
                warn!(%error, profile = %pid, "custody profile durable activation write failed");
                return (
                    axum::http::StatusCode::SERVICE_UNAVAILABLE,
                    Json(json!({"error":"persistence_failed","reason":"database write failed for custody profile activation"})),
                ).into_response();
            }
        }
    }
    {
        let mut map = profiles_store().lock().expect("mutex");
        map.insert(pid, profile.clone());
    }
    if let Err(reason) = record_durable_custody_audit(
        &state,
        &ctx,
        Some(pid),
        None,
        "profile_activated",
        Some(&from_status),
        Some(profile.status.as_str()),
        Some(profile.provider_type.as_str()),
        Some(profile.provider_type.as_str()),
        "",
    )
    .await
    {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({"error":"custody_audit_unavailable","reason": reason})),
        )
            .into_response();
    }
    state
        .audit
        .record(
            "saas",
            "saas.custody.profile.activated",
            Some(&pid.to_string()),
            bot_core::audit::AuditOutcome::Success,
            json!({"organization": ctx.organization.id.to_string()}),
        )
        .await;
    (
        axum::http::StatusCode::OK,
        Json(json!({"id": pid.to_string(), "status": profile.status.as_str()})),
    )
        .into_response()
}

async fn revoke_profile(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    let ctx = match authorize_request(
        &state,
        &headers,
        AccessRequest::manage(Permission::WalletManage),
    )
    .await
    {
        Ok(ctx) => ctx,
        Err(d) => return deny_response(&state, &d).await,
    };
    if let Err(response) = hydrate_from_database(&state, ctx.organization.id).await {
        return response;
    }
    let pid = match CustodyProfileId::parse(&id) {
        Some(v) => v,
        None => {
            return (
                axum::http::StatusCode::BAD_REQUEST,
                Json(json!({"error":"invalid_id","reason":"profile id must be uuid"})),
            )
                .into_response()
        }
    };
    let mut profile = match find_profile(ctx.organization.id, pid) {
        Some(p) => p,
        None => {
            return (
                axum::http::StatusCode::NOT_FOUND,
                Json(json!({"error":"not_found","reason":"custody profile not found"})),
            )
                .into_response()
        }
    };
    if !profile.status.can_transition_to(CustodyStatus::Revoked) {
        return (axum::http::StatusCode::CONFLICT, Json(json!({"error":"invalid_transition","reason": format!("cannot revoke from {}", profile.status.as_str())}))).into_response();
    }
    let from_status = profile.status.as_str().to_string();
    profile.status = CustodyStatus::Revoked;
    profile.revoked_at = Some(Utc::now());
    profile.updated_at = Utc::now();

    // Durable status write (fail-closed: DB first).
    if let Some(db) = &state.db {
        let update = sqlx::query(
            "UPDATE custody_profiles SET status = $1, updated_at = $2, revoked_at = $3, revoke_reason = $4 WHERE id = $5 AND organization_id = $6",
        )
        .bind(profile.status.as_str())
        .bind(profile.updated_at)
        .bind(profile.revoked_at)
        .bind(&profile.revoke_reason)
        .bind(pid.as_uuid())
        .bind(ctx.organization.id.as_uuid())
        .execute(db.pool())
        .await;
        match update {
            Ok(result) if result.rows_affected() == 1 => {}
            Ok(_) => {
                return (
                    axum::http::StatusCode::NOT_FOUND,
                    Json(json!({"error":"not_found","reason":"custody profile disappeared before revocation"})),
                ).into_response();
            }
            Err(error) => {
                warn!(%error, profile = %pid, "custody profile durable revocation write failed");
                return (
                    axum::http::StatusCode::SERVICE_UNAVAILABLE,
                    Json(json!({"error":"persistence_failed","reason":"database write failed for custody profile revocation"})),
                ).into_response();
            }
        }
    }
    {
        let mut map = profiles_store().lock().expect("mutex");
        map.insert(pid, profile.clone());
    }
    if let Err(reason) = record_durable_custody_audit(
        &state,
        &ctx,
        Some(pid),
        None,
        "profile_revoked",
        Some(&from_status),
        Some(profile.status.as_str()),
        Some(profile.provider_type.as_str()),
        Some(profile.provider_type.as_str()),
        &profile.revoke_reason,
    )
    .await
    {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({"error":"custody_audit_unavailable","reason": reason})),
        )
            .into_response();
    }
    state
        .audit
        .record(
            "saas",
            "saas.custody.profile.revoked",
            Some(&pid.to_string()),
            bot_core::audit::AuditOutcome::Success,
            json!({"organization": ctx.organization.id.to_string()}),
        )
        .await;
    (
        axum::http::StatusCode::OK,
        Json(json!({"id": pid.to_string(), "status": profile.status.as_str()})),
    )
        .into_response()
}

/// Create signer under a profile — public address only, no secret material.
#[derive(Debug, Deserialize)]
pub struct CreateSignerRequest {
    pub custody_profile_id: String,
    pub logical_identity: String,
    pub public_address: String,
    pub capabilities: Vec<String>,
    pub provider_ref: Option<String>,
}

async fn create_signer(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Json(body): Json<CreateSignerRequest>,
) -> Response {
    let ctx = match authorize_request(
        &state,
        &headers,
        AccessRequest::manage(Permission::WalletManage),
    )
    .await
    {
        Ok(ctx) => ctx,
        Err(d) => return deny_response(&state, &d).await,
    };
    if ctx.organization.status == bot_core::tenant::OrganizationStatus::Closed {
        return (axum::http::StatusCode::FORBIDDEN, Json(json!({"error":"tenant_closed","reason":"closed organization cannot create signers"}))).into_response();
    }
    if let Err(response) = hydrate_from_database(&state, ctx.organization.id).await {
        return response;
    }
    let pid = match CustodyProfileId::parse(&body.custody_profile_id) {
        Some(v) => v,
        None => {
            return (
                axum::http::StatusCode::BAD_REQUEST,
                Json(json!({"error":"invalid_profile_id","reason":"must be uuid"})),
            )
                .into_response()
        }
    };
    let profile = match find_profile(ctx.organization.id, pid) {
        Some(p) => p,
        None => {
            return (
                axum::http::StatusCode::NOT_FOUND,
                Json(json!({"error":"not_found","reason":"custody profile not found"})),
            )
                .into_response()
        }
    };
    if !matches!(
        profile.status,
        CustodyStatus::Pending | CustodyStatus::Active
    ) {
        return (
            axum::http::StatusCode::CONFLICT,
            Json(json!({
                "error": "profile_unavailable",
                "reason": format!("cannot create a signer for a {} profile", profile.status.as_str()),
            })),
        )
            .into_response();
    }
    // Validate provider match: signer uses the profile's provider type.
    if body.logical_identity.trim().is_empty() || body.public_address.trim().is_empty() {
        return (axum::http::StatusCode::BAD_REQUEST, Json(json!({"error":"invalid_request","reason":"logical_identity and public_address required"}))).into_response();
    }
    // Ensure public_address looks like a plausible address (non-empty, not secret-like)
    if body.public_address.to_ascii_lowercase().contains("private")
        || body.public_address.to_ascii_lowercase().contains("secret")
    {
        return (axum::http::StatusCode::BAD_REQUEST, Json(json!({"error":"invalid_address","reason":"public address must not contain secret material"}))).into_response();
    }

    let now = Utc::now();
    let mut signer = SignerRecord::new(
        ctx.organization.id,
        pid,
        body.logical_identity.trim(),
        profile.provider_type,
        body.public_address.trim(),
        now,
    );
    signer.created_by = ctx.authorization.user_id.map(|user_id| user_id.as_uuid());
    signer.capabilities = body
        .capabilities
        .into_iter()
        .map(|c| c.trim().to_string())
        .filter(|c| !c.is_empty())
        .collect();
    signer.provider_ref = body.provider_ref;

    // Durable insert (fail-closed: DB first).
    if let Some(db) = &state.db {
        ensure_organization_row(db, &ctx.organization).await;
        let insert = sqlx::query("INSERT INTO custody_signers (id, organization_id, custody_profile_id, logical_identity, provider_type, public_address, provider_ref, capabilities, status, created_by, created_at, updated_at) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12) ON CONFLICT DO NOTHING")
            .bind(signer.id.as_uuid()).bind(ctx.organization.id.as_uuid()).bind(signer.custody_profile_id.as_uuid()).bind(&signer.logical_identity).bind(profile.provider_type.as_str()).bind(&signer.public_address).bind(&signer.provider_ref).bind(serde_json::to_value(&signer.capabilities).unwrap_or_else(|_| serde_json::json!([]))).bind(signer.status.as_str()).bind(signer.created_by).bind(signer.created_at).bind(signer.updated_at)
            .execute(db.pool()).await;
        match insert {
            Ok(result) if result.rows_affected() == 1 => {}
            Ok(_) => {
                return (
                    axum::http::StatusCode::CONFLICT,
                    Json(json!({"error":"already_exists","reason":"a signer with this identity or public address already exists"})),
                ).into_response();
            }
            Err(error) => {
                warn!(%error, signer = %signer.id, "custody signer durable insert failed");
                return (
                    axum::http::StatusCode::SERVICE_UNAVAILABLE,
                    Json(json!({"error":"persistence_failed","reason":"database write failed for custody signer"})),
                ).into_response();
            }
        }
    }
    {
        let mut map = signers_store().lock().expect("mutex");
        map.insert(signer.id, signer.clone());
    }
    if let Err(reason) = record_durable_custody_audit(
        &state,
        &ctx,
        Some(signer.custody_profile_id),
        Some(signer.id),
        "signer_created",
        None,
        Some(signer.status.as_str()),
        Some(signer.provider_type.as_str()),
        Some(signer.provider_type.as_str()),
        "",
    )
    .await
    {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({"error":"custody_audit_unavailable","reason": reason})),
        )
            .into_response();
    }

    state.audit.record("saas", "saas.custody.signer.created", Some(&signer.id.to_string()),
        bot_core::audit::AuditOutcome::Success,
        json!({"organization": ctx.organization.id.to_string(), "profile": pid.to_string(), "identity": signer.logical_identity, "address": signer.public_address})).await;

    // Never expose private key material — only public address
    (
        axum::http::StatusCode::CREATED,
        Json(json!(SignerView::from(signer))),
    )
        .into_response()
}

async fn list_signers(State(state): State<ApiState>, headers: HeaderMap) -> Response {
    let ctx = match authorize_request(
        &state,
        &headers,
        AccessRequest::read(Permission::WalletRead),
    )
    .await
    {
        Ok(ctx) => ctx,
        Err(d) => return deny_response(&state, &d).await,
    };
    if let Err(response) = hydrate_from_database(&state, ctx.organization.id).await {
        return response;
    }
    let map = signers_store().lock().expect("mutex");
    let signers: Vec<SignerView> = map
        .values()
        .filter(|s| s.organization_id == ctx.organization.id)
        .cloned()
        .map(SignerView::from)
        .collect();
    (
        axum::http::StatusCode::OK,
        Json(json!({"organization_id": ctx.organization.id.to_string(), "signers": signers})),
    )
        .into_response()
}

async fn list_profile_signers(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    let ctx = match authorize_request(
        &state,
        &headers,
        AccessRequest::read(Permission::WalletRead),
    )
    .await
    {
        Ok(ctx) => ctx,
        Err(d) => return deny_response(&state, &d).await,
    };
    if let Err(response) = hydrate_from_database(&state, ctx.organization.id).await {
        return response;
    }
    let pid = match CustodyProfileId::parse(&id) {
        Some(v) => v,
        None => {
            return (
                axum::http::StatusCode::BAD_REQUEST,
                Json(json!({"error":"invalid_id","reason":"profile id must be uuid"})),
            )
                .into_response()
        }
    };
    let map = signers_store().lock().expect("mutex");
    let signers: Vec<SignerView> = map
        .values()
        .filter(|s| s.organization_id == ctx.organization.id && s.custody_profile_id == pid)
        .cloned()
        .map(SignerView::from)
        .collect();
    (
        axum::http::StatusCode::OK,
        Json(json!({"organization_id": ctx.organization.id.to_string(), "profile_id": pid.to_string(), "signers": signers})),
    )
        .into_response()
}

async fn activate_signer(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    let ctx = match authorize_request(
        &state,
        &headers,
        AccessRequest::manage(Permission::WalletManage),
    )
    .await
    {
        Ok(ctx) => ctx,
        Err(d) => return deny_response(&state, &d).await,
    };
    if ctx.organization.status == bot_core::tenant::OrganizationStatus::Closed {
        return (
            axum::http::StatusCode::FORBIDDEN,
            Json(json!({"error":"tenant_closed","reason":"closed tenant cannot activate signer"})),
        )
            .into_response();
    }
    if let Err(response) = hydrate_from_database(&state, ctx.organization.id).await {
        return response;
    }
    let sid = match SignerId::parse(&id) {
        Some(v) => v,
        None => {
            return (
                axum::http::StatusCode::BAD_REQUEST,
                Json(json!({"error":"invalid_id","reason":"signer id must be uuid"})),
            )
                .into_response()
        }
    };
    let mut signer = match find_signer(ctx.organization.id, sid) {
        Some(s) => s,
        None => {
            return (
                axum::http::StatusCode::NOT_FOUND,
                Json(json!({"error":"not_found","reason":"signer not found"})),
            )
                .into_response()
        }
    };
    let profile = match find_profile(ctx.organization.id, signer.custody_profile_id) {
        Some(p) => p,
        None => {
            return (
                axum::http::StatusCode::CONFLICT,
                Json(json!({"error":"profile_not_found","reason":"signer profile is no longer available"})),
            )
                .into_response();
        }
    };
    if profile.status != CustodyStatus::Active {
        return (
            axum::http::StatusCode::CONFLICT,
            Json(json!({"error":"profile_not_active","reason":"signer activation requires an active custody profile"})),
        )
            .into_response();
    }
    if !signer.status.can_transition_to(CustodyStatus::Active) {
        return (axum::http::StatusCode::CONFLICT, Json(json!({"error":"invalid_transition","reason": format!("cannot activate from {}", signer.status.as_str())}))).into_response();
    }
    let from_status = signer.status.as_str().to_string();
    // Provider failure must fail closed: check registry
    // For now we allow activation; resolve will fail if provider not configured.
    signer.status = CustodyStatus::Active;
    signer.activated_at = Some(Utc::now());
    signer.updated_at = Utc::now();

    // Durable status write (fail-closed: DB first).
    if let Some(db) = &state.db {
        let update = sqlx::query(
            "UPDATE custody_signers SET status = $1, updated_at = $2, activated_at = $3 WHERE id = $4 AND organization_id = $5",
        )
        .bind(signer.status.as_str())
        .bind(signer.updated_at)
        .bind(signer.activated_at)
        .bind(sid.as_uuid())
        .bind(ctx.organization.id.as_uuid())
        .execute(db.pool())
        .await;
        match update {
            Ok(result) if result.rows_affected() == 1 => {}
            Ok(_) => {
                return (
                    axum::http::StatusCode::NOT_FOUND,
                    Json(json!({"error":"not_found","reason":"custody signer disappeared before activation"})),
                ).into_response();
            }
            Err(error) => {
                warn!(%error, signer = %sid, "custody signer durable activation write failed");
                return (
                    axum::http::StatusCode::SERVICE_UNAVAILABLE,
                    Json(json!({"error":"persistence_failed","reason":"database write failed for custody signer activation"})),
                ).into_response();
            }
        }
    }
    {
        let mut map = signers_store().lock().expect("mutex");
        map.insert(sid, signer.clone());
    }
    if let Err(reason) = record_durable_custody_audit(
        &state,
        &ctx,
        Some(signer.custody_profile_id),
        Some(sid),
        "signer_activated",
        Some(&from_status),
        Some(signer.status.as_str()),
        Some(signer.provider_type.as_str()),
        Some(signer.provider_type.as_str()),
        "",
    )
    .await
    {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({"error":"custody_audit_unavailable","reason": reason})),
        )
            .into_response();
    }
    state.audit.record("saas", "saas.custody.signer.activated", Some(&sid.to_string()),
        bot_core::audit::AuditOutcome::Success,
        json!({"organization": ctx.organization.id.to_string(), "address": signer.public_address})).await;
    (
        axum::http::StatusCode::OK,
        Json(json!({"id": sid.to_string(), "status": signer.status.as_str()})),
    )
        .into_response()
}

async fn revoke_signer(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    let ctx = match authorize_request(
        &state,
        &headers,
        AccessRequest::manage(Permission::WalletManage),
    )
    .await
    {
        Ok(ctx) => ctx,
        Err(d) => return deny_response(&state, &d).await,
    };
    if let Err(response) = hydrate_from_database(&state, ctx.organization.id).await {
        return response;
    }
    let sid = match SignerId::parse(&id) {
        Some(v) => v,
        None => {
            return (
                axum::http::StatusCode::BAD_REQUEST,
                Json(json!({"error":"invalid_id","reason":"signer id must be uuid"})),
            )
                .into_response()
        }
    };
    let mut signer = match find_signer(ctx.organization.id, sid) {
        Some(s) => s,
        None => {
            return (
                axum::http::StatusCode::NOT_FOUND,
                Json(json!({"error":"not_found","reason":"signer not found"})),
            )
                .into_response()
        }
    };
    if !signer.status.can_transition_to(CustodyStatus::Revoked) {
        return (axum::http::StatusCode::CONFLICT, Json(json!({"error":"invalid_transition","reason": format!("cannot revoke from {}", signer.status.as_str())}))).into_response();
    }
    let from_status = signer.status.as_str().to_string();
    signer.status = CustodyStatus::Revoked;
    signer.revoked_at = Some(Utc::now());
    signer.updated_at = Utc::now();

    // Durable status write (fail-closed: DB first).
    if let Some(db) = &state.db {
        let update = sqlx::query(
            "UPDATE custody_signers SET status = $1, updated_at = $2, revoked_at = $3, revoke_reason = $4 WHERE id = $5 AND organization_id = $6",
        )
        .bind(signer.status.as_str())
        .bind(signer.updated_at)
        .bind(signer.revoked_at)
        .bind(&signer.revoke_reason)
        .bind(sid.as_uuid())
        .bind(ctx.organization.id.as_uuid())
        .execute(db.pool())
        .await;
        match update {
            Ok(result) if result.rows_affected() == 1 => {}
            Ok(_) => {
                return (
                    axum::http::StatusCode::NOT_FOUND,
                    Json(json!({"error":"not_found","reason":"custody signer disappeared before revocation"})),
                ).into_response();
            }
            Err(error) => {
                warn!(%error, signer = %sid, "custody signer durable revocation write failed");
                return (
                    axum::http::StatusCode::SERVICE_UNAVAILABLE,
                    Json(json!({"error":"persistence_failed","reason":"database write failed for custody signer revocation"})),
                ).into_response();
            }
        }
    }
    {
        let mut map = signers_store().lock().expect("mutex");
        map.insert(sid, signer.clone());
    }
    if let Err(reason) = record_durable_custody_audit(
        &state,
        &ctx,
        Some(signer.custody_profile_id),
        Some(sid),
        "signer_revoked",
        Some(&from_status),
        Some(signer.status.as_str()),
        Some(signer.provider_type.as_str()),
        Some(signer.provider_type.as_str()),
        &signer.revoke_reason,
    )
    .await
    {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({"error":"custody_audit_unavailable","reason": reason})),
        )
            .into_response();
    }
    state.audit.record("saas", "saas.custody.signer.revoked", Some(&sid.to_string()),
        bot_core::audit::AuditOutcome::Success,
        json!({"organization": ctx.organization.id.to_string(), "address": signer.public_address})).await;
    (
        axum::http::StatusCode::OK,
        Json(json!({"id": sid.to_string(), "status": signer.status.as_str()})),
    )
        .into_response()
}

async fn attach_capability(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(body): Json<AttachCapabilityRequest>,
) -> Response {
    let ctx = match authorize_request(
        &state,
        &headers,
        AccessRequest::manage(Permission::WalletManage),
    )
    .await
    {
        Ok(ctx) => ctx,
        Err(d) => return deny_response(&state, &d).await,
    };
    if let Err(response) = hydrate_from_database(&state, ctx.organization.id).await {
        return response;
    }
    let sid = match SignerId::parse(&id) {
        Some(v) => v,
        None => {
            return (
                axum::http::StatusCode::BAD_REQUEST,
                Json(json!({"error":"invalid_id","reason":"signer id must be uuid"})),
            )
                .into_response()
        }
    };
    let mut signer = match find_signer(ctx.organization.id, sid) {
        Some(s) => s,
        None => {
            return (
                axum::http::StatusCode::NOT_FOUND,
                Json(json!({"error":"not_found","reason":"signer not found"})),
            )
                .into_response()
        }
    };
    if signer.status != CustodyStatus::Active && signer.status != CustodyStatus::Pending {
        return (axum::http::StatusCode::CONFLICT, Json(json!({"error":"signer_not_active","reason": format!("signer status is {}", signer.status.as_str())}))).into_response();
    }
    let cap = body.capability.trim().to_string();
    if cap.is_empty() {
        return (
            axum::http::StatusCode::BAD_REQUEST,
            Json(json!({"error":"invalid_capability","reason":"capability must not be empty"})),
        )
            .into_response();
    }
    if !signer.capabilities.contains(&cap) {
        signer.capabilities.push(cap.clone());
        signer.updated_at = Utc::now();
        if let Some(db) = &state.db {
            let update = sqlx::query(
                "UPDATE custody_signers SET capabilities = $1, updated_at = $2 WHERE id = $3 AND organization_id = $4",
            )
            .bind(serde_json::to_value(&signer.capabilities).unwrap_or_else(|_| serde_json::json!([])))
            .bind(signer.updated_at)
            .bind(sid.as_uuid())
            .bind(ctx.organization.id.as_uuid())
            .execute(db.pool())
            .await;
            match update {
                Ok(result) if result.rows_affected() == 1 => {}
                Ok(_) => {
                    return (
                        axum::http::StatusCode::NOT_FOUND,
                        Json(json!({"error":"not_found","reason":"custody signer disappeared before capability update"})),
                    ).into_response();
                }
                Err(error) => {
                    warn!(%error, signer = %sid, "custody signer durable capability update failed");
                    return (
                        axum::http::StatusCode::SERVICE_UNAVAILABLE,
                        Json(json!({"error":"persistence_failed","reason":"database write failed for signer capabilities"})),
                    ).into_response();
                }
            }
        }
        {
            let mut map = signers_store().lock().expect("mutex");
            map.insert(sid, signer.clone());
        }
        if let Err(reason) = record_durable_custody_audit(
            &state,
            &ctx,
            Some(signer.custody_profile_id),
            Some(sid),
            "capability_attached",
            None,
            Some(signer.status.as_str()),
            Some(signer.provider_type.as_str()),
            Some(signer.provider_type.as_str()),
            &cap,
        )
        .await
        {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(json!({"error":"custody_audit_unavailable","reason": reason})),
            )
                .into_response();
        }
        state
            .audit
            .record(
                "saas",
                "saas.custody.capability.attached",
                Some(&sid.to_string()),
                bot_core::audit::AuditOutcome::Success,
                json!({"organization": ctx.organization.id.to_string(), "capability": cap}),
            )
            .await;
    }
    (
        axum::http::StatusCode::OK,
        Json(json!(SignerView::from(signer))),
    )
        .into_response()
}

async fn get_signer(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    let ctx = match authorize_request(
        &state,
        &headers,
        AccessRequest::read(Permission::WalletRead),
    )
    .await
    {
        Ok(ctx) => ctx,
        Err(d) => return deny_response(&state, &d).await,
    };
    if let Err(response) = hydrate_from_database(&state, ctx.organization.id).await {
        return response;
    }
    let sid = match SignerId::parse(&id) {
        Some(v) => v,
        None => {
            return (
                axum::http::StatusCode::BAD_REQUEST,
                Json(json!({"error":"invalid_id","reason":"signer id must be uuid"})),
            )
                .into_response()
        }
    };
    let signer = match find_signer(ctx.organization.id, sid) {
        Some(s) => s,
        None => {
            return (
                axum::http::StatusCode::NOT_FOUND,
                Json(json!({"error":"not_found","reason":"signer not found"})),
            )
                .into_response()
        }
    };
    // Public address/status only — never private key
    (
        axum::http::StatusCode::OK,
        Json(json!(SignerView::from(signer))),
    )
        .into_response()
}

async fn resolve_signer(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    let ctx = match authorize_request(
        &state,
        &headers,
        AccessRequest::read(Permission::WalletRead),
    )
    .await
    {
        Ok(ctx) => ctx,
        Err(d) => return deny_response(&state, &d).await,
    };
    if let Err(response) = hydrate_from_database(&state, ctx.organization.id).await {
        return response;
    }
    let sid = match SignerId::parse(&id) {
        Some(v) => v,
        None => {
            return (
                axum::http::StatusCode::BAD_REQUEST,
                Json(json!({"error":"invalid_id","reason":"signer id must be uuid"})),
            )
                .into_response()
        }
    };
    let signer = match find_signer(ctx.organization.id, sid) {
        Some(s) => s,
        None => {
            return (
                axum::http::StatusCode::NOT_FOUND,
                Json(json!({"error":"not_found","reason":"signer not found"})),
            )
                .into_response()
        }
    };
    let profile = match find_profile(ctx.organization.id, signer.custody_profile_id) {
        Some(p) => p,
        None => {
            return (
                axum::http::StatusCode::NOT_FOUND,
                Json(json!({"error":"not_found","reason":"profile not found for signer"})),
            )
                .into_response()
        }
    };

    // Policy check: tenant owns, signer active, tenant allowed, capability etc.
    // For resolve, we check that signer is active and tenant not closed.
    let req = CustodyRequest::new(
        ctx.organization.id,
        ctx.organization.status,
        "module.sniper",
        "module.sniper",
    );
    let verdict = check_custody_policy(Some(&profile), Some(&signer), &req);
    if !verdict.is_allowed() {
        let reason = verdict.deny_reason().unwrap();
        return (
            axum::http::StatusCode::FORBIDDEN,
            Json(json!({"error": reason.as_str(), "reason": format!("{:?}", verdict)})),
        )
            .into_response();
    }

    // Provider failure must fail closed — do not fall back to local.
    // This control-plane resolve endpoint does not itself perform remote
    // resolution: the configured remote custody provider has no implementation (fail-closed refusal naming
    // the PKCS#11 dependency), and Vault/KMS resolve + sign through the
    // custody sign boundary (`crates/server/src/custody/sign_boundary.rs`),
    // never through a local wallet fallback.
    if signer.provider_type != ProviderType::Local {
        return (axum::http::StatusCode::NOT_IMPLEMENTED, Json(json!({"error":"unsupported_provider","reason": format!("provider {} is not resolvable on this control-plane endpoint; remote custody resolves through the sign boundary; no local fallback", signer.provider_type.as_str())}))).into_response();
    }

    // Return public address/status — never private key
    let view = SignerView::from(signer);
    (
        axum::http::StatusCode::OK,
        Json(json!({"signer": view, "resolved": true})),
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use bot_core::custody::model::CustodyStatus;

    #[test]
    fn signer_view_never_contains_private_key() {
        let mut s = SignerRecord::new(
            bot_core::tenant::OrganizationId::new(),
            CustodyProfileId::new(),
            "sniper",
            ProviderType::Vault,
            "Pubkey123",
            chrono::Utc::now(),
        );
        s.status = CustodyStatus::Active;
        let view = SignerView::from(s);
        let json = serde_json::to_string(&view).unwrap();
        assert!(!json.to_ascii_lowercase().contains("private"));
        assert!(!json.to_ascii_lowercase().contains("secret"));
        assert!(json.contains("Pubkey123"));
    }
}
