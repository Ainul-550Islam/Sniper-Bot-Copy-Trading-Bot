//! Venue-side existence check for an ambiguous Polymarket submission, driven
//! against an in-process mock CLOB.
//!
//! The durable claim stops a duplicate POST; it cannot tell us whether the
//! venue holds the order. These tests pin the venue probe's four answers
//! through the REAL `ClobClient` (L2 auth, request paths, JSON decoding) against
//! a local HTTP server that impersonates `GET /data/order` and
//! `GET /data/orders`:
//!
//! * the venue holds the id derived from our signed struct → `landed`,
//!   `link_allowed`, and **never** `abandon_allowed`;
//! * the venue answers 404 → `absent`, which authorises nothing;
//! * the venue holds an order on the same token under another id → `mismatch`;
//! * the venue errors → `unknown`, never `absent`.
//!
//! The venue is a mock: this is not live Polymarket evidence.

use std::sync::Arc;

use axum::{extract::Query, http::StatusCode, routing::get, Json, Router};
use module_polymarket::auth::ApiKey;
use module_polymarket::clob::ClobClient;
use module_polymarket::venue_probe::probe_order;
use module_polymarket::venue_state::VenuePresence;
use serde_json::{json, Value};

const DERIVED: &str = "0x1234567890abcdef1234567890abcdef1234567890abcdef1234567890abcdef";
const TOKEN: &str = "71321045679252212594626385532706912750332728571942532289631379312455583992563";

/// What the mock venue does for `GET /data/order`.
#[derive(Clone)]
enum OrderAnswer {
    Found { id: String, status: &'static str },
    NotFound,
    Boom,
}

/// A real `ClobClient` (unauthenticated: the mock ignores auth, but the client
/// still builds the L2 path) pointed at the local mock.
async fn mock_venue(answer: OrderAnswer, open: Vec<Value>) -> ClobClient {
    let state = Arc::new((answer, open));
    let app = Router::new()
        .route(
            "/data/order",
            get({
                let state = state.clone();
                move |Query(q): Query<std::collections::HashMap<String, String>>| {
                    let state = state.clone();
                    async move {
                        let asked = q.get("order_id").cloned().unwrap_or_default();
                        match &state.0 {
                            OrderAnswer::Boom => (
                                StatusCode::INTERNAL_SERVER_ERROR,
                                Json(json!({ "error": "venue down" })),
                            ),
                            OrderAnswer::NotFound => (StatusCode::NOT_FOUND, Json(Value::Null)),
                            OrderAnswer::Found { id, status } => {
                                // Only the id we were asked about is "held".
                                if !asked.eq_ignore_ascii_case(id) {
                                    return (StatusCode::NOT_FOUND, Json(Value::Null));
                                }
                                (
                                    StatusCode::OK,
                                    Json(json!({
                                        "id": id,
                                        "status": status,
                                        "asset_id": TOKEN,
                                        "size_matched": "25.5",
                                        "original_size": "25.5",
                                    })),
                                )
                            }
                        }
                    }
                }
            }),
        )
        .route(
            "/data/orders",
            get({
                let state = state.clone();
                move || {
                    let state = state.clone();
                    async move {
                        if matches!(state.0, OrderAnswer::Boom) {
                            return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!([])));
                        }
                        (StatusCode::OK, Json(Value::Array(state.1.clone())))
                    }
                }
            }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    tokio::task::yield_now().await;
    // A disposable L2 credential: the mock does not verify the HMAC, but the
    // client must actually sign and attach it (that is the production path).
    let key = ApiKey {
        key: "mock-key".into(),
        secret: "bW9jay1zZWNyZXQ=".into(),
        passphrase: "mock-pass".into(),
    };
    ClobClient::new(format!("http://{addr}"), 137)
        .expect("clob client")
        .with_auth("0x0000000000000000000000000000000000000001", key)
}

#[tokio::test]
async fn a_venue_holding_the_derived_order_is_landed_and_never_releasable() {
    let client = mock_venue(
        OrderAnswer::Found {
            id: DERIVED.to_string(),
            status: "matched",
        },
        vec![],
    )
    .await;
    let check = probe_order(&client, DERIVED, TOKEN).await;
    assert_eq!(check.presence, VenuePresence::Landed, "{check:?}");
    assert!(check.link_allowed());
    assert!(
        !check.abandon_allowed(),
        "a landing must never free the trade key for a retry"
    );
    assert_eq!(check.venue_status.as_deref(), Some("matched"));
    assert_eq!(check.size_matched.as_deref(), Some("25.5"), "{check:?}");
}

#[tokio::test]
async fn a_venue_404_is_absent_and_authorises_nothing() {
    let client = mock_venue(OrderAnswer::NotFound, vec![]).await;
    let check = probe_order(&client, DERIVED, TOKEN).await;
    assert_eq!(check.presence, VenuePresence::Absent, "{check:?}");
    assert!(!check.link_allowed() && !check.abandon_allowed());
    assert!(check.reason.contains("no order"), "{}", check.reason);
}

#[tokio::test]
async fn an_order_on_the_same_token_under_another_id_is_a_mismatch() {
    let client = mock_venue(
        OrderAnswer::NotFound,
        vec![json!({
            "id": "0xfeedface",
            "status": "live",
            "asset_id": TOKEN,
            "size_matched": "0",
        })],
    )
    .await;
    let check = probe_order(&client, DERIVED, TOKEN).await;
    assert_eq!(check.presence, VenuePresence::Mismatch, "{check:?}");
    assert!(!check.link_allowed() && !check.abandon_allowed());
    assert_eq!(
        check.open_order_ids,
        vec!["0xfeedface".to_string()],
        "the operator must see what the venue did hold"
    );
}

#[tokio::test]
async fn a_venue_error_is_unknown_never_absent() {
    let client = mock_venue(OrderAnswer::Boom, vec![]).await;
    let check = probe_order(&client, DERIVED, TOKEN).await;
    assert!(check.presence.is_unknown(), "{check:?}");
    assert!(
        !check.link_allowed() && !check.abandon_allowed(),
        "an unanswered venue authorises nothing"
    );
    assert!(
        check.reason.contains("venue probe failed"),
        "{}",
        check.reason
    );
}

#[tokio::test]
async fn a_malformed_id_never_reaches_the_venue() {
    let client = mock_venue(OrderAnswer::NotFound, vec![]).await;
    for bad in ["", "0x", "0xnothex", &"1".repeat(66)] {
        let check = probe_order(&client, bad, TOKEN).await;
        assert!(check.presence.is_unknown(), "{bad:?} → {check:?}");
        assert!(check.reason.contains("not asking the venue"), "{bad:?}");
    }
}
