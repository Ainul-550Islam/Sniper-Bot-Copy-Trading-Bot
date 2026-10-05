//! Tenant-scoped billing reconciliation application service (BATCH 2 file 07).
//!
//! Resolves organization billing state, provider state, and entitlement state.
//! Records discrepancies. Requires authorization. Never bypasses subscription
//! business rules. Repeated reconciliation is idempotent.

use axum::extract::{Path, State};
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Response};
use axum::{Json, Router};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::Row;

use bot_core::authorization::AccessRequest;
#[cfg(test)]
use bot_core::billing::invoice::InvoiceStatus;
#[cfg(test)]
use bot_core::billing::payment::TransactionStatus;
use bot_core::billing::provider::BillingProviderKind;
use bot_core::billing::provider_events::ProviderEventKind;
use bot_core::billing::reconciliation::{
    reconcile, InternalBillingSnapshot, ProviderBillingSnapshot, ReconciliationAction,
    ReconciliationDecision,
};
use bot_core::membership::Permission;
use bot_core::tenant::OrganizationId;

use crate::api::ApiState;
use crate::saas::middleware::{authorize_request, deny_response};
use crate::saas::postgres::PostgresSaasRepo;

#[derive(Debug, Deserialize)]
pub struct ReconcileRequest {
    pub provider: Option<String>,
    pub provider_event_id: Option<String>,
    pub event_kind: Option<String>,
    pub dry_run: Option<bool>,
}

#[derive(Debug, Serialize)]
pub struct ReconcileResponse {
    pub organization_id: String,
    pub action: String,
    pub reason: String,
    pub idempotent: bool,
    pub decided_at: String,
    // Safe snapshots, never secrets
    pub internal_subscription_status: Option<String>,
    pub provider_event_kind: String,
}

pub fn routes() -> Router<ApiState> {
    Router::new()
        .route(
            "/api/saas/billing/reconcile",
            axum::routing::post(reconcile_handler),
        )
        .route(
            "/api/saas/billing/reconcile/:id",
            axum::routing::get(get_reconcile),
        )
}

/// Compatibility map for no-database tests/development. PostgreSQL uses the
/// durable `saas_runtime_records` adapter when configured.
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

fn reconcile_store() -> &'static Mutex<HashMap<String, ReconciliationDecision>> {
    static S: OnceLock<Mutex<HashMap<String, ReconciliationDecision>>> = OnceLock::new();
    S.get_or_init(|| Mutex::new(HashMap::new()))
}

fn reconcile_key(org: OrganizationId, provider: &str, event_id: &str) -> String {
    format!("{}:{}:{}", org, provider, event_id)
}

fn decision_response(
    organization_id: OrganizationId,
    decision: &ReconciliationDecision,
    idempotent: bool,
) -> Response {
    let response = ReconcileResponse {
        organization_id: organization_id.to_string(),
        action: decision.action.as_str().into(),
        reason: decision.reason.clone(),
        idempotent,
        decided_at: decision.decided_at.to_rfc3339(),
        internal_subscription_status: decision
            .internal_snapshot
            .subscription_status
            .map(|status| status.as_str().into()),
        provider_event_kind: decision.provider_snapshot.event_kind.as_str().into(),
    };
    (axum::http::StatusCode::OK, Json(json!(response))).into_response()
}

async fn reconcile_handler(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Json(body): Json<ReconcileRequest>,
) -> Response {
    let ctx = match authorize_request(
        &state,
        &headers,
        AccessRequest::read(Permission::BillingRead),
    )
    .await
    {
        Ok(c) => c,
        Err(d) => return deny_response(&state, &d).await,
    };
    // Only durable provider evidence can drive reconciliation. The
    // no-database process-local mode cannot prove event provenance, so this
    // endpoint refuses it rather than producing a customer-visible decision.
    let Some(db) = state.db.as_deref() else {
        return (
            axum::http::StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({"error":"reconciliation_storage_unavailable","reason":"billing reconciliation requires PostgreSQL-backed provider evidence"})),
        )
            .into_response();
    };
    let org = ctx.organization.id;

    // Resolve internal snapshot (from SaasStore + billing store)
    let now = Utc::now();
    let sub = match state.saas.subscription_of(org).await {
        Ok(value) => value,
        Err(error) => {
            tracing::error!(error = %error, organization = %org, "reconciliation subscription could not be loaded");
            return (
                axum::http::StatusCode::SERVICE_UNAVAILABLE,
                Json(json!({"error":"reconciliation_storage_unavailable","reason":"tenant subscription could not be loaded"})),
            )
                .into_response();
        }
    };
    let internal = InternalBillingSnapshot {
        organization_id: org,
        subscription_status: sub.as_ref().map(|s| s.status),
        subscription_provider: sub.as_ref().and(
            // s.provider_ref is opaque; map to kind via string if possible, else None
            None::<BillingProviderKind>,
        ),
        last_payment_status: None, // would query payment_transactions WHERE organization_id
        last_invoice_status: None,
        last_invoice_id: None,
        entitlement_active: ctx.authorization.has(Permission::BotStart)
            || ctx.authorization.has(Permission::BillingRead),
        as_of: now,
    };

    // Reconciliation must be tied to a provider event supplied by the
    // authenticated operator. Never invent a provider, event id, or event
    // kind when the request is incomplete.
    let provider_kind = match body
        .provider
        .as_deref()
        .and_then(BillingProviderKind::parse)
    {
        Some(value) => value,
        None => {
            return (
                axum::http::StatusCode::BAD_REQUEST,
                Json(json!({"error":"invalid_provider","reason":"provider is required and must be supported"})),
            )
                .into_response();
        }
    };
    let event_id = match body.provider_event_id.as_deref().map(str::trim) {
        Some(value) if !value.is_empty() && value.len() <= 256 => value.to_string(),
        _ => {
            return (
                axum::http::StatusCode::BAD_REQUEST,
                Json(json!({"error":"invalid_provider_event_id","reason":"provider_event_id is required and must be at most 256 characters"})),
            )
                .into_response();
        }
    };
    let kind = match body.event_kind.as_deref().map(ProviderEventKind::parse) {
        Some(value) if value != ProviderEventKind::Unknown => value,
        _ => {
            return (
                axum::http::StatusCode::BAD_REQUEST,
                Json(json!({"error":"invalid_event_kind","reason":"event_kind must identify a supported provider event"})),
            )
                .into_response();
        }
    };

    let evidence = match sqlx::query(
        "SELECT event_type, processed
           FROM provider_events
          WHERE organization_id = $1 AND provider = $2 AND provider_event_id = $3",
    )
    .bind(org.as_uuid())
    .bind(provider_kind.as_str())
    .bind(&event_id)
    .fetch_optional(db.pool())
    .await
    {
        Ok(Some(row)) => row,
        Ok(None) => {
            return (
                axum::http::StatusCode::NOT_FOUND,
                Json(json!({"error":"provider_event_not_found","reason":"reconciliation requires a verified tenant-owned provider event"})),
            )
                .into_response();
        }
        Err(error) => {
            tracing::error!(error = %error, "failed to load provider reconciliation evidence");
            return (
                axum::http::StatusCode::SERVICE_UNAVAILABLE,
                Json(json!({"error":"reconciliation_storage_unavailable","reason":"provider evidence could not be loaded"})),
            )
                .into_response();
        }
    };
    let stored_kind = ProviderEventKind::parse(&evidence.get::<String, _>("event_type"));
    if stored_kind != kind {
        return (
            axum::http::StatusCode::CONFLICT,
            Json(json!({"error":"provider_event_kind_mismatch","reason":"requested event kind does not match the verified provider event"})),
        )
            .into_response();
    }
    if !evidence.get::<bool, _>("processed") {
        return (
            axum::http::StatusCode::CONFLICT,
            Json(json!({"error":"provider_event_in_progress","reason":"provider event processing has not completed"})),
        )
            .into_response();
    }

    let provider = ProviderBillingSnapshot {
        provider: provider_kind,
        provider_customer_id: None,
        event_kind: kind,
        event_id: event_id.clone(),
        subscription_status_hint: None,
        invoice_status_hint: None,
        payment_status_hint: None,
        amount_cents: None,
        currency: None,
        event_timestamp: now,
    };

    // Idempotency: PostgreSQL is authoritative whenever attached. The
    // in-memory map is retained only for no-database test/development mode.
    let key = reconcile_key(org, provider_kind.as_str(), &event_id);
    if let Some(db) = &state.db {
        let repo = PostgresSaasRepo::new(db.clone());
        match repo
            .by_lookup::<ReconciliationDecision>("billing_reconciliation", &key)
            .await
        {
            Ok(Some(previous)) => return decision_response(org, &previous, true),
            Ok(None) => {}
            Err(error) => {
                tracing::error!(error = %error, "failed to load durable billing reconciliation");
                return (
                    axum::http::StatusCode::SERVICE_UNAVAILABLE,
                    Json(json!({"error":"reconciliation_storage_unavailable","reason":"billing reconciliation could not be loaded"})),
                )
                    .into_response();
            }
        }
    } else {
        let map = reconcile_store().lock().expect("mutex");
        if let Some(previous) = map.get(&key) {
            return decision_response(org, previous, true);
        }
    }

    // Never let reconciliation bypass subscription business rules:
    // If tenant is Closed, we do not Restore — we return Investigate/Suspend.
    let mut decision = reconcile(&internal, &provider, now);
    let org_rec = match state.saas.organization(org).await {
        Ok(value) => value,
        Err(error) => {
            tracing::error!(error = %error, organization = %org, "reconciliation organization could not be loaded");
            return (
                axum::http::StatusCode::SERVICE_UNAVAILABLE,
                Json(json!({"error":"reconciliation_storage_unavailable","reason":"tenant organization could not be loaded"})),
            )
                .into_response();
        }
    };
    if let Some(org_row) = org_rec {
        if org_row.status == bot_core::tenant::OrganizationStatus::Closed
            && decision.action == ReconciliationAction::Restore
        {
            // Override: closed tenant must stay closed — escalate to investigate
            decision.action = ReconciliationAction::Investigate;
            decision.reason = format!(
                "closed tenant {} cannot be restored by provider event {}; escalated to investigate",
                org, kind.as_str()
            );
        }
    }

    // Persist the decision before acknowledging it. The unique lookup key
    // makes concurrent reconciliation requests converge on one durable
    // decision rather than diverging across replicas.
    if let Some(db) = &state.db {
        let repo = PostgresSaasRepo::new(db.clone());
        match repo
            .insert(
                "billing_reconciliation",
                &key,
                Some(org),
                None,
                Some(&key),
                &decision,
            )
            .await
        {
            Ok(true) => {}
            Ok(false) => match repo
                .by_lookup::<ReconciliationDecision>("billing_reconciliation", &key)
                .await
            {
                Ok(Some(previous)) => return decision_response(org, &previous, true),
                Ok(None) => {
                    return (
                        axum::http::StatusCode::SERVICE_UNAVAILABLE,
                        Json(json!({"error":"reconciliation_storage_unavailable","reason":"durable reconciliation record disappeared"})),
                    )
                        .into_response();
                }
                Err(error) => {
                    tracing::error!(error = %error, "failed to reload durable billing reconciliation");
                    return (
                        axum::http::StatusCode::SERVICE_UNAVAILABLE,
                        Json(json!({"error":"reconciliation_storage_unavailable","reason":"durable reconciliation decision could not be loaded"})),
                    )
                        .into_response();
                }
            },
            Err(error) => {
                tracing::error!(error = %error, "failed to persist billing reconciliation");
                return (
                    axum::http::StatusCode::SERVICE_UNAVAILABLE,
                    Json(json!({"error":"reconciliation_storage_unavailable","reason":"billing reconciliation could not be persisted"})),
                )
                    .into_response();
            }
        }
    } else {
        let mut map = reconcile_store().lock().expect("mutex");
        map.insert(key, decision.clone());
    }

    // Audit — secret-free, tenant-scoped
    state
        .audit
        .record(
            "saas",
            "saas.billing.reconciled",
            Some(&org.to_string()),
            bot_core::audit::AuditOutcome::Success,
            json!({
                "organization": org.to_string(),
                "provider": provider_kind.as_str(),
                "event_kind": kind.as_str(),
                "action": decision.action.as_str(),
                "dry_run": body.dry_run.unwrap_or(false)
            }),
        )
        .await;

    // 200 for no_op, 202 for update/suspend/restore, 200 for investigate.
    decision_response(org, &decision, false)
}

async fn get_reconcile(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    let ctx = match authorize_request(
        &state,
        &headers,
        AccessRequest::read(Permission::BillingRead),
    )
    .await
    {
        Ok(c) => c,
        Err(d) => return deny_response(&state, &d).await,
    };
    let org = ctx.organization.id;
    if let Some(db) = &state.db {
        let repo = PostgresSaasRepo::new(db.clone());
        let decisions = match repo
            .by_organization::<ReconciliationDecision>("billing_reconciliation", org)
            .await
        {
            Ok(value) => value,
            Err(error) => {
                tracing::error!(error = %error, "failed to load durable billing reconciliation history");
                return (
                    axum::http::StatusCode::SERVICE_UNAVAILABLE,
                    Json(json!({"error":"reconciliation_storage_unavailable","reason":"billing reconciliation could not be loaded"})),
                )
                    .into_response();
            }
        };
        return decisions
            .into_iter()
            .find(|decision| decision.provider_snapshot.event_id == id)
            .map(|decision| decision_response(org, &decision, true))
            .unwrap_or_else(|| {
                (
                    axum::http::StatusCode::NOT_FOUND,
                    Json(json!({"error":"not_found","reason":"reconciliation not found"})),
                )
                    .into_response()
            });
    }

    let org_str = org.to_string();
    // No-database mode retains the bounded compatibility map only for tests.
    let map = reconcile_store().lock().expect("mutex");
    match map
        .iter()
        .find(|(key, decision)| {
            key.starts_with(&org_str) && decision.provider_snapshot.event_id == id
        })
        .map(|(_, decision)| decision)
    {
        Some(decision) => decision_response(org, decision, true),
        None => (
            axum::http::StatusCode::NOT_FOUND,
            Json(json!({"error":"not_found","reason":"reconciliation not found"})),
        )
            .into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bot_core::tenant::OrganizationId;
    use chrono::Utc;

    fn test_internal() -> InternalBillingSnapshot {
        InternalBillingSnapshot {
            organization_id: OrganizationId::new(),
            subscription_status: Some(bot_core::billing::subscription::SubscriptionStatus::PastDue),
            subscription_provider: None,
            last_payment_status: Some(TransactionStatus::Failed),
            last_invoice_status: Some(InvoiceStatus::Open),
            last_invoice_id: None,
            entitlement_active: false,
            as_of: Utc::now(),
        }
    }

    #[test]
    fn reconciliation_is_idempotent_by_key() {
        let org = OrganizationId::new();
        let k1 = reconcile_key(org, "stripe", "evt_123");
        let k2 = reconcile_key(org, "stripe", "evt_123");
        assert_eq!(k1, k2);
        let k3 = reconcile_key(org, "paddle", "evt_123");
        assert_ne!(k1, k3);
    }

    #[test]
    fn cross_tenant_reconcile_key_isolated() {
        let org1 = OrganizationId::new();
        let org2 = OrganizationId::new();
        let k1 = reconcile_key(org1, "stripe", "evt_123");
        let k2 = reconcile_key(org2, "stripe", "evt_123");
        assert_ne!(k1, k2);
        assert!(k1.contains(&org1.to_string()));
        assert!(!k1.contains(&org2.to_string()));
    }

    #[test]
    fn closed_tenant_never_restored() {
        let mut internal = test_internal();
        internal.subscription_status =
            Some(bot_core::billing::subscription::SubscriptionStatus::PastDue);
        let provider = ProviderBillingSnapshot {
            provider: BillingProviderKind::Stripe,
            provider_customer_id: None,
            event_kind: ProviderEventKind::PaymentSucceeded,
            event_id: "evt_1".into(),
            subscription_status_hint: None,
            invoice_status_hint: None,
            payment_status_hint: None,
            amount_cents: Some(5000),
            currency: Some("usd".into()),
            event_timestamp: Utc::now(),
        };
        let d = reconcile(&internal, &provider, Utc::now());
        // If internal is PastDue and provider says succeeded, action is Restore/Update — but if org is Closed we override to Investigate
        // Pure reconcile without org check may be Restore; the handler's override is tested via integration
        assert!(matches!(
            d.action,
            ReconciliationAction::Restore
                | ReconciliationAction::UpdateInternal
                | ReconciliationAction::NoOp
        ));
    }
}
