//! LIVE HashiCorp Vault transit integration — executed against a real Vault
//! server, not a stub.
//!
//! This test is `#[ignore]`d by default because it needs an external service.
//! It was written to be run against a genuine `vault server -dev` with the
//! transit engine enabled and an ed25519 key named by `VAULT_TRANSIT_KEY`:
//!
//! ```text
//! vault secrets enable transit
//! vault write -f transit/keys/sniper-ed25519 type=ed25519
//! export VAULT_ADDR=http://127.0.0.1:8200
//! export VAULT_TOKEN=root
//! export VAULT_TRANSIT_KEY=sniper-ed25519
//! cargo test -p sniper-suite --test vault_live_transit -- --ignored --nocapture
//! ```
//!
//! Every signature asserted below was produced by Vault's own transit engine
//! and verified with the ed25519 public key Vault returned. Nothing here is
//! mocked, and no local key material exists anywhere in the path.

use base64::Engine;
use bot_core::custody::{
    CustodyProfileId, CustodyProvider, CustodyProviderError, CustodySigner, CustodyStatus,
    ProviderType, SignerRecord,
};
use bot_core::tenant::OrganizationId;
use sniper_suite::custody::vault::{VaultClient, VaultConfig, VaultCustodyProvider};
use solana_sdk::pubkey::Pubkey;

fn live_config() -> Option<VaultConfig> {
    let addr = std::env::var("VAULT_ADDR").unwrap_or_default();
    let token = std::env::var("VAULT_TOKEN").unwrap_or_default();
    let key = std::env::var("VAULT_TRANSIT_KEY").unwrap_or_default();
    if addr.is_empty() || token.is_empty() || key.is_empty() {
        eprintln!(
            "SKIPPED: set VAULT_ADDR/VAULT_TOKEN/VAULT_TRANSIT_KEY (a live Vault transit engine) \
             to run this test"
        );
        return None;
    }
    let (config, state) = VaultConfig::from_env();
    assert!(
        state.sign_ready(),
        "VAULT_ADDR/VAULT_TOKEN/VAULT_TRANSIT_KEY must all resolve: {state:?}"
    );
    Some(config)
}

fn record_for(public_address: &str, provider_ref: Option<&str>) -> SignerRecord {
    let mut record = SignerRecord::new(
        OrganizationId::new(),
        CustodyProfileId::new(),
        "vault-live-signer".to_string(),
        ProviderType::Vault,
        public_address.to_string(),
        chrono::Utc::now(),
    );
    record.provider_ref = provider_ref.map(|s| s.to_string());
    record.status = CustodyStatus::Active;
    record
}

/// `GET /v1/sys/health` + `GET /v1/auth/token/lookup-self` against real Vault.
#[tokio::test]
#[ignore = "requires a live Vault transit engine (VAULT_ADDR/VAULT_TOKEN/VAULT_TRANSIT_KEY)"]
async fn live_vault_health_and_token_permission() {
    let Some(config) = live_config() else { return };
    let provider = VaultCustodyProvider::new(config.clone());
    provider
        .health_check()
        .await
        .expect("real Vault must report service-ready and accept the token");

    // The same two calls, one layer down, so the evidence names what ran.
    let client = VaultClient::new(&config);
    let state = client.sys_health().await;
    assert!(state.can_serve(), "vault sys/health state: {state:?}");
    client
        .token_lookup_self()
        .await
        .expect("real Vault must accept the service token");
}

/// The transit key really is ed25519 and really lives in Vault.
#[tokio::test]
#[ignore = "requires a live Vault transit engine (VAULT_ADDR/VAULT_TOKEN/VAULT_TRANSIT_KEY)"]
async fn live_vault_transit_key_metadata() {
    let Some(config) = live_config() else { return };
    let key_name = config.default_key().expect("VAULT_TRANSIT_KEY").to_string();
    let client = VaultClient::new(&config);
    let info = client
        .transit_key(&key_name)
        .await
        .expect("transit key must exist in the live Vault");
    assert!(info.is_ed25519(), "live key type: {info:?}");
    let public_key_b64 = info
        .public_key_b64
        .clone()
        .expect("live Vault key must expose a public key");
    assert!(!public_key_b64.is_empty());
}

/// The end-to-end signing path: resolve → Vault `transit/sign` → ed25519
/// signature that verifies against Vault's own public key. Then the same
/// signature must FAIL against a different message — proof the signature is
/// real and message-bound, not a placeholder.
#[tokio::test]
#[ignore = "requires a live Vault transit engine (VAULT_ADDR/VAULT_TOKEN/VAULT_TRANSIT_KEY)"]
async fn live_vault_sign_and_verify() {
    let Some(config) = live_config() else { return };
    let key_name = config.default_key().expect("VAULT_TRANSIT_KEY").to_string();
    let client = VaultClient::new(&config);
    let info = client.transit_key(&key_name).await.expect("live key");
    let raw_public_key = base64::engine::general_purpose::STANDARD
        .decode(info.public_key_b64.expect("public key"))
        .expect("Vault public key must be valid base64");
    let provider_pubkey = Pubkey::try_from(raw_public_key.as_slice())
        .expect("Vault public key must decode to an ed25519 pubkey");

    let provider = VaultCustodyProvider::new(config.clone());
    let record = record_for(&provider_pubkey.to_string(), None);
    let signer: std::sync::Arc<dyn CustodySigner> = provider
        .resolve_signer(&record)
        .await
        .expect("resolver must bind the live key");
    assert_eq!(signer.pubkey(), provider_pubkey);

    let message = b"sniper-suite live vault transit probe".to_vec();
    let signature = signer
        .sign_message(&message)
        .await
        .expect("Vault must return a signature");
    let pubkey_bytes = provider_pubkey.to_bytes();
    assert!(
        signature.verify(&pubkey_bytes, &message),
        "signature returned by Vault must verify against Vault's public key"
    );
    // Same key, different message → must not verify.
    assert!(
        !signature.verify(&pubkey_bytes, b"a different message"),
        "a Vault signature must be bound to the message it signed"
    );

    // A second signature over the same message from the same key must also
    // verify (ed25519 is deterministic, so it is byte-identical).
    let again = signer.sign_message(&message).await.expect("second sign");
    assert_eq!(signature, again, "ed25519 is deterministic in Vault");
}

/// Fail-closed against the REAL Vault: a record pinned to the wrong key, a
/// key that does not exist, and a bad token.
#[tokio::test]
#[ignore = "requires a live Vault transit engine (VAULT_ADDR/VAULT_TOKEN/VAULT_TRANSIT_KEY)"]
async fn live_vault_fails_closed_on_real_service() {
    let Some(config) = live_config() else { return };
    let provider = VaultCustodyProvider::new(config.clone());

    // 1. Pinned address ≠ Vault's key → PubkeyMismatch, never a signature.
    let wrong = Pubkey::new_unique();
    let err = provider
        .resolve_signer(&record_for(&wrong.to_string(), None))
        .await
        .expect_err("a stale pinned pubkey must be refused by the live provider");
    assert!(
        matches!(err, CustodyProviderError::PubkeyMismatch { .. }),
        "unexpected error: {err:?}"
    );

    // 2. A transit key that does not exist → NotConfigured (nothing to sign).
    let missing = record_for("", Some("transit/keys/does-not-exist-live-probe"));
    let err = provider
        .resolve_signer(&missing)
        .await
        .expect_err("a missing transit key must be refused");
    assert!(
        matches!(err, CustodyProviderError::NotConfigured(_)),
        "unexpected error: {err:?}"
    );

    // 3. A token the real Vault rejects → an error, never "healthy". The
    //    provider maps Vault's 403 to `NotConfigured` (Vault is present but
    //    does not serve this deployment) and transport faults to `Transport`;
    //    either way the probe must fail closed rather than report readiness.
    let original_token = std::env::var("VAULT_TOKEN").unwrap_or_default();
    std::env::set_var("VAULT_TOKEN", "hvs.definitely-not-a-valid-token");
    let (bad_config, _state) = VaultConfig::from_env();
    let bad_provider = VaultCustodyProvider::new(bad_config);
    let err = bad_provider.health_check().await;
    // Restore the working token before asserting, so a failure here cannot
    // leak a bad token into any other test in this binary.
    std::env::set_var("VAULT_TOKEN", original_token);
    let err = err.expect_err("a token the real Vault rejects must not report healthy");
    assert!(
        matches!(
            err,
            CustodyProviderError::Transport(_) | CustodyProviderError::NotConfigured(_)
        ),
        "unexpected error: {err:?}"
    );
}

#[test]
fn base64_engine_roundtrip_is_trustworthy() {
    // Independent check that the decoder used above is correct before it is
    // trusted in the live test: 32 zero bytes → 44 base64 chars.
    let encoded = base64::engine::general_purpose::STANDARD.encode([0u8; 32]);
    assert_eq!(encoded, "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=");
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(&encoded)
        .expect("decode");
    assert_eq!(decoded, vec![0u8; 32]);
}
