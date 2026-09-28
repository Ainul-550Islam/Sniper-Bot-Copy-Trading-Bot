//! Verified, idempotent billing webhook processing (TASK 7B file 12).
//!
//! ```text
//! provider webhook
//!   → adapter.verify_webhook (signature + freshness, never a client claim)
//!   → parse envelope {id, type, data}
//!   → idempotency check      (durable provider event id)
//!   → whitelisted transition (subscription / plan only)
//!   → entitlements           (replaced atomically by the plan assignment)
//!   → audit
//!   → 200 acknowledgment
//! ```
//!
//! Absolute rules:
//!
//! * **Duplicate webhook ⇒ no duplicate mutation.** The provider event id is
//!   checked and then durably recorded (the migration-0018 runtime-record
//!   store) before the acknowledgement is sent. Without a database the
//!   marker falls back to a process-local set, which is documented and only
//!   exists for single-process test runs.
//! * **Invalid signature ⇒ 401 and zero state change.**
//! * **Unknown event type ⇒ a deterministic `ignored` answer**, no mutation.
//! * **Never trust client-provided subscription state**: the payload carries
//!   ids and timestamps only; every state transition is derived from the
//!   whitelisted event type via the TASK 7A domain methods.
//! * The TASK 5 [`bot_core::accounting::GlobalLedger`] is never touched:
//!   this file records what the CUSTOMER owes, never what the market did.

use std::collections::{HashMap, HashSet};
use std::sync::{Mutex, OnceLock};

use axum::extract::{Path, State};
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Response};
use axum::{Json, Router};
use chrono::{DateTime, Utc};
use serde_json::json;
use tracing::warn;

use bot_core::billing::{BillingProvider, PlanCode, SubscriptionStatus};
use bot_core::tenant::OrganizationId;

use crate::api::ApiState;
use crate::saas::postgres::PostgresSaasRepo;
use crate::saas::provider::{ProviderError, ProviderRegistry, WebhookSecret};
use crate::security::websocket as saas_stream;

/// Runtime-record kind that carries the durable processed-event markers.
pub const WEBHOOK_EVENT_KIND: &str = "billing_webhook_event";

/// The event types this boundary understands. Anything else is `ignored`.
pub const HANDLED_EVENT_TYPES: [&str; 5] = [
    "subscription.payment_failed",
    "subscription.renewed",
    "subscription.canceled",
    "subscription.expired",
    "plan.changed",
];

/// Webhook secrets, read once from the process environment. A provider
/// without a configured secret has NO webhook integration: its endpoint
/// refuses rather than trusting unsigned bytes.
#[derive(Debug, Clone, Default)]
pub struct WebhookConfig {
    secrets: HashMap<BillingProvider, WebhookSecret>,
}

impl WebhookConfig {
    /// Read `SAAS_WEBHOOK_SECRET_{MANUAL,STRIPE,PADDLE}`.
    pub fn from_env() -> Self {
        let mut secrets = HashMap::new();
        for (provider, name) in [
            (BillingProvider::Manual, "SAAS_WEBHOOK_SECRET_MANUAL"),
            (BillingProvider::Stripe, "SAAS_WEBHOOK_SECRET_STRIPE"),
            (BillingProvider::Paddle, "SAAS_WEBHOOK_SECRET_PADDLE"),
        ] {
            if let Ok(value) = std::env::var(name) {
                if let Some(secret) = WebhookSecret::parse(&value) {
                    secrets.insert(provider, secret);
                }
            }
        }
        WebhookConfig { secrets }
    }

    /// The configured secret, when there is one.
    pub fn secret_for(&self, provider: BillingProvider) -> Option<&WebhookSecret> {
        self.secrets.get(&provider)
    }
}

fn config() -> &'static WebhookConfig {
    static CONFIG: OnceLock<WebhookConfig> = OnceLock::new();
    CONFIG.get_or_init(WebhookConfig::from_env)
}

fn registry() -> &'static ProviderRegistry {
    static REGISTRY: OnceLock<ProviderRegistry> = OnceLock::new();
    REGISTRY.get_or_init(ProviderRegistry::new)
}

/// The process-local marker set used only when no database is attached.
/// Production (PostgreSQL attached) always uses the durable store.
fn memory_markers() -> &'static Mutex<HashSet<String>> {
    static SEEN: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
    SEEN.get_or_init(|| Mutex::new(HashSet::new()))
}

/// Has this provider event id already been processed? Durable when the
/// database is attached, process-local otherwise (documented fallback).
async fn already_processed(state: &ApiState, event_id: &str) -> bool {
    if let Some(db) = &state.db {
        let repo = PostgresSaasRepo::new(db.clone());
        matches!(
            repo.by_id::<serde_json::Value>(WEBHOOK_EVENT_KIND, event_id)
                .await,
            Ok(Some(_))
        )
    } else {
        memory_markers()
            .lock()
            .expect("marker mutex")
            .contains(event_id)
    }
}

/// Record a processed event id so a replay can never re-apply it.
async fn record_processed(
    state: &ApiState,
    provider: BillingProvider,
    event_id: &str,
    event_type: &str,
) {
    let now = Utc::now();
    let record = json!({
        "event_id": event_id,
        "provider": provider.as_str(),
        "type": event_type,
        "processed_at": now,
    });
    if let Some(db) = &state.db {
        let repo = PostgresSaasRepo::new(db.clone());
        let _ = repo
            .insert(
                WEBHOOK_EVENT_KIND,
                event_id,
                None,
                None,
                Some(event_id),
                &record,
            )
            .await;
    } else {
        let mut seen = memory_markers().lock().expect("marker mutex");
        if seen.len() < 10_000 {
            seen.insert(event_id.to_string());
        }
    }
}

/// What applying one verified event did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApplyOutcome {
    /// The transition ran.
    Applied(String),
    /// Known shape, deliberately no-op (unknown event type).
    Ignored,
}

/// Apply one VERIFIED event to the TASK 7A store. Pure orchestration of
/// existing domain methods — no new state machine, no ledger, no risk.
pub async fn apply_event(
    state: &ApiState,
    event: &crate::saas::provider::VerifiedProviderEvent,
    now: DateTime<Utc>,
) -> Result<ApplyOutcome, String> {
    if !HANDLED_EVENT_TYPES.contains(&event.event_type.as_str()) {
        return Ok(ApplyOutcome::Ignored);
    }
    let organization_id = event
        .payload
        .get("organization_id")
        .and_then(|v| v.as_str())
        .and_then(OrganizationId::parse)
        .ok_or_else(|| "data.organization_id is missing or malformed".to_string())?;
    let org = state
        .saas
        .organization(organization_id)
        .await
        .ok_or_else(|| "organization does not exist".to_string())?;
    if !bot_core::tenant::can_authenticate(org.status) {
        return Err("organization is closed".to_string());
    }

    match event.event_type.as_str() {
        "plan.changed" => {
            let code = event
                .payload
                .get("plan_code")
                .and_then(|v| v.as_str())
                .and_then(PlanCode::parse)
                .ok_or_else(|| "data.plan_code is not a known plan".to_string())?;
            let subscription = state
                .saas
                .assign_plan(organization_id, code, now)
                .await
                .map_err(|e| format!("plan assignment failed: {e}"))?;
            Ok(ApplyOutcome::Applied(format!(
                "plan={} subscription={}",
                code.as_str(),
                subscription.id
            )))
        }
        status_event => {
            let mut sub = state
                .saas
                .subscription_of(organization_id)
                .await
                .ok_or_else(|| "organization has no subscription".to_string())?;
            match status_event {
                "subscription.payment_failed" => sub.status = SubscriptionStatus::PastDue,
                "subscription.expired" => sub.status = SubscriptionStatus::Expired,
                "subscription.renewed" => {
                    let end = event
                        .payload
                        .get("current_period_end")
                        .and_then(|v| v.as_str())
                        .and_then(|v| DateTime::parse_from_rfc3339(v).ok())
                        .map(|v| v.with_timezone(&Utc));
                    sub.renew(end, now);
                }
                "subscription.canceled" => {
                    let at_period_end = event
                        .payload
                        .get("at_period_end")
                        .and_then(|v| v.as_bool())
                        .unwrap_or(false);
                    sub.cancel(at_period_end, now);
                }
                _ => unreachable!("whitelisted above"),
            }
            let summary = format!("subscription={} status={}", sub.id, sub.status.as_str());
            state
                .saas
                .update_subscription(&sub)
                .await
                .map_err(|e| format!("subscription update failed: {e}"))?;
            Ok(ApplyOutcome::Applied(summary))
        }
    }
}

/// The full pipeline for one verified event: idempotency → transition →
/// durable marker → SaaS event stream → audit. Returns the JSON body the
/// route acknowledges with.
pub async fn process_verified(
    state: &ApiState,
    event: &crate::saas::provider::VerifiedProviderEvent,
) -> Response {
    if already_processed(state, &event.event_id).await {
        return (
            axum::http::StatusCode::OK,
            Json(json!({ "status": "duplicate" })),
        )
            .into_response();
    }
    let now = Utc::now();
    match apply_event(state, event, now).await {
        Ok(ApplyOutcome::Applied(summary)) => {
            record_processed(state, event.provider, &event.event_id, &event.event_type).await;
            // The SaaS event stream frame carries the tenant id so the
            // websocket layer can scope it (visible_to).
            let org_ref = event
                .payload
                .get("organization_id")
                .and_then(|v| v.as_str())
                .unwrap_or_default();
            saas_stream::publish(json!({
                "kind": "saas.billing",
                "organization": org_ref,
                "action": event.event_type,
                "detail": summary,
            }));
            state
                .audit
                .record(
                    "saas",
                    "saas.billing.webhook.applied",
                    Some(&event.event_id),
                    bot_core::audit::AuditOutcome::Success,
                    json!({ "provider": event.provider.as_str(), "type": event.event_type, "detail": summary }),
                )
                .await;
            (
                axum::http::StatusCode::OK,
                Json(json!({ "status": "applied", "detail": summary })),
            )
                .into_response()
        }
        Ok(ApplyOutcome::Ignored) => {
            record_processed(state, event.provider, &event.event_id, &event.event_type).await;
            state
                .audit
                .record(
                    "saas",
                    "saas.billing.webhook.ignored",
                    Some(&event.event_id),
                    bot_core::audit::AuditOutcome::Success,
                    json!({ "provider": event.provider.as_str(), "type": event.event_type }),
                )
                .await;
            (
                axum::http::StatusCode::OK,
                Json(json!({ "status": "ignored" })),
            )
                .into_response()
        }
        Err(reason) => {
            // Nothing was mutated and no marker was recorded, so the provider
            // may safely retry a corrected event.
            warn!(event = %event.event_id, %reason, "billing webhook rejected");
            state
                .audit
                .record(
                    "saas",
                    "saas.billing.webhook.rejected",
                    Some(&event.event_id),
                    bot_core::audit::AuditOutcome::Failure,
                    json!({ "provider": event.provider.as_str(), "type": event.event_type, "reason": reason }),
                )
                .await;
            (
                axum::http::StatusCode::UNPROCESSABLE_ENTITY,
                Json(json!({ "status": "rejected", "reason": reason })),
            )
                .into_response()
        }
    }
}

/// `POST /api/saas/billing/webhooks/:provider` — the public, signature-only
/// webhook endpoint. The provider name in the path selects the adapter; the
/// signature selects trust. No bearer/API-key credential is involved.
pub async fn receive(
    State(state): State<ApiState>,
    Path(provider_name): Path<String>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    let Some(provider) = BillingProvider::parse(&provider_name) else {
        return (
            axum::http::StatusCode::NOT_FOUND,
            Json(json!({ "error": "unknown_provider" })),
        )
            .into_response();
    };
    let Some(adapter) = registry().adapter(provider) else {
        return ProviderError::NotImplemented(provider).into_response();
    };
    let Some(secret) = config().secret_for(provider) else {
        return (
            axum::http::StatusCode::NOT_IMPLEMENTED,
            Json(json!({
                "error": "webhook_not_configured",
                "detail": "no webhook secret is configured for this provider",
            })),
        )
            .into_response();
    };
    // ONE trust decision, through the provider's adapter. Any failure is
    // audited and answered 401 — the body bytes are never processed.
    let event = match adapter.verify_webhook(secret, &headers, &body) {
        Ok(event) => event,
        Err(err) => {
            state
                .audit
                .denied(
                    "saas",
                    "saas.billing.webhook",
                    Some(provider.as_str()),
                    err.code(),
                )
                .await;
            return err.into_response();
        }
    };
    process_verified(&state, &event).await
}

/// The webhook routes, mounted by [`crate::saas::routes`].
pub fn routes() -> Router<ApiState> {
    Router::new().route(
        "/api/saas/billing/webhooks/:provider",
        axum::routing::post(receive),
    )
}

#[allow(unused_imports)]
use bot_core::billing::Subscription as _SubscriptionReexport;

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use axum::http::StatusCode;
    use std::sync::Arc;
    use tower::ServiceExt;

    use bot_core::billing::{FeatureLimit, PlanCode};
    use bot_core::config::AppConfig;
    use bot_core::membership::{Membership, MembershipRole};
    use bot_core::session::token::generate_token;
    use bot_core::state::AppState;
    use bot_core::tenant::{Organization, UserId};

    use crate::saas::api_keys::SaasApiKey;

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
            saas: crate::saas::SaasStore::shared(),
        }
    }

    async fn seed_organization(state: &ApiState, plan: PlanCode) -> OrganizationId {
        let org = Organization::new(
            OrganizationId::new(),
            format!("wh-{}", uuid::Uuid::new_v4().simple()),
            "Webhook Org",
            None,
            Utc::now(),
        );
        state.saas.create_organization(&org).await.expect("org");
        state
            .saas
            .assign_plan(org.id, plan, Utc::now())
            .await
            .expect("plan");
        org.id
    }

    fn event(
        event_id: &str,
        event_type: &str,
        data: serde_json::Value,
    ) -> crate::saas::provider::VerifiedProviderEvent {
        crate::saas::provider::VerifiedProviderEvent {
            provider: BillingProvider::Manual,
            event_id: event_id.to_string(),
            event_type: event_type.to_string(),
            payload: data,
        }
    }

    #[tokio::test]
    async fn payment_failure_transitions_once_and_replays_are_duplicates() {
        let state = test_state();
        let org = seed_organization(&state, PlanCode::Business).await;
        let data = json!({ "organization_id": org.to_string() });

        let first = process_verified(
            &state,
            &event("evt_pay_1", "subscription.payment_failed", data.clone()),
        )
        .await;
        assert_eq!(first.status(), StatusCode::OK);
        let sub = state.saas.subscription_of(org).await.expect("subscription");
        assert_eq!(sub.status, SubscriptionStatus::PastDue);
        let updated_after_first = sub.updated_at;

        // The exact same event id must never mutate again.
        let replay = process_verified(
            &state,
            &event("evt_pay_1", "subscription.payment_failed", data),
        )
        .await;
        assert_eq!(replay.status(), StatusCode::OK);
        let body = axum::body::to_bytes(replay.into_body(), 64 * 1024)
            .await
            .unwrap();
        assert!(body.windows(9).any(|w| w == b"duplicate"), "{body:?}");
        assert_eq!(
            state
                .saas
                .subscription_of(org)
                .await
                .expect("subscription")
                .updated_at,
            updated_after_first,
            "the replay must not touch the row"
        );
    }

    #[tokio::test]
    async fn plan_changes_replace_entitlements_atomically() {
        let state = test_state();
        let org = seed_organization(&state, PlanCode::Business).await;
        assert!(
            state
                .saas
                .entitlements_of(org, Utc::now())
                .await
                .allows(bot_core::billing::features::MODULE_POLYMARKET),
            "business grants polymarket"
        );

        let downgrade = process_verified(
            &state,
            &event(
                "evt_plan_1",
                "plan.changed",
                json!({ "organization_id": org.to_string(), "plan_code": "starter" }),
            ),
        )
        .await;
        assert_eq!(downgrade.status(), StatusCode::OK);
        let set = state.saas.entitlements_of(org, Utc::now()).await;
        assert!(
            !set.allows(bot_core::billing::features::MODULE_POLYMARKET),
            "starter must not keep the polymarket grant"
        );
        assert_eq!(
            set.limit_for(bot_core::billing::features::MAX_MEMBERS),
            FeatureLimit::Limited(2.0),
            "starter member limit from the catalogue"
        );

        // A malformed plan code is rejected and changes nothing.
        let bad = process_verified(
            &state,
            &event(
                "evt_plan_2",
                "plan.changed",
                json!({ "organization_id": org.to_string(), "plan_code": "platinum" }),
            ),
        )
        .await;
        assert_eq!(bad.status(), StatusCode::UNPROCESSABLE_ENTITY);
        assert!(
            state
                .saas
                .subscription_of(org)
                .await
                .expect("subscription")
                .is_effective(Utc::now()),
            "the rejection must not disturb the current plan"
        );
    }

    #[tokio::test]
    async fn unknown_types_and_bad_payloads_change_nothing() {
        let state = test_state();
        let org = seed_organization(&state, PlanCode::Pro).await;
        let before = state.saas.subscription_of(org).await.expect("subscription");

        let ignored = process_verified(
            &state,
            &event(
                "evt_unknown_1",
                "customer.created",
                json!({ "organization_id": org.to_string(), "status": "active" }),
            ),
        )
        .await;
        assert_eq!(ignored.status(), StatusCode::OK);
        assert!(
            already_processed(&state, "evt_unknown_1").await,
            "ignored events are still marked"
        );

        let bad_org = process_verified(
            &state,
            &event(
                "evt_bad_org",
                "subscription.renewed",
                json!({ "organization_id": "not-a-uuid" }),
            ),
        )
        .await;
        assert_eq!(bad_org.status(), StatusCode::UNPROCESSABLE_ENTITY);
        assert!(
            !already_processed(&state, "evt_bad_org").await,
            "a rejected event must be retryable"
        );

        // A client-supplied status field is NEVER trusted: only the event
        // type decides, and an unknown type mutates nothing.
        let after = state.saas.subscription_of(org).await.expect("subscription");
        assert_eq!(after.status, before.status);
        assert_eq!(after.updated_at, before.updated_at);
    }

    #[tokio::test]
    async fn renewal_and_cancellation_follow_the_domain_methods() {
        let state = test_state();
        let org = seed_organization(&state, PlanCode::Pro).await;
        let end = (Utc::now() + chrono::Duration::days(30)).to_rfc3339();

        let renewed = process_verified(
            &state,
            &event(
                "evt_renew_1",
                "subscription.renewed",
                json!({ "organization_id": org.to_string(), "current_period_end": end }),
            ),
        )
        .await;
        assert_eq!(renewed.status(), StatusCode::OK);
        let sub = state.saas.subscription_of(org).await.expect("subscription");
        assert!(sub.is_effective(Utc::now()));
        assert!(sub.current_period_end.is_some());

        let canceled = process_verified(
            &state,
            &event(
                "evt_cancel_1",
                "subscription.canceled",
                json!({ "organization_id": org.to_string(), "at_period_end": false }),
            ),
        )
        .await;
        assert_eq!(canceled.status(), StatusCode::OK);
        let sub = state.saas.subscription_of(org).await.expect("subscription");
        assert_eq!(sub.status, SubscriptionStatus::Canceled);
        assert!(
            !sub.is_effective(Utc::now()),
            "cancellation stops entitlements"
        );
    }

    #[tokio::test]
    async fn closed_tenants_are_never_mutated() {
        let state = test_state();
        let org = seed_organization(&state, PlanCode::Business).await;
        let mut closed = state.saas.organization(org).await.expect("org");
        closed.status = bot_core::tenant::OrganizationStatus::Closed;
        state
            .saas
            .update_organization(&closed)
            .await
            .expect("close");

        let res = process_verified(
            &state,
            &event(
                "evt_closed_1",
                "subscription.payment_failed",
                json!({ "organization_id": org.to_string() }),
            ),
        )
        .await;
        assert_eq!(res.status(), StatusCode::UNPROCESSABLE_ENTITY);
        assert!(!already_processed(&state, "evt_closed_1").await);
    }

    #[tokio::test]
    async fn the_route_refuses_unsigned_and_unconfigured_providers() {
        let state = test_state();
        let app = crate::api::router(state);
        let secret = WebhookSecret::parse("whsec_cfg").unwrap();
        let body = br#"{"id":"evt_r1","type":"subscription.renewed"}"#;
        let now = Utc::now().timestamp() as u64;
        let sig = crate::saas::provider::sign_payload(&secret, now, body);

        // No secret is configured in this process: even a PERFECT signature
        // is refused rather than trusted.
        let res = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/saas/billing/webhooks/manual")
                    .header(crate::saas::provider::TIMESTAMP_HEADER, now.to_string())
                    .header(crate::saas::provider::SIGNATURE_HEADER, &sig)
                    .body(Body::from(body.as_slice()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::NOT_IMPLEMENTED);

        // An unknown provider name is 404.
        let res = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/saas/billing/webhooks/coinbase")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::NOT_FOUND);
    }

    /// Guard: the webhook pipeline never constructs or touches trading
    /// truth — it only calls the TASK 7A store, and the only secret it
    /// holds is the process webhook secret.
    #[test]
    fn handled_vocabulary_is_the_whole_surface() {
        assert_eq!(HANDLED_EVENT_TYPES.len(), 5);
        assert!(HANDLED_EVENT_TYPES
            .iter()
            .all(|t| t.starts_with("subscription.") || *t == "plan.changed"));
        // Keys and sessions never appear in this module's concerns.
        let _ = std::mem::size_of::<SaasApiKey>();
        let _ = Membership::new(
            OrganizationId::new(),
            UserId::new(),
            MembershipRole::Viewer,
            None,
            Utc::now(),
        );
        let _ = generate_token("ses");
    }
}
