//! Transport-level regression tests use real ConnectInfo extensions and the
//! production router/middleware, never a caller-controlled internal header.
//! Rate tests are hermetic. The durable allowlist test is explicitly ignored
//! unless requested with a real migrated PostgreSQL database:
//! POSTGRES_URL=... cargo test -p sniper-suite --test trusted_proxy_client_ip -- --ignored

use std::net::SocketAddr;
use std::sync::Arc;

use axum::body::Body;
use axum::extract::ConnectInfo;
use axum::http::{Request, StatusCode};
use axum::Router;
use tower::ServiceExt;

use bot_core::audit::AuditTrail;
use bot_core::auth::RateLimiter;
use bot_core::config::AppConfig;
use bot_core::obs::health::HealthRegistry;
use bot_core::state::AppState;

use sniper_suite::api::{router, ApiState};
use sniper_suite::saas::trusted_proxy::TrustedProxies;
use sniper_suite::saas::SaasStore;

fn state(rpm: u32, trusted: &[&str]) -> ApiState {
    let shared = AppState::new(AppConfig::from_defaults());
    ApiState {
        audit: AuditTrail::new(None, shared.events.clone()),
        shared,
        api_key: None,
        auth: None,
        limiter: RateLimiter::new(rpm),
        sensitive_limiter: RateLimiter::new(0),
        trusted_proxies: TrustedProxies::from_config(
            &trusted
                .iter()
                .map(|value| value.to_string())
                .collect::<Vec<_>>(),
        )
        .unwrap(),
        db: None,
        journal: None,
        serve_dashboard: false,
        health: Arc::new(HealthRegistry::new()),
        metrics_enabled: false,
        saas: Arc::new(SaasStore::new()),
        trading: None,
        module_registry: Arc::new(
            sniper_suite::module_runtime::module_registry::TenantModuleRegistry::new(),
        ),
    }
}

async fn send(
    app: Router,
    path: &str,
    peer: Option<&str>,
    headers: &[(&str, &str)],
) -> (StatusCode, serde_json::Value) {
    let mut builder = Request::builder().uri(path);
    for &(name, value) in headers {
        builder = builder.header(name, value);
    }
    let mut req = builder.body(Body::empty()).unwrap();
    if let Some(peer) = peer {
        req.extensions_mut()
            .insert(ConnectInfo(peer.parse::<SocketAddr>().unwrap()));
    }
    let response = app.oneshot(req).await.unwrap();
    let status = response.status();
    let body = axum::body::to_bytes(response.into_body(), 1 << 20)
        .await
        .unwrap();
    (status, serde_json::from_slice(&body).unwrap_or_default())
}

#[tokio::test]
async fn trusted_proxy_clients_have_independent_rate_buckets() {
    let state = state(1, &["10.0.0.0/8"]);
    let limiter = state.limiter.clone();
    let app = router(state);
    for client in ["192.0.2.1", "192.0.2.2"] {
        assert_eq!(
            send(
                app.clone(),
                "/health",
                Some("10.0.0.1:4000"),
                &[("x-forwarded-for", client)]
            )
            .await
            .0,
            StatusCode::OK
        );
    }
    assert_eq!(limiter.tracked().await, 2);
    assert_eq!(
        send(
            app,
            "/health",
            Some("10.0.0.1:4001"),
            &[("x-forwarded-for", "192.0.2.1")]
        )
        .await
        .0,
        StatusCode::TOO_MANY_REQUESTS
    );
}

#[tokio::test]
async fn untrusted_peer_cannot_evade_rate_limit_by_changing_either_header() {
    let state = state(1, &["10.0.0.0/8"]);
    let limiter = state.limiter.clone();
    let app = router(state);
    assert_eq!(
        send(
            app.clone(),
            "/health",
            Some("192.0.3.4:4000"),
            &[("x-forwarded-for", "192.0.2.1"), ("x-real-ip", "192.0.2.1")]
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(
        send(
            app,
            "/health",
            Some("192.0.3.4:5000"),
            &[("x-forwarded-for", "192.0.2.2"), ("x-real-ip", "192.0.2.2")]
        )
        .await
        .0,
        StatusCode::TOO_MANY_REQUESTS
    );
    assert_eq!(limiter.tracked().await, 1);
}

#[tokio::test]
async fn missing_connect_info_keeps_the_unknown_rate_bucket() {
    let state = state(1, &["10.0.0.0/8"]);
    let limiter = state.limiter.clone();
    let app = router(state);
    assert_eq!(
        send(
            app.clone(),
            "/health",
            None,
            &[("x-forwarded-for", "192.0.2.1")]
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(
        send(app, "/health", None, &[("x-forwarded-for", "192.0.2.2")])
            .await
            .0,
        StatusCode::TOO_MANY_REQUESTS
    );
    assert_eq!(limiter.tracked().await, 1);
}

#[tokio::test]
async fn trusted_ipv6_proxy_uses_the_rightmost_untrusted_client_for_rate_limits() {
    let app = router(state(1, &["2001:db8:ffff::/48"]));
    let peer = Some("[2001:db8:ffff::1]:4000");
    assert_eq!(
        send(
            app.clone(),
            "/health",
            peer,
            &[("x-forwarded-for", "2001:db8::1, 2001:db8:ffff::2")]
        )
        .await
        .0,
        StatusCode::OK
    );
    // Different spoofed leftmost origins cannot change the selected client.
    assert_eq!(
        send(
            app.clone(),
            "/health",
            peer,
            &[(
                "x-forwarded-for",
                "2001:db8::9, 2001:db8::1, 2001:db8:ffff::2"
            )]
        )
        .await
        .0,
        StatusCode::TOO_MANY_REQUESTS
    );
    assert_eq!(
        send(
            app,
            "/health",
            peer,
            &[("x-forwarded-for", "2001:db8::2, 2001:db8:ffff::2")]
        )
        .await
        .0,
        StatusCode::OK
    );
}

#[tokio::test]
#[ignore = "requires real PostgreSQL: set POSTGRES_URL and run this test with --ignored"]
async fn router_enforces_durable_allowlist_against_spoofed_headers() {
    use bot_core::config::DatabaseConfig;
    use bot_core::db::Database;
    use bot_core::membership::MembershipRole;
    use bot_core::tenant::{Organization, OrganizationId};
    use chrono::Utc;
    use sniper_suite::saas::api_keys::build_api_key;

    let url = std::env::var("POSTGRES_URL").expect("POSTGRES_URL is required, never silently skip");
    let db = Arc::new(
        Database::connect(&DatabaseConfig::default(), &url)
            .await
            .unwrap(),
    );
    db.migrate().await.unwrap();
    let store = Arc::new(SaasStore::with_database(Some(db.clone())).await.unwrap());
    let org = Organization::new(
        OrganizationId::new(),
        format!("proxy-regression-{}", uuid::Uuid::new_v4().simple()),
        "Proxy regression",
        None,
        Utc::now(),
    );
    store.create_organization(&org).await.unwrap();
    let key = build_api_key(
        org.id,
        "proxy regression",
        MembershipRole::OrgOwner,
        vec![],
        None,
        None,
        Utc::now(),
    );
    store.create_api_key(&key.key).await.unwrap();
    sqlx::query(
        "INSERT INTO tenant_security_policies (organization_id, ip_allowlist) VALUES ($1, $2)",
    )
    .bind(org.id.as_uuid())
    .bind(vec!["192.0.2.1/32", "2001:db8::1/128"])
    .execute(db.pool())
    .await
    .unwrap();
    let mut state = state(0, &["10.0.0.0/8", "2001:db8:ffff::/48"]);
    state.db = Some(db.clone());
    state.saas = store;
    let app = router(state);
    let bearer = format!("Bearer {}", key.plaintext);
    let path = "/api/saas/organizations/current/members";

    // Valid credential, but an untrusted direct peer cannot pick its client IP.
    let (status, body) = send(
        app.clone(),
        path,
        Some("192.0.3.4:4000"),
        &[
            ("authorization", &bearer),
            ("x-forwarded-for", "192.0.2.1"),
            ("x-real-ip", "192.0.2.1"),
        ],
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert_eq!(body["error"], "deny_permission");
    let (status, body) = send(
        app.clone(),
        path,
        Some("10.0.0.1:4000"),
        &[
            ("authorization", &bearer),
            ("x-forwarded-for", "192.0.2.9, 192.0.2.1, 10.0.0.2"),
        ],
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(body["members"].is_array());
    assert_eq!(
        send(
            app.clone(),
            path,
            Some("10.0.0.1:4000"),
            &[
                ("authorization", &bearer),
                ("x-forwarded-for", "192.0.2.1, 192.0.3.4, 10.0.0.2")
            ]
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        send(
            app.clone(),
            path,
            Some("[2001:db8:ffff::1]:4000"),
            &[
                ("authorization", &bearer),
                ("x-forwarded-for", "2001:db8::1, 2001:db8:ffff::2")
            ]
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(
        send(
            app.clone(),
            path,
            None,
            &[("authorization", &bearer), ("x-forwarded-for", "192.0.2.1")]
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        send(
            app.clone(),
            path,
            Some("10.0.0.1:4000"),
            &[("authorization", &bearer), ("x-forwarded-for", "malformed")]
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        send(
            app.clone(),
            path,
            Some("10.0.0.1:4000"),
            &[
                ("authorization", &bearer),
                ("x-forwarded-for", "malformed"),
                ("x-real-ip", "192.0.2.1")
            ]
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(
        send(
            app,
            path,
            Some("192.0.2.1:4000"),
            &[("authorization", &bearer), ("x-forwarded-for", "192.0.3.4")]
        )
        .await
        .0,
        StatusCode::OK
    );

    sqlx::query("DELETE FROM saas_runtime_records WHERE organization_id = $1")
        .bind(org.id.as_uuid())
        .execute(db.pool())
        .await
        .unwrap();
    sqlx::query("DELETE FROM organizations WHERE id = $1")
        .bind(org.id.as_uuid())
        .execute(db.pool())
        .await
        .unwrap();
}
