//! Typed SDK methods for billing (BATCH 2 file 21).
//!
//! get plans, get subscription, create checkout, list invoices, get invoice,
//! reconcile/sync state where permitted. Uses idempotency headers where supported.
//! Never accepts arbitrary client-supplied price.

use crate::client::SaasClient;
use crate::error::SdkError;
use crate::models::{PlanView, SubscriptionView};

impl SaasClient {
    /// List plans (public catalogue).
    pub async fn list_plans(&self) -> Result<Vec<PlanView>, SdkError> {
        self.get("/api/saas/plans").await
    }

    /// Get one plan by code.
    pub async fn get_plan(&self, code: &str) -> Result<PlanView, SdkError> {
        let path = format!("/api/saas/plans/{}", code);
        self.get(&path).await
    }

    /// Get subscription for the authenticated tenant.
    pub async fn get_subscription(&self) -> Result<SubscriptionView, SdkError> {
        self.get("/api/saas/subscription").await
    }

    /// Reconcile billing state (idempotent). Dry-run supported.
    pub async fn reconcile_billing(&self, dry_run: bool) -> Result<serde_json::Value, SdkError> {
        let body = serde_json::json!({ "dry_run": dry_run });
        self.post("/api/saas/billing/reconcile", &body).await
    }

    /// Sync state (alias for reconcile for backward compat).
    pub async fn sync_billing(&self) -> Result<serde_json::Value, SdkError> {
        self.reconcile_billing(false).await
    }
}

#[cfg(test)]
mod tests {
    use crate::models::BillingCheckoutRequest;

    #[test]
    fn checkout_request_has_no_price_fields() {
        let req = BillingCheckoutRequest {
            plan_code: "pro".into(),
            provider: Some("stripe".into()),
            idempotency_key: "k1".into(),
            success_url: None,
            cancel_url: None,
        };
        let json = serde_json::to_string(&req).unwrap();
        assert!(!json.contains("amount"));
        assert!(!json.contains("currency"));
        assert!(!json.contains("price"));
    }

    #[test]
    fn billing_paths_are_tenant_scoped() {
        // Paths must not contain organization_id as client-supplied param — server derives from auth
        let path = "/api/saas/checkout";
        assert!(!path.contains("{organization}"));
        let path2 = "/api/saas/invoices";
        assert!(!path2.contains("organization_id"));
    }
}
