//! AWS KMS custody configuration (§H, spec file 54).
//!
//! Reference-only configuration: key identifiers and region names, never
//! private material and never credentials. AWS credentials are resolved
//! from the **standard environment chain** (`AWS_ACCESS_KEY_ID`,
//! `AWS_SECRET_ACCESS_KEY`, `AWS_SESSION_TOKEN`) by the client at request
//! time — they are not stored on any long-lived structure, not rendered
//! by `Debug`, and never written to logs, audit rows, or errors.
//!
//! Environment contract (extends the single `LiveCustodyConfig` contract,
//! rule #35):
//!
//! * `KMS_KEY_ID` — KMS key id or ARN (an Ed25519/ECC_ED25519 key for
//!   Solana signing)
//! * `KMS_REGION` — AWS region (default `us-east-1`)
//! * `KMS_ENDPOINT` — optional explicit endpoint (tests / private endpoints)
//! * credentials: `AWS_ACCESS_KEY_ID` + `AWS_SECRET_ACCESS_KEY`
//!   (+ optional `AWS_SESSION_TOKEN`)

use bot_core::custody::SignerRecord;

/// Which references a KMS deployment needs, and which are present.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct KmsReferenceState {
    /// `KMS_KEY_ID` (or a per-signer `provider_ref`) names a key.
    pub key_present: bool,
    /// `AWS_ACCESS_KEY_ID` and `AWS_SECRET_ACCESS_KEY` are both set.
    pub credentials_present: bool,
}

impl KmsReferenceState {
    /// Everything required before the KMS client can make an
    /// authenticated call.
    pub fn ready(&self) -> bool {
        self.key_present && self.credentials_present
    }

    /// Machine-readable, secret-free gap list for health surfaces.
    pub fn missing(&self) -> Vec<&'static str> {
        let mut out = Vec::new();
        if !self.key_present {
            out.push("KMS_KEY_ID (or signer provider_ref)");
        }
        if !self.credentials_present {
            out.push(
                "AWS_ACCESS_KEY_ID + AWS_SECRET_ACCESS_KEY (standard AWS env credential chain)",
            );
        }
        out
    }
}

/// KMS configuration — references only.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KmsConfig {
    /// Default key id or full ARN; a signer's `provider_ref` overrides.
    key_id: Option<String>,
    /// AWS region, defaulted to `us-east-1`.
    region: String,
    /// Optional explicit endpoint override.
    endpoint: Option<String>,
}

impl KmsConfig {
    pub fn from_env() -> (Self, KmsReferenceState) {
        let key_raw = std::env::var("KMS_KEY_ID").unwrap_or_default();
        let region_raw = std::env::var("KMS_REGION").unwrap_or_default();
        let endpoint_raw = std::env::var("KMS_ENDPOINT").unwrap_or_default();

        let key_id = {
            let k = key_raw.trim();
            if k.is_empty() {
                None
            } else {
                Some(k.to_string())
            }
        };
        let region = {
            let r = region_raw.trim();
            if r.is_empty() {
                "us-east-1".to_string()
            } else {
                r.to_string()
            }
        };
        let endpoint = {
            let e = endpoint_raw.trim();
            if e.is_empty() {
                None
            } else {
                Some(e.to_string())
            }
        };

        let access_key = std::env::var("AWS_ACCESS_KEY_ID").unwrap_or_default();
        let secret_key = std::env::var("AWS_SECRET_ACCESS_KEY").unwrap_or_default();
        let credentials_present = !access_key.trim().is_empty() && !secret_key.trim().is_empty();

        let key_present = key_id.is_some();
        (
            Self {
                key_id,
                region,
                endpoint,
            },
            KmsReferenceState {
                key_present,
                credentials_present,
            },
        )
    }

    pub fn key_id(&self) -> Option<&str> {
        self.key_id.as_deref()
    }

    pub fn region(&self) -> &str {
        &self.region
    }

    /// The HTTPS endpoint KMS calls go to.
    pub fn endpoint(&self) -> String {
        self.endpoint
            .clone()
            .unwrap_or_else(|| format!("https://kms.{}.amazonaws.com", self.region))
    }

    /// Resolve the key id for one signer record: the signer's
    /// `provider_ref` (a key id or full ARN) takes priority over the
    /// deployment default.
    pub fn key_id_for(&self, signer: &SignerRecord) -> Option<String> {
        if let Some(reference) = signer.provider_ref.as_deref() {
            let name = Self::normalize_key_reference(reference);
            if !name.is_empty() {
                return Some(name);
            }
        }
        self.key_id.clone()
    }

    /// Normalize a `provider_ref` into a KMS key id / ARN. Rejects
    /// anything that could carry credentials or be interpolated unsafely
    /// into a request (query strings, fragments, whitespace).
    pub fn normalize_key_reference(reference: &str) -> String {
        let trimmed = reference.trim();
        if trimmed.is_empty()
            || trimmed.contains('?')
            || trimmed.contains('#')
            || trimmed.contains(char::is_whitespace)
        {
            return String::new();
        }
        // A full ARN must look like an ARN; anything else is treated as a
        // bare key id (UUID or alias/name form) and passed through — KMS
        // validates key ids server-side and returns a typed error.
        if let Some(rest) = trimmed.strip_prefix("arn:") {
            // arn:aws:kms:region:account:key/key-id  (or alias/...)
            let parts: Vec<&str> = rest.splitn(2, ':').collect();
            if parts.len() != 2 {
                return String::new();
            }
        }
        trimmed.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env_isolated(vars: &[(&str, Option<&str>)], f: impl FnOnce()) {
        let saved: Vec<(String, Option<String>)> = vars
            .iter()
            .map(|(k, _)| ((*k).to_string(), std::env::var(k).ok()))
            .collect();
        for (k, v) in vars {
            match v {
                Some(val) => std::env::set_var(k, val),
                None => std::env::remove_var(k),
            }
        }
        f();
        for (key, prev) in saved {
            match prev {
                Some(val) => std::env::set_var(&key, val),
                None => std::env::remove_var(&key),
            }
        }
    }

    #[test]
    fn from_env_reports_missing_references() {
        let _lock = crate::custody::test_support::ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        env_isolated(
            &[
                ("KMS_KEY_ID", None),
                ("KMS_REGION", None),
                ("KMS_ENDPOINT", None),
                ("AWS_ACCESS_KEY_ID", None),
                ("AWS_SECRET_ACCESS_KEY", None),
            ],
            || {
                let (config, state) = KmsConfig::from_env();
                assert!(!state.ready());
                assert_eq!(
                    state.missing(),
                    vec![
                        "KMS_KEY_ID (or signer provider_ref)",
                        "AWS_ACCESS_KEY_ID + AWS_SECRET_ACCESS_KEY (standard AWS env credential chain)",
                    ]
                );
                assert!(config.key_id().is_none());
                assert_eq!(config.region(), "us-east-1");
                assert_eq!(config.endpoint(), "https://kms.us-east-1.amazonaws.com");
            },
        );
    }

    #[test]
    fn from_env_reads_references_and_credentials_state() {
        let _lock = crate::custody::test_support::ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        env_isolated(
            &[
                ("KMS_KEY_ID", Some("1234abcd-12ab-34cd-56ef-1234567890ab")),
                ("KMS_REGION", Some("eu-west-1")),
                ("AWS_ACCESS_KEY_ID", Some("AKIAEXAMPLE")),
                ("AWS_SECRET_ACCESS_KEY", Some("secret")),
            ],
            || {
                let (config, state) = KmsConfig::from_env();
                assert!(state.ready());
                assert_eq!(config.region(), "eu-west-1");
                assert_eq!(config.endpoint(), "https://kms.eu-west-1.amazonaws.com");
            },
        );
    }

    #[test]
    fn custom_endpoint_is_honored() {
        let _lock = crate::custody::test_support::ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        env_isolated(
            &[
                ("KMS_KEY_ID", Some("k")),
                ("KMS_REGION", None),
                ("KMS_ENDPOINT", Some("https://kms.test.local:8443/")),
                ("AWS_ACCESS_KEY_ID", None),
                ("AWS_SECRET_ACCESS_KEY", None),
            ],
            || {
                let (config, state) = KmsConfig::from_env();
                assert_eq!(config.endpoint(), "https://kms.test.local:8443/");
                assert!(!state.ready(), "credentials still required");
            },
        );
    }

    #[test]
    fn key_references_normalize_safely() {
        assert_eq!(
            KmsConfig::normalize_key_reference(
                "arn:aws:kms:us-east-1:111122223333:key/1234abcd-12ab-34cd-56ef-1234567890ab"
            ),
            "arn:aws:kms:us-east-1:111122223333:key/1234abcd-12ab-34cd-56ef-1234567890ab"
        );
        assert_eq!(
            KmsConfig::normalize_key_reference("1234abcd-12ab-34cd-56ef-1234567890ab"),
            "1234abcd-12ab-34cd-56ef-1234567890ab"
        );
        assert_eq!(
            KmsConfig::normalize_key_reference("alias/sniper"),
            "alias/sniper"
        );
        // Credential-carrier / malformed forms are refused.
        assert_eq!(KmsConfig::normalize_key_reference(""), "");
        assert_eq!(KmsConfig::normalize_key_reference("k?x=1"), "");
        assert_eq!(KmsConfig::normalize_key_reference("k#f"), "");
        assert_eq!(KmsConfig::normalize_key_reference("a b"), "");
        assert_eq!(KmsConfig::normalize_key_reference("arn:aws"), "");
    }

    #[test]
    fn per_signer_reference_wins_over_default() {
        use bot_core::custody::{CustodyProfileId, ProviderType};
        use bot_core::tenant::OrganizationId;
        let mut record = SignerRecord::new(
            OrganizationId::new(),
            CustodyProfileId::new(),
            "kms-signer".to_string(),
            ProviderType::Kms,
            "".to_string(),
            chrono::Utc::now(),
        );
        let config = KmsConfig {
            key_id: Some("deployment-default".to_string()),
            region: "us-east-1".to_string(),
            endpoint: None,
        };
        assert_eq!(config.key_id_for(&record).unwrap(), "deployment-default");
        record.provider_ref = Some("arn:aws:kms:us-east-1:1:key/abc".to_string());
        assert_eq!(
            config.key_id_for(&record).unwrap(),
            "arn:aws:kms:us-east-1:1:key/abc"
        );
        record.provider_ref = Some("bad ref with spaces".to_string());
        assert_eq!(
            config.key_id_for(&record).unwrap(),
            "deployment-default",
            "malformed per-signer reference falls back to the default, never to an unsafe string"
        );
    }
}
