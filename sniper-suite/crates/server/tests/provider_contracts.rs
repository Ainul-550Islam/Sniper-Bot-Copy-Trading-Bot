//! Test canonical provider contract statuses (Batch 7).
//! HERMETIC — verify no external provider becomes PASS without execution evidence.
//! Verify missing credentials -> EXTERNAL_REQUIRED/NOT_RUN.

use sniper_suite::ops::provider_contract::{
    ProviderCapability, ProviderContract, ProviderContractRegistry, ProviderStatus,
};
use std::collections::HashMap;

#[test]
fn no_external_provider_defaults_to_pass() {
    // HERMETIC
    let r = ProviderContractRegistry::default_registry();
    for c in &r.contracts {
        assert_ne!(
            c.status,
            ProviderStatus::Pass,
            "provider {} capability {} must not default to PASS",
            c.provider,
            c.capability.as_str()
        );
    }
    assert!(r.all_not_pass_without_evidence());
}

#[test]
fn missing_credentials_external_required() {
    // HERMETIC
    let c = ProviderContract::new(
        "stripe",
        ProviderCapability::BillingCheckout,
        vec!["STRIPE_API_KEY".into()],
        "LIVE_BILLING=1 cargo test --test live_billing_contract",
        "evidence",
    )
    .external_required("missing STRIPE_API_KEY");
    assert_eq!(c.status, ProviderStatus::ExternalRequired);
}

#[test]
fn provider_contract_never_default_pass() {
    // HERMETIC
    let c = ProviderContract::new(
        "vault",
        ProviderCapability::CustodySign,
        vec!["VAULT_ADDR".into()],
        "cmd",
        "evidence",
    );
    assert_eq!(c.status, ProviderStatus::NotRun);
    assert!(!c.is_default_pass());
}

#[test]
fn status_semantics_exact() {
    // HERMETIC
    assert_eq!(ProviderStatus::Pass.as_str(), "PASS");
    assert_eq!(ProviderStatus::Fail.as_str(), "FAIL");
    assert_eq!(ProviderStatus::NotRun.as_str(), "NOT_RUN");
    assert_eq!(
        ProviderStatus::ExternalRequired.as_str(),
        "EXTERNAL_REQUIRED"
    );
    assert_eq!(ProviderStatus::Blocked.as_str(), "BLOCKED");
}

#[test]
fn postgres_without_url_is_external_required_via_runner() {
    // HERMETIC — runner should classify missing POSTGRES_URL as EXTERNAL_REQUIRED
    use sniper_suite::ops::provider_contract_runner::{
        ContractRunnerConfig, ProviderContractRunner,
    };
    let mut config = ContractRunnerConfig::default();
    config.enable("postgres");
    let runner = ProviderContractRunner::new(config);
    let contract = ProviderContract::new(
        "postgres",
        ProviderCapability::DatabaseMigration,
        vec!["POSTGRES_URL".into()],
        "POSTGRES_URL=... cargo test",
        "evidence",
    );
    let env: HashMap<String, String> = HashMap::new();
    let r = runner.run(&contract, &env);
    assert_eq!(r.status, ProviderStatus::ExternalRequired);
}

#[test]
fn solana_without_rpc_is_external_required() {
    // HERMETIC
    let c = ProviderContract::new(
        "solana_rpc",
        ProviderCapability::SolanaRpc,
        vec!["RPC_URL".into()],
        "RPC_URL=... cargo test --test solana_contract",
        "evidence",
    )
    .external_required("missing RPC_URL");
    assert_eq!(c.status, ProviderStatus::ExternalRequired);
}

#[test]
fn deployment_without_url_is_external_required() {
    // HERMETIC
    let c = ProviderContract::new(
        "deployment",
        ProviderCapability::DeploymentHealth,
        vec!["DEPLOYMENT_BASE_URL".into()],
        "DEPLOYMENT_BASE_URL=... cargo test --test deployment_smoke",
        "evidence",
    )
    .external_required("missing DEPLOYMENT_BASE_URL");
    assert_eq!(c.status, ProviderStatus::ExternalRequired);
}

// ---------------------------------------------------------------------------
// Batch 10 — evidence integrity + documented-command consistency
// ---------------------------------------------------------------------------

use sniper_suite::ops::external_evidence_verify::ExternalEvidenceVerifier;
use sniper_suite::ops::final_gap_ledger::FinalGapLedger;
use std::path::{Path, PathBuf};

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// Batch 10: every evidence file produced by `scripts/run-external-validation.sh`
/// must verify against the canonical hash rule (hash + schema). This makes the
/// documented command `cargo test --test provider_contracts` actually verify the
/// saved evidence — previously nothing executed the verification.
#[test]
fn evidence_files_verify_with_canonical_hash() {
    let dir = repo_root().join("evidence/external");
    if !dir.is_dir() {
        eprintln!(
            "NOT_RUN: evidence/external not present — run `bash scripts/run-external-validation.sh all-safe` first"
        );
        return;
    }
    let mut entries: Vec<PathBuf> = std::fs::read_dir(&dir)
        .expect("read evidence dir")
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().map(|x| x == "json").unwrap_or(false))
        .collect();
    entries.sort();
    assert!(
        entries.len() >= 6,
        "expected at least the six hermetic evidence files, found {}",
        entries.len()
    );
    let mut problems: Vec<String> = Vec::new();
    for path in &entries {
        match ExternalEvidenceVerifier::verify_file(path.to_str().unwrap()) {
            Ok(r) if r.valid && r.evidence_hash_match && r.schema_valid => {}
            Ok(r) => problems.push(format!("{}: {}", path.display(), r.detail)),
            Err(e) => problems.push(format!("{}: {e}", path.display())),
        }
    }
    assert!(
        problems.is_empty(),
        "evidence verification problems: {problems:?}"
    );
}

/// Batch 10: the ledger commands and the buyer-facing documents must agree.
/// Catches "command in documentation differs materially from the executable harness",
/// the removed `--dry-run-check` flag, and a staking command missing its workspace cd.
#[test]
fn ledger_commands_match_documented_harnesses() {
    let ledger = FinalGapLedger::default_ledger();
    let runbook = std::fs::read_to_string(repo_root().join("docs/EXTERNAL-VALIDATION-RUNBOOK.md"))
        .expect("runbook present");
    let gap_doc = std::fs::read_to_string(repo_root().join("docs/FINAL-BUYER-GAP-LEDGER.md"))
        .expect("gap ledger doc present");

    for g in &ledger.gaps {
        assert!(runbook.contains(&g.id), "runbook must map {}", g.id);
        assert!(gap_doc.contains(&g.id), "gap ledger doc must list {}", g.id);
    }

    let tokens = [
        ("GAP-001", "live_billing_contract"),
        ("GAP-002", "live_custody_contract"),
        ("GAP-003", "deployment_smoke"),
        ("GAP-004", "funded_mode_guard"),
        ("GAP-005", "validator_e2e"),
    ];
    for (id, token) in tokens {
        let cmd = &ledger.get(id).expect("gap present").verification_command;
        assert!(cmd.contains(token), "{id} command must reference {token}");
        assert!(
            runbook.contains(token),
            "runbook must show {token} for {id}"
        );
        assert!(
            gap_doc.contains(token),
            "gap ledger doc must show {token} for {id}"
        );
    }

    // Any line that actually documents the validator E2E command must cd into the
    // excluded programs workspace first (plain file references are not commands).
    for (name, doc) in [("runbook", &runbook), ("gap ledger", &gap_doc)] {
        for line in doc
            .lines()
            .filter(|l| l.contains("cargo test --test validator_e2e"))
        {
            assert!(
                line.contains("cd programs/staking-suite"),
                "{name}: validator E2E command must cd into programs/staking-suite: {line}"
            );
        }
    }

    // The removed fake flag and localhost-as-production must not appear anywhere.
    for (name, doc) in [("runbook", &runbook), ("gap ledger", &gap_doc)] {
        assert!(
            !doc.contains("--dry-run-check"),
            "{name} must not use the removed flag"
        );
    }
    assert!(!ledger
        .get("GAP-003")
        .unwrap()
        .verification_command
        .contains("localhost"));

    // Evidence schema/rule is documented for the buyer.
    assert!(
        runbook.contains("gap_id"),
        "runbook must document the gap_id field"
    );
    for f in [
        "billing_stripe.json",
        "custody_vault.json",
        "deployment_deployment.json",
        "solana_solana_rpc.json",
        "staking_staking_validator.json",
        "funded-preflight_funded.json",
    ] {
        assert!(runbook.contains(f), "runbook must name evidence file {f}");
    }
}
