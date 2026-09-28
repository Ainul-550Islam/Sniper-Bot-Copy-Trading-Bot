//! Shared harness for the Polymarket engine integration tests (TASK 4).
//!
//! A scriptable mock venue (CLOB REST + Gamma + a Polygon JSON-RPC stub for
//! the collateral reader) runs on an ephemeral loopback port so the REAL
//! `PolyBot` — staged pipeline, OMS idempotency, the shared risk engine,
//! EIP-712 signing, L2-authenticated posting, status polling, user-channel
//! events, cancel/replace, reconciliation and restart recovery — is
//! exercised end to end without any external network. Everything here is
//! test-only code; nothing in `src/` depends on it.

#![allow(dead_code)]
#![allow(unused_imports)]

use std::collections::{BTreeMap, HashMap};
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

use axum::{
    extract::{Path, RawQuery, State},
    http::{HeaderMap, Method, StatusCode, Uri},
    response::IntoResponse,
    routing::{delete, get, post},
    Json, Router,
};
use chrono::{DateTime, Duration, Utc};
use k256::ecdsa::SigningKey;
use serde_json::{json, Value};
use tokio::net::TcpListener;

use bot_core::config::{AppConfig, Config, PolymarketConfig};
use bot_core::events::AppEvent;
use bot_core::models::{ExecutionMode, PolyMarket, PolyOutcome};
use bot_core::oms::OrderManager;
use bot_core::state::{AppState, Shared};

use module_polymarket::auth::ApiKey;
use module_polymarket::eip712::address_from_signing_key;
use module_polymarket::orders::{sign_order_bundle, OrderParams, OrderSignal, SignedOrderBundle};
use module_polymarket::store::MemoryPolyStore;
use module_polymarket::strategy::{OrderDecision, Quote};
use module_polymarket::PolyBot;

/// Condition id of the fixture market.
pub const CONDITION: &str = "0x1111222233334444555566667777888899990000aaaabbbbccccddddeeeeaaaa";
/// Outcome token ids of the fixture market.
pub const YES: &str = "111";
pub const NO: &str = "222";
/// A second market for multi-market tests.
pub const CONDITION_B: &str = "0xbbbb222233334444555566667777888899990000aaaabbbbccccddddeeeebbbb";
pub const YES_B: &str = "333";
pub const NO_B: &str = "444";

/// Everything the mock remembers about a request.
#[derive(Debug, Clone)]
pub struct Captured {
    pub method: String,
    pub path: String,
    pub query: Option<String>,
    pub poly_headers: BTreeMap<String, String>,
    pub body: String,
}

/// How `POST /order` answers.
#[derive(Debug, Clone)]
pub enum PostBehaviour {
    /// `{"success": true, "status": <status>}` — `live` rests, `matched`
    /// fills at once.
    Accept { status: String },
    /// `{"success": false, "errorMsg": <msg>}` — definite venue rejection.
    Reject { msg: String },
    /// HTTP 500 — transport-level ambiguity (SubmitUnknown).
    ServerError,
}

/// How `DELETE /order` answers.
#[derive(Debug, Clone)]
pub enum CancelBehaviour {
    /// Confirms whatever id is asked for.
    Confirm,
    /// Refuses with a reason (the order stays open on the venue).
    Refuse { reason: String },
    /// Answers `{}` — a shape that names neither list. The order is left
    /// untouched on the venue (whatever `set_order` scripted stays).
    Unrecognised,
}

/// Scriptable venue state shared with the axum handlers.
#[derive(Default)]
pub struct VenueScript {
    pub captures: Vec<Captured>,
    pub post: Option<PostBehaviour>,
    pub cancel: Option<CancelBehaviour>,
    /// `GET /data/order?order_id=` answers (missing = 404).
    pub orders: HashMap<String, Value>,
    /// `GET /data/orders` answer.
    pub open_orders: Vec<Value>,
    /// `GET /data/trades` answer.
    pub trades: Vec<Value>,
    /// Collateral stub: balance / allowance / decimals returned by `eth_call`.
    pub balance_raw: u128,
    pub allowance_raw: u128,
    pub decimals: u128,
    /// Fixture markets served by Gamma.
    pub markets: Vec<Value>,
    /// Order books served by `GET /book`.
    pub books: HashMap<String, (f64, f64)>,
}

#[derive(Clone)]
pub struct MockVenue {
    pub addr: SocketAddr,
    pub script: Arc<Mutex<VenueScript>>,
}

impl MockVenue {
    /// Base URL of the CLOB routes.
    pub fn clob_url(&self) -> String {
        format!("http://{}/clob", self.addr)
    }
    /// Base URL of the Gamma routes.
    pub fn gamma_url(&self) -> String {
        format!("http://{}/gamma", self.addr)
    }
    /// JSON-RPC endpoint for the collateral / CTF readers.
    pub fn rpc_url(&self) -> String {
        format!("http://{}/rpc", self.addr)
    }

    pub fn set_post(&self, b: PostBehaviour) {
        self.script.lock().unwrap().post = Some(b);
    }
    pub fn set_cancel(&self, b: CancelBehaviour) {
        self.script.lock().unwrap().cancel = Some(b);
    }
    pub fn set_balance(&self, balance_raw: u128, allowance_raw: u128) {
        let mut s = self.script.lock().unwrap();
        s.balance_raw = balance_raw;
        s.allowance_raw = allowance_raw;
    }
    /// Script the venue's view of one order (`GET /data/order`).
    pub fn set_order(&self, id: &str, status: &str, size_matched: f64, original: f64) {
        let v = venue_order(id, status, size_matched, original);
        self.script
            .lock()
            .unwrap()
            .orders
            .insert(id.to_ascii_lowercase(), v);
    }
    pub fn set_order_with_trades(
        &self,
        id: &str,
        status: &str,
        size_matched: f64,
        original: f64,
        trades: &[&str],
    ) {
        let v = venue_order_with_trades(id, status, size_matched, original, trades);
        self.script
            .lock()
            .unwrap()
            .orders
            .insert(id.to_ascii_lowercase(), v);
    }
    pub fn remove_order(&self, id: &str) {
        self.script
            .lock()
            .unwrap()
            .orders
            .remove(&id.to_ascii_lowercase());
    }
    pub fn set_open_orders(&self, orders: Vec<Value>) {
        self.script.lock().unwrap().open_orders = orders;
    }
    pub fn set_trades(&self, trades: Vec<Value>) {
        self.script.lock().unwrap().trades = trades;
    }
    pub fn captures(&self) -> Vec<Captured> {
        self.script.lock().unwrap().captures.clone()
    }
    pub fn count(&self, method: &str, path: &str) -> usize {
        self.captures()
            .iter()
            .filter(|c| c.method == method && c.path == path)
            .count()
    }
    pub fn last_body(&self, method: &str, path: &str) -> Option<Value> {
        self.captures()
            .iter()
            .rev()
            .find(|c| c.method == method && c.path == path)
            .and_then(|c| serde_json::from_str(&c.body).ok())
    }
    /// The venue order id the mock last saw on `POST /order` (from the
    /// signed order's salt/maker we cannot derive; the bot reports it in the
    /// outcome, so tests read it from there instead). Kept for symmetry.
    pub fn posted_orders(&self) -> usize {
        self.count("POST", "/clob/order")
    }
}

/// One venue-side order document in the CLOB's shape.
pub fn venue_order(id: &str, status: &str, size_matched: f64, original: f64) -> Value {
    venue_order_with_trades(id, status, size_matched, original, &[])
}

/// A venue order payload that names the trades behind `size_matched`
/// (`associate_trades`), as `GET /data/order` and the open list do.
pub fn venue_order_with_trades(
    id: &str,
    status: &str,
    size_matched: f64,
    original: f64,
    trades: &[&str],
) -> Value {
    json!({
        "id": id,
        "status": status,
        "market": CONDITION,
        "asset_id": YES,
        "side": "BUY",
        "price": "0.400",
        "original_size": format!("{original}"),
        "size_matched": format!("{size_matched}"),
        "order_type": "GTC",
        "expiration": "0",
        "associate_trades": trades,
        "owner": "mock-key",
        "created_at": 1700000000
    })
}

fn capture(
    script: &Arc<Mutex<VenueScript>>,
    method: &Method,
    uri: &Uri,
    headers: &HeaderMap,
    body: &str,
) {
    let mut poly_headers = BTreeMap::new();
    for (k, v) in headers.iter() {
        let name = k.as_str().to_ascii_uppercase();
        if name.starts_with("POLY_") {
            if let Ok(s) = v.to_str() {
                poly_headers.insert(name, s.to_string());
            }
        }
    }
    script.lock().unwrap().captures.push(Captured {
        method: method.to_string(),
        path: uri.path().to_string(),
        query: uri.query().map(String::from),
        poly_headers,
        body: body.to_string(),
    });
}

fn word(v: u128) -> String {
    format!("0x{v:064x}")
}

/// Default Gamma document for the fixture market.
pub fn gamma_market(condition: &str, yes: &str, no: &str) -> Value {
    json!({
        "conditionId": condition,
        "question": format!("Will market {} resolve Yes?", &condition[..6]),
        "slug": "mock-market",
        "negRisk": false,
        "active": true,
        "closed": false,
        "acceptingOrders": true,
        "endDate": "2030-01-01T00:00:00Z",
        "volume": "123456.78",
        "liquidity": "9876.5",
        "outcomes": "[\"Yes\",\"No\"]",
        "clobTokenIds": format!("[\"{yes}\",\"{no}\"]"),
        "outcomePrices": "[\"0.40\",\"0.55\"]"
    })
}

/// Start the mock venue. Default script: posts rest (`live`), cancels
/// confirm, balance 1 000 USDC (6 dp) with matching allowance, one fixture
/// market with books YES 0.39/0.40 and NO 0.54/0.55 (basket edge 0.05).
pub async fn mock_venue() -> MockVenue {
    let script = Arc::new(Mutex::new(VenueScript {
        post: Some(PostBehaviour::Accept {
            status: "live".into(),
        }),
        cancel: Some(CancelBehaviour::Confirm),
        balance_raw: 1_000_000_000,
        allowance_raw: 1_000_000_000,
        decimals: 6,
        markets: vec![gamma_market(CONDITION, YES, NO)],
        books: HashMap::from([
            (YES.to_string(), (0.39, 0.40)),
            (NO.to_string(), (0.54, 0.55)),
        ]),
        ..Default::default()
    }));

    type S = Arc<Mutex<VenueScript>>;

    async fn gamma_markets(
        State(st): State<S>,
        method: Method,
        uri: Uri,
        headers: HeaderMap,
        RawQuery(q): RawQuery,
    ) -> impl IntoResponse {
        capture(&st, &method, &uri, &headers, "");
        let markets = st.lock().unwrap().markets.clone();
        if let Some(q) = q {
            if let Some(cid) = q
                .split('&')
                .find_map(|kv| kv.strip_prefix("condition_ids="))
            {
                let filtered: Vec<Value> = markets
                    .into_iter()
                    .filter(|m| m["conditionId"].as_str() == Some(cid))
                    .collect();
                return Json(Value::Array(filtered));
            }
        }
        Json(Value::Array(markets))
    }

    async fn clob_time(
        State(st): State<S>,
        method: Method,
        uri: Uri,
        headers: HeaderMap,
    ) -> impl IntoResponse {
        capture(&st, &method, &uri, &headers, "");
        Json(Value::String("1700000123".into()))
    }

    async fn clob_book(
        State(st): State<S>,
        method: Method,
        uri: Uri,
        headers: HeaderMap,
        RawQuery(q): RawQuery,
    ) -> impl IntoResponse {
        capture(&st, &method, &uri, &headers, "");
        let token = q
            .as_deref()
            .and_then(|q| q.split('&').find_map(|kv| kv.strip_prefix("token_id=")))
            .unwrap_or("")
            .to_string();
        let (bid, ask) = st
            .lock()
            .unwrap()
            .books
            .get(&token)
            .copied()
            .unwrap_or((0.0, 0.0));
        Json(json!({
            "market": CONDITION,
            "asset_id": token,
            "bids": [{"price": format!("{bid}"), "size": "150.0"}],
            "asks": [{"price": format!("{ask}"), "size": "120.0"}],
            "hash": "abc",
            "timestamp": format!("{}", Utc::now().timestamp_millis())
        }))
    }

    async fn clob_market(
        State(st): State<S>,
        method: Method,
        uri: Uri,
        headers: HeaderMap,
        Path(cid): Path<String>,
    ) -> impl IntoResponse {
        capture(&st, &method, &uri, &headers, "");
        Json(json!({
            "condition_id": cid,
            "question": "Mock?",
            "tokens": [
                {"token_id": YES, "outcome": "Yes", "price": 0.40, "winner": false},
                {"token_id": NO, "outcome": "No", "price": 0.55, "winner": false}
            ],
            "active": true,
            "closed": false,
            "accepting_orders": true,
            "minimum_tick_size": 0.01
        }))
    }

    async fn clob_post_order(
        State(st): State<S>,
        method: Method,
        uri: Uri,
        headers: HeaderMap,
        body: axum::body::Bytes,
    ) -> axum::response::Response {
        let text = String::from_utf8_lossy(&body).to_string();
        capture(&st, &method, &uri, &headers, &text);
        let behaviour = st.lock().unwrap().post.clone();
        match behaviour.unwrap_or(PostBehaviour::Accept {
            status: "live".into(),
        }) {
            PostBehaviour::Accept { status } => Json(json!({
                "success": true,
                "orderID": "",
                "status": status,
                "takingAmount": "",
                "makingAmount": "",
                "errorMsg": ""
            }))
            .into_response(),
            PostBehaviour::Reject { msg } => Json(json!({
                "success": false,
                "orderID": "",
                "status": "",
                "errorMsg": msg
            }))
            .into_response(),
            PostBehaviour::ServerError => {
                (StatusCode::INTERNAL_SERVER_ERROR, "gateway exploded").into_response()
            }
        }
    }

    async fn clob_cancel_order(
        State(st): State<S>,
        method: Method,
        uri: Uri,
        headers: HeaderMap,
        body: axum::body::Bytes,
    ) -> impl IntoResponse {
        let text = String::from_utf8_lossy(&body).to_string();
        capture(&st, &method, &uri, &headers, &text);
        let id = serde_json::from_str::<Value>(&text)
            .ok()
            .and_then(|v| v.get("orderID").and_then(|s| s.as_str()).map(String::from))
            .unwrap_or_default();
        let behaviour = st.lock().unwrap().cancel.clone();
        match behaviour.unwrap_or(CancelBehaviour::Confirm) {
            CancelBehaviour::Confirm => {
                let mut s = st.lock().unwrap();
                s.orders.remove(&id.to_ascii_lowercase());
                s.open_orders
                    .retain(|o| !o["id"].as_str().unwrap_or("").eq_ignore_ascii_case(&id));
                Json(json!({"canceled": [id], "not_canceled": {}}))
            }
            CancelBehaviour::Refuse { reason } => {
                Json(json!({"canceled": [], "not_canceled": {id: reason}}))
            }
            CancelBehaviour::Unrecognised => Json(json!({})),
        }
    }

    async fn clob_cancel_all(
        State(st): State<S>,
        method: Method,
        uri: Uri,
        headers: HeaderMap,
    ) -> impl IntoResponse {
        capture(&st, &method, &uri, &headers, "");
        let mut s = st.lock().unwrap();
        let ids: Vec<Value> = s.open_orders.iter().map(|o| o["id"].clone()).collect();
        s.open_orders.clear();
        s.orders.clear();
        Json(json!({"canceled": ids, "not_canceled": {}}))
    }

    async fn clob_data_order(
        State(st): State<S>,
        method: Method,
        uri: Uri,
        headers: HeaderMap,
        RawQuery(q): RawQuery,
    ) -> axum::response::Response {
        capture(&st, &method, &uri, &headers, "");
        let id = q
            .as_deref()
            .and_then(|q| q.split('&').find_map(|kv| kv.strip_prefix("order_id=")))
            .unwrap_or("")
            .to_ascii_lowercase();
        let found = st.lock().unwrap().orders.get(&id).cloned();
        match found {
            Some(v) => Json(v).into_response(),
            None => (StatusCode::NOT_FOUND, "not found").into_response(),
        }
    }

    async fn clob_data_orders(
        State(st): State<S>,
        method: Method,
        uri: Uri,
        headers: HeaderMap,
    ) -> impl IntoResponse {
        capture(&st, &method, &uri, &headers, "");
        let open = st.lock().unwrap().open_orders.clone();
        Json(Value::Array(open))
    }

    async fn clob_data_trades(
        State(st): State<S>,
        method: Method,
        uri: Uri,
        headers: HeaderMap,
    ) -> impl IntoResponse {
        capture(&st, &method, &uri, &headers, "");
        let trades = st.lock().unwrap().trades.clone();
        Json(Value::Array(trades))
    }

    async fn clob_heartbeat(
        State(st): State<S>,
        method: Method,
        uri: Uri,
        headers: HeaderMap,
    ) -> impl IntoResponse {
        capture(&st, &method, &uri, &headers, "");
        Json(json!({"ok": true}))
    }

    async fn derive_key(
        State(st): State<S>,
        method: Method,
        uri: Uri,
        headers: HeaderMap,
        body: axum::body::Bytes,
    ) -> impl IntoResponse {
        let text = String::from_utf8_lossy(&body).to_string();
        capture(&st, &method, &uri, &headers, &text);
        Json(
            json!({"apiKey": "mock-key", "secret": "c2VjcmV0LWtleS1mb3ItbW9jay10ZXN0", "passphrase": "mock-pass"}),
        )
    }

    async fn rpc(
        State(st): State<S>,
        method: Method,
        uri: Uri,
        headers: HeaderMap,
        Json(body): Json<Value>,
    ) -> impl IntoResponse {
        capture(&st, &method, &uri, &headers, &body.to_string());
        let data = body["params"][0]["data"]
            .as_str()
            .unwrap_or("")
            .to_ascii_lowercase();
        let selector = data.strip_prefix("0x").unwrap_or(&data);
        let s = st.lock().unwrap();
        let result = if selector.starts_with("70a08231") {
            word(s.balance_raw)
        } else if selector.starts_with("313ce567") {
            word(s.decimals)
        } else if selector.starts_with("dd62ed3e") {
            word(s.allowance_raw)
        } else if selector.starts_with("00fdd58e") {
            // ERC-1155 balanceOf(address,uint256) — CTF reader.
            word(0)
        } else {
            word(0)
        };
        Json(json!({"jsonrpc": "2.0", "id": body["id"].clone(), "result": result}))
    }

    let app = Router::new()
        .route("/gamma/markets", get(gamma_markets))
        .route("/clob/time", get(clob_time))
        .route("/clob/book", get(clob_book))
        .route("/clob/markets/:cid", get(clob_market))
        .route(
            "/clob/order",
            post(clob_post_order).delete(clob_cancel_order),
        )
        .route("/clob/cancel-all", delete(clob_cancel_all))
        .route("/clob/data/order", get(clob_data_order))
        .route("/clob/data/orders", get(clob_data_orders))
        .route("/clob/data/trades", get(clob_data_trades))
        .route("/clob/heartbeat", post(clob_heartbeat))
        .route("/clob/auth/derive-api-key", post(derive_key))
        .route("/rpc", post(rpc))
        .with_state(script.clone());

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    tokio::task::yield_now().await;
    MockVenue { addr, script }
}

// ---------------------------------------------------------------------------
// Config / state / bot builders
// ---------------------------------------------------------------------------

/// Base config: polymarket enabled, paper mode, value strategy, permissive
/// gates, no minimum time-to-resolution, small min order size.
pub fn base_config(venue: &MockVenue) -> Config {
    let mut cfg = Config::default();
    cfg.polymarket.enabled = true;
    cfg.polymarket.clob_url = venue.clob_url();
    cfg.polymarket.gamma_url = venue.gamma_url();
    cfg.polymarket.ctf_rpc_url = venue.rpc_url();
    cfg.polymarket.ws_url = format!("ws://{}/ws/", venue.addr);
    cfg.polymarket.use_websocket = false;
    cfg.polymarket.use_user_websocket = false;
    cfg.polymarket.heartbeat = false;
    cfg.polymarket.strategy = "value".into();
    cfg.polymarket.stake_usd = 10.0;
    cfg.polymarket.min_edge = 0.03;
    cfg.polymarket.min_order_size = 5.0;
    cfg.polymarket.max_spread = 0.10;
    cfg.polymarket.min_liquidity_usd = 0.0;
    cfg.polymarket.quote_max_age_secs = 120;
    cfg.polymarket.min_time_to_resolution_secs = 0;
    cfg.polymarket.max_open_markets = 5;
    cfg.polymarket.order_type = "GTC".into();
    cfg.polymarket.expiration_secs = 0;
    cfg.polymarket.order_poll_interval_secs = 1;
    cfg.polymarket.order_ttl_secs = 0;
    cfg.polymarket.reprice_threshold = 0.0;
    cfg.polymarket.reconcile_interval_secs = 60;
    cfg.polymarket.reconcile_cancel_orphans = false;
    cfg.polymarket.cancel_on_shutdown = true;
    cfg.risk.poly_min_liquidity_usd = 0.0;
    cfg.risk.poly_price_floor = 0.02;
    cfg.risk.poly_price_ceiling = 0.98;
    cfg.risk.max_position_quote = 1_000.0;
    cfg.risk.max_open_positions = 20;
    cfg.risk.max_position_fraction = 1.0;
    cfg.risk.min_sol_reserve = 0.0;
    cfg.execution.mode = ExecutionMode::Paper;
    cfg.execution.allow_live_trading = false;
    cfg
}

/// Live-mode variant (orders are POSTed to the mock venue).
pub fn live_config(venue: &MockVenue) -> Config {
    let mut cfg = base_config(venue);
    cfg.execution.mode = ExecutionMode::Live;
    cfg.execution.allow_live_trading = true;
    cfg
}

pub fn state_with(cfg: Config) -> Shared {
    AppState::new(AppConfig {
        raw: cfg,
        source_path: None,
        warnings: Vec::new(),
    })
}

/// State with a shared in-memory OMS attached (the server does the same).
pub fn state_with_oms(cfg: Config) -> (Shared, Arc<OrderManager>) {
    let state = state_with(cfg);
    let oms = OrderManager::new(None, 1_000);
    state.attach_orders(oms.clone());
    (state, oms)
}

/// A fixed, obviously fake signing key (never used on a real chain).
pub fn test_key() -> SigningKey {
    SigningKey::from_slice(&[0x11u8; 32]).unwrap()
}

pub fn test_api_key() -> ApiKey {
    ApiKey {
        key: "mock-key".into(),
        secret: "c2VjcmV0LWtleS1mb3ItbW9jay10ZXN0".into(),
        passphrase: "mock-pass".into(),
    }
}

/// A paper bot with an in-memory journal.
pub async fn paper_bot(state: &Shared, store: Arc<MemoryPolyStore>) -> PolyBot {
    PolyBot::new(state.clone())
        .await
        .expect("bot builds")
        .with_store(store)
}

/// A live bot: signer + pre-derived credentials + in-memory journal.
pub async fn live_bot(state: &Shared, store: Arc<MemoryPolyStore>) -> PolyBot {
    PolyBot::new(state.clone())
        .await
        .expect("bot builds")
        .with_signer(test_key())
        .with_api_key(test_api_key())
        .with_store(store)
}

/// The signer's EOA address.
pub fn test_address() -> String {
    address_from_signing_key(&test_key())
}

/// Sign `signal` exactly the way the engine does (same key, same frozen
/// tick/expiry/type, same venue config) so a test can know the venue order
/// id — the EIP-712 struct hash — BEFORE the POST and script the venue's
/// answer for it.
pub fn sign_for(
    signal: &OrderSignal,
    size_tokens: f64,
    poly: &PolymarketConfig,
) -> (SignedOrderBundle, String) {
    let d = &signal.decision;
    let params = OrderParams {
        token_id: &d.token_id,
        is_buy: d.is_buy,
        size: size_tokens,
        price: d.limit_price,
        tick_size: &signal.tick_size,
        neg_risk: d.neg_risk,
        chain_id: poly.chain_id,
        domain_version: &poly.exchange_domain_version,
        exchange_address: &poly.exchange_address,
        neg_risk_exchange_address: &poly.neg_risk_exchange_address,
        signature_type: poly.signature_type,
        funder: poly.funder_address.as_deref(),
        expiration_timestamp: signal.expiration,
        builder_code: poly.builder_code.as_deref(),
    };
    let bundle = sign_order_bundle(&test_key(), &params).expect("test signing");
    let id = bundle.derived_order_id().expect("struct hash");
    (bundle, id)
}

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

pub fn market() -> PolyMarket {
    market_with(CONDITION, YES, NO)
}

pub fn market_b() -> PolyMarket {
    market_with(CONDITION_B, YES_B, NO_B)
}

pub fn market_with(condition: &str, yes: &str, no: &str) -> PolyMarket {
    PolyMarket {
        condition_id: condition.to_string(),
        question: format!("Will market {} resolve Yes?", &condition[..6]),
        slug: "mock-market".into(),
        neg_risk: false,
        active: true,
        closed: false,
        accepting_orders: true,
        end_date: Some(Utc::now() + Duration::days(30)),
        volume: 123_456.78,
        liquidity: 9_876.5,
        outcomes: vec![
            PolyOutcome {
                outcome: "Yes".into(),
                token_id: yes.to_string(),
                price: 0.40,
                winner: None,
            },
            PolyOutcome {
                outcome: "No".into(),
                token_id: no.to_string(),
                price: 0.55,
                winner: None,
            },
        ],
    }
}

/// Fresh two-sided quotes for the fixture market: YES 0.39/0.40, NO
/// 0.54/0.55 (asks sum 0.95 → basket edge 0.05).
pub fn quotes() -> HashMap<String, Quote> {
    quotes_for(YES, NO)
}

pub fn quotes_for(yes: &str, no: &str) -> HashMap<String, Quote> {
    let now = Utc::now();
    HashMap::from([
        (yes.to_string(), Quote::observed(0.39, 0.40, now)),
        (no.to_string(), Quote::observed(0.54, 0.55, now)),
    ])
}

/// A BUY decision for YES at 0.40: 25 tokens = 10 USDC.
pub fn decision() -> OrderDecision {
    decision_for(YES, "Yes", 0.40, 25.0)
}

pub fn decision_for(token: &str, outcome: &str, price: f64, size: f64) -> OrderDecision {
    OrderDecision {
        token_id: token.to_string(),
        outcome: outcome.to_string(),
        is_buy: true,
        size_tokens: size,
        limit_price: price,
        stake_usd: size * price,
        condition_id: CONDITION.to_string(),
        neg_risk: false,
        reason: "test decision".into(),
    }
}

/// Freeze a decision the way `PolyBot::build_signal` does, with a fixed
/// tick and no GTD expiry.
pub fn signal(decision: OrderDecision, mode: ExecutionMode) -> OrderSignal {
    OrderSignal::new(
        decision,
        "value",
        mode,
        "GTC",
        "0.01",
        0,
        "Will the mock market resolve Yes?",
        Utc::now(),
    )
}

/// Collect audit actions published so far (the bus is asynchronous; wait
/// a little first).
pub async fn audit_actions(state: &Shared) -> Vec<String> {
    tokio::time::sleep(std::time::Duration::from_millis(60)).await;
    state
        .events
        .recent(500)
        .await
        .into_iter()
        .filter_map(|e| match e {
            AppEvent::Audit { action, .. } => Some(action),
            _ => None,
        })
        .collect()
}

/// Number of `Fill` events published so far.
pub async fn fill_events(state: &Shared) -> usize {
    tokio::time::sleep(std::time::Duration::from_millis(60)).await;
    state
        .events
        .recent(500)
        .await
        .into_iter()
        .filter(|e| matches!(e, AppEvent::Fill { .. }))
        .count()
}

/// `PolymarketConfig` snapshot of a state.
pub async fn poly_cfg(state: &Shared) -> PolymarketConfig {
    state.config_snapshot().await.polymarket.clone()
}
