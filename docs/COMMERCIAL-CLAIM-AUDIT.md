# Commercial Claim Audit — single source of truth

> Regenerated: 2026-10-08 (P0-B TASK 5 rework). This is the ONE claims table
> for the package.
> Rule (GAP-MAP v2, rule 2): a claim is closed only by a machine-generated
> `evidence/live/*.json` with `"status": "PASSED"` (or a passing test the
> buyer can re-run), **never by editing a document**. The gate
> `scripts/verify-marketing-claims.sh` fails CI/docs while unsupported
> terms (VERIFIED / 100% / HSM / FIPS / SOC2 / sub-millisecond, plus the
> ALWAYS-banned phrases listed in the script) appear without backing
> evidence.

## PUBLIC-SAFE logic

Every claim below carries TWO evidence columns:

1. **Shipped-artifact evidence** — code, harnesses, and scripts that exist
   in this tree and that a buyer can read or re-run. These facts are always
   safe to state.
2. **Live evidence** — a machine-generated `evidence/live/*.json` file with
   `"status": "PASSED"`. Until that file exists and says PASSED, the claim
   has NO live proof in this package.

A claim is **PUBLIC-SAFE** only when its live-evidence column shows a
PASSED file. Until then the only public wording allowed is the
**PUBLIC-SAFE wording** column: it describes the harness/artifact, never a
result. Claims #01, #04, #08, #10 below carry explicit rewordings; the same
rewording applies everywhere these claims are repeated (listing pages,
fact sheets, handover docs).

## 1. Live-performance and integration claims

| # | Claim category | Shipped-artifact evidence (in-tree) | Live evidence (`evidence/live/*.json`) | PUBLIC-SAFE? |
|---|----------------|--------------------------------------|----------------------------------------|--------------|
| 1 | Small funded Solana trade lands | `crates/solana-kit` sign/send/confirm path; `crates/solana-kit/tests/latency_bench.rs` (simulate leg) | `solana_funded_preflight.json` — exists, **NOT_RUN** | **NO** |
| 2 | pump.fun buy + sell round-trip | `crates/module-sniper` entry/exit logic; `tests/mock_pumpportal.rs` | `pumpfun_buy_sell_roundtrip.json` — exists, **NOT_RUN** | **NO** |
| 3 | PumpSwap buy + sell round-trip | `crates/module-sniper` swap routing | `pumpswap_buy_sell_roundtrip.json` — exists, **NOT_RUN** | **NO** |
| 4 | Polymarket place + cancel + fill | `crates/module-polymarket` (builder.rs/clob.rs order + cancel paths) + mock test suite | `polymarket_order_roundtrip.json` — exists, **NOT_RUN** | **NO** |
| 5 | Stripe checkout + webhook verify (test mode) | `live_billing_contract` harness; `docs/WEBHOOK-COMPATIBILITY-MATRIX.md` fixture tests | `stripe_checkout_roundtrip.json` — exists, **NOT_RUN** | **NO** |
| 6 | AWS KMS signing round-trip | `live_custody_contract` harness; signer registry code | `kms_sign_transit.json` — exists, **NOT_RUN** | **NO** |
| 7 | Vault Transit signing round-trip | `live_custody_contract` harness; signer registry code | `vault_transit.json` — exists, **NOT_RUN** | **NO** |
| 8 | Deployment smoke against a real deployment | `scripts/run-external-validation.sh` (`deployment_smoke` op); `docs/DEPLOYMENT.md` | `deployment_smoke.json` — exists, **NOT_RUN** | **NO** |
| 9 | Staking program e2e (devnet/validator) | `programs/staking-suite/tests/validator_e2e.rs` (gated on `STAKING_E2E=1`) | `staking_devnet_e2e.json` — exists, **NOT_RUN** | **NO** |
| 10 | Latency report (p50/p95 detect→submit→landed) | `crates/solana-kit/tests/latency_bench.rs` (read-only + simulate legs; no landed leg without funding) | `latency_report.json` — exists, **NOT_RUN** | **NO** |
| 11 | One green CI run at the release commit | `.github/workflows/ci.yml` exists | `ci_run.json` — exists, **NOT_RUN** | **NO** |

### PUBLIC-SAFE rewordings for claims #01, #04, #08, #10

Use ONLY these wordings in any public/buyer-facing surface until the
live-evidence column shows PASSED:

- **#01 (funded Solana trade):** "The Solana sign/send/confirm path ships
  with a latency harness whose simulate leg is runnable in-tree. A funded
  mainnet trade has NOT been demonstrated in this package; no run log
  ships."
- **#04 (Polymarket place + cancel + fill):** "The Polymarket module ships
  V2 order build, place and cancel paths with a mock test suite. A live
  place + cancel + fill round-trip has NOT been demonstrated in this
  package; no run log ships."
- **#08 (deployment smoke):** "A scripted deployment-smoke operation ships
  in `scripts/run-external-validation.sh`. A smoke run against a real
  deployment has NOT been demonstrated in this package; no run log ships."
- **#10 (latency report):** "A latency bench harness ships
  (`crates/solana-kit/tests/latency_bench.rs`, read-only + simulate legs).
  No p50/p95 detect→submit→landed numbers exist in this package; no run
  log ships."

## 2. External-validation records carried over (`evidence/external/`)

All six files currently declare `NOT_RUN` and remain so until a real run
writes `PASSED` with identifiers (rule 2):

| File | Status |
|------|--------|
| `evidence/external/billing_stripe.json` | NOT_RUN |
| `evidence/external/custody_vault.json` | NOT_RUN |
| `evidence/external/deployment_deployment.json` | NOT_RUN |
| `evidence/external/funded-preflight_funded.json` | NOT_RUN |
| `evidence/external/solana_solana_rpc.json` | NOT_RUN |
| `evidence/external/staking_staking_validator.json` | NOT_RUN |

## 3. Claims that may NEVER be made (regardless of evidence)

- Profit/risk-absence promises (the phrases banned by
  `scripts/verify-marketing-claims.sh`) — trading outcomes are uncertain;
  the license disclaims them.
- Any SOC 2 / FIPS / hardware-security-module certification for this
  product — no such audit or certification exists. (Code paths for
  KMS/Vault/hardware-module signing exist; that is a code fact, not a
  certification.)
- Sub-millisecond end-to-end latency — no measurement exists.
- "Audited" (security) — no external audit has been performed
  (`evidence/audits/` is empty).

## 4. What IS currently supportable (code facts, buyer can re-verify)

These are statements about the shipped source, checkable by reading or
running the code, and are the only basis for marketing today:

| Fact | Where to verify |
|------|-----------------|
| Workspace + program test suites exist and are runnable | `cargo test --workspace`; `programs/staking-suite/tests/` (requires a Rust toolchain; none is bundled) |
| Zero-panic hardening across execution, risk, accounting, data plane | `AUDIT-ROUND-{3..7}-2026-10-07.md` static-review findings (static review only — see each report's verification banner) |
| Proprietary license + third-party notices | `LICENSE`, `legal/THIRD-PARTY-NOTICES.md` |
| Claims gate fails on unsupported terms | `scripts/verify-marketing-claims.sh` |
| Release packaging refuses sandbox artifacts / ELF / >5 MB files | `scripts/build-release-package.sh` |

Everything else must wait for Section 1 evidence. Historical counts and
"VERIFIED" statements from earlier documents were archived with those
documents (`docs/archive/`) and are NOT part of the buyer package.
