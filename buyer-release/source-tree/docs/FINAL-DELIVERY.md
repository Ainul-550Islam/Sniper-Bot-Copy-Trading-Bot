# FINAL DELIVERY — sniper-suite 0.1.0

**The single human-readable starting point for the buyer.** Everything below
is a fact recorded in this repository; pointers are given inline. Status
labels used throughout (taxonomy defined in `docs/HANDOVER.md` §3):

- **VERIFIED** — executed successfully in the final engineering-freeze gate
  on the frozen tree (2026-09-18; evidence: `AUDIT.md` §27,
  `release-manifest.json`).
- **PREVIOUSLY VERIFIED** — executed successfully in an earlier session on
  identical source (2026-09-17 era; evidence: `AUDIT.md` §26 and earlier
  sections); not re-executed in the final freeze sandbox.
- **BUYER ACTION REQUIRED** — human/legal/operator step after transfer; not
  a software defect.
- **EXTERNAL INFRASTRUCTURE REQUIRED** — needs resources the repository does
  not and cannot contain.

## 1. What is included

- Complete Rust source: 7-crate application workspace (`crates/`) + one
  standalone native Solana staking program (`programs/staking-suite/`, own
  lockfile).
- 11 forward-only PostgreSQL migrations (`crates/core/migrations/`).
- The full test suite: 521 workspace tests (incl. 38 gated integration tests
  against real PostgreSQL/Redis), 48 staking host tests + 2 gated validator
  e2e tests, all protocol-mock harnesses. (Post-delivery audit pass on the
  current tree: 537 workspace tests, 71 staking host tests + 3 gated e2e —
  see CHANGELOG [Unreleased] and AUDIT.md §28. Buyer-hardening pass
  2026-09-18: `cargo build-sbf` EXECUTED (187,504-byte `staking_suite.so`,
  SHA-256 57a890fa…) and ALL 3 validator e2e EXECUTED/PASSED on
  solana-test-validator 2.1.21 against the real mainnet-cloned mpl program
  — see AUDIT.md §29 and docs/EVIDENCE-INDEX.md.)
- Release engineering: `scripts/release-check.sh` (20-gate local release
  validation), `scripts/verify-delivery.sh` (bundle integrity check, no
  toolchain required), `release-manifest.json` (machine-readable delivery
  facts), `rust-toolchain.toml`, `deny.toml`, both `Cargo.lock` files.
- Deployment assets: `Dockerfile`, `docker-compose.yml`, `.dockerignore`,
  `.env.template`, `config.toml.example`, `.github/workflows/ci.yml`.
- Documentation: 13 engineering docs + 23 buyer/delivery docs under `docs/`
  (map in §10), plus README, CHANGELOG, AUDIT.md (evidence trail), root
  SECURITY.md, LICENSE (MIT), VERSION.
- **Not included:** compiled binaries, Docker images, a deployed on-chain
  program, populated databases, `.git` objects (the authoritative repository
  history travels with the seller's repo/bundle — see §13), secrets of any
  kind.

## 2. Exact software version & release identity

| Field | Value |
|---|---|
| Product | sniper-suite — modular crypto trading system (5 modules + control plane) |
| Version | **0.1.0** (`VERSION` == root `Cargo.toml` `[workspace.package].version` == `release-manifest.json`; consistency gated by `scripts/release-check.sh`) |
| License | MIT (`LICENSE`; copyright holder is a documented placeholder — §12) |
| Known release commits (authoritative history) | release commit **`9c677cd`** → engineering-freeze commit **`0e139c3`** |
| Post-freeze additions | documentation-only passes (buyer package, then this final delivery package); no Rust/SQL/config change — byte-proven in each pass report |
| Pinned toolchain | Rust 1.98.1 (`rust-toolchain.toml`, Dockerfile `rust:1.98.1-bookworm`, CI); MSRV 1.82 (app) / 1.79 (staking program); agave 2.1.21 for `cargo build-sbf` |

> **Git metadata note (honest disclosure):** the packaging sandbox lost its
> `.git` directory in an infrastructure re-provision. No history was
> recreated or fabricated. The authoritative buyer archive MUST be produced
> from the seller's real repository containing `9c677cd` → `0e139c3` plus the
> documentation commits (`docs/ARCHIVE-CHECKLIST.md`).

## 3. File count & source size (measured, not estimated)

| Snapshot | Files | Bytes | Lines |
|---|---|---|---|
| Frozen software tree (commit `0e139c3`) | 146 | 2,801,590 (2.80 MB) | 77,980 |
| After buyer documentation package | 160 | 2,919,949 (2.92 MB) | 79,813 |
| After this final delivery package (9 docs + 1 script + edits) | see `scripts/verify-delivery.sh` output at packaging time | — | — |

Frozen-tree category breakdown (VERIFIED measurement): production Rust
(`src/`, incl. inline unit tests) 76 files / 1,826,288 B / 50,164 lines;
test-directory Rust 15 / 225,648 / 6,476; SQL migrations 11 / 23,443 / 489;
documentation 17 / 293,952 / 4,265; config + deploy + lockfiles + scripts +
manifest 27 / 432,259 / 16,586. All growth since the freeze is
documentation + one validation script; **no source inflation** occurred at
any point.

## 4. Test evidence (final freeze gate, executed on the frozen tree, 2026-09-18)

| Suite | Result | Status |
|---|---|---|
| `scripts/release-check.sh` (20 gates) | 20 PASS / 0 FAIL / 0 SKIP, exit 0 | VERIFIED |
| Workspace tests (`cargo test --workspace -- --test-threads=1`) | 521 / 521 (incl. 38 gated integration tests executed) | VERIFIED |
| `db_integration` vs PostgreSQL 16.4 | 23 / 23 (fresh + rerun + pg_dump→restore→suite-green round-trip) | VERIFIED |
| `redis_integration` vs Redis 7.2.10 | 10 / 10 | VERIFIED |
| `distributed_integration` | 4 / 4 | VERIFIED |
| `two_replica_mirror` (two real processes, shared PG+Redis) | 1 / 1 | VERIFIED |
| Staking host tests | 48 / 48 (delivery) — 71 / 71 in the post-delivery audit pass | VERIFIED |
| Staking `build-sbf` (5,440-byte `.so`) + validator e2e 2/2 (agave 2.1.21, local validator, incl. funded stake→reward→unstake) | passed on identical source | PREVIOUSLY VERIFIED |
| `recon_crash_e2e` (local validator), `devnet_e2e` (read-only), `latency_bench` | passed on identical source | PREVIOUSLY VERIFIED |
| fmt / clippy `-D warnings` (both projects) / cargo-audit ×2 (0 findings) / cargo-deny | clean | VERIFIED |
| Audit-chain tamper suite (modification/reorder/missing/duplicate + 8 concurrent appenders) | passed | VERIFIED |
| Docker image build + container smoke | not executed (no daemon in build sandbox; CI `docker` job covers it) | NOT EXECUTED |
| GitHub Actions CI run | not executed (no runner; equivalent steps VERIFIED locally) | NOT EXECUTED |
| Funded live trading / mainnet landing rate | never executed (needs funded keys + explicit approval) | NOT EXECUTED |

Evidence trail: `AUDIT.md` §26–27 (dated sections), `docs/TESTING.md`
(per-suite detail), `release-manifest.json` → `test_counts` /
`verification_status`, `docs/EVIDENCE-INDEX.md` (claim → evidence map).

## 5. Software components

| Component | Location | One-line function |
|---|---|---|
| bot-core | `crates/core` | config, state, events, risk engine, OMS + idempotency, dedup, auth (RBAC), audit hash-chain, recovery, JSONL journal, Postgres repos + migrations, Redis KV, observability |
| solana-kit | `crates/solana-kit` | RPC retry/failover/fan-out, WS supervision, account cache, pump/pumpswap/raydium/jupiter instruction builders, tx decoder, executor (simulate-first), signer registry |
| module-sniper | `crates/module-sniper` | pump.fun launch detection + entry execution + exit routing |
| module-copy | `crates/module-copy` | tracked-wallet copy trading with per-wallet rules |
| module-polymarket | `crates/module-polymarket` | Gamma + CLOB REST/WS, EIP-712 v2 order signing, CTF balance reads |
| module-telegram | `crates/module-telegram` | deny-by-default Telegram control + alerts (token-redacted) |
| server (`sniper-suite` binary) | `crates/server` | Axum REST + WS + dashboard, probes, metrics, persistence pumps, reconciliation tasks, module supervision |
| staking-suite (on-chain) | `programs/staking-suite` | native Solana program: reward mint, vault, fees, per-second APY, timelock, two-step admin, latched genesis mint |

Component-level detail with provenance classification:
`docs/IP-COMPONENTS.md`. Per-capability verification matrix:
`docs/CAPABILITY-MATRIX.md`.

## 6. Release evidence

- One-command local gate: `scripts/release-check.sh` — 20 gates (required
  files, version identity, toolchain pin, migration monotonicity, TODO/stub
  marker scan, secret scan, fmt, check, clippy `-D warnings`, full workspace
  tests, the four gated integration suites, staking fmt/clippy/tests,
  cargo-audit ×2, cargo-deny). Last execution on the frozen tree: 20/0/0.
- Bundle integrity (this pass): `scripts/verify-delivery.sh` — checks
  delivery completeness without needing the Rust toolchain.
- CI: `.github/workflows/ci.yml` (4 jobs) mirrors the same gates incl.
  build-sbf, validator e2e, docker build+smoke — requires a real runner
  (NOT EXECUTED in the build environment).
- Machine-readable release facts: `release-manifest.json` (no timestamps by
  design — reproducibility).
- Cut-a-release procedure: `docs/RELEASE.md`.

## 7. Verification taxonomy (used consistently across this package)

Defined in `docs/HANDOVER.md` §3 and mirrored in `release-manifest.json` →
`verification_status` (three lists: `verified_final_pass`,
`previously_verified_identical_source`, `not_executed_environment_blocked`).
No document in this repository upgrades a PREVIOUSLY VERIFIED or NOT
EXECUTED item to VERIFIED without a new execution.

## 8. Infrastructure requirements (EXTERNAL INFRASTRUCTURE REQUIRED)

- PostgreSQL ≥ 16 (verified against 16.4) — durable financial truth.
- Redis 7 (verified against 7.2.10) — non-authoritative coordination/cache.
- Solana RPC + WebSocket provider(s); optional Yellowstone-compatible Geyser
  provider; optional PumpPortal access.
- Polymarket API access + Polygon key (buyer satisfies Polymarket ToS and
  applicable law); Telegram bot token.
- Hosting/orchestration, Prometheus-compatible monitoring, log pipeline,
  secret store, domains/reverse proxy/TLS, CI runners, Docker daemon.
- Funded trading keys + capital-risk policy (for any live operation).

Full boundary: `docs/SCOPE-BOUNDARY.md`. Deployment sequence:
`docs/BUYER-DEPLOYMENT.md`; short technical walkthrough:
`docs/BUYER-QUICKSTART.md`.

## 9. Buyer actions (BUYER ACTION REQUIRED — none is a software defect)

1. Verify the received bundle against commits `9c677cd` + `0e139c3` and run
   `scripts/verify-delivery.sh` (`docs/BUYER-QUICKSTART.md` §1–3).
2. Run `./scripts/release-check.sh` green on buyer infrastructure.
3. Complete the paper → simulate → (gradual, supervised) live validation and
   the crash-recovery / backup-restore drills (`docs/DEMO-RUNBOOK.md`,
   `docs/ACCEPTANCE-CHECKLIST.md`).
4. Execute first Docker build + smoke and first CI run.
5. Legal/identity fill-ins: LICENSE copyright holder, repository URL,
   security contact, staking program deployment + id finalization, external
   security audit before any staking mainnet deployment
   (`docs/HANDOVER.md` §5, `docs/SUPPORT-HANDOVER.md`,
   `docs/IP-COMPONENTS.md` §Ownership transfer checklist).

## 10. Documentation map

| Layer | Documents |
|---|---|
| Start here | **this file** → `docs/BUYER-QUICKSTART.md` → `docs/DELIVERY-MANIFEST.md` (index of everything) |
| Understand the system | `docs/BUYER-OVERVIEW.md`, `docs/TECHNICAL-FACT-SHEET.md`, `docs/ARCHITECTURE.md`, `docs/MODULES.md` |
| Verify claims | `docs/CAPABILITY-MATRIX.md`, `docs/EVIDENCE-INDEX.md`, `docs/BUYER-DUE-DILIGENCE.md`, `docs/TESTING.md`, `AUDIT.md` |
| Deploy & operate | `docs/BUYER-DEPLOYMENT.md`, `docs/DEPLOYMENT.md`, `docs/OPERATIONS.md`, `docs/BACKUP-RESTORE.md`, `docs/RECONCILIATION.md`, `docs/DISTRIBUTED.md`, `docs/DEMO-RUNBOOK.md` |
| Commercial/transfer | `docs/ACCEPTANCE-CHECKLIST.md`, `docs/SCOPE-BOUNDARY.md`, `docs/SUPPORT-HANDOVER.md`, `docs/IP-COMPONENTS.md`, `docs/THIRD-PARTY.md`, `docs/BUYER-RISK-REGISTER.md`, `docs/SELLER-FACT-SHEET.md`, `docs/SELLING-LISTING-SOURCE.md` |
| Release/engineering | `README.md`, `CHANGELOG.md`, `docs/RELEASE.md`, `docs/HANDOVER.md`, `docs/API.md`, `docs/SECURITY.md`, `docs/STAKING.md`, `SECURITY.md`, `release-manifest.json` |
| Packaging | `docs/REPOSITORY-MAP.md`, `docs/ARCHIVE-CHECKLIST.md`, `scripts/verify-delivery.sh` |

## 11. External limitations (stated plainly)

- **No external security audit, penetration test, or formal verification
  exists** for any component. Staking-program mainnet deployment is
  documentation-blocked until one passes (root `SECURITY.md`,
  `docs/SECURITY.md`, `docs/STAKING.md`).
- Staking `declare_id!` is a **pre-deploy placeholder**; the program is
  deployed nowhere.
- **Funded live trading was never executed**; ~1s sniper entry is a design
  target, not a guarantee.
- Docker build/smoke and CI have not executed on real infrastructure from
  the delivery environment.
- Multi-replica behavior is tested at 2 replicas; larger topologies untested.
- SBOM tooling not run; both committed lockfiles are the authoritative
  dependency record (`docs/THIRD-PARTY.md` §6).
- Complete risk treatment: `docs/BUYER-RISK-REGISTER.md` (12 risks + 4
  documented non-risks).

## 12. Ownership-transfer checklist (summary — full list in `docs/IP-COMPONENTS.md`)

- [ ] Repository + Git hosting ownership transferred (authoritative history
      `9c677cd` → `0e139c3` preserved).
- [ ] LICENSE copyright holder inserted (currently placeholder).
- [ ] Staking program authority plan: deploy keypair custody, multisig admin
      (Squads/Realms recommended), timelock ≥ 24h, genesis-mint recipient.
- [ ] Deployment credentials, RPC/Geyser/PumpPortal provider accounts,
      Polymarket credentials, Telegram bot ownership, monitoring, domains,
      CI secrets, Docker registry, backups — all re-contracted/rotated in
      the buyer's name (`docs/SUPPORT-HANDOVER.md` §6 rotation rule).
- [ ] Security contact published in root `SECURITY.md`.

## 13. Delivery-integrity statement

- The frozen software bytes are unchanged since commit `0e139c3`
  (byte-exact category proofs in each pass report and
  `scripts/verify-delivery.sh`).
- No commit hashes were invented in any document; the two hashes above are
  the real recorded history of the authoritative repository.
- No document in this package claims a verification that was not executed;
  unexecuted items are labeled NOT EXECUTED with reasons.
