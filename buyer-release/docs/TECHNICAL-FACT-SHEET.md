# Technical fact sheet — sniper-suite 0.1.0

Facts only, each supported by the repository. No promotional adjectives, no
comparisons, no projections.

## Identity

| Fact | Value | Source |
|---|---|---|
| Product name | sniper-suite | root `Cargo.toml`, `release-manifest.json` |
| Version | 0.1.0 | `VERSION`, `Cargo.toml`, `release-manifest.json` (gated consistent) |
| License | MIT (copyright holder placeholder pending transfer) | `LICENSE` |
| Release history | `9c677cd` (release) → `0e139c3` (engineering freeze) | authoritative git history (seller repo) |

## Language & codebase

| Fact | Value |
|---|---|
| Implementation language | Rust (100% of production code; 91 `.rs` files at freeze) |
| Frozen software tree size | 146 files / 2,801,590 bytes (2.80 MB) / 77,980 lines |
| Production Rust | 76 files / 1,826,288 B / 50,164 lines (incl. inline unit tests) |
| Test-directory Rust | 15 files / 225,648 B / 6,476 lines |
| SQL migrations | 11 files / 23,443 B / 489 lines |
| Pinned toolchain | Rust 1.98.1 (`rust-toolchain.toml` + Dockerfile + CI); MSRV 1.82 app / 1.79 program |
| On-chain build toolchain | agave 2.1.21 (`cargo build-sbf`), platform-tools v1.43 |

## Architecture

| Fact | Value |
|---|---|
| Workspace crates | 7: bot-core, solana-kit, module-sniper, module-copy, module-polymarket, module-telegram, server (`sniper-suite` binary) |
| Standalone programs | 1: `programs/staking-suite` (native Solana program, no Anchor, own lockfile) |
| Modules | 5 (sniper, copy, Polymarket, staking program, Telegram control) behind one Axum control plane |
| Event flow | in-process event bus (`crates/core/src/events.rs`); module supervision with defined startup/shutdown ordering |
| Configuration | single TOML, `deny_unknown_fields` (unknown keys rejected); precedence defaults → TOML → `.env` → env vars |

## API / control plane

| Fact | Value |
|---|---|
| HTTP framework | Axum (+ tower/tower-http) |
| Endpoints | 23 REST endpoints over 21 `/api` routes + WS feed `/api/events` + 4 infra routes (`/`, `/health`, `/ready`, `/metrics`) = 28 documented in `docs/API.md` |
| Dashboard | embedded single-file HTML served at `/` |
| AuthN | shared `x-api-key` on mutating routes; non-loopback bind refused without API auth |
| AuthZ | Telegram roles owner/operator/readonly; `/mode live` + key/journal mutations owner-only; readonly cannot mutate |
| Correlation | `x-request-id` on every response, honored when valid (≤128 chars `[A-Za-z0-9-_]`), mirrored in logs |
| Rate limiting | per-IP and per-principal on the API |

## Execution model

| Fact | Value |
|---|---|
| Modes | `paper` (default) / `simulate` (build + RPC-simulate, no broadcast) / `live` (dual gate: `mode="live"` **and** `allow_live_trading=true`, plus real keys) |
| Money-path pipeline | risk → ownership claim → idempotency → intent journal → authorization → execution → persistence → reconciliation → audit (maintenance invariant, `docs/HANDOVER.md` §6) |
| Broadcast policy | simulate-first by default (`SIMULATE_FIRST`, `ABORT_ON_SIMULATION_FAILURE`); optional fan-out race across RPCs (`BROADCAST_FANOUT`) |
| Venues | Solana (pump.fun bonding curve incl. v2, PumpSwap, Raydium AMM v1/v2, Jupiter routing) + Polymarket CLOB (EIP-712 v2 order signing) |
| Signing | `TransactionSigner` + named `SignerRegistry`; multi-signer completeness enforced; `local` custody implemented, `vault`/`kms`/`hsm` fail startup |

## Persistence

| Fact | Value |
|---|---|
| Authoritative store | PostgreSQL ≥ 16 (verified 16.4); sqlx; 11 forward-only migrations 0001–0011 embedded in the binary |
| Non-authoritative store | Redis 7 (verified 7.2.10): dedup L2, claims coordination, cache — may die without losing money-relevant truth |
| Local journal | JSONL intent journal with rotation + corrupt-line tolerance |
| Dedup | 3 levels: memory / Redis / Postgres, deterministic idempotency keys |

## Reconciliation & recovery

| Fact | Value |
|---|---|
| Model | intents recorded before send; startup replay + venue-truth resolution before new work; ambiguity matrix + handoff grace (`docs/RECONCILIATION.md`) |
| Attribution | unknown on-chain transactions attributed via migration `0006_transaction_attribution` |
| Crash testing | restart-fidelity, corrupt-line, OMS restart-recovery tests VERIFIED; `recon_crash_e2e` vs local validator PREVIOUSLY VERIFIED |

## Distributed ownership

| Fact | Value |
|---|---|
| Invariant | one logical execution ⇒ ≤1 active owner ⇒ ≤1 money-moving submission |
| Mechanisms | claim stores (PG authoritative / Redis / memory), leases + epochs + fencing tokens, handoff grace, cross-replica kill-switch + module-flag sync, position-book sync, tighten-only `GlobalRiskOracle`, append-only `execution_claim_events` lineage |
| Tested | `distributed_integration` 4/4 + `two_replica_mirror` 1/1 (two real processes) VERIFIED; >2 replicas untested |

## Observability

| Fact | Value |
|---|---|
| Logs | tracing; text or JSON; exactly one info line per HTTP request with `request_id` |
| Probes | `/health` liveness (process-only, always 200 while serving); `/ready` readiness (503 + component report when degraded) |
| Metrics | Prometheus text 0.0.4 at `/metrics`; stable `bot_*` names; bounded labels only (no symbols/wallets/signatures/secrets); full table in README §Observability, names verified against source |

## Staking program (on-chain)

| Fact | Value |
|---|---|
| Type | native Solana program (borsh instructions; PDAs `["staking-config"]`, `["staking-stake", staker]`; errors 6000+) |
| Features | reward mint (authority = config PDA), vault + fee treasury ATAs, deposit fee, per-second APY accrual, param caps (fee ≤ 10%, reward ≤ 100% APR), queue/apply/cancel timelock (permissionless apply; delay change waits old delay), pause-deposits-only (withdrawals never gated), two-step admin transfer, one-shot latched `GenesisMint` (second attempt → 6026) |
| Build | `cargo build-sbf` → 5,440-byte `.so` (PREVIOUSLY VERIFIED, agave 2.1.21) |
| Deployment status | NOT deployed; `declare_id!` is a pre-deploy placeholder |
| Audit status | NO external audit; mainnet deployment documentation-blocked until one passes |

## Test counts (final freeze gate on the frozen tree, 2026-09-18)

| Suite | Count |
|---|---|
| Workspace total | 521 passed / 0 failed (incl. 38 gated integration executed) |
| db_integration / redis_integration / distributed_integration / two_replica_mirror | 23 / 10 / 4 / 1 — all passed |
| Staking host / validator e2e | 48 passed / 2 gated-skipped in freeze sandbox (e2e 2/2 PREVIOUSLY VERIFIED); hardening pass 2026-09-18 on the audit-pass source: 71 host passed / **3 validator e2e EXECUTED + PASSED** (Agave 2.1.21, .so SHA-256 57a890fa…) |
| Release gate | 20 PASS / 0 FAIL / 0 SKIP |
| Whole-script test executions | 609 / 0 failures |

## CI

| Fact | Value |
|---|---|
| Workflow | `.github/workflows/ci.yml`, 4 jobs: app workspace (fmt, clippy `-D warnings`, build, test vs PG16+Redis7 service containers, compose-config gate); staking program (fmt, clippy, host tests, build-sbf, gated validator e2e); security (cargo-audit ×2, cargo-deny advisories/bans/sources + licenses); docker (image build + container health smoke) |
| Execution status | workflow delivered; a CI run requires the buyer's runner — NOT EXECUTED from the delivery environment |

## Dependency scanning

| Fact | Value |
|---|---|
| Lockfiles | `Cargo.lock` (706 packages) + `programs/staking-suite/Cargo.lock` (580 packages), both committed |
| cargo-audit | both lockfiles, 0 findings at freeze (cargo-audit 0.22.2) |
| cargo-deny | advisories/bans/licenses/sources policy in `deny.toml`; license allow-list of 14 permissive licenses; unknown registries + all git deps denied; clean at freeze (cargo-deny 0.18.9) |
| SBOM | generator not run (NOT EXECUTED); lockfiles are the authoritative record; command documented in `docs/RELEASE.md` / `docs/THIRD-PARTY.md` §7 |

## Docker

| Fact | Value |
|---|---|
| Image | multi-stage, non-root runtime user, healthcheck, base `rust:1.98.1-bookworm` |
| Compose | bot + `postgres:16-alpine` + `redis:7-alpine`, healthcheck-gated, API on loopback by default |
| Execution status | build + smoke NOT EXECUTED in delivery sandbox (no daemon); static inspection + `docker compose config` CI gate only |

## Security posture

| Fact | Value |
|---|---|
| Defaults | paper mode; all modules disabled in `Config::default()` |
| Secrets | env-var indirection only; secret-scan release gate; redacted `/api/config`; token-redaction regression test for Telegram error paths |
| Audit trail | append-only hash-chained; `GET /api/audit/verify`; advisory-lock serialized appends; tamper suite VERIFIED |
| Bind safety | non-loopback bind refused without API auth |
| External audit | **none exists** (any component) |

## External-audit status (explicit)

No external security audit, penetration test, or formal verification has
been performed on any component of this repository. This is stated in root
`SECURITY.md` and `docs/SECURITY.md`, and no document in this repository
claims otherwise.
