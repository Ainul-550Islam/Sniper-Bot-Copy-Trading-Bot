//! Transaction-signer abstraction and signer registry (key-custody boundary).
//!
//! # Why this module exists
//!
//! Trading modules (Sniper, Copy, execution layer) must never depend on a
//! concrete keypair implementation or touch private key material. They ask the
//! [`SignerRegistry`] for signing capability — by logical identity
//! (`primary_trading`, `treasury`, …) or by public key (transaction assembly
//! in [`crate::tx`]) — and receive a [`TransactionSigner`]: something that can
//! produce a signature for arbitrary message bytes without ever exposing the
//! secret behind it.
//!
//! # Backends
//!
//! * [`LocalKeypairSigner`] / [`Wallet`](crate::tokens::Wallet) — local key
//!   material loaded through the existing `SOLANA_KEYPAIR` wallet path. This
//!   is the only backend implemented in this build.
//! * Vault / KMS / HSM — declared in configuration
//!   ([`SigningProvider`](bot_core::config::SigningProvider)) but **not
//!   implemented**. Selecting one fails startup with
//!   [`SignerError::UnsupportedBackend`]; the app never silently falls back
//!   to local keys. Implementing a backend means implementing
//!   [`TransactionSigner`] (async by design — remote signers do I/O) and
//!   extending [`build_signer_registry`]; no business-logic changes required.
//!
//! # Scope
//!
//! Solana only. Polymarket order signing is EVM (secp256k1 + EIP-712) and
//! deliberately lives in `module-polymarket` — the two signing models are not
//! merged.
//!
//! # Secret hygiene
//!
//! No type in this module implements `Debug`/`Display` in a way that can emit
//! private key bytes; [`SignerError`] variants carry identities, public keys
//! and context strings only.

use std::sync::Arc;

use async_trait::async_trait;
use solana_sdk::message::VersionedMessage;
use solana_sdk::pubkey::Pubkey;
use solana_sdk::signature::Signature;
use tracing::info;

pub use bot_core::config::PRIMARY_SIGNER_IDENTITY;
use bot_core::config::{Config, SigningProvider};
use bot_core::error::{BotResult, SignerError};

use crate::tokens::Wallet;

/// Conventional logical identity for the sniper trading signer.
pub const SNIPER_IDENTITY: &str = "sniper";
/// Conventional logical identity for the copy-trading signer.
pub const COPY_TRADING_IDENTITY: &str = "copy_trading";
/// Conventional logical identity for treasury operations.
pub const TREASURY_IDENTITY: &str = "treasury";
/// Conventional logical identity for staking-program administration.
pub const STAKING_ADMIN_IDENTITY: &str = "staking_admin";

/// A thing that can sign Solana transaction payloads.
///
/// The unit of signing is the *serialized message*: for versioned
/// transactions the runtime signs `VersionedMessage::serialize()` bytes, so
/// one method covers legacy messages, v0 messages and arbitrary payloads
/// (Jupiter limit-order payloads, off-chain auth messages…).
///
/// Async because production backends (Vault/KMS/HSM) perform I/O per
/// signature; the local implementation completes synchronously inside the
/// async wrapper.
#[async_trait]
pub trait TransactionSigner: Send + Sync + std::fmt::Debug {
    // `Debug` is a supertrait on purpose: signer types are held in shared
    // registries that get logged during triage, so every implementation must
    // consciously provide a secret-free `Debug` (see [`LocalKeypairSigner`]
    // and [`Wallet`](crate::tokens::Wallet)).
    /// Public key whose signatures this signer produces.
    fn pubkey(&self) -> Pubkey;

    /// Sign raw message bytes. Implementations must never log or return the
    /// secret; failures are reported as [`SignerError::SigningFailed`] with a
    /// secret-free context string.
    async fn sign_message(&self, message: &[u8]) -> Result<Signature, SignerError>;

    /// Sign a compiled versioned (or legacy) message — serializes and
    /// delegates to [`Self::sign_message`].
    async fn sign_versioned_message(
        &self,
        message: &VersionedMessage,
    ) -> Result<Signature, SignerError> {
        self.sign_message(&message.serialize()).await
    }
}

/// Local-development / current-production signer: wraps the existing
/// [`Wallet`] (and therefore the existing keypair-loading path) behind the
/// [`TransactionSigner`] boundary.
pub struct LocalKeypairSigner {
    identity: String,
    wallet: Arc<Wallet>,
}

impl LocalKeypairSigner {
    /// Wrap an already-loaded wallet under a logical identity.
    pub fn new(identity: impl Into<String>, wallet: Wallet) -> Self {
        LocalKeypairSigner {
            identity: identity.into(),
            wallet: Arc::new(wallet),
        }
    }

    /// Wrap a shared wallet (e.g. the primary trading wallet) under an
    /// identity.
    pub fn from_wallet(identity: impl Into<String>, wallet: Arc<Wallet>) -> Self {
        LocalKeypairSigner {
            identity: identity.into(),
            wallet,
        }
    }

    /// Load key material from a keypair spec (path / base58 / JSON array —
    /// the exact formats [`Wallet::load`] already supports). Errors never
    /// contain the spec itself, only the identity and the underlying parse
    /// failure class.
    pub fn from_spec(identity: impl Into<String>, spec: &str) -> BotResult<Self> {
        let identity = identity.into();
        let wallet = Wallet::load(spec).map_err(|e| {
            bot_core::error::BotError::Signer(SignerError::SecretLoad {
                context: format!("identity '{identity}': {e}"),
            })
        })?;
        Ok(LocalKeypairSigner {
            identity,
            wallet: Arc::new(wallet),
        })
    }

    /// The logical identity this signer was registered under.
    pub fn identity(&self) -> &str {
        &self.identity
    }

    /// The wrapped wallet (public-key facts only; the keypair itself is not
    /// reachable through `Wallet`'s public API).
    pub fn wallet(&self) -> &Arc<Wallet> {
        &self.wallet
    }
}

#[async_trait]
impl TransactionSigner for LocalKeypairSigner {
    fn pubkey(&self) -> Pubkey {
        self.wallet.pubkey
    }

    async fn sign_message(&self, message: &[u8]) -> Result<Signature, SignerError> {
        Ok(self.wallet.sign_message_sync(message))
    }
}

/// Debug output is deliberately limited to identity + public key.
impl std::fmt::Debug for LocalKeypairSigner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LocalKeypairSigner")
            .field("identity", &self.identity)
            .field("pubkey", &self.wallet.pubkey.to_string())
            .finish()
    }
}

/// Named set of available signers.
///
/// Lookup is deterministic: identities are unique (registration rejects
/// duplicates) and iteration order is registration order. There is no default
/// fallback — requesting an identity that was not registered is an error, so
/// a module can never silently sign with an unrelated wallet.
pub struct SignerRegistry {
    entries: Vec<(String, Arc<dyn TransactionSigner>)>,
}

impl SignerRegistry {
    /// Empty registry (startup wiring adds the primary wallet).
    pub fn new() -> Self {
        SignerRegistry {
            entries: Vec::new(),
        }
    }

    /// Register a signer under a logical identity.
    ///
    /// Fails on empty names and duplicate identities — a second signer for
    /// the same name would make lookup ambiguous.
    pub fn register(
        &mut self,
        identity: impl Into<String>,
        signer: Arc<dyn TransactionSigner>,
    ) -> Result<(), SignerError> {
        let identity = identity.into();
        if identity.trim().is_empty() {
            return Err(SignerError::InvalidSigner {
                reason: "identity must not be empty".into(),
            });
        }
        if self.entries.iter().any(|(n, _)| *n == identity) {
            return Err(SignerError::DuplicateIdentity { identity });
        }
        self.entries.push((identity, signer));
        Ok(())
    }

    /// Look a signer up by identity.
    pub fn get(&self, identity: &str) -> Option<Arc<dyn TransactionSigner>> {
        self.entries
            .iter()
            .find(|(n, _)| n == identity)
            .map(|(_, s)| Arc::clone(s))
    }

    /// Like [`Self::get`] but returns a structured [`SignerError::NotFound`].
    pub fn require(&self, identity: &str) -> Result<Arc<dyn TransactionSigner>, SignerError> {
        self.get(identity).ok_or_else(|| SignerError::NotFound {
            identity: identity.to_string(),
        })
    }

    /// Find the signer that controls a public key (used by the transaction
    /// builder to satisfy the message's required-signer list). First
    /// registration wins when two identities share a key (aliases).
    pub fn find_by_pubkey(&self, pubkey: &Pubkey) -> Option<Arc<dyn TransactionSigner>> {
        self.entries
            .iter()
            .find(|(_, s)| &s.pubkey() == pubkey)
            .map(|(_, s)| Arc::clone(s))
    }

    /// Registered identity names, in registration order.
    pub fn identities(&self) -> Vec<&str> {
        self.entries.iter().map(|(n, _)| n.as_str()).collect()
    }

    /// (identity, pubkey) pairs — safe to log: public keys only.
    pub fn public_keys(&self) -> Vec<(&str, Pubkey)> {
        self.entries
            .iter()
            .map(|(n, s)| (n.as_str(), s.pubkey()))
            .collect()
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

impl Default for SignerRegistry {
    fn default() -> Self {
        Self::new()
    }
}

/// Secret-free by construction: identity names and public keys only.
impl std::fmt::Debug for SignerRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SignerRegistry")
            .field(
                "entries",
                &self
                    .entries
                    .iter()
                    .map(|(n, s)| (n.clone(), s.pubkey().to_string()))
                    .collect::<Vec<_>>(),
            )
            .finish()
    }
}

/// Build the process-wide registry from configuration + the loaded primary
/// wallet. This is the startup validation point for the signing subsystem:
///
/// * unsupported provider (`vault`/`kms`/`hsm` in this build) → hard error,
///   never a silent fallback to local keys;
/// * the primary trading wallet is always registered as
///   `primary_trading`;
/// * every configured identity must resolve (alias target registered,
///   keypair env var set and parseable, keypair path readable) or startup
///   fails with a structured, secret-free error;
/// * duplicate identities are rejected.
pub fn build_signer_registry(cfg: &Config, primary: Arc<Wallet>) -> BotResult<SignerRegistry> {
    if !cfg.signing.provider.is_supported() {
        return Err(SignerError::UnsupportedBackend {
            provider: cfg.signing.provider.as_str().to_string(),
        }
        .into());
    }

    let mut registry = SignerRegistry::new();
    let primary_pubkey = primary.pubkey;
    registry
        .register(
            PRIMARY_SIGNER_IDENTITY,
            primary.clone() as Arc<dyn TransactionSigner>,
        )
        .expect("fresh registry cannot have duplicates");

    for id in &cfg.signing.identities {
        let name = id.name.trim().to_string();
        let signer: Arc<dyn TransactionSigner> = if let Some(alias) = &id.alias {
            // Share an already-registered signer. No fallback: an unknown
            // alias is a hard error.
            Arc::clone(&registry.require(alias.trim())?)
        } else if let Some(env) = &id.keypair_env {
            let env = env.trim();
            let spec = std::env::var(env).map_err(|_| SignerError::SecretLoad {
                context: format!("identity '{name}': environment variable {env} is not set"),
            })?;
            if spec.trim().is_empty() {
                return Err(SignerError::SecretLoad {
                    context: format!("identity '{name}': environment variable {env} is empty"),
                }
                .into());
            }
            Arc::new(LocalKeypairSigner::from_spec(&name, &spec)?)
        } else if let Some(path) = &id.keypair_path {
            Arc::new(LocalKeypairSigner::from_spec(&name, path.trim())?)
        } else {
            return Err(SignerError::InvalidSigner {
                reason: format!(
                    "identity '{name}': exactly one of alias / keypair_env / keypair_path \
                     must be configured"
                ),
            }
            .into());
        };
        registry.register(&name, signer)?;
    }

    // Public-key visibility without private-key exposure (startup evidence).
    for (name, pubkey) in registry.public_keys() {
        info!(identity = %name, %pubkey, "signer registered");
    }
    if cfg.signing.provider != SigningProvider::Local {
        // Unreachable today (is_supported() gate above), kept explicit so the
        // match stays honest when new backends land.
        info!(provider = %cfg.signing.provider.as_str(), "signing provider");
    }
    debug_assert_eq!(
        registry.require(PRIMARY_SIGNER_IDENTITY)?.pubkey(),
        primary_pubkey
    );
    Ok(registry)
}

#[cfg(test)]
mod tests {
    use super::*;
    use bot_core::config::{SignerIdentityConfig, SigningConfig};
    use solana_sdk::hash::Hash;
    use solana_sdk::message::v0::Message as MessageV0;
    use solana_sdk::signature::Keypair;
    use solana_sdk::signer::Signer as SdkSigner;
    use solana_system_interface::instruction as system_instruction;

    fn wallet() -> Wallet {
        Wallet::generate()
    }

    #[tokio::test]
    async fn local_signer_pubkey_and_signature_verify() {
        let w = Arc::new(wallet());
        let pk = w.pubkey;
        let signer = LocalKeypairSigner::from_wallet(TREASURY_IDENTITY, Arc::clone(&w));
        assert_eq!(signer.pubkey(), pk);
        let msg = b"sign this message";
        let sig = signer.sign_message(msg).await.expect("local signing");
        assert!(sig.verify(pk.as_ref(), msg), "signature must verify");
        // A different message must not verify against the same signature.
        assert!(!sig.verify(pk.as_ref(), b"other message"));
    }

    #[tokio::test]
    async fn shared_wallet_signer_keeps_identity_and_pubkey() {
        let w = Arc::new(wallet());
        let signer = LocalKeypairSigner::from_wallet(SNIPER_IDENTITY, Arc::clone(&w));
        assert_eq!(signer.identity(), SNIPER_IDENTITY);
        assert_eq!(signer.pubkey(), w.pubkey);
        let sig = signer.sign_message(b"m").await.unwrap();
        assert!(sig.verify(w.pubkey.as_ref(), b"m"));
    }

    #[tokio::test]
    async fn sign_versioned_message_default_impl_matches_message_bytes() {
        let w = Arc::new(wallet());
        let msg = MessageV0::try_compile(
            &w.pubkey,
            &[system_instruction::transfer(
                &w.pubkey,
                &Pubkey::new_unique(),
                1,
            )],
            &[],
            Hash::default(),
        )
        .unwrap();
        let versioned = VersionedMessage::V0(msg.clone());
        let sig = w
            .sign_versioned_message(&versioned)
            .await
            .expect("wallet implements TransactionSigner");
        assert!(sig.verify(w.pubkey.as_ref(), &versioned.serialize()));
    }

    #[tokio::test]
    async fn from_spec_roundtrips_a_base58_keypair() {
        let kp = Keypair::new();
        let spec = bs58::encode(kp.to_bytes()).into_string();
        let signer =
            LocalKeypairSigner::from_spec("spec_identity", &spec).expect("base58 spec must load");
        assert_eq!(signer.pubkey(), kp.pubkey());
        let sig = signer.sign_message(b"x").await.unwrap();
        assert!(sig.verify(kp.pubkey().as_ref(), b"x"));
    }

    #[test]
    fn from_spec_error_does_not_echo_the_secret() {
        // A syntactically broken spec must produce a SecretLoad error whose
        // message does not contain the spec itself.
        let bogus = "not-a-real-key-spec-$$$";
        let err = LocalKeypairSigner::from_spec("bad", bogus).unwrap_err();
        let text = err.to_string();
        assert!(
            !text.contains(bogus),
            "secret spec leaked into error: {text}"
        );
        assert!(text.contains("bad"), "error should carry the identity");
    }

    #[test]
    fn local_signer_debug_is_redacted() {
        let kp = Keypair::new();
        let spec = bs58::encode(kp.to_bytes()).into_string();
        let signer = LocalKeypairSigner::from_spec("dbg", &spec).unwrap();
        let dbg = format!("{signer:?}");
        assert!(dbg.contains("dbg"));
        assert!(dbg.contains(&kp.pubkey().to_string()));
        assert!(!dbg.contains(&spec), "Debug leaked key material: {dbg}");
    }

    #[tokio::test]
    async fn registry_lookup_and_not_found() {
        let mut reg = SignerRegistry::new();
        let w = Arc::new(wallet());
        reg.register(
            PRIMARY_SIGNER_IDENTITY,
            Arc::clone(&w) as Arc<dyn TransactionSigner>,
        )
        .unwrap();
        assert!(reg.get(PRIMARY_SIGNER_IDENTITY).is_some());
        assert_eq!(
            reg.require(TREASURY_IDENTITY).unwrap_err(),
            SignerError::NotFound {
                identity: TREASURY_IDENTITY.to_string()
            }
        );
        // find_by_pubkey resolves the same signer.
        let found = reg.find_by_pubkey(&w.pubkey).expect("pubkey lookup");
        assert_eq!(found.pubkey(), w.pubkey);
        assert!(reg.find_by_pubkey(&Pubkey::new_unique()).is_none());
    }

    #[tokio::test]
    async fn registry_rejects_duplicate_and_empty_identities() {
        let mut reg = SignerRegistry::new();
        let w = Arc::new(wallet());
        reg.register("a", Arc::clone(&w) as Arc<dyn TransactionSigner>)
            .unwrap();
        assert_eq!(
            reg.register("a", Arc::clone(&w) as Arc<dyn TransactionSigner>)
                .unwrap_err(),
            SignerError::DuplicateIdentity {
                identity: "a".into()
            }
        );
        assert!(matches!(
            reg.register("  ", w as Arc<dyn TransactionSigner>)
                .unwrap_err(),
            SignerError::InvalidSigner { .. }
        ));
    }

    fn cfg_with(provider: SigningProvider, identities: Vec<SignerIdentityConfig>) -> Config {
        Config {
            signing: SigningConfig {
                provider,
                identities,
            },
            ..Config::default()
        }
    }

    #[tokio::test]
    async fn build_registry_registers_primary_by_default() {
        let w = Arc::new(wallet());
        let reg = build_signer_registry(&cfg_with(SigningProvider::Local, vec![]), Arc::clone(&w))
            .expect("local provider must build");
        assert_eq!(reg.len(), 1);
        let primary = reg.require(PRIMARY_SIGNER_IDENTITY).unwrap();
        assert_eq!(primary.pubkey(), w.pubkey);
    }

    #[tokio::test]
    async fn build_registry_rejects_unsupported_backends_without_fallback() {
        let w = Arc::new(wallet());
        for provider in [
            SigningProvider::Vault,
            SigningProvider::Kms,
            SigningProvider::Hsm,
        ] {
            let err = build_signer_registry(&cfg_with(provider, vec![]), Arc::clone(&w))
                .expect_err("unsupported provider must fail startup");
            match err {
                bot_core::error::BotError::Signer(SignerError::UnsupportedBackend {
                    provider: p,
                }) => assert_eq!(p, provider.as_str()),
                other => panic!("expected UnsupportedBackend, got {other:?}"),
            }
            // And it must be classified as a configuration failure.
            assert!(
                bot_core::error::BotError::Signer(SignerError::UnsupportedBackend {
                    provider: provider.as_str().into()
                })
                .is_alertable()
            );
        }
    }

    #[tokio::test]
    async fn build_registry_alias_shares_primary_signer() {
        let w = Arc::new(wallet());
        let cfg = cfg_with(
            SigningProvider::Local,
            vec![
                SignerIdentityConfig {
                    name: SNIPER_IDENTITY.into(),
                    alias: Some(PRIMARY_SIGNER_IDENTITY.into()),
                    ..Default::default()
                },
                SignerIdentityConfig {
                    name: COPY_TRADING_IDENTITY.into(),
                    alias: Some(SNIPER_IDENTITY.into()),
                    ..Default::default()
                },
            ],
        );
        let reg = build_signer_registry(&cfg, Arc::clone(&w)).expect("aliases must resolve");
        assert_eq!(reg.len(), 3);
        for id in [SNIPER_IDENTITY, COPY_TRADING_IDENTITY] {
            assert_eq!(reg.require(id).unwrap().pubkey(), w.pubkey);
        }
    }

    #[tokio::test]
    async fn build_registry_alias_to_unknown_identity_fails() {
        let w = Arc::new(wallet());
        let cfg = cfg_with(
            SigningProvider::Local,
            vec![SignerIdentityConfig {
                name: "ghost".into(),
                alias: Some("does_not_exist".into()),
                ..Default::default()
            }],
        );
        let err = build_signer_registry(&cfg, w).unwrap_err();
        assert!(err.to_string().contains("does_not_exist"));
    }

    #[tokio::test]
    async fn build_registry_env_identity_loads_and_missing_env_fails() {
        let kp = Keypair::new();
        let env_name = format!(
            "SIGNER_TEST_ENV_{}",
            bs58::encode(&kp.pubkey().to_bytes()[..8]).into_string()
        );
        std::env::set_var(&env_name, bs58::encode(kp.to_bytes()).into_string());

        let w = Arc::new(wallet());
        let cfg = cfg_with(
            SigningProvider::Local,
            vec![SignerIdentityConfig {
                name: TREASURY_IDENTITY.into(),
                keypair_env: Some(env_name.clone()),
                ..Default::default()
            }],
        );
        let reg = build_signer_registry(&cfg, Arc::clone(&w)).expect("env identity must load");
        assert_eq!(
            reg.require(TREASURY_IDENTITY).unwrap().pubkey(),
            kp.pubkey()
        );
        // The primary wallet stays independently registered.
        assert_eq!(
            reg.require(PRIMARY_SIGNER_IDENTITY).unwrap().pubkey(),
            w.pubkey
        );

        // Missing env var → SecretLoad, no silent skip.
        std::env::remove_var(&env_name);
        let err = build_signer_registry(&cfg, w).unwrap_err();
        let text = err.to_string();
        assert!(
            text.contains(&env_name) && text.contains("not set"),
            "{text}"
        );
    }

    #[tokio::test]
    async fn build_registry_rejects_identity_without_source() {
        let w = Arc::new(wallet());
        let cfg = cfg_with(
            SigningProvider::Local,
            vec![SignerIdentityConfig {
                name: "sourceless".into(),
                ..Default::default()
            }],
        );
        let err = build_signer_registry(&cfg, w).unwrap_err();
        assert!(err.to_string().contains("exactly one"));
    }

    #[tokio::test]
    async fn build_registry_rejects_duplicate_identities() {
        let w = Arc::new(wallet());
        let cfg = cfg_with(
            SigningProvider::Local,
            vec![
                SignerIdentityConfig {
                    name: "same".into(),
                    alias: Some(PRIMARY_SIGNER_IDENTITY.into()),
                    ..Default::default()
                },
                SignerIdentityConfig {
                    name: "same".into(),
                    alias: Some(PRIMARY_SIGNER_IDENTITY.into()),
                    ..Default::default()
                },
            ],
        );
        let err = build_signer_registry(&cfg, w).unwrap_err();
        assert!(matches!(
            err,
            bot_core::error::BotError::Signer(SignerError::DuplicateIdentity { .. })
        ));
    }

    #[test]
    fn registry_public_keys_exposes_pubkeys_only() {
        let mut reg = SignerRegistry::new();
        let w = Arc::new(wallet());
        reg.register("x", Arc::clone(&w) as Arc<dyn TransactionSigner>)
            .unwrap();
        let pairs = reg.public_keys();
        assert_eq!(pairs, vec![("x", w.pubkey)]);
        assert_eq!(reg.identities(), vec!["x"]);
        assert_eq!(reg.len(), 1);
        assert!(!reg.is_empty());
    }
}
