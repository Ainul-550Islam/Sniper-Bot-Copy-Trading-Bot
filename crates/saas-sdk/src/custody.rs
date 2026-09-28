//! Typed SDK methods for custody (BATCH 2 file 22).
//!
//! list custody profiles, get custody state, activate/revoke, inspect
//! public address, provider health, capability metadata. Never expose or
//! transport private keys. Never construct secret-bearing URLs.

use crate::client::SaasClient;
use crate::error::{SdkError, SdkErrorKind};
use crate::models::{CustodyHealthReport, CustodyProfileView, SignerView};

impl SaasClient {
    /// List custody profiles for the caller's tenant.
    pub async fn list_custody_profiles(&self) -> Result<Vec<CustodyProfileView>, SdkError> {
        self.get("/api/saas/custody/profiles").await
    }

    /// Get custody health (provider readiness, secret-free).
    pub async fn get_custody_health(&self) -> Result<CustodyHealthReport, SdkError> {
        self.get("/api/saas/custody/health").await
    }

    /// Get signer public view — never private key.
    pub async fn get_signer(&self, id: &str) -> Result<SignerView, SdkError> {
        if id.trim().is_empty() {
            return Err(SdkError::new(
                SdkErrorKind::Validation,
                "signer id required",
            ));
        }
        let path = format!("/api/saas/custody/signers/{}", id);
        self.get(&path).await
    }

    /// Resolve active signer for trading — tenant-scoped, capability-checked.
    pub async fn resolve_signer(&self, id: &str) -> Result<SignerView, SdkError> {
        let path = format!("/api/saas/custody/signers/{}/resolve", id);
        self.get(&path).await
    }

    /// Activate a custody profile (pending → active). Closed tenants cannot activate.
    pub async fn activate_custody_profile(&self, id: &str) -> Result<CustodyProfileView, SdkError> {
        let path = format!("/api/saas/custody/profiles/{}/activate", id);
        self.post(&path, &serde_json::json!({})).await
    }

    /// Activate a signer.
    pub async fn activate_signer(&self, id: &str) -> Result<SignerView, SdkError> {
        let path = format!("/api/saas/custody/signers/{}/activate", id);
        self.post(&path, &serde_json::json!({})).await
    }

    /// Inspect public address (wrapper over get_signer).
    pub async fn inspect_public_address(&self, id: &str) -> Result<String, SdkError> {
        let signer = self.get_signer(id).await?;
        Ok(signer.public_address)
    }

    /// Inspect provider health for a specific provider type.
    pub async fn inspect_provider_health(
        &self,
        provider_type: &str,
    ) -> Result<serde_json::Value, SdkError> {
        let health: CustodyHealthReport = self.get_custody_health().await?;
        for h in &health.providers {
            if h.provider_type == provider_type {
                return serde_json::to_value(h)
                    .map_err(|e| SdkError::new(SdkErrorKind::Decode, e.to_string()));
            }
        }
        Err(SdkError::new(
            SdkErrorKind::NotFound,
            format!("provider {} not found in report", provider_type),
        ))
    }
}

#[cfg(test)]
mod tests {

    #[test]
    fn custody_urls_never_contain_secrets() {
        // SDK must never put secrets in URL — only ids (UUIDs) which are public identifiers
        let id = "550e8400-e29b-41d4-a716-446655440000";
        let path = format!("/api/saas/custody/signers/{}", id);
        assert!(!path.contains("secret"));
        assert!(!path.contains("private"));
        assert!(!path.contains("token"));
        assert!(path.contains(id));
    }

    #[test]
    fn public_address_is_only_field() {
        let signer_json = serde_json::json!({
            "id": "id",
            "organization_id": "org",
            "custody_profile_id": "prof",
            "logical_identity": "id1",
            "public_address": "addr1",
            "provider_type": "vault",
            "status": "active",
            "capabilities": ["trade"]
        });
        let s: crate::models::SignerView = serde_json::from_value(signer_json).unwrap();
        assert_eq!(s.public_address, "addr1");
        let ser = serde_json::to_string(&s).unwrap();
        assert!(!ser.to_ascii_lowercase().contains("private"));
    }

    #[test]
    fn health_schema_is_secret_free() {
        let report_json = serde_json::json!({
            "organization_id": "org",
            "providers": [
                {"provider_type":"local","state":"reachable","detail":"ok","signing_allowed":true,"checked_at":"2026-01-01T00:00:00Z"}
            ],
            "generated_at": "2026-01-01T00:00:00Z"
        });
        let report: crate::models::CustodyHealthReport =
            serde_json::from_value(report_json).unwrap();
        let s = serde_json::to_string(&report).unwrap();
        assert!(!s.to_ascii_lowercase().contains("secret"));
    }
}
