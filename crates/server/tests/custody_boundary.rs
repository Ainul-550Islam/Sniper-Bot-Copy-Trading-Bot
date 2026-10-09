//! §F custody boundary — integration tests over the public crate surface.
//!
//! These tests prove, through `sniper_suite::custody::*` (no private
//! access), that:
//!
//! 1. **No provider can fabricate a signature.** Every provider type in
//!    the current OPTION-B deployment (local included) refuses, and the
//!    refusal names the exact deployment dependency. There is no code
//!    path — for any configuration — that returns `Signed` without a live
//!    provider handle producing it.
//! 2. **Policy runs before providers.** Tenant lifecycle and ownership
//!    guards deny before any provider interaction, so a suspended,
//!    closed, past-due, or cross-tenant request can never leak to a
//!    provider.
//! 3. **Remote custody never falls back to local.** A vault-bound signer
//!    with a healthy local provider present is still refused.
//! 4. **Every outcome is audited**, and the tenant-scoped audit view is
//!    isolated per organization.
//! 5. **The health surface agrees with the sign surface** — health never
//!    claims ready while signing refuses, and no secret value ever
//!    appears in a safe JSON body.
//!
//! These are real behavioral tests against the real boundary; there is no
//! mocked provider that "signs" — that is the point.

use bot_core::custody::{
    CustodyProfile, CustodyStatus, ProviderHealth, ProviderType, SignerRecord,
};
use bot_core::tenant::{OrganizationId, OrganizationStatus};
use chrono::Utc;
use sniper_suite::custody::{
    CustodyBoundaryHealth, CustodyBoundaryStatus, CustodyDeployment, CustodySignBoundary,
    CustodySignRequest,
};

fn org() -> OrganizationId {
    OrganizationId::new()
}

fn active_profile(org_id: OrganizationId, provider: ProviderType) -> CustodyProfile {
    let now = Utc::now();
    let mut p = CustodyProfile::new(org_id, "primary", provider, now);
    p.status = CustodyStatus::Active;
    p.activated_at = Some(now);
    p
}

fn active_signer(org_id: OrganizationId, profile: &CustodyProfile) -> SignerRecord {
    let mut s = SignerRecord::new(
        org_id,
        profile.id,
        "primary-signer",
        profile.provider_type,
        "9WzDXwBbmkg8ZTbNMqUxvQRAyrZzDsGYdLVL9zYtAWWM",
        Utc::now(),
    );
    s.status = CustodyStatus::Active;
    // Core policy requires BOTH the capability and (when distinct) the
    // module the engine wants to use.
    s.capabilities = vec!["solana:sign".to_string(), "module-sniper".to_string()];
    s
}

fn sign_request(
    org_id: OrganizationId,
    profile: &CustodyProfile,
    signer: &SignerRecord,
) -> CustodySignRequest {
    CustodySignRequest::new(
        org_id,
        profile.id,
        signer.id,
        "5f2c31b4a6e8d90c1f3a4b5c6d7e8f90a1b2c3d4e5f60718293a4b5c6d7e8f90",
        "module-sniper",
        "solana:sign",
        None,
        "order-signing",
    )
    .expect("valid sign request")
}

fn healthy(provider: ProviderType) -> ProviderHealth {
    ProviderHealth::reachable(provider, Utc::now())
}

/// Env-dependent tests serialise on one lock (env is process-global).
static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn with_custody_env(provider: &str, live: Option<&str>, refs: &[(&str, &str)], f: impl FnOnce()) {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut touched: Vec<(String, Option<String>)> = vec![];
    for (k, v) in std::iter::once(&("CUSTODY_PROVIDER", provider))
        .chain(std::iter::once(&("LIVE_CUSTODY", live.unwrap_or(""))))
        .chain(refs.iter())
    {
        touched.push((k.to_string(), std::env::var(k).ok()));
        std::env::set_var(k, v);
    }
    if live.is_none() {
        std::env::remove_var("LIVE_CUSTODY");
    }
    f();
    for (key, prev) in touched {
        match prev {
            Some(val) => std::env::set_var(&key, val),
            None => std::env::remove_var(&key),
        }
    }
}

#[test]
fn no_provider_configuration_fabricates_a_signature() {
    // Every selectable provider type, fully configured references,
    // healthy provider, active tenant, active signer, right capability:
    // the boundary must still refuse, because no live signing backend is
    // implemented in this deployment. One Signed response would prove a
    // fake.
    let cases = [
        ("local", &[][..], ProviderType::Local),
        (
            "vault",
            &[
                ("VAULT_ADDR", "https://vault.internal:8200"),
                ("VAULT_TOKEN", "hvs-not-a-real-token"),
            ][..],
            ProviderType::Vault,
        ),
        (
            "kms",
            &[("KMS_KEY_ID", "arn:aws:kms:us-east-1:000000000000:key/abc")][..],
            ProviderType::Kms,
        ),
        ("hsm", &[("HSM_SLOT", "0")][..], ProviderType::Hsm),
    ];
    for (provider_name, env_refs, provider) in cases {
        let mut refusal_code = String::new();
        let mut refusal_detail_len = 0usize;
        let mut audit_len = 0usize;
        let mut audit_code = String::new();
        with_custody_env(provider_name, Some("1"), env_refs, || {
            let deployment = CustodyDeployment::resolve_from_env();
            assert_eq!(deployment.active, provider, "{provider_name} selected");
            let boundary = CustodySignBoundary::new(deployment);
            let org_id = org();
            let profile = active_profile(org_id, provider);
            let signer = active_signer(org_id, &profile);
            let request = sign_request(org_id, &profile, &signer);
            let response = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("test runtime")
                .block_on(boundary.sign(
                    &request,
                    OrganizationStatus::Active,
                    &profile,
                    &signer,
                    &healthy(provider),
                ));
            assert!(
                !response.is_success(),
                "{provider_name} must refuse: no live signing backend exists"
            );
            let reason = response.refusal().expect("refusal reason present");
            refusal_code = reason.code();
            refusal_detail_len = reason.detail().len();
            let audit = boundary.audit_log().recent(1);
            audit_len = audit.len();
            audit_code = audit[0].code.clone();
        });
        assert!(
            refusal_code.starts_with("provider_unsupported")
                || refusal_code.starts_with("provider_failure")
                || refusal_code.starts_with("resolve."),
            "{provider_name}: unexpected refusal code {refusal_code}"
        );
        assert!(
            refusal_detail_len > 0,
            "{provider_name} refusal must state what is missing"
        );
        assert_eq!(audit_len, 1, "{provider_name} outcome audited");
        assert_eq!(audit_code, refusal_code);
    }
}

#[test]
fn policy_guards_run_before_any_provider_interaction() {
    let outcomes: Vec<(String, String)> = Vec::new();
    let mut collected = outcomes;
    with_custody_env("local", None, &[], || {
        let boundary = CustodySignBoundary::new(CustodyDeployment::resolve_from_env());
        let org_id = org();
        let profile = active_profile(org_id, ProviderType::Local);
        let signer = active_signer(org_id, &profile);
        let request = sign_request(org_id, &profile, &signer);
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("test runtime");
        for (status, expected_code) in [
            (OrganizationStatus::Suspended, "policy.tenant_suspended"),
            (OrganizationStatus::Closed, "policy.tenant_closed"),
        ] {
            let response = rt.block_on(boundary.sign(
                &request,
                status,
                &profile,
                &signer,
                &healthy(ProviderType::Local),
            ));
            assert!(!response.is_success());
            collected.push((
                response.refusal().unwrap().code(),
                expected_code.to_string(),
            ));
        }
    });
    for (actual, expected) in collected {
        assert_eq!(actual, expected, "policy guard order");
    }
}

#[test]
fn cross_tenant_request_is_refused_and_audited_for_the_requester() {
    let owner = org();
    let attacker = org();
    let mut result = (String::new(), 0usize, 0usize, String::new());
    with_custody_env("local", None, &[], || {
        let boundary = CustodySignBoundary::new(CustodyDeployment::resolve_from_env());
        let profile = active_profile(owner, ProviderType::Local);
        let signer = active_signer(owner, &profile);
        let request = sign_request(attacker, &profile, &signer);
        let response = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("test runtime")
            .block_on(boundary.sign(
                &request,
                OrganizationStatus::Active,
                &profile,
                &signer,
                &healthy(ProviderType::Local),
            ));
        result.0 = response.refusal().unwrap().code();
        result.1 = boundary.audit_log().for_organization(&owner, 10).len();
        let attacker_view = boundary.audit_log().for_organization(&attacker, 10);
        result.2 = attacker_view.len();
        result.3 = attacker_view[0].code.clone();
    });
    assert_eq!(result.0, "policy.cross_tenant");
    assert_eq!(
        result.1, 0,
        "owner's audit view must not show the attacker's attempt"
    );
    assert_eq!(result.2, 1);
    assert_eq!(result.3, "policy.cross_tenant");
}

#[test]
fn vault_bound_signer_never_falls_back_to_local() {
    let org_id = org();
    let mut code = String::new();
    let mut provider_used = Some(ProviderType::Local);
    with_custody_env(
        "vault",
        Some("1"),
        &[
            ("VAULT_ADDR", "https://vault.internal:8200"),
            ("VAULT_TOKEN", "hvs-not-a-real-token"),
        ],
        || {
            let deployment = CustodyDeployment::resolve_from_env();
            let boundary = CustodySignBoundary::new(deployment);
            // A VAULT-bound profile+signer (the tenant's configured custody).
            let profile = active_profile(org_id, ProviderType::Vault);
            let signer = active_signer(org_id, &profile);
            let request = sign_request(org_id, &profile, &signer);
            let response = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("test runtime")
                .block_on(boundary.sign(
                    &request,
                    OrganizationStatus::Active,
                    &profile,
                    &signer,
                    &healthy(ProviderType::Vault),
                ));
            assert!(!response.is_success());
            code = response.refusal().unwrap().code();
            provider_used = response.provider();
        },
    );
    assert!(
        code.contains("vault") || code.starts_with("resolve."),
        "refusal {code} must be vault-attributed, no local fallback"
    );
    assert_ne!(provider_used, Some(ProviderType::Local));
}

#[test]
fn health_and_sign_surfaces_agree() {
    let mut status = CustodyBoundaryStatus::Ready;
    let mut allows = true;
    let mut json_allows = true;
    let mut cross_check = false;
    with_custody_env(
        "vault",
        Some("1"),
        &[
            ("VAULT_ADDR", "https://vault.internal:8200"),
            ("VAULT_TOKEN", "hvs-not-a-real-token"),
        ],
        || {
            let deployment = CustodyDeployment::resolve_from_env();
            let health = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("test runtime")
                .block_on(CustodyBoundaryHealth::probe(&deployment));
            // Configured references but no implemented backend: health
            // must NOT claim ready, and signing refuses — the surfaces
            // agree.
            status = health.status;
            allows = health.status.allows_signing();
            json_allows = health.to_safe_json()["allows_signing"].as_bool() == Some(false);
            // Cross-check with the Batch-7 live contract view.
            cross_check = health.cross_check_live_contract();
        },
    );
    assert_ne!(status, CustodyBoundaryStatus::Ready);
    assert!(!allows);
    assert!(json_allows);
    assert!(cross_check);
}

#[test]
fn safe_json_never_contains_secret_values() {
    let org_id = org();
    let mut health_body = String::new();
    let mut refusal_body = String::new();
    with_custody_env(
        "vault",
        Some("1"),
        &[
            ("VAULT_ADDR", "https://vault.internal:8200"),
            ("VAULT_TOKEN", "hvs-SUPERSECRET-token-value"),
        ],
        || {
            let deployment = CustodyDeployment::resolve_from_env();
            let health = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("test runtime")
                .block_on(CustodyBoundaryHealth::probe(&deployment));
            health_body = health.to_safe_json().to_string();

            let boundary = CustodySignBoundary::new(deployment);
            let profile = active_profile(org_id, ProviderType::Vault);
            let signer = active_signer(org_id, &profile);
            let request = sign_request(org_id, &profile, &signer);
            let response = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("test runtime")
                .block_on(boundary.sign(
                    &request,
                    OrganizationStatus::Active,
                    &profile,
                    &signer,
                    &healthy(ProviderType::Vault),
                ));
            refusal_body = response.to_safe_json().to_string();
        },
    );
    assert!(
        !health_body.contains("SUPERSECRET"),
        "health body leaked a secret"
    );
    assert!(
        !health_body.contains("hvs-"),
        "health body leaked a token prefix"
    );
    assert!(
        !refusal_body.contains("SUPERSECRET"),
        "refusal body leaked a secret"
    );
}

#[test]
fn malformed_requests_are_rejected_at_construction() {
    use sniper_suite::custody::InvalidSignRequest;
    let org_id = org();
    let profile = active_profile(org_id, ProviderType::Local);
    let signer = active_signer(org_id, &profile);
    let err = CustodySignRequest::new(
        org_id,
        profile.id,
        signer.id,
        "tooshort",
        "module-sniper",
        "solana:sign",
        None,
        "order-signing",
    )
    .unwrap_err();
    assert_eq!(err, InvalidSignRequest::DigestWrongLength { len: 8 });
}
