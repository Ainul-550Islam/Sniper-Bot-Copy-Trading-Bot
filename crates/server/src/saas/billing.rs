//! SaaS billing application service (BATCH file 11).
//!
//! Integrates existing plans/subscriptions/entitlements/usage with the new
//! provider-neutral payment domain. Exposes internal operations for checkout,
//! payment sync, subscription synchronization, invoices, cancellation.
//! Uses organization-scoped authorization; never trusts client-supplied price.

use chrono::{DateTime, Utc};
use sqlx::Row;

use bot_core::billing::checkout::{CheckoutRecord, CreateCheckout};
use bot_core::billing::invoice::{Invoice, InvoiceId, InvoiceStatus};
use bot_core::billing::payment::{PaymentTransaction, TransactionStatus};
use bot_core::billing::plan::PlanCode;
use bot_core::billing::provider::BillingProviderKind;

use bot_core::error::{BotError, BotResult};
use bot_core::tenant::OrganizationId;

use crate::api::ApiState;

/// Billing service — thin orchestration over SaasStore + plan authority + payment domain.
/// All methods are organization-scoped. No method trusts client-supplied price.
pub struct BillingService;

impl BillingService {
    /// Validate checkout request server-side, create idempotent checkout record.
    /// Price/plan authority comes from SaasStore's default catalogue; client amount is ignored.
    #[allow(clippy::too_many_arguments)]
    pub async fn create_checkout(
        state: &ApiState,
        org: OrganizationId,
        plan_code: PlanCode,
        provider: BillingProviderKind,
        idempotency_key: String,
        success_url: Option<String>,
        cancel_url: Option<String>,
        now: DateTime<Utc>,
    ) -> BotResult<CheckoutRecord> {
        // Check organization exists and is not closed (tenant policy)
        let org_record = state
            .saas
            .organization(org)
            .await?
            .ok_or_else(|| BotError::NotFound(format!("organization {}", org)))?;
        if org_record.status == bot_core::tenant::OrganizationStatus::Closed {
            return Err(BotError::invalid("organization is closed"));
        }

        // Typed provider configuration check — Stripe/Paddle without credentials must not silently fall back to Manual
        if matches!(
            provider,
            BillingProviderKind::Stripe | BillingProviderKind::Paddle
        ) {
            Self::require_provider_configured(provider)?;
        }

        // Load known plans from catalogue (server authority)
        let plans = state
            .saas
            .plans()
            .await
            .map_err(|e| BotError::db(format!("plan catalogue unavailable: {e}")))?;
        let req = CreateCheckout::new(org, plan_code, provider, idempotency_key.clone(), now)
            .with_redirects(success_url.clone(), cancel_url.clone());
        req.validate(&plans).map_err(BotError::invalid)?;

        // Validate plan exists
        let _plan = plans
            .iter()
            .find(|p| p.code == plan_code)
            .ok_or_else(|| BotError::NotFound(format!("plan {}", plan_code.as_str())))?;

        // Idempotency: durable checkout_sessions with UNIQUE (organization_id, idempotency_key).
        // If a record for this org+key already exists, return it (including across restarts and replicas).
        if let Some(existing) = Self::find_checkout(state, org, &idempotency_key).await? {
            return Ok(existing);
        }

        // Create record
        let mut record = CheckoutRecord::new(&req, now);
        // For Manual: no provider call, just pending
        // For Stripe/Paddle: durable pending first, then provider call with idempotency propagation
        record.checkout_url = None;

        // Persist durably pending first (A-E) — ensures idempotency and restart safety before network
        let stored = Self::store_checkout_durable(state, &record).await?;

        // F) Call provider if not Manual
        let provider_result =
            match provider {
                BillingProviderKind::Manual => None,
                BillingProviderKind::Stripe => {
                    // Resolve Stripe adapter (typed NotConfigured, no fallback)
                    let adapter = match crate::billing::provider_registry::registry_for(
                        BillingProviderKind::Stripe,
                    ) {
                        Ok(crate::billing::provider_registry::RegistryAdapter::Stripe(a)) => a,
                        // Fail closed with a typed error instead of panicking: a handler
                        // must never unwind on a provider-resolution mismatch.
                        Ok(_) => return Err(BotError::invalid(
                            "provider registry returned a non-Stripe adapter for a Stripe checkout",
                        )),
                        Err(e) => return Err(BotError::invalid(e.to_string())),
                    };
                    let intent = crate::billing::stripe_adapter::StripeCheckoutIntent {
                        organization: org,
                        plan: plan_code,
                        idempotency_key: stored.idempotency_key.clone(),
                        success_url: stored.success_url.clone(),
                        cancel_url: stored.cancel_url.clone(),
                    };
                    match adapter.create_checkout(&intent).await {
                        Ok(sess) => Some(Ok((sess.session_id, sess.checkout_url))),
                        Err(e) => Some(Err(e.to_string())),
                    }
                }
                BillingProviderKind::Paddle => {
                    let adapter = match crate::billing::provider_registry::registry_for(
                        BillingProviderKind::Paddle,
                    ) {
                        Ok(crate::billing::provider_registry::RegistryAdapter::Paddle(a)) => a,
                        // Fail closed with a typed error instead of panicking.
                        Ok(_) => return Err(BotError::invalid(
                            "provider registry returned a non-Paddle adapter for a Paddle checkout",
                        )),
                        Err(e) => return Err(BotError::invalid(e.to_string())),
                    };
                    let intent = crate::billing::paddle_adapter::PaddleCheckoutIntent {
                        organization: org,
                        plan: plan_code,
                        idempotency_key: stored.idempotency_key.clone(),
                        success_url: stored.success_url.clone(),
                        cancel_url: stored.cancel_url.clone(),
                    };
                    match adapter.create_checkout(&intent).await {
                        Ok(sess) => Some(Ok((sess.session_id, sess.checkout_url))),
                        Err(e) => Some(Err(e.to_string())),
                    }
                }
            };

        // G/H) Handle provider result
        let final_record = if let Some(res) = provider_result {
            match res {
                Ok((session_id, checkout_url)) => {
                    // Provider success — update durable record with session_id and checkout_url
                    // Must be atomic durable update; if DB update fails, do not claim success
                    let updated = Self::update_checkout_with_provider(
                        state,
                        &stored,
                        Some(session_id.clone()),
                        Some(checkout_url.clone()),
                        now,
                    )
                    .await;
                    match updated {
                        Ok(rec) => rec,
                        Err(db_err) => {
                            // Provider succeeded but DB update failed — do not return success without durable state
                            // Leave durable pending and surface reconciliation-needed error (no fake success)
                            // Provider event will be reconciled via provider_events / audit
                            state
                                .audit
                                .record(
                                    "saas",
                                    "saas.billing.checkout.provider_db_update_failed",
                                    Some(&stored.id.to_string()),
                                    bot_core::audit::AuditOutcome::Failure,
                                    serde_json::json!({
                                        "organization": org.to_string(),
                                        "plan": plan_code.as_str(),
                                        "provider": provider.as_str(),
                                        "provider_session_id": session_id,
                                        "error": db_err.to_string()
                                    }),
                                )
                                .await;
                            return Err(BotError::db(format!(
                                "provider checkout succeeded but durable update failed (reconciliation required): {}",
                                db_err
                            )));
                        }
                    }
                }
                Err(provider_err) => {
                    // Provider failure — do not mark successful; leave durable pending (or could mark failed if model supported)
                    // Check for NOT_RUN / EXTERNAL_REQUIRED semantics
                    let msg = provider_err.clone();
                    // Propagate typed error: contains LIVE_BILLING NOT_RUN or missing config
                    // For live gating, return 503-like error that checkout handler maps to 501/503
                    if msg.contains("NOT_RUN") || msg.contains("LIVE_BILLING") {
                        return Err(BotError::invalid(format!(
                            "provider {} NOT_RUN: {} (requires LIVE_BILLING=1 and real credentials)",
                            provider.as_str(),
                            msg
                        )));
                    }
                    if msg.contains("not configured") || msg.contains("missing") {
                        return Err(BotError::invalid(msg));
                    }
                    // Generic provider failure — keep durable pending, surface redacted error
                    // Do not expose api keys or full payload (adapter already redacts)
                    state
                        .audit
                        .record(
                            "saas",
                            "saas.billing.checkout.provider_failed",
                            Some(&stored.id.to_string()),
                            bot_core::audit::AuditOutcome::Failure,
                            serde_json::json!({
                                "organization": org.to_string(),
                                "plan": plan_code.as_str(),
                                "provider": provider.as_str(),
                                "error": msg
                            }),
                        )
                        .await;
                    return Err(BotError::invalid(format!(
                        "provider {} checkout failed: {}",
                        provider.as_str(),
                        msg
                    )));
                }
            }
        } else {
            stored.clone()
        };

        // Audit (secret-free)
        state
            .audit
            .record(
                "saas",
                "saas.billing.checkout.created",
                Some(&final_record.id.to_string()),
                bot_core::audit::AuditOutcome::Success,
                serde_json::json!({
                    "organization": org.to_string(),
                    "plan": plan_code.as_str(),
                    "provider": provider.as_str(),
                    "idempotency_key": idempotency_key,
                    "checkout_url_present": final_record.checkout_url.is_some()
                }),
            )
            .await;

        Ok(final_record)
    }

    /// Get checkout by org+idempotency_key (idempotency lookup). Returns None if not found or cross-tenant.
    /// Durable when DB is attached; memory fallback only in tests.
    pub async fn find_checkout(
        state: &ApiState,
        org: OrganizationId,
        idempotency_key: &str,
    ) -> BotResult<Option<CheckoutRecord>> {
        if let Some(db) = &state.db {
            let row = sqlx::query(
                "SELECT id, organization_id, plan_code, provider, provider_session_id, checkout_url, idempotency_key, status, success_url, cancel_url, expires_at, created_at, updated_at FROM checkout_sessions WHERE organization_id=$1 AND idempotency_key=$2"
            )
            .bind(org.as_uuid())
            .bind(idempotency_key)
            .fetch_optional(db.pool())
            .await
            .map_err(|error| BotError::db(format!("checkout lookup failed: {error}")))?;
            match row {
                Some(row) => Self::map_checkout_row(&row).map(Some).map_err(|error| {
                    BotError::db(format!("checkout record decode failed: {error}"))
                }),
                None => Ok(None),
            }
        } else {
            // Test-only memory fallback; production without DB fails closed via store path, but find still uses memory for hermetic tests.
            Ok(find_checkout_memory(org, idempotency_key).await)
        }
    }

    /// Synchronize payment status from provider event (idempotent).
    /// Maps normalized provider event to payment transaction state, then to subscription status.
    pub async fn sync_payment(
        state: &ApiState,
        org: OrganizationId,
        payment: &mut PaymentTransaction,
        new_status: TransactionStatus,
        now: DateTime<Utc>,
    ) -> BotResult<()> {
        if payment.organization_id != org {
            return Err(BotError::invalid("cross-tenant payment sync denied"));
        }
        if !payment.can_transition_to(new_status) && payment.status != new_status {
            return Err(BotError::invalid(format!(
                "illegal payment transition {} -> {}",
                payment.status.as_str(),
                new_status.as_str()
            )));
        }
        if payment.status == new_status {
            return Ok(()); // idempotent
        }
        payment
            .transition_to(new_status, None, now)
            .map_err(BotError::invalid)?;

        // If payment succeeded and linked to subscription, activate subscription entitlements
        if new_status == TransactionStatus::Succeeded {
            if let Some(sub_id) = payment.subscription_id {
                if let Some(mut sub) = state.saas.subscription_of(org).await? {
                    if sub.id.as_uuid() == sub_id {
                        let current_end = sub.current_period_end;
                        sub.renew(current_end, now);
                        state.saas.update_subscription(&sub).await?;
                    }
                }
            }
        }

        // §G: persist the transitioned transaction durably through the
        // store (payment_transaction kind; table 0019 is the durable
        // substrate behind the Postgres repo). No fake success: a store
        // failure surfaces as an error after the audit row below records
        // the outcome.
        state.saas.update_payment(payment).await?;

        state
            .audit
            .record(
                "saas",
                "saas.billing.payment.synced",
                Some(&payment.id.to_string()),
                bot_core::audit::AuditOutcome::Success,
                serde_json::json!({"organization": org.to_string(), "status": new_status.as_str()}),
            )
            .await;
        Ok(())
    }

    /// Cancel subscription at period end or immediately. Never trusts client-supplied price.
    pub async fn cancel_subscription(
        state: &ApiState,
        org: OrganizationId,
        at_period_end: bool,
        now: DateTime<Utc>,
    ) -> BotResult<()> {
        let mut sub = state
            .saas
            .subscription_of(org)
            .await?
            .ok_or_else(|| BotError::NotFound("subscription not found".into()))?;
        sub.cancel(at_period_end, now);
        state.saas.update_subscription(&sub).await?;
        state.audit.record("saas", "saas.billing.subscription.canceled", Some(&sub.id.to_string()),
            bot_core::audit::AuditOutcome::Success,
            serde_json::json!({"organization": org.to_string(), "at_period_end": at_period_end})).await;
        Ok(())
    }

    /// List invoices for organization (tenant-scoped query). Never leaks cross-tenant existence.
    /// Durable: SELECT ... WHERE organization_id=$1 ORDER BY created_at, id
    pub async fn list_invoices(state: &ApiState, org: OrganizationId) -> BotResult<Vec<Invoice>> {
        if let Some(db) = &state.db {
            let rows = sqlx::query(
                "SELECT id, organization_id, subscription_id, payment_transaction_id, provider, provider_invoice_id, invoice_number, status, amount_cents, amount_paid_cents, amount_due_cents, currency, period_start, period_end, due_date, paid_at, hosted_invoice_url, invoice_pdf_url, created_at, updated_at FROM invoices WHERE organization_id=$1 ORDER BY created_at ASC, id ASC"
            )
            .bind(org.as_uuid())
            .fetch_all(db.pool())
            .await
            .map_err(|e| BotError::db(format!("list invoices failed: {e}")))?;
            let mut out = Vec::with_capacity(rows.len());
            for r in rows {
                match Self::map_invoice_row(&r) {
                    Ok(inv) => out.push(inv),
                    Err(e) => return Err(BotError::db(format!("invoice decode failed: {e}"))),
                }
            }
            Ok(out)
        } else {
            // No durable store: return empty list deterministically (test-only, does not leak)
            Ok(Vec::new())
        }
    }

    /// Get invoice detail, tenant-scoped. Returns NotFound for both missing and cross-tenant (no leak).
    /// Durable: SELECT ... WHERE id=$1 AND organization_id=$2
    pub async fn get_invoice(
        state: &ApiState,
        org: OrganizationId,
        invoice_id: InvoiceId,
    ) -> BotResult<Invoice> {
        if let Some(db) = &state.db {
            let row = sqlx::query(
                "SELECT id, organization_id, subscription_id, payment_transaction_id, provider, provider_invoice_id, invoice_number, status, amount_cents, amount_paid_cents, amount_due_cents, currency, period_start, period_end, due_date, paid_at, hosted_invoice_url, invoice_pdf_url, created_at, updated_at FROM invoices WHERE id=$1 AND organization_id=$2"
            )
            .bind(invoice_id.as_uuid())
            .bind(org.as_uuid())
            .fetch_optional(db.pool())
            .await
            .map_err(|e| BotError::db(format!("get invoice failed: {e}")))?;
            match row {
                Some(r) => Self::map_invoice_row(&r).map_err(BotError::db),
                None => Err(BotError::NotFound(format!("invoice {}", invoice_id))),
            }
        } else {
            Err(BotError::NotFound(format!("invoice {}", invoice_id)))
        }
    }

    // -----------------------------------------------------------------------
    // Durable checkout persistence helpers
    // -----------------------------------------------------------------------

    fn require_provider_configured(provider: BillingProviderKind) -> BotResult<()> {
        let (env_key, webhook_key) = match provider {
            BillingProviderKind::Stripe => ("STRIPE_API_KEY", "STRIPE_WEBHOOK_SECRET"),
            BillingProviderKind::Paddle => ("PADDLE_API_KEY", "PADDLE_WEBHOOK_SECRET"),
            _ => return Ok(()),
        };
        let api_ok = std::env::var(env_key)
            .map(|v| !v.trim().is_empty())
            .unwrap_or(false);
        let wh_ok = std::env::var(webhook_key)
            .map(|v| !v.trim().is_empty())
            .unwrap_or(false);
        // Also accept SAAS_WEBHOOK_SECRET_* as alternative webhook secret name
        let wh_alt = match provider {
            BillingProviderKind::Stripe => std::env::var("SAAS_WEBHOOK_SECRET_STRIPE")
                .map(|v| !v.trim().is_empty())
                .unwrap_or(false),
            BillingProviderKind::Paddle => std::env::var("SAAS_WEBHOOK_SECRET_PADDLE")
                .map(|v| !v.trim().is_empty())
                .unwrap_or(false),
            _ => false,
        };
        if !api_ok {
            return Err(BotError::invalid(format!(
                "provider {} not configured: missing {} (set {} in environment; no silent fallback to manual)",
                provider.as_str(),
                env_key,
                env_key
            )));
        }
        if !wh_ok && !wh_alt {
            // Webhook secret missing is not fatal for checkout creation, but we warn via typed error if webhook path is expected
            // For checkout we allow missing webhook secret (only required for webhook verification)
            // So we do not fail here
        }
        Ok(())
    }

    async fn store_checkout_durable(
        state: &ApiState,
        record: &CheckoutRecord,
    ) -> BotResult<CheckoutRecord> {
        if let Some(db) = &state.db {
            // Attempt durable insert with UNIQUE (organization_id, idempotency_key) fencing concurrent replicas
            let res = sqlx::query(
                r#"INSERT INTO checkout_sessions
                    (id, organization_id, plan_code, provider, provider_session_id, checkout_url, idempotency_key, status, success_url, cancel_url, expires_at, created_at, updated_at)
                   VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13)
                   ON CONFLICT (organization_id, idempotency_key) DO NOTHING"#,
            )
            .bind(record.id)
            .bind(record.organization_id.as_uuid())
            .bind(record.plan_code.as_str())
            .bind(record.provider.as_str())
            .bind(&record.provider_session_id)
            .bind(&record.checkout_url)
            .bind(&record.idempotency_key)
            .bind(record.status.as_str())
            .bind(&record.success_url)
            .bind(&record.cancel_url)
            .bind(record.expires_at)
            .bind(record.created_at)
            .bind(record.updated_at)
            .execute(db.pool())
            .await
            .map_err(|e| BotError::db(format!("checkout insert failed: {e}")))?;

            if res.rows_affected() == 1 {
                // Inserted — return the same record (it is the winner)
                return Ok(record.clone());
            }
            // Conflict: another replica/restart already inserted — fetch the winner
            let row = sqlx::query(
                "SELECT id, organization_id, plan_code, provider, provider_session_id, checkout_url, idempotency_key, status, success_url, cancel_url, expires_at, created_at, updated_at FROM checkout_sessions WHERE organization_id=$1 AND idempotency_key=$2"
            )
            .bind(record.organization_id.as_uuid())
            .bind(&record.idempotency_key)
            .fetch_one(db.pool())
            .await
            .map_err(|e| BotError::db(format!("checkout fetch after conflict failed: {e}")))?;
            Self::map_checkout_row(&row).map_err(BotError::db)
        } else {
            // No database attached
            // In tests we allow memory fallback; in production we fail clearly (no silent in-memory durable)
            #[cfg(test)]
            {
                store_checkout_idempotent_memory(record).await;
                Ok(record.clone())
            }
            #[cfg(not(test))]
            {
                Err(BotError::db(
                    "database unavailable — checkout requires POSTGRES_URL and DATABASE_ENABLED=true (no durable store; restart would lose idempotency)",
                ))
            }
        }
    }

    async fn update_checkout_with_provider(
        state: &ApiState,
        stored: &CheckoutRecord,
        provider_session_id: Option<String>,
        checkout_url: Option<String>,
        now: DateTime<Utc>,
    ) -> BotResult<CheckoutRecord> {
        if let Some(db) = &state.db {
            // Attempt to update durable record; if fails, caller must not claim success.
            // The WHERE clause carries organization_id so a record can never be
            // written across tenants, and rows_affected must be exactly one: a
            // zero-row update means the durable identity is gone or foreign.
            let res = sqlx::query(
                r#"UPDATE checkout_sessions SET provider_session_id=$1, checkout_url=$2, status='open', updated_at=$3 WHERE id=$4 AND organization_id=$5"#,
            )
            .bind(&provider_session_id)
            .bind(&checkout_url)
            .bind(now)
            .bind(stored.id)
            .bind(stored.organization_id.as_uuid())
            .execute(db.pool())
            .await
            .map_err(|e| BotError::db(format!("checkout update with provider failed: {e}")))?;
            if res.rows_affected() != 1 {
                return Err(BotError::db(
                    "checkout update with provider affected no durable row (tenant mismatch or missing record) — reconciliation required",
                ));
            }
            // Fetch updated record
            let row = sqlx::query(
                "SELECT id, organization_id, plan_code, provider, provider_session_id, checkout_url, idempotency_key, status, success_url, cancel_url, expires_at, created_at, updated_at FROM checkout_sessions WHERE id=$1 AND organization_id=$2"
            )
            .bind(stored.id)
            .bind(stored.organization_id.as_uuid())
            .fetch_one(db.pool())
            .await
            .map_err(|e| BotError::db(format!("checkout fetch after provider update failed: {e}")))?;
            Self::map_checkout_row(&row).map_err(BotError::db)
        } else {
            // No DB: test-only, update memory record directly
            let mut rec = stored.clone();
            rec.provider_session_id = provider_session_id;
            rec.checkout_url = checkout_url;
            rec.status = bot_core::billing::provider::CheckoutStatus::Open;
            rec.updated_at = now;
            // Update memory store
            {
                let mut map = checkout_store().write().await;
                map.insert(
                    (rec.organization_id, rec.idempotency_key.clone()),
                    rec.clone(),
                );
            }
            Ok(rec)
        }
    }

    fn map_checkout_row(row: &sqlx::postgres::PgRow) -> Result<CheckoutRecord, String> {
        let id: uuid::Uuid = row.try_get("id").map_err(|e| e.to_string())?;
        let org_uuid: uuid::Uuid = row.try_get("organization_id").map_err(|e| e.to_string())?;
        let plan_s: String = row.try_get("plan_code").map_err(|e| e.to_string())?;
        let provider_s: String = row.try_get("provider").map_err(|e| e.to_string())?;
        let provider_session_id: Option<String> = row
            .try_get("provider_session_id")
            .map_err(|e| e.to_string())?;
        let checkout_url: Option<String> =
            row.try_get("checkout_url").map_err(|e| e.to_string())?;
        let idempotency_key: String = row.try_get("idempotency_key").map_err(|e| e.to_string())?;
        let status_s: String = row.try_get("status").map_err(|e| e.to_string())?;
        let success_url: Option<String> = row.try_get("success_url").map_err(|e| e.to_string())?;
        let cancel_url: Option<String> = row.try_get("cancel_url").map_err(|e| e.to_string())?;
        let expires_at: Option<DateTime<Utc>> =
            row.try_get("expires_at").map_err(|e| e.to_string())?;
        let created_at: DateTime<Utc> = row.try_get("created_at").map_err(|e| e.to_string())?;
        let updated_at: DateTime<Utc> = row.try_get("updated_at").map_err(|e| e.to_string())?;

        let plan_code = bot_core::billing::plan::PlanCode::parse(&plan_s)
            .ok_or_else(|| format!("unknown plan_code in db: {plan_s}"))?;
        let provider = BillingProviderKind::parse(&provider_s)
            .ok_or_else(|| format!("unknown provider in db: {provider_s}"))?;
        let status = bot_core::billing::provider::CheckoutStatus::parse(&status_s)
            .ok_or_else(|| format!("unknown checkout status in db: {status_s}"))?;

        Ok(CheckoutRecord {
            id,
            organization_id: OrganizationId::from(org_uuid),
            plan_code,
            provider,
            provider_session_id,
            idempotency_key,
            status,
            checkout_url,
            success_url,
            cancel_url,
            expires_at,
            created_at,
            updated_at,
        })
    }

    fn map_invoice_row(row: &sqlx::postgres::PgRow) -> Result<Invoice, String> {
        let id: uuid::Uuid = row.try_get("id").map_err(|e| e.to_string())?;
        let org_uuid: uuid::Uuid = row.try_get("organization_id").map_err(|e| e.to_string())?;
        let subscription_id: Option<uuid::Uuid> =
            row.try_get("subscription_id").map_err(|e| e.to_string())?;
        let payment_transaction_id: Option<uuid::Uuid> = row
            .try_get("payment_transaction_id")
            .map_err(|e| e.to_string())?;
        let provider_s: String = row.try_get("provider").map_err(|e| e.to_string())?;
        let provider_invoice_id: Option<String> = row
            .try_get("provider_invoice_id")
            .map_err(|e| e.to_string())?;
        let invoice_number: Option<String> =
            row.try_get("invoice_number").map_err(|e| e.to_string())?;
        let status_s: String = row.try_get("status").map_err(|e| e.to_string())?;
        let amount_cents: i64 = row.try_get("amount_cents").map_err(|e| e.to_string())?;
        let amount_paid_cents: i64 = row
            .try_get("amount_paid_cents")
            .map_err(|e| e.to_string())?;
        let amount_due_cents: i64 = row.try_get("amount_due_cents").map_err(|e| e.to_string())?;
        let currency: String = row.try_get("currency").map_err(|e| e.to_string())?;
        let period_start: Option<DateTime<Utc>> =
            row.try_get("period_start").map_err(|e| e.to_string())?;
        let period_end: Option<DateTime<Utc>> =
            row.try_get("period_end").map_err(|e| e.to_string())?;
        let due_date: Option<DateTime<Utc>> = row.try_get("due_date").map_err(|e| e.to_string())?;
        let paid_at: Option<DateTime<Utc>> = row.try_get("paid_at").map_err(|e| e.to_string())?;
        let hosted_url: Option<String> = row
            .try_get("hosted_invoice_url")
            .map_err(|e| e.to_string())?;
        let pdf_url: Option<String> = row.try_get("invoice_pdf_url").map_err(|e| e.to_string())?;
        let created_at: DateTime<Utc> = row.try_get("created_at").map_err(|e| e.to_string())?;
        let updated_at: DateTime<Utc> = row.try_get("updated_at").map_err(|e| e.to_string())?;

        let provider = BillingProviderKind::parse(&provider_s)
            .ok_or_else(|| format!("unknown provider in db: {provider_s}"))?;
        let status = InvoiceStatus::parse(&status_s)
            .ok_or_else(|| format!("unknown invoice status in db: {status_s}"))?;

        Ok(Invoice {
            id: InvoiceId(id),
            organization_id: OrganizationId::from(org_uuid),
            subscription_id,
            payment_transaction_id,
            provider,
            provider_invoice_id,
            invoice_number,
            status,
            amount_cents,
            amount_paid_cents,
            amount_due_cents,
            currency,
            period_start,
            period_end,
            due_date,
            paid_at,
            hosted_url,
            pdf_url,
            created_at,
            updated_at,
        })
    }
}

// ---------------------------------------------------------------------------
// In-memory idempotency store for checkout records (test-only fallback;
// production uses checkout_sessions table).
use std::collections::HashMap;
use std::sync::OnceLock;
use tokio::sync::RwLock;

fn checkout_store() -> &'static RwLock<HashMap<(OrganizationId, String), CheckoutRecord>> {
    static STORE: OnceLock<RwLock<HashMap<(OrganizationId, String), CheckoutRecord>>> =
        OnceLock::new();
    STORE.get_or_init(|| RwLock::new(HashMap::new()))
}

async fn store_checkout_idempotent_memory(record: &CheckoutRecord) {
    let mut map = checkout_store().write().await;
    let key = (record.organization_id, record.idempotency_key.clone());
    map.entry(key).or_insert_with(|| record.clone());
}

async fn find_checkout_memory(org: OrganizationId, key: &str) -> Option<CheckoutRecord> {
    let map = checkout_store().read().await;
    map.get(&(org, key.to_string())).cloned()
}

// Helper for tests to clear memory store when needed
#[cfg(test)]
pub(crate) async fn clear_memory_store() {
    checkout_store().write().await.clear();
}

#[cfg(test)]
mod tests {
    use super::*;
    use bot_core::tenant::OrganizationId;
    use chrono::Utc;

    #[tokio::test]
    async fn checkout_idempotency_is_per_org_memory() {
        clear_memory_store().await;
        let org1 = OrganizationId::new();
        let org2 = OrganizationId::new();
        let now = Utc::now();
        let rec1 = CheckoutRecord::new(
            &CreateCheckout::new(
                org1,
                PlanCode::Starter,
                BillingProviderKind::Manual,
                "key-1",
                now,
            ),
            now,
        );
        let rec2 = CheckoutRecord::new(
            &CreateCheckout::new(
                org2,
                PlanCode::Starter,
                BillingProviderKind::Manual,
                "key-1",
                now,
            ),
            now,
        );
        store_checkout_idempotent_memory(&rec1).await;
        store_checkout_idempotent_memory(&rec2).await;
        let found1 = find_checkout_memory(org1, "key-1").await.unwrap();
        let found2 = find_checkout_memory(org2, "key-1").await.unwrap();
        assert_ne!(found1.id, found2.id);
        let rec_dup = CheckoutRecord::new(
            &CreateCheckout::new(
                org1,
                PlanCode::Starter,
                BillingProviderKind::Manual,
                "key-1",
                now,
            ),
            now,
        );
        store_checkout_idempotent_memory(&rec_dup).await;
        let found_again = find_checkout_memory(org1, "key-1").await.unwrap();
        assert_eq!(found_again.id, rec1.id, "duplicate must not overwrite");
    }

    // PG integration: durable checkout — UNIQUE (organization_id, idempotency_key), duplicate retrieve, restart durable, cross-tenant isolation, concurrent replicas one record, DB unavailable path
    #[tokio::test]
    async fn checkout_pg_durable_idempotency() {
        let Some(url) = std::env::var("POSTGRES_URL")
            .ok()
            .filter(|v| !v.trim().is_empty())
        else {
            eprintln!("POSTGRES_URL not set — checkout_pg_durable_idempotency NOT_RUN");
            return;
        };
        let cfg = bot_core::config::DatabaseConfig {
            enabled: true,
            url_env: "POSTGRES_URL".into(),
            required: true,
            auto_migrate: true,
            max_connections: 5,
            min_connections: 1,
            acquire_timeout_ms: 5000,
            statement_timeout_ms: 5000,
            query_timeout_ms: 5000,
        };
        let db = std::sync::Arc::new(
            bot_core::db::Database::connect(&cfg, &url)
                .await
                .expect("connect"),
        );
        db.migrate().await.expect("migrate");
        // Need an organization row to satisfy FK
        let org = OrganizationId::new();
        let org_rec = bot_core::tenant::Organization::new(
            org,
            format!("test-org-{}", org),
            "test",
            None,
            Utc::now(),
        );
        sqlx::query(
            "INSERT INTO organizations (id, slug, name, status, created_at, updated_at) VALUES ($1,$2,$3,$4,$5,$6) ON CONFLICT DO NOTHING"
        )
        .bind(org.as_uuid())
        .bind(&org_rec.slug)
        .bind(&org_rec.name)
        .bind("active")
        .bind(org_rec.created_at)
        .bind(org_rec.updated_at)
        .execute(db.pool())
        .await
        .expect("insert org");

        let shared = bot_core::state::AppState::new(bot_core::config::AppConfig::from_defaults());
        let state = crate::api::ApiState {
            shared: shared.clone(),
            api_key: None,
            auth: None,
            limiter: bot_core::auth::RateLimiter::new(0),
            sensitive_limiter: bot_core::auth::RateLimiter::new(10),
            audit: bot_core::audit::AuditTrail::new(None, shared.events.clone()),
            db: Some(db.clone()),
            journal: None,
            serve_dashboard: false,
            health: std::sync::Arc::new(bot_core::obs::health::HealthRegistry::new()),
            metrics_enabled: false,
            saas: std::sync::Arc::new(
                crate::saas::SaasStore::with_database(Some(db.clone()))
                    .await
                    .expect("saas store"),
            ),
            module_registry: std::sync::Arc::new(
                crate::module_runtime::module_registry::TenantModuleRegistry::new(),
            ),
            trading: None,
        };

        // Register the organization through the store so the durable runtime
        // projection (saas_runtime_records) resolves it for every replica.
        state
            .saas
            .create_organization(&org_rec)
            .await
            .expect("seed org in saas store");

        let now = Utc::now();
        let key = format!("itest-{}", uuid::Uuid::new_v4());
        let rec = BillingService::create_checkout(
            &state,
            org,
            PlanCode::Starter,
            BillingProviderKind::Manual,
            key.clone(),
            None,
            None,
            now,
        )
        .await
        .expect("create checkout");

        // Direct SQL: exactly one durable row for (organization_id, idempotency_key)
        let count_db: (i64,) = sqlx::query_as(
            "SELECT COUNT(*) FROM checkout_sessions WHERE organization_id=$1 AND idempotency_key=$2",
        )
        .bind(org.as_uuid())
        .bind(&key)
        .fetch_one(db.pool())
        .await
        .expect("count durable rows");
        assert_eq!(count_db.0, 1, "exactly one durable checkout row");

        // The durable unique constraint must actually exist in this database
        let constraint: Option<(String,)> = sqlx::query_as(
            "SELECT conname FROM pg_constraint WHERE conrelid='checkout_sessions'::regclass AND contype='u' AND conname='checkout_sessions_organization_id_idempotency_key_key'",
        )
        .fetch_optional(db.pool())
        .await
        .expect("pg_constraint");
        assert!(
            constraint.is_some(),
            "UNIQUE (organization_id, idempotency_key) must exist in the live database"
        );
        let provider_unique: Option<(String,)> = sqlx::query_as(
            "SELECT conname FROM pg_constraint WHERE conrelid='checkout_sessions'::regclass AND contype='u' AND conname='checkout_sessions_provider_provider_session_id_key'",
        )
        .fetch_optional(db.pool())
        .await
        .expect("pg_constraint provider");
        assert!(
            provider_unique.is_some(),
            "UNIQUE (provider, provider_session_id) must exist in the live database"
        );

        // Duplicate retrieve must return same id (idempotency)
        let dup = BillingService::create_checkout(
            &state,
            org,
            PlanCode::Starter,
            BillingProviderKind::Manual,
            key.clone(),
            None,
            None,
            now,
        )
        .await
        .expect("duplicate checkout");
        assert_eq!(rec.id, dup.id, "duplicate must return same record");

        // Cross-tenant isolation: same key different org creates distinct record
        let org2 = OrganizationId::new();
        let org2_rec = bot_core::tenant::Organization::new(
            org2,
            format!("test-org2-{}", org2),
            "test2",
            None,
            Utc::now(),
        );
        sqlx::query(
            "INSERT INTO organizations (id, slug, name, status, created_at, updated_at) VALUES ($1,$2,$3,$4,$5,$6) ON CONFLICT DO NOTHING"
        )
        .bind(org2.as_uuid())
        .bind(&org2_rec.slug)
        .bind(&org2_rec.name)
        .bind("active")
        .bind(org2_rec.created_at)
        .bind(org2_rec.updated_at)
        .execute(db.pool())
        .await
        .expect("insert org2");
        // Need to also ensure saas store knows org2
        state
            .saas
            .create_organization(&org2_rec)
            .await
            .expect("create org2 in saas");
        let rec_other = BillingService::create_checkout(
            &state,
            org2,
            PlanCode::Starter,
            BillingProviderKind::Manual,
            key.clone(),
            None,
            None,
            now,
        )
        .await
        .expect("checkout other org");
        assert_ne!(rec.id, rec_other.id, "cross-tenant must be isolated");

        // ---------------------------------------------------------- same org, different key --
        let key_other = format!("itest-other-{}", uuid::Uuid::new_v4());
        let rec_diff_key = BillingService::create_checkout(
            &state,
            org,
            PlanCode::Starter,
            BillingProviderKind::Manual,
            key_other.clone(),
            None,
            None,
            Utc::now(),
        )
        .await
        .expect("checkout with different key");
        assert_ne!(
            rec.id, rec_diff_key.id,
            "same org + different idempotency key must produce a distinct checkout identity"
        );

        // ------------------------------------------- direct SQL uniqueness behaviour --
        // A raw duplicate insert for (organization_id, idempotency_key) must be rejected
        // by the database itself — not merely avoided by application logic.
        let dup_raw = sqlx::query(
            "INSERT INTO checkout_sessions (id, organization_id, plan_code, provider, idempotency_key, status, created_at, updated_at) VALUES ($1,$2,$3,$4,$5,$6,$7,$8)",
        )
        .bind(uuid::Uuid::new_v4())
        .bind(org.as_uuid())
        .bind("starter")
        .bind("manual")
        .bind(&key)
        .bind("pending")
        .bind(Utc::now())
        .bind(Utc::now())
        .execute(db.pool())
        .await;
        assert!(
            dup_raw.is_err(),
            "database must reject a duplicate (organization_id, idempotency_key) insert"
        );

        // Cross-tenant: same idempotency key for a different organization is allowed (2 rows total)
        let cross_count: (i64,) =
            sqlx::query_as("SELECT COUNT(*) FROM checkout_sessions WHERE idempotency_key=$1")
                .bind(&key)
                .fetch_one(db.pool())
                .await
                .expect("cross-tenant count");
        assert_eq!(
            cross_count.0, 2,
            "same key across two organizations must yield exactly two rows"
        );

        // --------------------------------------- RESTART: fresh pool + fresh store --
        // A brand new connection pool and a brand new SaasStore stand in for a new
        // process: nothing from the first state's memory is reused.
        let db_restart = std::sync::Arc::new(
            bot_core::db::Database::connect(&cfg, &url)
                .await
                .expect("restart connect"),
        );
        let shared_restart =
            bot_core::state::AppState::new(bot_core::config::AppConfig::from_defaults());
        let state_restart = crate::api::ApiState {
            shared: shared_restart.clone(),
            api_key: None,
            auth: None,
            limiter: bot_core::auth::RateLimiter::new(0),
            sensitive_limiter: bot_core::auth::RateLimiter::new(10),
            audit: bot_core::audit::AuditTrail::new(None, shared_restart.events.clone()),
            db: Some(db_restart.clone()),
            journal: None,
            serve_dashboard: false,
            health: std::sync::Arc::new(bot_core::obs::health::HealthRegistry::new()),
            metrics_enabled: false,
            saas: std::sync::Arc::new(
                crate::saas::SaasStore::with_database(Some(db_restart.clone()))
                    .await
                    .expect("restart saas store"),
            ),
            module_registry: std::sync::Arc::new(
                crate::module_runtime::module_registry::TenantModuleRegistry::new(),
            ),
            trading: None,
        };

        // Duplicate lookup after restart must return the same durable identity
        let found = BillingService::find_checkout(&state_restart, org, &key)
            .await
            .expect("find after restart")
            .expect("durable checkout record");
        assert_eq!(found.id, rec.id, "restart must find durable record");

        // The restarted process resolves the organization from durable storage alone:
        // re-inserting it would fail ("slug is taken"), which is the durable-authority proof.
        let slug_taken = state_restart.saas.create_organization(&org_rec).await;
        assert!(
            slug_taken.is_err(),
            "organization must already exist durably for the restarted process"
        );
        assert!(
            state_restart
                .saas
                .organization(org)
                .await
                .expect("organization")
                .is_some(),
            "restarted process must resolve the organization from durable storage"
        );

        // Re-submitting the same request against the restarted process must not create a
        // second checkout: the durable record wins.
        let replayed = BillingService::create_checkout(
            &state_restart,
            org,
            PlanCode::Starter,
            BillingProviderKind::Manual,
            key.clone(),
            None,
            None,
            Utc::now(),
        )
        .await
        .expect("replay after restart");
        assert_eq!(
            replayed.id, rec.id,
            "same org + same key after restart must return the same durable checkout"
        );
        let after_restart_count: (i64,) = sqlx::query_as(
            "SELECT COUNT(*) FROM checkout_sessions WHERE organization_id=$1 AND idempotency_key=$2",
        )
        .bind(org.as_uuid())
        .bind(&key)
        .fetch_one(db_restart.pool())
        .await
        .expect("count after restart");
        assert_eq!(after_restart_count.0, 1, "restart must not duplicate rows");

        // Concurrent replicas one record: spawn two inserts with same key concurrently
        let conc_key = format!("conc-{}", uuid::Uuid::new_v4());
        let s1 = state.clone();
        let s2 = state.clone();
        let org_c = org;
        let now_c = Utc::now();
        let k1 = conc_key.clone();
        let k2 = conc_key.clone();
        let (r1, r2) = tokio::join!(
            BillingService::create_checkout(
                &s1,
                org_c,
                PlanCode::Pro,
                BillingProviderKind::Manual,
                k1,
                None,
                None,
                now_c
            ),
            BillingService::create_checkout(
                &s2,
                org_c,
                PlanCode::Pro,
                BillingProviderKind::Manual,
                k2,
                None,
                None,
                now_c
            )
        );
        let r1 = r1.expect("concurrent 1");
        let r2 = r2.expect("concurrent 2");
        assert_eq!(
            r1.id, r2.id,
            "concurrent replicas must converge to one record"
        );
        let conc_count: (i64,) = sqlx::query_as(
            "SELECT COUNT(*) FROM checkout_sessions WHERE organization_id=$1 AND idempotency_key=$2",
        )
        .bind(org.as_uuid())
        .bind(&conc_key)
        .fetch_one(db.pool())
        .await
        .expect("count concurrent rows");
        assert_eq!(
            conc_count.0, 1,
            "two concurrent creates for the same key must leave exactly one row"
        );

        // Four concurrent requests for the same org + key must still converge on one row
        let conc4_key = format!("conc4-{}", uuid::Uuid::new_v4());
        let s_a = state.clone();
        let s_b = state.clone();
        let s_c = state.clone();
        let s_d = state.clone();
        let k_a = conc4_key.clone();
        let k_b = conc4_key.clone();
        let k_c = conc4_key.clone();
        let k_d = conc4_key.clone();
        let now4 = Utc::now();
        let (c1, c2, c3, c4) = tokio::join!(
            BillingService::create_checkout(
                &s_a,
                org,
                PlanCode::Business,
                BillingProviderKind::Manual,
                k_a,
                None,
                None,
                now4
            ),
            BillingService::create_checkout(
                &s_b,
                org,
                PlanCode::Business,
                BillingProviderKind::Manual,
                k_b,
                None,
                None,
                now4
            ),
            BillingService::create_checkout(
                &s_c,
                org,
                PlanCode::Business,
                BillingProviderKind::Manual,
                k_c,
                None,
                None,
                now4
            ),
            BillingService::create_checkout(
                &s_d,
                org,
                PlanCode::Business,
                BillingProviderKind::Manual,
                k_d,
                None,
                None,
                now4
            )
        );
        let c1 = c1.expect("concurrent 4.1");
        let c2 = c2.expect("concurrent 4.2");
        let c3 = c3.expect("concurrent 4.3");
        let c4 = c4.expect("concurrent 4.4");
        assert_eq!(c1.id, c2.id, "four concurrent must converge (1==2)");
        assert_eq!(c2.id, c3.id, "four concurrent must converge (2==3)");
        assert_eq!(c3.id, c4.id, "four concurrent must converge (3==4)");
        let conc4_count: (i64,) = sqlx::query_as(
            "SELECT COUNT(*) FROM checkout_sessions WHERE organization_id=$1 AND idempotency_key=$2",
        )
        .bind(org.as_uuid())
        .bind(&conc4_key)
        .fetch_one(db.pool())
        .await
        .expect("count concurrent-4 rows");
        assert_eq!(
            conc4_count.0, 1,
            "four concurrent creates for the same key must leave exactly one row"
        );

        // --------------------------- provider failure keeps durable pending state --
        // Stripe without LIVE_BILLING=1 must fail typed (NOT_RUN / SERVICE_UNAVAILABLE)
        // and must leave the durable row pending with no session id and no checkout URL.
        let prev_live = std::env::var("LIVE_BILLING").ok();
        let prev_key_env = std::env::var("STRIPE_API_KEY").ok();
        std::env::set_var("STRIPE_API_KEY", "sk_test_provider_failure_12345678");
        std::env::remove_var("LIVE_BILLING");
        let fail_key = format!("fail-{}", uuid::Uuid::new_v4());
        let failed = BillingService::create_checkout(
            &state,
            org,
            PlanCode::Pro,
            BillingProviderKind::Stripe,
            fail_key.clone(),
            None,
            None,
            Utc::now(),
        )
        .await;
        assert!(
            failed.is_err(),
            "Stripe checkout without LIVE_BILLING=1 must be a typed failure, never a success"
        );
        // The durable row survives as pending: no fake session id, no fake URL, no success claim.
        let (status_after_fail, session_after_fail, url_after_fail): (
            String,
            Option<String>,
            Option<String>,
        ) = sqlx::query_as(
            "SELECT status, provider_session_id, checkout_url FROM checkout_sessions WHERE organization_id=$1 AND idempotency_key=$2",
        )
        .bind(org.as_uuid())
        .bind(&fail_key)
        .fetch_one(db.pool())
        .await
        .expect("pending row after provider failure");
        assert_eq!(
            status_after_fail, "pending",
            "provider failure must leave the durable record pending"
        );
        assert!(
            session_after_fail.is_none(),
            "provider failure must not fabricate a provider session id"
        );
        assert!(
            url_after_fail.is_none(),
            "provider failure must not fabricate a checkout url"
        );
        if let Some(v) = prev_live {
            std::env::set_var("LIVE_BILLING", v);
        } else {
            std::env::remove_var("LIVE_BILLING");
        }
        if let Some(v) = prev_key_env {
            std::env::set_var("STRIPE_API_KEY", v);
        } else {
            std::env::remove_var("STRIPE_API_KEY");
        }

        // DB unavailable fail clearly: use state without db
        let shared_no_db =
            bot_core::state::AppState::new(bot_core::config::AppConfig::from_defaults());
        let state_no_db = crate::api::ApiState {
            shared: shared_no_db.clone(),
            api_key: None,
            auth: None,
            limiter: bot_core::auth::RateLimiter::new(0),
            sensitive_limiter: bot_core::auth::RateLimiter::new(10),
            audit: bot_core::audit::AuditTrail::new(None, shared_no_db.events.clone()),
            db: None,
            journal: None,
            serve_dashboard: false,
            health: std::sync::Arc::new(bot_core::obs::health::HealthRegistry::new()),
            metrics_enabled: false,
            saas: std::sync::Arc::new(crate::saas::SaasStore::new()),
            module_registry: std::sync::Arc::new(
                crate::module_runtime::module_registry::TenantModuleRegistry::new(),
            ),
            trading: None,
        };
        // Seed an org in memory for no-db test
        let org_no_db = OrganizationId::new();
        let org_no_db_rec = bot_core::tenant::Organization::new(
            org_no_db,
            format!("nodc-{}", org_no_db),
            "nodc",
            None,
            Utc::now(),
        );
        state_no_db
            .saas
            .create_organization(&org_no_db_rec)
            .await
            .expect("seed org no db");
        // In test cfg, memory fallback will succeed; in non-test cfg it would fail with db error — we verify test fallback path exists
        let rec_mem = BillingService::create_checkout(
            &state_no_db,
            org_no_db,
            PlanCode::Starter,
            BillingProviderKind::Manual,
            format!("mem-{}", uuid::Uuid::new_v4()),
            None,
            None,
            Utc::now(),
        )
        .await;
        assert!(
            rec_mem.is_ok(),
            "test memory fallback must succeed when db is None in cfg(test)"
        );
    }

    /// Provider-success half of the checkout wiring, against real PostgreSQL.
    ///
    /// The adapter's network call is live-gated (LIVE_BILLING=1 + real credentials),
    /// so this test does not fabricate a provider response. Instead it exercises the
    /// post-provider durable write with the exact function the service calls on
    /// provider success, proving: session id + checkout URL persist, the record moves
    /// to `open`, the tenant scope is enforced, a duplicate provider session id is
    /// rejected by the database, and a lost/foreign durable row fails closed instead
    /// of reporting success.
    #[tokio::test]
    async fn checkout_provider_success_persists_session_and_url_pg() {
        let Some(url) = std::env::var("POSTGRES_URL")
            .ok()
            .filter(|v| !v.trim().is_empty())
        else {
            eprintln!(
                "POSTGRES_URL not set — checkout_provider_success_persists_session_and_url_pg NOT_RUN"
            );
            return;
        };
        let cfg = bot_core::config::DatabaseConfig {
            enabled: true,
            url_env: "POSTGRES_URL".into(),
            required: true,
            auto_migrate: true,
            max_connections: 5,
            min_connections: 1,
            acquire_timeout_ms: 5000,
            statement_timeout_ms: 5000,
            query_timeout_ms: 5000,
        };
        let db = std::sync::Arc::new(
            bot_core::db::Database::connect(&cfg, &url)
                .await
                .expect("connect"),
        );
        db.migrate().await.expect("migrate");

        let shared = bot_core::state::AppState::new(bot_core::config::AppConfig::from_defaults());
        let state = crate::api::ApiState {
            shared: shared.clone(),
            api_key: None,
            auth: None,
            limiter: bot_core::auth::RateLimiter::new(0),
            sensitive_limiter: bot_core::auth::RateLimiter::new(10),
            audit: bot_core::audit::AuditTrail::new(None, shared.events.clone()),
            db: Some(db.clone()),
            journal: None,
            serve_dashboard: false,
            health: std::sync::Arc::new(bot_core::obs::health::HealthRegistry::new()),
            metrics_enabled: false,
            saas: std::sync::Arc::new(
                crate::saas::SaasStore::with_database(Some(db.clone()))
                    .await
                    .expect("saas"),
            ),
            module_registry: std::sync::Arc::new(
                crate::module_runtime::module_registry::TenantModuleRegistry::new(),
            ),
            trading: None,
        };

        let now = Utc::now();
        let mk_org = || {
            let org = OrganizationId::new();
            let rec = bot_core::tenant::Organization::new(
                org,
                format!("prov-org-{}", org),
                "prov test",
                None,
                Utc::now(),
            );
            (org, rec)
        };
        let (org, org_rec) = mk_org();
        sqlx::query("INSERT INTO organizations (id, slug, name, status, created_at, updated_at) VALUES ($1,$2,$3,$4,$5,$6) ON CONFLICT DO NOTHING")
            .bind(org.as_uuid())
            .bind(&org_rec.slug)
            .bind(&org_rec.name)
            .bind("active")
            .bind(org_rec.created_at)
            .bind(org_rec.updated_at)
            .execute(db.pool())
            .await
            .expect("insert org");
        state
            .saas
            .create_organization(&org_rec)
            .await
            .expect("seed org");

        // Durable pending Stripe checkout, created through the same durable store the
        // service uses (no provider call yet).
        let idem = format!("prov-{}", uuid::Uuid::new_v4());
        let req = CreateCheckout::new(
            org,
            PlanCode::Pro,
            BillingProviderKind::Stripe,
            idem.clone(),
            now,
        );
        let pending = CheckoutRecord::new(&req, now);
        let stored = BillingService::store_checkout_durable(&state, &pending)
            .await
            .expect("store pending");
        assert_eq!(
            stored.status,
            bot_core::billing::provider::CheckoutStatus::Pending
        );
        assert!(stored.provider_session_id.is_none());
        assert!(stored.checkout_url.is_none());

        // Values a provider returns on success — persisted, never invented by us.
        let session_id = format!("cs_test_{}", uuid::Uuid::new_v4());
        let hosted = format!("https://checkout.stripe.com/pay/{session_id}");
        let opened = BillingService::update_checkout_with_provider(
            &state,
            &stored,
            Some(session_id.clone()),
            Some(hosted.clone()),
            Utc::now(),
        )
        .await
        .expect("persist provider success");
        assert_eq!(
            opened.provider_session_id.as_deref(),
            Some(session_id.as_str())
        );
        assert_eq!(opened.checkout_url.as_deref(), Some(hosted.as_str()));
        assert_eq!(
            opened.status,
            bot_core::billing::provider::CheckoutStatus::Open
        );
        assert_eq!(opened.id, stored.id, "same durable identity");

        // The values really landed in the database, not just in the returned value.
        let (db_session, db_url, db_status): (Option<String>, Option<String>, String) =
            sqlx::query_as(
                "SELECT provider_session_id, checkout_url, status FROM checkout_sessions WHERE id=$1 AND organization_id=$2",
            )
            .bind(stored.id)
            .bind(org.as_uuid())
            .fetch_one(db.pool())
            .await
            .expect("read back");
        assert_eq!(db_session.as_deref(), Some(session_id.as_str()));
        assert_eq!(db_url.as_deref(), Some(hosted.as_str()));
        assert_eq!(db_status, "open");

        // Adding a second checkout row with the same (provider, provider_session_id)
        // must be rejected by the database — one provider session, one checkout.
        let dup = CheckoutRecord::new(
            &CreateCheckout::new(
                org,
                PlanCode::Pro,
                BillingProviderKind::Stripe,
                format!("prov-dup-{}", uuid::Uuid::new_v4()),
                now,
            ),
            now,
        );
        let dup_stored = BillingService::store_checkout_durable(&state, &dup)
            .await
            .expect("store dup pending");
        let dup_update = BillingService::update_checkout_with_provider(
            &state,
            &dup_stored,
            Some(session_id.clone()),
            Some(hosted.clone()),
            Utc::now(),
        )
        .await;
        assert!(
            dup_update.is_err(),
            "a provider session id must not be attachable to two checkouts"
        );

        // Zero-row update (foreign tenant) must fail closed, never claim success.
        let (other_org, other_rec) = mk_org();
        sqlx::query("INSERT INTO organizations (id, slug, name, status, created_at, updated_at) VALUES ($1,$2,$3,$4,$5,$6) ON CONFLICT DO NOTHING")
            .bind(other_org.as_uuid())
            .bind(&other_rec.slug)
            .bind(&other_rec.name)
            .bind("active")
            .bind(other_rec.created_at)
            .bind(other_rec.updated_at)
            .execute(db.pool())
            .await
            .expect("insert other org");
        let mut foreign = stored.clone();
        foreign.organization_id = other_org;
        let cross = BillingService::update_checkout_with_provider(
            &state,
            &foreign,
            Some(format!("cs_test_{}", uuid::Uuid::new_v4())),
            Some("https://checkout.stripe.com/pay/other".into()),
            Utc::now(),
        )
        .await;
        assert!(
            cross.is_err(),
            "a cross-tenant provider write must fail closed, not silently succeed"
        );
        // The original row is untouched by the cross-tenant attempt.
        let untouched: (Option<String>, String) = sqlx::query_as(
            "SELECT provider_session_id, status FROM checkout_sessions WHERE id=$1 AND organization_id=$2",
        )
        .bind(stored.id)
        .bind(org.as_uuid())
        .fetch_one(db.pool())
        .await
        .expect("read original after cross-tenant attempt");
        assert_eq!(untouched.0.as_deref(), Some(session_id.as_str()));
        assert_eq!(untouched.1, "open");
    }

    #[tokio::test]
    async fn invoice_pg_durable_reads() {
        let Some(url) = std::env::var("POSTGRES_URL")
            .ok()
            .filter(|v| !v.trim().is_empty())
        else {
            eprintln!("POSTGRES_URL not set — invoice_pg_durable_reads NOT_RUN");
            return;
        };
        let cfg = bot_core::config::DatabaseConfig {
            enabled: true,
            url_env: "POSTGRES_URL".into(),
            required: true,
            auto_migrate: true,
            max_connections: 5,
            min_connections: 1,
            acquire_timeout_ms: 5000,
            statement_timeout_ms: 5000,
            query_timeout_ms: 5000,
        };
        let db = std::sync::Arc::new(
            bot_core::db::Database::connect(&cfg, &url)
                .await
                .expect("connect"),
        );
        db.migrate().await.expect("migrate");
        let org = OrganizationId::new();
        let org_rec = bot_core::tenant::Organization::new(
            org,
            format!("inv-org-{}", org),
            "inv test",
            None,
            Utc::now(),
        );
        sqlx::query("INSERT INTO organizations (id, slug, name, status, created_at, updated_at) VALUES ($1,$2,$3,$4,$5,$6) ON CONFLICT DO NOTHING")
            .bind(org.as_uuid())
            .bind(&org_rec.slug)
            .bind(&org_rec.name)
            .bind("active")
            .bind(org_rec.created_at)
            .bind(org_rec.updated_at)
            .execute(db.pool()).await.expect("insert org");

        let shared = bot_core::state::AppState::new(bot_core::config::AppConfig::from_defaults());
        let state = crate::api::ApiState {
            shared: shared.clone(),
            api_key: None,
            auth: None,
            limiter: bot_core::auth::RateLimiter::new(0),
            sensitive_limiter: bot_core::auth::RateLimiter::new(10),
            audit: bot_core::audit::AuditTrail::new(None, shared.events.clone()),
            db: Some(db.clone()),
            journal: None,
            serve_dashboard: false,
            health: std::sync::Arc::new(bot_core::obs::health::HealthRegistry::new()),
            metrics_enabled: false,
            saas: std::sync::Arc::new(
                crate::saas::SaasStore::with_database(Some(db.clone()))
                    .await
                    .expect("saas"),
            ),
            module_registry: std::sync::Arc::new(
                crate::module_runtime::module_registry::TenantModuleRegistry::new(),
            ),
            trading: None,
        };
        state.saas.create_organization(&org_rec).await.ok();

        // Insert two invoices with deterministic ordering (created_at staggered)
        let now = Utc::now();
        let earlier = now - chrono::Duration::seconds(10);
        let inv1 = Invoice {
            id: InvoiceId::new(),
            organization_id: org,
            subscription_id: None,
            payment_transaction_id: None,
            provider: BillingProviderKind::Manual,
            provider_invoice_id: Some(format!("inv_1_{}", org.as_uuid())),
            invoice_number: Some(format!("INV-001-{}", org.as_uuid())),
            status: InvoiceStatus::Open,
            amount_cents: 1000,
            amount_paid_cents: 0,
            amount_due_cents: 1000,
            currency: "usd".into(),
            period_start: None,
            period_end: None,
            due_date: None,
            paid_at: None,
            hosted_url: None,
            pdf_url: None,
            created_at: earlier,
            updated_at: earlier,
        };
        let inv2 = Invoice {
            id: InvoiceId::new(),
            organization_id: org,
            subscription_id: None,
            payment_transaction_id: None,
            provider: BillingProviderKind::Manual,
            provider_invoice_id: Some(format!("inv_2_{}", org.as_uuid())),
            invoice_number: Some(format!("INV-002-{}", org.as_uuid())),
            status: InvoiceStatus::Paid,
            amount_cents: 2000,
            amount_paid_cents: 2000,
            amount_due_cents: 0,
            currency: "usd".into(),
            period_start: None,
            period_end: None,
            due_date: None,
            paid_at: Some(now),
            hosted_url: Some("https://example.com/inv2".into()),
            pdf_url: None,
            created_at: now,
            updated_at: now,
        };
        for inv in [&inv1, &inv2] {
            sqlx::query(r#"INSERT INTO invoices (id, organization_id, provider, provider_invoice_id, invoice_number, status, amount_cents, amount_paid_cents, amount_due_cents, currency, hosted_invoice_url, invoice_pdf_url, created_at, updated_at) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14) ON CONFLICT DO NOTHING"#)
                .bind(inv.id.as_uuid())
                .bind(inv.organization_id.as_uuid())
                .bind(inv.provider.as_str())
                .bind(&inv.provider_invoice_id)
                .bind(&inv.invoice_number)
                .bind(inv.status.as_str())
                .bind(inv.amount_cents)
                .bind(inv.amount_paid_cents)
                .bind(inv.amount_due_cents)
                .bind(&inv.currency)
                .bind(&inv.hosted_url)
                .bind(&inv.pdf_url)
                .bind(inv.created_at)
                .bind(inv.updated_at)
                .execute(db.pool()).await.expect("insert invoice");
        }

        // List must be tenant-scoped and deterministic order
        let list = BillingService::list_invoices(&state, org)
            .await
            .expect("list");
        assert!(list.len() >= 2);
        let pos1 = list
            .iter()
            .position(|i| i.id == inv1.id)
            .expect("inv1 in list");
        let pos2 = list
            .iter()
            .position(|i| i.id == inv2.id)
            .expect("inv2 in list");
        assert!(pos1 < pos2, "deterministic ordering by created_at");

        // Detail: tenant-isolated 200/404
        let got = BillingService::get_invoice(&state, org, inv1.id)
            .await
            .expect("get inv1");
        assert_eq!(got.id, inv1.id);
        // Cross-tenant must be 404, not leak
        let other_org = OrganizationId::new();
        let err = BillingService::get_invoice(&state, other_org, inv1.id).await;
        assert!(
            matches!(err, Err(BotError::NotFound(_))),
            "cross-tenant must be NotFound"
        );
    }
}
