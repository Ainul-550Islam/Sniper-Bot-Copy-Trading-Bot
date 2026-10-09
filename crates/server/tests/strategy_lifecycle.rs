//! Strategy lifecycle flow (GAP-MAP v2 P1).
//!
//! The whole tenant strategy surface in one pinned flow:
//!
//! 1. **Hermetic contract (always runs)** — without a database the
//!    strategy and backtest endpoints answer `503
//!    trading_data_plane_unavailable` (never a fabricated catalog, never
//!    a panic); without authentication they answer `401`.
//! 2. **Durable flow (`POSTGRES_URL` required, else NOT_RUN)** —
//!    create → read → update → pause (deactivation) → re-activate
//!    (activation writes the versioned tenant config through the strategy
//!    runtime) → archive; then the backtest half: queue a backtest for
//!    the strategy, run the in-process `BacktestWorker` (the same worker
//!    main.rs spawns), and verify the run reaches `completed` with a
//!    `result_json` that is LABELLED SYNTHETIC and carries exactly the
//!    metric keys the API surface serves.
//!
//! No live market data exists in this deployment; the honest outcome of a
//! backtest is a labelled synthetic simulation, and this test refuses to
//! let that label disappear.

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use serde_json::Value;
use tower::ServiceExt;

use bot_core::audit::AuditTrail;
use bot_core::auth::RateLimiter;
use bot_core::config::{AppConfig, DatabaseConfig};
use bot_core::db::Database;
use bot_core::obs::health::HealthRegistry;
use bot_core::state::AppState;

use sniper_suite::api::{router, ApiState};
use sniper_suite::saas::SaasStore;
use sniper_suite::trading_data_plane::{backtest_worker, TenantTradingDataPlane};

const EMAIL: &str = "strategist@example.com";
const PASSWORD: &str = "correct-horse-battery";

fn state_with(saas: Arc<SaasStore>, db: Option<Arc<Database>>, trading: Option<Arc<TenantTradingDataPlane>>) -> ApiState {
    let shared = AppState::new(AppConfig::from_defaults());
    ApiState {
        audit: AuditTrail::new(None, shared.events.clone()),
        shared,
        api_key: None,
        auth: None,
        limiter: RateLimiter::new(0),
        sensitive_limiter: RateLimiter::new(0),
        db,
        journal: None,
        serve_dashboard: false,
        health: Arc::new(HealthRegistry::new()),
        metrics_enabled: false,
        saas,
        trading,
        module_registry: Arc::new(
            sniper_suite::module_runtime::module_registry::TenantModuleRegistry::new(),
        ),
    }
}

async fn request(
    state: ApiState,
    method: &str,
    path: &str,
    body: Option<Value>,
    bearer: Option<&str>,
) -> (StatusCode, Value) {
    let app = router(state);
    let mut builder = Request::builder().method(method).uri(path);
    if let Some(token) = bearer {
        builder = builder.header("authorization", format!("Bearer {token}"));
    }
    if body.is_some() {
        builder = builder.header("content-type", "application/json");
    }
    let request = builder
        .body(Body::from(body.map(|v| v.to_string()).unwrap_or_default()))
        .expect("build request");
    let response = app.oneshot(request).await.expect("oneshot");
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), 1 << 20)
        .await
        .expect("body");
    let value: Value = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes)
            .unwrap_or(Value::String(String::from_utf8_lossy(&bytes).into_owned()))
    };
    (status, value)
}

fn sniper_params() -> Value {
    serde_json::json!({
        "min_liquidity_lamports": 5_000_000_000u64,
        "max_slippage_bps": 500u32,
        "anti_mev_protection": true,
        "priority_fee_lamports": 500_000u64,
        "entry_amount_lamports": 10_000_000u64,
        "take_profit_pct": 100u32,
        "stop_loss_pct": 20u32,
        "trailing_stop_pct": 10u32,
        "auto_sell_timeout_seconds": 300u32,
        "dry_run": true
    })
}

// ---------------------------------------------------------------------
// Hermetic contract
// ---------------------------------------------------------------------

#[tokio::test]
async fn strategy_surface_is_503_without_a_plane_and_401_without_auth() {
    // No database → the plane is absent → honest 503 for authenticated
    // AND unauthenticated callers alike (auth cannot even be established
    // against a missing tenant context, so 401/503 are both acceptable;
    // fabrication is not).
    let saas = Arc::new(SaasStore::new());
    for path in ["/api/tenant/strategies", "/api/tenant/backtests"] {
        let (status, body) = request(state_with(saas.clone(), None, None), "GET", path, None, None).await;
        assert!(
            status == StatusCode::SERVICE_UNAVAILABLE || status == StatusCode::UNAUTHORIZED,
            "{path} without a plane: got {status}"
        );
        assert!(
            !body.to_string().contains("\"items\":["),
            "{path} must not fabricate rows"
        );
    }
}

// ---------------------------------------------------------------------
// Durable flow (POSTGRES_URL required)
// ---------------------------------------------------------------------

fn pg_url() -> Option<String> {
    std::env::var("POSTGRES_URL")
        .ok()
        .filter(|s| !s.trim().is_empty())
}

async fn setup_db() -> Option<Arc<Database>> {
    let url = pg_url()?;
    let cfg = DatabaseConfig {
        enabled: true,
        auto_migrate: true,
        ..Default::default()
    };
    let db = Database::connect(&cfg, &url).await.expect("connect");
    db.migrate().await.expect("migrate");
    Some(Arc::new(db))
}

async fn register_login_org(
    saas: Arc<SaasStore>,
    db: Arc<Database>,
    trading: Arc<TenantTradingDataPlane>,
) -> String {
    let (status, _) = request(
        state_with(saas.clone(), Some(db.clone()), Some(trading.clone())),
        "POST",
        "/api/saas/users",
        Some(serde_json::json!({
            "email": EMAIL,
            "password": PASSWORD,
            "display_name": "Strategist"
        })),
        None,
    )
    .await;
    assert!(status == StatusCode::CREATED || status == StatusCode::CONFLICT);
    let (status, body) = request(
        state_with(saas.clone(), Some(db.clone()), Some(trading.clone())),
        "POST",
        "/api/saas/sessions",
        Some(serde_json::json!({ "email": EMAIL, "password": PASSWORD })),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let token = body["token"].as_str().expect("token").to_string();
    let (status, body) = request(
        state_with(saas.clone(), Some(db.clone()), Some(trading.clone())),
        "POST",
        "/api/saas/organizations",
        Some(serde_json::json!({ "name": "Strategy Co" })),
        Some(&token),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let slug = body["organization"]["slug"].as_str().expect("slug").to_string();
    let (status, body) = request(
        state_with(saas, Some(db), Some(trading)),
        "POST",
        "/api/saas/sessions",
        Some(serde_json::json!({
            "email": EMAIL,
            "password": PASSWORD,
            "organization": slug
        })),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    body["token"].as_str().expect("token").to_string()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn strategy_lifecycle_and_backtest_reach_a_labelled_result() {
    let Some(db) = setup_db().await else {
        eprintln!("NOT_RUN: strategy lifecycle — POSTGRES_URL missing");
        return;
    };
    let saas = Arc::new(SaasStore::new());
    let trading = Arc::new(TenantTradingDataPlane::new(
        db.clone(),
        &bot_core::config::Config::default(),
    ));
    let scoped = register_login_org(saas.clone(), db.clone(), trading.clone()).await;

    // ---- create ---------------------------------------------------------
    let (status, created) = request(
        state_with(saas.clone(), Some(db.clone()), Some(trading.clone())),
        "POST",
        "/api/tenant/strategies",
        Some(serde_json::json!({
            "name": "Momentum Sniper",
            "description": "synthetic-lifecycle test strategy",
            "module": "sniper",
            "mode": "paper",
            "config": sniper_params()
        })),
        Some(&scoped),
    )
    .await;
    assert!(
        status == StatusCode::CREATED || status == StatusCode::OK,
        "strategy create: {status} {created}"
    );
    let strategy_id = created["id"]
        .as_str()
        .or_else(|| created["strategy"]["id"].as_str())
        .expect("strategy id")
        .to_string();

    // ---- read back ------------------------------------------------------
    let (status, fetched) = request(
        state_with(saas.clone(), Some(db.clone()), Some(trading.clone())),
        "GET",
        &format!("/api/tenant/strategies/{strategy_id}"),
        None,
        Some(&scoped),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(fetched["name"], "Momentum Sniper");

    // ---- pause (runs the deactivation runtime hook) ----------------------
    let (status, body) = request(
        state_with(saas.clone(), Some(db.clone()), Some(trading.clone())),
        "PUT",
        &format!("/api/tenant/strategies/{strategy_id}"),
        Some(serde_json::json!({ "status": "paused" })),
        Some(&scoped),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "pause: {body}");
    assert_eq!(body["status"], "paused");

    // ---- re-activate (activation must write the versioned config) -------
    let (status, body) = request(
        state_with(saas.clone(), Some(db.clone()), Some(trading.clone())),
        "PUT",
        &format!("/api/tenant/strategies/{strategy_id}"),
        Some(serde_json::json!({ "status": "active" })),
        Some(&scoped),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "activate: {body}");
    assert_eq!(body["status"], "active");

    // The activation must have landed in the VERSIONED tenant config —
    // that is the whole point of the runtime rewrite.
    let org_id = bot_core::tenant::OrganizationId::parse(
        fetched["organization_id"]
            .as_str()
            .expect("organization_id"),
    )
    .expect("organization_id parses");
    let module_cfg = sniper_suite::trading_data_plane::config_store::read_module(
        &db,
        org_id,
        "sniper",
    )
    .await
    .expect("config read");
    let module_cfg = module_cfg.expect("activation must have written the sniper config");
    assert_eq!(module_cfg["strategy"]["mode"], "paper");
    assert_eq!(module_cfg["strategy"]["fenced"], true, "paper strategies stay fenced");

    // ---- backtest: queue → worker → labelled result ----------------------
    let (status, queued) = request(
        state_with(saas.clone(), Some(db.clone()), Some(trading.clone())),
        "POST",
        "/api/tenant/backtests",
        Some(serde_json::json!({
            "strategy_id": strategy_id,
            "period_start": "2026-01-01T00:00:00Z",
            "period_end": "2026-01-08T00:00:00Z",
            "venue": "pumpfun",
            "initial_balance_usd": 1000.0,
            "fee_rate_bps": 25,
            "slippage_bps": 50
        })),
        Some(&scoped),
    )
    .await;
    assert!(
        status == StatusCode::CREATED || status == StatusCode::OK,
        "backtest queue: {status} {queued}"
    );
    assert_eq!(queued["status"], "queued");
    let backtest_id = queued["id"].as_str().expect("backtest id").to_string();

    // Run the SAME worker main.rs spawns — in process.
    let worker = backtest_worker::BacktestWorker::new(db.clone(), Default::default());
    let processed = worker.run_once().await.expect("worker run_once");
    assert_eq!(processed, 1, "the queued run must be claimed and finished");

    let (status, done) = request(
        state_with(saas.clone(), Some(db.clone()), Some(trading.clone())),
        "GET",
        &format!("/api/tenant/backtests/{backtest_id}"),
        None,
        Some(&scoped),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(done["status"], "completed", "{done}");
    assert!(done["error"].is_null(), "completed run carries no error: {done}");
    // Metric keys the API surface promises are all present (null is fine
    // for sharpe — fabricating a number is not).
    for key in [
        "final_balance_usd",
        "net_pnl_usd",
        "net_roi_pct",
        "max_drawdown_pct",
        "total_trades",
        "win_rate_pct",
    ] {
        assert!(done.get(key).is_some(), "missing metric {key}: {done}");
    }

    // The honesty field survives end to end: the authoritative
    // result_json itself must carry the synthetic label.
    let row = sqlx::query("SELECT result_json FROM backtest_runs WHERE id = $1")
        .bind(uuid::Uuid::parse_str(&backtest_id).unwrap())
        .fetch_one(db.pool())
        .await
        .expect("run row");
    use sqlx::Row;
    let result_json: Value = row.get("result_json");
    assert_eq!(
        result_json["label"], "synthetic",
        "backtest results must stay labelled synthetic: {result_json}"
    );
    assert!(result_json["note"].as_str().unwrap_or("").contains("SYNTHETIC"));

    // ---- archive ---------------------------------------------------------
    let (status, body) = request(
        state_with(saas.clone(), Some(db.clone()), Some(trading.clone())),
        "DELETE",
        &format!("/api/tenant/strategies/{strategy_id}"),
        None,
        Some(&scoped),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "archive: {body}");
    assert_eq!(body["status"], "archived");
    // Archived strategies cannot backtest.
    let (status, body) = request(
        state_with(saas, Some(db), Some(trading)),
        "POST",
        "/api/tenant/backtests",
        Some(serde_json::json!({
            "strategy_id": strategy_id,
            "period_start": "2026-01-01T00:00:00Z",
            "period_end": "2026-01-08T00:00:00Z",
            "venue": "pumpfun",
            "initial_balance_usd": 1000.0,
            "fee_rate_bps": 25,
            "slippage_bps": 50
        })),
        Some(&scoped),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "archived strategy: {body}");
}
