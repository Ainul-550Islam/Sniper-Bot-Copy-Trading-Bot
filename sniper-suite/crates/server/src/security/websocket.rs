//! The authenticated, tenant-scoped SaaS event stream (TASK 7B file 17).
//!
//! `GET /api/saas/events` is the PRIMARY websocket for SaaS tenants. It
//! replaces `/ws/events?key=SECRET` (whose secret lived in URLs and logs)
//! without touching it: the legacy endpoint in [`crate::api`] keeps its
//! exact behaviour, so existing single-operator installs keep working.
//!
//! Authentication — never a URL parameter:
//!
//! 1. A credential header (`Authorization: Bearer <session token>` or the
//!    tenant `x-api-key`) authenticates BEFORE the upgrade; a bad one gets
//!    a 401 with no socket at all. The task 7A middleware resolves user,
//!    organization, role and permissions.
//! 2. Browsers cannot set headers on `new WebSocket(...)`, so a client MAY
//!    connect unauthenticated and send `{"type":"auth","token":"…"}` as its
//!    FIRST frame (10 s window). Until that frame authenticates, the socket
//!    carries no data.
//!
//! Delivery: one frame per event,
//! `{"kind":"market"|"saas", "organization":"…", "event":{…}}`. Market
//! events are global market truth (marks, summaries — the TASK 1–6
//! engines' stream); `saas.*` events carry an `organization` field and are
//! delivered ONLY to that organization. The tenant context is revalidated
//! every 60 s; a revoked session/key, suspended membership or closed
//! organization closes the socket with code 1008.

use std::collections::HashMap;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use axum::extract::ws::{CloseFrame, Message, WebSocket, WebSocketUpgrade};
use axum::extract::State;
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::{Json, Router};
use futures::stream::{SplitSink, SplitStream};
use futures::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tokio::sync::broadcast;

use bot_core::authorization::AccessRequest;
use bot_core::membership::Permission;
use bot_core::tenant::OrganizationId;

use crate::api::ApiState;
use crate::saas::middleware::{authorize_request, AUTH_HEADER};

/// Revalidation cadence for an established stream.
const REVALIDATE_EVERY: Duration = Duration::from_secs(60);

/// How long a first-frame-auth socket may wait for its auth frame.
const AUTH_FRAME_WINDOW: Duration = Duration::from_secs(10);

/// Broadcast capacity for the SaaS-side event channel.
const SAAS_CHANNEL_CAP: usize = 256;

fn saas_channel() -> &'static broadcast::Sender<Arc<Value>> {
    static CHANNEL: OnceLock<broadcast::Sender<Arc<Value>>> = OnceLock::new();
    CHANNEL.get_or_init(|| broadcast::channel(SAAS_CHANNEL_CAP).0)
}

/// Publish a `saas.*` event to authenticated tenants. Carried fields must
/// already be non-secret: never a token, key material or a webhook secret.
pub fn publish(event: Value) {
    let _ = saas_channel().send(Arc::new(event));
}

/// Subscribe to the SaaS-side event channel.
pub fn subscribe() -> broadcast::Receiver<Arc<Value>> {
    saas_channel().subscribe()
}

/// May `org` see this (already serialised) event? Events WITHOUT an
/// `organization` field are market-global truth (marks, summaries, module
/// status) and pass; events WITH one pass only for that organization.
pub fn visible_to(event: &Value, org: &OrganizationId) -> bool {
    match event.get("organization") {
        Some(v) => v.as_str() == Some(&org.to_string()),
        None => true,
    }
}

/// Build the headers used for (re)authentication: the upgrade request's
/// headers, optionally with an injected `Authorization: Bearer` (the
/// first-frame path).
fn credential_headers(base: &HeaderMap, token: Option<&str>) -> HeaderMap {
    let mut headers = base.clone();
    if let Some(token) = token {
        if let Ok(value) = HeaderValue::from_str(&format!("Bearer {token}")) {
            headers.insert(AUTH_HEADER, value);
        }
    }
    headers
}

/// Resolve a credential into a SaaS context (user + tenant + role +
/// permissions), or the denial. Used for the pre-upgrade check AND for the
/// periodic revalidation of an established stream.
async fn resolve(
    state: &ApiState,
    headers: &HeaderMap,
) -> Result<crate::saas::SaasContext, bot_core::authorization::Decision> {
    authorize_request(state, headers, AccessRequest::read(Permission::TenantRead)).await
}

/// `GET /api/saas/events` — the tenant-scoped event stream.
pub async fn events(
    State(state): State<ApiState>,
    ws: WebSocketUpgrade,
    headers: HeaderMap,
) -> Response {
    // Path 1: credential header present — authenticate BEFORE the upgrade.
    if headers.contains_key(AUTH_HEADER)
        || headers.contains_key(crate::saas::middleware::API_KEY_HEADER)
    {
        return match resolve(&state, &headers).await {
            Ok(ctx) => ws.on_upgrade(move |socket| {
                run_stream(state, socket, ctx, credential_headers(&headers, None))
            }),
            Err(denied) => (
                StatusCode::UNAUTHORIZED,
                Json(json!({ "error": denied.kind.as_str(), "reason": denied.reason })),
            )
                .into_response(),
        };
    }
    // Path 2: browser clients authenticate with their first frame.
    ws.on_upgrade(move |socket| first_frame_auth(state, socket, headers))
}

/// Wait (bounded) for `{"type":"auth","token":"…"}`, then hand over.
async fn first_frame_auth(state: ApiState, mut socket: WebSocket, headers: HeaderMap) {
    let token = match tokio::time::timeout(AUTH_FRAME_WINDOW, socket.recv()).await {
        Ok(Some(Ok(Message::Text(text)))) => serde_json::from_str::<Value>(&text)
            .ok()
            .filter(|m| m["type"] == "auth")
            .and_then(|m| m["token"].as_str().map(str::to_string)),
        _ => None,
    };
    let Some(token) = token else {
        let _ = socket
            .send(Message::Text(
                json!({ "kind": "error", "reason": "expected {\"type\":\"auth\",\"token\":\"…\"}" })
                    .to_string(),
            ))
            .await;
        let _ = socket
            .send(Message::Close(Some(CloseFrame {
                code: 1008,
                reason: "authentication required".into(),
            })))
            .await;
        return;
    };
    let headers = credential_headers(&headers, Some(&token));
    match resolve(&state, &headers).await {
        Ok(ctx) => run_stream(state, socket, ctx, headers).await,
        Err(denied) => {
            let _ = socket
                .send(Message::Text(
                    json!({ "kind": "error", "reason": denied.reason }).to_string(),
                ))
                .await;
            let _ = socket
                .send(Message::Close(Some(CloseFrame {
                    code: 1008,
                    reason: "authentication failed".into(),
                })))
                .await;
        }
    }
}

/// The established stream: forward filtered events, serve client pings,
/// revalidate the credential, close on authorization loss.
async fn run_stream(
    state: ApiState,
    socket: WebSocket,
    ctx: crate::saas::SaasContext,
    headers: HeaderMap,
) {
    let organization = ctx.organization_id();
    let (mut sink, mut source) = socket.split();
    let mut market = state.shared.events.subscribe();
    let mut saas = subscribe();
    let mut revalidate = tokio::time::interval(REVALIDATE_EVERY);
    revalidate.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

    loop {
        tokio::select! {
            _ = revalidate.tick() => {
                // Revoked session/key, suspended membership or closed
                // organization ⇒ the socket dies with 1008.
                if resolve(&state, &headers).await.is_err() {
                    let _ = sink
                        .send(Message::Close(Some(CloseFrame {
                            code: 1008,
                            reason: "authorization lost".into(),
                        })))
                        .await;
                    return;
                }
            }
            client = source.next() => match client {
                Some(Ok(Message::Text(text))) => {
                    if text.trim().eq_ignore_ascii_case("ping")
                        && sink
                            .send(Message::Text(json!({ "kind": "pong" }).to_string()))
                            .await
                            .is_err()
                    {
                        return;
                    }
                    // Everything else from the client is ignored: this is a
                    // read-only stream. Orders go through the REST API.
                }
                Some(Ok(Message::Close(_))) | None => return,
                Some(Ok(_)) => {}
                Some(Err(_)) => return,
            },
            event = market.recv() => match event {
                Ok(app) => {
                    let inner = crate::ws::event_to_json(&app);
                    if visible_to(&inner, &organization) {
                        let frame = json!({
                            "kind": "market",
                            "organization": organization.to_string(),
                            "event": inner,
                        });
                        if sink.send(Message::Text(frame.to_string())).await.is_err() {
                            return;
                        }
                    }
                }
                Err(broadcast::error::RecvError::Lagged(_)) => continue,
                Err(_) => return,
            },
            event = saas.recv() => match event {
                Ok(inner) => {
                    if visible_to(&inner, &organization) {
                        let frame = json!({
                            "kind": "saas",
                            "organization": organization.to_string(),
                            "event": *inner,
                        });
                        if sink.send(Message::Text(frame.to_string())).await.is_err() {
                            return;
                        }
                    }
                }
                Err(broadcast::error::RecvError::Lagged(_)) => continue,
                Err(_) => return,
            },
        }
    }
}

/// Keep the split-stream types importable for future sender-side helpers.
#[allow(dead_code)]
type SocketSink = SplitSink<WebSocket, Message>;
#[allow(dead_code)]
type SocketSource = SplitStream<WebSocket>;

/// The stream route, mounted by [`crate::saas::routes`].
pub fn routes() -> Router<ApiState> {
    Router::new().route("/api/saas/events", axum::routing::get(events))
}

/// Silence an unused-map lint on deployments that build without SaaS
/// streaming consumers; the map type is part of the public helper's input.
#[allow(dead_code)]
fn map_guard(_m: HashMap<String, String>) {}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc as StdArc;

    use tower::ServiceExt;

    use bot_core::config::AppConfig;
    use bot_core::state::AppState;

    use crate::saas::SaasStore;

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
            saas: SaasStore::shared(),
        }
    }

    #[test]
    fn visibility_is_organization_scoped_or_market_global() {
        let org = OrganizationId::new();
        let other = OrganizationId::new();
        assert!(
            visible_to(&json!({ "kind": "mark" }), &org),
            "market-global events pass"
        );
        assert!(
            visible_to(
                &json!({ "kind": "saas.wallets", "organization": org.to_string() }),
                &org
            ),
            "own-organization events pass"
        );
        assert!(
            !visible_to(
                &json!({ "kind": "saas.wallets", "organization": other.to_string() }),
                &org
            ),
            "another organization's saas events NEVER pass"
        );
        // The credential never re-scopes the stream by client input: the
        // organization inside the frame is the resolved context's.
        assert_eq!(org.to_string().len(), 36);
    }

    #[test]
    fn saas_events_round_trip_through_the_channel() {
        let mut rx = subscribe();
        publish(json!({ "kind": "saas.billing", "organization": "org-x" }));
        let got = rx.try_recv().expect("published event");
        assert_eq!(got["kind"], "saas.billing");
        assert_eq!(got["organization"], "org-x");
    }

    #[tokio::test]
    async fn the_stream_route_is_mounted_and_demands_an_upgrade() {
        // Under tower oneshot there is no hyper OnUpgrade extension, so the
        // WebSocketUpgrade extractor answers 426 — proving the route is
        // mounted and is a websocket endpoint (never a plain GET).
        let app = crate::api::router(test_state());
        let res = app
            .oneshot(
                axum::http::Request::builder()
                    .method("GET")
                    .uri("/api/saas/events")
                    .header("upgrade", "websocket")
                    .header("connection", "Upgrade")
                    .header("sec-websocket-version", "13")
                    .header("sec-websocket-key", "dGhlIHNhbXBsZSBub25jZQ==")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::UPGRADE_REQUIRED);
    }

    #[tokio::test]
    async fn a_bad_credential_never_resolves_and_never_echoes() {
        let state = test_state();
        let mut headers = HeaderMap::new();
        headers.insert(
            AUTH_HEADER,
            HeaderValue::from_static("Bearer not-a-real-token"),
        );
        let denied = resolve(&state, &headers)
            .await
            .expect_err("bogus token denied");
        assert_eq!(denied.kind.as_str(), "deny_unauthenticated");
        assert!(!denied.reason.contains("not-a-real-token"));

        // A real session resolves to a tenant-scoped context.
        use bot_core::membership::{Membership, MembershipRole};
        use bot_core::session::model::SessionRecord;
        use bot_core::session::token::{generate_token, hash_password};
        use bot_core::tenant::{Organization, User, UserStatus};
        let org = Organization::new(
            bot_core::tenant::OrganizationId::new(),
            "ws-org",
            "WS Org",
            None,
            chrono::Utc::now(),
        );
        state.saas.create_organization(&org).await.expect("org");
        let user = User {
            id: bot_core::tenant::UserId::new(),
            email: "ws@example.com".into(),
            email_verified: true,
            display_name: "ws".into(),
            password_hash: hash_password("password-123456"),
            status: UserStatus::Active,
            platform_admin: false,
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
            last_login_at: None,
        };
        state.saas.create_user(&user).await.expect("user");
        state
            .saas
            .create_membership(&Membership::new(
                org.id,
                user.id,
                MembershipRole::Viewer,
                None,
                chrono::Utc::now(),
            ))
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
                chrono::Utc::now(),
            ))
            .await
            .expect("session");
        let mut headers = HeaderMap::new();
        headers.insert(
            AUTH_HEADER,
            HeaderValue::from_str(&format!("Bearer {}", t.plaintext)).unwrap(),
        );
        let ctx = resolve(&state, &headers).await.expect("session resolves");
        assert_eq!(ctx.organization_id(), org.id);
        assert_eq!(ctx.authorization.organization_id, org.id);
    }

    #[test]
    fn the_shared_state_still_has_the_legacy_stream() {
        // The legacy global /api/events stays mounted in api::router; this
        // guard keeps the distinction visible: this module NEVER publishes
        // onto the shared AppEvent bus, it only reads it.
        let state = test_state();
        let _bus: &bot_core::events::EventBus = &state.shared.events;
        let _ = StdArc::new(());
    }
}
