//! OpenAPI schema definitions for commercial endpoints (Batch 3).
//!
//! billing status, usage limits, commercial state, readiness-safe endpoint, lifecycle status.
//! Stable external schemas only, no DB internals or credentials.

use serde_json::{json, Value};

pub fn commercial_schemas() -> Value {
    json!({
        "BillingStatus": {
            "type": "object",
            "required": ["organization_id", "plan_code", "subscription_status", "billing_provider", "entitlements_active", "dunning_state", "as_of"],
            "properties": {
                "organization_id": {"type": "string", "format": "uuid"},
                "plan_code": {"type": "string", "enum": ["starter","pro","business","enterprise"]},
                "plan_version": {"type": "integer"},
                "subscription_status": {"type": "string", "enum": ["trialing","active","past_due","paused","canceled","expired"]},
                "billing_provider": {"type": "string", "enum": ["manual","stripe","paddle","test"]},
                "payment_state": {"type": ["string","null"]},
                "invoice_state": {"type": ["string","null"]},
                "entitlements_active": {"type": "boolean"},
                "usage": {"type": "object"},
                "dunning_state": {"type": "string", "enum": ["current","payment_failed","retry_pending","grace_period","billing_suspended","recovered","manually_resolved"]},
                "grace_until": {"type": ["string","null"], "format":"date-time"},
                "suspension_reason": {"type": ["string","null"]},
                "as_of": {"type":"string","format":"date-time"}
            }
        },
        "UsageLimits": {
            "type":"object",
            "required":["organization_id","period","plan_code","limits","as_of"],
            "properties":{
                "organization_id":{"type":"string","format":"uuid"},
                "period":{"type":"string","pattern":"^[0-9]{4}-[0-9]{2}$"},
                "plan_code":{"type":"string"},
                "limits":{"type":"array","items":{"type":"object","required":["feature","state","allows"],"properties":{
                    "feature":{"type":"string"},
                    "state":{"type":"string","enum":["within_allowance","soft_limit_exceeded","hard_limit_exceeded","suspended"]},
                    "limit":{"type":["number","null"]},
                    "current":{"type":"number"},
                    "remaining":{"type":["number","null"]},
                    "allows":{"type":"boolean"}
                }}},
                "as_of":{"type":"string","format":"date-time"}
            }
        },
        "CommercialState": {
            "type":"object",
            "required":["organization_id","plan_code","subscription_status","billing_provider","entitlements_active","dunning_state","lifecycle_status","commercial_consistent","as_of"],
            "properties":{
                "organization_id":{"type":"string","format":"uuid"},
                "plan_code":{"type":"string"},
                "subscription_status":{"type":"string"},
                "billing_provider":{"type":"string"},
                "entitlements_active":{"type":"boolean"},
                "dunning_state":{"type":"string"},
                "usage":{"type":"object"},
                "lifecycle_status":{"type":"string","enum":["active","suspended","closed","pending"]},
                "suspension_reason":{"type":["string","null"]},
                "commercial_consistent":{"type":"boolean"},
                "as_of":{"type":"string","format":"date-time"}
            }
        },
        "ReadinessPublic": {
            "type":"object",
            "required":["ok","version","as_of","services"],
            "properties":{
                "ok":{"type":"boolean"},
                "version":{"type":"string"},
                "as_of":{"type":"string","format":"date-time"},
                "services":{"type":"object"}
            }
        },
        "ReadinessOperator": {
            "type":"object",
            "required":["ok","version","as_of","services","diagnostics"],
            "properties":{
                "ok":{"type":"boolean"},
                "version":{"type":"string"},
                "as_of":{"type":"string","format":"date-time"},
                "services":{"type":"object"},
                "diagnostics":{"type":"object","description":"redacted — no secrets"}
            }
        }
    })
}

pub fn commercial_paths() -> Value {
    json!({
        "/api/saas/billing/status": {
            "get": {
                "operationId":"saas.billingStatus",
                "tags":["billing"],
                "security":[{"bearerAuth":[]}],
                "responses":{
                    "200":{"description":"Billing status","content":{"application/json":{"schema":{"$ref":"#/components/schemas/BillingStatus"}}}},
                    "401":{"$ref":"#/components/responses/Unauthorized"},
                    "403":{"$ref":"#/components/responses/Forbidden"},
                    "404":{"description":"Organization not found"}
                }
            }
        },
        "/api/saas/usage/limits": {
            "get": {
                "operationId":"saas.usageLimits",
                "tags":["billing"],
                "security":[{"bearerAuth":[]}],
                "responses":{
                    "200":{"description":"Usage vs limits","content":{"application/json":{"schema":{"$ref":"#/components/schemas/UsageLimits"}}}},
                    "401":{"$ref":"#/components/responses/Unauthorized"}
                }
            }
        },
        "/api/saas/commercial/state": {
            "get": {
                "operationId":"saas.commercialState",
                "tags":["billing"],
                "security":[{"bearerAuth":[]}],
                "responses":{
                    "200":{"description":"Aggregated commercial state","content":{"application/json":{"schema":{"$ref":"#/components/schemas/CommercialState"}}}},
                    "401":{"$ref":"#/components/responses/Unauthorized"}
                }
            }
        },
        "/api/saas/readiness": {
            "get": {
                "operationId":"saas.readinessPublic",
                "tags":["ops"],
                "responses":{
                    "200":{"description":"Public readiness (safe)","content":{"application/json":{"schema":{"$ref":"#/components/schemas/ReadinessPublic"}}}}
                }
            }
        },
        "/api/saas/readiness/operator": {
            "get": {
                "operationId":"saas.readinessOperator",
                "tags":["ops"],
                "security":[{"bearerAuth":[]}],
                "responses":{
                    "200":{"description":"Operator readiness (redacted)","content":{"application/json":{"schema":{"$ref":"#/components/schemas/ReadinessOperator"}}}},
                    "401":{"$ref":"#/components/responses/Unauthorized"},
                    "403":{"description":"Operator only"}
                }
            }
        },
        "/api/saas/data-lifecycle/{id}/status": {
            "get": {
                "operationId":"saas.lifecycleStatus",
                "tags":["lifecycle"],
                "security":[{"bearerAuth":[]}],
                "parameters":[{"name":"id","in":"path","required":true,"schema":{"type":"string","format":"uuid"}}],
                "responses":{
                    "200":{"description":"Lifecycle status"},
                    "404":{"description":"Not found"}
                }
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schemas_serializable() {
        let s = commercial_schemas().to_string();
        assert!(s.contains("BillingStatus"));
        assert!(s.contains("CommercialState"));
    }

    #[test]
    fn no_secret_fields() {
        let s = commercial_schemas().to_string().to_ascii_lowercase()
            + &commercial_paths().to_string().to_ascii_lowercase();
        for banned in ["secret", "password", "private_key", "api_key", "token"] {
            // Ensure we don't expose secret-bearing fields as required properties
            // Paths may contain 401/403 but not secret fields
            assert!(
                !s.contains(&format!("\"{}\"", banned)),
                "should not expose {}",
                banned
            );
        }
    }

    #[test]
    fn expected_status_codes() {
        let p = commercial_paths().to_string();
        for code in ["200", "401", "403", "404"] {
            assert!(p.contains(code));
        }
    }
}
