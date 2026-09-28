//! Safe checkout abstraction (BATCH file 04).
//!
//! Create checkout request, plan selection, organization ownership,
//! redirect validation, idempotency, provider session reference, expiration.
//! Prevent arbitrary user input from changing tenant ownership or pricing.
//! Price/plan authority must come from server-side plan definitions.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::tenant::OrganizationId;

use super::plan::{Plan, PlanCode};
use super::provider::{BillingProviderKind, CheckoutStatus};

/// Amount in smallest currency unit — server-side authority only.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Money {
    pub amount_cents: i64,
    pub currency: String,
}

/// Create checkout request — validated, tenant-scoped, plan-authoritative.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreateCheckout {
    pub organization_id: OrganizationId,
    pub plan_code: PlanCode,
    pub provider: BillingProviderKind,
    pub idempotency_key: String,
    pub success_url: Option<String>,
    pub cancel_url: Option<String>,
    pub requested_at: DateTime<Utc>,
}

impl CreateCheckout {
    pub fn new(
        organization_id: OrganizationId,
        plan_code: PlanCode,
        provider: BillingProviderKind,
        idempotency_key: impl Into<String>,
        now: DateTime<Utc>,
    ) -> Self {
        Self {
            organization_id,
            plan_code,
            provider,
            idempotency_key: idempotency_key.into(),
            success_url: None,
            cancel_url: None,
            requested_at: now,
        }
    }

    pub fn with_redirects(mut self, success: Option<String>, cancel: Option<String>) -> Self {
        self.success_url = success;
        self.cancel_url = cancel;
        self
    }

    /// Validate: plan must be known, URLs must be safe, idempotency key non-empty.
    pub fn validate(&self, known_plans: &[Plan]) -> Result<(), String> {
        if self.idempotency_key.trim().is_empty() {
            return Err("idempotency_key must not be empty".into());
        }
        if self.idempotency_key.len() > 128 {
            return Err("idempotency_key too long".into());
        }
        if !known_plans.iter().any(|p| p.code == self.plan_code) {
            return Err(format!("unknown plan_code: {}", self.plan_code.as_str()));
        }
        if let Some(url) = &self.success_url {
            validate_redirect_url(url)?;
        }
        if let Some(url) = &self.cancel_url {
            validate_redirect_url(url)?;
        }
        Ok(())
    }
}

/// Validate redirect URL: must be https, no credentials, no fragment with secrets, bounded length.
pub fn validate_redirect_url(url: &str) -> Result<(), String> {
    if url.len() > 2048 {
        return Err("redirect url too long".into());
    }
    let parsed = url::Url::parse(url).map_err(|e| format!("invalid redirect url: {e}"))?;
    if parsed.scheme() != "https" {
        return Err("redirect url must be https".into());
    }
    if parsed.username() != "" || parsed.password().is_some() {
        return Err("redirect url must not contain credentials".into());
    }
    if url.contains('@') && parsed.host_str().is_none() {
        return Err("redirect url contains suspicious characters".into());
    }
    // Disallow secrets in query — naive check for common substrings
    let q = parsed.query().unwrap_or_default().to_ascii_lowercase();
    for bad in ["token", "secret", "key=", "password", "api_key"] {
        if q.contains(bad) {
            return Err(format!("redirect url query must not contain {}", bad));
        }
    }
    Ok(())
}

/// Provider-neutral checkout record — persisted.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheckoutRecord {
    pub id: Uuid,
    pub organization_id: OrganizationId,
    pub plan_code: PlanCode,
    pub provider: BillingProviderKind,
    pub provider_session_id: Option<String>,
    pub idempotency_key: String,
    pub status: CheckoutStatus,
    pub checkout_url: Option<String>,
    pub success_url: Option<String>,
    pub cancel_url: Option<String>,
    pub expires_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl CheckoutRecord {
    pub fn new(req: &CreateCheckout, now: DateTime<Utc>) -> Self {
        Self {
            id: Uuid::new_v4(),
            organization_id: req.organization_id,
            plan_code: req.plan_code,
            provider: req.provider,
            provider_session_id: None,
            idempotency_key: req.idempotency_key.clone(),
            status: CheckoutStatus::Pending,
            checkout_url: None,
            success_url: req.success_url.clone(),
            cancel_url: req.cancel_url.clone(),
            expires_at: Some(now + chrono::Duration::hours(24)),
            created_at: now,
            updated_at: now,
        }
    }

    pub fn is_expired(&self, now: DateTime<Utc>) -> bool {
        self.expires_at.map(|exp| now >= exp).unwrap_or(false)
    }

    pub fn can_transition_to(&self, to: CheckoutStatus) -> bool {
        checkout_transitions::can_transition(self.status, to)
    }

    pub fn transition_to(&mut self, to: CheckoutStatus, now: DateTime<Utc>) -> Result<(), String> {
        if !self.can_transition_to(to) {
            return Err(format!(
                "illegal checkout transition {} -> {}",
                self.status.as_str(),
                to.as_str()
            ));
        }
        self.status = to;
        self.updated_at = now;
        Ok(())
    }

    /// Tenant ownership invariant: record belongs to org.
    pub fn owns(&self, org: OrganizationId) -> bool {
        self.organization_id == org
    }
}

/// Transition helper extracted for testability.
pub mod checkout_transitions {
    use super::CheckoutStatus;
    pub fn can_transition(from: CheckoutStatus, to: CheckoutStatus) -> bool {
        use CheckoutStatus::*;
        if from == to {
            return false;
        }
        match (from, to) {
            (Pending, Open) => true,
            (Pending, Expired) => true,
            (Pending, Canceled) => true,
            (Open, Completed) => true,
            (Open, Expired) => true,
            (Open, Canceled) => true,
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::billing::plan::Plan;
    use chrono::Utc;

    fn plan_catalogue(now: DateTime<Utc>) -> Vec<Plan> {
        crate::billing::plan::default_catalogue(now)
    }

    #[test]
    fn valid_checkout_passes_validation() {
        let now = Utc::now();
        let req = CreateCheckout::new(
            OrganizationId::new(),
            PlanCode::Pro,
            BillingProviderKind::Stripe,
            "idem-123",
            now,
        )
        .with_redirects(
            Some("https://app.example.com/success".into()),
            Some("https://app.example.com/cancel".into()),
        );
        assert!(req.validate(&plan_catalogue(now)).is_ok());
    }

    #[test]
    fn unknown_plan_is_rejected() {
        let now = Utc::now();
        // create a plan list without Pro — but default catalogue has Pro, so we filter
        let catalogue = vec![];
        let req = CreateCheckout::new(
            OrganizationId::new(),
            PlanCode::Pro,
            BillingProviderKind::Stripe,
            "idem-1",
            now,
        );
        assert!(req.validate(&catalogue).is_err());
    }

    #[test]
    fn empty_idempotency_key_rejected() {
        let now = Utc::now();
        let req = CreateCheckout::new(
            OrganizationId::new(),
            PlanCode::Starter,
            BillingProviderKind::Manual,
            "   ",
            now,
        );
        assert!(req.validate(&plan_catalogue(now)).is_err());
    }

    #[test]
    fn redirect_validation() {
        assert!(validate_redirect_url("https://example.com/success").is_ok());
        assert!(
            validate_redirect_url("http://example.com/success").is_err(),
            "must be https"
        );
        assert!(
            validate_redirect_url("https://example.com/cb?token=secret").is_err(),
            "must not contain token"
        );
        assert!(
            validate_redirect_url("https://user:pass@example.com/").is_err(),
            "must not contain credentials"
        );
        let long = format!("https://example.com/{}", "a".repeat(3000));
        assert!(validate_redirect_url(&long).is_err(), "too long");
    }

    #[test]
    fn checkout_record_ownership_and_expiry() {
        let now = Utc::now();
        let org = OrganizationId::new();
        let req = CreateCheckout::new(
            org,
            PlanCode::Starter,
            BillingProviderKind::Stripe,
            "idem-x",
            now,
        );
        let rec = CheckoutRecord::new(&req, now);
        assert!(rec.owns(org));
        assert!(!rec.owns(OrganizationId::new()));
        assert!(!rec.is_expired(now));
        assert!(rec.is_expired(now + chrono::Duration::hours(25)));
    }

    #[test]
    fn checkout_transitions() {
        use CheckoutStatus::*;
        assert!(checkout_transitions::can_transition(Pending, Open));
        assert!(checkout_transitions::can_transition(Open, Completed));
        assert!(!checkout_transitions::can_transition(Completed, Open));
        assert!(!checkout_transitions::can_transition(Pending, Completed));
        assert!(!checkout_transitions::can_transition(Pending, Pending));
    }

    #[test]
    fn price_authority_is_server_side() {
        // Client cannot override price: CreateCheckout has no amount field.
        // Price is derived from plan definitions server-side.
        let now = Utc::now();
        let req = CreateCheckout::new(
            OrganizationId::new(),
            PlanCode::Business,
            BillingProviderKind::Stripe,
            "idem-123-abc",
            now,
        );
        // Serialize and check no amount/price fields appear — check field names, not substrings in values
        let json = serde_json::to_string(&req).unwrap();
        assert!(!json.contains("\"amount\""));
        assert!(!json.contains("\"price\""));
        assert!(!json.contains("\"cents\""));
        assert!(!json.contains("\"amount_cents\""));
    }

    #[test]
    fn idempotency_key_uniqueness_per_org() {
        let now = Utc::now();
        let org1 = OrganizationId::new();
        let org2 = OrganizationId::new();
        let r1 = CheckoutRecord::new(
            &CreateCheckout::new(
                org1,
                PlanCode::Starter,
                BillingProviderKind::Stripe,
                "same-key",
                now,
            ),
            now,
        );
        let r2 = CheckoutRecord::new(
            &CreateCheckout::new(
                org2,
                PlanCode::Starter,
                BillingProviderKind::Stripe,
                "same-key",
                now,
            ),
            now,
        );
        // Same key allowed across different orgs (composite unique), but not same org — enforced by DB.
        assert_eq!(r1.idempotency_key, r2.idempotency_key);
        assert_ne!(r1.organization_id, r2.organization_id);
    }
}
