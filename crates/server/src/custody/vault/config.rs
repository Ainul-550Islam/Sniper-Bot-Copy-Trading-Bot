//! Vault custody configuration (§G, spec file 49).
//!
//! Configuration is built exclusively from **references** — environment
//! variable names and identifiers — never from embedded secret material.
//! The only secret this module ever touches is the Vault service token,
//! which is read from `VAULT_TOKEN`, held in a redacted wrapper, and never
//! rendered by `Debug`, `Display`, logs, audit rows, or error strings.
//!
//! Environment contract (extends the single `LiveCustodyConfig` contract,
//! rule #35 — one env contract, not two):
//!
//! * `VAULT_ADDR` — Vault base address, e.g. `https://vault.internal:8200`
//! * `VAULT_TOKEN` — service token with `update` on `transit/sign/{key}`
//! * `VAULT_TRANSIT_MOUNT` — transit mount point (default `transit`)
//! * `VAULT_TRANSIT_KEY` — default ed25519 transit key name; a signer may
//!   override this per-signer through its `provider_ref`

use bot_core::custody::SignerRecord;
use url::Url;

/// A Vault service token that never renders its contents.
///
/// Constructed only from the environment. `Debug` prints a redaction
/// marker with the token's length — enough to debug configuration
/// problems, never enough to leak the secret.
#[derive(Clone)]
pub struct VaultToken {
    value: String,
}

impl VaultToken {
    /// Read the token from an environment value. Empty/whitespace values
    /// are rejected — an empty token must never be sent to Vault.
    pub fn from_env_value(value: &str) -> Result<Self, String> {
        let trimmed = value.trim();
        if trimmed.is_empty() {
            return Err("VAULT_TOKEN is empty".to_string());
        }
        Ok(Self {
            value: trimmed.to_string(),
        })
    }

    /// The raw token value — internal use only (request header). Never
    /// expose through Debug/Display/serialization.
    pub(crate) fn secret(&self) -> &str {
        &self.value
    }

    /// Non-revealing fingerprint for logs: length only.
    pub fn log_fingerprint(&self) -> String {
        format!("vault-token(len={})", self.value.len())
    }
}

impl std::fmt::Debug for VaultToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "VaultToken(REDACTED, {})", self.log_fingerprint())
    }
}

/// Which references a Vault deployment needs, and which are present.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct VaultReferenceState {
    /// `VAULT_ADDR` parses as an http/https URL.
    pub addr_present: bool,
    /// `VAULT_TOKEN` is set and non-empty.
    pub token_present: bool,
    /// `VAULT_TRANSIT_KEY` (or a per-signer `provider_ref`) names a key.
    pub key_present: bool,
}

impl VaultReferenceState {
    /// The deployment-wide references required before the Vault provider
    /// can do anything: address + token.
    pub fn base_ready(&self) -> bool {
        self.addr_present && self.token_present
    }

    /// Everything required for signing: address + token + a named key.
    pub fn sign_ready(&self) -> bool {
        self.base_ready() && self.key_present
    }

    /// Machine-readable, secret-free gap list for health surfaces.
    pub fn missing(&self) -> Vec<&'static str> {
        let mut out = Vec::new();
        if !self.addr_present {
            out.push("VAULT_ADDR");
        }
        if !self.token_present {
            out.push("VAULT_TOKEN");
        }
        if !self.key_present {
            out.push("VAULT_TRANSIT_KEY (or signer provider_ref)");
        }
        out
    }
}

/// Vault transit-engine configuration — references only, no secrets.
#[derive(Clone)]
pub struct VaultConfig {
    /// Parsed base address (scheme + host + port). No query or fragment —
    /// those could smuggle credentials and are rejected at parse time.
    addr: Url,
    token: Option<VaultToken>,
    /// Transit mount name (default `transit`).
    transit_mount: String,
    /// Default transit key name; optional when every signer pins its own
    /// key through `provider_ref`.
    default_key: Option<String>,
}

impl std::fmt::Debug for VaultConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VaultConfig")
            .field("addr", &self.addr.as_str())
            .field("token", &self.token)
            .field("transit_mount", &self.transit_mount)
            .field("default_key", &self.default_key)
            .finish()
    }
}

impl VaultConfig {
    /// Parse the base address. Rejects non-http(s) schemes and URLs that
    /// carry a query string or fragment (potential credential carriers).
    pub fn parse_addr(raw: &str) -> Result<Url, String> {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            return Err("VAULT_ADDR is empty".to_string());
        }
        let url = Url::parse(trimmed).map_err(|e| format!("VAULT_ADDR is not a valid URL: {e}"))?;
        if url.scheme() != "http" && url.scheme() != "https" {
            return Err(format!(
                "VAULT_ADDR must be http(s), got scheme {}",
                url.scheme()
            ));
        }
        if url.query().is_some() || url.fragment().is_some() {
            return Err("VAULT_ADDR must not contain a query string or fragment".to_string());
        }
        Ok(url)
    }

    /// Resolve the configuration from the operator environment. Never
    /// panics; every problem is carried in the returned reference state.
    pub fn from_env() -> (Self, VaultReferenceState) {
        let addr_raw = std::env::var("VAULT_ADDR").unwrap_or_default();
        let token_raw = std::env::var("VAULT_TOKEN").unwrap_or_default();
        let mount_raw = std::env::var("VAULT_TRANSIT_MOUNT").unwrap_or_default();
        let key_raw = std::env::var("VAULT_TRANSIT_KEY").unwrap_or_default();

        let addr = Self::parse_addr(&addr_raw).ok();
        let token = VaultToken::from_env_value(&token_raw).ok();
        let mount = {
            let m = mount_raw.trim().trim_matches('/');
            if m.is_empty() {
                "transit".to_string()
            } else {
                m.to_string()
            }
        };
        let default_key = {
            let k = key_raw.trim();
            if k.is_empty() {
                None
            } else {
                Some(k.to_string())
            }
        };

        let state = VaultReferenceState {
            addr_present: addr.is_some(),
            token_present: token.is_some(),
            key_present: default_key.is_some(),
        };

        // Fail-closed placeholder address: if VAULT_ADDR was invalid we
        // still need a URL-shaped value for the struct. Requests made
        // with this config are impossible — the client refuses to operate
        // unless `base_ready()` references were present, and health
        // reports exactly which reference is missing.
        let addr = addr
            .unwrap_or_else(|| Url::parse("http://vault.invalid:0").expect("static url parses"));

        (
            Self {
                addr,
                token,
                transit_mount: mount,
                default_key,
            },
            state,
        )
    }

    /// Base address (no credentials — it is scheme/host/port only).
    pub fn addr(&self) -> &Url {
        &self.addr
    }

    /// Transit mount name.
    pub fn transit_mount(&self) -> &str {
        &self.transit_mount
    }

    /// Default transit key, if configured.
    pub fn default_key(&self) -> Option<&str> {
        self.default_key.as_deref()
    }

    /// The token, if configured. `None` until `VAULT_TOKEN` is set.
    pub fn token(&self) -> Option<&VaultToken> {
        self.token.as_ref()
    }

    /// Resolve the transit key name for one signer record.
    ///
    /// Priority: the signer's own `provider_ref` (a `transit/keys/<name>`
    /// path or a bare key name) over the deployment-wide default. A
    /// reference that still contains a credential-looking form is
    /// rejected — references name resources, they never carry secrets.
    pub fn transit_key_for(&self, signer: &SignerRecord) -> Option<String> {
        if let Some(reference) = signer.provider_ref.as_deref() {
            let name = Self::key_name_from_reference(reference);
            if !name.is_empty() {
                return Some(name);
            }
        }
        self.default_key.clone()
    }

    /// Normalize a `provider_ref` into a transit key name.
    ///
    /// Accepted forms (both are the same resource in Vault's own API
    /// surface, `GET /v1/transit/keys/<name>`):
    /// * bare name — `sniper-mainnet`
    /// * keys path — `transit/keys/sniper-mainnet`
    ///
    /// Anything with a query string, fragment, `://`, or embedded
    /// whitespace is rejected (returns empty) so a malformed reference
    /// can never be interpolated into a request path.
    pub fn key_name_from_reference(reference: &str) -> String {
        let trimmed = reference.trim().trim_matches('/');
        if trimmed.is_empty()
            || trimmed.contains('?')
            || trimmed.contains('#')
            || trimmed.contains("://")
            || trimmed.contains(char::is_whitespace)
        {
            return String::new();
        }
        if let Some(stripped) = trimmed.strip_prefix("transit/keys/") {
            return stripped.trim_matches('/').to_string();
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
    fn addr_rejects_non_http_and_credential_carriers() {
        assert!(VaultConfig::parse_addr("").is_err());
        assert!(VaultConfig::parse_addr("ftp://vault:8200").is_err());
        assert!(VaultConfig::parse_addr("https://vault:8200/?token=x").is_err());
        assert!(VaultConfig::parse_addr("https://vault:8200/#frag").is_err());
        assert!(VaultConfig::parse_addr("https://vault.internal:8200").is_ok());
        assert!(VaultConfig::parse_addr("http://127.0.0.1:8200").is_ok());
    }

    #[test]
    fn from_env_reports_missing_references_without_panic() {
        let _lock = crate::custody::test_support::ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        env_isolated(
            &[
                ("VAULT_ADDR", None),
                ("VAULT_TOKEN", None),
                ("VAULT_TRANSIT_MOUNT", None),
                ("VAULT_TRANSIT_KEY", None),
            ],
            || {
                let (config, state) = VaultConfig::from_env();
                assert!(!state.base_ready());
                assert!(!state.sign_ready());
                assert_eq!(
                    state.missing(),
                    vec![
                        "VAULT_ADDR",
                        "VAULT_TOKEN",
                        "VAULT_TRANSIT_KEY (or signer provider_ref)"
                    ]
                );
                assert!(config.token().is_none());
                // The placeholder address is never usable for requests.
                assert_eq!(config.addr().as_str(), "http://vault.invalid:0/");
                // Default mount.
                assert_eq!(config.transit_mount(), "transit");
            },
        );
    }

    #[test]
    fn from_env_reads_all_references_and_defaults_mount() {
        let _lock = crate::custody::test_support::ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        env_isolated(
            &[
                ("VAULT_ADDR", Some("https://vault.internal:8200/")),
                ("VAULT_TOKEN", Some("  hvs.service.token  ")),
                ("VAULT_TRANSIT_MOUNT", Some("/transit/")),
                ("VAULT_TRANSIT_KEY", Some("sniper-mainnet")),
            ],
            || {
                let (config, state) = VaultConfig::from_env();
                assert!(state.base_ready());
                assert!(state.sign_ready());
                assert_eq!(config.addr().as_str(), "https://vault.internal:8200/");
                assert_eq!(config.transit_mount(), "transit");
                assert_eq!(config.default_key().unwrap(), "sniper-mainnet");
                // Token is held trimmed and never rendered.
                assert_eq!(config.token().unwrap().secret(), "hvs.service.token");
                assert!(!format!("{:?}", config).contains("hvs.service.token"));
            },
        );
    }

    #[test]
    fn token_debug_never_reveals_value() {
        let token = VaultToken::from_env_value("hvs.secret").unwrap();
        let rendered = format!("{:?}", token);
        assert!(rendered.contains("REDACTED"));
        assert!(!rendered.contains("hvs.secret"));
        assert!(token.log_fingerprint().contains("len=10"));
    }

    #[test]
    fn empty_token_is_rejected() {
        assert!(VaultToken::from_env_value("").is_err());
        assert!(VaultToken::from_env_value("   ").is_err());
    }

    #[test]
    fn key_reference_forms_normalize_to_key_name() {
        assert_eq!(
            VaultConfig::key_name_from_reference("transit/keys/sniper-mainnet"),
            "sniper-mainnet"
        );
        assert_eq!(
            VaultConfig::key_name_from_reference("sniper-mainnet"),
            "sniper-mainnet"
        );
        assert_eq!(
            VaultConfig::key_name_from_reference(" /transit/keys/k/ "),
            "k"
        );
        // Credential-carrier / malformed forms are refused.
        assert_eq!(VaultConfig::key_name_from_reference(""), "");
        assert_eq!(VaultConfig::key_name_from_reference("k?token=x"), "");
        assert_eq!(VaultConfig::key_name_from_reference("k#f"), "");
        assert_eq!(VaultConfig::key_name_from_reference("https://x/k"), "");
        assert_eq!(VaultConfig::key_name_from_reference("a b"), "");
    }
}
