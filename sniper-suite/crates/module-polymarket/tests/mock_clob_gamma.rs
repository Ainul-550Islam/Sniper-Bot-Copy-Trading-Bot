//! Integration tests for the Polymarket REST clients against a mock CLOB +
//! Gamma HTTP server (axum on an ephemeral loopback port).
//!
//! Proves the real production code paths over real HTTP: query construction,
//! response parsing, L1 (EIP-712) auth headers on `derive-api-key`, and L2
//! (HMAC) auth headers + signed-order body on `POST /order`. Offline and
//! deterministic — no external network, no real credentials.

use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

use axum::{
    extract::{RawQuery, State},
    http::{HeaderMap, Method, Uri},
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
use k256::ecdsa::SigningKey;
use serde_json::{json, Value};
use tokio::net::TcpListener;

use module_polymarket::auth::ApiKey;
use module_polymarket::clob::{build_signed_order, ClobClient};
use module_polymarket::eip712::address_from_signing_key;
use module_polymarket::gamma::{GammaClient, MarketQuery};
use module_polymarket::orders::OrderParams;

/// Everything the mock needs to remember about a request for assertions.
#[derive(Debug, Clone)]
struct Captured {
    method: String,
    path: String,
    query: Option<String>,
    /// POLY_* headers only — nothing else is interesting and this keeps the
    /// capture free of incidental data.
    poly_headers: BTreeMap<String, String>,
    body: String,
}

type Captures = Arc<Mutex<Vec<Captured>>>;

#[derive(Clone)]
struct MockState {
    captures: Captures,
}

fn capture(state: &MockState, method: &Method, uri: &Uri, headers: &HeaderMap, body: &str) {
    let mut poly_headers = BTreeMap::new();
    for (k, v) in headers.iter() {
        let name = k.as_str().to_ascii_uppercase();
        if name.starts_with("POLY_") {
            if let Ok(s) = v.to_str() {
                poly_headers.insert(name, s.to_string());
            }
        }
    }
    state.captures.lock().unwrap().push(Captured {
        method: method.to_string(),
        path: uri.path().to_string(),
        query: uri.query().map(String::from),
        poly_headers,
        body: body.to_string(),
    });
}

async fn mock_server() -> (SocketAddr, Captures) {
    let captures: Captures = Arc::new(Mutex::new(Vec::new()));
    let state = MockState {
        captures: captures.clone(),
    };

    async fn gamma_markets(
        State(st): State<MockState>,
        method: Method,
        uri: Uri,
        headers: HeaderMap,
        RawQuery(q): RawQuery,
    ) -> impl IntoResponse {
        capture(&st, &method, &uri, &headers, "");
        let _ = q;
        Json(json!([{
            "conditionId": "0x1111222233334444555566667777888899990000aaaabbbbccccddddeeeeaaaa",
            "question": "Will the mock market resolve Yes?",
            "slug": "will-the-mock-market-resolve-yes",
            "negRisk": false,
            "active": true,
            "closed": false,
            "acceptingOrders": true,
            "endDate": "2030-01-01T00:00:00Z",
            "volume": "123456.78",
            "liquidity": "9876.5",
            // Gamma encodes these as stringified JSON arrays.
            "outcomes": "[\"Yes\",\"No\"]",
            "clobTokenIds": "[\"111\",\"222\"]",
            "outcomePrices": "[\"0.62\",\"0.38\"]"
        }]))
    }

    async fn clob_time(
        State(st): State<MockState>,
        method: Method,
        uri: Uri,
        headers: HeaderMap,
    ) -> impl IntoResponse {
        capture(&st, &method, &uri, &headers, "");
        // The real endpoint returns a bare stringified number.
        Json(Value::String("1700000123".into()))
    }

    async fn clob_book(
        State(st): State<MockState>,
        method: Method,
        uri: Uri,
        headers: HeaderMap,
        RawQuery(q): RawQuery,
    ) -> impl IntoResponse {
        capture(&st, &method, &uri, &headers, "");
        let _ = q;
        Json(json!({
            "market": "0x1111",
            "asset_id": "111",
            "bids": [{"price": "0.60", "size": "150.0"}, {"price": "0.61", "size": "90.0"}],
            "asks": [{"price": "0.63", "size": "120.0"}, {"price": "0.62", "size": "80.0"}],
            "hash": "abc",
            "timestamp": "1700000000"
        }))
    }

    async fn clob_price(
        State(st): State<MockState>,
        method: Method,
        uri: Uri,
        headers: HeaderMap,
    ) -> impl IntoResponse {
        capture(&st, &method, &uri, &headers, "");
        Json(json!({"price": "0.42"}))
    }

    async fn clob_midpoint(
        State(st): State<MockState>,
        method: Method,
        uri: Uri,
        headers: HeaderMap,
    ) -> impl IntoResponse {
        capture(&st, &method, &uri, &headers, "");
        Json(json!({"mid": "0.55"}))
    }

    async fn clob_market(
        State(st): State<MockState>,
        method: Method,
        uri: Uri,
        headers: HeaderMap,
    ) -> impl IntoResponse {
        capture(&st, &method, &uri, &headers, "");
        Json(json!({
            "condition_id": "0x1111",
            "question": "Mock?",
            "tokens": [
                {"token_id": "111", "outcome": "Yes", "price": 0.62, "winner": false},
                {"token_id": "222", "outcome": "No", "price": 0.38, "winner": false}
            ],
            "active": true,
            "closed": false,
            "accepting_orders": true,
            "minimum_tick_size": 0.001
        }))
    }

    async fn clob_order(
        State(st): State<MockState>,
        method: Method,
        uri: Uri,
        headers: HeaderMap,
        body: axum::body::Bytes,
    ) -> impl IntoResponse {
        let text = String::from_utf8_lossy(&body).to_string();
        capture(&st, &method, &uri, &headers, &text);
        // Mirror the live CLOB's camelCase response keys — the client must
        // parse them via its serde aliases.
        Json(json!({
            "success": true,
            "orderID": "0xdeadbeef",
            "status": "matched",
            "takingAmount": "100",
            "makingAmount": "62",
            "errorMsg": ""
        }))
    }

    async fn derive_key(
        State(st): State<MockState>,
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

    let app = Router::new()
        .route("/gamma/markets", get(gamma_markets))
        .route("/clob/time", get(clob_time))
        .route("/clob/book", get(clob_book))
        .route("/clob/price", get(clob_price))
        .route("/clob/midpoint", get(clob_midpoint))
        .route("/clob/markets/:cid", get(clob_market))
        .route("/clob/order", post(clob_order))
        .route("/clob/auth/derive-api-key", post(derive_key))
        .with_state(state);

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (addr, captures)
}

fn test_key() -> SigningKey {
    // A fixed, obviously-fake private scalar (never used on a real chain).
    SigningKey::from_slice(&[0x11u8; 32]).unwrap()
}

fn find(captures: &Captures, path: &str) -> Captured {
    captures
        .lock()
        .unwrap()
        .iter()
        .find(|c| c.path == path)
        .unwrap_or_else(|| panic!("no captured request for {path}"))
        .clone()
}

#[tokio::test]
async fn gamma_markets_query_and_parse() {
    let (addr, captures) = mock_server().await;
    let client = GammaClient::new(format!("http://{addr}/gamma")).unwrap();

    let markets = client
        .markets(&MarketQuery {
            active: Some(true),
            closed: Some(false),
            limit: Some(10),
            ..Default::default()
        })
        .await
        .unwrap();

    // Query parameters went out as the client documents them.
    let cap = find(&captures, "/gamma/markets");
    let q = cap.query.unwrap();
    assert!(q.contains("active=true"), "{q}");
    assert!(q.contains("closed=false"), "{q}");
    assert!(q.contains("limit=10"), "{q}");

    assert_eq!(markets.len(), 1);
    let m = &markets[0];
    assert_eq!(
        m.condition_id,
        "0x1111222233334444555566667777888899990000aaaabbbbccccddddeeeeaaaa"
    );
    assert_eq!(m.question, "Will the mock market resolve Yes?");
    assert!(!m.neg_risk && m.active && !m.closed && m.accepting_orders);
    assert_eq!(m.volume, 123456.78);
    assert_eq!(m.liquidity, 9876.5);
    assert!(m.end_date.is_some());
    assert_eq!(m.outcomes.len(), 2);
    assert_eq!(m.outcomes[0].outcome, "Yes");
    assert_eq!(m.outcomes[0].token_id, "111");
    assert_eq!(m.outcomes[0].price, 0.62);
    assert_eq!(m.outcomes[1].outcome, "No");
    assert_eq!(m.outcomes[1].price, 0.38);

    // market_by_id round-trips through the same endpoint with id=...&limit=1.
    let one = client.market_by_id("0x1111").await.unwrap();
    assert!(one.is_some());
    let caps = captures.lock().unwrap();
    let last = caps.last().unwrap();
    let q = last.query.clone().unwrap();
    assert!(q.contains("id=0x1111"), "{q}");
    assert!(q.contains("limit=1"), "{q}");
}

#[tokio::test]
async fn clob_public_endpoints_parse() {
    let (addr, captures) = mock_server().await;
    let client = ClobClient::new(format!("http://{addr}/clob"), 137).unwrap();

    assert_eq!(client.server_time().await.unwrap(), 1_700_000_123);

    let book = client.order_book("111").await.unwrap();
    assert_eq!(find(&captures, "/clob/book").query.unwrap(), "token_id=111");
    assert_eq!(book.best_bid(), Some(0.61), "best bid is the HIGHEST");
    assert_eq!(book.best_ask(), Some(0.62), "best ask is the LOWEST");
    let quote = book.to_quote();
    assert!((quote.midpoint - 0.615).abs() < 1e-9);

    assert_eq!(client.price("111", "buy").await.unwrap(), 0.42);
    let q = find(&captures, "/clob/price").query.unwrap();
    assert!(q.contains("token_id=111") && q.contains("side=buy"), "{q}");
    assert_eq!(client.midpoint("111").await.unwrap(), 0.55);

    let market = client.market("0x1111").await.unwrap();
    assert_eq!(market.tokens.len(), 2);
    assert_eq!(market.tokens[0].token_id, "111");
    // numeric tick 0.001 maps to the CLOB tick-size string.
    assert_eq!(client.tick_size("0x1111").await.unwrap(), "0.001");
}

#[tokio::test]
async fn derive_api_key_sends_l1_headers() {
    let (addr, captures) = mock_server().await;
    let key = test_key();
    let address = address_from_signing_key(&key);

    let creds = ClobClient::derive_api_key(&format!("http://{addr}/clob"), 137, &key, &address)
        .await
        .unwrap();
    assert_eq!(creds.key, "mock-key");
    assert_eq!(creds.secret, "c2VjcmV0LWtleS1mb3ItbW9jay10ZXN0");
    assert_eq!(creds.passphrase, "mock-pass");

    let cap = find(&captures, "/clob/auth/derive-api-key");
    assert_eq!(cap.method, "POST");
    // L1: address + EIP-712 signature + timestamp + nonce.
    assert_eq!(cap.poly_headers.get("POLY_ADDRESS").unwrap(), &address);
    assert_eq!(cap.poly_headers.get("POLY_NONCE").unwrap(), "0");
    let sig = cap.poly_headers.get("POLY_SIGNATURE").unwrap();
    assert!(
        sig.starts_with("0x") && sig.len() == 132,
        "65-byte hex sig: {sig}"
    );
    assert!(cap.poly_headers.contains_key("POLY_TIMESTAMP"));
    // L1 must NOT carry L2 credentials.
    assert!(!cap.poly_headers.contains_key("POLY_API_KEY"));
}

#[tokio::test]
async fn post_order_sends_l2_headers_and_signed_bundle() {
    let (addr, captures) = mock_server().await;
    let key = test_key();
    let address = address_from_signing_key(&key);
    let creds = ApiKey {
        key: "mock-key".into(),
        secret: "c2VjcmV0LWtleS1mb3ItbW9jay10ZXN0".into(),
        passphrase: "mock-pass".into(),
    };
    let client = ClobClient::new(format!("http://{addr}/clob"), 137)
        .unwrap()
        .with_auth(address.clone(), creds);

    let params = OrderParams {
        token_id: "111",
        is_buy: true,
        size: 100.0,
        price: 0.62,
        tick_size: "0.01",
        neg_risk: false,
        chain_id: 137,
        domain_version: "2",
        // Verified live V2 exchange addresses (see README/config).
        exchange_address: "0xE111180000d2663C0091e4f400237545B87B996B",
        neg_risk_exchange_address: "0xe2222d279d744050d28e00520010520000310F59",
        signature_type: 0,
        funder: None,
        expiration_timestamp: 0,
        builder_code: None,
    };
    let bundle = build_signed_order(&key, &params).unwrap();
    assert!(bundle.signature.starts_with("0x"));

    let resp = client.post_order(&bundle, "GTC").await.unwrap();
    // The live CLOB answers in camelCase; aliases must surface the id.
    assert_eq!(resp.success, Some(true));
    assert_eq!(resp.order_id.as_deref(), Some("0xdeadbeef"));
    assert_eq!(resp.status.as_deref(), Some("matched"));
    assert_eq!(resp.taking_amount.as_deref(), Some("100"));
    assert_eq!(resp.making_amount.as_deref(), Some("62"));

    let cap = find(&captures, "/clob/order");
    // L2 header set is complete.
    for h in [
        "POLY_ADDRESS",
        "POLY_SIGNATURE",
        "POLY_TIMESTAMP",
        "POLY_API_KEY",
        "POLY_PASSPHRASE",
    ] {
        assert!(cap.poly_headers.contains_key(h), "missing {h}");
    }
    assert_eq!(cap.poly_headers.get("POLY_ADDRESS").unwrap(), &address);
    assert_eq!(cap.poly_headers.get("POLY_API_KEY").unwrap(), "mock-key");
    assert_eq!(
        cap.poly_headers.get("POLY_PASSPHRASE").unwrap(),
        "mock-pass"
    );

    // Body: {order: {...signed V2 fields...}, owner: apiKey, orderType}.
    let body: Value = serde_json::from_str(&cap.body).unwrap();
    assert_eq!(body["owner"], "mock-key");
    assert_eq!(body["orderType"], "GTC");
    let order = &body["order"];
    assert_eq!(order["tokenId"], "111");
    assert_eq!(order["side"], "BUY");
    assert_eq!(order["signatureType"], 0);
    assert_eq!(order["maker"], address.to_ascii_lowercase());
    assert_eq!(
        order["signature"].as_str().unwrap(),
        bundle.signature,
        "the EIP-712 signature must travel verbatim"
    );
    // Empty bytes32 fields go as 64 zeros, per CLOB spec.
    assert_eq!(order["metadata"], format!("0x{}", "0".repeat(64)));
    assert_eq!(order["builder"], format!("0x{}", "0".repeat(64)));
    // makerAmount/takerAmount are integer strings (raw units).
    assert!(order["makerAmount"]
        .as_str()
        .unwrap()
        .parse::<u128>()
        .is_ok());
    assert!(order["takerAmount"]
        .as_str()
        .unwrap()
        .parse::<u128>()
        .is_ok());
}

#[tokio::test]
async fn authenticated_call_without_credentials_fails_locally() {
    let (addr, captures) = mock_server().await;
    let client = ClobClient::new(format!("http://{addr}/clob"), 137).unwrap(); // no with_auth
    let err = client
        .post_order(
            &build_signed_order(
                &test_key(),
                &OrderParams {
                    token_id: "111",
                    is_buy: false,
                    size: 1.0,
                    price: 0.5,
                    tick_size: "0.01",
                    neg_risk: false,
                    chain_id: 137,
                    domain_version: "2",
                    exchange_address: "0xE111180000d2663C0091e4f400237545B87B996B",
                    neg_risk_exchange_address: "0xe2222d279d744050d28e00520010520000310F59",
                    signature_type: 0,
                    funder: None,
                    expiration_timestamp: 0,
                    builder_code: None,
                },
            )
            .unwrap(),
            "GTC",
        )
        .await
        .expect_err("must fail without API credentials");
    assert!(
        err.to_string().to_ascii_lowercase().contains("credential")
            || err.to_string().to_ascii_lowercase().contains("not set"),
        "clear configuration error, got: {err}"
    );
    // Nothing hit the network.
    assert!(captures.lock().unwrap().is_empty());
}
