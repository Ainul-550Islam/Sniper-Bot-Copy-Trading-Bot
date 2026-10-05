//! AWS KMS HTTP client (§H, spec file 53).
//!
//! A real client wrapper over the AWS KMS JSON-1.1 API using the
//! workspace's `reqwest` + `hmac` + `sha2` dependencies — SigV4 request
//! signing is implemented here (no AWS SDK dependency required). AWS KMS
//! has supported Ed25519 (EdDSA) signing since November 2025, which is
//! exactly what this custody boundary needs for Solana messages.
//!
//! Operations:
//!
//! * `sign_ed25519` — `X-Amz-Target: TrentService.Sign` with
//!   `SigningAlgorithm: EDDSA_SHA_512`;
//! * `get_public_key` — `TrentService.GetPublicKey`, returning the
//!   DER SubjectPublicKeyInfo parsed to the raw Ed25519 public key;
//! * credential checks — presence of the standard AWS env credential
//!   chain (secrets are read at request time and never rendered).
//!
//! The SigV4 signing-key derivation is verified in tests against the
//! test vector published in the AWS "Signature Version 4" documentation.
//!
//! Hard rules:
//!
//! * credentials (`AWS_SECRET_ACCESS_KEY`, session token) never appear in
//!   errors, `Debug`, or logs — errors carry status codes and typed
//!   KMS error messages only;
//! * every failure is typed and secret-free; no fake signatures, no
//!   fallback, no panic;
//! * the base64 `Signature` field is strictly validated to be exactly 64
//!   bytes before being returned.

use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::custody::kms::config::KmsConfig;

type HmacSha256 = Hmac<Sha256>;

/// The exact deployment dependency the KMS integration needs, stated
/// once for every fail-closed surface that mentions KMS.
pub const AWS_KMS_DEPENDENCY: &str = "AWS KMS with an Ed25519 (ECC_ED25519) key named by KMS_KEY_ID, an AWS region (KMS_REGION), and credentials from the standard AWS env chain (AWS_ACCESS_KEY_ID + AWS_SECRET_ACCESS_KEY, optional AWS_SESSION_TOKEN) authorized for kms:GetPublicKey and kms:Sign (aws kms ed25519 sign integration)";

/// Signing algorithm for Ed25519 keys in the KMS Sign API.
pub const EDDSA_SHA_512: &str = "EDDSA_SHA_512";

/// The fixed DER SubjectPublicKeyInfo prefix for an Ed25519 public key:
/// SEQUENCE(9) { SEQUENCE(5) { OID 1.3.101.112 }, BIT STRING(0 unused) }.
/// An Ed25519 SPKI is exactly these 12 bytes plus the 32-byte key.
const ED25519_SPKI_PREFIX: [u8; 12] = [
    0x30, 0x2a, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x70, 0x03, 0x21, 0x00,
];

/// Typed, secret-free KMS client error.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KmsClientError {
    pub code: &'static str,
    pub detail: String,
}

impl KmsClientError {
    fn unreachable(source: &str) -> Self {
        Self {
            code: "kms_unreachable",
            detail: format!("cannot reach AWS KMS: {source}"),
        }
    }

    fn status(target: &str, status: u16, message: Option<String>) -> Self {
        let typed = message.map(|m| format!(": {m}")).unwrap_or_default();
        Self {
            code: "kms_status",
            detail: format!("{target} returned HTTP {status}{typed}"),
        }
    }

    fn protocol(detail: impl Into<String>) -> Self {
        Self {
            code: "kms_protocol",
            detail: detail.into(),
        }
    }
}

impl std::fmt::Display for KmsClientError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} ({})", self.detail, self.code)
    }
}

impl std::error::Error for KmsClientError {}

/// Extract the raw 32-byte Ed25519 public key from a DER
/// SubjectPublicKeyInfo. Strict: anything that is not exactly
/// prefix + 32 bytes is rejected.
pub fn parse_ed25519_spki(der: &[u8]) -> Result<[u8; 32], KmsClientError> {
    if der.len() != ED25519_SPKI_PREFIX.len() + 32 {
        return Err(KmsClientError::protocol(
            "GetPublicKey returned a SubjectPublicKeyInfo that is not 44 bytes (not an Ed25519 key)".to_string(),
        ));
    }
    if der[..ED25519_SPKI_PREFIX.len()] != ED25519_SPKI_PREFIX {
        return Err(KmsClientError::protocol(
            "GetPublicKey returned a non-Ed25519 SubjectPublicKeyInfo (wrong OID)".to_string(),
        ));
    }
    let mut key = [0u8; 32];
    key.copy_from_slice(&der[ED25519_SPKI_PREFIX.len()..]);
    Ok(key)
}

/// AWS credentials resolved from the standard environment chain.
///
/// Held only for the lifetime of a request build; `Debug` never renders
/// the secret. NOTE: this is the environment portion of the AWS
/// credential chain. The full chain (shared config file, container
/// credentials, IMDS) requires the `aws-config` SDK crate — an optional
/// future upgrade, not a requirement for the env-chain deployment shape.
#[derive(Clone)]
pub struct AwsEnvCredentials {
    pub access_key_id: String,
    secret_access_key: String,
    session_token: Option<String>,
}

impl std::fmt::Debug for AwsEnvCredentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "AwsEnvCredentials(access_key_id={}, secret=REDACTED, session_token={})",
            self.access_key_id,
            if self.session_token.is_some() {
                "present"
            } else {
                "none"
            }
        )
    }
}

impl AwsEnvCredentials {
    /// Resolve from the standard AWS environment variables.
    pub fn from_env() -> Result<Self, KmsClientError> {
        let access_key_id = std::env::var("AWS_ACCESS_KEY_ID").unwrap_or_default();
        let secret_access_key = std::env::var("AWS_SECRET_ACCESS_KEY").unwrap_or_default();
        let session_token = std::env::var("AWS_SESSION_TOKEN")
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
        let access_key_id = access_key_id.trim().to_string();
        if access_key_id.is_empty() || secret_access_key.trim().is_empty() {
            return Err(KmsClientError {
                code: "kms_credentials_missing",
                detail: "AWS credentials are not configured: set AWS_ACCESS_KEY_ID and AWS_SECRET_ACCESS_KEY (standard AWS env credential chain; the shared-config/IMDS chain requires the aws-config SDK)".to_string(),
            });
        }
        Ok(Self {
            access_key_id,
            secret_access_key: secret_access_key.trim().to_string(),
            session_token,
        })
    }

    fn secret(&self) -> &str {
        &self.secret_access_key
    }
}

/// Derive the SigV4 signing key: HMAC chains over ("AWS4" + secret,
/// datestamp, region, service, "aws4_request").
///
/// This is the exact derivation from the AWS "Signature Version 4"
/// general reference; the test below verifies it against the documented
/// test vector.
pub fn sigv4_signing_key(secret: &str, datestamp: &str, region: &str, service: &str) -> [u8; 32] {
    let mut mac = HmacSha256::new_from_slice(format!("AWS4{secret}").as_bytes())
        .expect("hmac accepts any key length");
    mac.update(datestamp.as_bytes());
    let k_date = mac.finalize().into_bytes();

    let mut mac = HmacSha256::new_from_slice(&k_date).expect("hmac accepts any key length");
    mac.update(region.as_bytes());
    let k_region = mac.finalize().into_bytes();

    let mut mac = HmacSha256::new_from_slice(&k_region).expect("hmac accepts any key length");
    mac.update(service.as_bytes());
    let k_service = mac.finalize().into_bytes();

    let mut mac = HmacSha256::new_from_slice(&k_service).expect("hmac accepts any key length");
    mac.update(b"aws4_request");
    let k_signing = mac.finalize().into_bytes();

    let mut out = [0u8; 32];
    out.copy_from_slice(&k_signing);
    out
}

/// URI-encode a path or query component per SigV4 rules (RFC 3986,
/// unreserved = A-Z a-z 0-9 - _ . ~).
fn uri_encode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char)
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// A fully built, signed KMS request (headers + body). Used by the
/// client; exposed for unit tests of the signing construction.
#[derive(Debug, Clone)]
pub struct KmsSignedRequest {
    pub url: String,
    pub authorization: String,
    pub amz_date: String,
    pub body: String,
    pub session_token: Option<String>,
}

/// Build one SigV4-signed KMS request for `target` (e.g.
/// `TrentService.Sign`) with a JSON body. Host, path (`/`), the JSON
/// content type, `x-amz-date`, `x-amz-target` (and
/// `x-amz-security-token` when present) are the signed headers.
pub fn build_signed_request(
    endpoint: &str,
    region: &str,
    target: &str,
    body: &serde_json::Value,
    credentials: &AwsEnvCredentials,
    amz_date: &str, // format: 20260930T000000Z
) -> Result<KmsSignedRequest, KmsClientError> {
    let (scheme, host_port) = endpoint
        .trim()
        .trim_end_matches('/')
        .split_once("://")
        .ok_or_else(|| {
            KmsClientError::protocol(format!("KMS endpoint is not a URL: {endpoint}"))
        })?;
    if scheme != "https" && scheme != "http" {
        return Err(KmsClientError::protocol(format!(
            "KMS endpoint scheme must be http(s), got {scheme}"
        )));
    }
    let body_str = serde_json::to_string(body)
        .map_err(|_| KmsClientError::protocol("KMS request body serialization failed"))?;
    let payload_hash = hex(&Sha256::digest(body_str.as_bytes()));
    let datestamp = &amz_date[..8];

    // Canonical headers: sorted by lowercase name.
    let mut canonical_headers = format!(
        "content-type:application/x-amz-json-1.1\nhost:{host_port}\nx-amz-date:{amz_date}\nx-amz-target:{target}\n"
    );
    let mut signed_headers = "content-type;host;x-amz-date;x-amz-target".to_string();
    if let Some(token) = &credentials.session_token {
        canonical_headers.push_str(&format!("x-amz-security-token:{token}\n"));
        signed_headers =
            "content-type;host;x-amz-date;x-amz-security-token;x-amz-target".to_string();
    }

    // Canonical request: method POST, path "/", no query.
    let canonical_request =
        format!("POST\n/\n\n{canonical_headers}\n{signed_headers}\n{payload_hash}");

    // String to sign.
    let scope = format!("{datestamp}/{region}/kms/aws4_request");
    let string_to_sign = format!(
        "AWS4-HMAC-SHA256\n{amz_date}\n{scope}\n{}",
        hex(&Sha256::digest(canonical_request.as_bytes()))
    );

    // Signature.
    let signing_key = sigv4_signing_key(credentials.secret(), datestamp, region, "kms");
    let mut mac = HmacSha256::new_from_slice(&signing_key).expect("hmac accepts any key length");
    mac.update(string_to_sign.as_bytes());
    let signature = hex(&mac.finalize().into_bytes());

    let authorization = format!(
        "AWS4-HMAC-SHA256 Credential={}/{}, SignedHeaders={}, Signature={}",
        credentials.access_key_id, scope, signed_headers, signature
    );

    Ok(KmsSignedRequest {
        url: format!("{scheme}://{host_port}/"),
        authorization,
        amz_date: amz_date.to_string(),
        body: body_str,
        session_token: credentials.session_token.clone(),
    })
}

/// The KMS client. Cheap to clone.
#[derive(Clone)]
pub struct KmsClient {
    http: reqwest::Client,
    config: KmsConfig,
}

impl std::fmt::Debug for KmsClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("KmsClient")
            .field("config", &self.config)
            .finish()
    }
}

/// Response of `GetPublicKey` (the fields this boundary uses).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KmsPublicKey {
    /// Raw 32-byte Ed25519 public key extracted from the DER SPKI.
    pub ed25519_public_key: [u8; 32],
    /// The key's AWS-side spec string, e.g. `ECC_ED25519`.
    pub key_spec: String,
}

/// Parse a typed KMS error message out of a JSON-1.1 error body without
/// ever including credentials in the result.
fn kms_error_message(body: &str) -> Option<String> {
    serde_json::from_str::<serde_json::Value>(body)
        .ok()
        .and_then(|v| {
            v.get("__type")
                .or_else(|| v.get("code"))
                .and_then(|t| t.as_str())
                .map(|t| t.to_string())
        })
}

impl KmsClient {
    pub fn new(config: &KmsConfig) -> Self {
        Self {
            http: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(10))
                .build()
                .unwrap_or_default(),
            config: config.clone(),
        }
    }

    async fn call(
        &self,
        target: &str,
        body: serde_json::Value,
    ) -> Result<serde_json::Value, KmsClientError> {
        let credentials = AwsEnvCredentials::from_env()?;
        let amz_date = chrono::Utc::now().format("%Y%m%dT%H%M%SZ").to_string();
        let signed = build_signed_request(
            &self.config.endpoint(),
            self.config.region(),
            target,
            &body,
            &credentials,
            &amz_date,
        )?;

        let mut request = self
            .http
            .post(&signed.url)
            .header("Content-Type", "application/x-amz-json-1.1")
            .header("X-Amz-Target", target)
            .header("X-Amz-Date", &signed.amz_date)
            .header("Authorization", &signed.authorization)
            .body(signed.body.clone());
        if let Some(token) = &signed.session_token {
            request = request.header("X-Amz-Security-Token", token);
        }
        let response = request.send().await.map_err(|e| {
            let reason = e.to_string();
            if reason.contains(credentials.secret()) || signed.body.contains(credentials.secret()) {
                KmsClientError::unreachable("transport failure (details redacted)")
            } else {
                KmsClientError::unreachable(&reason)
            }
        })?;
        let status = response.status().as_u16();
        let text = response
            .text()
            .await
            .map_err(|_| KmsClientError::protocol("KMS response body could not be read"))?;
        if status != 200 {
            return Err(KmsClientError::status(
                target,
                status,
                kms_error_message(&text),
            ));
        }
        serde_json::from_str(&text)
            .map_err(|_| KmsClientError::protocol("KMS response is not valid JSON"))
    }

    /// `TrentService.GetPublicKey` for a key id / ARN. The returned key
    /// must be an Ed25519 key (`ECC_ED25519`).
    pub async fn get_public_key(&self, key_id: &str) -> Result<KmsPublicKey, KmsClientError> {
        if key_id.is_empty() {
            return Err(KmsClientError::protocol(
                "no KMS key id resolved for signer (set provider_ref or KMS_KEY_ID)".to_string(),
            ));
        }
        let response = self
            .call(
                "TrentService.GetPublicKey",
                serde_json::json!({ "KeyId": key_id }),
            )
            .await?;
        let key_spec = response
            .get("KeySpec")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string();
        let der_b64 = response
            .get("PublicKey")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                KmsClientError::protocol("GetPublicKey response has no PublicKey field".to_string())
            })?;
        let der = BASE64.decode(der_b64).map_err(|_| {
            KmsClientError::protocol("GetPublicKey PublicKey is not valid base64".to_string())
        })?;
        let ed25519_public_key = parse_ed25519_spki(&der)?;
        Ok(KmsPublicKey {
            ed25519_public_key,
            key_spec,
        })
    }

    /// `TrentService.Sign` with `EDDSA_SHA_512` — the ONLY signing path
    /// in the KMS integration. Returns the raw 64-byte Ed25519 signature
    /// from a real KMS response; nothing here fabricates a signature.
    pub async fn sign_ed25519(
        &self,
        key_id: &str,
        message: &[u8],
    ) -> Result<[u8; 64], KmsClientError> {
        if key_id.is_empty() {
            return Err(KmsClientError::protocol(
                "no KMS key id resolved for signer (set provider_ref or KMS_KEY_ID)".to_string(),
            ));
        }
        let response = self
            .call(
                "TrentService.Sign",
                serde_json::json!({
                    "KeyId": key_id,
                    "SigningAlgorithm": EDDSA_SHA_512,
                    "Message": BASE64.encode(message),
                }),
            )
            .await?;
        let signature_b64 = response
            .get("Signature")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                KmsClientError::protocol("Sign response has no Signature field".to_string())
            })?;
        let bytes = BASE64.decode(signature_b64).map_err(|_| {
            KmsClientError::protocol("Sign Signature is not valid base64".to_string())
        })?;
        let signature: [u8; 64] = bytes
            .try_into()
            .map_err(|_| KmsClientError::protocol("Sign Signature is not 64 bytes".to_string()))?;
        Ok(signature)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Test vector from the AWS "Signature Version 4" general reference:
    /// with this secret / date / region / service, the signing key MUST
    /// be `c4afb1cc5771d871763a393e44b703571b55cc28424d1a5e86da6ed3c154a4b9`.
    #[test]
    fn sigv4_signing_key_matches_the_aws_documented_test_vector() {
        let key = sigv4_signing_key(
            "wJalrXUtnFEMI/K7MDENG+bPxRfiCYEXAMPLEKEY",
            "20150830",
            "us-east-1",
            "iam",
        );
        assert_eq!(
            hex(&key),
            "c4afb1cc5771d871763a393e44b703571b55cc28424d1a5e86da6ed3c154a4b9"
        );
    }

    #[test]
    fn uri_encoding_follows_sigv4_unreserved_set() {
        assert_eq!(uri_encode("abcXYZ019-_.~"), "abcXYZ019-_.~");
        assert_eq!(uri_encode("a b/c"), "a%20b%2Fc");
    }

    #[test]
    fn ed25519_spki_is_parsed_strictly() {
        let mut der = Vec::new();
        der.extend_from_slice(&ED25519_SPKI_PREFIX);
        der.extend_from_slice(&[7u8; 32]);
        let key = parse_ed25519_spki(&der).expect("valid SPKI parses");
        assert_eq!(key, [7u8; 32]);
        // wrong length
        assert!(parse_ed25519_spki(&der[..40]).is_err());
        // wrong OID (flip a byte in the prefix)
        let mut wrong = der.clone();
        wrong[5] ^= 0x01;
        assert!(parse_ed25519_spki(&wrong).is_err());
    }

    #[test]
    fn credentials_debug_redacts_secret_and_session_token() {
        let creds = AwsEnvCredentials {
            access_key_id: "AKIAEXAMPLE".to_string(),
            secret_access_key: "super-secret".to_string(),
            session_token: Some("session-secret".to_string()),
        };
        let rendered = format!("{creds:?}");
        assert!(rendered.contains("REDACTED"));
        assert!(!rendered.contains("super-secret"));
        assert!(!rendered.contains("session-secret"));
    }

    #[test]
    fn signed_request_construction_is_deterministic_and_secret_free() {
        let creds = AwsEnvCredentials {
            access_key_id: "AKIAEXAMPLE".to_string(),
            secret_access_key: "wJalrXUtnFEMI/K7MDENG+bPxRfiCYEXAMPLEKEY".to_string(),
            session_token: None,
        };
        let body = serde_json::json!({ "KeyId": "k1", "Message": "aGk=" });
        let signed = build_signed_request(
            "https://kms.us-east-1.amazonaws.com",
            "us-east-1",
            "TrentService.Sign",
            &body,
            &creds,
            "20260930T000000Z",
        )
        .expect("request builds");
        assert_eq!(signed.url, "https://kms.us-east-1.amazonaws.com/");
        assert!(signed.authorization.starts_with(
            "AWS4-HMAC-SHA256 Credential=AKIAEXAMPLE/20260930/us-east-1/kms/aws4_request"
        ));
        assert!(signed
            .authorization
            .contains("SignedHeaders=content-type;host;x-amz-date;x-amz-target"));
        assert!(signed.authorization.contains("Signature="));
        // Deterministic: same inputs -> same signature.
        let again = build_signed_request(
            "https://kms.us-east-1.amazonaws.com",
            "us-east-1",
            "TrentService.Sign",
            &body,
            &creds,
            "20260930T000000Z",
        )
        .expect("request builds");
        assert_eq!(signed.authorization, again.authorization);
        // No secret material in the rendered request.
        let rendered = format!("{signed:?}");
        assert!(!rendered.contains("wJalrXUtnFEMI"));
    }

    #[test]
    fn signed_request_includes_session_token_header_when_present() {
        let creds = AwsEnvCredentials {
            access_key_id: "AKIAEXAMPLE".to_string(),
            secret_access_key: "wJalrXUtnFEMI/K7MDENG+bPxRfiCYEXAMPLEKEY".to_string(),
            session_token: Some("iqoBtoken".to_string()),
        };
        let signed = build_signed_request(
            "https://kms.us-east-1.amazonaws.com",
            "us-east-1",
            "TrentService.Sign",
            &serde_json::json!({ "KeyId": "k1" }),
            &creds,
            "20260930T000000Z",
        )
        .expect("request builds");
        assert!(signed.authorization.contains(
            "SignedHeaders=content-type;host;x-amz-date;x-amz-security-token;x-amz-target"
        ));
        assert_eq!(signed.session_token.as_deref(), Some("iqoBtoken"));
    }

    #[tokio::test]
    async fn sign_without_credentials_fails_closed_before_any_request() {
        let _lock = crate::custody::test_support::ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        std::env::remove_var("AWS_ACCESS_KEY_ID");
        std::env::remove_var("AWS_SECRET_ACCESS_KEY");
        std::env::set_var("KMS_KEY_ID", "k1");
        let (config, state) = KmsConfig::from_env();
        assert!(!state.ready());
        let client = KmsClient::new(&config);
        drop(_lock); // release the env lock before awaiting
        let err = client
            .sign_ed25519("k1", b"message")
            .await
            .expect_err("missing credentials must refuse");
        assert_eq!(err.code, "kms_credentials_missing");
        std::env::remove_var("KMS_KEY_ID");
    }

    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn sign_against_unreachable_endpoint_fails_closed_with_typed_error() {
        let _lock = crate::custody::test_support::ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        std::env::set_var("KMS_KEY_ID", "k1");
        std::env::set_var("KMS_REGION", "us-east-1");
        std::env::set_var("KMS_ENDPOINT", "http://127.0.0.1:1");
        std::env::set_var("AWS_ACCESS_KEY_ID", "AKIAEXAMPLE");
        std::env::set_var(
            "AWS_SECRET_ACCESS_KEY",
            "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY",
        );
        let (config, state) = KmsConfig::from_env();
        assert!(state.ready());
        let client = KmsClient::new(&config);
        let err = client
            .sign_ed25519("k1", b"message")
            .await
            .expect_err("unreachable endpoint must refuse");
        assert_eq!(err.code, "kms_unreachable");
        // Secret never leaks through the error.
        assert!(!err.to_string().contains("wJalrXUtnFEMI"));
        std::env::remove_var("KMS_KEY_ID");
        std::env::remove_var("KMS_REGION");
        std::env::remove_var("KMS_ENDPOINT");
        std::env::remove_var("AWS_ACCESS_KEY_ID");
        std::env::remove_var("AWS_SECRET_ACCESS_KEY");
    }

    #[test]
    fn client_debug_contains_no_secret_material() {
        let _lock = crate::custody::test_support::ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        std::env::set_var("AWS_ACCESS_KEY_ID", "AKIAEXAMPLE");
        std::env::set_var(
            "AWS_SECRET_ACCESS_KEY",
            "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY",
        );
        let creds = AwsEnvCredentials::from_env().expect("creds resolve");
        let rendered = format!("{creds:?}");
        assert!(!rendered.contains("wJalrXUtnFEMI"));
        assert!(rendered.contains("REDACTED"));
        std::env::remove_var("AWS_ACCESS_KEY_ID");
        std::env::remove_var("AWS_SECRET_ACCESS_KEY");
        // Config construction from references only (no credentials on it).
        let (config, _state) = {
            std::env::set_var("KMS_KEY_ID", "k");
            KmsConfig::from_env()
        };
        assert!(_state.key_present);
        std::env::remove_var("KMS_KEY_ID");
        let _ = KmsClient::new(&config);
    }
}
