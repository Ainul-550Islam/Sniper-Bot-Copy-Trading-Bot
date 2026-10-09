//! Durable tenant outbound webhook management and delivery.
//!
//! Endpoint metadata and delivery attempts are PostgreSQL-authoritative. The
//! handler never creates demo rows, never reports a delivery that was not
//! attempted, and never returns a secret after the create/rotate response.
//! Secrets are encrypted at rest with AES-256-GCM using the mandatory
//! `WEBHOOK_SECRET_ENCRYPTION_KEY` deployment key; legacy plaintext rows are
//! refused for delivery until explicitly rotated.

use std::net::{IpAddr, SocketAddr};
use std::time::{Duration as StdDuration, SystemTime, UNIX_EPOCH};

use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::{Json, Router};
use base64::{
    engine::general_purpose::{STANDARD, STANDARD_NO_PAD},
    Engine as _,
};
use chrono::{DateTime, Utc};
use futures::StreamExt;
use hmac::{Hmac, Mac};
use reqwest::redirect::Policy;
use ring::aead::{Aad, LessSafeKey, Nonce, UnboundKey, AES_256_GCM};
use ring::rand::{SecureRandom, SystemRandom};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};
use sqlx::Row;
use tokio::net::lookup_host;
use url::Url;
use uuid::Uuid;

use bot_core::authorization::AccessRequest;
use bot_core::membership::Permission;
use bot_core::session::token::generate_token;

use crate::api::ApiState;
use crate::saas::middleware::{authorize_request, deny_response};

const MAX_EVENT_TYPES: usize = 32;
const MAX_EVENT_TYPE_LENGTH: usize = 64;
const MAX_DESCRIPTION_LENGTH: usize = 256;
pub(crate) const MAX_RESPONSE_BODY_DIGEST_BYTES: usize = 64;
pub(crate) const MAX_RESPONSE_BODY_BYTES: usize = 1_048_576;
pub(crate) const DELIVERY_TIMEOUT: StdDuration = StdDuration::from_secs(10);
const WEBHOOK_SECRET_KEY_ENV: &str = "WEBHOOK_SECRET_ENCRYPTION_KEY";
const ENCRYPTED_SECRET_PREFIX: &str = "enc:v1:";
const SECRET_NONCE_BYTES: usize = 12;

type HmacSha256 = Hmac<Sha256>;

#[derive(Debug, Clone, Serialize)]
pub struct WebhookEndpoint {
    pub id: String,
    pub organization_id: String,
    pub url: String,
    pub description: String,
    pub events: Vec<String>,
    pub is_active: bool,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateWebhookBody {
    pub url: String,
    #[serde(default)]
    pub description: String,
    pub events: Vec<String>,
}

#[derive(Debug, Clone)]
struct StoredEndpoint {
    id: Uuid,
    organization_id: Uuid,
    url: String,
    secret: String,
    description: String,
    events: Vec<String>,
    created_at: DateTime<Utc>,
}

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
            "/api/saas/webhooks/:id/rotate",
            axum::routing::post(rotate_webhook),
        )
        .route(
            "/api/saas/webhooks/:id/test",
            axum::routing::post(test_webhook),
        )
}

fn error_response(status: StatusCode, error: &'static str, reason: impl Into<String>) -> Response {
    (
        status,
        Json(json!({
            "error": error,
            "reason": reason.into(),
        })),
    )
        .into_response()
}

fn database(state: &ApiState) -> Result<&bot_core::db::Database, Response> {
    state.db.as_deref().ok_or_else(|| {
        error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "webhook_storage_unavailable",
            "webhook storage requires an attached PostgreSQL database",
        )
    })
}

fn validate_event_types(events: &[String]) -> Result<Vec<String>, String> {
    if events.is_empty() {
        return Err("at least one event type is required".into());
    }
    if events.len() > MAX_EVENT_TYPES {
        return Err(format!(
            "no more than {MAX_EVENT_TYPES} event types are allowed"
        ));
    }

    let mut normalized = Vec::with_capacity(events.len());
    for raw in events {
        let event = raw.trim();
        if event.is_empty() || event.len() > MAX_EVENT_TYPE_LENGTH {
            return Err(format!(
                "event type must be between 1 and {MAX_EVENT_TYPE_LENGTH} characters"
            ));
        }
        if !event
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | ':' | '-'))
        {
            return Err(format!(
                "event type '{event}' contains unsupported characters"
            ));
        }
        if !normalized.iter().any(|existing| existing == event) {
            normalized.push(event.to_string());
        }
    }
    Ok(normalized)
}

fn forbidden_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(value) => {
            value.is_private()
                || value.is_loopback()
                || value.is_link_local()
                || value.is_unspecified()
                || value.is_multicast()
                || value.octets()[0] == 0
                || value.octets()[0] == 100 && (value.octets()[1] & 0b1100_0000) == 0b0100_0000
                || value.octets()[0] == 198 && matches!(value.octets()[1], 18 | 19)
                || value.octets()[0] == 198 && value.octets()[1] == 51 && value.octets()[2] == 100
                || value.octets()[0] == 203 && value.octets()[1] == 0 && value.octets()[2] == 113
        }
        IpAddr::V6(value) => {
            value.is_loopback()
                || value.is_unspecified()
                || value.is_multicast()
                || (value.segments()[0] & 0xfe00) == 0xfc00
                || (value.segments()[0] & 0xffc0) == 0xfe80
                || value
                    .to_ipv4()
                    .is_some_and(|mapped| forbidden_ip(IpAddr::V4(mapped)))
        }
    }
}

pub(crate) fn parse_https_url(raw: &str) -> Result<Url, String> {
    let url = Url::parse(raw.trim()).map_err(|_| "url must be a valid absolute URL".to_string())?;
    if url.scheme() != "https" {
        return Err("webhook URL must use HTTPS".into());
    }
    if url.host_str().is_none() {
        return Err("webhook URL must contain a host".into());
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err("webhook URL must not contain userinfo".into());
    }
    let host = url.host_str().unwrap_or_default().to_ascii_lowercase();
    if host == "localhost"
        || host.ends_with(".localhost")
        || host.ends_with(".local")
        || host.ends_with(".internal")
        || host == "metadata.google.internal"
    {
        return Err("webhook host is not publicly routable".into());
    }
    if let Ok(ip) = host.parse::<IpAddr>() {
        if forbidden_ip(ip) {
            return Err("webhook host resolves to a reserved or private address".into());
        }
    }
    Ok(url)
}

pub(crate) async fn resolve_public_socket(url: &Url) -> Result<SocketAddr, String> {
    let host = url
        .host_str()
        .ok_or_else(|| "webhook host is missing".to_string())?;
    let port = url
        .port_or_known_default()
        .ok_or_else(|| "webhook port is unavailable".to_string())?;
    let mut addresses = lookup_host((host, port))
        .await
        .map_err(|_| "webhook host could not be resolved".to_string())?;
    addresses
        .find(|address| !forbidden_ip(address.ip()))
        .ok_or_else(|| "webhook host resolves only to reserved or private addresses".to_string())
}

pub(crate) fn sign_payload(secret: &str, timestamp: u64, body: &[u8]) -> String {
    let mut mac = HmacSha256::new_from_slice(secret.as_bytes())
        .expect("HMAC accepts secrets of every non-empty length");
    mac.update(timestamp.to_string().as_bytes());
    mac.update(b".");
    mac.update(body);
    hex::encode(mac.finalize().into_bytes())
}

fn webhook_secret_key() -> Result<[u8; 32], String> {
    let encoded = std::env::var(WEBHOOK_SECRET_KEY_ENV).map_err(|_| {
        format!("{WEBHOOK_SECRET_KEY_ENV} must be configured as a base64-encoded 32-byte key")
    })?;
    let decoded = STANDARD_NO_PAD
        .decode(encoded.trim())
        .or_else(|_| STANDARD.decode(encoded.trim()))
        .map_err(|_| format!("{WEBHOOK_SECRET_KEY_ENV} is not valid base64"))?;
    let key: [u8; 32] = decoded
        .try_into()
        .map_err(|_| format!("{WEBHOOK_SECRET_KEY_ENV} must decode to exactly 32 bytes"))?;
    Ok(key)
}

fn encrypt_webhook_secret(secret: &str) -> Result<String, String> {
    let key = webhook_secret_key()?;
    let unbound = UnboundKey::new(&AES_256_GCM, &key)
        .map_err(|_| "webhook secret encryption key could not be initialized".to_string())?;
    let sealing_key = LessSafeKey::new(unbound);
    let mut nonce_bytes = [0u8; SECRET_NONCE_BYTES];
    SystemRandom::new().fill(&mut nonce_bytes).map_err(|_| {
        "secure randomness for webhook secret encryption is unavailable".to_string()
    })?;
    let nonce = Nonce::assume_unique_for_key(nonce_bytes);
    let mut ciphertext = secret.as_bytes().to_vec();
    sealing_key
        .seal_in_place_append_tag(nonce, Aad::empty(), &mut ciphertext)
        .map_err(|_| "webhook secret encryption failed".to_string())?;
    let mut encoded = Vec::with_capacity(nonce_bytes.len() + ciphertext.len());
    encoded.extend_from_slice(&nonce_bytes);
    encoded.extend_from_slice(&ciphertext);
    Ok(format!(
        "{ENCRYPTED_SECRET_PREFIX}{}",
        STANDARD_NO_PAD.encode(encoded)
    ))
}

pub(crate) fn decrypt_webhook_secret(stored: &str) -> Result<String, String> {
    let encoded = stored
        .strip_prefix(ENCRYPTED_SECRET_PREFIX)
        .ok_or_else(|| {
            "stored webhook secret is not encrypted; rotate the endpoint before delivery"
                .to_string()
        })?;
    let decoded = STANDARD_NO_PAD
        .decode(encoded)
        .map_err(|_| "stored webhook secret ciphertext is not valid base64".to_string())?;
    if decoded.len() <= SECRET_NONCE_BYTES {
        return Err("stored webhook secret ciphertext is truncated".to_string());
    }
    let key = webhook_secret_key()?;
    let unbound = UnboundKey::new(&AES_256_GCM, &key)
        .map_err(|_| "webhook secret encryption key could not be initialized".to_string())?;
    let opening_key = LessSafeKey::new(unbound);
    let nonce_bytes: [u8; SECRET_NONCE_BYTES] = decoded[..SECRET_NONCE_BYTES]
        .try_into()
        .map_err(|_| "stored webhook secret nonce is invalid".to_string())?;
    let nonce = Nonce::assume_unique_for_key(nonce_bytes);
    let mut plaintext = decoded[SECRET_NONCE_BYTES..].to_vec();
    let plaintext = opening_key
        .open_in_place(nonce, Aad::empty(), &mut plaintext)
        .map_err(|_| "stored webhook secret could not be decrypted".to_string())?;
    String::from_utf8(plaintext.to_vec())
        .map_err(|_| "stored webhook secret plaintext is not valid UTF-8".to_string())
}

fn public_endpoint(row: StoredEndpoint) -> WebhookEndpoint {
    WebhookEndpoint {
        id: row.id.to_string(),
        organization_id: row.organization_id.to_string(),
        url: row.url,
        description: row.description,
        events: row.events,
        is_active: true,
        created_at: row.created_at,
    }
}

async fn list_webhooks(State(state): State<ApiState>, headers: HeaderMap) -> Response {
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
    let db = match database(&state) {
        Ok(value) => value,
        Err(response) => return response,
    };

    let rows = match sqlx::query(
        "SELECT id, organization_id, url, description, event_types, created_at
           FROM webhook_endpoints
          WHERE organization_id = $1 AND status = 'active'
          ORDER BY created_at DESC, id DESC",
    )
    .bind(ctx.organization.id.as_uuid())
    .fetch_all(db.pool())
    .await
    {
        Ok(value) => value,
        Err(error) => {
            tracing::error!(error = %error, "failed to list tenant webhook endpoints");
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "webhook_storage_error",
                "webhook endpoints could not be loaded",
            );
        }
    };

    let mut items = Vec::with_capacity(rows.len());
    for row in rows {
        let description: String = match row.try_get("description") {
            Ok(value) => value,
            Err(error) => {
                tracing::error!(error = %error, "webhook endpoint description column could not be decoded");
                return error_response(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "webhook_storage_error",
                    "webhook endpoint data is invalid",
                );
            }
        };
        let events: Vec<String> = match row.try_get("event_types") {
            Ok(value) => value,
            Err(error) => {
                tracing::error!(error = %error, "webhook endpoint event types could not be decoded");
                return error_response(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "webhook_storage_error",
                    "webhook endpoint data is invalid",
                );
            }
        };
        let endpoint = StoredEndpoint {
            id: row.get("id"),
            organization_id: row.get("organization_id"),
            url: row.get("url"),
            secret: String::new(),
            description,
            events,
            created_at: row.get("created_at"),
        };
        items.push(public_endpoint(endpoint));
    }

    (
        StatusCode::OK,
        Json(json!({
            "organization_id": ctx.organization.id.to_string(),
            "items": items,
            "count": items.len(),
        })),
    )
        .into_response()
}

async fn create_webhook(
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
    let db = match database(&state) {
        Ok(value) => value,
        Err(response) => return response,
    };
    let url = match parse_https_url(&body.url) {
        Ok(value) => value,
        Err(reason) => {
            return error_response(StatusCode::BAD_REQUEST, "invalid_webhook_url", reason)
        }
    };
    if body.description.trim().len() > MAX_DESCRIPTION_LENGTH {
        return error_response(
            StatusCode::BAD_REQUEST,
            "invalid_webhook_description",
            format!("description must be no longer than {MAX_DESCRIPTION_LENGTH} characters"),
        );
    }
    let events = match validate_event_types(&body.events) {
        Ok(value) => value,
        Err(reason) => {
            return error_response(StatusCode::BAD_REQUEST, "invalid_webhook_events", reason)
        }
    };
    if let Err(reason) = resolve_public_socket(&url).await {
        return error_response(StatusCode::BAD_REQUEST, "webhook_host_unavailable", reason);
    }

    let id = Uuid::new_v4();
    let generated = generate_token("whsec");
    let encrypted_secret = match encrypt_webhook_secret(&generated.plaintext) {
        Ok(value) => value,
        Err(reason) => {
            tracing::error!(error = %reason, "webhook secret encryption is unavailable");
            return error_response(
                StatusCode::SERVICE_UNAVAILABLE,
                "webhook_secret_storage_unavailable",
                "webhook endpoint creation requires configured secret encryption",
            );
        }
    };
    let now = Utc::now();
    let description = body.description.trim().to_string();
    let insert = sqlx::query(
        "INSERT INTO webhook_endpoints
             (id, organization_id, url, secret, description, event_types, status, created_at, updated_at)
         VALUES ($1, $2, $3, $4, $5, $6, 'active', $7, $7)",
    )
    .bind(id)
    .bind(ctx.organization.id.as_uuid())
    .bind(url.as_str())
    .bind(&encrypted_secret)
    .bind(&description)
    .bind(&events)
    .bind(now)
    .execute(db.pool())
    .await;

    if let Err(error) = insert {
        tracing::error!(error = %error, "failed to create tenant webhook endpoint");
        return error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "webhook_storage_error",
            "webhook endpoint could not be created",
        );
    }

    state
        .audit
        .success(
            &ctx.actor_label(),
            "saas.webhooks.endpoint_created",
            Some(&id.to_string()),
        )
        .await;

    (
        StatusCode::CREATED,
        Json(json!({
            "id": id,
            "organization_id": ctx.organization.id.to_string(),
            "url": url.as_str(),
            "description": description,
            "events": events,
            "secret": generated.plaintext,
            "warning": "store this secret now; it cannot be retrieved again",
            "is_active": true,
            "created_at": now,
        })),
    )
        .into_response()
}

async fn rotate_webhook(
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
    let db = match database(&state) {
        Ok(value) => value,
        Err(response) => return response,
    };
    let id = match Uuid::parse_str(&webhook_id) {
        Ok(value) => value,
        Err(_) => {
            return error_response(
                StatusCode::NOT_FOUND,
                "webhook_not_found",
                "webhook endpoint was not found",
            )
        }
    };
    let generated = generate_token("whsec");
    let encrypted_secret = match encrypt_webhook_secret(&generated.plaintext) {
        Ok(value) => value,
        Err(reason) => {
            tracing::error!(error = %reason, "webhook secret rotation encryption is unavailable");
            return error_response(
                StatusCode::SERVICE_UNAVAILABLE,
                "webhook_secret_storage_unavailable",
                "webhook secret rotation requires configured secret encryption",
            );
        }
    };
    let result = sqlx::query(
        "UPDATE webhook_endpoints
            SET secret = $1, updated_at = now()
          WHERE id = $2 AND organization_id = $3 AND status = 'active'",
    )
    .bind(&encrypted_secret)
    .bind(id)
    .bind(ctx.organization.id.as_uuid())
    .execute(db.pool())
    .await;
    match result {
        Ok(done) if done.rows_affected() == 1 => {
            state
                .audit
                .success(
                    &ctx.actor_label(),
                    "saas.webhooks.endpoint_secret_rotated",
                    Some(&id.to_string()),
                )
                .await;
            (
                StatusCode::OK,
                Json(json!({
                    "id": id,
                    "secret": generated.plaintext,
                    "warning": "store this secret now; it cannot be retrieved again",
                })),
            )
                .into_response()
        }
        Ok(_) => error_response(
            StatusCode::NOT_FOUND,
            "webhook_not_found",
            "webhook endpoint was not found",
        ),
        Err(error) => {
            tracing::error!(error = %error, "failed to rotate tenant webhook secret");
            error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "webhook_storage_error",
                "webhook secret could not be rotated",
            )
        }
    }
}

async fn delete_webhook(
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
    let db = match database(&state) {
        Ok(value) => value,
        Err(response) => return response,
    };
    let id = match Uuid::parse_str(&webhook_id) {
        Ok(value) => value,
        Err(_) => {
            return error_response(
                StatusCode::NOT_FOUND,
                "webhook_not_found",
                "webhook endpoint was not found",
            )
        }
    };

    let result = sqlx::query(
        "UPDATE webhook_endpoints
            SET status = 'deleted', updated_at = now()
          WHERE id = $1 AND organization_id = $2 AND status = 'active'",
    )
    .bind(id)
    .bind(ctx.organization.id.as_uuid())
    .execute(db.pool())
    .await;

    match result {
        Ok(done) if done.rows_affected() == 1 => {
            state
                .audit
                .success(
                    &ctx.actor_label(),
                    "saas.webhooks.endpoint_deleted",
                    Some(&id.to_string()),
                )
                .await;
            (StatusCode::OK, Json(json!({ "success": true, "id": id }))).into_response()
        }
        Ok(_) => error_response(
            StatusCode::NOT_FOUND,
            "webhook_not_found",
            "webhook endpoint was not found",
        ),
        Err(error) => {
            tracing::error!(error = %error, "failed to delete tenant webhook endpoint");
            error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "webhook_storage_error",
                "webhook endpoint could not be deleted",
            )
        }
    }
}

async fn load_endpoint(
    db: &bot_core::db::Database,
    organization_id: &bot_core::tenant::OrganizationId,
    endpoint_id: Uuid,
) -> Result<Option<StoredEndpoint>, Response> {
    let row = sqlx::query(
        "SELECT id, organization_id, url, secret, description, event_types, created_at
           FROM webhook_endpoints
          WHERE id = $1 AND organization_id = $2 AND status = 'active'",
    )
    .bind(endpoint_id)
    .bind(organization_id.as_uuid())
    .fetch_optional(db.pool())
    .await
    .map_err(|error| {
        tracing::error!(error = %error, "failed to load tenant webhook endpoint");
        error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "webhook_storage_error",
            "webhook endpoint could not be loaded",
        )
    })?;

    let Some(value) = row else {
        return Ok(None);
    };
    let description: String = value.try_get("description").map_err(|error| {
        tracing::error!(error = %error, "webhook endpoint description column could not be decoded");
        error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "webhook_storage_error",
            "webhook endpoint data is invalid",
        )
    })?;
    let events: Vec<String> = value.try_get("event_types").map_err(|error| {
        tracing::error!(error = %error, "webhook endpoint event types could not be decoded");
        error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "webhook_storage_error",
            "webhook endpoint data is invalid",
        )
    })?;
    let stored_secret: String = value.try_get("secret").map_err(|error| {
        tracing::error!(error = %error, "webhook endpoint secret column could not be decoded");
        error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "webhook_storage_error",
            "webhook endpoint secret data is invalid",
        )
    })?;
    let secret = decrypt_webhook_secret(&stored_secret).map_err(|reason| {
        tracing::error!(error = %reason, "webhook endpoint secret could not be decrypted");
        error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "webhook_secret_unavailable",
            "webhook endpoint secret cannot be used until the endpoint is rotated",
        )
    })?;
    Ok(Some(StoredEndpoint {
        id: value.get("id"),
        organization_id: value.get("organization_id"),
        url: value.get("url"),
        secret,
        description,
        events,
        created_at: value.get("created_at"),
    }))
}

async fn test_webhook(
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
    let db = match database(&state) {
        Ok(value) => value,
        Err(response) => return response,
    };
    let id = match Uuid::parse_str(&webhook_id) {
        Ok(value) => value,
        Err(_) => {
            return error_response(
                StatusCode::NOT_FOUND,
                "webhook_not_found",
                "webhook endpoint was not found",
            )
        }
    };
    let endpoint = match load_endpoint(db, &ctx.organization.id, id).await {
        Ok(Some(value)) => value,
        Ok(None) => {
            return error_response(
                StatusCode::NOT_FOUND,
                "webhook_not_found",
                "webhook endpoint was not found",
            )
        }
        Err(response) => return response,
    };
    let endpoint_url = match parse_https_url(&endpoint.url) {
        Ok(value) => value,
        Err(reason) => {
            return error_response(StatusCode::BAD_GATEWAY, "invalid_webhook_url", reason)
        }
    };
    let public_socket = match resolve_public_socket(&endpoint_url).await {
        Ok(value) => value,
        Err(reason) => {
            return error_response(StatusCode::BAD_GATEWAY, "webhook_host_unavailable", reason)
        }
    };
    let event_id = Uuid::new_v4().to_string();
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or_default();
    let payload = json!({
        "id": event_id,
        "type": "webhook.test",
        "created_at": Utc::now(),
        "organization_id": ctx.organization.id.to_string(),
        "data": { "test": true },
    });
    let body = match serde_json::to_vec(&payload) {
        Ok(value) => value,
        Err(_) => {
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "webhook_payload_error",
                "test payload could not be encoded",
            )
        }
    };
    let signature = sign_payload(&endpoint.secret, timestamp, &body);
    let target = match Url::parse(&endpoint.url) {
        Ok(value) => value,
        Err(_) => {
            return error_response(
                StatusCode::BAD_GATEWAY,
                "invalid_webhook_url",
                "stored webhook URL is invalid",
            )
        }
    };
    let host = target.host_str().unwrap_or_default().to_string();
    let client = match reqwest::Client::builder()
        .redirect(Policy::none())
        .timeout(DELIVERY_TIMEOUT)
        .resolve(&host, public_socket)
        .build()
    {
        Ok(value) => value,
        Err(error) => {
            tracing::error!(error = %error, "failed to construct webhook HTTP client");
            return error_response(
                StatusCode::BAD_GATEWAY,
                "webhook_client_error",
                "webhook delivery client could not be created",
            );
        }
    };

    let started = std::time::Instant::now();
    let delivery = client
        .post(endpoint.url.clone())
        .header("content-type", "application/json")
        .header("user-agent", "sniper-suite-webhook/1")
        .header("x-webhook-id", &event_id)
        .header("x-webhook-timestamp", timestamp.to_string())
        .header("x-webhook-signature", format!("v1={signature}"))
        .body(body.clone())
        .send()
        .await;
    let latency_ms = started.elapsed().as_millis() as i64;

    let (status, response_status, response_digest, delivery_error) = match delivery {
        Ok(response) => {
            let code = response.status().as_u16() as i32;
            let mut stream = response.bytes_stream();
            let mut hasher = Sha256::new();
            let mut total = 0usize;
            let mut body_read_error = None;
            while let Some(chunk) = stream.next().await {
                match chunk {
                    Ok(bytes) => {
                        let remaining = MAX_RESPONSE_BODY_BYTES.saturating_sub(total);
                        let accepted = bytes.len().min(remaining);
                        hasher.update(&bytes[..accepted]);
                        total = total.saturating_add(accepted);
                        if accepted < bytes.len() {
                            break;
                        }
                    }
                    Err(_) => {
                        body_read_error =
                            Some("webhook response body could not be read".to_string());
                        break;
                    }
                }
            }
            let state = if (200..300).contains(&code) {
                "succeeded"
            } else {
                "failed"
            };
            let error = if let Some(read_error) = body_read_error {
                Some(read_error)
            } else if state == "failed" {
                Some(format!("remote endpoint returned HTTP {code}"))
            } else {
                None
            };
            let response_digest =
                hex::encode(hasher.finalize())[..MAX_RESPONSE_BODY_DIGEST_BYTES].to_string();
            (state, Some(code), Some(response_digest), error)
        }
        Err(error) => (
            "failed",
            None,
            None,
            Some(if error.is_timeout() {
                "webhook delivery timed out".to_string()
            } else {
                "webhook delivery failed".to_string()
            }),
        ),
    };

    let delivery_id = Uuid::new_v4();
    // A failed attempt is handed to the retry dispatcher: the payload is
    // persisted and the first retry is scheduled (exponential backoff from
    // `webhook_delivery::retry_backoff`). Succeeded rows carry no schedule.
    let next_retry_at = if status == "failed" {
        Some(Utc::now() + super::webhook_delivery::retry_backoff(1))
    } else {
        None
    };
    let delivery_insert = sqlx::query(
        "INSERT INTO webhook_deliveries
             (id, organization_id, endpoint_id, event_id, event_type, status,
              response_status, attempt_count, error, response_body_digest,
              payload, next_retry_at, created_at)
         VALUES ($1, $2, $3, $4, 'webhook.test', $5, $6, 1, $7, $8, $9, $10, now())",
    )
    .bind(delivery_id)
    .bind(ctx.organization.id.as_uuid())
    .bind(endpoint.id)
    .bind(&event_id)
    .bind(status)
    .bind(response_status)
    .bind(delivery_error.as_deref())
    .bind(response_digest.as_deref())
    .bind(sqlx::types::Json(payload.clone()))
    .bind(next_retry_at)
    .execute(db.pool())
    .await;

    if let Err(error) = delivery_insert {
        tracing::error!(error = %error, "failed to persist webhook delivery result");
        return error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "webhook_delivery_record_error",
            "delivery was attempted but its result could not be recorded",
        );
    }

    state
        .audit
        .success(
            &ctx.actor_label(),
            "saas.webhooks.test_dispatched",
            Some(&endpoint.id.to_string()),
        )
        .await;

    if status == "succeeded" {
        (
            StatusCode::OK,
            Json(json!({
                "success": true,
                "delivery_id": delivery_id,
                "status_code": response_status,
                "latency_ms": latency_ms,
            })),
        )
            .into_response()
    } else {
        error_response(
            StatusCode::BAD_GATEWAY,
            "webhook_delivery_failed",
            delivery_error.unwrap_or_else(|| "webhook delivery failed".to_string()),
        )
    }
}
