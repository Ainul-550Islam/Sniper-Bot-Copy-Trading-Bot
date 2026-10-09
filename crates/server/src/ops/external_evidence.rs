//! External verification evidence record (Batch 7).
//! Every external validation result must have validation_id, gap_id, provider, environment,
//! timestamp, command/mode, status, evidence_hash, redacted metadata.
//! Do not persist secrets or credentials.
//!
//! Canonicalization rule (Batch 10, deterministic):
//! `evidence_hash` = SHA256 of the compact JSON of
//! `{command, endpoint_ref, environment, gap_id, mode, provider, redacted_metadata, status, validation_id}`
//! with object keys sorted (serde_json `Map` is a `BTreeMap`) and compact separators.
//! The `timestamp` field is deliberately **excluded** from the hash: it is informational, so
//! the same input tree + same command + same canonical payload always produce the same hash.
//! The exact rule is locked by `canonical_payload_fixture_is_stable` here and by the shell
//! self-test in `scripts/run-external-validation.sh` (same fixture, same expected digest).

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::provider_contract::ProviderStatus;

fn is_secret_like(s: &str) -> bool {
    let lower = s.to_lowercase();
    lower.contains("sk_live")
        || lower.contains("sk_test")
        || lower.contains("whsec_")
        || lower.contains("vault_token")
        || lower.contains("private_key")
        || lower.contains("begin private key")
        || lower.contains("postgres://")
        || lower.contains("redis://")
        || lower.contains("bearer ")
        || lower.contains("seed phrase")
        || lower.contains("mnemonic")
        || s.contains("DATABASE_URL")
        || s.contains("POSTGRES_URL")
        || s.contains("REDIS_URL")
        || lower.contains("api_key") && s.contains("=")
}

fn redact_metadata(metadata: &serde_json::Value) -> serde_json::Value {
    match metadata {
        serde_json::Value::String(s) if is_secret_like(s) => {
            serde_json::Value::String("<redacted>".into())
        }
        serde_json::Value::Object(map) => {
            let mut out = serde_json::Map::new();
            for (k, v) in map {
                let kl = k.to_lowercase();
                if kl.contains("token")
                    || kl.contains("secret")
                    || kl.contains("key")
                    || kl.contains("url")
                        && v.is_string()
                        && v.as_str().unwrap_or("").contains("://")
                {
                    // Redact URL values that might contain credentials, but keep host if possible
                    if let Some(s) = v.as_str() {
                        if s.contains("://") && s.contains('@') {
                            out.insert(
                                k.clone(),
                                serde_json::Value::String("<redacted url>".into()),
                            );
                        } else if kl.contains("secret")
                            || kl.contains("token")
                            || kl.contains("private")
                        {
                            out.insert(k.clone(), serde_json::Value::String("<redacted>".into()));
                        } else {
                            out.insert(k.clone(), redact_metadata(v));
                        }
                    } else {
                        out.insert(k.clone(), serde_json::Value::String("<redacted>".into()));
                    }
                } else {
                    out.insert(k.clone(), redact_metadata(v));
                }
            }
            serde_json::Value::Object(out)
        }
        serde_json::Value::Array(arr) => {
            serde_json::Value::Array(arr.iter().map(redact_metadata).collect())
        }
        _ => metadata.clone(),
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExternalEvidence {
    pub validation_id: String,
    /// Canonical gap id from `ops::final_gap_ledger` (e.g. `GAP-001`), or `n/a`
    /// for a read-only validation that is not one of the six buyer gaps.
    pub gap_id: String,
    pub provider: String,
    pub environment: String,
    pub timestamp: String,
    pub command: String,
    pub mode: String,
    pub status: ProviderStatus,
    pub evidence_hash: String,
    pub redacted_metadata: serde_json::Value,
    pub endpoint_ref: String,
}

impl ExternalEvidence {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        validation_id: impl Into<String>,
        gap_id: impl Into<String>,
        provider: impl Into<String>,
        environment: impl Into<String>,
        command: impl Into<String>,
        mode: impl Into<String>,
        status: ProviderStatus,
        redacted_metadata: serde_json::Value,
        endpoint_ref: impl Into<String>,
    ) -> Self {
        let validation_id = validation_id.into();
        let gap_id = gap_id.into();
        let provider = provider.into();
        let environment = environment.into();
        let command = redact_command(&command.into());
        let mode = mode.into();
        let redacted_metadata = redact_metadata(&redacted_metadata);
        let endpoint_ref = endpoint_ref.into();
        let timestamp = chrono::Utc::now().to_rfc3339();
        let evidence_hash = Self::compute_hash(
            &validation_id,
            &gap_id,
            &provider,
            &environment,
            &command,
            &mode,
            status,
            &redacted_metadata,
            &endpoint_ref,
        );
        Self {
            validation_id,
            gap_id,
            provider,
            environment,
            timestamp,
            command,
            mode,
            status,
            evidence_hash,
            redacted_metadata,
            endpoint_ref,
        }
    }

    /// Deterministic canonical payload — the exact bytes that are hashed.
    /// Sorted keys, compact separators, timestamp excluded (see module docs).
    #[allow(clippy::too_many_arguments)]
    pub fn canonical_payload(
        validation_id: &str,
        gap_id: &str,
        provider: &str,
        environment: &str,
        command: &str,
        mode: &str,
        status: ProviderStatus,
        metadata: &serde_json::Value,
        endpoint_ref: &str,
    ) -> String {
        // `serde_json::Map` is a BTreeMap: iteration (and therefore serialization)
        // is in lexicographic key order, recursively.
        let mut obj = serde_json::Map::new();
        obj.insert(
            "command".to_string(),
            serde_json::Value::String(command.to_string()),
        );
        obj.insert(
            "endpoint_ref".to_string(),
            serde_json::Value::String(endpoint_ref.to_string()),
        );
        obj.insert(
            "environment".to_string(),
            serde_json::Value::String(environment.to_string()),
        );
        obj.insert(
            "gap_id".to_string(),
            serde_json::Value::String(gap_id.to_string()),
        );
        obj.insert(
            "mode".to_string(),
            serde_json::Value::String(mode.to_string()),
        );
        obj.insert(
            "provider".to_string(),
            serde_json::Value::String(provider.to_string()),
        );
        obj.insert("redacted_metadata".to_string(), metadata.clone());
        obj.insert(
            "status".to_string(),
            serde_json::Value::String(status.as_str().to_string()),
        );
        obj.insert(
            "validation_id".to_string(),
            serde_json::Value::String(validation_id.to_string()),
        );
        serde_json::Value::Object(obj).to_string()
    }

    #[allow(clippy::too_many_arguments)]
    pub fn compute_hash(
        validation_id: &str,
        gap_id: &str,
        provider: &str,
        environment: &str,
        command: &str,
        mode: &str,
        status: ProviderStatus,
        metadata: &serde_json::Value,
        endpoint_ref: &str,
    ) -> String {
        let payload = Self::canonical_payload(
            validation_id,
            gap_id,
            provider,
            environment,
            command,
            mode,
            status,
            metadata,
            endpoint_ref,
        );
        let mut hasher = Sha256::new();
        hasher.update(payload.as_bytes());
        hex::encode(hasher.finalize())
    }

    pub fn verify_hash(&self) -> bool {
        let expected = Self::compute_hash(
            &self.validation_id,
            &self.gap_id,
            &self.provider,
            &self.environment,
            &self.command,
            &self.mode,
            self.status,
            &self.redacted_metadata,
            &self.endpoint_ref,
        );
        expected == self.evidence_hash
    }

    pub fn to_safe_json(&self) -> serde_json::Value {
        serde_json::json!({
            "validation_id": self.validation_id,
            "gap_id": self.gap_id,
            "provider": self.provider,
            "environment": self.environment,
            "timestamp": self.timestamp,
            "command": self.command,
            "mode": self.mode,
            "status": self.status.as_str(),
            "evidence_hash": self.evidence_hash,
            "redacted_metadata": self.redacted_metadata,
            "endpoint_ref": self.endpoint_ref,
        })
    }

    pub fn save_to(&self, dir: &str) -> Result<String, String> {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        let path = format!("{}/{}_{}.json", dir, self.validation_id, self.provider);
        let json = serde_json::to_string_pretty(&self.to_safe_json()).map_err(|e| e.to_string())?;
        // Ensure no secrets in serialized JSON
        if is_secret_like(&json) && (json.contains("sk_live") || json.contains("sk_test")) {
            return Err("secret leak in evidence".into());
        }
        std::fs::write(&path, json).map_err(|e| e.to_string())?;
        Ok(path)
    }
}

fn redact_command(cmd: &str) -> String {
    const SECRET_ENV: [&str; 20] = [
        "DATABASE_URL",
        "POSTGRES_URL",
        "REDIS_URL",
        "STRIPE_API_KEY",
        "STRIPE_WEBHOOK_SECRET",
        "PADDLE_API_KEY",
        "PADDLE_WEBHOOK_SECRET",
        "VAULT_TOKEN",
        "VAULT_NAMESPACE",
        "KMS_KEY_ID",
        "KMS_SECRET",
        "HSM_SLOT",
        "HSM_PIN",
        "RPC_URL",
        "GEYSER_URL",
        "DEPLOYMENT_BASE_URL",
        "SOLANA_KEYPAIR",
        "WALLET_PRIVATE_KEY",
        "SEED_PHRASE",
        "MNEMONIC",
    ];
    // These two can carry space-separated material; every following token that is
    // not itself `NAME=...` is treated as part of the secret and dropped.
    const MULTIWORD_ENV: [&str; 2] = ["SEED_PHRASE", "MNEMONIC"];

    let mut parts: Vec<String> = Vec::new();
    let mut drop_secret_continuation = false;
    let mut after_bearer = false;
    for tok in cmd.split_whitespace() {
        if after_bearer {
            parts.push("<redacted>".to_string());
            after_bearer = false;
            continue;
        }
        if tok.eq_ignore_ascii_case("bearer") {
            parts.push(tok.to_string());
            after_bearer = true;
            continue;
        }
        let is_env_assignment = tok.contains('=');
        if drop_secret_continuation && !is_env_assignment {
            continue; // part of a multi-word secret (seed phrase / mnemonic)
        }
        if is_env_assignment {
            drop_secret_continuation = false;
            let name = tok.split('=').next().unwrap_or("");
            if SECRET_ENV.contains(&name) {
                parts.push(format!("{name}=<redacted>"));
                if MULTIWORD_ENV.contains(&name) {
                    drop_secret_continuation = true;
                }
                continue;
            }
        }
        parts.push(tok.to_string());
    }
    let mut out = parts.join(" ");
    if out.contains("BEGIN PRIVATE KEY") {
        out = out.replace("BEGIN PRIVATE KEY", "<redacted>");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn evidence_hash_is_deterministic_and_verifiable() {
        let e = ExternalEvidence::new(
            "test-001",
            "GAP-001",
            "stripe",
            "test",
            "LIVE_BILLING=1 cargo test --test live_billing_contract",
            "billing",
            ProviderStatus::NotRun,
            serde_json::json!({"endpoint": "https://api.stripe.com", "latency_ms": 123}),
            "https://api.stripe.com (redacted)",
        );
        assert!(e.verify_hash());
        assert!(!e.evidence_hash.is_empty());
    }

    #[test]
    fn tampered_evidence_fails_verify() {
        let mut e = ExternalEvidence::new(
            "test-002",
            "GAP-002",
            "vault",
            "test",
            "LIVE_CUSTODY=1 cargo test",
            "custody",
            ProviderStatus::Pass,
            serde_json::json!({"public_key": "abc"}),
            "vault ref",
        );
        e.redacted_metadata = serde_json::json!({"public_key": "tampered"});
        assert!(!e.verify_hash());
    }

    #[test]
    fn no_secrets_in_evidence() {
        let e = ExternalEvidence::new(
            "test-003",
            "n/a",
            "postgres",
            "test",
            "DATABASE_URL=postgres://secret cargo test",
            "database",
            ProviderStatus::NotRun,
            serde_json::json!({"secret": "sk_live_abc", "url": "postgres://user:pass@host/db"}),
            "postgres ref",
        );
        let json = e.to_safe_json().to_string();
        assert!(!json.contains("sk_live_abc"));
        // key name "secret" remains but value must be redacted
        assert_eq!(e.redacted_metadata["secret"], "<redacted>");
        assert!(!json.contains("postgres://user:pass"));
        assert!(e.command.contains("<redacted>"));
    }

    #[test]
    fn redacted_metadata_hides_tokens() {
        let e = ExternalEvidence::new(
            "test-004",
            "GAP-005",
            "solana",
            "test",
            "RPC_URL=https://example.com cargo test",
            "solana",
            ProviderStatus::Pass,
            serde_json::json!({"token": "secret123", "slot": 123}),
            "rpc ref",
        );
        assert_eq!(e.redacted_metadata["token"], "<redacted>");
        assert_eq!(e.redacted_metadata["slot"], 123);
    }

    /// Batch 10: the timestamp is informational and MUST NOT change the hash —
    /// same input tree + same command + same canonical payload = same hash.
    #[test]
    fn hash_is_independent_of_timestamp() {
        let a = ExternalEvidence::new(
            "test-005",
            "GAP-001",
            "stripe",
            "test",
            "cmd",
            "billing",
            ProviderStatus::NotRun,
            serde_json::json!({"ok": true}),
            "ref",
        );
        let mut b = a.clone();
        b.timestamp = "2099-01-01T00:00:00Z".to_string();
        assert_eq!(a.evidence_hash, b.evidence_hash);
        assert!(
            b.verify_hash(),
            "timestamp change must not break verification"
        );
    }

    /// Batch 10: every integrity-relevant field participates in the hash —
    /// tampering with any of them must be detected.
    #[test]
    fn tampering_with_any_canonical_field_fails_verify() {
        let base = ExternalEvidence::new(
            "test-006",
            "GAP-003",
            "deployment",
            "test",
            "cmd",
            "deployment",
            ProviderStatus::NotRun,
            serde_json::json!({"health": null}),
            "deployment ref",
        );
        assert!(base.verify_hash());

        let mut e = base.clone();
        e.mode = "all-safe-live".into();
        assert!(!e.verify_hash(), "tampered mode must fail");

        let mut e = base.clone();
        e.endpoint_ref = "other ref".into();
        assert!(!e.verify_hash(), "tampered endpoint_ref must fail");

        let mut e = base.clone();
        e.gap_id = "GAP-999".into();
        assert!(!e.verify_hash(), "tampered gap_id must fail");

        let mut e = base.clone();
        e.status = ProviderStatus::Pass;
        assert!(!e.verify_hash(), "tampered status must fail");

        let mut e = base;
        e.command = "different cmd".into();
        assert!(!e.verify_hash(), "tampered command must fail");
    }

    /// Batch 10: locks the canonicalization rule shared with the shell pipeline
    /// (`scripts/run-external-validation.sh` self-test uses the same fixture and digest).
    /// Changing the rule requires changing this fixture in both places, on purpose.
    #[test]
    fn canonical_payload_fixture_is_stable() {
        let metadata = serde_json::json!({"signature_verified": null, "checkout_created": null});
        let payload = ExternalEvidence::canonical_payload(
            "billing",
            "GAP-001",
            "stripe",
            "test",
            "LIVE_BILLING=1 cargo test --test live_billing_contract",
            "all-safe",
            ProviderStatus::NotRun,
            &metadata,
            "stripe ref",
        );
        assert_eq!(
            payload,
            r#"{"command":"LIVE_BILLING=1 cargo test --test live_billing_contract","endpoint_ref":"stripe ref","environment":"test","gap_id":"GAP-001","mode":"all-safe","provider":"stripe","redacted_metadata":{"checkout_created":null,"signature_verified":null},"status":"NOT_RUN","validation_id":"billing"}"#
        );
        assert_eq!(
            ExternalEvidence::compute_hash(
                "billing",
                "GAP-001",
                "stripe",
                "test",
                "LIVE_BILLING=1 cargo test --test live_billing_contract",
                "all-safe",
                ProviderStatus::NotRun,
                &metadata,
                "stripe ref",
            ),
            "1e3bf39a6d142631318d8b4d2f9da5072a2747935ced440194a20074e4e9c1b3"
        );
    }

    #[test]
    fn command_redaction_covers_env_and_authorization() {
        let cmd = "POSTGRES_URL=postgres://u:p@h/db STRIPE_WEBHOOK_SECRET=whsec_x Authorization: Bearer sk_test_y cargo test";
        let redacted = redact_command(cmd);
        assert!(!redacted.contains("postgres://u:p"));
        assert!(!redacted.contains("whsec_x"));
        assert!(!redacted.contains("sk_test_y"));
        assert!(redacted.contains("POSTGRES_URL=<redacted>"));
        assert!(redacted.contains("STRIPE_WEBHOOK_SECRET=<redacted>"));
        assert!(redacted.contains("Bearer <redacted>"));
    }

    #[test]
    fn seed_and_wallet_secrets_are_redacted() {
        let cmd = "SEED_PHRASE=alpha beta gamma WALLET_PRIVATE_KEY=deadbeef cargo test";
        let redacted = redact_command(cmd);
        assert!(!redacted.contains("alpha"));
        assert!(!redacted.contains("beta"));
        assert!(!redacted.contains("gamma"));
        assert!(!redacted.contains("deadbeef"));
        assert!(redacted.contains("SEED_PHRASE=<redacted>"));
        assert!(redacted.contains("WALLET_PRIVATE_KEY=<redacted>"));
    }
}
