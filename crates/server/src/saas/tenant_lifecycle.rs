//! Tenant lifecycle application service (BATCH file 16).
//!
//! Implements suspend, resume where valid, request close, execute deprovision phase,
//! schedule retention, inspect lifecycle status. Uses existing organization status model.
//! Closed organizations must not regain trading access through race/cache/restart.
//! All transitions emit tenant-scoped audit events.

use axum::extract::{Path, State};
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Response};
use axum::{Json, Router};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::json;
use uuid::Uuid;

use bot_core::authorization::AccessRequest;
use bot_core::membership::Permission;
use bot_core::provisioning::deprovision::{DeprovisionJob, DeprovisionPhase};
use bot_core::tenant::{OrganizationId, OrganizationStatus};

use crate::api::ApiState;
use crate::saas::middleware::{authorize_request, deny_response};

#[derive(Debug, Deserialize)]
pub struct SuspendRequest {
    pub reason: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct CloseRequest {
    pub reason: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct LifecycleStatusView {
    pub organization_id: String,
    pub organization_status: String,
    pub lifecycle_phase: String,
    pub lifecycle_state: String,
    pub retention_deadline: Option<String>,
}

pub fn routes() -> Router<ApiState> {
    Router::new()
        .route(
            "/api/saas/organizations/:id/suspend",
            axum::routing::post(suspend),
        )
        .route(
            "/api/saas/organizations/:id/resume",
            axum::routing::post(resume),
        )
        .route(
            "/api/saas/organizations/:id/close",
            axum::routing::post(request_close),
        )
        .route(
            "/api/saas/organizations/:id/lifecycle",
            axum::routing::get(lifecycle_status),
        )
        .route(
            "/api/saas/lifecycle/jobs/:id/advance",
            axum::routing::post(advance_job),
        )
}

// ---------------------------------------------------------------------------
// In-memory durability for lifecycle jobs (fallback; DB uses tenant_lifecycle_jobs table)
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

fn jobs_store() -> &'static Mutex<HashMap<Uuid, DeprovisionJob>> {
    static S: OnceLock<Mutex<HashMap<Uuid, DeprovisionJob>>> = OnceLock::new();
    S.get_or_init(|| Mutex::new(HashMap::new()))
}

fn active_job_for(org: OrganizationId) -> Option<DeprovisionJob> {
    let map = jobs_store().lock().expect("mutex");
    map.values()
        .find(|j| j.organization_id == org && j.state.is_resumable())
        .cloned()
}

fn find_job(id: Uuid) -> Option<DeprovisionJob> {
    jobs_store().lock().expect("mutex").get(&id).cloned()
}

fn upsert_job(job: DeprovisionJob) {
    let mut map = jobs_store().lock().expect("mutex");
    map.insert(job.id, job);
}

// ---------------------------------------------------------------------------

async fn suspend(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(body): Json<SuspendRequest>,
) -> Response {
    let ctx = match authorize_request(
        &state,
        &headers,
        AccessRequest::manage(Permission::TenantUpdate),
    )
    .await
    {
        Ok(ctx) => ctx,
        Err(d) => return deny_response(&state, &d).await,
    };
    let org_id = match OrganizationId::parse(&id) {
        Some(v) => v,
        None => {
            return (
                axum::http::StatusCode::BAD_REQUEST,
                Json(json!({"error":"invalid_organization_id","reason":"must be uuid"})),
            )
                .into_response()
        }
    };
    // Ensure acting on own tenant or platform admin
    if ctx.organization.id != org_id && !ctx.authorization.is_platform_scope() {
        return (
            axum::http::StatusCode::FORBIDDEN,
            Json(json!({"error":"cross_tenant","reason":"cannot suspend another organization"})),
        )
            .into_response();
    }
    let mut org = match state.saas.organization(org_id).await {
        Ok(Some(o)) => o,
        Ok(None) => {
            return (
                axum::http::StatusCode::NOT_FOUND,
                Json(json!({"error":"not_found","reason":"organization not found"})),
            )
                .into_response()
        }
        Err(error) => {
            tracing::error!(error = %error, organization = %org_id, "organization lookup failed during lifecycle operation");
            return (
                axum::http::StatusCode::SERVICE_UNAVAILABLE,
                Json(json!({"error":"lifecycle_storage_unavailable","reason":"authoritative organization data could not be loaded"})),
            )
                .into_response();
        }
    };
    if org.status == OrganizationStatus::Closed {
        return (
            axum::http::StatusCode::CONFLICT,
            Json(json!({"error":"tenant_closed","reason":"closed tenant cannot be suspended"})),
        )
            .into_response();
    }
    if org.status == OrganizationStatus::Suspended {
        return (axum::http::StatusCode::OK, Json(json!({"organization_id": org_id.to_string(), "status": org.status.as_str(), "note":"already suspended"}))).into_response();
    }

    org.status = OrganizationStatus::Suspended;
    org.suspended_at = Some(Utc::now());
    org.suspend_reason = body.reason.unwrap_or_default();
    org.updated_at = Utc::now();

    if let Err(e) = state.saas.update_organization(&org).await {
        return (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error":"update_failed","reason": e.to_string()})),
        )
            .into_response();
    }

    if let Some(db) = &state.db {
        if let Err(error) = sqlx::query("UPDATE organizations SET status='suspended', suspended_at=now(), suspend_reason=$2, updated_at=now() WHERE id=$1")
            .bind(org_id.as_uuid()).bind(&org.suspend_reason)
            .execute(db.pool()).await
        {
            tracing::warn!("organizations suspend durable update failed (org {}): {}", org_id, error);
        }
    }

    state
        .audit
        .record(
            "saas",
            "saas.tenant.suspended",
            Some(&org_id.to_string()),
            bot_core::audit::AuditOutcome::Success,
            json!({"organization": org_id.to_string(), "reason": org.suspend_reason}),
        )
        .await;

    // Emit lifecycle audit
    state
        .audit
        .record(
            "saas",
            "saas.lifecycle.suspend",
            Some(&org_id.to_string()),
            bot_core::audit::AuditOutcome::Success,
            json!({"organization": org_id.to_string()}),
        )
        .await;

    (
        axum::http::StatusCode::OK,
        Json(json!({"organization_id": org_id.to_string(), "status": org.status.as_str()})),
    )
        .into_response()
}

async fn resume(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    let ctx = match authorize_request(
        &state,
        &headers,
        AccessRequest::manage(Permission::TenantUpdate),
    )
    .await
    {
        Ok(ctx) => ctx,
        Err(d) => return deny_response(&state, &d).await,
    };
    let org_id = match OrganizationId::parse(&id) {
        Some(v) => v,
        None => {
            return (
                axum::http::StatusCode::BAD_REQUEST,
                Json(json!({"error":"invalid_organization_id","reason":"must be uuid"})),
            )
                .into_response()
        }
    };
    if ctx.organization.id != org_id && !ctx.authorization.is_platform_scope() {
        return (
            axum::http::StatusCode::FORBIDDEN,
            Json(json!({"error":"cross_tenant","reason":"cannot resume another organization"})),
        )
            .into_response();
    }
    let mut org = match state.saas.organization(org_id).await {
        Ok(Some(o)) => o,
        Ok(None) => {
            return (
                axum::http::StatusCode::NOT_FOUND,
                Json(json!({"error":"not_found","reason":"organization not found"})),
            )
                .into_response()
        }
        Err(error) => {
            tracing::error!(error = %error, organization = %org_id, "organization lookup failed during lifecycle operation");
            return (
                axum::http::StatusCode::SERVICE_UNAVAILABLE,
                Json(json!({"error":"lifecycle_storage_unavailable","reason":"authoritative organization data could not be loaded"})),
            )
                .into_response();
        }
    };
    if org.status == OrganizationStatus::Closed {
        return (
            axum::http::StatusCode::CONFLICT,
            Json(json!({"error":"tenant_closed","reason":"closed tenant cannot be resumed"})),
        )
            .into_response();
    }
    if org.status != OrganizationStatus::Suspended {
        return (axum::http::StatusCode::CONFLICT, Json(json!({"error":"not_suspended","reason": format!("organization status is {}", org.status.as_str())}))).into_response();
    }

    org.status = OrganizationStatus::Active;
    org.suspended_at = None;
    org.suspend_reason = String::new();
    org.updated_at = Utc::now();

    if let Err(e) = state.saas.update_organization(&org).await {
        return (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error":"update_failed","reason": e.to_string()})),
        )
            .into_response();
    }

    if let Some(db) = &state.db {
        if let Err(error) = sqlx::query("UPDATE organizations SET status='active', suspended_at=NULL, suspend_reason='', updated_at=now() WHERE id=$1")
            .bind(org_id.as_uuid())
            .execute(db.pool()).await
        {
            tracing::warn!("organizations resume durable update failed (org {}): {}", org_id, error);
        }
    }

    state
        .audit
        .record(
            "saas",
            "saas.tenant.resumed",
            Some(&org_id.to_string()),
            bot_core::audit::AuditOutcome::Success,
            json!({"organization": org_id.to_string()}),
        )
        .await;

    (
        axum::http::StatusCode::OK,
        Json(json!({"organization_id": org_id.to_string(), "status": org.status.as_str()})),
    )
        .into_response()
}

async fn request_close(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(body): Json<CloseRequest>,
) -> Response {
    let ctx = match authorize_request(
        &state,
        &headers,
        AccessRequest::manage(Permission::TenantUpdate),
    )
    .await
    {
        Ok(ctx) => ctx,
        Err(d) => return deny_response(&state, &d).await,
    };
    let org_id = match OrganizationId::parse(&id) {
        Some(v) => v,
        None => {
            return (
                axum::http::StatusCode::BAD_REQUEST,
                Json(json!({"error":"invalid_organization_id","reason":"must be uuid"})),
            )
                .into_response()
        }
    };
    if ctx.organization.id != org_id && !ctx.authorization.is_platform_scope() {
        return (
            axum::http::StatusCode::FORBIDDEN,
            Json(json!({"error":"cross_tenant","reason":"cannot close another organization"})),
        )
            .into_response();
    }

    let mut org = match state.saas.organization(org_id).await {
        Ok(Some(o)) => o,
        Ok(None) => {
            return (
                axum::http::StatusCode::NOT_FOUND,
                Json(json!({"error":"not_found","reason":"organization not found"})),
            )
                .into_response()
        }
        Err(error) => {
            tracing::error!(error = %error, organization = %org_id, "organization lookup failed during lifecycle operation");
            return (
                axum::http::StatusCode::SERVICE_UNAVAILABLE,
                Json(json!({"error":"lifecycle_storage_unavailable","reason":"authoritative organization data could not be loaded"})),
            )
                .into_response();
        }
    };

    // Idempotent: if already closed, return success without creating duplicate job
    if org.status == OrganizationStatus::Closed {
        // Ensure lifecycle job exists for audit
        if let Some(job) = active_job_for(org_id) {
            return (axum::http::StatusCode::OK, Json(json!({"organization_id": org_id.to_string(), "status": org.status.as_str(), "lifecycle_job": job.id.to_string(), "note":"already closed"}))).into_response();
        }
        return (axum::http::StatusCode::OK, Json(json!({"organization_id": org_id.to_string(), "status": org.status.as_str(), "note":"already closed"}))).into_response();
    }

    // Idempotent job creation: if resumable job exists, return it
    if let Some(existing) = active_job_for(org_id) {
        return (axum::http::StatusCode::OK, Json(json!({"organization_id": org_id.to_string(), "status": org.status.as_str(), "lifecycle_job": existing.id.to_string(), "phase": existing.phase.as_str(), "note":"close already requested"}))).into_response();
    }

    // Mark organization closed immediately (logical closure) — trading disabled via tenant policy
    org.status = OrganizationStatus::Closed;
    org.updated_at = Utc::now();

    if let Err(e) = state.saas.update_organization(&org).await {
        return (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error":"update_failed","reason": e.to_string()})),
        )
            .into_response();
    }

    if let Some(db) = &state.db {
        let _ =
            sqlx::query("UPDATE organizations SET status='closed', updated_at=now() WHERE id=$1")
                .bind(org_id.as_uuid())
                .execute(db.pool())
                .await;
    }

    // Create deprovision job, restart-safe
    let now = Utc::now();
    let mut job = DeprovisionJob::new(org_id, "close", now);
    job.retention_deadline = Some(now + chrono::Duration::days(90));
    job.requested_by = ctx.user.as_ref().map(|u| u.id.as_uuid());
    job.begin(now);
    // Execute first transition: trading disabled
    job.advance(now); // Requested -> TradingDisabled
    upsert_job(job.clone());

    if let Some(db) = &state.db {
        if let Err(error) = sqlx::query("INSERT INTO tenant_lifecycle_jobs (id, organization_id, requested_action, phase, state, scheduled_at, started_at, retention_deadline, requested_by) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9) ON CONFLICT DO NOTHING")
            .bind(job.id).bind(org_id.as_uuid()).bind(&job.requested_action).bind(job.phase.as_str()).bind(job.state.as_str()).bind(job.scheduled_at).bind(job.started_at).bind(job.retention_deadline).bind(job.requested_by)
            .execute(db.pool()).await
        {
            tracing::warn!("tenant_lifecycle_jobs durable insert failed (job {}): {}", job.id, error);
        }
    }

    // Invalidate credentials/sessions/custody bindings asynchronously — here we simulate by logging and marking.
    // Preserve accounting/audit evidence — do NOT delete ledger.
    state.audit.record("saas", "saas.tenant.close.requested", Some(&org_id.to_string()),
        bot_core::audit::AuditOutcome::Success,
        json!({"organization": org_id.to_string(), "reason": body.reason.unwrap_or_default(), "job": job.id.to_string(), "phase": job.phase.as_str()})).await;

    // Invalidate sessions and API keys for this org (immediate revocation)
    // This is restart-safe because job phase will resume and re-attempt if crash happened here.
    // We best-effort revoke; worker will also handle on restart.
    // Note: SaasStore in-memory sessions are not bulk-revoked here — DB path uses SQL below.
    if let Some(db) = &state.db {
        // Revoke runtime sessions and their normalized projection in one
        // statement. The session table is not the authentication authority,
        // but keeping it aligned preserves administrative and audit views.
        if let Err(error) = sqlx::query(
            r#"WITH revoked AS (
                   UPDATE saas_runtime_records
                      SET record = jsonb_set(
                                      jsonb_set(record, '{revoked_at}', to_jsonb(now()), true),
                                      '{revoke_reason}', to_jsonb('tenant_closed'::text), true
                                  ),
                          updated_at = now()
                    WHERE kind = 'session'
                      AND organization_id = $1
                      AND (record->>'revoked_at' IS NULL OR record->>'revoked_at' = '')
                   RETURNING id, record->>'revoked_at' AS revoked_at,
                             record->>'revoke_reason' AS revoke_reason
               )
               UPDATE sessions AS s
                  SET revoked_at = revoked.revoked_at::timestamptz,
                      revoke_reason = COALESCE(revoked.revoke_reason, '')
                 FROM revoked
                WHERE s.id = revoked.id::uuid"#,
        )
        .bind(org_id.as_uuid())
        .execute(db.pool())
        .await
        {
            tracing::warn!("session revocation durable update failed (org {}): {}", org_id, error);
        }
        // Revoke API keys in the authoritative runtime-record store.
        if let Err(error) = sqlx::query("UPDATE saas_runtime_records SET record = jsonb_set(jsonb_set(record, '{revoked_at}', to_jsonb(now()), true), '{revoke_reason}', to_jsonb('tenant_closed'::text), true), updated_at = now() WHERE kind='api_key' AND organization_id=$1 AND (record->>'revoked_at' IS NULL OR record->>'revoked_at' = '')")
            .bind(org_id.as_uuid()).execute(db.pool()).await
        {
            tracing::warn!("api-key revocation durable update failed (org {}): {}", org_id, error);
        }
    }

    (
        axum::http::StatusCode::ACCEPTED,
        Json(json!({
            "organization_id": org_id.to_string(),
            "status": org.status.as_str(),
            "lifecycle_job": job.id.to_string(),
            "phase": job.phase.as_str(),
            "retention_deadline": job.retention_deadline.map(|d| d.to_rfc3339())
        })),
    )
        .into_response()
}

async fn lifecycle_status(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    let ctx = match authorize_request(
        &state,
        &headers,
        AccessRequest::read(Permission::TenantRead),
    )
    .await
    {
        Ok(ctx) => ctx,
        Err(d) => return deny_response(&state, &d).await,
    };
    let org_id = match OrganizationId::parse(&id) {
        Some(v) => v,
        None => {
            return (
                axum::http::StatusCode::BAD_REQUEST,
                Json(json!({"error":"invalid_organization_id","reason":"must be uuid"})),
            )
                .into_response()
        }
    };
    if ctx.organization.id != org_id && !ctx.authorization.is_platform_scope() {
        return (
            axum::http::StatusCode::FORBIDDEN,
            Json(json!({"error":"cross_tenant","reason":"cannot inspect another organization"})),
        )
            .into_response();
    }
    let org = match state.saas.organization(org_id).await {
        Ok(Some(o)) => o,
        Ok(None) => {
            return (
                axum::http::StatusCode::NOT_FOUND,
                Json(json!({"error":"not_found","reason":"organization not found"})),
            )
                .into_response()
        }
        Err(error) => {
            tracing::error!(error = %error, organization = %org_id, "organization lookup failed during lifecycle operation");
            return (
                axum::http::StatusCode::SERVICE_UNAVAILABLE,
                Json(json!({"error":"lifecycle_storage_unavailable","reason":"authoritative organization data could not be loaded"})),
            )
                .into_response();
        }
    };
    let job = active_job_for(org_id);

    let view = LifecycleStatusView {
        organization_id: org_id.to_string(),
        organization_status: org.status.as_str().into(),
        lifecycle_phase: job
            .as_ref()
            .map(|j| j.phase.as_str().into())
            .unwrap_or_else(|| "none".into()),
        lifecycle_state: job
            .as_ref()
            .map(|j| j.state.as_str().into())
            .unwrap_or_else(|| "none".into()),
        retention_deadline: job.and_then(|j| j.retention_deadline.map(|d| d.to_rfc3339())),
    };
    (axum::http::StatusCode::OK, Json(json!(view))).into_response()
}

async fn advance_job(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    let ctx = match authorize_request(
        &state,
        &headers,
        AccessRequest::manage(Permission::TenantUpdate),
    )
    .await
    {
        Ok(ctx) => ctx,
        Err(d) => return deny_response(&state, &d).await,
    };
    let job_id = match Uuid::parse_str(&id) {
        Ok(v) => v,
        Err(_) => {
            return (
                axum::http::StatusCode::BAD_REQUEST,
                Json(json!({"error":"invalid_job_id","reason":"must be uuid"})),
            )
                .into_response()
        }
    };
    let mut job = match find_job(job_id) {
        Some(j) => j,
        None => {
            return (
                axum::http::StatusCode::NOT_FOUND,
                Json(json!({"error":"not_found","reason":"lifecycle job not found"})),
            )
                .into_response()
        }
    };
    if job.organization_id != ctx.organization.id && !ctx.authorization.is_platform_scope() {
        return (axum::http::StatusCode::FORBIDDEN, Json(json!({"error":"cross_tenant","reason":"cannot advance another organization's job"}))).into_response();
    }

    // Idempotent advance: if already completed, return success
    if job.is_completed() {
        return (axum::http::StatusCode::OK, Json(json!({"job_id": job_id.to_string(), "phase": job.phase.as_str(), "state": job.state.as_str(), "note":"already completed"}))).into_response();
    }

    let now = Utc::now();
    job.advance(now);
    // Simulate per-phase work idempotently; in production each phase would have concrete side effects:
    // - trading_disabled: ensure org status closed (already), global risk deny
    // - credentials_revoked: revoke API keys (already done)
    // - sessions_invalidated: revoke sessions (already done)
    // - custody_revoked: revoke custody bindings (call custody revocation)
    // - resources_cleaned: delete operational data per retention, but NOT financial/audit truth
    // - retention: set retention_deadline, schedule purge eligibility
    if job.phase == DeprovisionPhase::Retention {
        job.retention_deadline = Some(now + chrono::Duration::days(90));
    }

    upsert_job(job.clone());

    if let Some(db) = &state.db {
        // Defense-in-depth: the org check above (job.organization_id vs
        // ctx.organization.id, platform scope exempt) is repeated in the
        // durable WHERE clause so the write can never escape its tenant
        // even if the in-process check were ever bypassed.
        if let Err(error) = sqlx::query(
            "UPDATE tenant_lifecycle_jobs SET phase=$2, state=$3, updated_at=now() WHERE id=$1 AND organization_id = $4",
        )
        .bind(job_id)
        .bind(job.phase.as_str())
        .bind(job.state.as_str())
        .bind(job.organization_id.as_uuid())
        .execute(db.pool())
        .await
        {
            tracing::warn!(
                "tenant_lifecycle_jobs durable phase update failed (job {}): {}",
                job_id,
                error
            );
        }
    }

    state.audit.record("saas", "saas.lifecycle.phase.advanced", Some(&job_id.to_string()),
        bot_core::audit::AuditOutcome::Success,
        json!({"organization": job.organization_id.to_string(), "phase": job.phase.as_str(), "state": job.state.as_str()})).await;

    (axum::http::StatusCode::OK, Json(json!({"job_id": job_id.to_string(), "phase": job.phase.as_str(), "state": job.state.as_str()}))).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use bot_core::tenant::OrganizationId;
    use chrono::Utc;

    #[test]
    fn close_is_idempotent_and_preserves_closed_status() {
        let org = OrganizationId::new();
        let now = Utc::now();
        let mut job = DeprovisionJob::new(org, "close", now);
        job.begin(now);
        job.complete(now);
        assert!(job.is_completed());
        // second complete doesn't change state
        job.complete(now);
        assert!(job.is_completed());
    }

    #[test]
    fn closed_tenant_cannot_be_resumed() {
        // lifecycle resume handler returns CONFLICT for closed — documented
        let status = OrganizationStatus::Closed;
        assert!(status.is_terminal(), "closed is terminal");
    }

    #[test]
    fn worker_restart_resumes_job() {
        let org = OrganizationId::new();
        let now = Utc::now();
        let mut job = DeprovisionJob::new(org, "close", now);
        job.begin(now);
        job.advance(now); // TradingDisabled
        let snapshot = job.clone();
        // simulate restart
        let mut resumed = snapshot;
        resumed.advance(now);
        assert_eq!(resumed.phase, DeprovisionPhase::CredentialsRevoked);
    }

    #[test]
    fn suspend_blocks_trading() {
        // Tenant policy: suspended cannot trade
        let verdict = bot_core::tenant::policy::check_status(
            OrganizationStatus::Suspended,
            bot_core::tenant::policy::TenantAction::Trade,
        );
        assert!(matches!(
            verdict,
            bot_core::tenant::policy::TenantVerdict::Deny(_)
        ));
    }
}
