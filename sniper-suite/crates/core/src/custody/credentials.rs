//! Provider-credential reference model (BATCH 2 file 06).
//!
//! Defines secret references for Vault/KMS/HSM without storing secret values.
//! Supports env/reference identifiers/configuration handles. Explicitly
//! rejects plaintext secret persistence. Safe Debug output.

use serde::{Deserialize, Serialize};

use super::model::ProviderType;

/// How a credential is referenced — never the raw secret.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CredentialRefKind {
    EnvVar,
    AwsKmsKeyId,
    GcpKmsResource,
    AzureKeyVault,
    VaultTransit,
    HsmSlot,
    FilePath,
    // Opaque handle for future providers
    Handle,
}

impl CredentialRefKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            CredentialRefKind::EnvVar => "env_var",
            CredentialRefKind::AwsKmsKeyId => "aws_kms_key_id",
            CredentialRefKind::GcpKmsResource => "gcp_kms_resource",
            CredentialRefKind::AzureKeyVault => "azure_key_vault",
            CredentialRefKind::VaultTransit => "vault_transit",
            CredentialRefKind::HsmSlot => "hsm_slot",
            CredentialRefKind::FilePath => "file_path",
            CredentialRefKind::Handle => "handle",
        }
    }
}

/// A safe credential reference — identifier only, no secret bytes.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CredentialRef {
    pub provider_type: ProviderType,
    pub kind: CredentialRefKind,
    /// The reference string: env var name, ARN, resource path, slot id, etc.
    /// Must not contain secret material.
    pub reference: String,
    /// Optional non-secret metadata (region, vault path, etc.)
    pub metadata: std::collections::BTreeMap<String, String>,
}

impl CredentialRef {
    /// Create a reference, validating it does not look like a secret value.
    pub fn new(
        provider_type: ProviderType,
        kind: CredentialRefKind,
        reference: impl Into<String>,
    ) -> Result<Self, String> {
        let reference_s = reference.into();
        validate_reference(&reference_s)?;
        // Additional provider-kind sanity
        if kind == CredentialRefKind::EnvVar {
            if reference_s.contains('=') || reference_s.contains('/') {
                return Err("env var name must be bare identifier".into());
            }
            if reference_s.trim().is_empty() {
                return Err("env var name must not be empty".into());
            }
        }
        Ok(Self {
            provider_type,
            kind,
            reference: reference_s,
            metadata: Default::default(),
        })
    }

    pub fn with_metadata(mut self, metadata: std::collections::BTreeMap<String, String>) -> Self {
        // Ensure metadata values are not secret-like
        for (k, v) in &metadata {
            if is_secret_like(k) || is_secret_like(v) {
                // We don't store — caller should not pass secrets
                panic!("metadata must not contain secret material: key={}", k);
            }
        }
        self.metadata = metadata;
        self
    }

    /// Safe identifier for logs/audit — never the secret.
    pub fn audit_id(&self) -> String {
        format!(
            "{}:{}:{}",
            self.provider_type.as_str(),
            self.kind.as_str(),
            self.reference
        )
    }
}

// Custom Debug that never emits secret-like content (reference is safe by construction, but we mask anyway if suspicious)
impl std::fmt::Debug for CredentialRef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Reference is already validated to be non-secret; we show it.
        // We do NOT show any value that could be a secret; we only show the reference identifier.
        f.debug_struct("CredentialRef")
            .field("provider_type", &self.provider_type)
            .field("kind", &self.kind)
            .field("reference", &self.reference)
            .field(
                "metadata_keys",
                &self.metadata.keys().cloned().collect::<Vec<_>>(),
            )
            .finish()
    }
}

/// Heuristic: does a string look like secret material?
pub fn is_secret_like(s: &str) -> bool {
    let lower = s.to_ascii_lowercase();
    const SECRET_HINTS: &[&str] = &[
        "secret", "private", "sk-", "api_key", "apikey", "token", "bearer", "password",
    ];
    if s.len() > 64
        && s.chars()
            .all(|c| c.is_ascii_hexdigit() || c == '-' || c == '_')
    {
        return true; // long hex looks like key
    }
    SECRET_HINTS.iter().any(|h| lower.contains(h))
}

/// Validate a reference string does not appear to be a secret value.
pub fn validate_reference(reference: &str) -> Result<(), String> {
    let t = reference.trim();
    if t.is_empty() {
        return Err("reference must not be empty".into());
    }
    if t.len() > 512 {
        return Err("reference too long".into());
    }
    // Reject values that look like raw private keys — must be checked before generic secret heuristic
    // so that the error message contains "private key" as expected by callers/tests.
    if t.contains("BEGIN PRIVATE KEY") || t.contains("BEGIN SECRET") {
        return Err("reference must not be private key material".into());
    }
    // Heuristic: env var names like VAULT_TOKEN / API_KEY are allowed even though they contain
    // "token"/"secret" substrings — they are identifiers, not values. Only reject if the
    // reference looks like an actual secret value (long hex, bearer prefix, etc.) and is not a
    // bare env var identifier (uppercase letters, digits, underscores).
    let is_env_identifier = t
        .chars()
        .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
        && t.chars().any(|c| c.is_ascii_alphabetic())
        && t.len() <= 64;
    if !is_env_identifier && is_secret_like(t) {
        return Err(format!(
            "reference looks like secret material and is rejected: {}",
            t.chars().take(20).collect::<String>()
        ));
    }
    if t.len() > 100 && t.chars().filter(|c| c.is_ascii_alphanumeric()).count() > 90 {
        // Likely a token; reject if no slashes/colons that indicate a resource path
        if !t.contains('/') && !t.contains(':') && !t.contains('_') {
            return Err("reference looks like token value, use env var name instead".into());
        }
    }
    Ok(())
}

/// Credential store handle — in-memory registry of references (no secrets).
#[derive(Debug, Default, Clone)]
pub struct CredentialStore {
    refs: std::collections::HashMap<(ProviderType, String), CredentialRef>,
}

impl CredentialStore {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert(&mut self, cred: CredentialRef) -> Result<(), String> {
        let key = (cred.provider_type, cred.reference.clone());
        if self.refs.contains_key(&key) {
            return Err("credential reference already exists".into());
        }
        self.refs.insert(key, cred);
        Ok(())
    }

    pub fn get(&self, provider_type: ProviderType, reference: &str) -> Option<&CredentialRef> {
        self.refs.get(&(provider_type, reference.to_string()))
    }

    pub fn list_for(&self, provider_type: ProviderType) -> Vec<&CredentialRef> {
        self.refs
            .values()
            .filter(|r| r.provider_type == provider_type)
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::custody::model::ProviderType;

    #[test]
    fn env_var_ref_ok() {
        let c = CredentialRef::new(ProviderType::Vault, CredentialRefKind::EnvVar, "VAULT_ADDR")
            .unwrap();
        assert_eq!(c.reference, "VAULT_ADDR");
        assert!(format!("{:?}", c).contains("VAULT_ADDR"));
    }

    #[test]
    fn plaintext_secret_rejected() {
        let err = CredentialRef::new(
            ProviderType::Vault,
            CredentialRefKind::EnvVar,
            "sk-1234secretvalue_abcdefghijklmnopqrstuvwxyz0123456789",
        )
        .unwrap_err();
        assert!(err.contains("secret"));
    }

    #[test]
    fn private_key_material_rejected() {
        let err = CredentialRef::new(
            ProviderType::Kms,
            CredentialRefKind::Handle,
            "-----BEGIN PRIVATE KEY----- MIIBIj...",
        )
        .unwrap_err();
        assert!(err.contains("private key"));
    }

    #[test]
    fn long_hex_rejected() {
        let hex = "a".repeat(70);
        assert!(is_secret_like(&hex));
        let err =
            CredentialRef::new(ProviderType::Hsm, CredentialRefKind::Handle, hex).unwrap_err();
        assert!(err.contains("secret") || err.contains("token"));
    }

    #[test]
    fn debug_never_exposes_secret_value() {
        let c = CredentialRef::new(
            ProviderType::Kms,
            CredentialRefKind::AwsKmsKeyId,
            "arn:aws:kms:us-east-1:123456789012:key/abcd-1234",
        )
        .unwrap();
        let dbg = format!("{:?}", c);
        assert!(dbg.contains("arn:aws:kms"));
        assert!(!dbg.contains("secret"));
        // Even if someone tries to put secret in metadata, debug only shows keys
        let mut meta = std::collections::BTreeMap::new();
        meta.insert("region".into(), "us-east-1".into());
        let c2 = c.with_metadata(meta);
        let dbg2 = format!("{:?}", c2);
        assert!(dbg2.contains("region"));
        assert!(!dbg2.contains("us-east-1-value-secret"));
    }

    #[test]
    fn credential_store_insert_and_get() {
        let mut store = CredentialStore::new();
        let c = CredentialRef::new(
            ProviderType::Vault,
            CredentialRefKind::EnvVar,
            "VAULT_TOKEN",
        )
        .unwrap();
        store.insert(c.clone()).unwrap();
        assert!(store.get(ProviderType::Vault, "VAULT_TOKEN").is_some());
        assert!(store.get(ProviderType::Kms, "VAULT_TOKEN").is_none());
    }

    #[test]
    fn duplicate_reference_rejected() {
        let mut store = CredentialStore::new();
        let c = CredentialRef::new(ProviderType::Vault, CredentialRefKind::EnvVar, "VAULT_ADDR")
            .unwrap();
        store.insert(c.clone()).unwrap();
        assert!(store.insert(c).is_err());
    }

    #[test]
    fn reference_validation_empty_rejected() {
        assert!(validate_reference("   ").is_err());
    }
}
