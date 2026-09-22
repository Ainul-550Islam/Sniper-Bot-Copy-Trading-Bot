//! Observability wiring for the control-plane server: HTTP handlers
//! (`/health`, `/ready`, `/metrics`), a request-correlation middleware, an
//! event pump that derives metrics from the [`EventBus`], and a periodic
//! state sampler that mirrors [`AppState`] into gauges and health components.
//!
//! SECURITY — three boundaries are enforced here:
//!
//! 1. Metric **label values** only ever come from closed, code-defined sets
//!    (module names, execution modes, matched route patterns, outcome
//!    literals). Nothing user-controlled (symbols, wallets, signatures,
//!    request paths, header values) becomes a label.
//! 2. Health `detail` strings contain booleans/counts/enum names only — never
//!    error payloads, because RPC error strings can embed provider URLs that
//!    carry API keys.
//! 3. `/health` is liveness: it inspects nothing but the process itself, so a
//!    downstream outage can never make the orchestrator kill a live process.
//!    `/ready` is the one that reflects dependency state.

use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::{
    body::Body,
    extract::{MatchedPath, State},
    http::{HeaderMap, HeaderName, Request, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::json;
use tokio::sync::broadcast;
use tracing::{info, info_span, Instrument};

use bot_core::db::Database;
use bot_core::events::AppEvent;
use bot_core::models::BotModule;
use bot_core::obs::health::{ComponentStatus, HealthRegistry};
use bot_core::obs::metrics::{self, Registry, LATENCY_BUCKETS_MS};
use bot_core::redis_kv::RedisKv;
use bot_core::state::Shared;
use solana_kit::rpc::Rpc;

use crate::api::ApiState;

/// A module heartbeat older than this means the loop is stuck or dead.
const HEARTBEAT_FRESH_SECS: i64 = 90;

// ---------------------------------------------------------------------------
// HTTP handlers
// ---------------------------------------------------------------------------

/// `GET /health` — liveness. Never touches external services or component
/// state; always 200 while the process can serve HTTP.
pub async fn health_live(State(state): State<ApiState>) -> Response {
    Json(json!({
        "status": "ok",
        "version": env!("CARGO_PKG_VERSION"),
        "uptime_s": state.health.uptime().as_secs(),
    }))
    .into_response()
}

/// `GET /ready` — readiness. 200 when every registered component is ready,
/// 503 otherwise; the body always carries the full component report.
pub async fn ready(State(state): State<ApiState>) -> Response {
    let report = state.health.snapshot();
    let code = if report.ready {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    };
    (code, Json(report)).into_response()
}

/// `GET /metrics` — Prometheus text exposition (0.0.4). 404 when metrics are
/// disabled in the config, so the surface can be turned off entirely.
pub async fn metrics_endpoint(State(state): State<ApiState>) -> Response {
    if !state.metrics_enabled {
        return (StatusCode::NOT_FOUND, "metrics disabled").into_response();
    }
    (
        [(
            axum::http::header::CONTENT_TYPE,
            "text/plain; version=0.0.4; charset=utf-8",
        )],
        metrics::global().encode(),
    )
        .into_response()
}

// ---------------------------------------------------------------------------
// Request middleware: correlation IDs + HTTP metrics
// ---------------------------------------------------------------------------

/// Derive the correlation ID for a request: an inbound `x-request-id` is
/// honoured only when it is short and made of safe characters (it is echoed
/// into responses and logs, so it must not smuggle control data); otherwise a
/// fresh ID is generated.
pub fn request_id(headers: &HeaderMap, shared: &Shared) -> String {
    match headers
        .get("x-request-id")
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
    {
        Some(s) if !s.is_empty() && s.len() <= 128 => {
            let safe = s
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
            if safe {
                return s.to_string();
            }
            shared.next_id("req")
        }
        _ => shared.next_id("req"),
    }
}

/// Axum middleware (applied via `route_layer` so [`MatchedPath`] is known):
/// assigns a correlation ID, wraps the handler in a `request` span, emits one
/// structured log line per request and records HTTP metrics.
///
/// The `route` label is the *matched pattern* (e.g. `/api/modules/:name/enable`),
/// never the concrete path — that keeps cardinality bounded even under random
/// 404 probing of matched-route space.
pub async fn request_context(
    State(state): State<ApiState>,
    req: Request<Body>,
    next: Next,
) -> Response {
    let request_id = request_id(req.headers(), &state.shared);
    let method = req.method().clone();
    let route = req
        .extensions()
        .get::<MatchedPath>()
        .map(|m| m.as_str().to_string())
        .unwrap_or_else(|| "unmatched".to_string());

    let started = Instant::now();
    let span = info_span!(
        "request",
        request_id = %request_id,
        method = %method,
        route = %route
    );
    let mut response = next.run(req).instrument(span).await;
    let status = response.status();
    let duration_ms = started.elapsed().as_millis() as u64;

    if state.metrics_enabled {
        let reg = metrics::global();
        reg.counter(
            "bot_http_requests_total",
            "HTTP requests served by the control plane.",
            &[
                ("route", route.as_str()),
                ("method", method.as_str()),
                ("status", &status.as_u16().to_string()),
            ],
        )
        .inc();
        reg.histogram(
            "bot_http_request_duration_ms",
            "HTTP request duration in milliseconds.",
            &[("route", route.as_str())],
            LATENCY_BUCKETS_MS,
        )
        .observe(duration_ms);
    }

    // Exactly one info line per request — no per-handler chatter needed.
    info!(
        request_id = %request_id,
        method = %method,
        route = %route,
        status = status.as_u16(),
        duration_ms,
        "http request"
    );

    // Echo the correlation ID back so clients can join logs to responses.
    // request_id() guarantees the value is safe ASCII (or generated), so the
    // parse can only fail on an internal bug — dropping the header then is
    // the right degradation.
    if let Ok(value) = request_id.parse::<axum::http::HeaderValue>() {
        response
            .headers_mut()
            .insert(HeaderName::from_static("x-request-id"), value);
    }
    response
}

// ---------------------------------------------------------------------------
// Event pump: EventBus -> metrics
// ---------------------------------------------------------------------------

/// Execution-mode label sanitiser: only the three known modes pass through,
/// anything else collapses to `other` so a stray string can never inflate
/// label cardinality.
fn sanitize_mode(mode: &str) -> &'static str {
    match mode {
        "paper" | "simulate" | "live" => match mode {
            "paper" => "paper",
            "simulate" => "simulate",
            _ => "live",
        },
        _ => "other",
    }
}

/// Translate one [`AppEvent`] into metric updates. Pure with respect to the
/// registry it is given, which makes it unit-testable with a private
/// [`Registry`].
///
/// Counters that the authoritative [`AppState`] already maintains
/// (orders sent/filled/failed, signals, risk rejections) are *not* duplicated
/// here — the sampler mirrors them; double counting would be worse than
/// sampling latency. What is recorded here are events whose detail (latency,
/// acceptance, fatality) lives only in the event itself.
pub fn record_event(reg: &Registry, ev: &AppEvent) {
    match ev {
        AppEvent::Launch { accepted, .. } => {
            reg.counter(
                "bot_launches_total",
                "Token launches observed by the sniper module.",
                &[("accepted", if *accepted { "true" } else { "false" })],
            )
            .inc();
        }
        AppEvent::OrderSent {
            module,
            mode,
            latency_ms: Some(ms),
            ..
        } => {
            reg.histogram(
                "bot_execution_latency_ms",
                "End-to-end order execution latency in milliseconds.",
                &[("module", module.as_str()), ("mode", sanitize_mode(mode))],
                LATENCY_BUCKETS_MS,
            )
            .observe(*ms);
        }
        AppEvent::WalletTrade { .. } => {
            reg.counter(
                "bot_whale_trades_total",
                "Tracked-wallet trades decoded by the copy module.",
                &[],
            )
            .inc();
        }
        AppEvent::Polymarket { .. } => {
            reg.counter(
                "bot_polymarket_events_total",
                "Polymarket market events surfaced by module 3.",
                &[],
            )
            .inc();
        }
        AppEvent::Command { accepted, .. } => {
            reg.counter(
                "bot_telegram_commands_total",
                "Telegram control commands received.",
                &[("accepted", if *accepted { "true" } else { "false" })],
            )
            .inc();
        }
        AppEvent::Error { module, fatal, .. } => {
            reg.counter(
                "bot_app_errors_total",
                "Application errors published on the event bus.",
                &[
                    (
                        "module",
                        module.as_ref().map(BotModule::as_str).unwrap_or("none"),
                    ),
                    ("fatal", if *fatal { "true" } else { "false" }),
                ],
            )
            .inc();
        }
        // Lifecycle / ModuleStatus / Signal / Fill / PositionUpdate /
        // PositionClosed / Info: covered by the state sampler or not
        // metric-worthy. Deliberately not counted to keep series bounded.
        _ => {}
    }
}

/// Subscribe to the event bus and feed every event through [`record_event`].
/// Runs until the bus closes (process shutdown). Lagged consumers are counted
/// (`bot_events_dropped_total`) instead of silently losing data.
pub fn spawn_event_pump(shared: Shared, reg: &'static Registry) {
    tokio::spawn(async move {
        let mut rx = shared.events.subscribe();
        let dropped = reg.counter(
            "bot_events_dropped_total",
            "Events lost because the metrics pump could not keep up.",
            &[],
        );
        loop {
            match rx.recv().await {
                Ok(ev) => record_event(reg, &ev),
                Err(broadcast::error::RecvError::Lagged(n)) => {
                    dropped.inc_by(n);
                    tracing::warn!(dropped = n, "event pump lagged");
                }
                Err(broadcast::error::RecvError::Closed) => break,
            }
        }
    });
}

// ---------------------------------------------------------------------------
// State sampler: AppState/Rpc -> gauges + health components
// ---------------------------------------------------------------------------

/// One sampling pass: refresh process/module gauges, mirror authoritative
/// module counters, and recompute every health component. Cheap (a few dozen
/// atomic stores plus one state snapshot); safe to call on a timer.
pub async fn sample_once(shared: &Shared, rpc: &Rpc, health: &HealthRegistry, reg: &Registry) {
    let summary = shared.summary().await;

    // TASK 6 — the worker's own readiness verdict is a first-class health
    // component: a worker that lost a required lease, has not finished
    // recovery or sits in a non-serving state must NEVER report READY, even
    // when every dependency below is green.
    let verdict = shared.ha().refresh_readiness().await;
    health.set(
        "worker",
        if verdict.ready {
            ComponentStatus::ok(verdict.detail())
        } else {
            ComponentStatus::not_ready(verdict.detail())
        },
    );

    reg.gauge(
        "bot_build_info",
        "Build information; value is always 1.",
        &[("version", env!("CARGO_PKG_VERSION"))],
    )
    .set(1);
    reg.gauge("bot_uptime_seconds", "Seconds since process start.", &[])
        .set(
            (chrono::Utc::now() - summary.started_at)
                .num_seconds()
                .max(0),
        );
    reg.gauge(
        "bot_kill_switch",
        "1 when the kill switch is engaged, 0 otherwise.",
        &[],
    )
    .set(i64::from(summary.kill_switch));
    reg.gauge("bot_open_positions", "Currently open positions.", &[])
        .set(summary.open_positions as i64);
    reg.gauge(
        "bot_event_subscribers",
        "Live event-bus subscribers (pumps, websockets).",
        &[],
    )
    .set(summary.event_subscribers as i64);
    reg.gauge(
        "bot_execution_mode",
        "Execution mode: 0=paper, 1=simulate, 2=live.",
        &[],
    )
    .set(match summary.execution_mode {
        bot_core::models::ExecutionMode::Paper => 0,
        bot_core::models::ExecutionMode::Simulate => 1,
        bot_core::models::ExecutionMode::Live => 2,
    });

    for ms in &summary.modules {
        let m = ms.module.as_str();
        let labels = [("module", m)];
        reg.gauge("bot_module_enabled", "Module enabled by operator.", &labels)
            .set(i64::from(ms.enabled));
        reg.gauge("bot_module_running", "Module task loop running.", &labels)
            .set(i64::from(ms.running));
        reg.gauge("bot_module_connected", "Module reports connected.", &labels)
            .set(i64::from(ms.connected));
        reg.gauge(
            "bot_module_healthy",
            "Module running and not degraded.",
            &labels,
        )
        .set(i64::from(ms.healthy));
        reg.gauge(
            "bot_module_consecutive_errors",
            "Consecutive errors recorded by the module.",
            &labels,
        )
        .set(ms.consecutive_errors as i64);

        // Counter::set mirrors the authoritative cumulative counters kept in
        // AppState (guaranteed non-decreasing), so restarts of the *sampler*
        // can never double count.
        reg.counter(
            "bot_module_events_seen_total",
            "Events seen by the module (mirrors AppState).",
            &labels,
        )
        .set(ms.events_seen);
        reg.counter(
            "bot_module_signals_total",
            "Signals generated by the module (mirrors AppState).",
            &labels,
        )
        .set(ms.signals_generated);
        reg.counter(
            "bot_module_orders_sent_total",
            "Orders sent by the module (mirrors AppState).",
            &labels,
        )
        .set(ms.orders_sent);
        reg.counter(
            "bot_module_orders_filled_total",
            "Orders filled by the module (mirrors AppState).",
            &labels,
        )
        .set(ms.orders_filled);
        reg.counter(
            "bot_module_orders_failed_total",
            "Orders failed by the module (mirrors AppState).",
            &labels,
        )
        .set(ms.orders_failed);
        reg.counter(
            "bot_module_risk_rejections_total",
            "Signals rejected by the risk engine (mirrors AppState).",
            &labels,
        )
        .set(ms.orders_rejected_by_risk);
    }

    let rpc_failures = rpc.failure_count();
    reg.gauge(
        "bot_rpc_consecutive_failures",
        "Consecutive primary-RPC failures (failover signal).",
        &[],
    )
    .set(rpc_failures as i64);

    // ---- health components ------------------------------------------------
    // RPC: ready iff below the failover threshold. Detail carries the count
    // only — never the error text.
    health.set(
        "rpc",
        if rpc.unhealthy() {
            ComponentStatus::not_ready(format!("consecutive_failures={rpc_failures}"))
        } else {
            ComponentStatus::ok(format!("consecutive_failures={rpc_failures}"))
        },
    );

    // Trading modules gate readiness. Telegram/Contract do not: telegram never
    // heartbeats, and the contract module is on-chain (not a server task).
    for ms in &summary.modules {
        if !matches!(
            ms.module,
            BotModule::Sniper | BotModule::Copy | BotModule::Polymarket
        ) {
            continue;
        }
        health.set(ms.module.as_str(), module_component(ms));
    }

    reg.gauge(
        "bot_health_ready",
        "1 when the readiness probe would pass, 0 otherwise.",
        &[],
    )
    .set(i64::from(health.ready()));
}

/// Run [`sample_once`] on a fixed interval until the process exits.
pub fn spawn_state_sampler(
    shared: Shared,
    rpc: Rpc,
    health: Arc<HealthRegistry>,
    reg: &'static Registry,
    interval: Duration,
) {
    tokio::spawn(async move {
        let interval = interval.max(Duration::from_millis(100));
        loop {
            sample_once(&shared, &rpc, &health, reg).await;
            tokio::time::sleep(interval).await;
        }
    });
}

/// Compute a trading module's health component from its status snapshot.
/// Pure so staleness can be unit-tested without waiting on a real clock.
fn module_component(ms: &bot_core::models::ModuleStatus) -> ComponentStatus {
    if !ms.enabled {
        return ComponentStatus::ok("disabled");
    }
    let hb_age = ms
        .last_heartbeat
        .map(|h| (chrono::Utc::now() - h).num_seconds());
    let fresh = hb_age.is_some_and(|a| (0..=HEARTBEAT_FRESH_SECS).contains(&a));
    if ms.running && fresh {
        ComponentStatus::ok(format!(
            "events_seen={} orders_sent={} heartbeat_age_secs={}",
            ms.events_seen,
            ms.orders_sent,
            hb_age.unwrap_or(-1)
        ))
    } else {
        // healthy tracks the module's own flag; ready is false until the
        // loop runs and heartbeats again.
        ComponentStatus {
            healthy: ms.healthy,
            ready: false,
            detail: Some(format!(
                "running={} heartbeat_age_secs={}",
                ms.running,
                hb_age
                    .map(|a| a.to_string())
                    .unwrap_or_else(|| "none".into())
            )),
        }
    }
}

/// Background health + gauges for the OPTIONAL persistence backends.
///
/// Policy: when a backend is configured `required`, its outage makes the
/// process not-ready (orchestrators should act); when optional, the outage
/// is `healthy=false, ready=true` — degraded but serving (the bot keeps
/// trading from memory/JSONL while operators investigate).
pub fn spawn_persistence_sampler(
    db: Option<std::sync::Arc<Database>>,
    db_required: bool,
    redis: Option<RedisKv>,
    redis_required: bool,
    health: Arc<HealthRegistry>,
    reg: &'static Registry,
    interval: Duration,
) {
    if db.is_none() && redis.is_none() {
        return;
    }
    tokio::spawn(async move {
        let interval = interval.max(Duration::from_millis(500));
        loop {
            if let Some(db) = &db {
                match db.ping().await {
                    Ok(()) => {
                        let stats = db.pool_stats().await;
                        health.set(
                            "postgres",
                            ComponentStatus::ok(format!(
                                "pool active={} idle={} max={}",
                                stats.active, stats.idle, stats.max
                            )),
                        );
                        reg.gauge(
                            "bot_db_pool_active",
                            "Active PostgreSQL pool connections.",
                            &[],
                        )
                        .set(stats.active as i64);
                        reg.gauge("bot_db_pool_idle", "Idle PostgreSQL pool connections.", &[])
                            .set(stats.idle as i64);
                    }
                    Err(e) => {
                        let detail = format!("ping failed: {e}");
                        health.set(
                            "postgres",
                            if db_required {
                                ComponentStatus::not_ready(detail)
                            } else {
                                ComponentStatus {
                                    healthy: false,
                                    ready: true,
                                    detail: Some(detail),
                                }
                            },
                        );
                    }
                }
            }
            if let Some(redis) = &redis {
                match redis.ping().await {
                    Ok(()) => health.set("redis", ComponentStatus::ok("pong")),
                    Err(e) => {
                        let detail = format!("ping failed: {e}");
                        health.set(
                            "redis",
                            if redis_required {
                                ComponentStatus::not_ready(detail)
                            } else {
                                ComponentStatus {
                                    healthy: false,
                                    ready: true,
                                    detail: Some(detail),
                                }
                            },
                        );
                    }
                }
            }
            tokio::time::sleep(interval).await;
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use bot_core::config::AppConfig;
    use bot_core::events::AppEvent;
    use bot_core::models::TokenLaunch;
    use bot_core::state::AppState;
    use solana_sdk::commitment_config::CommitmentConfig;

    fn launch(mint: &str) -> TokenLaunch {
        TokenLaunch {
            mint: mint.into(),
            name: "Test".into(),
            symbol: "TST".into(),
            uri: None,
            creator: "creator".into(),
            pool: "pool".into(),
            initial_buy_sol: 1.0,
            market_cap_sol: 10.0,
            market_cap_usd: None,
            total_supply: None,
            slot: None,
            signature: None,
            tx_type: None,
            observed_at: chrono::Utc::now(),
            feed: bot_core::models::LaunchFeed::Manual,
            socials: None,
        }
    }

    fn order_sent(module: BotModule, mode: &str, latency_ms: Option<u64>) -> AppEvent {
        AppEvent::OrderSent {
            ts: chrono::Utc::now(),
            module,
            symbol: "TST".into(),
            venue: "pumpfun".into(),
            mode: mode.into(),
            quote_amount: 0.5,
            signer: None,
            attempts: None,
            signature: None,
            latency_ms,
        }
    }

    #[test]
    fn record_event_counts_launches_commands_and_errors() {
        let reg = Registry::new();
        record_event(
            &reg,
            &AppEvent::Launch {
                ts: chrono::Utc::now(),
                launch: Box::new(launch("mint1")),
                accepted: true,
                reason: None,
            },
        );
        record_event(
            &reg,
            &AppEvent::Launch {
                ts: chrono::Utc::now(),
                launch: Box::new(launch("mint2")),
                accepted: false,
                reason: Some("liq".into()),
            },
        );
        record_event(
            &reg,
            &AppEvent::Command {
                ts: chrono::Utc::now(),
                chat_id: 1,
                user_id: None,
                text: "/status".into(),
                accepted: true,
                response: None,
            },
        );
        record_event(
            &reg,
            &AppEvent::Error {
                ts: chrono::Utc::now(),
                module: Some(BotModule::Sniper),
                message: "boom".into(),
                fatal: false,
            },
        );
        record_event(
            &reg,
            &AppEvent::Error {
                ts: chrono::Utc::now(),
                module: None,
                message: "boom".into(),
                fatal: true,
            },
        );

        let text = reg.encode();
        assert!(text.contains("bot_launches_total{accepted=\"true\"} 1"));
        assert!(text.contains("bot_launches_total{accepted=\"false\"} 1"));
        assert!(text.contains("bot_telegram_commands_total{accepted=\"true\"} 1"));
        assert!(text.contains("bot_app_errors_total{fatal=\"false\",module=\"sniper\"} 1"));
        assert!(text.contains("bot_app_errors_total{fatal=\"true\",module=\"none\"} 1"));
        // The error *message* must never reach the exposition format.
        assert!(!text.contains("boom"));
    }

    #[test]
    fn record_event_observes_execution_latency_and_sanitises_mode() {
        let reg = Registry::new();
        record_event(&reg, &order_sent(BotModule::Sniper, "live", Some(42)));
        record_event(&reg, &order_sent(BotModule::Sniper, "weird", Some(7)));
        record_event(&reg, &order_sent(BotModule::Copy, "paper", None));

        let text = reg.encode();
        assert!(text.contains("bot_execution_latency_ms_count{mode=\"live\",module=\"sniper\"} 1"));
        assert!(text.contains("bot_execution_latency_ms_sum{mode=\"live\",module=\"sniper\"} 42"));
        assert!(
            text.contains("bot_execution_latency_ms_count{mode=\"other\",module=\"sniper\"} 1"),
            "unknown modes collapse to 'other': {text}"
        );
        assert!(
            !text.contains("weird"),
            "raw unknown mode string must not become a label"
        );
        // latency None => no observation for copy.
        assert!(!text.contains("module=\"copy\""));
    }

    #[test]
    fn request_id_honours_safe_inbound_and_rejects_everything_else() {
        let shared = AppState::new(AppConfig::from_defaults());
        let mut h = HeaderMap::new();

        // Missing => generated.
        let id = request_id(&h, &shared);
        assert!(id.starts_with("req-"), "generated ids are prefixed: {id}");

        // Safe inbound id is honoured verbatim.
        h.insert("x-request-id", "abc-123_XYZ".parse().unwrap());
        assert_eq!(request_id(&h, &shared), "abc-123_XYZ");

        // Overlong (129 chars) => replaced.
        h.insert("x-request-id", "a".repeat(129).parse().unwrap());
        assert!(request_id(&h, &shared).starts_with("req-"));

        // Exactly 128 chars => honoured.
        h.insert("x-request-id", "a".repeat(128).parse().unwrap());
        assert_eq!(request_id(&h, &shared), "a".repeat(128));

        // Control chars / quotes / newlines => replaced.
        for bad in ["a\"b", "a\nb", "a b", "<script>", "a\\b"] {
            // Header values cannot hold raw newlines; use the char-safe subset.
            if bad.contains('\n') {
                continue;
            }
            h.insert("x-request-id", bad.parse().unwrap());
            assert!(
                request_id(&h, &shared).starts_with("req-"),
                "{bad:?} must be rejected"
            );
        }
    }

    fn test_rpc() -> Rpc {
        // Construction performs no network I/O.
        Rpc::with_urls(
            "http://127.0.0.1:1".into(),
            String::new(),
            Vec::new(),
            CommitmentConfig::confirmed(),
            1,
            Duration::from_millis(50),
        )
        .expect("rpc builds offline")
    }

    /// TASK 6: `sample_once` publishes the worker's own readiness as a
    /// health component, so a state that never registered or recovered is
    /// deliberately NOT ready. These probe tests are about the OTHER
    /// components, so bring the worker to READY the way the server does.
    async fn ready_worker(shared: &bot_core::state::Shared) {
        shared.ha().register("test", 1, "0.1.0").await.unwrap();
        shared
            .ha()
            .set_state(bot_core::ha::WorkerState::Recovering, "test")
            .await
            .unwrap();
        shared.ha().set_recovery_complete(true).await;
        shared
            .ha()
            .set_state(bot_core::ha::WorkerState::Ready, "test")
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn sample_once_mirrors_state_into_metrics_and_health() {
        let shared = AppState::new(AppConfig::from_defaults());
        ready_worker(&shared).await;
        let rpc = test_rpc();
        let health = HealthRegistry::new();
        let reg = Registry::new();

        sample_once(&shared, &rpc, &health, &reg).await;

        let text = reg.encode();
        assert!(text.contains("bot_build_info{version="));
        assert!(text.contains("bot_uptime_seconds "));
        assert!(text.contains("bot_kill_switch 0"));
        assert!(text.contains("bot_open_positions 0"));
        assert!(text.contains("bot_execution_mode 0"), "paper = 0: {text}");
        assert!(text.contains("bot_module_enabled{module=\"sniper\"} "));
        assert!(text.contains("bot_module_orders_sent_total{module=\"copy\"} "));
        assert!(text.contains("bot_rpc_consecutive_failures 0"));
        assert!(text.contains("bot_health_ready "));

        let snap = health.snapshot();
        let names: Vec<&str> = snap.components.iter().map(|c| c.name.as_str()).collect();
        assert!(names.contains(&"rpc"), "{names:?}");
        assert!(names.contains(&"sniper"), "{names:?}");
        // Default config: modules disabled => ready; fresh rpc => ready.
        assert!(snap.ready, "defaults must be ready: {snap:?}");
    }

    #[tokio::test]
    async fn sample_once_marks_enabled_but_stopped_module_not_ready() {
        let shared = AppState::new(AppConfig::from_defaults());
        ready_worker(&shared).await;
        shared.set_enabled(BotModule::Sniper, true).await;
        // Enabled but the task loop never started: running=false, no heartbeat.

        let rpc = test_rpc();
        let health = HealthRegistry::new();
        let reg = Registry::new();
        sample_once(&shared, &rpc, &health, &reg).await;

        let snap = health.snapshot();
        let sniper = snap
            .components
            .iter()
            .find(|c| c.name == "sniper")
            .expect("sniper component");
        assert!(!sniper.status.ready, "enabled but stopped => not ready");
        assert!(
            sniper
                .status
                .detail
                .as_deref()
                .unwrap_or("")
                .contains("running=false"),
            "{:?}",
            sniper.status.detail
        );
        assert!(!snap.ready);
        assert!(reg.encode().contains("bot_health_ready 0"));

        // Starting the loop (set_running also stamps a heartbeat) flips it
        // to ready on the next sample.
        shared.set_running(BotModule::Sniper, true, true).await;
        sample_once(&shared, &rpc, &health, &reg).await;
        let snap = health.snapshot();
        assert!(snap.ready, "running + fresh heartbeat => ready: {snap:?}");
        assert!(reg.encode().contains("bot_health_ready 1"));
    }

    /// TASK 6 §11: a worker that has not finished recovery (or lost a
    /// required lease) must never report READY, however healthy the
    /// dependencies are.
    #[tokio::test]
    async fn sample_once_reports_the_worker_component_and_gates_readiness() {
        let shared = AppState::new(AppConfig::from_defaults());
        let rpc = test_rpc();
        let health = HealthRegistry::new();
        let reg = Registry::new();

        // Not registered, recovery not complete => not ready.
        sample_once(&shared, &rpc, &health, &reg).await;
        let snap = health.snapshot();
        let worker = snap
            .components
            .iter()
            .find(|c| c.name == "worker")
            .expect("worker component");
        assert!(!worker.status.ready, "{:?}", worker.status.detail);
        assert!(!snap.ready);
        // The `ha_*` family lives in the process-wide registry (the HA
        // runtime publishes it directly), not in this test's local one.
        assert!(bot_core::obs::metrics::global()
            .encode()
            .contains("ha_readiness "));

        // After registration + recovery the component turns ready.
        ready_worker(&shared).await;
        sample_once(&shared, &rpc, &health, &reg).await;
        let snap = health.snapshot();
        let worker = snap
            .components
            .iter()
            .find(|c| c.name == "worker")
            .expect("worker component");
        assert!(worker.status.ready, "{:?}", worker.status.detail);
        assert!(snap.ready, "{snap:?}");
        assert!(bot_core::obs::metrics::global()
            .encode()
            .contains("ha_readiness 1"));
    }

    fn module_status(
        module: BotModule,
        enabled: bool,
        running: bool,
        heartbeat_age: Option<chrono::Duration>,
    ) -> bot_core::models::ModuleStatus {
        bot_core::models::ModuleStatus {
            module,
            enabled,
            running,
            healthy: running,
            connected: running,
            events_seen: 3,
            signals_generated: 2,
            orders_sent: 1,
            orders_filled: 1,
            orders_failed: 0,
            orders_rejected_by_risk: 0,
            realized_pnl: 0.0,
            unrealized_pnl: 0.0,
            consecutive_errors: 0,
            last_error: None,
            last_heartbeat: heartbeat_age.map(|a| chrono::Utc::now() - a),
            last_event: None,
            detail: None,
        }
    }

    #[test]
    fn module_component_readiness_rules() {
        use chrono::Duration as CD;

        // Disabled => ready, never blocks startup.
        let c = module_component(&module_status(BotModule::Sniper, false, false, None));
        assert!(c.ready && c.healthy);
        assert_eq!(c.detail.as_deref(), Some("disabled"));

        // Enabled, running, fresh heartbeat => ready.
        let c = module_component(&module_status(
            BotModule::Sniper,
            true,
            true,
            Some(CD::seconds(5)),
        ));
        assert!(c.ready && c.healthy, "{c:?}");
        assert!(c.detail.unwrap().contains("heartbeat_age_secs=5"));

        // Enabled, running, STALE heartbeat (past the window) => not ready.
        let c = module_component(&module_status(
            BotModule::Copy,
            true,
            true,
            Some(CD::seconds(HEARTBEAT_FRESH_SECS + 30)),
        ));
        assert!(!c.ready, "stale heartbeat must fail readiness: {c:?}");

        // Boundary: exactly at the window edge is still fresh.
        let c = module_component(&module_status(
            BotModule::Copy,
            true,
            true,
            Some(CD::seconds(HEARTBEAT_FRESH_SECS - 1)),
        ));
        assert!(c.ready, "{c:?}");

        // Enabled, not running => not ready.
        let c = module_component(&module_status(BotModule::Polymarket, true, false, None));
        assert!(!c.ready);
        assert!(c.detail.unwrap().contains("heartbeat_age_secs=none"));
    }

    #[test]
    fn sanitize_mode_is_closed_set() {
        assert_eq!(sanitize_mode("paper"), "paper");
        assert_eq!(sanitize_mode("simulate"), "simulate");
        assert_eq!(sanitize_mode("live"), "live");
        assert_eq!(sanitize_mode("LIVE"), "other");
        assert_eq!(sanitize_mode(""), "other");
        assert_eq!(sanitize_mode("../../../etc"), "other");
    }
}
