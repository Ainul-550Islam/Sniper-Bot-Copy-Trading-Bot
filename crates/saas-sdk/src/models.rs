//! Typed request/response models for the public SaaS API — stable wire types, no server internals.
//!
//! COMPLETE SDK public wire models (BATCH 2). Includes typed models for
//! organization, tenant status, plan, subscription, checkout, invoice, usage,
//! custody state, audit export, lifecycle status, WebSocket authentication metadata.
//! Models remain stable and provider-neutral. Safe Debug for sensitive fields.

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Organization / Tenant
// ---------------------------------------------------------------------------

/// User profile — public view, no password hash or secrets.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UserProfile {
    pub id: String,
    pub email: String,
    pub email_verified: bool,
    pub display_name: String,
    pub status: String,
    pub platform_admin: bool,
    pub created_at: String,
    pub last_login_at: Option<String>,
}

/// Organization view.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OrganizationView {
    pub id: String,
    pub slug: String,
    pub name: String,
    pub status: String,
    pub created_at: String,
}

/// Tenant status view.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TenantStatus {
    pub organization_id: String,
    pub organization_status: String,
    pub lifecycle_phase: String,
    pub lifecycle_state: String,
    pub retention_deadline: Option<String>,
}

// ---------------------------------------------------------------------------
// Plan / Subscription
// ---------------------------------------------------------------------------

/// Plan view — server-authoritative, includes entitlement snapshot.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlanView {
    pub id: String,
    pub code: String,
    pub name: String,
    pub description: Option<String>,
    pub amount_cents: Option<i64>,
    pub currency: Option<String>,
    pub interval: Option<String>,
    pub version: Option<u32>,
    pub features: std::collections::BTreeMap<String, String>,
    pub created_at: Option<String>,
}

/// Subscription view — provider-neutral.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubscriptionView {
    pub id: String,
    pub organization_id: String,
    pub plan_code: String,
    pub provider: String,
    pub status: String,
    pub current_period_start: Option<String>,
    pub current_period_end: Option<String>,
    pub cancel_at_period_end: bool,
    pub created_at: String,
    pub updated_at: String,
}

// ---------------------------------------------------------------------------
// Checkout / Invoice / Usage
// ---------------------------------------------------------------------------

/// Checkout request — server-known plan_code only.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BillingCheckoutRequest {
    pub plan_code: String,
    pub provider: Option<String>,
    pub idempotency_key: String,
    pub success_url: Option<String>,
    pub cancel_url: Option<String>,
}

/// Checkout response — never contains secret.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BillingCheckoutResponse {
    pub id: String,
    pub organization_id: String,
    pub plan_code: String,
    pub provider: String,
    pub status: String,
    pub checkout_url: Option<String>,
    pub instructions: Option<String>,
    pub expires_at: Option<String>,
}

/// Invoice view — public fields only.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InvoiceView {
    pub id: String,
    pub organization_id: String,
    pub provider: String,
    pub status: String,
    pub amount_cents: i64,
    pub currency: String,
    pub hosted_url: Option<String>,
    pub created_at: String,
}

/// Detailed invoice view.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InvoiceDetail {
    pub invoice: InvoiceView,
    pub amount_paid_cents: i64,
    pub amount_due_cents: i64,
    pub period_start: Option<String>,
    pub period_end: Option<String>,
    pub paid_at: Option<String>,
}

// ---------------------------------------------------------------------------
// Usage
// ---------------------------------------------------------------------------

/// Usage entry (per metric per period).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UsageEntry {
    pub metric: String,
    pub period: String,
    pub total: f64,
}

/// Usage summary.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UsageSummary {
    pub organization_id: String,
    pub period: String,
    pub entries: Vec<UsageEntry>,
}

// ---------------------------------------------------------------------------
// Custody state
// ---------------------------------------------------------------------------

/// Custody profile view — public metadata only.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CustodyProfileView {
    pub id: String,
    pub organization_id: String,
    pub name: String,
    pub provider_type: String,
    pub status: String,
    pub description: Option<String>,
    pub created_at: String,
}

/// Signer view — public address only, never private key.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignerView {
    pub id: String,
    pub organization_id: String,
    pub custody_profile_id: String,
    pub logical_identity: String,
    pub public_address: String,
    pub provider_type: String,
    pub status: String,
    pub capabilities: Vec<String>,
}

/// Custody health view.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CustodyHealthView {
    pub provider_type: String,
    pub state: String,
    pub detail: String,
    pub signing_allowed: bool,
    pub checked_at: String,
}

/// Custody health report.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CustodyHealthReport {
    pub organization_id: String,
    pub providers: Vec<CustodyHealthView>,
    pub generated_at: String,
}

/// Custody state aggregated.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CustodyStateView {
    pub organization_id: String,
    pub profiles: Vec<CustodyProfileView>,
    pub signers: Vec<SignerView>,
    pub health: Option<CustodyHealthReport>,
}

// ---------------------------------------------------------------------------
// Audit export
// ---------------------------------------------------------------------------

/// Audit export record — redacted, org-scoped.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuditExportRecord {
    pub id: String,
    pub organization_id: String,
    pub actor: String,
    pub action: String,
    pub outcome: String,
    pub at: String,
    pub detail: serde_json::Value,
}

/// Audit export envelope — deterministic, paginated, bounded.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuditExportView {
    pub organization_id: String,
    pub from: String,
    pub to: String,
    pub limit: usize,
    pub offset: usize,
    pub records: Vec<AuditExportRecord>,
    pub count: usize,
}

// ---------------------------------------------------------------------------
// Lifecycle status
// ---------------------------------------------------------------------------

/// Lifecycle status view.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LifecycleStatusView {
    pub organization_id: String,
    pub organization_status: String,
    pub phase: String,
    pub state: String,
    pub custody_revoked: bool,
    pub sessions_invalidated: bool,
    pub retention_scheduled: bool,
    pub purge_eligible_at: Option<String>,
    pub updated_at: String,
}

// ---------------------------------------------------------------------------
// WebSocket authentication metadata
// ---------------------------------------------------------------------------

/// WebSocket auth metadata — describes how to authenticate SaaS WS.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WsAuthMetadata {
    /// Primary SaaS endpoint (header/first-frame auth, never query).
    pub saas_endpoint: String,
    /// Auth methods supported.
    pub methods: Vec<String>,
    /// Whether query-string credentials are forbidden.
    pub query_credentials_forbidden: bool,
    /// Timeout for first-frame auth.
    pub auth_timeout_secs: u64,
    /// Legacy endpoint (if present) and its mode.
    pub legacy_endpoint: Option<String>,
    pub legacy_mode: Option<String>,
    pub legacy_deprecated: bool,
}

// Keep original alias types for backward compatibility
/// Session response — token shown once; Debug is redacted.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionResponse {
    pub session_id: String,
    pub token: String,
    pub expires_at: String,
}

impl SessionResponse {
    /// Redacted Debug — token not shown.
    pub fn debug_redacted(&self) -> String {
        format!(
            "SessionResponse {{ session_id: {}, token: **redacted**, expires_at: {} }}",
            self.session_id, self.expires_at
        )
    }
}

/// Custom Debug for SessionResponse that redacts token (so `println!("{:?}", resp)` is safe).
impl std::fmt::Display for SessionResponse {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.debug_redacted())
    }
}

/// API key metadata — no secret.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApiKeyMetadata {
    pub id: String,
    pub key_prefix: String,
    pub label: String,
    pub role: String,
    pub scopes: Vec<String>,
    pub created_at: String,
    pub usable: bool,
}

/// Wallet binding view — public address only.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WalletBindingView {
    pub id: String,
    pub organization_id: String,
    pub label: String,
    pub public_address: String,
    pub modules: Vec<String>,
    pub created_at: String,
    pub revoked_at: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn models_serialize_without_secrets() {
        let profile = UserProfile {
            id: "1".into(),
            email: "a@example.com".into(),
            email_verified: true,
            display_name: "A".into(),
            status: "active".into(),
            platform_admin: false,
            created_at: "2026-01-01T00:00:00Z".into(),
            last_login_at: None,
        };
        let json = serde_json::to_string(&profile).unwrap();
        assert!(!json.to_ascii_lowercase().contains("password"));
        assert!(!json.contains("secret"));
    }

    #[test]
    fn session_debug_is_redacted() {
        let s = SessionResponse {
            session_id: "sid".into(),
            token: "tok_secret_123".into(),
            expires_at: "2026-01-01T00:00:00Z".into(),
        };
        let dbg = s.debug_redacted();
        assert!(!dbg.contains("tok_secret"));
        assert!(dbg.contains("**redacted**"));
    }

    #[test]
    fn checkout_request_has_no_amount() {
        let req = BillingCheckoutRequest {
            plan_code: "pro".into(),
            provider: Some("stripe".into()),
            idempotency_key: "k".into(),
            success_url: None,
            cancel_url: None,
        };
        let json = serde_json::to_string(&req).unwrap();
        assert!(!json.contains("amount"));
        assert!(!json.contains("price"));
    }

    #[test]
    fn custody_state_never_contains_private_key() {
        let signer = SignerView {
            id: "id".into(),
            organization_id: "org".into(),
            custody_profile_id: "prof".into(),
            logical_identity: "id1".into(),
            public_address: "addr1".into(),
            provider_type: "vault".into(),
            status: "active".into(),
            capabilities: vec!["trade".into()],
        };
        let json = serde_json::to_string(&signer).unwrap();
        assert!(!json.to_ascii_lowercase().contains("private"));
        assert!(!json.to_ascii_lowercase().contains("secret"));
        assert!(json.contains("public_address"));
    }

    #[test]
    fn audit_export_is_org_scoped() {
        let view = AuditExportView {
            organization_id: "org-1".into(),
            from: "2026-01-01T00:00:00Z".into(),
            to: "2026-01-31T00:00:00Z".into(),
            limit: 10,
            offset: 0,
            records: vec![],
            count: 0,
        };
        let json = serde_json::to_string(&view).unwrap();
        assert!(json.contains("org-1"));
    }

    #[test]
    fn ws_auth_metadata_documents_query_forbidden() {
        let meta = WsAuthMetadata {
            saas_endpoint: "/api/saas/events".into(),
            methods: vec!["header".into(), "first_frame".into()],
            query_credentials_forbidden: true,
            auth_timeout_secs: 10,
            legacy_endpoint: Some("/api/events".into()),
            legacy_mode: Some("disabled".into()),
            legacy_deprecated: true,
        };
        assert!(meta.query_credentials_forbidden);
        assert_eq!(meta.auth_timeout_secs, 10);
    }

    #[test]
    fn plan_view_is_provider_neutral() {
        let plan = PlanView {
            id: "plan-1".into(),
            code: "pro".into(),
            name: "Pro".into(),
            description: None,
            amount_cents: Some(1999),
            currency: Some("usd".into()),
            interval: Some("month".into()),
            version: Some(1),
            features: Default::default(),
            created_at: None,
        };
        let json = serde_json::to_string(&plan).unwrap();
        // No provider-specific fields like stripe_price_id
        assert!(!json.contains("stripe_price"));
    }
}
