//! Axum REST + WebSocket control plane.
//!
//! Authorization model (BUILD PLAN §4-xi):
//!   * With `[[auth.keys]]` (or a legacy `[api]` key) configured, an
//!     [`Authenticator`] maps each presented key to a [`Principal`] with a
//!     role: `owner` ⊃ `operator` ⊃ `readonly`. Mutating routes require
//!     `operator`; key administration and audit verification require
//!     `owner`; reads require `readonly` (any authenticated principal).
//!   * With no key configured at all, the API stays open (loopback dev
//!     mode) — `main` refuses to bind a reachable interface in that case.
//!   * Every authenticated decision (granted or denied) on a mutating
//!     route is written to the [`AuditTrail`].
//!   * A token-bucket [`RateLimiter`] guards every `/api` route: one bucket
//!     per client IP plus one per principal.
//!
//! The `/api/events` websocket streams every [`AppEvent`] as JSON.

use std::net::SocketAddr;
use std::sync::Arc;

pub mod openapi_billing;
pub mod openapi_commercial;
pub mod openapi_custody;
pub mod openapi_ops;
pub mod openapi_product;
/// The `/api/ops/**` operator surface (platform-admin only).
pub mod ops_routes;

use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        ConnectInfo, Path, Query, State,
    },
    http::{header, HeaderMap, StatusCode},
    middleware::{from_fn, from_fn_with_state, Next},
    response::{Html, IntoResponse, Response},
    routing::{delete, get, post},
    Json, Router,
};
use futures::{SinkExt, StreamExt};
use serde::Deserialize;
use serde_json::{json, Value};
use tokio::sync::broadcast;
use tracing::debug;

use bot_core::audit::{AuditOutcome, AuditTrail};
use bot_core::auth::{sha256_hex, Authenticator, Principal, RateLimiter, RateVerdict, Role};
use bot_core::db::{repo::ReconRepo, Database};
use bot_core::models::{BotModule, ExecutionMode};
use bot_core::obs::health::HealthRegistry;
use bot_core::state::Shared;

use crate::dashboard::{DASHBOARD_HTML, DASHBOARD_JS};
use crate::obs;
use crate::recon::{ReservationSweepService, SweepDecision};
use crate::ws::event_to_json;

/// Server-side shared state handed to every handler.
#[derive(Clone)]
pub struct ApiState {
    /// The bot's shared state.
    pub shared: Shared,
    /// Legacy single key (kept for compatibility + the no-authenticator
    /// path). `None` = open dev mode.
    pub api_key: Option<String>,
    /// Role-aware key registry. `None` when no keys are configured.
    pub auth: Option<Arc<Authenticator>>,
    /// Per-principal / per-IP token buckets (`rpm = 0` disables).
    pub limiter: Arc<RateLimiter>,
    /// Explicit proxy CIDRs shared by rate limiting and tenant allowlists.
    pub trusted_proxies: crate::saas::trusted_proxy::TrustedProxies,
    /// Lower-volume, per-identity buckets for login, MFA, and enrollment
    /// attempts. Production initializes this to ten requests per minute.
    pub sensitive_limiter: Arc<RateLimiter>,
    /// Immutable audit trail (DB-chained when the database is attached).
    pub audit: Arc<AuditTrail>,
    /// Durable store for orders/keys/recovery reads (`None` = memory-only).
    pub db: Option<Arc<Database>>,
    /// JSONL journal handle (sizes + rotation; `None` when the data
    /// directory could not be opened).
    pub journal: Option<bot_core::storage::Store>,
    /// Chain-side existence reader for the ambiguous-reservation sweep
    /// (`landed` / `absent` / `unknown`). `None` = no RPC configured, in
    /// which case an `abandon` is refused rather than made blind.
    pub chain_probe: Option<Arc<crate::reservation_chain::ChainExistenceProbe>>,
    /// Whether to serve the dashboard at `/`.
    pub serve_dashboard: bool,
    /// Liveness/readiness component registry (sampled in the background).
    pub health: Arc<HealthRegistry>,
    /// When false, `/metrics` is 404 and HTTP request metrics are not recorded.
    pub metrics_enabled: bool,
    /// TASK 7A — the SaaS control plane (tenants, users, memberships,
    /// sessions, tenant API keys, plans, entitlements, usage, provisioning).
    /// Always present; it holds no trading truth.
    pub saas: Arc<crate::saas::SaasStore>,
    /// PROMPT 3/10 — the tenant trading data plane (orders,
    /// executions, positions, copy, polymarket, recovery, reporting),
    /// built over the attached database. `None` when no database is
    /// attached: every `/api/tenant/...` trading route then answers
    /// 503 (`trading_data_plane_unavailable`).
    pub trading: Option<Arc<crate::trading_data_plane::TenantTradingDataPlane>>,
    /// §H — the live tenant module registry (§A): which bot runtimes
    /// exist per (organization, module) and their lifecycle phase. The
    /// customer bots surface reads THIS registry; an empty registry is
    /// the honest "no bot runtimes registered in this deployment"
    /// answer, never a synthesized list.
    pub module_registry: Arc<crate::module_runtime::module_registry::TenantModuleRegistry>,
}

impl ApiState {
    /// The authoritative tenant module-control store (§S-2, migration
    /// 0036's `tenant_module_controls`).
    ///
    /// Built on demand rather than held as a field: the store owns no
    /// connection of its own (it is an `Option<Arc<Database>>` plus a
    /// process-wide read-through cache), so construction is free and
    /// every call site is guaranteed to see the state's CURRENT database
    /// attachment rather than one captured at startup.
    pub fn module_controls(
        &self,
    ) -> crate::trading_data_plane::module_control_store::ModuleControlStore {
        crate::trading_data_plane::module_control_store::ModuleControlStore::new(self.db.clone())
    }

    /// The authoritative custody rotation store (§S-4, migration 0036's
    /// `custody_rotations`). Same construction rationale as
    /// [`ApiState::module_controls`].
    pub fn custody_rotations(&self) -> crate::saas::custody_rotation_store::CustodyRotationStore {
        crate::saas::custody_rotation_store::CustodyRotationStore::new(self.db.clone())
    }

    /// The shared WebSocket replay-protection store (migration 0036's
    /// `ws_replay_tokens`). Same construction rationale as
    /// [`ApiState::module_controls`].
    pub fn websocket_replay(&self) -> crate::saas::websocket_replay_store::WebsocketReplayStore {
        crate::saas::websocket_replay_store::WebsocketReplayStore::new(self.db.clone())
    }
}

/// Build the full router.
pub fn router(state: ApiState) -> Router {
    Router::new()
        .route("/", get(root))
        .route("/dashboard", get(root))
        .route("/dashboard/app.js", get(dashboard_script))
        .route("/health", get(obs::health_live))
        .route("/ready", get(obs::ready))
        .route("/metrics", get(obs::metrics_endpoint))
        .route("/api/health", get(health))
        .route("/api/status", get(status))
        .route("/api/modules", get(modules))
        .route("/api/positions", get(positions))
        .route("/api/trades", get(trades))
        .route("/api/config", get(config_view))
        .route("/api/orders", get(orders))
        .route("/api/orders/:id", get(order_by_id))
        .route("/api/executions", get(executions))
        .route("/api/executions/:id", get(execution_by_id))
        .route("/api/audit", get(audit_list))
        .route("/api/audit/verify", get(audit_verify))
        .route("/api/keys", get(keys_list).post(keys_add))
        .route("/api/keys/:hash", delete(keys_revoke))
        .route("/api/recovery/failed", get(recovery_failed))
        .route("/api/db", get(db_status))
        .route("/api/wallets", get(wallets))
        .route("/api/journal", get(journal_status).post(journal_rotate))
        .route("/api/kill", post(kill))
        .route("/api/resume", post(resume))
        .route("/api/mode", post(set_mode))
        .route("/api/modules/:name/enable", post(enable_module))
        .route("/api/modules/:name/disable", post(disable_module))
        .route("/api/accounting/portfolio", get(accounting_portfolio))
        .route(
            "/api/accounting/events",
            get(accounting_events).post(accounting_event_post),
        )
        .route("/api/accounting/findings", get(accounting_findings))
        .route("/api/accounting/conflict", get(accounting_conflict))
        .route(
            "/api/accounting/reservations/sweep",
            get(accounting_reservation_sweep),
        )
        .route(
            "/api/accounting/reservations/sweep",
            post(accounting_reservation_sweep_resolve),
        )
        .route(
            "/api/accounting/reservations/chain-check",
            get(accounting_reservation_chain_check),
        )
        .route(
            "/api/accounting/reservations/signature-scan",
            get(accounting_reservation_signature_scan),
        )
        .route(
            "/api/accounting/conflict/repair",
            post(accounting_conflict_repair),
        )
        .route("/api/reconciliation/findings", get(accounting_findings))
        .route("/api/risk/global", get(risk_global))
        .route("/api/risk/kill-switch", post(risk_kill_switch))
        .route("/api/ha", get(ha_status))
        // TASK 7A — the multi-tenant control plane. Mounted here so it
        // shares the rate limiter, the audit trail and the request-id
        // middleware with the existing API.
        .merge(crate::saas::routes())
        // PROMPT 3/10 — the tenant trading data plane (503 when the
        // database is not attached).
        .merge(crate::trading_data_plane::routes())
        // P1 — the operator surface (`/api/ops/**`). Platform-admin
        // only; it describes the deployment, not a tenant.
        .merge(crate::api::ops_routes::routes())
        .route("/api/events", get(events_ws))
        // route_layer (not layer): runs after routing, so handlers see the
        // MatchedPath and the middleware can label metrics with the bounded
        // route pattern instead of the raw path.
        .route_layer(from_fn_with_state(state.clone(), obs::request_context))
        .route_layer(from_fn_with_state(state.clone(), ip_rate_limit))
        // TASK 7B — security response headers on EVERY response (CSP,
        // XCTO, frame/referrer/permissions policy, HSTS over TLS). Appends
        // headers only; it never rewrites bodies or blocks the WS upgrade.
        .route_layer(from_fn(crate::security::headers::apply_security_headers))
        .route_layer(from_fn(crate::saas::scope_request_peer))
        .route_layer(from_fn(tenant_rls_request_scope))
        .with_state(state)
}

async fn tenant_rls_request_scope(
    request: axum::http::Request<axum::body::Body>,
    next: Next,
) -> Response {
    bot_core::tenant::rls_context::scope_request_context(
        crate::saas::custody::scope_custody_context(next.run(request)),
    )
    .await
}

/// Per-IP token bucket over every matched route (DoS guard). The
/// per-principal bucket in [`require_role`] stacks on top of this.
async fn ip_rate_limit(
    State(state): State<ApiState>,
    peer: Option<ConnectInfo<SocketAddr>>,
    req: axum::http::Request<axum::body::Body>,
    next: Next,
) -> Response {
    if state.limiter.rpm() > 0 {
        // Without connect info (unit tests via oneshot) limit under one
        // shared bucket rather than skipping the limiter entirely.
        let key = crate::saas::trusted_proxy::resolve_client_ip(
            peer.map(|p| p.0.ip()),
            req.headers(),
            &state.trusted_proxies,
        )
        .map(|ip| format!("ip:{ip}"))
        .unwrap_or_else(|| "ip:unknown".to_string());
        if let RateVerdict::Limited { retry_after_secs } = state.limiter.check(&key).await {
            return (
                StatusCode::TOO_MANY_REQUESTS,
                [(header::RETRY_AFTER, retry_after_secs.to_string())],
                "rate limit exceeded",
            )
                .into_response();
        }
    }
    next.run(req).await
}

/// Compare fixed-length digests, never raw secret strings. Shared by the
/// HTTP owner-key branch and the legacy WebSocket authentication branch.
fn legacy_key_matches(presented: &str, expected: &str) -> bool {
    bot_core::session::token::constant_time_eq(
        sha256_hex(presented).as_bytes(),
        sha256_hex(expected).as_bytes(),
    )
}

/// Extract a presented API key: `x-api-key` header first, then
/// `Authorization: Bearer …`.
fn extract_key(headers: &HeaderMap) -> Option<String> {
    if let Some(v) = headers.get("x-api-key").and_then(|v| v.to_str().ok()) {
        if !v.trim().is_empty() {
            return Some(v.trim().to_string());
        }
    }
    if let Some(v) = headers.get("authorization").and_then(|v| v.to_str().ok()) {
        if let Some(rest) = v
            .strip_prefix("Bearer ")
            .or_else(|| v.strip_prefix("bearer "))
        {
            if !rest.trim().is_empty() {
                return Some(rest.trim().to_string());
            }
        }
    }
    None
}

/// Authenticate + authorize + per-principal rate limit.
///
/// `Ok(principal)` only when the caller may perform an action needing
/// `required`. Every denial is audited; grants are audited by the handlers
/// themselves (they know the action semantics).
// axum's `Response` is intrinsically ~128 bytes; every handler returns it
// directly on denial, so boxing the Err would only add indirection.
#[allow(clippy::result_large_err)]
async fn require_role(
    state: &ApiState,
    headers: &HeaderMap,
    required: Role,
    action: &str,
) -> Result<Principal, Response> {
    let presented = extract_key(headers);

    // Per-principal bucket (keyed by hash — never the plaintext).
    if state.limiter.rpm() > 0 {
        let bucket = presented
            .as_deref()
            .map(sha256_hex)
            .unwrap_or_else(|| "principal:anonymous".to_string());
        if let RateVerdict::Limited { retry_after_secs } = state.limiter.check(&bucket).await {
            return Err((
                StatusCode::TOO_MANY_REQUESTS,
                [(header::RETRY_AFTER, retry_after_secs.to_string())],
                "rate limit exceeded",
            )
                .into_response());
        }
    }

    let principal = if let Some(auth) = &state.auth {
        match presented {
            Some(key) => match auth.authenticate(&key).await {
                Some(p) => p,
                None => {
                    state
                        .audit
                        .denied(&actor_of(None), action, None, "invalid api key")
                        .await;
                    return Err(
                        (StatusCode::UNAUTHORIZED, "invalid or missing x-api-key").into_response()
                    );
                }
            },
            None => {
                state
                    .audit
                    .denied("anonymous", action, None, "missing api key")
                    .await;
                return Err(
                    (StatusCode::UNAUTHORIZED, "invalid or missing x-api-key").into_response()
                );
            }
        }
    } else if let Some(expected) = &state.api_key {
        // Legacy single-key deployments: the key is an owner key.
        match presented {
            Some(key) if legacy_key_matches(&key, expected) => Principal {
                label: "legacy-api-key".into(),
                role: Role::Owner,
                key_hash: sha256_hex(expected),
            },
            _ => {
                state
                    .audit
                    .denied("anonymous", action, None, "invalid api key")
                    .await;
                return Err(
                    (StatusCode::UNAUTHORIZED, "invalid or missing x-api-key").into_response()
                );
            }
        }
    } else {
        // No key configured anywhere: loopback dev mode (main refuses to
        // bind a reachable interface in this state).
        Principal {
            label: "anonymous".into(),
            role: Role::Owner,
            key_hash: String::new(),
        }
    };

    if !Authenticator::authorize(&principal, required) {
        state
            .audit
            .denied(
                &actor_of(Some(&principal)),
                action,
                None,
                &format!(
                    "role {} insufficient (need {})",
                    principal.role.as_str(),
                    required.as_str()
                ),
            )
            .await;
        return Err((
            StatusCode::FORBIDDEN,
            format!(
                "role '{}' cannot perform '{action}'",
                principal.role.as_str()
            ),
        )
            .into_response());
    }

    Ok(principal)
}

/// Audit actor label for a principal.
fn actor_of(principal: Option<&Principal>) -> String {
    match principal {
        Some(p) => format!("api:{}:{}", p.role.as_str(), p.label),
        None => "anonymous".into(),
    }
}

async fn root(State(state): State<ApiState>) -> Response {
    if state.serve_dashboard {
        Html(DASHBOARD_HTML).into_response()
    } else {
        (StatusCode::NOT_FOUND, "dashboard disabled").into_response()
    }
}

async fn dashboard_script(State(state): State<ApiState>) -> Response {
    if state.serve_dashboard {
        (
            [(
                header::CONTENT_TYPE,
                "application/javascript; charset=utf-8",
            )],
            DASHBOARD_JS,
        )
            .into_response()
    } else {
        (StatusCode::NOT_FOUND, "dashboard disabled").into_response()
    }
}

async fn health() -> Response {
    Json(json!({ "ok": true })).into_response()
}

async fn status(State(state): State<ApiState>, headers: HeaderMap) -> Response {
    if let Err(e) = require_role(&state, &headers, Role::Readonly, "status_read").await {
        return e;
    }
    Json(state.shared.summary().await).into_response()
}

async fn modules(State(state): State<ApiState>, headers: HeaderMap) -> Response {
    if let Err(e) = require_role(&state, &headers, Role::Readonly, "modules_read").await {
        return e;
    }
    Json(state.shared.all_module_status().await).into_response()
}

async fn positions(State(state): State<ApiState>, headers: HeaderMap) -> Response {
    if let Err(e) = require_role(&state, &headers, Role::Readonly, "positions_read").await {
        return e;
    }
    Json(state.shared.open_positions().await).into_response()
}

#[derive(Deserialize)]
struct TradeQuery {
    limit: Option<usize>,
}

async fn trades(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Query(q): Query<TradeQuery>,
) -> Response {
    if let Err(e) = require_role(&state, &headers, Role::Readonly, "trades_read").await {
        return e;
    }
    let limit = q.limit.unwrap_or(50).clamp(1, 500);
    Json(state.shared.trades(limit).await).into_response()
}

/// Recursive redaction helper to strip sensitive fields from any JSON level.
fn redact_secrets_recursive(v: &mut serde_json::Value) {
    match v {
        serde_json::Value::Object(map) => {
            const SENSITIVE_KEYS: &[&str] = &[
                "secret",
                "key",
                "password",
                "token",
                "credential",
                "private",
                "api_key",
                "webhook_secret",
                "access_key",
                "secret_key",
                "signing_key",
            ];
            for (k, val) in map.iter_mut() {
                let lower = k.to_ascii_lowercase();
                if SENSITIVE_KEYS.iter().any(|s| lower.contains(s)) {
                    *val = serde_json::json!("<redacted>");
                } else {
                    redact_secrets_recursive(val);
                }
            }
        }
        serde_json::Value::Array(arr) => {
            for item in arr.iter_mut() {
                redact_secrets_recursive(item);
            }
        }
        _ => {}
    }
}

/// A secrets-redacted view of the running config.
async fn config_view(State(state): State<ApiState>, headers: HeaderMap) -> Response {
    if let Err(e) = require_role(&state, &headers, Role::Readonly, "config_read").await {
        return e;
    }
    let cfg = state.shared.config_snapshot().await;
    let mut v = serde_json::to_value(&cfg).unwrap_or_else(|_| json!({}));
    redact_secrets_recursive(&mut v);
    Json(v).into_response()
}

async fn kill(State(state): State<ApiState>, headers: HeaderMap) -> Response {
    let principal = match require_role(&state, &headers, Role::Operator, "kill_switch").await {
        Ok(p) => p,
        Err(e) => return e,
    };
    state.shared.emergency_stop("api /kill").await;
    state
        .audit
        .success(&actor_of(Some(&principal)), "kill_switch", Some("system"))
        .await;
    Json(json!({ "ok": true, "kill_switch": true })).into_response()
}

async fn resume(State(state): State<ApiState>, headers: HeaderMap) -> Response {
    let principal = match require_role(&state, &headers, Role::Operator, "resume").await {
        Ok(p) => p,
        Err(e) => return e,
    };
    // `resume` is the exact inverse of `/kill`: it releases the kill switch
    // AND the emergency halt latch (propagating both to the other replicas)
    // and then reports the modules an emergency stop disabled, which a resume
    // deliberately does not re-enable on the operator's behalf. The old handler
    // cleared only the kill switch, so `/resume` answered `ok` on a deployment
    // that still refused every entry and still had all modules disabled.
    let outcome = state.shared.resume("api /resume", true).await;
    let actor = actor_of(Some(&principal));
    state.audit.success(&actor, "resume", Some("system")).await;
    Json(json!({
        "ok": true,
        "kill_switch": state.shared.kill_switch(),
        "halted_latch_held": state.shared.halted(),
        "halt_reason": state.shared.config_snapshot().await.risk.halt_reason,
        "released": outcome,
    }))
    .into_response()
}

#[derive(Deserialize)]
struct ModeBody {
    mode: String,
}

async fn set_mode(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Json(body): Json<ModeBody>,
) -> Response {
    // Switching TO live is an owner action; everything else operator.
    let required = if body.mode.eq_ignore_ascii_case("live") {
        Role::Owner
    } else {
        Role::Operator
    };
    let principal = match require_role(&state, &headers, required, "set_mode").await {
        Ok(p) => p,
        Err(e) => return e,
    };
    let mode = match body.mode.to_ascii_lowercase().as_str() {
        "paper" => ExecutionMode::Paper,
        "simulate" | "sim" => ExecutionMode::Simulate,
        "live" => ExecutionMode::Live,
        other => {
            return (StatusCode::BAD_REQUEST, format!("unknown mode '{other}'")).into_response();
        }
    };
    state
        .shared
        .update_config(|c| c.execution.mode = mode)
        .await;
    let gate = state.shared.summary().await.live_allowed;
    state
        .audit
        .record(
            &actor_of(Some(&principal)),
            "set_mode",
            Some(mode.as_str()),
            AuditOutcome::Success,
            json!({ "live_allowed": gate }),
        )
        .await;
    Json(json!({
        "ok": true,
        "mode": mode.as_str(),
        "live_allowed": gate,
        "note": if mode == ExecutionMode::Live && !gate {
            "live requested but allow_live_trading is false; orders will simulate"
        } else { "" }
    }))
    .into_response()
}

async fn enable_module(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Path(name): Path<String>,
) -> Response {
    toggle_module(&state, &headers, &name, true).await
}

async fn disable_module(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Path(name): Path<String>,
) -> Response {
    toggle_module(&state, &headers, &name, false).await
}

async fn toggle_module(
    state: &ApiState,
    headers: &HeaderMap,
    name: &str,
    enabled: bool,
) -> Response {
    let action = if enabled {
        "module_enable"
    } else {
        "module_disable"
    };
    let principal = match require_role(state, headers, Role::Operator, action).await {
        Ok(p) => p,
        Err(e) => return e,
    };
    let module = match name.parse::<BotModule>() {
        Ok(m) => m,
        Err(_) => {
            return (StatusCode::BAD_REQUEST, format!("unknown module '{name}'")).into_response()
        }
    };
    state.shared.set_enabled(module, enabled).await;
    state
        .audit
        .success(&actor_of(Some(&principal)), action, Some(module.as_str()))
        .await;
    Json(json!({ "ok": true, "module": module.as_str(), "enabled": enabled })).into_response()
}

// ---------------------------------------------------------------------------
// Orders (OMS read API)
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct OrdersQuery {
    limit: Option<usize>,
    /// Optional status filter (`filled`, `submitted`, …).
    status: Option<String>,
}

async fn orders(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Query(q): Query<OrdersQuery>,
) -> Response {
    if let Err(e) = require_role(&state, &headers, Role::Readonly, "orders_read").await {
        return e;
    }
    let limit = q.limit.unwrap_or(100).clamp(1, 500);
    // Validate the filter before anything else so bad input is a 400 even
    // when the manager is not attached.
    let status_filter = match q.status.as_deref() {
        Some(filter) => match bot_core::oms::OrderStatus::parse(filter) {
            Some(st) => Some(st),
            None => {
                return (
                    StatusCode::BAD_REQUEST,
                    format!("unknown order status '{filter}'"),
                )
                    .into_response()
            }
        },
        None => None,
    };
    let Some(mgr) = state.shared.orders() else {
        return Json(json!({ "orders": [], "note": "order manager not attached" })).into_response();
    };
    let mut list = mgr.list(limit).await;
    if let Some(st) = status_filter {
        list.retain(|o| o.status == st);
    }
    Json(json!({ "count": list.len(), "orders": list })).into_response()
}

async fn order_by_id(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    if let Err(e) = require_role(&state, &headers, Role::Readonly, "order_read").await {
        return e;
    }
    let Some(mgr) = state.shared.orders() else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            "order manager not attached",
        )
            .into_response();
    };
    match mgr.get(&id).await {
        Some(order) => Json(order).into_response(),
        // Fall back to the durable log for evicted (old terminal) orders.
        None => {
            if let Some(db) = &state.db {
                if let Ok(Some(order)) = bot_core::db::repo::OrderRepo::new(db.clone())
                    .get(&id)
                    .await
                {
                    return Json(order).into_response();
                }
            }
            (StatusCode::NOT_FOUND, format!("order '{id}' not found")).into_response()
        }
    }
}

// ---------------------------------------------------------------------------
// Execution lifecycle ledger
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct ExecutionsQuery {
    limit: Option<usize>,
    /// Optional state filter (`pending`, `confirmed`, `failed`, …).
    state: Option<String>,
    /// `true` = only attempts that are still live (created … pending).
    open: Option<bool>,
}

/// The execution lifecycle ledger: every transaction intent with its current
/// state, attempt count, signature, fee and failure class. In-memory mirror
/// (bounded); older settled intents live in `execution_lifecycle`.
async fn executions(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Query(q): Query<ExecutionsQuery>,
) -> Response {
    if let Err(e) = require_role(&state, &headers, Role::Readonly, "executions_read").await {
        return e;
    }
    let limit = q.limit.unwrap_or(100).clamp(1, 500);
    let state_filter = match q.state.as_deref() {
        Some(filter) => match bot_core::execution::ExecutionState::parse(filter) {
            Some(st) => Some(st),
            None => {
                return (
                    StatusCode::BAD_REQUEST,
                    format!("unknown execution state '{filter}'"),
                )
                    .into_response()
            }
        },
        None => None,
    };
    let ledger = bot_core::execution::ledger();
    let mut list = if q.open.unwrap_or(false) {
        ledger.open().await
    } else {
        ledger.list(limit).await
    };
    if let Some(st) = state_filter {
        list.retain(|r| r.state == st);
    }
    list.truncate(limit);
    let counts = ledger.counts().await;
    let counts: serde_json::Map<String, Value> = counts
        .into_iter()
        .map(|(k, v)| (k.as_str().to_string(), json!(v)))
        .collect();
    Json(json!({
        "count": list.len(),
        "states": counts,
        "executions": list,
    }))
    .into_response()
}

async fn execution_by_id(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    if let Err(e) = require_role(&state, &headers, Role::Readonly, "execution_read").await {
        return e;
    }
    let ledger = bot_core::execution::ledger();
    // Accept either an intent id or a transaction signature.
    let record = match ledger.get(&id).await {
        Some(r) => Some(r),
        None => ledger.get_by_signature(&id).await,
    };
    if let Some(record) = record {
        return Json(record).into_response();
    }
    // Fall back to the durable ledger for evicted (old settled) intents.
    if let Some(db) = &state.db {
        let repo = bot_core::db::execution::ExecutionRepo::new(db.clone());
        let found = match repo.get(&id).await {
            Ok(Some(r)) => Some(r),
            _ => repo.get_by_signature(&id).await.ok().flatten(),
        };
        if let Some(record) = found {
            let events = repo
                .events(&record.intent_id, 100)
                .await
                .unwrap_or_default();
            return Json(json!({ "record": record, "events": events })).into_response();
        }
    }
    (StatusCode::NOT_FOUND, format!("execution '{id}' not found")).into_response()
}

// ---------------------------------------------------------------------------
// Audit
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct AuditQuery {
    limit: Option<usize>,
}

async fn audit_list(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Query(q): Query<AuditQuery>,
) -> Response {
    if let Err(e) = require_role(&state, &headers, Role::Readonly, "audit_read").await {
        return e;
    }
    let limit = q.limit.unwrap_or(100).clamp(1, 1000);
    let records = state.audit.recent(limit).await;
    Json(json!({
        "durable": state.audit.durable(),
        "count": records.len(),
        "records": records,
    }))
    .into_response()
}

async fn audit_verify(State(state): State<ApiState>, headers: HeaderMap) -> Response {
    let principal = match require_role(&state, &headers, Role::Operator, "audit_verify").await {
        Ok(p) => p,
        Err(e) => return e,
    };
    let result = state.audit.verify().await;
    state
        .audit
        .record(
            &actor_of(Some(&principal)),
            "audit_verify",
            None,
            AuditOutcome::Success,
            serde_json::to_value(&result).unwrap_or(json!({})),
        )
        .await;
    Json(result).into_response()
}

// ---------------------------------------------------------------------------
// API key administration (owner only)
// ---------------------------------------------------------------------------

async fn keys_list(State(state): State<ApiState>, headers: HeaderMap) -> Response {
    if let Err(e) = require_role(&state, &headers, Role::Owner, "keys_list").await {
        return e;
    }
    let mut runtime: Vec<Value> = match &state.auth {
        Some(auth) => auth
            .list()
            .await
            .into_iter()
            .map(|p| {
                json!({
                    "label": p.label,
                    "role": p.role.as_str(),
                    "hash": p.key_hash,
                    "key_hash": p.key_hash,
                    "source": "runtime",
                })
            })
            .collect(),
        None => Vec::new(),
    };
    if let Some(db) = &state.db {
        if let Ok(rows) = bot_core::db::repo::ApiKeyRepo::new(db.clone()).list().await {
            for r in rows {
                runtime.push(json!({
                    "label": r.label,
                    "role": r.role,
                    "hash": r.key_hash,
                    "key_hash": r.key_hash,
                    "enabled": r.enabled,
                    "created_at": r.created_at.to_rfc3339(),
                    "last_used_at": r.last_used_at.map(|t| t.to_rfc3339()),
                    "source": "database",
                }));
            }
        }
    }
    Json(json!({ "keys": runtime })).into_response()
}

#[derive(Deserialize)]
struct KeyBody {
    label: String,
    /// Plaintext key. Accepted over the API only because the control plane
    /// is loopback/TLS-fronted; it is hashed immediately and never stored,
    /// logged or echoed.
    key: String,
    role: String,
}

async fn keys_add(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Json(body): Json<KeyBody>,
) -> Response {
    let principal = match require_role(&state, &headers, Role::Owner, "key_add").await {
        Ok(p) => p,
        Err(e) => return e,
    };
    let Some(role) = Role::parse(&body.role) else {
        return (
            StatusCode::BAD_REQUEST,
            "role must be owner|operator|readonly",
        )
            .into_response();
    };
    if body.label.trim().is_empty() || body.key.trim().is_empty() {
        return (StatusCode::BAD_REQUEST, "label and key must be non-empty").into_response();
    }
    if body.key.len() < 24 {
        return (
            StatusCode::BAD_REQUEST,
            "key must be at least 24 characters",
        )
            .into_response();
    }
    let Some(auth) = &state.auth else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            "key registry not active (no keys configured at startup)",
        )
            .into_response();
    };
    let hash = auth.add_key(&body.label, &body.key, role).await;
    if let Some(db) = &state.db {
        if let Err(e) = bot_core::db::repo::ApiKeyRepo::new(db.clone())
            .insert(&hash, &body.label, role.as_str())
            .await
        {
            debug!(error = %e, "api key DB persistence failed (runtime key still active)");
        }
    }
    state
        .audit
        .record(
            &actor_of(Some(&principal)),
            "key_add",
            Some(&body.label),
            AuditOutcome::Success,
            json!({ "role": role.as_str(), "key_hash": hash }),
        )
        .await;
    Json(json!({ "ok": true, "label": body.label, "role": role.as_str(), "hash": hash, "key_hash": hash }))
        .into_response()
}

async fn keys_revoke(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Path(hash): Path<String>,
) -> Response {
    let principal = match require_role(&state, &headers, Role::Owner, "key_revoke").await {
        Ok(p) => p,
        Err(e) => return e,
    };
    let Some(auth) = &state.auth else {
        return (StatusCode::SERVICE_UNAVAILABLE, "key registry not active").into_response();
    };
    let removed = auth.revoke_hash(&hash).await;
    if let Some(db) = &state.db {
        if let Err(e) = bot_core::db::repo::ApiKeyRepo::new(db.clone())
            .set_enabled(&hash, false)
            .await
        {
            debug!(error = %e, "api key DB revoke failed");
        }
    }
    state
        .audit
        .record(
            &actor_of(Some(&principal)),
            "key_revoke",
            Some(&hash),
            if removed {
                AuditOutcome::Success
            } else {
                AuditOutcome::Failure
            },
            json!({ "removed": removed }),
        )
        .await;
    if removed {
        Json(json!({ "ok": true, "revoked": hash })).into_response()
    } else {
        (StatusCode::NOT_FOUND, "no such key hash").into_response()
    }
}

// ---------------------------------------------------------------------------
// Recovery / persistence status
// ---------------------------------------------------------------------------

async fn recovery_failed(State(state): State<ApiState>, headers: HeaderMap) -> Response {
    if let Err(e) = require_role(&state, &headers, Role::Operator, "recovery_read").await {
        return e;
    }
    let Some(db) = &state.db else {
        return Json(json!({ "available": false, "items": [] })).into_response();
    };
    match ReconRepo::new(db.clone()).list_failed(200).await {
        Ok(items) => {
            Json(json!({ "available": true, "count": items.len(), "items": items })).into_response()
        }
        Err(e) => (
            StatusCode::SERVICE_UNAVAILABLE,
            format!("recovery queue unavailable: {e}"),
        )
            .into_response(),
    }
}

async fn db_status(State(state): State<ApiState>, headers: HeaderMap) -> Response {
    if let Err(e) = require_role(&state, &headers, Role::Operator, "db_status").await {
        return e;
    }
    let dedup_backend = state
        .shared
        .dedup()
        .map(|d| d.backend().as_str())
        .unwrap_or("memory");
    let Some(db) = &state.db else {
        return Json(json!({
            "database": "disabled",
            "dedup_backend": dedup_backend,
        }))
        .into_response();
    };
    let stats = db.pool_stats().await;
    let migrations = db.migration_count().await.unwrap_or(-1);
    Json(json!({
        "database": "connected",
        "pool": { "size": stats.size, "idle": stats.idle, "active": stats.active, "max": stats.max },
        "migrations_applied": migrations,
        "dedup_backend": dedup_backend,
        "audit_durable": state.audit.durable(),
    }))
    .into_response()
}

async fn wallets(State(state): State<ApiState>, headers: HeaderMap) -> Response {
    if let Err(e) = require_role(&state, &headers, Role::Readonly, "wallets_read").await {
        return e;
    }
    let Some(db) = &state.db else {
        return Json(json!({ "available": false, "wallets": [] })).into_response();
    };
    match bot_core::db::repo::WalletRepo::new(db.clone()).list().await {
        Ok(rows) => Json(json!({ "available": true, "wallets": rows })).into_response(),
        Err(e) => (
            StatusCode::SERVICE_UNAVAILABLE,
            format!("wallet registry unavailable: {e}"),
        )
            .into_response(),
    }
}

// ---------------------------------------------------------------------------
// JSONL journal
// ---------------------------------------------------------------------------

async fn journal_status(State(state): State<ApiState>, headers: HeaderMap) -> Response {
    if let Err(e) = require_role(&state, &headers, Role::Readonly, "journal_read").await {
        return e;
    }
    let Some(store) = &state.journal else {
        return Json(json!({ "available": false })).into_response();
    };
    Json(json!({
        "available": true,
        "dir": store.dir().display().to_string(),
        "trades_bytes": store.size_of(store.trades_path()).await,
        "positions_bytes": store.size_of(store.positions_path()).await,
        "events_bytes": store.size_of(store.events_path()).await,
    }))
    .into_response()
}

/// Archive + restart all three journals (owner action; audited).
async fn journal_rotate(State(state): State<ApiState>, headers: HeaderMap) -> Response {
    use bot_core::storage::JournalKind;
    let principal = match require_role(&state, &headers, Role::Owner, "journal_rotate").await {
        Ok(p) => p,
        Err(e) => return e,
    };
    let Some(store) = &state.journal else {
        return (StatusCode::SERVICE_UNAVAILABLE, "journal not available").into_response();
    };
    let mut rotated = Vec::new();
    let mut errors = Vec::new();
    for kind in [
        JournalKind::Trades,
        JournalKind::Positions,
        JournalKind::Events,
    ] {
        match store.rotate(kind).await {
            Ok(Some(archive)) => rotated.push(archive.display().to_string()),
            Ok(None) => {}
            Err(e) => errors.push(format!("{kind:?}: {e}")),
        }
    }
    state
        .audit
        .record(
            &actor_of(Some(&principal)),
            "journal_rotate",
            None,
            if errors.is_empty() {
                AuditOutcome::Success
            } else {
                AuditOutcome::Failure
            },
            json!({ "rotated": rotated, "errors": errors }),
        )
        .await;
    Json(json!({ "ok": errors.is_empty(), "rotated": rotated, "errors": errors })).into_response()
}

// ---------------------------------------------------------------------------
// Events websocket
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct EventsQuery {
    key: Option<String>,
}

/// Upgrade to the live event websocket.
///
/// When keys are configured the stream is authenticated (any role may
/// read): browsers cannot set headers on a WebSocket, so the key is
/// accepted either via the `x-api-key` header (non-browser clients) or the
/// `?key=` query param (the dashboard). With no key configured (loopback
/// dev) the stream is open.
async fn events_ws(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Query(q): Query<EventsQuery>,
    ws: WebSocketUpgrade,
) -> Response {
    let presented = q.key.clone().or_else(|| extract_key(&headers));
    if state.auth.is_some() || state.api_key.is_some() {
        let Some(key) = presented else {
            return (StatusCode::UNAUTHORIZED, "invalid or missing key").into_response();
        };
        let ok = if let Some(auth) = &state.auth {
            auth.authenticate(&key).await.is_some()
        } else {
            state
                .api_key
                .as_deref()
                .is_some_and(|expected| legacy_key_matches(&key, expected))
        };
        if !ok {
            return (StatusCode::UNAUTHORIZED, "invalid or missing key").into_response();
        }
    }
    ws.on_upgrade(move |socket| handle_socket(socket, state.shared))
}

async fn handle_socket(socket: WebSocket, shared: Shared) {
    let mut rx = shared.events.subscribe();
    let (mut sender, mut receiver) = socket.split();

    // Send a hello frame with the current snapshot so the UI is populated fast.
    let hello = json!({ "kind": "hello", "summary": shared.summary().await });
    if sender.send(Message::Text(hello.to_string())).await.is_err() {
        return;
    }

    // Drain client frames (we ignore them, but must read to detect close).
    let mut send_task = tokio::spawn(async move {
        loop {
            match rx.recv().await {
                Ok(event) => {
                    let text = event_to_json(&event).to_string();
                    if sender.send(Message::Text(text)).await.is_err() {
                        break;
                    }
                }
                Err(broadcast::error::RecvError::Lagged(n)) => {
                    debug!(skipped = n, "ws event subscriber lagged");
                }
                Err(broadcast::error::RecvError::Closed) => break,
            }
        }
    });

    let mut recv_task = tokio::spawn(async move {
        while let Some(Ok(msg)) = receiver.next().await {
            if matches!(msg, Message::Close(_)) {
                break;
            }
        }
    });

    tokio::select! {
        _ = &mut send_task => recv_task.abort(),
        _ = &mut recv_task => send_task.abort(),
    }
}

// ---------------------------------------------------------------------------
// TASK 5 — global risk / accounting
// ---------------------------------------------------------------------------

/// `GET /api/accounting/portfolio` — the aggregated portfolio view
/// (exposure / PnL / fees / utilization per venue, wallet, strategy, asset,
/// module; native per quote asset; missing reference rates).
async fn accounting_portfolio(State(state): State<ApiState>, headers: HeaderMap) -> Response {
    if let Err(e) = require_role(&state, &headers, Role::Readonly, "portfolio_read").await {
        return e;
    }
    let marks = state.shared.marks().await;
    let view = state.shared.global_risk().portfolio(marks).await;
    Json(view).into_response()
}

#[derive(Deserialize)]
struct LimitQuery {
    limit: Option<usize>,
}

/// `GET /api/accounting/events?limit=` — recent ledger events (newest
/// first), counts by kind and the ids not yet durably journaled.
async fn accounting_events(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Query(q): Query<LimitQuery>,
) -> Response {
    if let Err(e) = require_role(&state, &headers, Role::Readonly, "ledger_read").await {
        return e;
    }
    let limit = q.limit.unwrap_or(100).clamp(1, 1000);
    let ledger = state.shared.ledger();
    let mut events = ledger.events().await;
    events.reverse();
    events.truncate(limit);
    let counts: serde_json::Map<String, Value> = ledger
        .count_by_kind()
        .await
        .into_iter()
        .map(|(k, n)| (k.as_str().to_string(), json!(n)))
        .collect();
    Json(json!({
        "count": events.len(),
        "total": ledger.len().await,
        "by_kind": counts,
        "pending": ledger.pending().await,
        "events": events,
    }))
    .into_response()
}

/// Operator-entered financial event. Fills and settlements are NOT
/// accepted here — they come from the modules that observed them.
#[derive(Deserialize)]
struct AccountingEventBody {
    /// `deposit` | `withdrawal` | `transfer` | `funding_adjustment` | `fee` | `correction`.
    kind: String,
    wallet: String,
    /// Base asset for corrections; the cash asset otherwise (defaults to
    /// `quote_asset`).
    asset: Option<String>,
    quote_asset: String,
    /// `buy` / `sell` (corrections, negative funding adjustments).
    side: Option<String>,
    quantity: Option<f64>,
    price: Option<f64>,
    quote_amount: Option<f64>,
    fee: Option<f64>,
    /// Unique reference of the fact (bank / exchange reference, ticket id).
    reference_id: String,
    /// Finding / ticket the entry answers (required for corrections).
    correlation_id: Option<String>,
    position_id: Option<String>,
    counterparty_wallet: Option<String>,
    venue: Option<String>,
    strategy: Option<String>,
    detail: Option<String>,
}

/// `POST /api/accounting/events` — book one operator-entered event through
/// the global ledger (same idempotency, postings, audit as module events).
async fn accounting_event_post(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Json(body): Json<AccountingEventBody>,
) -> Response {
    use bot_core::accounting::{AccountingEvent, Applied, EventKind, EventSide};
    use bot_core::models::Venue;
    let principal = match require_role(&state, &headers, Role::Operator, "ledger_event").await {
        Ok(p) => p,
        Err(e) => return e,
    };
    let Some(kind) = EventKind::parse(body.kind.trim()) else {
        return (
            StatusCode::BAD_REQUEST,
            format!("unknown event kind '{}'", body.kind),
        )
            .into_response();
    };
    if matches!(kind, EventKind::Fill | EventKind::Settlement) {
        return (
            StatusCode::BAD_REQUEST,
            "fills and settlements are booked by the module that observed them, not over the API",
        )
            .into_response();
    }
    let side = match body.side.as_deref().map(str::trim) {
        None | Some("") => None,
        Some(s) => match EventSide::parse(s) {
            Some(side) => Some(side),
            None => {
                return (StatusCode::BAD_REQUEST, format!("unknown side '{s}'")).into_response()
            }
        },
    };
    let venue = match body.venue.as_deref().map(str::trim) {
        None | Some("") => Venue::Paper,
        Some(v) => match Venue::parse(v) {
            Some(venue) => venue,
            None => {
                return (StatusCode::BAD_REQUEST, format!("unknown venue '{v}'")).into_response()
            }
        },
    };
    let actor = actor_of(Some(&principal));
    let event = AccountingEvent {
        kind,
        module: BotModule::Telegram,
        venue,
        wallet: body.wallet.trim().to_string(),
        strategy: body
            .strategy
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or("operator")
            .to_string(),
        asset: body
            .asset
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or(body.quote_asset.trim())
            .to_string(),
        quote_asset: body.quote_asset.trim().to_string(),
        side,
        quantity: body.quantity.unwrap_or(0.0),
        price: body.price,
        quote_amount: body.quote_amount.unwrap_or(0.0),
        fee: body.fee.unwrap_or(0.0),
        mode: state.shared.execution_mode().await,
        reference_id: body.reference_id.trim().to_string(),
        correlation_id: body.correlation_id.clone(),
        position_id: body.position_id.clone(),
        trade_id: None,
        counterparty_wallet: body.counterparty_wallet.clone(),
        ts: chrono::Utc::now(),
        detail: format!(
            "operator={} {}",
            actor,
            body.detail.clone().unwrap_or_default()
        ),
    };
    let event_id = event.event_id();
    let applied = state.shared.ledger().submit(event).await;
    let (outcome, status) = match &applied {
        Applied::New(_) => ("new", StatusCode::OK),
        Applied::Duplicate => ("duplicate", StatusCode::OK),
        Applied::Rejected(_) => ("rejected", StatusCode::BAD_REQUEST),
    };
    state
        .audit
        .record(
            &actor,
            "ledger_event",
            Some(&event_id),
            if status == StatusCode::OK {
                AuditOutcome::Success
            } else {
                AuditOutcome::Failure
            },
            json!({ "kind": kind.as_str(), "outcome": outcome }),
        )
        .await;
    match applied {
        Applied::New(effect) => Json(json!({
            "ok": true,
            "outcome": outcome,
            "event_id": event_id,
            "effect": effect,
        }))
        .into_response(),
        Applied::Duplicate => Json(json!({
            "ok": true,
            "outcome": outcome,
            "event_id": event_id,
        }))
        .into_response(),
        Applied::Rejected(reason) => (
            status,
            Json(json!({ "ok": false, "outcome": outcome, "reason": reason })),
        )
            .into_response(),
    }
}

#[derive(Debug, serde::Deserialize)]
struct SweepResolveBody {
    trade_key: String,
    /// Fencing identity the operator read from the sweep (`owner` + `epoch`).
    owner: String,
    epoch: i64,
    /// `abandon` (provably never broadcast) or `link` (on-chain signature).
    decision: String,
    signature: Option<String>,
    /// The operator's EXPLICIT claim that the attempt never left the process.
    /// Required for an `abandon` whose signature the chain does not know —
    /// a free trade key lets a retry send, so this must be a human assertion
    /// rather than an inference from an RPC answer.
    #[serde(default)]
    operator_asserts_never_sent: bool,
}

/// `GET /api/accounting/reservations/sweep?limit=` — the ambiguous durable
/// pre-broadcast reservations: lapsed `reserved` claims whose owner never
/// linked or abandoned them, and `linked` rows with no signature. Read-only
/// and fail-closed: an unreadable sweep reports `available: false`, never an
/// empty list.
async fn accounting_reservation_sweep(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Query(q): Query<LimitQuery>,
) -> Response {
    if let Err(e) = require_role(&state, &headers, Role::Readonly, "reservation_sweep_read").await {
        return e;
    }
    let Some(db) = state.db.clone() else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            "broadcast reservation sweep needs the durable store",
        )
            .into_response();
    };
    let limit = q.limit.unwrap_or(100).clamp(1, 1000) as i64;
    let sweep = ReservationSweepService::new(db).scan(limit).await;
    let status = if sweep.available {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    };
    (status, Json(sweep)).into_response()
}

#[derive(Debug, serde::Deserialize)]
struct ChainCheckQuery {
    trade_key: String,
    signature: Option<String>,
    #[serde(default)]
    operator_asserts_never_sent: bool,
}

#[derive(Debug, serde::Deserialize)]
struct SignatureScanQuery {
    trade_key: String,
    signature: Option<String>,
    /// Alternate signatures the same attempt could have landed under (a
    /// re-signed or fee-payer-swapped transaction lands under a different
    /// signature). Comma-separated; ignored entries are reported as `unknown`.
    #[serde(default)]
    alternates: Option<String>,
    #[serde(default)]
    operator_asserts_never_sent: bool,
}

/// `GET /api/accounting/reservations/signature-scan?trade_key=&signature=&alternates=a,b`
/// — the same question as the chain check, asked across EVERY signature the
/// attempt could have landed under. `duplicate_detected` is true when more
/// than one of them is on chain, which means a duplicate send already
/// happened: nothing is freed, nothing is booked, and a human decides.
/// Without a chain reader the answer is `unknown` — never `absent`.
async fn accounting_reservation_signature_scan(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Query(q): Query<SignatureScanQuery>,
) -> Response {
    if let Err(e) = require_role(
        &state,
        &headers,
        Role::Readonly,
        "reservation_signature_scan",
    )
    .await
    {
        return e;
    }
    let alternates: Vec<String> = q
        .alternates
        .as_deref()
        .unwrap_or("")
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect();
    let report = crate::reservation_chain::verify_signatures(
        state.chain_probe.as_deref(),
        &q.trade_key,
        q.signature.as_deref(),
        &alternates,
        q.operator_asserts_never_sent,
    )
    .await;
    (StatusCode::OK, Json(report)).into_response()
}

/// `GET /api/accounting/reservations/chain-check?trade_key=&signature=` —
/// does the ambiguous attempt's transaction exist on chain, and which
/// decision does that justify? Read-only. `existence` is `landed` (the node
/// returned the transaction), `absent` (the node does not know it — NOT proof
/// nothing was sent) or `unknown` (the lookup failed; never treated as
/// absent). `abandon_allowed` is true only for `absent` WITH an explicit
/// operator non-send assertion.
async fn accounting_reservation_chain_check(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Query(q): Query<ChainCheckQuery>,
) -> Response {
    if let Err(e) = require_role(&state, &headers, Role::Readonly, "reservation_chain_check").await
    {
        return e;
    }
    let Some(db) = state.db.clone() else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            "broadcast reservation sweep needs the durable store",
        )
            .into_response();
    };
    let service = ReservationSweepService::new(db).with_chain(state.chain_probe.clone());
    let report = service
        .chain_report(
            &q.trade_key,
            q.signature.as_deref(),
            q.operator_asserts_never_sent,
        )
        .await;
    (StatusCode::OK, Json(report)).into_response()
}

/// `POST /api/accounting/reservations/sweep` — resolve ONE ambiguous claim.
/// The owner + epoch from the sweep are a fencing token: if the claim changed
/// hands since it was read the decision is refused (409), never applied to a
/// live attempt. Deciding `abandon` frees the trade key for a retry, so this
/// is an operator (money-moving) action and it is audited either way.
async fn accounting_reservation_sweep_resolve(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Json(body): Json<SweepResolveBody>,
) -> Response {
    let principal = match require_role(
        &state,
        &headers,
        Role::Operator,
        "reservation_sweep_resolve",
    )
    .await
    {
        Ok(p) => p,
        Err(e) => return e,
    };
    let actor = actor_of(Some(&principal));
    // Validate the request shape before touching the store, so a malformed
    // decision is a 400 regardless of database availability.
    let decision = match body.decision.as_str() {
        "abandon" => SweepDecision::Abandon,
        "link" => match body.signature.as_deref() {
            Some(sig) => SweepDecision::Link(sig.to_string()),
            None => {
                return (
                    StatusCode::BAD_REQUEST,
                    "a link decision needs the on-chain signature",
                )
                    .into_response()
            }
        },
        other => {
            return (
                StatusCode::BAD_REQUEST,
                format!("unknown decision {other:?}; expected `abandon` or `link`"),
            )
                .into_response()
        }
    };
    let Some(db) = state.db.clone() else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            "broadcast reservation sweep needs the durable store",
        )
            .into_response();
    };
    let service = ReservationSweepService::new(db).with_chain(state.chain_probe.clone());
    // Freeing a trade key lets a retry send. Refuse an `abandon` whose
    // transaction is on chain, or whose non-send the operator did not assert.
    if decision == SweepDecision::Abandon {
        let signature = body.signature.as_deref();
        if let Err(blocked) = service
            .authorize_abandon(&body.trade_key, signature, body.operator_asserts_never_sent)
            .await
        {
            tracing::warn!(
                trade = %body.trade_key,
                reason = %blocked,
                "reservation abandon refused by the chain check"
            );
            state
                .audit
                .record(
                    &actor,
                    "reservation_sweep_resolve",
                    Some(&body.trade_key),
                    AuditOutcome::Failure,
                    json!({ "decision": "abandon", "blocked": blocked }),
                )
                .await;
            return (
                StatusCode::CONFLICT,
                Json(json!({
                    "ok": false,
                    "resolved": false,
                    "reason": blocked,
                    "chain_checked": true,
                })),
            )
                .into_response();
        }
    }
    let resolved = service
        .resolve(&body.trade_key, &body.owner, body.epoch, decision)
        .await;
    let (status, ok, reason) = match resolved {
        Ok(true) => (StatusCode::OK, true, None),
        Ok(false) => (
            StatusCode::CONFLICT,
            false,
            Some(
                "the reservation no longer matches the owner/epoch you read \
                 (already terminal or taken over); re-read the sweep"
                    .to_string(),
            ),
        ),
        Err(e) => (StatusCode::BAD_REQUEST, false, Some(e)),
    };
    state
        .audit
        .record(
            &actor,
            "reservation_sweep_resolve",
            Some(&body.trade_key),
            if ok {
                AuditOutcome::Success
            } else {
                AuditOutcome::Failure
            },
            json!({
                "decision": body.decision,
                "owner": body.owner,
                "epoch": body.epoch,
                "signature": body.signature,
                "reason": reason,
            }),
        )
        .await;
    (
        status,
        Json(json!({ "ok": ok, "resolved": ok, "reason": reason })),
    )
        .into_response()
}

/// `GET /api/accounting/conflict` — the latched journal-vs-book conflict, if
/// any. A non-null id means every entry decision is refused (fail closed)
/// until the book is repaired from the durable journal.
async fn accounting_conflict(State(state): State<ApiState>, headers: HeaderMap) -> Response {
    if let Err(e) = require_role(&state, &headers, Role::Readonly, "ledger_conflict_read").await {
        return e;
    }
    let ledger = state.shared.ledger();
    let mismatches = ledger.conflict_payload_mismatches().await;
    Json(json!({
        "conflict": ledger.journal_conflict().await,
        "pending": ledger.pending_count().await,
        "events": ledger.len().await,
        // Both sides of a proven same-id conflict: the event this process held
        // against the durable rows, and which fields diverged. Empty when no
        // conflict is latched. Read-only — the latch and the fail-closed state
        // are untouched.
        "payload_mismatches": mismatches,
    }))
    .into_response()
}

/// `POST /api/accounting/conflict/repair` — rebuild this replica's book from
/// the durable journal. This is the only way out of a proven conflict: the
/// journal wins, and local rows it does not back are dropped (never
/// reversed into invented money). Returns 409 when the journal cannot
/// adjudicate, so the operator keeps the fail-closed state.
async fn accounting_conflict_repair(State(state): State<ApiState>, headers: HeaderMap) -> Response {
    let principal =
        match require_role(&state, &headers, Role::Operator, "ledger_conflict_repair").await {
            Ok(p) => p,
            Err(e) => return e,
        };
    let actor = actor_of(Some(&principal));
    let report = state.shared.ledger().reconcile_from_journal().await;
    let status = if report.repaired {
        StatusCode::OK
    } else {
        StatusCode::CONFLICT
    };
    state
        .audit
        .record(
            &actor,
            "ledger_conflict_repair",
            report.claimed_conflict.as_deref(),
            if report.repaired {
                AuditOutcome::Success
            } else {
                AuditOutcome::Failure
            },
            json!({ "repaired": report.repaired, "reason": report.reason }),
        )
        .await;
    (
        status,
        Json(json!({ "ok": report.repaired, "report": report })),
    )
        .into_response()
}

/// `GET /api/accounting/findings?limit=` — recent accounting reconciliation
/// findings (from the journal in use).
async fn accounting_findings(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Query(q): Query<LimitQuery>,
) -> Response {
    if let Err(e) = require_role(&state, &headers, Role::Readonly, "findings_read").await {
        return e;
    }
    let limit = q.limit.unwrap_or(100).clamp(1, 1000);
    let store = state.shared.ledger().store().await;
    match store.recent_findings(limit).await {
        Some(findings) => {
            Json(json!({ "count": findings.len(), "findings": findings })).into_response()
        }
        None => (
            StatusCode::SERVICE_UNAVAILABLE,
            "accounting findings journal unavailable",
        )
            .into_response(),
    }
}

/// `GET /api/risk/global` — the global risk configuration in force, the
/// active venue / strategy kill switches, the portfolio totals and the
/// most recent decisions.
async fn risk_global(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Query(q): Query<LimitQuery>,
) -> Response {
    if let Err(e) = require_role(&state, &headers, Role::Readonly, "risk_global_read").await {
        return e;
    }
    let engine = state.shared.global_risk();
    let limit = q.limit.unwrap_or(50).clamp(1, 256);
    let view = engine.portfolio(state.shared.marks().await).await;
    Json(json!({
        "config": engine.config().await,
        "global_kill_switch": state.shared.kill_switch(),
        "kill_switches": engine.switches().active(),
        "portfolio": {
            "reference_asset": view.reference_asset,
            "total_exposure_ref": view.total_exposure_ref,
            "open_positions": view.open_positions,
            "realized_today_ref": view.realized_today_ref,
            "drawdown_ref": view.drawdown_ref(),
            "utilization": view.utilization,
            "missing_rates": view.missing_rates,
        },
        "recent_decisions": engine.recent_decisions(limit).await,
    }))
    .into_response()
}

#[derive(Deserialize)]
struct KillSwitchBody {
    /// `venue:<venue>` or `strategy:<label>`.
    scope: String,
    engaged: bool,
    reason: Option<String>,
}

/// `POST /api/risk/kill-switch` — engage or release a venue / strategy
/// kill switch (durable, audited). Configuration-pinned switches cannot be
/// released here.
async fn risk_kill_switch(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Json(body): Json<KillSwitchBody>,
) -> Response {
    use bot_core::global_risk::{KillScope, SwitchOutcome};
    let principal = match require_role(&state, &headers, Role::Operator, "kill_switch_scope").await
    {
        Ok(p) => p,
        Err(e) => return e,
    };
    let Some(scope) = KillScope::parse(&body.scope) else {
        return (
            StatusCode::BAD_REQUEST,
            format!(
                "unknown scope '{}' (use venue:<venue> or strategy:<label>)",
                body.scope
            ),
        )
            .into_response();
    };
    let actor = actor_of(Some(&principal));
    let reason = body
        .reason
        .clone()
        .unwrap_or_else(|| "api /risk/kill-switch".into());
    let engine = state.shared.global_risk();
    let outcome = if body.engaged {
        engine.engage(scope.clone(), &reason, &actor).await
    } else {
        engine.release(scope.clone(), &reason, &actor).await
    };
    let (label, ok) = match outcome {
        SwitchOutcome::Changed => ("changed", true),
        SwitchOutcome::Unchanged => ("unchanged", true),
        SwitchOutcome::PinnedByConfig => ("pinned_by_config", false),
    };
    state
        .audit
        .record(
            &actor,
            if body.engaged {
                "kill_switch_engage"
            } else {
                "kill_switch_release"
            },
            Some(&scope.as_string()),
            if ok {
                AuditOutcome::Success
            } else {
                AuditOutcome::Denied
            },
            json!({ "outcome": label, "reason": reason }),
        )
        .await;
    let status = if ok {
        StatusCode::OK
    } else {
        StatusCode::CONFLICT
    };
    (
        status,
        Json(json!({
            "ok": ok,
            "scope": scope.as_string(),
            "outcome": label,
            "active": engine.switches().get(&scope).map(|s| s.is_active()).unwrap_or(false),
        })),
    )
        .into_response()
}

/// `GET /api/ha` — TASK 6 distributed state: this worker's identity,
/// generation, state and readiness (with reasons), the roles it holds, the
/// cluster registry with heartbeat ages, every lease with holder /
/// generation / expiry, the durable feed cursors with lag, unresolved feed
/// gaps and the most recent recovery records.
async fn ha_status(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Query(q): Query<LimitQuery>,
) -> Response {
    if let Err(e) = require_role(&state, &headers, Role::Readonly, "ha_read").await {
        return e;
    }
    let limit = q.limit.unwrap_or(50).clamp(1, 500);
    let ha = state.shared.ha();
    let store = ha.store().await;
    let now = store.now().await.unwrap_or_else(|_| chrono::Utc::now());
    let settings = ha.settings().await;
    let timeout = settings.heartbeat_timeout;
    let readiness = ha.readiness().await;
    let registration = ha.registration().await;

    let workers: Vec<Value> = store
        .workers()
        .await
        .unwrap_or_default()
        .into_iter()
        .map(|w| {
            json!({
                "worker_id": w.worker_id,
                "generation": w.generation,
                "mode": w.mode,
                "state": w.state.as_str(),
                "host": w.host,
                "pid": w.pid,
                "version": w.version,
                "detail": w.detail,
                "last_seen_at": w.last_seen_at,
                "age_secs": w.age_secs(now),
                "health": bot_core::ha::WorkerHealth::of(&w, now, timeout).as_str(),
            })
        })
        .collect();

    let leases: Vec<Value> = store
        .leases()
        .await
        .unwrap_or_default()
        .into_iter()
        .map(|l| {
            json!({
                "role": l.role.as_string(),
                "holder": l.holder,
                "generation": l.generation,
                "acquired_at": l.acquired_at,
                "expires_at": l.expires_at,
                "ttl_secs": l.ttl_secs(now),
                "takeover_count": l.takeover_count,
                "previous_holder": l.previous_holder,
                "released": l.released,
                "live": l.is_live(now),
            })
        })
        .collect();

    let cursors: Vec<Value> = store
        .cursors()
        .await
        .unwrap_or_default()
        .into_iter()
        .map(|c| {
            json!({
                "key": c.key(),
                "feed": c.feed.as_str(),
                "scope": c.scope,
                "position": c.position,
                "token": c.token,
                "lag_secs": c.lag_secs(now),
                "processed": c.processed_count,
                "duplicates": c.duplicate_count,
                "gaps": c.gap_count,
                "worker_id": c.worker_id,
                "updated_at": c.updated_at,
            })
        })
        .collect();

    let gaps: Vec<Value> = store
        .gaps(true, limit)
        .await
        .unwrap_or_default()
        .into_iter()
        .map(|g| {
            json!({
                "feed": g.feed.as_str(),
                "scope": g.scope,
                "from_position": g.from_position,
                "to_position": g.to_position,
                "missing": g.len(),
                "status": g.status.as_str(),
                "worker_id": g.worker_id,
                "detected_at": g.detected_at,
            })
        })
        .collect();

    let recovery: Vec<Value> = store
        .recovery_records(limit)
        .await
        .unwrap_or_default()
        .into_iter()
        .map(|r| {
            json!({
                "worker_id": r.worker_id,
                "generation": r.generation,
                "trigger": r.trigger,
                "scope": r.scope,
                "subject": r.subject,
                "action": r.action.as_str(),
                "detail": r.detail,
                "ts": r.ts,
            })
        })
        .collect();

    Json(json!({
        "worker": {
            "worker_id": ha.worker_id(),
            "generation": ha.generation().await,
            "state": ha.state().await.as_str(),
            "mode": settings.mode.as_str(),
            "backend": store.backend(),
            "draining": ha.is_draining(),
            "recovery_complete": ha.recovery_complete().await,
            "registered": registration.is_some(),
            "held_roles": ha.held_roles().await,
            "required_roles": settings
                .required_roles
                .iter()
                .map(|r| r.as_string())
                .collect::<Vec<_>>(),
        },
        "readiness": {
            "ready": readiness.ready,
            "detail": readiness.detail(),
            "reasons": readiness
                .reasons
                .iter()
                .map(|r| json!({ "kind": r.as_str(), "detail": r.to_string() }))
                .collect::<Vec<_>>(),
        },
        "workers": workers,
        "leases": leases,
        "cursors": cursors,
        "unresolved_gaps": gaps,
        "recent_recovery": recovery,
    }))
    .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    use bot_core::config::AppConfig;
    use bot_core::obs::health::ComponentStatus;
    use bot_core::state::AppState;

    fn test_state(metrics_enabled: bool) -> ApiState {
        let shared = AppState::new(AppConfig::from_defaults());
        ApiState {
            audit: AuditTrail::new(None, shared.events.clone()),
            shared,
            api_key: None,
            auth: None,
            limiter: RateLimiter::new(0),
            sensitive_limiter: RateLimiter::new(10),
            trusted_proxies: Default::default(),
            db: None,
            journal: None,
            chain_probe: None,
            serve_dashboard: false,
            health: Arc::new(HealthRegistry::new()),
            metrics_enabled,
            saas: crate::saas::SaasStore::shared(),
            trading: None,
            module_registry: Arc::new(
                crate::module_runtime::module_registry::TenantModuleRegistry::new(),
            ),
        }
    }

    #[tokio::test]
    async fn journal_routes_report_availability() {
        let (state, owner, _, ro) = rbac_state().await;
        let app = router(state);
        let (status, _, body) = request(
            app.clone(),
            "GET",
            "/api/journal",
            &[("x-api-key", &ro)],
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert!(body.contains("\"available\":false"), "{body}");
        // Rotation is an owner action.
        let (status, _, _) = request(
            app.clone(),
            "POST",
            "/api/journal",
            &[("x-api-key", &ro)],
            None,
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        let (status, _, body) =
            request(app, "POST", "/api/journal", &[("x-api-key", &owner)], None).await;
        // Owner is authorized but no journal is attached in tests => 503.
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{body}");
    }

    async fn get(
        app: Router,
        uri: &str,
        headers: &[(&str, &str)],
    ) -> (StatusCode, HeaderMap, String) {
        request(app, "GET", uri, headers, None).await
    }

    async fn request(
        app: Router,
        method: &str,
        uri: &str,
        headers: &[(&str, &str)],
        body: Option<Value>,
    ) -> (StatusCode, HeaderMap, String) {
        let mut builder = axum::http::Request::builder().method(method).uri(uri);
        for (k, v) in headers {
            builder = builder.header(*k, *v);
        }
        let body = match body {
            Some(v) => {
                builder = builder.header("content-type", "application/json");
                Body::from(v.to_string())
            }
            None => Body::empty(),
        };
        let resp = app.oneshot(builder.body(body).unwrap()).await.unwrap();
        let status = resp.status();
        let hdrs = resp.headers().clone();
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        (status, hdrs, String::from_utf8(bytes.to_vec()).unwrap())
    }

    #[tokio::test]
    async fn health_is_liveness_and_never_degrades() {
        let state = test_state(true);
        // Even with every component down, liveness must stay 200.
        state.health.set("rpc", ComponentStatus::not_ready("down"));
        let (status, _, body) = get(router(state), "/health", &[]).await;
        assert_eq!(status, StatusCode::OK);
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["status"], "ok");
        assert!(v["version"].is_string());
        assert!(v["uptime_s"].is_u64());
        // Liveness leaks nothing about components or configuration.
        assert_eq!(v.as_object().unwrap().len(), 3, "body was {body}");
    }

    #[tokio::test]
    async fn ready_reflects_component_state() {
        let state = test_state(true);
        let (status, _, _body) = get(router(state.clone()), "/ready", &[]).await;
        assert_eq!(status, StatusCode::OK, "empty registry => ready");

        state
            .health
            .set("rpc", ComponentStatus::not_ready("consecutive_failures=5"));
        let (status, _, body) = get(router(state), "/ready", &[]).await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["status"], "degraded");
        assert_eq!(v["ready"], false);
        assert_eq!(v["components"][0]["name"], "rpc");
        assert_eq!(v["components"][0]["ready"], false);
        // Detail is a safe counter string, never an error payload.
        assert_eq!(v["components"][0]["detail"], "consecutive_failures=5");
        assert!(!body.contains("api_key"), "no secret material: {body}");
    }

    #[tokio::test]
    async fn metrics_endpoint_serves_prometheus_text_when_enabled() {
        let state = test_state(true);
        let (status, hdrs, _body) = get(router(state), "/metrics", &[]).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            hdrs.get("content-type").unwrap().to_str().unwrap(),
            "text/plain; version=0.0.4; charset=utf-8"
        );
        // The request itself was instrumented into the global registry, so the
        // exposition must contain at least the HTTP metric family. Values are
        // not asserted: the global registry is shared across tests.
    }

    #[tokio::test]
    async fn metrics_endpoint_is_404_when_disabled() {
        let state = test_state(false);
        let (status, _, _) = get(router(state), "/metrics", &[]).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn request_id_is_echoed_when_safe_and_replaced_when_not() {
        let app = router(test_state(true));
        let (_, hdrs, _) = get(
            app.clone(),
            "/health",
            &[("x-request-id", "client-trace_42")],
        )
        .await;
        assert_eq!(
            hdrs.get("x-request-id").unwrap().to_str().unwrap(),
            "client-trace_42"
        );

        let (_, hdrs, _) = get(app.clone(), "/health", &[("x-request-id", "bad\"value")]).await;
        let echoed = hdrs.get("x-request-id").unwrap().to_str().unwrap();
        assert_ne!(echoed, "bad\"value");
        assert!(echoed.starts_with("req-"), "generated: {echoed}");

        // No inbound header => a generated one is still echoed.
        let (_, hdrs, _) = get(app, "/health", &[]).await;
        let echoed = hdrs.get("x-request-id").unwrap().to_str().unwrap();
        assert!(echoed.starts_with("req-"), "generated: {echoed}");
    }

    #[tokio::test]
    async fn existing_api_health_route_is_unchanged() {
        let (status, _, body) = get(router(test_state(true)), "/api/health", &[]).await;
        assert_eq!(status, StatusCode::OK);
        assert!(body.contains("\"ok\":true"), "{body}");
    }

    // ---- legacy single-key compatibility ---------------------------------

    #[tokio::test]
    async fn legacy_key_still_authorizes_mutations() {
        let mut state = test_state(true);
        state.api_key = Some("legacy-secret".into());
        let app = router(state);
        let (status, _, _) = request(app.clone(), "POST", "/api/kill", &[], None).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        let (status, _, body) = request(
            app.clone(),
            "POST",
            "/api/kill",
            &[("x-api-key", "legacy-secret")],
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        let (status, _, _) = request(
            app,
            "POST",
            "/api/kill",
            &[("authorization", "Bearer legacy-secret")],
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "bearer form accepted");
    }

    #[tokio::test]
    async fn wrong_legacy_keys_are_refused_for_both_header_styles() {
        let mut state = test_state(true);
        state.api_key = Some("legacy-secret".into());
        let app = router(state);
        for (header, value) in [
            ("x-api-key", "legacy-secreu"),
            ("x-api-key", "legacy-secret-longer"),
            ("authorization", "Bearer wrong"),
            ("authorization", "Bearer legacy-secreu"),
        ] {
            let (status, _, _) =
                request(app.clone(), "POST", "/api/kill", &[(header, value)], None).await;
            assert_eq!(status, StatusCode::UNAUTHORIZED);
        }
    }

    #[tokio::test]
    async fn open_dev_mode_allows_mutations_without_key() {
        let app = router(test_state(true));
        let (status, _, _) = request(app, "POST", "/api/kill", &[], None).await;
        assert_eq!(status, StatusCode::OK);
    }

    // ---- RBAC -------------------------------------------------------------

    async fn rbac_state() -> (ApiState, String, String, String) {
        let mut state = test_state(true);
        // Build a registry without touching the process env.
        let auth = Authenticator::empty();
        auth.add_key("owner", "owner-key-1234567890123456", Role::Owner)
            .await;
        auth.add_key("op", "operator-key-12345678901234", Role::Operator)
            .await;
        auth.add_key("ro", "readonly-key-12345678901234", Role::Readonly)
            .await;
        state.auth = Some(Arc::new(auth));
        (
            state,
            "owner-key-1234567890123456".into(),
            "operator-key-12345678901234".into(),
            "readonly-key-12345678901234".into(),
        )
    }

    #[tokio::test]
    async fn analytics_never_labels_a_raw_trade_amount_or_a_latency_as_usd_or_fill_time() {
        // The endpoint needs a database; the CONTRACT that matters is the unit
        // vocabulary, so the same assertions run against the response shape
        // through the router with no DB: 503 is fine, but the moment analytics
        // can answer, no raw amount may be called `_usd` and a latency may not
        // be called fill time.
        let (state, owner, _op, _ro) = rbac_state().await;
        let app = router(state);
        let (status, _, body) = get(
            app,
            "/api/tenant/analytics?timeframe=30d",
            &[("x-api-key", &owner)],
        )
        .await;
        // The trading data plane gates on a tenant membership the test state
        // does not carry, so 401/403 is a legitimate refusal too; what is NOT
        // acceptable is a 200 that mislabels units.
        assert!(
            matches!(
                status,
                StatusCode::SERVICE_UNAVAILABLE
                    | StatusCode::OK
                    | StatusCode::FORBIDDEN
                    | StatusCode::UNAUTHORIZED
            ),
            "unexpected status {status}"
        );
        if status == StatusCode::OK {
            let v: serde_json::Value = serde_json::from_str(&body).expect("json");
            let breakdown = v["module_breakdown"]
                .as_array()
                .cloned()
                .unwrap_or_default();
            for row in breakdown {
                assert!(
                    row.get("volume_usd").is_none() && row.get("fees_usd").is_none(),
                    "a raw trade amount must never be labelled usd: {row}"
                );
                assert_eq!(row["volume_unit"], "raw_trade_amount_unlabelled");
            }
            let q = &v["execution_quality"];
            assert!(
                q.get("avg_fill_time_ms").is_none(),
                "a latency is not fill time"
            );
            assert_eq!(q["reverted_txs"], serde_json::Value::Null);
            assert_eq!(
                v["units"]["usd_source"],
                "tenant_daily_accounting (global ledger, USD-quoted)"
            );
        }
    }

    /// The unit vocabulary is asserted AT THE SOURCE: the router-level test
    /// above cannot reach a 200 without a database and a tenant membership, so
    /// a future edit could silently restore `volume_usd` on a raw amount. This
    /// check fails the moment the misleading keys come back.
    #[test]
    fn analytics_vocabulary_never_relabels_raw_amounts_or_latency() {
        let source = include_str!("trading_data_plane/analytics.rs");
        assert!(
            !source.contains("\"volume_usd\": row.get"),
            "a raw trade amount must not be labelled usd"
        );
        assert!(
            !source.contains("\"fees_usd\": row.get"),
            "a raw fee amount must not be labelled usd"
        );
        assert!(
            !source.contains("\"avg_fill_time_ms\":"),
            "an execution latency is not fill time"
        );
        assert!(
            source.contains("\"avg_execution_latency_ms\":"),
            "the latency must be reported under a name that says what it measures"
        );
        assert!(
            source.contains("\"reverted_txs\": Value::Null"),
            "an execution attempt is not a reverted transaction"
        );
        assert!(
            source.contains("\"units\":"),
            "the response must state which figures are USD and which are not"
        );
    }

    #[tokio::test]
    async fn rbac_roles_gate_mutations() {
        let (state, owner, op, ro) = rbac_state().await;
        let app = router(state);

        // Readonly cannot kill.
        let (status, _, body) = request(
            app.clone(),
            "POST",
            "/api/kill",
            &[("x-api-key", &ro)],
            None,
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{body}");

        // Operator can kill.
        let (status, _, _) = request(
            app.clone(),
            "POST",
            "/api/resume",
            &[("x-api-key", &op)],
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK);

        // Operator cannot switch to LIVE (owner action) but can go to paper.
        let (status, _, _) = request(
            app.clone(),
            "POST",
            "/api/mode",
            &[("x-api-key", &op), ("content-type", "application/json")],
            Some(json!({"mode": "live"})),
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        let (status, _, _) = request(
            app.clone(),
            "POST",
            "/api/mode",
            &[("x-api-key", &op)],
            Some(json!({"mode": "paper"})),
        )
        .await;
        assert_eq!(status, StatusCode::OK);

        // Owner can do everything, incl. key admin.
        let (status, _, body) = request(
            app.clone(),
            "GET",
            "/api/keys",
            &[("x-api-key", &owner)],
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert!(body.contains("owner"), "runtime keys listed: {body}");
        let (status, _, _) =
            request(app.clone(), "GET", "/api/keys", &[("x-api-key", &op)], None).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "operator cannot list keys");

        // Unknown key is 401, not 403.
        let (status, _, _) = request(
            app.clone(),
            "POST",
            "/api/kill",
            &[("x-api-key", "wrong-key")],
            None,
        )
        .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);

        // Missing key is 401.
        let (status, _, _) = request(app, "POST", "/api/kill", &[], None).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn rbac_reads_require_authentication_but_any_role() {
        let (state, _owner, _op, ro) = rbac_state().await;
        let app = router(state);
        let (status, _, _) = request(app.clone(), "GET", "/api/orders", &[], None).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        let (status, _, _) = request(
            app.clone(),
            "GET",
            "/api/orders",
            &[("x-api-key", &ro)],
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let (status, _, body) =
            request(app, "GET", "/api/audit", &[("x-api-key", &ro)], None).await;
        assert_eq!(status, StatusCode::OK);
        assert!(body.contains("\"records\""), "{body}");
    }

    #[tokio::test]
    async fn denied_and_granted_mutations_are_audited() {
        let (state, _owner, _op, ro) = rbac_state().await;
        let audit = state.audit.clone();
        let app = router(state);
        // Denied kill by readonly.
        let (status, _, _) = request(
            app.clone(),
            "POST",
            "/api/kill",
            &[("x-api-key", &ro)],
            None,
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        // Granted kill by the same key after role upgrade is not needed —
        // open path: use owner key.
        let (status, _, _) = request(
            app,
            "POST",
            "/api/kill",
            &[("x-api-key", "owner-key-1234567890123456")],
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let records = audit.recent(50).await;
        let actions: Vec<&str> = records.iter().map(|r| r.action.as_str()).collect();
        assert!(
            actions.contains(&"kill_switch"),
            "granted action audited: {actions:?}"
        );
        let denied = records
            .iter()
            .find(|r| r.outcome == "denied" && r.action == "kill_switch");
        assert!(denied.is_some(), "denial audited: {actions:?}");
    }

    #[tokio::test]
    async fn rate_limit_returns_429_with_retry_after() {
        let mut state = test_state(true);
        state.limiter = RateLimiter::new(2); // 2 requests/min total burst
        let app = router(state);
        let (s1, _, _) = get(app.clone(), "/api/status", &[]).await;
        let (s2, _, _) = get(app.clone(), "/api/status", &[]).await;
        let (s3, hdrs, body) = get(app, "/api/status", &[]).await;
        assert_eq!(s1, StatusCode::OK);
        assert_eq!(s2, StatusCode::OK);
        assert_eq!(s3, StatusCode::TOO_MANY_REQUESTS, "{body}");
        assert!(hdrs.get("retry-after").is_some(), "retry-after header set");
    }

    #[tokio::test]
    async fn key_admin_validates_input() {
        let (state, owner, _, _) = rbac_state().await;
        let app = router(state);
        // Too-short key rejected.
        let (status, _, _) = request(
            app.clone(),
            "POST",
            "/api/keys",
            &[("x-api-key", &owner)],
            Some(json!({"label": "x", "key": "short", "role": "operator"})),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        // Bad role rejected.
        let (status, _, _) = request(
            app.clone(),
            "POST",
            "/api/keys",
            &[("x-api-key", &owner)],
            Some(json!({"label": "x", "key": "012345678901234567890123456789", "role": "root"})),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        // Valid add works and the new key authenticates.
        let (status, _, body) = request(
            app.clone(),
            "POST",
            "/api/keys",
            &[("x-api-key", &owner)],
            Some(
                json!({"label": "ci", "key": "012345678901234567890123456789", "role": "readonly"}),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert!(body.contains("key_hash"));
        assert!(
            !body.contains("012345678901234567890123456789"),
            "plaintext never echoed"
        );
        let (status, _, _) = request(
            app.clone(),
            "GET",
            "/api/audit",
            &[("x-api-key", "012345678901234567890123456789")],
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "new key works immediately");
        // Revoke by hash.
        let v: Value = serde_json::from_str(&body).unwrap();
        let hash = v["key_hash"].as_str().unwrap().to_string();
        let (status, _, _) = request(
            app.clone(),
            "DELETE",
            &format!("/api/keys/{hash}"),
            &[("x-api-key", &owner)],
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let (status, _, _) = request(
            app,
            "GET",
            "/api/audit",
            &[("x-api-key", "012345678901234567890123456789")],
            None,
        )
        .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "revoked key is dead");
    }

    #[tokio::test]
    async fn db_dependent_routes_degrade_without_database() {
        let (state, owner, _, _) = rbac_state().await;
        let app = router(state);
        let (status, _, body) = request(
            app.clone(),
            "GET",
            "/api/db",
            &[("x-api-key", &owner)],
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert!(body.contains("\"database\":\"disabled\""), "{body}");
        let (status, _, body) = request(
            app.clone(),
            "GET",
            "/api/recovery/failed",
            &[("x-api-key", &owner)],
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert!(body.contains("\"available\":false"), "{body}");
        let (status, _, body) =
            request(app, "GET", "/api/wallets", &[("x-api-key", &owner)], None).await;
        assert_eq!(status, StatusCode::OK);
        assert!(body.contains("\"available\":false"), "{body}");
    }

    // ------------------------------------------------------------------
    // TASK 5 — global risk / accounting routes
    // ------------------------------------------------------------------

    #[tokio::test]
    async fn accounting_routes_expose_portfolio_events_and_findings() {
        let (state, _owner, op, ro) = rbac_state().await;
        let app = router(state.clone());
        // Portfolio starts empty and is readable by any role.
        let (status, _, body) = get(
            app.clone(),
            "/api/accounting/portfolio",
            &[("x-api-key", &ro)],
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert!(body.contains("\"open_positions\":0"), "{body}");
        assert!(body.contains("\"reference_asset\":\"USD\""), "{body}");
        // Read-only keys cannot book operator events.
        let deposit = json!({
            "kind": "deposit", "wallet": "treasury", "quote_asset": "USDC",
            "quote_amount": 250.0, "reference_id": "bank-ref-1"
        });
        let (status, _, _) = request(
            app.clone(),
            "POST",
            "/api/accounting/events",
            &[("x-api-key", &ro)],
            Some(deposit.clone()),
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        // An operator can; the same reference twice is a duplicate, not a
        // second booking.
        let (status, _, body) = request(
            app.clone(),
            "POST",
            "/api/accounting/events",
            &[("x-api-key", &op)],
            Some(deposit.clone()),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert!(body.contains("\"outcome\":\"new\""), "{body}");
        let (status, _, body) = request(
            app.clone(),
            "POST",
            "/api/accounting/events",
            &[("x-api-key", &op)],
            Some(deposit),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert!(body.contains("\"outcome\":\"duplicate\""), "{body}");
        assert_eq!(state.shared.ledger().len().await, 1);
        // Fills are never accepted over the API; malformed events are 400.
        let (status, _, _) = request(
            app.clone(),
            "POST",
            "/api/accounting/events",
            &[("x-api-key", &op)],
            Some(json!({
                "kind": "fill", "wallet": "w", "quote_asset": "SOL",
                "quote_amount": 1.0, "reference_id": "x"
            })),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        let (status, _, _) = request(
            app.clone(),
            "POST",
            "/api/accounting/events",
            &[("x-api-key", &op)],
            Some(json!({
                "kind": "correction", "wallet": "w", "quote_asset": "SOL",
                "quantity": 1.0, "side": "buy", "reference_id": "fix-1"
            })),
        )
        .await;
        assert_eq!(
            status,
            StatusCode::BAD_REQUEST,
            "correction without correlation"
        );
        // Events and findings are readable.
        let (status, _, body) = get(
            app.clone(),
            "/api/accounting/events?limit=5",
            &[("x-api-key", &ro)],
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert!(body.contains("\"total\":1"), "{body}");
        assert!(body.contains("\"deposit\":1"), "{body}");
        let (status, _, body) = get(
            app.clone(),
            "/api/accounting/findings",
            &[("x-api-key", &ro)],
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert!(body.contains("\"count\":0"), "{body}");
        // Conflict status is readable; repair is an operator action and a
        // healthy book reports a clean repair (no conflict to clear).
        let (status, _, body) = get(
            app.clone(),
            "/api/accounting/conflict",
            &[("x-api-key", &ro)],
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert!(body.contains("\"conflict\":null"), "{body}");
        let (status, _, _) = request(
            app.clone(),
            "POST",
            "/api/accounting/conflict/repair",
            &[("x-api-key", &ro)],
            None,
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        let (status, _, body) = request(
            app.clone(),
            "POST",
            "/api/accounting/conflict/repair",
            &[("x-api-key", &op)],
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert!(body.contains("\"repaired\":true"), "{body}");
        assert_eq!(state.shared.ledger().len().await, 1);
    }

    #[tokio::test]
    async fn reservation_sweep_is_readonly_readable_and_operator_resolved() {
        let (state, _owner, op, ro) = rbac_state().await;
        let app = router(state.clone());
        // Without a durable store the sweep is an explicit 503, never an
        // empty "nothing to do" answer.
        let (status, _, _) = get(
            app.clone(),
            "/api/accounting/reservations/sweep",
            &[("x-api-key", &ro)],
        )
        .await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        // A malformed decision is refused on shape alone, before the store.
        let (status, _, _) = request(
            app.clone(),
            "POST",
            "/api/accounting/reservations/sweep",
            &[("x-api-key", &op)],
            Some(json!({
                "trade_key": "copy:w:m", "owner": "replica-1", "epoch": 1,
                "decision": "guess"
            })),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        // A `link` decision without a signature is refused too.
        let (status, _, _) = request(
            app.clone(),
            "POST",
            "/api/accounting/reservations/sweep",
            &[("x-api-key", &op)],
            Some(json!({
                "trade_key": "copy:w:m", "owner": "replica-1", "epoch": 1,
                "decision": "link"
            })),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        // Deciding a claim frees money to move, so read-only keys cannot.
        let (status, _, _) = request(
            app.clone(),
            "POST",
            "/api/accounting/reservations/sweep",
            &[("x-api-key", &ro)],
            Some(json!({
                "trade_key": "copy:w:m", "owner": "replica-1", "epoch": 1,
                "decision": "abandon"
            })),
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        // An operator's well-formed decision reaches the store, which is
        // absent here: still an explicit 503, never a silent success.
        let (status, _, _) = request(
            app,
            "POST",
            "/api/accounting/reservations/sweep",
            &[("x-api-key", &op)],
            Some(json!({
                "trade_key": "copy:w:m", "owner": "replica-1", "epoch": 1,
                "decision": "abandon"
            })),
        )
        .await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    }

    #[tokio::test]
    async fn signature_scan_reports_no_chain_reader_as_inconclusive() {
        let (state, _owner, _op, ro) = rbac_state().await;
        let app = router(state.clone());
        let (status, _, body) = get(
            app.clone(),
            "/api/accounting/reservations/signature-scan?trade_key=copy:w:m&signature=abc&alternates=def,ghi",
            &[("x-api-key", &ro)],
        )
        .await;
        assert_eq!(status, StatusCode::OK, "read-only and store-independent");
        let v: serde_json::Value = serde_json::from_str(&body).expect("json body");
        assert_eq!(v["existence"], "unknown", "no reader configured: {v}");
        assert_eq!(v["abandon_allowed"], false);
        assert_eq!(v["link_allowed"], false);
        assert_eq!(v["settled_allowed"], false);
        assert_eq!(v["duplicate_detected"], false);
        assert!(
            v["reason"]
                .as_str()
                .unwrap_or_default()
                .contains("no chain reader"),
            "{v}"
        );
    }

    #[tokio::test]
    async fn signature_scan_needs_a_readonly_role() {
        let (state, _owner, _op, _ro) = rbac_state().await;
        let app = router(state.clone());
        let (status, _, _) = get(
            app.clone(),
            "/api/accounting/reservations/signature-scan?trade_key=copy:w:m",
            &[("x-api-key", "wrong")],
        )
        .await;
        assert!(
            status == StatusCode::UNAUTHORIZED || status == StatusCode::FORBIDDEN,
            "unauthenticated access must be refused, got {status}"
        );
    }

    #[tokio::test]
    async fn reservation_chain_check_is_read_only_and_never_invents_an_absence() {
        let (state, _owner, op, ro) = rbac_state().await;
        let app = router(state.clone());
        // Readable by the read-only role...
        let (status, _, _) = get(
            app.clone(),
            "/api/accounting/reservations/chain-check?trade_key=copy:w:m",
            &[("x-api-key", &ro)],
        )
        .await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "no durable store");
        // ...and a malformed signature is still refused as such once the
        // endpoint can answer; without a store the answer is the store.
        let (status, _, body) = get(
            app.clone(),
            "/api/accounting/reservations/chain-check?trade_key=copy:w:m&signature=abc",
            &[("x-api-key", &ro)],
        )
        .await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        assert!(body.is_empty() || !body.contains("\"existence\""), "{body}");
        // An `abandon` that does not assert the non-send is refused on shape
        // before any store or chain is consulted.
        let (status, _, _) = request(
            app.clone(),
            "POST",
            "/api/accounting/reservations/sweep",
            &[("x-api-key", &op)],
            Some(json!({
                "trade_key": "copy:w:m", "owner": "replica-1", "epoch": 1,
                "decision": "abandon",
                "signature": "abc",
                "operator_asserts_never_sent": true
            })),
        )
        .await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "store is required");
    }

    #[tokio::test]
    async fn kill_switch_scopes_are_operator_actions_and_audited() {
        let (state, _owner, op, ro) = rbac_state().await;
        let app = router(state.clone());
        let body = json!({ "scope": "venue:polymarket", "engaged": true, "reason": "incident" });
        let (status, _, _) = request(
            app.clone(),
            "POST",
            "/api/risk/kill-switch",
            &[("x-api-key", &ro)],
            Some(body.clone()),
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        let (status, _, text) = request(
            app.clone(),
            "POST",
            "/api/risk/kill-switch",
            &[("x-api-key", &op)],
            Some(body),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{text}");
        assert!(text.contains("\"outcome\":\"changed\""), "{text}");
        assert!(state
            .shared
            .global_risk()
            .switches()
            .venue_killed(bot_core::models::Venue::PolymarketClob)
            .is_some());
        // Visible on the read route; unknown scopes are 400.
        let (status, _, text) = get(app.clone(), "/api/risk/global", &[("x-api-key", &ro)]).await;
        assert_eq!(status, StatusCode::OK);
        assert!(
            text.contains("venue:polymarket") || text.contains("\"scope\":\"venue\""),
            "{text}"
        );
        let (status, _, _) = request(
            app.clone(),
            "POST",
            "/api/risk/kill-switch",
            &[("x-api-key", &op)],
            Some(json!({ "scope": "exchange:nowhere", "engaged": true })),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        // Release.
        let (status, _, text) = request(
            app.clone(),
            "POST",
            "/api/risk/kill-switch",
            &[("x-api-key", &op)],
            Some(json!({ "scope": "venue:polymarket", "engaged": false })),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{text}");
        assert!(state
            .shared
            .global_risk()
            .switches()
            .venue_killed(bot_core::models::Venue::PolymarketClob)
            .is_none());
        let audit = state.audit.recent(20).await;
        assert!(
            audit.iter().any(|r| r.action == "kill_switch_engage"),
            "{audit:?}"
        );
        assert!(
            audit.iter().any(|r| r.action == "kill_switch_release"),
            "{audit:?}"
        );
    }

    #[tokio::test]
    async fn orders_route_rejects_unknown_status_filter() {
        let (state, _owner, _op, ro) = rbac_state().await;
        let app = router(state);
        let (status, _, _) = request(
            app,
            "GET",
            "/api/orders?status=bogus",
            &[("x-api-key", &ro)],
            None,
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }
}
