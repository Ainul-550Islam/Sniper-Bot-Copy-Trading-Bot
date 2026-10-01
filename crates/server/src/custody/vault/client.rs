//! Vault transit-engine HTTP client (§G, spec file 48).
//!
//! A real client wrapper over Vault's HTTP API using the workspace's
//! `reqwest` dependency — no SDK required, the transit engine is plain
//! REST + JSON. It performs four operations:
//!
//! * `sys_health` — `GET /v1/sys/health` (Vault's own liveness semantics,
//!   including sealed/standby status codes);
//! * `token_lookup_self` — `GET /v1/auth/token/lookup-self` (permission
//!   probe for the configured token);
//! * `transit_key` — `GET /v1/{mount}/keys/{name}` (existence + key type
//!   + public key);
//! * `sign_ed25519` — `POST /v1/{mount}/sign/{name}` (the signing
//!   operation itself, for ed25519 transit keys).
//!
//! Hard rules enforced here:
//!
//! * the `X-Vault-Token` header is attached to every request and NEVER
//!   appears in any error message, debug output, or log line — errors are
//!   built from status codes and secret-free response fields only;
//! * every failure is a typed, secret-free error — no panic, no fake
//!   signature, no fallback;
//! * the signature string Vault returns (`vault:v1:<base64>`) is parsed
//!   and strictly validated to be a 64-byte Ed25519 signature before it
//!   is returned.

use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;
use serde::{Deserialize, Serialize};

use crate::custody::vault::config::{VaultConfig, VaultToken};

/// The exact deployment dependency the Vault integration needs, stated
/// once for every fail-closed surface that mentions Vault.
pub const VAULT_TRANSIT_DEPENDENCY: &str = "HashiCorp Vault transit engine with an ed25519 key, reachable at VAULT_ADDR, with a VAULT_TOKEN authorized for update on transit/keys/* and transit/sign/* (vault transit sign integration)";

/// Typed Vault service state from `GET /v1/sys/health`.
///
/// Vault signals degraded states through deliberate non-200 status codes;
/// they are mapped here instead of being treated as generic transport
/// errors.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VaultServiceState {
    /// 200 — active, unsealed, ready.
    Active,
    /// 429 — standby node (cannot serve requests).
    Standby,
    /// 472 — disaster-recovery mode.
    DisasterMode,
    /// 473 — performance standby.
    PerformanceStandby,
    /// 501 — not initialized.
    NotInitialized,
    /// 503 — sealed.
    Sealed,
    /// HTTP reachable but returned an unexpected status.
    UnexpectedStatus(u16),
    /// Connection refused / timeout / TLS failure — with a secret-free
    /// reason string.
    Unreachable(String),
}

impl VaultServiceState {
    /// True only for the one state in which Vault can serve requests.
    pub fn can_serve(&self) -> bool {
        matches!(self, VaultServiceState::Active)
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            VaultServiceState::Active => "active",
            VaultServiceState::Standby => "standby",
            VaultServiceState::DisasterMode => "disaster_mode",
            VaultServiceState::PerformanceStandby => "performance_standby",
            VaultServiceState::NotInitialized => "not_initialized",
            VaultServiceState::Sealed => "sealed",
            VaultServiceState::UnexpectedStatus(_) => "unexpected_status",
            VaultServiceState::Unreachable(_) => "unreachable",
        }
    }
}

/// Non-secret metadata about one transit key, from
/// `GET /v1/{mount}/keys/{name}`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TransitKeyInfo {
    /// Vault key type — must be `ed25519` for this custody boundary.
    pub key_type: String,
    /// Latest key version (versions are ints in Vault's JSON).
    pub latest_version: u64,
    /// Base64-encoded public key of the latest version, if the key type
    /// exposes one (ed25519 does).
    pub public_key_b64: Option<String>,
    /// True when the key is scheduled for deletion (soft-deleted).
    pub deletion_time_present: bool,
}

impl TransitKeyInfo {
    /// This boundary signs Solana Ed25519 messages; only ed25519 keys are
    /// acceptable.
    pub fn is_ed25519(&self) -> bool {
        self.key_type.eq_ignore_ascii_case("ed25519")
    }
}

/// Secret-free Vault client error. The token never appears in `detail`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VaultClientError {
    /// Machine-readable, stable code for audit surfaces.
    pub code: &'static str,
    /// Secret-free human-readable detail.
    pub detail: String,
}

impl VaultClientError {
    fn unreachable(source: &str) -> Self {
        Self {
            code: "vault_unreachable",
            detail: format!("cannot reach Vault: {source}"),
        }
    }

    fn status(path: &str, status: u16) -> Self {
        Self {
            code: "vault_status",
            detail: format!("{path} returned HTTP {status}"),
        }
    }

    fn protocol(detail: impl Into<String>) -> Self {
        Self {
            code: "vault_protocol",
            detail: detail.into(),
        }
    }
}

impl std::fmt::Display for VaultClientError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} ({})", self.detail, self.code)
    }
}

impl std::error::Error for VaultClientError {}

/// Parse and strictly validate a Vault transit signature string.
///
/// Vault returns `vault:v<N>:<base64(signature)>`. The base64 part must
/// decode to exactly 64 bytes (an Ed25519 signature). Any other shape is
/// a protocol error, never a partial result.
pub fn parse_vault_signature(signature_field: &str) -> Result<[u8; 64], VaultClientError> {
    let mut parts = signature_field.split(':');
    let prefix = parts.next().unwrap_or_default();
    let version = parts.next().unwrap_or_default();
    let payload = parts.next().unwrap_or_default();
    if parts.next().is_some() || prefix != "vault" || version.is_empty() {
        return Err(VaultClientError::protocol(
            "malformed vault signature envelope: expected 'vault:vN:<base64>'",
        ));
    }
    let bytes = BASE64
        .decode(payload)
        .map_err(|_| VaultClientError::protocol("vault signature payload is not valid base64"))?;
    let signature: [u8; 64] = bytes
        .try_into()
        .map_err(|_| VaultClientError::protocol("vault signature is not 64 bytes"))?;
    Ok(signature)
}

/// The Vault transit client. Cheap to clone (`reqwest::Client` is an
/// `Arc` internally; connection pools are shared).
#[derive(Clone)]
pub struct VaultClient {
    http: reqwest::Client,
    addr: String,
    mount: String,
    token: Option<VaultToken>,
}

impl std::fmt::Debug for VaultClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VaultClient")
            .field("addr", &self.addr)
            .field("mount", &self.mount)
            .field("token", &self.token)
            .finish()
    }
}

impl VaultClient {
    /// Build a client from resolved configuration. No network I/O
    /// happens here; requests are made lazily by the operations below.
    pub fn new(config: &VaultConfig) -> Self {
        Self {
            http: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(10))
                .build()
                .unwrap_or_default(),
            addr: config.addr().to_string(),
            mount: config.transit_mount().to_string(),
            token: config.token().cloned(),
        }
    }

    fn token(&self) -> Result<&VaultToken, VaultClientError> {
        self.token
            .as_ref()
            .ok_or_else(|| VaultClientError::protocol("VAULT_TOKEN is not configured"))
    }

    /// Build a secret-free error from a transport failure. The reqwest
    /// error text for a request never includes headers, but we take no
    /// chances: only the error's Display string is used and it is
    /// explicitly checked not to contain the token before inclusion.
    fn transport_error(&self, err: &reqwest::Error) -> VaultClientError {
        let reason = err.to_string();
        if let Some(token) = &self.token {
            if !token.secret().is_empty() && reason.contains(token.secret()) {
                return VaultClientError::unreachable("transport failure (details redacted)");
            }
        }
        VaultClientError::unreachable(&reason)
    }

    /// `GET /v1/sys/health` — Vault's liveness endpoint.
    pub async fn sys_health(&self) -> VaultServiceState {
        let url = format!("{}/v1/sys/health", self.addr.trim_end_matches('/'));
        // NOTE: sys/health is unauthenticated by design; the token is not
        // required and not attached.
        let response = match self.http.get(&url).send().await {
            Ok(r) => r,
            Err(e) => return VaultServiceState::Unreachable(e.to_string()),
        };
        match response.status().as_u16() {
            200 => VaultServiceState::Active,
            429 => VaultServiceState::Standby,
            472 => VaultServiceState::DisasterMode,
            473 => VaultServiceState::PerformanceStandby,
            501 => VaultServiceState::NotInitialized,
            503 => VaultServiceState::Sealed,
            other => VaultServiceState::UnexpectedStatus(other),
        }
    }

    /// `GET /v1/auth/token/lookup-self` — proves the configured token is
    /// valid. A 403 means the token is invalid or revoked.
    pub async fn token_lookup_self(&self) -> Result<(), VaultClientError> {
        let token = self.token()?;
        let url = format!(
            "{}/v1/auth/token/lookup-self",
            self.addr.trim_end_matches('/')
        );
        let response = self
            .http
            .get(&url)
            .header("X-Vault-Token", token.secret())
            .send()
            .await
            .map_err(|e| self.transport_error(&e))?;
        match response.status().as_u16() {
            200 => Ok(()),
            403 => Err(VaultClientError {
                code: "vault_token_forbidden",
                detail: "VAULT_TOKEN was rejected by Vault (invalid or revoked token)".to_string(),
            }),
            status => Err(VaultClientError::status("lookup-self", status)),
        }
    }

    /// `GET /v1/{mount}/keys/{name}` — key existence, type, and public
    /// key. The key name is normalized by `VaultConfig` and must be
    /// non-empty; this method refuses empty names rather than hitting
    /// `/keys/` (which would list keys — a different permission).
    pub async fn transit_key(&self, key_name: &str) -> Result<TransitKeyInfo, VaultClientError> {
        if key_name.is_empty() {
            return Err(VaultClientError::protocol(
                "no transit key name resolved for signer (set provider_ref or VAULT_TRANSIT_KEY)",
            ));
        }
        let token = self.token()?;
        let url = format!(
            "{}/v1/{}/keys/{}",
            self.addr.trim_end_matches('/'),
            self.mount,
            key_name
        );
        let response = self
            .http
            .get(&url)
            .header("X-Vault-Token", token.secret())
            .send()
            .await
            .map_err(|e| self.transport_error(&e))?;
        match response.status().as_u16() {
            200 => {}
            403 => {
                return Err(VaultClientError {
                    code: "vault_key_forbidden",
                    detail: format!(
                        "VAULT_TOKEN may not read transit key '{key_name}' (missing read on {mount}/keys/{key_name})",
                        mount = self.mount
                    ),
                })
            }
            404 => {
                return Err(VaultClientError {
                    code: "vault_key_not_found",
                    detail: format!("transit key '{key_name}' does not exist in mount '{mount}'", mount = self.mount),
                })
            }
            status => return Err(VaultClientError::status("transit/keys", status)),
        }
        let body: serde_json::Value = response
            .json()
            .await
            .map_err(|_| VaultClientError::protocol("transit/keys response is not JSON"))?;
        let data = body
            .get("data")
            .ok_or_else(|| VaultClientError::protocol("transit/keys response has no data"))?;
        let key_type = data
            .get("type")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string();
        let latest_version = data
            .get("latest_version")
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
        let deletion_time = data
            .get("deletion_time")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let public_key_b64 = data
            .get("keys")
            .and_then(|keys| keys.get(latest_version.to_string()))
            .and_then(|v| v.get("public_key"))
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        Ok(TransitKeyInfo {
            key_type,
            latest_version,
            public_key_b64,
            deletion_time_present: !deletion_time.is_empty(),
        })
    }

    /// `POST /v1/{mount}/sign/{name}` — sign a message with an ed25519
    /// transit key. Returns the raw 64-byte Ed25519 signature on success.
    ///
    /// This is the ONLY signing path in the Vault integration, and it
    /// always comes from a real Vault response — there is no synthetic
    /// signature anywhere in this module.
    pub async fn sign_ed25519(
        &self,
        key_name: &str,
        message: &[u8],
    ) -> Result<[u8; 64], VaultClientError> {
        if key_name.is_empty() {
            return Err(VaultClientError::protocol(
                "no transit key name resolved for signer (set provider_ref or VAULT_TRANSIT_KEY)",
            ));
        }
        let token = self.token()?;
        let url = format!(
            "{}/v1/{}/sign/{}",
            self.addr.trim_end_matches('/'),
            self.mount,
            key_name
        );
        let payload = serde_json::json!({ "input": BASE64.encode(message) });
        let response = self
            .http
            .post(&url)
            .header("X-Vault-Token", token.secret())
            .json(&payload)
            .send()
            .await
            .map_err(|e| self.transport_error(&e))?;
        match response.status().as_u16() {
            200 => {}
            403 => {
                return Err(VaultClientError {
                    code: "vault_sign_forbidden",
                    detail: format!(
                        "VAULT_TOKEN may not sign with transit key '{key_name}' (missing update on {mount}/sign/{key_name})",
                        mount = self.mount
                    ),
                })
            }
            404 => {
                return Err(VaultClientError {
                    code: "vault_key_not_found",
                    detail: format!("transit key '{key_name}' does not exist in mount '{mount}'", mount = self.mount),
                })
            }
            status => return Err(VaultClientError::status("transit/sign", status)),
        }
        let body: serde_json::Value = response
            .json()
            .await
            .map_err(|_| VaultClientError::protocol("transit/sign response is not JSON"))?;
        let signature_field = body
            .get("data")
            .and_then(|d| d.get("signature"))
            .and_then(|v| v.as_str())
            .ok_or_else(|| VaultClientError::protocol("transit/sign response has no signature"))?;
        parse_vault_signature(signature_field)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vault_signature_envelope_is_parsed_strictly() {
        let signature_bytes: [u8; 64] = core::array::from_fn(|i| (i as u8).wrapping_mul(7));
        let field = format!("vault:v1:{}", BASE64.encode(signature_bytes));
        let parsed = parse_vault_signature(&field).expect("valid envelope parses");
        assert_eq!(parsed, signature_bytes);
    }

    #[test]
    fn malformed_signatures_are_rejected_not_faked() {
        // wrong prefix
        assert!(parse_vault_signature("nope:v1:AAAA").is_err());
        // missing version
        assert!(parse_vault_signature("vault::AAAA").is_err());
        // extra segment
        assert!(parse_vault_signature("vault:v1:AAAA:BBBB").is_err());
        // invalid base64
        assert!(parse_vault_signature("vault:v1:!!!").is_err());
        // not 64 bytes (3 bytes here)
        assert!(
            parse_vault_signature(&format!("vault:v1:{}", BASE64.encode([1u8, 2, 3]))).is_err()
        );
        // empty
        assert!(parse_vault_signature("").is_err());
    }

    #[test]
    fn service_state_maps_vault_status_semantics() {
        assert!(VaultServiceState::Active.can_serve());
        assert!(!VaultServiceState::Standby.can_serve());
        assert!(!VaultServiceState::Sealed.can_serve());
        assert!(!VaultServiceState::NotInitialized.can_serve());
        assert!(!VaultServiceState::PerformanceStandby.can_serve());
        assert!(!VaultServiceState::DisasterMode.can_serve());
        assert!(!VaultServiceState::UnexpectedStatus(500).can_serve());
        assert!(!VaultServiceState::Unreachable("timeout".into()).can_serve());
        assert_eq!(VaultServiceState::Sealed.as_str(), "sealed");
    }

    #[test]
    fn transit_key_info_requires_ed25519() {
        let ok = TransitKeyInfo {
            key_type: "ed25519".into(),
            latest_version: 2,
            public_key_b64: Some("AA==".into()),
            deletion_time_present: false,
        };
        assert!(ok.is_ed25519());
        let wrong = TransitKeyInfo {
            key_type: "ecdsa-p256".into(),
            latest_version: 1,
            public_key_b64: Some("AA==".into()),
            deletion_time_present: false,
        };
        assert!(!wrong.is_ed25519());
    }

    #[test]
    fn client_debug_never_contains_token() {
        let _lock = crate::custody::test_support::ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        std::env::set_var("VAULT_ADDR", "https://vault.internal:8200");
        std::env::set_var("VAULT_TOKEN", "hvs.debug.leak.check");
        std::env::set_var("VAULT_TRANSIT_KEY", "k1");
        let (config, state) = VaultConfig::from_env();
        assert!(state.sign_ready());
        let client = VaultClient::new(&config);
        let rendered = format!("{:?}", client);
        assert!(!rendered.contains("hvs.debug.leak.check"), "{rendered}");
        std::env::remove_var("VAULT_ADDR");
        std::env::remove_var("VAULT_TOKEN");
        std::env::remove_var("VAULT_TRANSIT_KEY");
    }

    #[tokio::test]
    async fn signing_against_an_unreachable_vault_fails_closed() {
        let _lock = crate::custody::test_support::ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        // Port 1 on localhost is not a Vault: connection refused or reset.
        std::env::set_var("VAULT_ADDR", "http://127.0.0.1:1");
        std::env::set_var("VAULT_TOKEN", "hvs.unreachable.test");
        std::env::set_var("VAULT_TRANSIT_KEY", "k1");
        let (config, state) = VaultConfig::from_env();
        assert!(state.sign_ready());
        let client = VaultClient::new(&config);
        drop(_lock); // release the env lock before awaiting
        let err = client
            .sign_ed25519("k1", b"message")
            .await
            .expect_err("unreachable vault must refuse");
        assert_eq!(err.code, "vault_unreachable");
        // The error must not contain the token.
        assert!(!err.to_string().contains("hvs.unreachable.test"));
        std::env::remove_var("VAULT_ADDR");
        std::env::remove_var("VAULT_TOKEN");
        std::env::remove_var("VAULT_TRANSIT_KEY");
    }

    #[tokio::test]
    async fn missing_token_refuses_before_any_request() {
        let _lock = crate::custody::test_support::ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        std::env::set_var("VAULT_ADDR", "https://vault.internal:8200");
        std::env::remove_var("VAULT_TOKEN");
        let (config, _state) = VaultConfig::from_env();
        let client = VaultClient::new(&config);
        drop(_lock); // release the env lock before awaiting
        let err = client
            .sign_ed25519("k1", b"message")
            .await
            .expect_err("no token configured must refuse");
        assert_eq!(err.code, "vault_protocol");
        std::env::remove_var("VAULT_ADDR");
    }

    #[tokio::test]
    async fn empty_key_name_refuses_rather_than_listing_keys() {
        let _lock = crate::custody::test_support::ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        std::env::set_var("VAULT_ADDR", "https://vault.internal:8200");
        std::env::set_var("VAULT_TOKEN", "hvs.test");
        let (config, _state) = VaultConfig::from_env();
        let client = VaultClient::new(&config);
        drop(_lock); // release the env lock before awaiting
        let err = client
            .transit_key("")
            .await
            .expect_err("empty key name must refuse");
        assert_eq!(err.code, "vault_protocol");
        std::env::remove_var("VAULT_ADDR");
        std::env::remove_var("VAULT_TOKEN");
    }
}
