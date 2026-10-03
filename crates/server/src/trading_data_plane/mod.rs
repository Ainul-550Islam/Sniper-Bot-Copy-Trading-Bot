//! Tenant trading data plane (PROMPT 3/10 #60 consolidated by #68).
//!
//! The authenticated, tenant-scoped trading HTTP surface. Every
//! handler:
//!
//! 1. authenticates through [`crate::saas::middleware::authorize_request`]
//!    (no handler may run without a [`SaasContext`]);
//! 2. builds the tenant scopes from the context's organization;
//! 3. calls ONLY the `bot-core` `trading_repository` repositories —
//!    every SQL predicate carries `organization_id` there;
//! 4. answers `503` when the database (and therefore the plane) is
//!    not attached.
//!
//! The legacy deployment API (`/api/orders`, `/api/positions`, …)
//! stays untouched beside these routes — that is the operator plane,
//! bound to the deployment organization.

pub mod authorization_chain;
pub mod bots;
pub mod copy;
pub mod executions;
pub mod module_control_store;
pub mod module_controls;
pub mod orders;
pub mod polymarket;
pub mod positions;
pub mod recovery;
pub mod service;
pub mod sniper;
pub mod telegram;

use axum::routing::get;
use axum::Router;

pub use service::TenantTradingDataPlane;

use crate::api::ApiState;

/// The tenant trading data-plane routes.
pub fn routes() -> Router<ApiState> {
    Router::new()
        // --- bots (§H customer surface) ------------------------------------
        .route("/api/tenant/bots", get(bots::list))
        .route("/api/tenant/bots/:module", get(bots::detail))
        // --- orders -------------------------------------------------------
        .route("/api/tenant/orders", get(orders::list))
        .route("/api/tenant/orders/:id", get(orders::get_one))
        .route("/api/tenant/orders/:id/cancel", post(orders::cancel))
        // --- executions / transactions ------------------------------------
        .route("/api/tenant/executions", get(executions::list))
        .route("/api/tenant/executions/:id", get(executions::get_one))
        .route(
            "/api/tenant/transactions/:signature",
            get(executions::transaction),
        )
        // --- positions / trades / balances --------------------------------
        .route("/api/tenant/positions", get(positions::list))
        .route("/api/tenant/positions/:id", get(positions::get_one))
        .route("/api/tenant/trades", get(positions::trades))
        .route(
            "/api/tenant/trades/:position_id",
            get(positions::trades_for_position),
        )
        .route("/api/tenant/balances", get(positions::balances))
        // --- copy trading --------------------------------------------------
        // --- module controls/status (§J 74–77) ------------------------------
        .route("/api/tenant/sniper/status", get(sniper::status))
        .route("/api/tenant/sniper/controls", post(sniper::controls))
        .route("/api/tenant/copy/status", get(copy::status))
        .route("/api/tenant/copy/controls", post(copy::controls))
        .route("/api/tenant/polymarket/status", get(polymarket::status))
        .route(
            "/api/tenant/polymarket/controls",
            post(polymarket::controls),
        )
        .route("/api/tenant/telegram/status", get(telegram::status))
        .route(
            "/api/tenant/telegram/binding",
            axum::routing::put(telegram::bind).delete(telegram::unbind),
        )
        .route("/api/tenant/copy/leaders", get(copy::leaders))
        .route("/api/tenant/copy/leaders/:address", get(copy::leader))
        .route("/api/tenant/copy/links", get(copy::open_links))
        // --- polymarket ----------------------------------------------------
        .route("/api/tenant/polymarket/orders", get(polymarket::orders))
        .route(
            "/api/tenant/polymarket/orders/:venue_order_id",
            get(polymarket::order),
        )
        .route("/api/tenant/polymarket/fills", get(polymarket::fills))
        .route(
            "/api/tenant/polymarket/reconciliation",
            get(polymarket::reconciliation),
        )
        // --- recovery ------------------------------------------------------
        .route("/api/tenant/recovery/intents", get(recovery::intents))
        .route("/api/tenant/recovery/sweep", get(recovery::sweep))
        // --- reporting -----------------------------------------------------
        .route("/api/tenant/reports/summary", get(recovery::summary))
        .route("/api/tenant/reports/pnl", get(recovery::pnl))
}

use axum::routing::post;

#[cfg(test)]
mod tests {
    //! Route-level tenant-isolation tests for the data plane
    //! (`tower::oneshot` against the REAL router). The PG-backed tests
    //! are gated on `POSTGRES_URL` exactly like the `bot-core`
    //! isolation suites; the 503 test runs fully in memory.

    use super::*;
    use crate::api::ApiState;
    use crate::saas::middleware::AUTH_HEADER;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use bot_core::billing::PlanCode;
    use bot_core::config::{AppConfig, DatabaseConfig};
    use bot_core::db::Database;
    use bot_core::membership::{Membership, MembershipRole};
    use bot_core::session::model::SessionRecord;
    use bot_core::session::token::{generate_token, hash_password};
    use bot_core::state::AppState;
    use bot_core::tenant::{Organization, OrganizationId, User, UserStatus};
    use http_body_util::BodyExt;
    use std::sync::Arc;
    use tower::ServiceExt;

    /// The gated connection (None ⇒ PG-backed tests report NOT_RUN).
    async fn pg() -> Option<Arc<Database>> {
        let url = std::env::var("POSTGRES_URL")
            .ok()
            .filter(|s| !s.trim().is_empty())?;
        let cfg = DatabaseConfig {
            enabled: true,
            auto_migrate: true,
            ..Default::default()
        };
        let db = Database::connect(&cfg, &url)
            .await
            .expect("configured database must connect");
        db.migrate().await.expect("migrations must apply");
        Some(Arc::new(db))
    }

    /// An `ApiState` with an optional attached trading plane.
    fn state(db: Option<Arc<Database>>) -> ApiState {
        let shared = AppState::new(AppConfig::from_defaults());
        ApiState {
            audit: bot_core::audit::AuditTrail::new(None, shared.events.clone()),
            shared,
            api_key: None,
            auth: None,
            limiter: bot_core::auth::RateLimiter::new(0),
            db: db.clone(),
            journal: None,
            serve_dashboard: false,
            health: Arc::new(bot_core::obs::health::HealthRegistry::new()),
            metrics_enabled: false,
            saas: crate::saas::SaasStore::shared(),
            module_registry: std::sync::Arc::new(
                crate::module_runtime::module_registry::TenantModuleRegistry::new(),
            ),
            trading: db.map(|db| Arc::new(TenantTradingDataPlane::new(db))),
        }
    }

    /// One tenant: org + owner user + session in the (in-memory) SaaS
    /// store AND a matching `organizations` row in PostgreSQL when a
    /// database is attached (the trading-truth FKs point there).
    async fn tenant(
        st: &ApiState,
        db: Option<&Arc<Database>>,
        slug: &str,
    ) -> (OrganizationId, String) {
        let org = Organization::new(
            OrganizationId::new(),
            format!("{slug}-{}", uuid::Uuid::new_v4().simple()),
            slug,
            None,
            chrono::Utc::now(),
        );
        st.saas.create_organization(&org).await.expect("org");
        st.saas
            .assign_plan(org.id, PlanCode::Business, chrono::Utc::now())
            .await
            .expect("plan");
        if let Some(db) = db {
            sqlx::query(
                r#"INSERT INTO organizations (id, slug, name, status)
                   VALUES ($1, $2, $3, 'active')"#,
            )
            .bind(org.id.as_uuid())
            .bind(&org.slug)
            .bind(&org.name)
            .execute(db.pool())
            .await
            .expect("pg organization row");
        }
        let user = User {
            id: bot_core::tenant::UserId::new(),
            email: format!("{slug}@example.com"),
            email_verified: true,
            display_name: slug.into(),
            password_hash: hash_password("password-123456"),
            status: UserStatus::Active,
            platform_admin: false,
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
            last_login_at: None,
        };
        st.saas.create_user(&user).await.expect("user");
        st.saas
            .create_membership(&Membership::new(
                org.id,
                user.id,
                MembershipRole::OrgOwner,
                None,
                chrono::Utc::now(),
            ))
            .await
            .expect("membership");
        let t = generate_token("ses");
        st.saas
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
        (org.id, t.plaintext)
    }

    async fn call(
        app: axum::Router,
        method: &str,
        uri: &str,
        token: Option<&str>,
        org: Option<&OrganizationId>,
    ) -> (StatusCode, serde_json::Value) {
        let mut builder = Request::builder().method(method).uri(uri);
        if let Some(t) = token {
            builder = builder.header(AUTH_HEADER, format!("Bearer {t}"));
        }
        if let Some(o) = org {
            builder = builder.header("x-organization", o.to_string());
        }
        let res = app
            .oneshot(builder.body(Body::empty()).unwrap())
            .await
            .unwrap();
        let status = res.status();
        let bytes = BodyExt::collect(res.into_body()).await.unwrap().to_bytes();
        (
            status,
            serde_json::from_slice(&bytes).unwrap_or(serde_json::json!({})),
        )
    }

    fn run() -> String {
        format!(
            "{}-{}",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)
        )
    }

    /// Percent-encode a query parameter (the opaque cursor is JSON and
    /// carries `{`, `}`, `"` — illegal raw URI characters).
    fn pct(s: &str) -> String {
        let mut out = String::with_capacity(s.len());
        for b in s.bytes() {
            match b {
                b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                    out.push(b as char)
                }
                _ => out.push_str(&format!("%{b:02X}")),
            }
        }
        out
    }

    #[tokio::test]
    async fn orders_routes_never_cross_tenants() {
        let Some(db) = pg().await else {
            eprintln!("NOT_RUN: trading_data_plane route tests — POSTGRES_URL missing");
            return;
        };
        let run = run();
        let st = state(Some(db.clone()));
        let (a, token_a) = tenant(&st, Some(&db), &format!("a-{run}")).await;
        let (b, token_b) = tenant(&st, Some(&db), &format!("b-{run}")).await;
        let app = crate::api::router(st);

        // Seed TWO orders for A (two rows guarantee a page cursor).
        for i in 0..2 {
            sqlx::query(
                r#"INSERT INTO orders
                       (organization_id, id, idempotency_key, module, side, symbol,
                        venue, mode, status, qty)
                   VALUES ($1, $2, NULL, 'sniper', 'buy', 'SOL/USDC', 'paper',
                           'paper', 'submitted', 1.0)
                   ON CONFLICT (id) DO NOTHING"#,
            )
            .bind(a.as_uuid())
            .bind(format!("ord-http-{i}-{run}"))
            .execute(db.pool())
            .await
            .expect("seed order");
        }
        let target_id = format!("ord-http-0-{run}");

        // Each list answers for its own tenant only.
        let (status, body) = call(
            app.clone(),
            "GET",
            "/api/tenant/orders",
            Some(&token_a),
            Some(&a),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["organization_id"], serde_json::json!(a.to_string()));
        let items = body["items"].as_array().expect("items");
        assert!(items
            .iter()
            .any(|o| o["id"] == serde_json::json!(target_id)));

        let (status, body) = call(
            app.clone(),
            "GET",
            "/api/tenant/orders",
            Some(&token_b),
            Some(&b),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["organization_id"], serde_json::json!(b.to_string()));
        assert!(
            !body["items"]
                .as_array()
                .expect("items")
                .iter()
                .any(|o| o["id"] == serde_json::json!(target_id)),
            "B's list must never contain A's order"
        );

        // Single-order read: 200 for the owner, 404 (not 403) for B —
        // no existence leak.
        let (status, _) = call(
            app.clone(),
            "GET",
            &format!("/api/tenant/orders/{target_id}"),
            Some(&token_a),
            Some(&a),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let (status, body) = call(
            app.clone(),
            "GET",
            &format!("/api/tenant/orders/{target_id}"),
            Some(&token_b),
            Some(&b),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
        assert_eq!(body["error"], serde_json::json!("not_found"));

        // Cancel: B's attempt is a 404 and must not change A's order.
        let (status, _) = call(
            app.clone(),
            "POST",
            &format!("/api/tenant/orders/{target_id}/cancel"),
            Some(&token_b),
            Some(&b),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        let (status, body) = call(
            app.clone(),
            "GET",
            &format!("/api/tenant/orders/{target_id}"),
            Some(&token_a),
            Some(&a),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["order"]["status"], serde_json::json!("submitted"));

        // A cancels its own order.
        let (status, body) = call(
            app.clone(),
            "POST",
            &format!("/api/tenant/orders/{target_id}/cancel"),
            Some(&token_a),
            Some(&a),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        let (status, body) = call(
            app.clone(),
            "GET",
            &format!("/api/tenant/orders/{target_id}"),
            Some(&token_a),
            Some(&a),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["order"]["status"], serde_json::json!("cancelled"));

        // A cursor minted by A is rejected for B at the HTTP layer.
        let (status, body) = call(
            app.clone(),
            "GET",
            "/api/tenant/orders?limit=1",
            Some(&token_a),
            Some(&a),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        let cursor = body["next_cursor"]
            .as_str()
            .expect("cursor minted")
            .to_string();
        let (status, body) = call(
            app.clone(),
            "GET",
            &format!("/api/tenant/orders?limit=1&cursor={}", pct(&cursor)),
            Some(&token_b),
            Some(&b),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
        assert_eq!(body["error"], serde_json::json!("invalid_pagination"));

        // Unauthenticated calls never reach the plane.
        let (status, _) = call(app.clone(), "GET", "/api/tenant/orders", None, None).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn plane_answers_503_without_database() {
        let st = state(None);
        let (org, token) = tenant(&st, None, "no-db").await;
        let app = crate::api::router(st);
        for (method, uri) in [
            ("GET", "/api/tenant/orders"),
            ("GET", "/api/tenant/positions"),
            ("GET", "/api/tenant/copy/leaders"),
            ("GET", "/api/tenant/polymarket/orders"),
            ("GET", "/api/tenant/recovery/intents"),
            ("GET", "/api/tenant/reports/summary"),
        ] {
            let (status, body) = call(app.clone(), method, uri, Some(&token), Some(&org)).await;
            assert_eq!(
                status,
                StatusCode::SERVICE_UNAVAILABLE,
                "{method} {uri}: {body}"
            );
            assert_eq!(
                body["error"],
                serde_json::json!("trading_data_plane_unavailable")
            );
        }
    }

    /// §H — the authorization chain refuses a SUSPENDED tenant before
    /// any data access, with the lifecycle reason in the body.
    #[tokio::test]
    async fn suspended_tenant_is_refused_before_data_access() {
        let Some(db) = pg().await else {
            eprintln!("NOT_RUN: suspended lifecycle guard — POSTGRES_URL missing");
            return;
        };
        let st = state(Some(Arc::clone(&db)));
        let (org, token) = tenant(&st, Some(&db), &run()).await;
        let mut row = st.saas.organization(org).await.expect("org row");
        row.status = bot_core::tenant::OrganizationStatus::Suspended;
        st.saas.update_organization(&row).await.expect("suspended");
        let app = crate::api::router(st);
        for uri in [
            "/api/tenant/orders",
            "/api/tenant/positions",
            "/api/tenant/copy/leaders",
            "/api/tenant/bots",
        ] {
            let (status, body) = call(app.clone(), "GET", uri, Some(&token), Some(&org)).await;
            assert_eq!(status, StatusCode::FORBIDDEN, "{uri}: {body}");
            assert_eq!(body["error"], serde_json::json!("tenant_lifecycle_blocked"));
        }
    }

    /// §H — the module-family entitlement gate fails closed: a Starter
    /// tenant (sniper only) may read core trading data but is refused on
    /// the copy and polymarket families.
    #[tokio::test]
    async fn module_family_entitlements_are_enforced_fail_closed() {
        let Some(db) = pg().await else {
            eprintln!("NOT_RUN: module entitlement guard — POSTGRES_URL missing");
            return;
        };
        let st = state(Some(Arc::clone(&db)));
        let (org, token) = tenant(&st, Some(&db), &run()).await;
        // Downgrade to Starter: sniper enabled, copy/polymarket disabled.
        st.saas
            .assign_plan(org, PlanCode::Starter, chrono::Utc::now())
            .await
            .expect("starter plan");
        let app = crate::api::router(st);

        let (status, body) = call(
            app.clone(),
            "GET",
            "/api/tenant/orders",
            Some(&token),
            Some(&org),
        )
        .await;
        assert_eq!(
            status,
            StatusCode::OK,
            "core trading is granted by sniper: {body}"
        );

        for uri in ["/api/tenant/copy/leaders", "/api/tenant/polymarket/orders"] {
            let (status, body) = call(app.clone(), "GET", uri, Some(&token), Some(&org)).await;
            assert_eq!(status, StatusCode::FORBIDDEN, "{uri}: {body}");
            assert_eq!(body["error"], serde_json::json!("module_not_entitled"));
        }
    }

    /// §H — the bots surface lists ONLY the caller's registered runtimes.
    #[tokio::test]
    async fn bots_routes_are_tenant_scoped() {
        let Some(db) = pg().await else {
            eprintln!("NOT_RUN: bots tenant scoping — POSTGRES_URL missing");
            return;
        };
        let st = state(Some(Arc::clone(&db)));
        let (org_a, token_a) = tenant(&st, Some(&db), &format!("{}-a", run())).await;
        let (org_b, token_b) = tenant(&st, Some(&db), &format!("{}-b", run())).await;

        // Register one runtime per org through the §A execution-context
        // path (the only public instance constructor).
        use crate::module_runtime::module_handle::ModuleHandle;
        use crate::module_runtime::module_lifecycle::ModulePhase;
        use bot_core::execution::{
            AuthorityChecklist, ExecutionTrace, TenantExecutionContext, AUTHORITY_CHECK_ORDER,
        };
        use bot_core::models::{BotModule, ExecutionMode};
        use bot_core::tenant::{RuntimeGeneration, RuntimeId, TenantSignerRef, TenantWalletRef};

        let instance = |org: OrganizationId, module: BotModule| {
            let now = chrono::Utc::now();
            let runtime = RuntimeId::new();
            let generation = RuntimeGeneration::first();
            let scope = bot_core::execution::ExecutionScope::new(
                org,
                runtime,
                generation,
                module,
                ExecutionMode::Paper,
            )
            .expect("scope");
            let mut checklist = AuthorityChecklist::new();
            for name in AUTHORITY_CHECK_ORDER {
                checklist.record(name, now).expect("checklist");
            }
            let authority = checklist.finish(&scope, now).expect("authority");
            let wallet = TenantWalletRef::new(org, "9WzDXwBbmkg8ZTbNMqUxvQRAyrZzDsGYdLVL9zYtAWWM")
                .expect("wallet");
            let signer =
                TenantSignerRef::new(org, bot_core::tenant::SignerProvider::Local, "mod-key")
                    .expect("signer");
            let context = TenantExecutionContext::issue(
                org,
                runtime,
                generation,
                module,
                ExecutionMode::Paper,
                authority,
                wallet,
                signer,
                ExecutionTrace::for_request(),
            )
            .expect("context");
            crate::module_runtime::tenant_module_instance::TenantModuleInstance::from_context(
                &context, true,
            )
        };

        st.module_registry
            .register(ModuleHandle::new(
                instance(org_a, BotModule::Sniper),
                ModulePhase::Running,
                chrono::Utc::now(),
            ))
            .expect("register a");
        st.module_registry
            .register(ModuleHandle::new(
                instance(org_b, BotModule::Copy),
                ModulePhase::Idle,
                chrono::Utc::now(),
            ))
            .expect("register b");

        let app = crate::api::router(st);
        let (status, body) = call(
            app.clone(),
            "GET",
            "/api/tenant/bots",
            Some(&token_a),
            Some(&org_a),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["count"], serde_json::json!(1));
        assert_eq!(body["items"][0]["module"], serde_json::json!("sniper"));
        assert_eq!(body["items"][0]["phase"], serde_json::json!("running"));

        let (status, body) = call(
            app.clone(),
            "GET",
            "/api/tenant/bots",
            Some(&token_b),
            Some(&org_b),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["count"], serde_json::json!(1));
        assert_eq!(body["items"][0]["module"], serde_json::json!("copy"));

        // Detail by module: A's sniper exists; A's copy is an honest
        // empty (registered for B only — never leaked).
        let (status, body) = call(
            app.clone(),
            "GET",
            "/api/tenant/bots/sniper",
            Some(&token_a),
            Some(&org_a),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["bot"]["module"], serde_json::json!("sniper"));
        let (status, body) = call(
            app.clone(),
            "GET",
            "/api/tenant/bots/copy",
            Some(&token_a),
            Some(&org_a),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert!(body["bot"].is_null(), "B's copy runtime must not leak to A");

        // Unknown module name is a 400, not a 404 oracle.
        let (status, body) = call(
            app.clone(),
            "GET",
            "/api/tenant/bots/telepathy",
            Some(&token_a),
            Some(&org_a),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
        assert_eq!(body["error"], serde_json::json!("unknown_module"));
    }
}
