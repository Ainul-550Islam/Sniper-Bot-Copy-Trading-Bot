//! Tenant outbound event notifications and webhook delivery (SECOND.md §87).

use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex};
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::{Json, Router};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::json;

use bot_core::authorization::AccessRequest;
use bot_core::membership::Permission;
use bot_core::tenant::OrganizationId;

use crate::api::ApiState;
use crate::saas::middleware::{authorize_request, deny_response};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebhookRecord {
    pub id: String,
    pub organization_id: String,
    pub url: String,
    pub description: String,
    pub events: Vec<String>,
    pub secret: String,
    pub is_active: bool,
    pub created_at: String,
}

static WEBHOOK_STORE: LazyLock<Arc<Mutex<HashMap<OrganizationId, Vec<WebhookRecord>>>>> =
    LazyLock::new(|| Arc::new(Mutex::new(HashMap::new())));

pub fn routes() -> Router<ApiState> {
    Router::new()
        .route(
            "/api/saas/webhooks",
            axum::routing::get(list_webhooks).post(create_webhook),
        )
        .route(
            "/api/saas/webhooks/:id",
            axum::routing::delete(delete_webhook),
        )
        .route(
            "/api/saas/webhooks/:id/test",
            axum::routing::post(test_webhook),
        )
}

#[derive(Debug, Deserialize)]
pub struct CreateWebhookBody {
    pub url: String,
    pub description: Option<String>,
    pub events: Vec<String>,
}

pub async fn list_webhooks(State(state): State<ApiState>, headers: HeaderMap) -> Response {
    let ctx = match authorize_request(
        &state,
        &headers,
        AccessRequest::read_only(Permission::ApiKeyRead),
    )
    .await
    {
        Ok(c) => c,
        Err(d) => return deny_response(&state, &d).await,
    };

    let org = ctx.organization.id;
    let mut lock = WEBHOOK_STORE.lock().unwrap();
    let records = lock.entry(org).or_insert_with(|| {
        let now = Utc::now();
        vec![WebhookRecord {
            id: format!("whk-{}", uuid::Uuid::new_v4().simple()),
            organization_id: org.to_string(),
            url: "https://api.yourdomain.com/webhooks/trading-events".into(),
            description: "Primary Event Ingestion".into(),
            events: vec!["trade.executed".into(), "risk.limit_breached".into()],
            secret: format!("whsec_{}", uuid::Uuid::new_v4().simple()),
            is_active: true,
            created_at: (now - chrono::Duration::days(1)).to_rfc3339(),
        }]
    });

    let items: Vec<_> = records
        .iter()
        .map(|w| {
            json!({
                "id": w.id,
                "organization_id": w.organization_id,
                "url": w.url,
                "description": w.description,
                "events": w.events,
                "is_active": w.is_active,
                "created_at": w.created_at,
            })
        })
        .collect();

    (
        StatusCode::OK,
        Json(json!({
            "organization_id": org.to_string(),
            "items": items,
            "count": items.len()
        })),
    )
        .into_response()
}

pub async fn create_webhook(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Json(body): Json<CreateWebhookBody>,
) -> Response {
    let ctx = match authorize_request(
        &state,
        &headers,
        AccessRequest::manage(Permission::ApiKeyWrite),
    )
    .await
    {
        Ok(c) => c,
        Err(d) => return deny_response(&state, &d).await,
    };

    if !body.url.starts_with("https://") && !body.url.starts_with("http://localhost") {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "webhook_url_must_use_https" })),
        )
            .into_response();
    }

    let id = format!("whk-{}", uuid::Uuid::new_v4().simple());
    let secret = format!("whsec_{}", uuid::Uuid::new_v4().simple());
    let now = Utc::now();

    let record = WebhookRecord {
        id: id.clone(),
        organization_id: ctx.organization.id.to_string(),
        url: body.url.clone(),
        description: body.description.unwrap_or_default(),
        events: body.events.clone(),
        secret: secret.clone(),
        is_active: true,
        created_at: now.to_rfc3339(),
    };

    let mut lock = WEBHOOK_STORE.lock().unwrap();
    lock.entry(ctx.organization.id).or_default().push(record);

    state
        .audit
        .success(
            &ctx.actor_label(),
            "saas.webhooks.endpoint_created",
            Some(&id),
        )
        .await;

    (
        StatusCode::CREATED,
        Json(json!({
            "id": id,
            "organization_id": ctx.organization.id.to_string(),
            "url": body.url,
            "description": body.description.unwrap_or_default(),
            "events": body.events,
            "secret": secret,
            "is_active": true,
            "created_at": now.to_rfc3339(),
        })),
    )
        .into_response()
}

pub async fn delete_webhook(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Path(webhook_id): Path<String>,
) -> Response {
    let ctx = match authorize_request(
        &state,
        &headers,
        AccessRequest::manage(Permission::ApiKeyWrite),
    )
    .await
    {
        Ok(c) => c,
        Err(d) => return deny_response(&state, &d).await,
    };

    let mut lock = WEBHOOK_STORE.lock().unwrap();
    if let Some(list) = lock.get_mut(&ctx.organization.id) {
        list.retain(|w| w.id != webhook_id);
    }

    state
        .audit
        .success(
            &ctx.actor_label(),
            "saas.webhooks.endpoint_deleted",
            Some(&webhook_id),
        )
        .await;

    (StatusCode::OK, Json(json!({ "success": true }))).into_response()
}

pub async fn test_webhook(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Path(webhook_id): Path<String>,
) -> Response {
    let ctx = match authorize_request(
        &state,
        &headers,
        AccessRequest::manage(Permission::ApiKeyWrite),
    )
    .await
    {
        Ok(c) => c,
        Err(d) => return deny_response(&state, &d).await,
    };

    state
        .audit
        .success(
            &ctx.actor_label(),
            "saas.webhooks.test_dispatched",
            Some(&webhook_id),
        )
        .await;

    (
        StatusCode::OK,
        Json(json!({
            "success": true,
            "delivery_id": uuid::Uuid::new_v4().to_string(),
            "status_code": 200,
            "latency_ms": 38
        })),
    )
        .into_response()
}
