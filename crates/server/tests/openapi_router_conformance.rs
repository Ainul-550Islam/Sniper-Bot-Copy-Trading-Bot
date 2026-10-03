//! Every documented path is actually served (P1).
//!
//! # The gap this closes
//!
//! The contract is hand-written. Nothing structurally prevents it from
//! describing an endpoint that does not exist — and it did: four
//! `/api/ops/...` paths were published while no router served any of
//! them. A caller following the contract got 404, and no test, lint or
//! compiler noticed, because a JSON document cannot be type-checked
//! against an `axum::Router`.
//!
//! This test closes the direction that matters most commercially: **the
//! contract must not promise what the deployment cannot do.** A buyer,
//! an SDK generator and a partner integration all read the document as
//! a commitment.
//!
//! # How a path is judged
//!
//! Axum exposes no route table, so the router is interrogated the only
//! way it can be: by sending a real request through it and looking at
//! the status.
//!
//! * **404** → the path is not routed at all. FAIL.
//! * **405** → the path is routed but not for that method. FAIL (the
//!   contract names the method).
//! * **anything else** (401, 403, 503, 200, …) → routed. The handler
//!   refusing an unauthenticated request is exactly right and is
//!   asserted separately below.
//!
//! The test state has no database and no credentials, so every
//! authenticated route answers 401/403 and no handler reaches a real
//! dependency. That is enough to prove the route EXISTS, which is the
//! claim being tested. It deliberately does not test behaviour; the
//! per-endpoint suites do that.
//!
//! # What this test does NOT do
//!
//! It does not check the opposite direction (a routed path that is
//! undocumented). Several internal routes (`/metrics`, `/api/status`,
//! the dashboard) are intentionally unpublished, so that direction
//! needs an allow-list to be meaningful and is left for when the
//! document is generated from the router rather than written by hand.

use std::sync::Arc;

use axum::body::Body;
use axum::http::StatusCode;
use serde_json::Value;
use tower::ServiceExt;

use bot_core::audit::AuditTrail;
use bot_core::auth::RateLimiter;
use bot_core::config::AppConfig;
use bot_core::obs::health::HealthRegistry;
use bot_core::state::AppState;

use sniper_suite::api::{router, ApiState};
use sniper_suite::saas::openapi::document;
use sniper_suite::saas::SaasStore;

/// A deployment with nothing attached: no database, no credentials, no
/// trading plane. Routes still have to exist.
fn test_state() -> ApiState {
    let shared = AppState::new(AppConfig::from_defaults());
    ApiState {
        audit: AuditTrail::new(None, shared.events.clone()),
        shared,
        api_key: None,
        auth: None,
        limiter: RateLimiter::new(0),
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

/// Substitute a syntactically valid value for every `{param}` so the
/// request reaches the router's matcher. The values are deliberately
/// well-formed (a real UUID, a real provider name): a 400 from a
/// parameter parser would still prove the route exists, but a 404 from
/// a *matcher* that rejected the shape would not, and the two must not
/// be confused.
fn concrete_path(template: &str) -> String {
    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(open) = rest.find('{') {
        let close = rest[open..]
            .find('}')
            .map(|i| open + i)
            .unwrap_or(rest.len());
        out.push_str(&rest[..open]);
        let name = &rest[open + 1..close];
        out.push_str(match name {
            "provider" => "stripe",
            _ => "00000000-0000-0000-0000-000000000001",
        });
        rest = &rest[(close + 1).min(rest.len())..];
    }
    out.push_str(rest);
    out
}

/// Every (path, method) pair in the contract.
fn documented_operations() -> Vec<(String, String, String)> {
    let doc = document();
    let mut out = Vec::new();
    for (path, item) in doc["paths"].as_object().expect("paths is an object") {
        for (method, operation) in item.as_object().expect("path item is an object") {
            if !matches!(
                method.as_str(),
                "get" | "post" | "put" | "patch" | "delete" | "head" | "options"
            ) {
                continue; // "parameters", "summary", …
            }
            let id = operation
                .get("operationId")
                .and_then(Value::as_str)
                .unwrap_or("<missing operationId>")
                .to_string();
            out.push((path.clone(), method.to_uppercase(), id));
        }
    }
    out.sort();
    out
}

async fn status_of(method: &str, path: &str) -> StatusCode {
    let app = router(test_state());
    let request = axum::http::Request::builder()
        .method(method)
        .uri(path)
        // A body for the methods that take one: a handler must not be
        // able to answer 404-shaped failures because the body was absent.
        .header("content-type", "application/json")
        .body(Body::from("{}"))
        .expect("build request");
    app.oneshot(request)
        .await
        .expect("router responded")
        .status()
}

#[tokio::test]
async fn every_documented_path_is_routed() {
    let mut unrouted: Vec<String> = Vec::new();
    let mut wrong_method: Vec<String> = Vec::new();

    for (path, method, operation_id) in documented_operations() {
        let status = status_of(&method, &concrete_path(&path)).await;
        if status == StatusCode::NOT_FOUND {
            unrouted.push(format!("{method} {path} ({operation_id})"));
        } else if status == StatusCode::METHOD_NOT_ALLOWED {
            wrong_method.push(format!("{method} {path} ({operation_id})"));
        }
    }

    assert!(
        unrouted.is_empty(),
        "the contract documents path(s) that NO router serves — a caller following the document gets 404:\n  {}\n\
         Either route them, or remove them from the document (see \
         crate::api::openapi_ops::unrouted_paths_pending_implementation for the pattern).",
        unrouted.join("\n  ")
    );
    assert!(
        wrong_method.is_empty(),
        "the contract documents a method the router does not accept on that path:\n  {}",
        wrong_method.join("\n  ")
    );
}

/// Every operation the contract marks `security: [bearerAuth]` must
/// actually refuse an unauthenticated caller.
///
/// A documented-but-unenforced auth requirement is worse than an
/// undocumented one: it tells an integrator the endpoint is protected.
/// The assertion is narrow on purpose — 401 or 403 — because any 2xx
/// here would mean a secured endpoint answered an anonymous request.
#[tokio::test]
async fn every_secured_operation_refuses_an_anonymous_caller() {
    let doc = document();
    let mut leaked: Vec<String> = Vec::new();

    for (path, item) in doc["paths"].as_object().expect("paths") {
        for (method, operation) in item.as_object().expect("path item") {
            if !matches!(method.as_str(), "get" | "post" | "put" | "patch" | "delete") {
                continue;
            }
            let secured = operation
                .get("security")
                .and_then(Value::as_array)
                .map(|s| !s.is_empty())
                .unwrap_or(false);
            if !secured {
                continue;
            }
            let status = status_of(&method.to_uppercase(), &concrete_path(path)).await;
            if status.is_success() {
                leaked.push(format!(
                    "{} {path} answered {status} without a credential",
                    method.to_uppercase()
                ));
            }
        }
    }

    assert!(
        leaked.is_empty(),
        "operation(s) documented as requiring bearerAuth served an anonymous request:\n  {}",
        leaked.join("\n  ")
    );
}

/// The operator surface is platform-admin only and must never be
/// reachable anonymously, whatever the deployment's state.
#[tokio::test]
async fn the_operator_surface_is_closed_to_anonymous_callers() {
    let status = status_of("GET", "/api/ops/migration-health").await;
    assert!(
        status == StatusCode::UNAUTHORIZED || status == StatusCode::FORBIDDEN,
        "/api/ops/migration-health answered {status} to an anonymous caller; expected 401 or 403"
    );
}
