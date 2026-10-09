# Buyer Handover — single entry point

> Consolidated 2026-10-07. This document replaces the former `BUYER-*`,
> `FINAL-*`, and `CURRENT-*` families (40+ overlapping files); those are
> preserved for internal history under `docs/archive/` and are NOT part of
> the buyer package. The canonical documentation set is listed in §2.

## 1. What is being delivered

| Item | Where | Verify with |
|------|-------|-------------|
| Complete source (workspace crates, control-plane app, on-chain program, migrations, scripts) | package root | `SHA256SUMS.txt` in the package; `scripts/verify-delivery.sh` |
| Proprietary license + sale documents | `LICENSE`, `legal/` | counsel review; no `[PLACEHOLDER]` may remain |
| Third-party notices (707 packages; MIT/Apache-2.0/MPL-2.0/BSD/Unicode) | `legal/THIRD-PARTY-NOTICES.md` | cross-check against `licenses.csv` |
| OpenAPI spec for the HTTP API | `openapi/openapi.json` | `scripts/export-openapi.sh --check` |
| Evidence pack | `evidence/` | §5 below |
| Release manifest + checksums | root `VERSION`, package `SHA256SUMS.txt` | `sha256sum -c` |

## 2. Canonical documentation map (the docs that matter)

| Topic | Document |
|-------|----------|
| Architecture | `docs/ARCHITECTURE.md`, `docs/ARCHITECTURE-OVERVIEW.md` |
| Module guide (sniper / copy / polymarket / telegram) | `docs/MODULES.md` |
| API reference | `docs/API.md` (+ `openapi/openapi.json`) |
| Operations | `docs/OPERATIONS.md`, `docs/OPERATIONS-RUNBOOK.md` |
| Security controls & threat model | `docs/SECURITY.md`, `docs/SECURITY-THREAT-MODEL.md` |
| Deployment | `docs/DEPLOYMENT.md`, `docs/DEPLOYMENT-ENVIRONMENT-MATRIX.md` |
| Testing strategy | `docs/TESTING.md` |
| Known limitations (honest list) | `docs/KNOWN-LIMITATIONS.md` |
| Evidence index (claim → source) | `docs/EVIDENCE-INDEX.md` |
| Claims audit (single claims table) | `docs/COMMERCIAL-CLAIM-AUDIT.md` |
| Marketing claims currently permitted | `docs/CURRENT-MARKETING-CLAIMS-2026.md` |
| This handover | `docs/BUYER-HANDOVER.md` |

Everything else under `docs/` is working material; where it conflicts with
the list above, the list above wins.

## 3. What the buyer must do before first use

1. **Legal:** complete `LICENSE` (real legal entity), have counsel complete
   `legal/SOURCE-CODE-BILL-OF-SALE.md` or `legal/IP-ASSIGNMENT-TEMPLATE.md`,
   and work through `legal/REGULATORY-CHECKLIST.md` for each target market.
   The packaging gate refuses to build while the LICENSE placeholder stands.
2. **Identity:** the staking program ships with a placeholder program id —
   run `scripts/staking-identity.sh` with the buyer keypair before any
   deployment (see KNOWN-LIMITATIONS #10).
3. **Credentials:** provision the buyer's own RPC / Jito / Polymarket CLOB /
   Stripe / email / KMS-or-Vault accounts; the package contains none.
4. **Toolchain:** the package contains no compilers (by design — the
   packaging gate rejects binaries). Install Rust ≥ 1.82 and Node ≥ 20 to
   build and run the test suites.

## 4. Re-verification (the buyer's own evidence beats ours)

```text
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cd programs/staking-suite && cargo test          # + STAKING_E2E=1 with a validator
cd apps/control-plane && npm ci && npm run typecheck && npm test
scripts/verify-migration-graph.sh
scripts/export-openapi.sh --check
scripts/verify-marketing-claims.sh
scripts/build-release-package.sh                 # rebuild & guard check
```

Database-backed suites need `POSTGRES_MIGRATION_URL` (and optionally
`REDIS_URL`) pointing at real services; without them the gated tests skip —
that skip is reported, never hidden.

## 5. Evidence status — honest

- `evidence/live/` — **does not exist yet**. No live funded trade, latency
  report, Stripe test-mode run, KMS/Vault signing round-trip, deployment
  smoke, staking devnet e2e, or green-CI record has been captured. See
  `docs/COMMERCIAL-CLAIM-AUDIT.md` for the exact file each claim requires.
- `evidence/external/` — six records, all `NOT_RUN`.
- `evidence/audits/` — empty: no independent security audit or pentest has
  been performed. Do not describe the product as "audited".
- The static-review rounds (`AUDIT-ROUND-2..7`) are code-review evidence
  only: no compiler or test runner existed in the review environment.

## 6. Support

Maintenance, protocol-drift SLAs, and credential handover are contract
terms, not code features: see `legal/MAINTENANCE-AND-SUPPORT-TERMS.md`
(template; must be executed separately if a support window is purchased).
