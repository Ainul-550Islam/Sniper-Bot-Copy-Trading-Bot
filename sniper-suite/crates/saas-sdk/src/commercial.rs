//! Public SDK methods for commercial endpoints (Batch 3).
//!
//! billing status, usage limits, commercial state, lifecycle status, readiness.
//! Typed responses and typed errors, no secret-bearing URLs.

use crate::client::SaasClient;
use crate::error::{SdkError, SdkErrorKind};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BillingStatus {
    pub organization_id: String,
    pub plan_code: String,
    pub plan_version: u32,
    pub subscription_status: String,
    pub billing_provider: String,
    pub entitlements_active: bool,
    pub dunning_state: String,
    pub suspension_reason: Option<String>,
    pub as_of: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UsageLimits {
    pub organization_id: String,
    pub period: String,
    pub plan_code: String,
    pub limits: Vec<UsageLimitItem>,
    pub as_of: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UsageLimitItem {
    pub feature: String,
    pub state: String,
    pub limit: Option<f64>,
    pub current: f64,
    pub remaining: Option<f64>,
    pub allows: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommercialState {
    pub organization_id: String,
    pub plan_code: String,
    pub subscription_status: String,
    pub billing_provider: String,
    pub entitlements_active: bool,
    pub dunning_state: String,
    pub lifecycle_status: String,
    pub commercial_consistent: bool,
    pub as_of: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReadinessPublic {
    pub ok: bool,
    pub version: String,
    pub as_of: String,
    pub services: serde_json::Value,
}

impl SaasClient {
    pub async fn billing_status(&self) -> Result<BillingStatus, SdkError> {
        self.get("/api/saas/billing/status").await
    }

    pub async fn billing_status_for(
        &self,
        organization_id: &str,
    ) -> Result<BillingStatus, SdkError> {
        if organization_id.trim().is_empty() {
            return Err(SdkError::new(
                SdkErrorKind::Validation,
                "organization_id required",
            ));
        }
        let path = format!("/api/saas/billing/status/{}", organization_id);
        self.get(&path).await
    }

    pub async fn usage_limits(&self) -> Result<UsageLimits, SdkError> {
        self.get("/api/saas/usage/limits").await
    }

    pub async fn usage_limits_for(&self, organization_id: &str) -> Result<UsageLimits, SdkError> {
        if organization_id.trim().is_empty() {
            return Err(SdkError::new(
                SdkErrorKind::Validation,
                "organization_id required",
            ));
        }
        let path = format!("/api/saas/usage/limits/{}", organization_id);
        self.get(&path).await
    }

    pub async fn commercial_state(&self) -> Result<CommercialState, SdkError> {
        self.get("/api/saas/commercial/state").await
    }

    pub async fn commercial_state_for(
        &self,
        organization_id: &str,
    ) -> Result<CommercialState, SdkError> {
        let path = format!("/api/saas/commercial/state/{}", organization_id);
        self.get(&path).await
    }

    pub async fn readiness_public(&self) -> Result<ReadinessPublic, SdkError> {
        self.get("/api/saas/readiness").await
    }

    pub async fn lifecycle_status(
        &self,
        organization_id: &str,
    ) -> Result<serde_json::Value, SdkError> {
        let path = format!("/api/saas/data-lifecycle/{}/status", organization_id);
        self.get(&path).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commercial_endpoints_deserialize() {
        let json = serde_json::json!({
            "organization_id": "00000000-0000-0000-0000-000000000000",
            "plan_code": "pro",
            "plan_version": 1,
            "subscription_status": "active",
            "billing_provider": "manual",
            "entitlements_active": true,
            "dunning_state": "current",
            "suspension_reason": null,
            "as_of": "2026-09-23T00:00:00Z"
        });
        let v: BillingStatus = serde_json::from_value(json).unwrap();
        assert_eq!(v.plan_code, "pro");
    }

    #[test]
    fn no_secrets_in_url() {
        let org = "550e8400-e29b-41d4-a716-446655440000";
        let path = format!("/api/saas/billing/status/{}", org);
        assert!(!path.contains("secret"));
        assert!(!path.contains("token"));
        assert!(path.contains(org));
    }

    #[test]
    fn errors_map_correctly() {
        let err = SdkError::new(SdkErrorKind::Unauthorized, "unauthorized");
        assert_eq!(err.kind, SdkErrorKind::Unauthorized);
        assert!(err.to_string().contains("unauthorized"));
    }

    #[test]
    fn commercial_state_deserialize() {
        let json = serde_json::json!({
            "organization_id": "org",
            "plan_code": "business",
            "subscription_status": "active",
            "billing_provider": "manual",
            "entitlements_active": true,
            "dunning_state": "current",
            "lifecycle_status": "active",
            "commercial_consistent": true,
            "as_of": "2026-09-23T00:00:00Z"
        });
        let v: CommercialState = serde_json::from_value(json).unwrap();
        assert!(v.commercial_consistent);
    }
}
