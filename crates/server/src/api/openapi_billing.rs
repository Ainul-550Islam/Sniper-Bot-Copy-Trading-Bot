//! OpenAPI schema definitions for billing endpoints (BATCH 2 file 17).
//!
//! Defines request/response/error schemas for checkout, invoices,
//! billing state, payment reconciliation, webhook behavior. Does not
//! expose internal DB structs directly.

use serde_json::{json, Value};

fn error_ref() -> Value {
    json!({ "$ref": "#/components/schemas/Error" })
}
fn ok_ref(schema: &str) -> Value {
    json!({ "$ref": format!("#/components/schemas/{}", schema) })
}

/// Shared billing schemas.
pub fn billing_schemas() -> Value {
    json!({
        "CheckoutRequest": {
            "type": "object",
            "required": ["plan_code", "idempotency_key"],
            "properties": {
                "plan_code": { "type": "string", "enum": ["starter", "pro", "business", "enterprise"] },
                "provider": { "type": "string", "enum": ["manual", "stripe", "paddle"], "default": "manual" },
                "idempotency_key": { "type": "string", "minLength": 1, "maxLength": 128 },
                "success_url": { "type": "string", "format": "uri" },
                "cancel_url": { "type": "string", "format": "uri" }
            },
            "additionalProperties": false,
            "description": "Client supplies only plan_code + idempotency, never amount/currency."
        },
        "CheckoutResponse": {
            "type": "object",
            "required": ["id", "organization_id", "plan_code", "provider", "status"],
            "properties": {
                "id": { "type": "string", "format": "uuid" },
                "organization_id": { "type": "string", "format": "uuid" },
                "plan_code": { "type": "string" },
                "provider": { "type": "string" },
                "status": { "type": "string", "enum": ["pending", "open", "completed", "expired", "canceled"] },
                "checkout_url": { "type": ["string", "null"], "format": "uri" },
                "instructions": { "type": ["string", "null"] },
                "expires_at": { "type": ["string", "null"], "format": "date-time" }
            }
        },
        "InvoiceView": {
            "type": "object",
            "required": ["id", "organization_id", "provider", "status", "amount_cents", "currency"],
            "properties": {
                "id": { "type": "string", "format": "uuid" },
                "organization_id": { "type": "string", "format": "uuid" },
                "provider": { "type": "string", "enum": ["manual", "stripe", "paddle"] },
                "provider_invoice_id": { "type": ["string", "null"] },
                "status": { "type": "string", "enum": ["draft", "open", "paid", "void", "uncollectible", "refunded"] },
                "amount_cents": { "type": "integer", "minimum": 0 },
                "amount_paid_cents": { "type": "integer" },
                "amount_due_cents": { "type": "integer" },
                "currency": { "type": "string", "pattern": "^[a-z]{3}$" },
                "period_start": { "type": ["string", "null"], "format": "date-time" },
                "period_end": { "type": ["string", "null"], "format": "date-time" },
                "hosted_url": { "type": ["string", "null"], "format": "uri" }
            }
        },
        "BillingState": {
            "type": "object",
            "required": ["organization_id", "subscription_status", "entitlement_active"],
            "properties": {
                "organization_id": { "type": "string", "format": "uuid" },
                "subscription_status": { "type": ["string", "null"], "enum": ["active", "trialing", "past_due", "canceled", "suspended", "expired", null] },
                "provider": { "type": ["string", "null"] },
                "entitlement_active": { "type": "boolean" },
                "as_of": { "type": "string", "format": "date-time" }
            }
        },
        "ReconcileRequest": {
            "type": "object",
            "properties": {
                "provider": { "type": "string", "enum": ["stripe", "paddle", "manual"] },
                "provider_event_id": { "type": "string" },
                "event_kind": { "type": "string", "enum": ["payment_succeeded","payment_failed","subscription_created","subscription_updated","subscription_canceled","invoice_created","invoice_paid","invoice_failed","refund_created","unknown"] },
                "dry_run": { "type": "boolean", "default": false }
            },
            "additionalProperties": false
        },
        "ReconcileResponse": {
            "type": "object",
            "required": ["organization_id", "action", "reason"],
            "properties": {
                "organization_id": { "type": "string", "format": "uuid" },
                "action": { "type": "string", "enum": ["no_op","update","suspend","restore","investigate"] },
                "reason": { "type": "string" },
                "idempotent": { "type": "boolean" },
                "decided_at": { "type": "string", "format": "date-time" },
                "internal_subscription_status": { "type": ["string","null"] },
                "provider_event_kind": { "type": "string" }
            }
        },
        // NOTE: `WebhookAck` is declared identically in the base document
        // and is kept there, so the two cannot drift apart.
    })
}

/// Paths contributed by billing.
pub fn billing_paths() -> Value {
    json!({
        // NOTE: `/api/saas/checkout` lives in the base document
        // (`saas::openapi`), which declares the full request body and the
        // 409/422 cases. One path, one source of truth.

        // NOTE: `/api/saas/invoices` lives in the base document, which also
        // declares 403/429. One path, one source of truth.
        "/api/saas/billing/reconcile": {
            "post": {
                "operationId": "saas.reconcileBilling",
                "tags": ["billing"],
                "security": [{"bearerAuth": []}],
                "requestBody": { "required": true, "content": { "application/json": { "schema": ok_ref("ReconcileRequest") } } },
                "responses": {
                    "200": { "description": "Reconciliation decision (idempotent)", "content": { "application/json": { "schema": ok_ref("ReconcileResponse") } } },
                    "401": { "$ref": "#/components/responses/Unauthorized" }
                }
            }
        },
        // NOTE: `/api/saas/billing/webhooks/{provider}` is deliberately NOT
        // declared here. The base document in `saas::openapi` already
        // declares it with a richer contract (422 and 501 responses, a
        // typed request body). Declaring it in both places made the
        // merge fail loudly rather than silently pick a winner; the base
        // document is the single source of truth for this path.
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schemas_do_not_expose_internal_db_structs() {
        let s = billing_schemas().to_string();
        for banned in ["secret_hash", "password_hash", "token_hash", "private_key"] {
            assert!(!s.contains(banned));
        }
        assert!(s.contains("CheckoutRequest"));
        assert!(s.contains("ReconcileResponse"));
    }

    #[test]
    fn checkout_request_has_no_amount_field() {
        let schema = &billing_schemas()["CheckoutRequest"];
        let props = schema["properties"].as_object().unwrap();
        assert!(!props.contains_key("amount_cents"));
        assert!(!props.contains_key("currency"));
        assert!(props.contains_key("plan_code"));
        assert!(props.contains_key("idempotency_key"));
    }
}
