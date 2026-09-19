# sniper-suite 0.1.0 — Commercial / Buyer Due-Diligence Package Report

Prepared from the frozen engineering tree (release commit `9c677cd`, freeze commit `0e139c3`, 146 tracked files / 2,801,590 bytes / 77,980 lines at freeze). This pass added the buyer/due-diligence documentation package. **No Rust, SQL, config, CI or script file was changed** — proven byte-exactly in §7.

> **Environment incident (disclosed for honesty):** the build sandbox was re-provisioned mid-session. The tracked file tree survived and was verified byte-identical to the frozen measurements, but the `.git` metadata directory, the Rust build cache, and the local PostgreSQL/Redis services did not survive. Consequently `git diff --check` and a full `release-check.sh` re-run were NOT EXECUTED in this session (justification in §7). No commit hash was invented and no git operation was fabricated.

---

## 1. COMMERCIALIZATION AUDIT

What was audited before writing a single buyer document:

1. **Frozen-state verification** — file count (146) and total bytes (2,801,590) matched the freeze measurement exactly before any edit; `VERSION` = `0.1.0`; `release-manifest.json` version/test counts cross-checked against `docs/TESTING.md`, `docs/HANDOVER.md` §3 and the run-7 gate record in `AUDIT.md` §27.
2. **Fact sourcing** — every claim written into the 14 new docs was traced to: source paths (verified to exist), `release-manifest.json`, README, CHANGELOG, docs/HANDOVER.md, docs/TESTING.md, SECURITY.md, deny.toml, ci.yml, release-check.sh, or the recorded final-gate results. No new verification was claimed.
3. **Directive compliance** — no marketing language, no valuation/sale-price claims, no "best"/ranking claims, no fake customer or live-trading claims, no fake audit claims, no source inflation (docs only; 2.80 MB → 2.92 MB total tree, all of it documentation), no code refactor (§17 respected byte-exactly).
4. **Consistency decisions taken (documented, minimal):**
   - `release-manifest.json` `docs_count` 13 → 27: the field became factually stale the moment the 14 docs landed in `docs/`; this is the only manifest change (§19 respected — no redundant fields added).
   - `docs/HANDOVER.md` §1 bullet and README layout comment: "13 documents" → 13 engineering + 14 buyer-package docs (factual drift correction only).
   - `CHANGELOG.md`: new `## [Unreleased]` entry recording the documentation pass; the historical `[0.1.0]` entry (incl. its "thirteen docs" statement, true at cut) was NOT rewritten.
   - `AUDIT.md`: untouched. Historical sections stay historical (§16). No new section was appended because this pass produced no engineering evidence — only documentation.
   - README: added a concise "Buyer / engineering handover" subsection under Documentation (§15 — useful, not marketing).
5. **Placeholder discipline** — the four deliberate placeholders (LICENSE holder, repository URL, security contact, staking program id) are reproduced as *buyer/seller actions* everywhere they appear; none was filled with a fake value.
6. **One pre-existing wording noted, not "fixed":** `docs/HANDOVER.md` §2 says "staking 50/50 host (gated e2e skipped)" while the manifest records 48 host + 2 gated e2e. Both describe the same run (48 host tests + the 2 e2e tests that self-skip and report as passed inside their gated binary); the manifest is authoritative and the new docs use its numbers. Historical text was left as-is per §16/§17 (no real defect).

## 2. FILES CREATED (14)

| # | File | Bytes | Lines |
|---|------|-------|-------|
| 1 | docs/BUYER-OVERVIEW.md | 12,618 | 220 |
| 2 | docs/CAPABILITY-MATRIX.md | 9,922 | 58 |
| 3 | docs/BUYER-DUE-DILIGENCE.md | 10,648 | 98 |
| 4 | docs/IP-COMPONENTS.md | 10,543 | 207 |
| 5 | docs/THIRD-PARTY.md | 6,339 | 125 |
| 6 | docs/BUYER-DEPLOYMENT.md | 9,160 | 207 |
| 7 | docs/ACCEPTANCE-CHECKLIST.md | 7,354 | 133 |
| 8 | docs/RELEASE-NOTES-0.1.0.md | 6,011 | 107 |
| 9 | docs/BUYER-FAQ.md | 8,652 | 154 |
| 10 | docs/SCOPE-BOUNDARY.md | 5,393 | 99 |
| 11 | docs/SUPPORT-HANDOVER.md | 6,571 | 133 |
| 12 | docs/BUYER-RISK-REGISTER.md | 7,634 | 35 |
| 13 | docs/TECHNICAL-DIFFERENTIATORS.md | 7,747 | 139 |
| 14 | docs/DELIVERY-MANIFEST.md | 7,548 | 80 |

Total: 116,140 bytes / 1,795 lines (all Markdown; zero source code duplicated into docs beyond short factual references).

## 3. FILES MODIFIED (4)

| File | Before | After | Change |
|------|--------|-------|--------|
| README.md | 28,311 B | 29,395 B | + "Buyer / engineering handover" subsection (links only); layout comment 13 → 27 docs. No other line touched. |
| CHANGELOG.md | 6,645 B | 7,348 B | + `## [Unreleased]` entry describing the documentation pass. Historical 0.1.0 entry untouched. |
| docs/HANDOVER.md | 7,380 B | 7,812 B | §1 "what is being handed over" docs bullet updated to list the 14 buyer docs. Nothing else touched. |
| release-manifest.json | 5,420 B | 5,420 B | `components.docs_count`: 13 → 27 (single value; same byte length). |

**Unmodified (byte-proven in §7):** all 91 `.rs` files, all 11 `.sql` migrations, both `Cargo.lock` files, all `Cargo.toml` files, `Dockerfile`, `docker-compose.yml`, `.env.template`, `config.toml.example`, `deny.toml`, `rust-toolchain.toml`, `.github/workflows/ci.yml`, `scripts/release-check.sh`, `AUDIT.md`, `SECURITY.md`, `LICENSE`, `VERSION`, and the other 12 engineering docs.

---

== SECTION 4: COMPLETE CONTENT OF EVERY CREATED FILE ==


----- COMPLETE FILE: docs/BUYER-OVERVIEW.md (12618 bytes, 220 lines) -----

`````markdown
# Buyer technical overview

Part of the buyer / due-diligence package for **sniper-suite 0.1.0** (frozen
engineering tree, release commit `9c677cd`, freeze commit `0e139c3`).
Everything stated here is backed by source files in this repository; pointers
are given inline. This is a technical description, not a sales document. No
market-value, sale-price, or performance-superiority claims are made anywhere
in this package.

## 1. Product overview

sniper-suite is a modular crypto trading system written entirely in Rust. It
bundles five cooperating modules behind one control plane (Axum REST +
WebSocket + embedded HTML dashboard), with PostgreSQL as the durable source of
financial truth, Redis as non-authoritative coordination/cache, and a Telegram
bot for remote on/off control.

The system defaults to **paper** trading. Live execution requires two explicit
configuration gates (`[execution] mode = "live"` **and**
`allow_live_trading = true`) plus real key material; with the second gate off,
live requests are downgraded and nothing is broadcast (README §"Going live",
`crates/core/src/config.rs`).

Delivery size at freeze: **146 tracked files, 2,801,590 bytes (2.80 MB),
77,980 lines** (measurement method in `docs/BUYER-DUE-DILIGENCE.md` §A).

## 2. Module overview

| # | Module | Crate / path | Function |
|---|--------|--------------|----------|
| 1 | Sniper | `crates/module-sniper` | Detects new pump.fun launches (PumpPortal WS, Geyser `transactionSubscribe`, poll fallback) and executes entries, with PumpSwap/Raydium/Jupiter exit routing (`detect.rs`, `entry.rs`, `exit.rs`). |
| 2 | Copy trading | `crates/module-copy` | Mirrors buys (and optionally exits) of tracked wallets with per-wallet rules, sizing and staleness guards (`feeds.rs`, `mirror.rs`, `exit.rs`). |
| 3 | Polymarket | `crates/module-polymarket` | Gamma + CLOB REST/WS integration, EIP-712 v2 order signing, CTF ERC-1155 balance reads (`gamma.rs`, `clob.rs`, `eip712.rs`, `orders.rs`, `ctf.rs`, `ws.rs`, `auth.rs`, `strategy.rs`). |
| 4 | Staking program | `programs/staking-suite` | Standalone native Solana program (no Anchor): reward mint, vault + fee treasury, per-second APY accrual, deposit fee, parameter timelock, two-step admin transfer, pause-deposits-only, hard parameter caps, one-time latched `GenesisMint`. |
| 5 | Telegram control | `crates/module-telegram` | Long-polling bot: deny-by-default RBAC, module on/off, kill switch, rate-limited alerts (`commands.rs`, `alerts.rs`, `api.rs`). |

Shared plumbing: `crates/core` (`bot-core` — config, state, event bus, risk,
OMS, dedup, auth, audit, recovery, journal storage, Postgres repositories,
Redis KV, observability) and `crates/solana-kit` (RPC, WS supervision, account
cache, instruction builders, transaction executor, signer registry, swap
decoding). `crates/server` is the runnable `sniper-suite` binary.

## 3. Architecture

Seven-crate Cargo workspace (`crates/`) plus one standalone program crate
(`programs/staking-suite`, own lockfile, excluded from the workspace). Data
flows over an in-process event bus (`crates/core/src/events.rs`); modules are
supervised by the server binary (`crates/server/src/main.rs`) with defined
startup/shutdown ordering (`docs/ARCHITECTURE.md`). Persistence pumps
(`crates/server/src/persist.rs`) write authoritative state to PostgreSQL;
reconciliation tasks (`crates/server/src/recon.rs`) resolve intent outcomes
against on-chain/venue truth (`docs/RECONCILIATION.md`).

## 4. Control plane

Axum server (`crates/server/src/api.rs`, `ws.rs`, `dashboard.rs`):
23 REST endpoints over 21 `/api` routes, a WebSocket event feed
(`/api/events`), and 4 infrastructure routes (`/`, `/health`, `/ready`,
`/metrics`) — 28 endpoints documented route-by-route in `docs/API.md`.
Mutating routes require the `x-api-key` header when `API_KEY` is set; the
server refuses to bind a non-loopback address without API auth configured.
Every response carries a correlated `x-request-id`. Liveness (`/health`) is
process-only; readiness (`/ready`) reflects component state and returns 503
when degraded.

## 5. Execution path

Every money-moving action follows one pipeline (maintenance invariant,
`docs/HANDOVER.md` §6):

**risk → ownership claim → idempotency → intent journal → authorization →
execution → persistence → reconciliation → audit**

Concretely: the global risk engine (`crates/core/src/risk.rs`) decides first;
distributed ownership claims the logical execution
(`crates/core/src/ownership.rs`, `redis_ownership.rs`, `db/claims.rs`); the
OMS assigns idempotency keys and drives the order state machine
(`crates/core/src/oms.rs`); the intent journal records intent before send
(`crates/core/src/storage.rs` + migration `0007_intent_journal.sql`);
the Solana executor builds, simulate-first checks, signs via the signer
registry and broadcasts (`crates/solana-kit/src/execute.rs`, `tx.rs`,
`signer.rs`); fills/positions persist to PostgreSQL
(`crates/core/src/db/repo.rs`); reconciliation resolves ambiguous outcomes
(`crates/core/src/reconciliation.rs`); and every step appends to the
hash-chained audit trail (`crates/core/src/audit.rs`).

## 6. Risk engine

`crates/core/src/risk.rs`: global pre-trade checks (position capacity,
exposure limits, daily-loss auto-disable), enforced before execution for all
modules — no module may bypass it. In multi-replica deployments a
cluster-wide `GlobalRiskOracle` propagates limits and may only **tighten**
them (`docs/DISTRIBUTED.md`). Kill switch halts everything
(`POST /api/kill`, Telegram `/kill`) and is synced across replicas via
runtime flags (migration `0010_runtime_flags.sql`).

## 7. Persistence

PostgreSQL is the durable source of truth for orders, executions, positions,
trades, intents, claims, flags and the audit trail — 11 forward-only
migrations (`crates/core/migrations/0001`–`0011`, embedded in the binary via
sqlx, applied at startup when `auto_migrate` is on). Redis is explicitly
non-authoritative (dedup L2, coordination, cache) and may die without losing
money-relevant truth (`docs/BACKUP-RESTORE.md`). A local JSONL journal with
rotation and corrupt-line tolerance provides restart fidelity independent of
the database (`crates/core/src/storage.rs`). Restart-safe dedup spans three
levels: memory / Redis / Postgres (`crates/core/src/dedup.rs`).

## 8. Reconciliation

`crates/core/src/reconciliation.rs` + `crates/server/src/recon.rs`: on startup
and continuously, recorded intents are resolved against venue truth (Solana
transaction status, Polymarket order state). Ambiguous outcomes (e.g.
simulate-mode `Sent`, broadcast timeouts) enter a handoff-grace path rather
than being silently re-executed; the full source-of-truth model, ambiguity
matrix and PnL replay rules are in `docs/RECONCILIATION.md`. Transaction
attribution (matching unknown on-chain transactions to intents) is backed by
migration `0006_transaction_attribution.sql`.

## 9. Distributed ownership

Invariant: **one logical execution ⇒ at most one active owner ⇒ at most one
money-moving submission** (`docs/DISTRIBUTED.md`). Implementation: claim
stores with three backends (Postgres authoritative —
`db/claims.rs`/`0009_execution_claims.sql`; Redis — `redis_ownership.rs`;
in-memory for single-process), leases + epochs + fencing tokens, handoff grace
for ambiguous outcomes, cross-replica kill-switch/module-flag sync, position-
book sync, and the append-only `execution_claim_events` lineage table
(`0011_execution_claim_events.sql`). Tested by `distributed_integration`
(4/4) and `two_replica_mirror` (1/1) against real PG + Redis.

## 10. Observability

Structured logs (text or JSON), exactly one info line per HTTP request with
`request_id` correlation; Prometheus metrics at `/metrics` (text format
0.0.4) with stable `bot_*` names and bounded label sets — never symbols,
wallets, signatures, secrets, or unbounded paths (full metric table in
README §Observability, operational runbook in `docs/OPERATIONS.md`).
Health/readiness semantics: `docs/BUYER-OVERVIEW.md` §4 above and
`crates/core/src/obs/health.rs`, `metrics.rs`.

## 11. Staking program

Native Solana program in `programs/staking-suite` (borsh-encoded
instructions, PDAs `["staking-config"]` / `["staking-stake", staker]`,
custom errors `6000+`). Security model: full account validation, parameter
caps (`MAX_FEE_BPS` 10%, `MAX_REWARD_RATE_BPS` 100% APR), pause that can
never trap funds (withdrawals/claims never gated), two-step admin transfer,
OpenZeppelin-style parameter timelock (queue → wait → permissionless apply,
cancel; delay changes wait out the old delay), one-shot latched `GenesisMint`
(`genesis_done`; second attempt fails `GenesisAlreadyDone` 6026). Economics,
governance and the launch sequence: `docs/STAKING.md`.

**Status honesty:** the program compiles to BPF (5,440-byte `.so`, agave
2.1.21) and passed 2/2 validator end-to-end tests on a local
`solana-test-validator` — both **PREVIOUSLY VERIFIED** on identical source,
not re-executed in the final freeze sandbox. It has **no external security
audit**, and `declare_id!` is a **pre-deploy placeholder**. Do not deploy to
mainnet until an independent audit passes (README, `docs/SECURITY.md`).

## 12. Deployment model

Three supported shapes (`docs/DEPLOYMENT.md`): (a) Docker Compose stack —
bot + PostgreSQL 16 + Redis 7, healthcheck-gated, API published on loopback
by default; (b) bare metal — pinned Rust toolchain (`rust-toolchain.toml`,
1.98.1), external PG ≥ 16 and Redis 7; (c) multi-replica — same binary, N
processes, distributed ownership per §9. CI (`.github/workflows/ci.yml`)
covers fmt/clippy `-D warnings`/build/test with real service containers, the
staking program job (host tests, `build-sbf`, gated validator e2e), security
job (cargo-audit both lockfiles, cargo-deny incl. licenses), and a Docker
image build + container smoke test. Step-by-step buyer deployment:
`docs/BUYER-DEPLOYMENT.md`.

## 13. Security model

Threat model, key management and limitations: `docs/SECURITY.md`; reporting
policy: root `SECURITY.md`. Highlights: paper-by-default with two live gates;
secrets only from environment/secret store, never committed (release-check
secret scan gate); signer boundary — trading modules never touch key
material, signing goes through `TransactionSigner` + named `SignerRegistry`
(`crates/solana-kit/src/signer.rs`), multi-signer transactions require every
signer declared and resolvable; `[signing] provider` backends `vault`/`kms`/
`hsm` fail startup rather than silently falling back; API RBAC + Telegram
deny-by-default roles (owner/operator/readonly; `/mode live` owner-only);
append-only hash-chained audit trail with `GET /api/audit/verify` and
tamper-detection tests (modification/reorder/missing/duplicate, linear chain
under 8 concurrent appenders); Telegram error strings provably never contain
the bot token (regression test `error_strings_never_contain_the_bot_token`).

## 14. Testing model

Layered (`docs/TESTING.md`): deterministic offline unit/integration tests
with protocol mocks (PumpPortal WS, Geyser `transactionSubscribe`, mock
JSON-RPC pair for fan-out, Polymarket CLOB/Gamma HTTP, storage journal);
gated integration suites against real PostgreSQL + Redis
(`db_integration` 23, `redis_integration` 10, `distributed_integration` 4,
`two_replica_mirror` 1 — all executed in the final freeze pass, 38 gated
tests total); network-gated e2e (devnet, latency bench, validator e2e) that
skip cleanly without their env vars and never run silently. Final freeze
numbers: **521/521 workspace tests, 48 staking host tests + 2 gated e2e,
release-check 20/20 gates PASS, 0 failures**. Verification taxonomy
(VERIFIED / PREVIOUSLY VERIFIED / GATED / NOT EXECUTED): `docs/HANDOVER.md` §3.

## 15. Recovery model

Crash recovery is a first-class path, not an afterthought
(`docs/RECONCILIATION.md`, `crates/core/src/recovery.rs`): on startup the
intent journal and Postgres intents are replayed, unresolved intents are
reconciled against venue truth before new work begins, ambiguous outcomes
respect handoff grace, and dedup state (memory/Redis/Postgres) prevents
double execution across restarts. Backup/restore: `pg_dump` → restore → full
`db_integration` suite green on the restored database was **VERIFIED** in the
final pass; migrations are forward-only by design; Redis loss degrades
coordination only (`docs/BACKUP-RESTORE.md`).

---

Next documents in this package: `CAPABILITY-MATRIX.md` (per-capability
evidence), `BUYER-DUE-DILIGENCE.md` (verification checklist),
`DELIVERY-MANIFEST.md` (index of everything delivered).
`````


----- COMPLETE FILE: docs/CAPABILITY-MATRIX.md (9922 bytes, 58 lines) -----

`````markdown
# Capability matrix — sniper-suite 0.1.0

Every row maps to actual repository evidence. Status labels follow the
taxonomy in `docs/HANDOVER.md` §3:

- **VERIFIED** — executed successfully in the final freeze pass (release gate
  run on the frozen tree, commit `0e139c3`; results in `AUDIT.md` §26–27 and
  `docs/TESTING.md`).
- **PREVIOUSLY VERIFIED** — executed successfully in an earlier build session
  on identical source; not re-executed in the final freeze sandbox.
- **NOT EXECUTED** — never executed anywhere; wired into CI or requires
  external resources.

"Environment" says where the evidence was produced. No row claims more than
its evidence supports.

| Capability | Implemented | Evidence | Tested | Environment | Known limitation |
|---|---|---|---|---|---|
| Sniper (pump.fun launch detection + entry) | Yes | `crates/module-sniper/src/{detect,entry,exit}.rs`; feeds via `crates/solana-kit/src/{pumpportal,ws,cache}.rs` | VERIFIED — unit + `detect_feed`, `geyser_detect` mock suites (part of 521) | Host, mocked PumpPortal WS / Geyser / RPC | "~1s" launch-to-buy is a design target, not a guarantee; funded mainnet landing rate NOT EXECUTED (needs funded keys + approval) |
| Copy trading | Yes | `crates/module-copy/src/{feeds,mirror,exit}.rs` | VERIFIED — `copy_feed`, `geyser_feed`, `two_replica_mirror` 1/1 vs real PG+Redis | Host mocks + real PG/Redis sandbox | Mirrors only tracked wallets configured by operator; strategy quality is operator-owned |
| Polymarket (CLOB/Gamma, EIP-712 v2) | Yes | `crates/module-polymarket/src/{gamma,clob,eip712,orders,ctf,ws,auth,strategy}.rs` | VERIFIED — `mock_clob_gamma` incl. L1/L2 auth headers and signed order wire format (part of 521) | Host, mocked CLOB/Gamma HTTP | Live order placement against real Polymarket NOT EXECUTED (needs funded Polygon key); third-party API changes are an external risk |
| Telegram control | Yes | `crates/module-telegram/src/{commands,alerts,api}.rs` | VERIFIED — command/RBAC tests + token-redaction regression `error_strings_never_contain_the_bot_token` (part of 521) | Host, closed-port + mocked API | Real bot token round-trip requires buyer's BotFather token; long-polling only (no webhook mode) |
| Staking program (on-chain) | Yes | `programs/staking-suite/src/{lib,processor,state,instruction,error}.rs` | Host tests 48/48 VERIFIED; `build-sbf` (5,440-byte .so) + validator e2e 2/2 PREVIOUSLY VERIFIED (agave 2.1.21) | Host; local `solana-test-validator` (earlier session) | `declare_id!` is a pre-deploy placeholder; NO external audit; mainnet deployment blocked until audit passes |
| Execution engine (simulate-first, retry/failover) | Yes | `crates/solana-kit/src/{execute,tx,rpc}.rs` | VERIFIED — executor unit tests + `devnet_e2e` read-only PREVIOUSLY VERIFIED; `recon_crash_e2e` PREVIOUSLY VERIFIED vs local validator | Host mocks; public devnet (earlier) | Funded live broadcast landing-rate NOT EXECUTED; `latency_bench` PREVIOUSLY VERIFIED only |
| Risk engine (global, pre-trade) | Yes | `crates/core/src/risk.rs`; oracle in `crates/core/src/ownership.rs` + `docs/DISTRIBUTED.md` | VERIFIED — risk decision unit tests + `risk_rejected` event/metric paths (part of 521) | Host | Limits are configuration; the engine enforces, it does not advise |
| Persistence (PostgreSQL truth) | Yes | `crates/core/src/db/{repo,claims,mod}.rs`; `crates/core/migrations/0001`–`0011` | VERIFIED — `db_integration` 23/23 vs real PostgreSQL 16.4 (fresh + rerun + pg_dump→restore round-trip) | Real PG 16.4 sandbox | Migrations forward-only by design (no down migrations); PG ≥ 16 required |
| Redis (non-authoritative coordination) | Yes | `crates/core/src/{redis_kv,redis_ownership,dedup}.rs` | VERIFIED — `redis_integration` 10/10 vs real Redis 7.2.10 | Real Redis 7.2.10 sandbox | Redis may die without losing money-relevant truth, but claim coordination degrades to Postgres/memory paths |
| Reconciliation | Yes | `crates/core/src/reconciliation.rs`; `crates/server/src/recon.rs`; `docs/RECONCILIATION.md` | VERIFIED — reconciliation unit tests + db_integration OMS restart-recovery; `recon_crash_e2e` PREVIOUSLY VERIFIED | Host + real PG; local validator (earlier) | Venue-truth resolution depends on RPC/Polymarket availability at reconcile time |
| Distributed execution (claims/leases/fencing) | Yes | `crates/core/src/{ownership,redis_ownership}.rs`, `db/claims.rs`, migrations `0009`–`0011`; `docs/DISTRIBUTED.md` | VERIFIED — `distributed_integration` 4/4 + `two_replica_mirror` 1/1 (two real processes vs shared PG+Redis) | Real PG+Redis sandbox | Invariant is ≤1 money-moving submission per logical execution; split-brain beyond tested 2-replica topology NOT EXECUTED at scale |
| Observability (logs/metrics/probes) | Yes | `crates/core/src/obs/{health,metrics}.rs`; `crates/server/src/obs.rs`; README §Observability | VERIFIED — health/readiness/metrics/correlation-ID tests (part of 521); 11 documented metric names match source | Host | Metrics are Prometheus text exposition; no push gateway/OTLP exporter |
| Control-plane API (REST + WS + dashboard) | Yes | `crates/server/src/{api,ws,dashboard}.rs`; `docs/API.md` (28 endpoints) | VERIFIED — route/RBAC/rate-limit/WS tests (part of 521) | Host | Embedded dashboard is single-file HTML; no separate frontend build |
| RBAC / authorization | Yes | `crates/core/src/auth.rs`; Telegram roles in `crates/module-telegram/src/commands.rs` | VERIFIED — authz tests incl. readonly-cannot-mutate, operator≠owner, live-mode owner-only (part of 521) | Host | API key is a shared secret; per-user API identities are not implemented |
| Backup / restore | Yes (procedure) | `docs/BACKUP-RESTORE.md`; `pg_dump` path exercised by release-check | VERIFIED — pg_dump → restore → full db_integration suite green on restored DB | Real PG 16.4 sandbox | Covers Postgres + journal files; no automated scheduled-backup tooling included |
| CI pipeline | Yes | `.github/workflows/ci.yml` (4 jobs: app workspace, staking program incl. build-sbf + gated validator e2e, security audit/deny, docker build+smoke) | NOT EXECUTED — no GitHub runner in build environment; equivalent steps executed locally via `scripts/release-check.sh` (20/20 VERIFIED) | Local sandbox equivalent | Buyer must run CI on their own GitHub (or adapt the workflow) |
| Security scanning (audit/deny) | Yes | `deny.toml`; cargo-audit + cargo-deny gates in `scripts/release-check.sh` and CI | VERIFIED — cargo-audit both lockfiles 0 findings; cargo-deny advisories/bans/licenses/sources ok (cargo-audit 0.22.2, cargo-deny 0.18.9) | Local sandbox | RustSec DB is a snapshot in time; buyer must re-run periodically |
| Docker packaging | Yes | `Dockerfile` (multi-stage, non-root, healthcheck, rust:1.98.1-bookworm), `docker-compose.yml`, `.dockerignore` | NOT EXECUTED — no Docker daemon in sandbox; static inspection + `docker compose config` gate in CI only | CI (untested here) | Buyer must execute image build + smoke test on a real daemon |
| Signer abstraction / registry | Yes | `crates/solana-kit/src/signer.rs` (`TransactionSigner`, `SignerRegistry`, multi-signer enforcement) | VERIFIED — signer registry unit tests incl. missing-signer structured failure (part of 521) | Host | Only `local` custody backend implemented; `vault`/`kms`/`hsm` are config-level extension points that fail startup (no silent fallback) |
| Geyser (`transactionSubscribe`) feeds | Yes | `crates/solana-kit/src/events.rs` + module feeds; tests `geyser_detect`, `geyser_feed` | VERIFIED — mock Yellowstone-style WS incl. failed-tx skipping + poll fallback (part of 521) | Host mocks | Real Geyser provider (Yellowstone-compatible) is buyer-supplied; provider e2e NOT EXECUTED |
| Account cache (warm, bounded) | Yes | `crates/solana-kit/src/cache.rs` (TTL default 30 s, FIFO cap 5 000, `ACCOUNT_CACHE_TTL_MS`/`ACCOUNT_CACHE_MAX_ENTRIES`) | VERIFIED — cache unit tests (part of 521) | Host | Cache is best-effort; staleness bounds are configuration |
| RPC fan-out / failover | Yes | `crates/solana-kit/src/rpc.rs` (retry chokepoint, failover after 3 consecutive failures, `BROADCAST_FANOUT` race-send) | VERIFIED — mock JSON-RPC pair fan-out race test + rpc retry/failover unit tests (part of 521) | Host mocks | Production RPC provider quality/rate limits are buyer-side |
| Recovery (crash/restart) | Yes | `crates/core/src/recovery.rs`, `storage.rs` (JSONL journal, rotation, corrupt-line tolerance), `docs/RECONCILIATION.md` | VERIFIED — journal restart-fidelity/corrupt-line tests + db_integration OMS restart-recovery; `recon_crash_e2e` PREVIOUSLY VERIFIED | Host + real PG; local validator (earlier) | Recovery resolves recorded intents; activity by foreign wallets is out of scope |
| Audit chain (append-only, hash-chained) | Yes | `crates/core/src/audit.rs`; `db/repo.rs` (advisory-lock serialized append); `GET /api/audit/verify` | VERIFIED — tamper detection (modification/reorder/missing/duplicate) + linear chain under 8 concurrent appenders (db_integration) | Real PG 16.4 sandbox | Chain is app-level (hash-linked rows), not a blockchain; DB-superuser tampering plus rehash is out of the threat model (`docs/SECURITY.md`) |

## Test-count summary (final freeze pass, gate run on commit `0e139c3`)

| Suite | Result |
|---|---|
| Workspace (`cargo test --workspace -- --test-threads=1`) | 521 / 521 passed (incl. 38 gated integration tests executed) |
| `db_integration` | 23 / 23 |
| `redis_integration` | 10 / 10 |
| `distributed_integration` | 4 / 4 |
| `two_replica_mirror` | 1 / 1 |
| Staking host | 48 / 48 (+2 validator e2e gated-skipped in freeze sandbox) |
| `scripts/release-check.sh` | 20 PASS / 0 FAIL / 0 SKIP, exit 0 |
| fmt / clippy `-D warnings` / cargo-audit ×2 / cargo-deny | all clean |

Machine-readable copy: `release-manifest.json`. Historical evidence trail:
`AUDIT.md`. Per-suite detail: `docs/TESTING.md`.
`````


----- COMPLETE FILE: docs/BUYER-DUE-DILIGENCE.md (10648 bytes, 98 lines) -----

`````markdown
# Buyer due-diligence checklist

How a technical buyer can independently verify every material claim about
sniper-suite 0.1.0. Each item states what to check, where the evidence lives,
and what the last known result was. Nothing here requires trusting this
document: every check is reproducible from the source tree.

## A. Source

| Check | How | Last known result |
|---|---|---|
| Repository structure | `git ls-files` / directory listing: `crates/` (7 workspace crates), `programs/staking-suite` (standalone), `docs/`, `scripts/`, `.github/workflows/`, root metadata files | Matches layout in README §"Project layout" |
| File count | `git ls-files \| wc -l` | 146 tracked files at freeze commit `0e139c3` (160 after this buyer-package documentation pass adds 14 docs; no source files changed) |
| Source size | `git ls-files -z \| xargs -0 wc -c` (bytes) and `wc -l` (lines) | 2,801,590 bytes (2.80 MB), 77,980 lines at freeze; by category: production Rust (src/, incl. inline unit tests) 76 files / 1,826,288 B / 50,164 lines; test-dir Rust 15 / 225,648 / 6,476; SQL migrations 11 / 23,443 / 489; docs (Markdown) 17 / 293,952 / 4,265; config + deploy + lockfiles + scripts + manifest 27 / 432,259 / 16,586 |
| Language | All application code is Rust (91 `.rs` files); SQL migrations (11); shell (release-check); TOML/YAML config | No other production languages |
| Crates | Workspace members: `bot-core`, `solana-kit`, `module-sniper`, `module-copy`, `module-polymarket`, `module-telegram`, `sniper-suite` (server binary) | Root `Cargo.toml` `[workspace] members` |
| Standalone staking program | `programs/staking-suite` — own `Cargo.lock`, excluded from workspace, `crate-type = ["cdylib","lib"]`, release profile `overflow-checks=true, lto="fat", panic="abort"` | Native Solana program, no Anchor |
| No build artifacts / secrets in tree | `git status`, `.gitignore`, `.dockerignore`; release-check secret-scan + marker-scan gates | Clean; scans pass (`scripts/release-check.sh`) |

## B. Build

| Check | How | Last known result |
|---|---|---|
| Toolchain pin | `rust-toolchain.toml` (1.98.1 + rustfmt + clippy); Dockerfile base `rust:1.98.1-bookworm`; CI `dtolnay/rust-toolchain@1.98.1`; MSRV `rust-version = 1.82` (app) / `1.79` (program, for agave platform-tools resolver) | Three-way pin consistency is a release-check gate |
| Lockfiles | `Cargo.lock` (706 packages) and `programs/staking-suite/Cargo.lock` (580 packages), both committed | `cargo build --locked` reproducible dependency graph |
| Build from zero | Fresh machine: rustup → `cargo build --release`; full procedure in `docs/HANDOVER.md` §2 | VERIFIED — whole suite rebuilt and re-gated from source alone on a wiped machine (fresh rustup, PG 16.4 from official tarball, Redis 7.2.10 from source, empty target dir, empty database) |
| Reproducibility caveats | `docs/RELEASE.md` §reproducible-build analysis: no build timestamps embedded; `release-manifest.json` deliberately omits timestamps and its own commit hash | Bit-for-bit binary reproducibility NOT claimed (rustc/OS toolchain variance); source-level reproducibility is what is verified |
| On-chain build | `cd programs/staking-suite && cargo build-sbf` | PREVIOUSLY VERIFIED — 5,440-byte `staking_suite.so` with agave 2.1.21; NOT EXECUTED in final freeze sandbox (no Solana toolchain) |

## C. Testing

| Check | How | Last known result |
|---|---|---|
| Exact latest counts | `release-manifest.json` `test_counts`; `docs/TESTING.md`; `AUDIT.md` §26–27 | Workspace 521/521 (38 gated executed), db_integration 23/23, redis_integration 10/10, distributed_integration 4/4, two_replica_mirror 1/1, staking host 48/48, release-check 20 PASS / 0 FAIL / 0 SKIP, 0 failures anywhere |
| Reproduce yourself | `./scripts/release-check.sh` with `POSTGRES_URL` + `REDIS_URL` exported (procedure: `docs/HANDOVER.md` §2) | One command re-runs the entire gate |
| Historical external verification | PREVIOUSLY VERIFIED on identical source: `build-sbf`, validator e2e 2/2 (incl. funded stake→reward→unstake on local validator), `recon_crash_e2e`, devnet `devnet_e2e` read-only, `latency_bench`, deterministic ledger replay | Listed with labels in `docs/HANDOVER.md` §3 and `release-manifest.json` `verification_status` |
| Gated checks | Env-gated suites announce themselves and skip cleanly (`POSTGRES_URL`, `REDIS_URL`, `STAKING_E2E`, `E2E_NETWORK`, `E2E_LIVE`); rule: a skipped test must never look like a pass | `docs/TESTING.md` §Rules |
| Never-executed items | Docker build+smoke (no daemon), GitHub CI run (no runner), funded live trading, external audit | Explicitly NOT EXECUTED in `release-manifest.json`; also `docs/BUYER-RISK-REGISTER.md` |

## D. Security

| Check | How | Last known result |
|---|---|---|
| Authentication | API key (`x-api-key`) on mutating routes; server refuses non-loopback bind without auth; Telegram deny-by-default allow-lists | `docs/API.md`, `docs/SECURITY.md`; authz tests part of the 521 |
| Authorization / RBAC | Roles: API key holders; Telegram `owner` / `operator` / `readonly`; `/mode live` and key/journal mutations owner-only; readonly cannot mutate; operator ≠ owner | `crates/core/src/auth.rs`, `crates/module-telegram/src/commands.rs`; regression tests in suite |
| Signer boundary | Modules never touch key material; `TransactionSigner` + `SignerRegistry`; undeclared/unresolvable extra signer = structured build failure; `vault`/`kms`/`hsm` providers fail startup | `crates/solana-kit/src/signer.rs`; `docs/SECURITY.md` |
| Secret handling | Env-var indirection (`*_env` fields), no secrets in repo (release-check secret scan), redacted `/api/config`, bounded metric labels exclude secrets, Telegram token stripped from all error strings (`Error::without_url()` × 10 sites + regression test) | Freeze-pass defect found & fixed; see CHANGELOG "Fixed (engineering-freeze pass)" |
| Audit chain | Append-only from all app APIs; hash-chained; `GET /api/audit/verify`; advisory-lock serialized appends | Tamper detection + 8-appender concurrency tests VERIFIED vs real PG |
| External audit status | **None exists.** No external security audit, penetration test, or formal verification of any component. Staking program mainnet deployment is documentation-blocked until an independent audit passes | Root `SECURITY.md`, `docs/SECURITY.md`, `docs/STAKING.md` |

## E. Infrastructure

| Check | How | Last known result |
|---|---|---|
| PostgreSQL | Required ≥ 16 (verified on 16.4); 11 forward-only migrations embedded via sqlx; `auto_migrate` at startup | db_integration 23/23 + pg_dump→restore round-trip VERIFIED |
| Redis | Required 7.x (verified on 7.2.10); non-authoritative only | redis_integration 10/10 VERIFIED; Redis-loss behavior in `docs/BACKUP-RESTORE.md` |
| RPC / WebSocket | Solana JSON-RPC + WS configured via `RPC_URL`/`WS_URL`; retry/failover/fan-out in `crates/solana-kit/src/rpc.rs` | Mock-verified; production provider is buyer-supplied |
| Geyser | Yellowstone-compatible `transactionSubscribe` endpoint via `GEYSER_WS_URL`; poll fallback when absent | Mock-verified (`geyser_detect`, `geyser_feed`); real provider NOT EXECUTED |
| Docker | `Dockerfile` (multi-stage, non-root, healthcheck), `docker-compose.yml` (bot + postgres:16-alpine + redis:7-alpine, healthcheck-gated, loopback API publish) | Static inspection + CI `docker compose config` gate only; image build NOT EXECUTED (no daemon in sandbox) |
| CI | `.github/workflows/ci.yml`: app workspace (fmt/clippy `-D warnings`/build/test with PG16+Redis7 service containers), staking program (host tests + build-sbf + gated validator e2e), security (audit ×2 + deny incl. licenses), docker (build + container health smoke) | Workflow file delivered; a CI *run* requires the buyer's GitHub — NOT EXECUTED here; equivalent steps VERIFIED locally via release-check |

## F. Operations

| Check | How | Last known result |
|---|---|---|
| Startup / shutdown | Ordered module supervision, readiness gating, graceful shutdown | `docs/ARCHITECTURE.md`, `docs/OPERATIONS.md`; lifecycle tests in suite |
| Recovery | Intent journal replay + reconciliation before new work; handoff grace for ambiguous outcomes | `docs/RECONCILIATION.md`; restart-recovery tests VERIFIED; `recon_crash_e2e` PREVIOUSLY VERIFIED |
| Backup / restore | pg_dump/restore procedure + journal file handling; Redis is disposable | pg_dump→restore→full-suite-green VERIFIED; `docs/BACKUP-RESTORE.md` |
| Monitoring | `/health` (liveness), `/ready` (readiness), `/metrics` (Prometheus), request-ID correlated JSON logs | `docs/OPERATIONS.md` runbook; metric names verified to match source (11/11) |

## G. Ownership / IP

| Check | How | Last known result |
|---|---|---|
| License | MIT (`LICENSE`); cargo-deny license allow-list gates all dependencies (`deny.toml`) | Deny licenses gate VERIFIED clean |
| Copyright holder | `LICENSE` currently reads "sniper-suite authors" — **deliberate placeholder**; the legal entity must be inserted at transfer | BUYER/SELLER ACTION (`docs/HANDOVER.md` §5.1) |
| Repository URL | Workspace `Cargo.toml` intentionally has **no** `repository` URL (placeholder was removed rather than faked) | BUYER ACTION on publish |
| Security contact | Root `SECURITY.md` points at "the current repository owner's security contact" — no real address published | BUYER/SELLER ACTION |
| Staking program identity | `declare_id!("3vEEMMFmdA88n8ApgZ3b9L3BXEh75yCeMbHbmUjR9mfy")` is a **pre-deploy placeholder**; program not deployed to any cluster | BUYER ACTION (deploy under it or change id+keypair and rebuild) |
| Component provenance | Original application code vs third-party protocol integration, itemized | `docs/IP-COMPONENTS.md`, `docs/THIRD-PARTY.md` |

## H. Open external actions (complete list)

These are the only open items at handover. None is a software defect; all are
external/human actions (mirrored in `release-manifest.json`
`external_handover_blockers` and `docs/ACCEPTANCE-CHECKLIST.md`):

1. Insert the legal copyright holder into `LICENSE`.
2. Publish a real security contact (root `SECURITY.md`).
3. Set the real repository URL when published.
4. Deploy the staking program and finalize its program id.
5. Commission an independent external security audit before any mainnet
   deployment of the staking program.
6. Provide production infrastructure: PostgreSQL ≥ 16, Redis 7, RPC/WS
   (and optionally Geyser) providers, funded keys, secret store.
7. Execute Docker image build + CI on real runners.
8. Funded live-trading validation under operator supervision (paper mode is
   the default and nothing broadcasts without both live gates).
`````


----- COMPLETE FILE: docs/IP-COMPONENTS.md (10543 bytes, 207 lines) -----

`````markdown
# IP / component inventory

Technically significant implementation work in sniper-suite 0.1.0, with
provenance classification. Two categories are used honestly:

- **Original application code** — written for this project; copyright
  transfers with the repository (MIT, see `LICENSE`; holder placeholder is a
  documented handover action).
- **External protocol integration** — original code that *implements against*
  a third-party protocol/API/spec. The code is original; the protocol, its
  programs, APIs, and trademarks are **not** owned by this project and are not
  transferred.

No item below claims ownership of any third-party protocol, SDK, API, or
standard.

## 1. Solana instruction builders (pump.fun / PumpSwap / Raydium / Jupiter)

- **Location:** `crates/solana-kit/src/pump.rs`, `pumpswap.rs`, `raydium.rs`,
  `jupiter.rs`, `layout.rs`, `consts.rs`, `tokens.rs`
- **Purpose:** Hand-rolled account-layout parsing and instruction
  construction for pump.fun bonding-curve buy/sell (incl. v2 variants),
  PumpSwap AMM buy/sell, Raydium AMM `SwapBaseIn`/`SwapBaseInV2`, and Jupiter
  exit routing; SPL token / ATA handling.
- **Dependencies:** `solana-sdk`, `solana-program`, `spl-token`,
  `spl-associated-token-account`, `bincode`, `bs58`.
- **Classification:** Original application code implementing against external
  on-chain protocols (pump.fun, PumpSwap, Raydium, Jupiter). Those protocols,
  their deployed programs and IDs belong to their respective operators;
  on-chain program addresses are facts, not IP.
- **Third-party licensing:** Only via the crates listed (see
  `docs/THIRD-PARTY.md`). No protocol SDK is vendored.

## 2. Transaction decoder / swap decoding

- **Location:** `crates/solana-kit/src/decode.rs`
- **Purpose:** Decodes executed transactions (wire + parsed forms) into typed
  swap/buy/sell events used by sniper exits, copy mirroring, attribution and
  reconciliation; handles versioned transactions and failed-tx skipping.
- **Dependencies:** `solana-sdk`, `solana-transaction-status`,
  `solana-account-decoder`.
- **Classification:** Original application code.

## 3. Execution engine

- **Location:** `crates/solana-kit/src/execute.rs`, `tx.rs`, `rpc.rs`,
  `ws.rs`
- **Purpose:** Transaction assembly, blockhash management, simulate-first
  policy (`SIMULATE_FIRST` / `ABORT_ON_SIMULATION_FAILURE`), retry/failover
  RPC chokepoint with bounded consecutive-failure tracking, optional
  broadcast fan-out race across primary+fallback RPCs, WebSocket supervision
  with resubscribe.
- **Dependencies:** `solana-sdk`, `solana-client`, `reqwest`,
  `tokio-tungstenite`, `ed25519-dalek`.
- **Classification:** Original application code.

## 4. Signer abstraction / signer registry

- **Location:** `crates/solana-kit/src/signer.rs`
- **Purpose:** `TransactionSigner` trait + named `SignerRegistry`
  (`primary_trading` + configured identities); enforces that every required
  signer of a multi-signer transaction is declared and resolvable (structured
  failure otherwise); custody-backend selection (`local` implemented;
  `vault`/`kms`/`hsm` fail startup — no silent fallback). Key material never
  reaches trading modules.
- **Dependencies:** `solana-sdk`, `ed25519-dalek`, `async-trait`.
- **Classification:** Original application code.

## 5. Risk engine + global risk oracle

- **Location:** `crates/core/src/risk.rs`; cluster oracle in
  `crates/core/src/ownership.rs` and `docs/DISTRIBUTED.md`
- **Purpose:** Pre-trade global checks (capacity, exposure, daily-loss
  auto-disable), risk-rejection events/metrics, tighten-only cluster-wide
  limit propagation (`GlobalRiskOracle`).
- **Dependencies:** none beyond `bot-core` internals.
- **Classification:** Original application code.

## 6. OMS (order management + idempotency)

- **Location:** `crates/core/src/oms.rs`, `dedup.rs`, `models.rs`
- **Purpose:** Order state machine with idempotency keys; restart-safe dedup
  across memory / Redis / Postgres levels; ambiguity-aware status handling
  (e.g. simulate-mode outcomes never masquerade as confirmed).
- **Classification:** Original application code.

## 7. Reconciliation + recovery

- **Location:** `crates/core/src/reconciliation.rs`, `recovery.rs`;
  `crates/server/src/recon.rs`; migrations `0005`, `0006`, `0007`, `0008`
- **Purpose:** Intent journal (record-before-send), startup replay,
  venue-truth resolution of unresolved intents, transaction attribution for
  unknown on-chain transactions, handoff grace for ambiguous outcomes, PnL
  replay rules (`docs/RECONCILIATION.md`).
- **Classification:** Original application code.

## 8. Distributed ownership (claims / leases / epochs / fencing)

- **Location:** `crates/core/src/ownership.rs`, `redis_ownership.rs`,
  `db/claims.rs`; migrations `0009`, `0010`, `0011`; `docs/DISTRIBUTED.md`
- **Purpose:** The invariant *one logical execution ⇒ ≤1 active owner ⇒ ≤1
  money-moving submission*: claim stores (Postgres authoritative, Redis,
  memory), leases + epochs + fencing tokens, handoff grace, cross-replica
  kill-switch/module-flag sync, position-book sync, append-only
  `execution_claim_events` lineage.
- **Classification:** Original application code (the design pattern of
  leases/fencing is general distributed-systems practice; this is an
  independent implementation, not derived from a specific third-party
  codebase).

## 9. Audit chain

- **Location:** `crates/core/src/audit.rs`; append serialization in
  `crates/core/src/db/repo.rs`; migration `0004`
- **Purpose:** Append-only, hash-chained audit trail; app APIs can never
  mutate or delete entries; `GET /api/audit/verify` re-computes the chain;
  advisory-lock serialized appends keep the chain linear under concurrency.
- **Classification:** Original application code (hash-chain audit logs are a
  standard technique; implementation is independent).

## 10. Geyser integration + account cache

- **Location:** `crates/solana-kit/src/events.rs` (Yellowstone-style
  `transactionSubscribe` client), `cache.rs` (TTL + FIFO-bounded warm account
  cache), `pumpportal.rs` (PumpPortal WS client)
- **Purpose:** Push-based launch/trade detection with poll fallback; warm
  caching of semi-static accounts to cut RPC latency/load.
- **Classification:** Original application code implementing against the
  Yellowstone gRPC/WS `transactionSubscribe` convention and the PumpPortal
  API — both external services; neither is owned by this project. A
  compatible Geyser provider and (optionally) PumpPortal access are
  buyer-contracted external services.

## 11. Polymarket EIP-712 / CLOB implementation

- **Location:** `crates/module-polymarket/src/eip712.rs` (v2 11-field `Order`
  struct hash, domain `Polymarket CTF Exchange` v2, chainId 137, type-3
  signature wrapping), `orders.rs`, `clob.rs`, `gamma.rs`, `auth.rs` (L1/L2
  auth headers), `ctf.rs` (ERC-1155 balance reads), `ws.rs`, `strategy.rs`
- **Purpose:** Complete client-side implementation of Polymarket's CLOB
  order signing and trading APIs.
- **Dependencies:** `k256`, `tiny-keccak`, `hmac`, `sha2`, `reqwest`,
  `tokio-tungstenite`, `num-bigint`.
- **Classification:** Original application code implementing against
  Polymarket's published API and the EIP-712 **standard** (Ethereum
  improvement proposal — a public specification, not owned by anyone here).
  Polymarket's contracts, APIs, exchange addresses and brand belong to
  Polymarket; nothing here grants rights to operate on their venue beyond
  their own terms of service, which the buyer must satisfy.

## 12. Telegram control plane

- **Location:** `crates/module-telegram/src/` (commands, RBAC, alerts,
  token-redacted Bot API client)
- **Purpose:** Deny-by-default remote control (owner/operator/readonly),
  kill switch, module toggles, rate-limited alerts.
- **Classification:** Original application code implementing against the
  Telegram Bot API (external service; bot token and Telegram ToS are
  buyer-side).

## 13. Staking program (on-chain)

- **Location:** `programs/staking-suite/src/{lib,processor,state,instruction,error}.rs`
- **Purpose:** Native Solana program (no Anchor): reward mint (authority =
  config PDA), staking vault + fee treasury ATAs, deposit fee, per-second APY
  accrual, hard parameter caps, queue/apply/cancel timelock, two-step admin
  transfer, pause-deposits-only, one-shot latched `GenesisMint`.
- **Dependencies:** `solana-program` 2.1, `spl-token`, `spl-associated-token-account`,
  `borsh`, `thiserror`, `solana-system-interface`.
- **Classification:** Original application code. **Caveats that transfer with
  it:** not externally audited; declared program id is a pre-deploy
  placeholder; mainnet deployment is documentation-blocked pending an
  independent audit.

## 14. Persistence model

- **Location:** `crates/core/src/db/` (sqlx repositories), migrations
  `0001`–`0011`, `storage.rs` (JSONL journal), `redis_kv.rs`
- **Purpose:** PostgreSQL as durable financial truth (orders, executions,
  positions, trades, intents, claims, flags, audit), forward-only migration
  discipline, crash-tolerant local journal, Redis strictly non-authoritative.
- **Classification:** Original application code (schema, queries, journal
  format all project-specific).

## 15. Control plane, observability, testing infrastructure

- **Location:** `crates/server/src/` (API, WS feed, dashboard, probes,
  metrics, persist/recon pumps), `crates/core/src/obs/`, all `tests/`
  directories, `scripts/release-check.sh`, `.github/workflows/ci.yml`,
  `deny.toml`
- **Purpose:** 28-endpoint control plane with RBAC/rate limits/correlation
  IDs; bounded-label Prometheus metrics; protocol-mock test harnesses
  (PumpPortal WS, Geyser WS, JSON-RPC pair, CLOB/Gamma HTTP); one-command
  release gate; CI and supply-chain policy.
- **Classification:** Original application code (the embedded dashboard is a
  single original HTML file served by Axum; no frontend framework).

## Summary of what is **not** transferred / not owned

- pump.fun, PumpSwap, Raydium, Jupiter, Solana, PumpPortal, Yellowstone,
  Polymarket (contracts, APIs, addresses, brands), Telegram (Bot API),
  PostgreSQL, Redis, Docker — all third-party; this project integrates with
  them under their own terms.
- Rust crate dependencies — licensed per `Cargo.lock` + `deny.toml` policy
  (see `docs/THIRD-PARTY.md`); MIT/Apache-2.0-style licenses permit
  redistribution with notices, which the lockfile + deny tooling document.
- The EIP-712 and borsh formats are public standards/specifications.
`````


----- COMPLETE FILE: docs/THIRD-PARTY.md (6339 bytes, 125 lines) -----

`````markdown
# Third-party & license inventory

Provenance and licensing posture of everything sniper-suite 0.1.0 depends on.
The authoritative dependency record is the two committed lockfiles — this
document does not attempt (and does not need) to enumerate every transitive
crate by hand; the tooling below does that reproducibly.

## 1. Authoritative sources

| Artifact | Scope | Size |
|---|---|---|
| `Cargo.lock` | Application workspace (7 crates) — full resolved graph | 706 packages |
| `programs/staking-suite/Cargo.lock` | Staking program — independent lockfile (own workspace root, MSRV-aware resolution for the agave platform-tools compiler) | 580 packages |
| `deny.toml` | cargo-deny policy: advisories, bans, licenses, sources | — |
| `rust-toolchain.toml` | Pinned Rust 1.98.1 for both host projects | — |

Both lockfiles are committed, and `scripts/release-check.sh` +
`.github/workflows/ci.yml` (Security job) hard-gate them on every pass.

## 2. Major direct dependencies (application workspace)

Grouped by role; versions are pinned via `[workspace.dependencies]` in the
root `Cargo.toml` and resolved in `Cargo.lock`.

| Role | Crates | Notes |
|---|---|---|
| Async runtime / utils | `tokio`, `futures`, `futures-util`, `async-trait`, `once_cell` | |
| HTTP server / middleware | `axum`, `tower`, `tower-http` | Control plane |
| HTTP / WS clients | `reqwest`, `tokio-tungstenite`, `url` | RPC, Bot API, CLOB/Gamma, PumpPortal/Geyser WS |
| Serialization | `serde`, `serde_json`, `toml`, `bincode`, `bs58`, `hex`, `base64` | |
| Solana (host side) | `solana-sdk`, `solana-client`, `solana-program`, `solana-system-interface`, `solana-transaction-status`, `solana-account-decoder`, `spl-token`, `spl-associated-token-account` | Apache-2.0 (Anza / Solana Labs / SPL) |
| Persistence | `sqlx` (PostgreSQL), `redis` | Durable truth / coordination |
| Crypto / signing | `ed25519-dalek` (Solana keys), `k256` + `tiny-keccak` (secp256k1/keccak for EIP-712), `hmac`, `sha2` | RustCrypto ecosystem |
| Observability | `tracing`, `tracing-subscriber`, `chrono`, `uuid` | |
| Errors / misc | `anyhow`, `thiserror`, `dotenvy`, `rand`, `num-bigint` | |

Staking program (on-chain binary) direct deps are deliberately minimal:
`solana-program` 2.1, `spl-token` 6 (no-entrypoint), `spl-associated-token-account` 4
(no-entrypoint), `borsh` 1.5, `thiserror` 1, `solana-system-interface` 1.0;
dev-only (never compiled into the BPF object): `bincode`, `solana-sdk`,
`solana-client`.

The freeze pass removed every direct dependency without code references
(`tokio-util`, `sha3`, `serde_with`); they remain in `Cargo.lock` only where
still required transitively (CHANGELOG, "Fixed (engineering-freeze pass)").

## 3. Protocol / service dependencies (not libraries)

These are external services the software talks to; they are **not** bundled
and **not** owned by this project (details: `docs/IP-COMPONENTS.md` §"Summary"):

- Solana clusters (mainnet/devnet RPC + WS) — buyer-contracted providers.
- pump.fun / PumpSwap / Raydium / Jupiter on-chain programs — public
  deployed programs; addresses are configuration facts.
- PumpPortal WebSocket API and a Yellowstone-compatible Geyser provider —
  optional buyer-contracted feeds (poll fallback exists).
- Polymarket Gamma + CLOB APIs and Polygon-hosted CTF Exchange contracts —
  buyer must satisfy Polymarket's terms and any applicable law.
- Telegram Bot API — buyer supplies the bot token.

## 4. License posture

- **This project:** MIT (`LICENSE`). Copyright holder is a documented
  placeholder pending transfer (`docs/HANDOVER.md` §5.1).
- **Dependency policy (enforced, not aspirational):** `deny.toml`
  `[licenses]` allows only: MIT, MIT-0, Apache-2.0, Apache-2.0 WITH
  LLVM-exception, BSD-2-Clause, BSD-3-Clause, ISC, CC0-1.0,
  CDLA-Permissive-2.0, Zlib, MPL-2.0, BSL-1.0, Unicode-3.0, Unlicense
  (confidence threshold 0.8). Anything else fails the release.
- **Copyleft note:** MPL-2.0 is file-level copyleft and is on the allow-list;
  if a buyer's policy forbids it, `cargo deny check licenses` output identifies
  the affected crates. No GPL/AGPL crates are permitted by policy.
- `[bans] multiple-versions = "warn"` (duplicate versions surface but do not
  fail); `[sources]` denies unknown registries and **all** git dependencies —
  every crate comes from crates.io.
- **Last known result:** `cargo deny check` (advisories, bans, licenses,
  sources) passed clean on both projects in the final freeze gate
  (cargo-deny 0.18.9).

## 5. Advisory scanning

- `cargo audit` runs against **both** lockfiles (app + staking program) as a
  release-check gate and a CI hard gate. Last known result: **0 findings**
  (cargo-audit 0.22.2, RustSec DB snapshot at freeze time).
- The RustSec database is a point-in-time snapshot; advisories published
  after the freeze are not reflected. Buyers must re-run periodically.

## 6. SBOM status

- **NOT EXECUTED in the build environment** (`cargo cyclonedx` /
  `cargo spdx` not installed). The command to generate a CycloneDX SBOM is
  documented in `docs/RELEASE.md`.
- Both committed `Cargo.lock` files *are* the authoritative, complete,
  machine-readable dependency record (name + version + checksum for every
  crate); an SBOM generator merely reformats them.

## 7. How a buyer reproduces all of the above

```bash
# Toolchain (rustup honors rust-toolchain.toml → 1.98.1):
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal
source ~/.cargo/bin/env

# Advisory + policy tooling (versions used in the final gate):
cargo install cargo-audit --version 0.22.2 --locked
cargo install cargo-deny --version 0.18.9 --locked
# (or download the prebuilt musl binaries from the rustsec / cargo-deny
#  GitHub releases, as the freeze environment did)

# App workspace:
cargo audit            # 0 findings at freeze
cargo deny check       # advisories / bans / licenses / sources ok

# Staking program (separate lockfile):
cd programs/staking-suite
cargo audit
cargo deny check --config ../../deny.toml   # or copy deny.toml alongside

# Optional SBOM:
cargo install cargo-cyclonedx --locked
cargo cyclonedx --workspace --all > sbom.cyclonedx.json
```

`scripts/release-check.sh` runs the audit/deny gates for both projects
automatically as part of the 20-gate release validation.
`````


----- COMPLETE FILE: docs/BUYER-DEPLOYMENT.md (9160 bytes, 207 lines) -----

`````markdown
# Buyer deployment handover

The actual deployment procedure for sniper-suite 0.1.0, in the order it must
be performed. It combines `docs/DEPLOYMENT.md` (reference) and
`docs/HANDOVER.md` §2 (verify-from-zero) into a single buyer-facing sequence
with explicit safety gates. **The sequence ends in paper mode.** Moving to
live trading is a separate, deliberate decision gated at step 15 — do not
shortcut it.

Requirements: Linux x86-64, ~2 GB RAM, ~20 GB disk for a full verification
build (a release-only build needs less), network access.

## 1. Obtain the source

Receive the repository (git bundle/archive) from the seller. The delivered
tree is the frozen engineering state: release commit `9c677cd`, freeze commit
`0e139c3`, 146 tracked files (+ the 14 buyer-package documents added after
the freeze).

## 2. Verify the commit / tree integrity

```bash
git log --oneline -3          # expect 0e139c3 (freeze) on 9c677cd (release)
git status --short            # expect empty (clean tree)
git ls-files | wc -l          # expect 146 (+14 once buyer docs are committed)
```

Cross-check the version identity: `VERSION` = `0.1.0`,
`release-manifest.json` `version` = `0.1.0`, root `Cargo.toml`
`[workspace.package].version` = `0.1.0`. `scripts/release-check.sh` gates
this three-way consistency.

## 3. Install the pinned toolchain

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal
source ~/.cargo/bin/env
rustc --version               # rust-toolchain.toml forces 1.98.1 automatically
```

Do not substitute a different Rust version: 1.98.1 is the version the whole
verification record (521 tests, clippy `-D warnings`, audit/deny) was
produced with. For the staking program build you additionally need the Solana
CLI (`cargo build-sbf`, agave 2.1.21 generation) — only for step 15's
on-chain deployment, not for the bot.

## 4. Configure PostgreSQL

- PostgreSQL **≥ 16** (verified on 16.4; compose uses `postgres:16-alpine`).
- Create a dedicated database and role. Export:
  `POSTGRES_URL=postgres://user:pass@host:5432/db`.
- Migrations `0001`–`0011` are embedded in the binary and applied at startup
  when `auto_migrate` is on (`[storage]` in `config.toml.example`). They are
  forward-only by design — there are no down migrations
  (`docs/BACKUP-RESTORE.md`).
- Alternatively use the delivered compose stack (step 10) which provisions
  PG + Redis healthcheck-gated.

## 5. Configure Redis

- Redis **7.x** (verified on 7.2.10; compose uses `redis:7-alpine`).
- Export `REDIS_URL=redis://host:6379`.
- Redis is **non-authoritative** (dedup L2, coordination, cache). It may die
  without losing money-relevant truth, but configure persistence/monitoring
  anyway to avoid needless claim-coordination churn (`docs/BACKUP-RESTORE.md`).

## 6. Configure RPC / WS (and optionally Geyser)

- `RPC_URL` / `WS_URL` (or `[network]` in `config.toml`): a Solana JSON-RPC
  + WebSocket provider you contract for. The retry/failover chokepoint
  tolerates flaky providers; `BROADCAST_FANOUT=true` races sends across
  primary + fallback RPCs when you have more than one.
- Optional `GEYSER_WS_URL`: a Yellowstone-compatible `transactionSubscribe`
  endpoint for push-based launch/trade detection. Without it, the modules
  fall back to PumpPortal WS and polling — functionality is preserved,
  latency is not.

## 7. Configure secrets (outside the repository)

Secrets come from the environment (or your secret store injecting env vars);
config files store only the **names** of env vars (`*_env` fields). Never
commit secrets; the release-check secret-scan gate would fail the build.

| Secret | Used by |
|---|---|
| `SOLANA_KEYPAIR` (path / base58 / JSON array) | Solana execution (live/simulate only) |
| `POLYMARKET_PRIVATE_KEY` (or `POLYGON_PRIVATE_KEY`) | Polymarket CLOB order signing |
| `TELEGRAM_BOT_TOKEN` | Module 5 |
| `API_KEY` | Mutating REST routes |
| `POSTGRES_PASSWORD` etc. | compose stack (`.env` from `.env.template`) |

`.env` is loaded via `dotenvy` and is gitignored; in production prefer real
secret management over `.env` files.

## 8. Configure authentication / RBAC

- Set `API_KEY`; the server **refuses to bind a non-loopback address without
  API auth** — this is a hard startup gate, not a warning.
- Telegram: fill `[telegram]` `owner_user_ids` (full control incl.
  `/mode live`), `allowed_user_ids`/`allowed_chat_ids` (operators),
  `readonly_user_ids`. Authorization is deny-by-default: with empty
  allow-lists, no commands are accepted.
- Review the RBAC matrix in `docs/API.md` before exposing anything.

## 9. Run migrations + the verification gate

```bash
export POSTGRES_URL=... REDIS_URL=...
./scripts/release-check.sh    # 20 gates; last known result 20/0/0 on the frozen tree
```

This applies migrations (via the test suites), runs fmt/check/clippy
`-D warnings`, all 521 workspace tests incl. the 38 gated integration tests
against your real PG/Redis, staking host tests, cargo-audit ×2 and
cargo-deny. **If this does not pass on your machine, stop and resolve before
deploying** — it is the same gate the delivery evidence was produced with.

## 10. Start in paper mode

```bash
cp config.toml.example config.toml && $EDITOR config.toml
# [execution] mode = "paper"  (the default; Config::default disables all modules)
cargo build --release
CONFIG_PATH=./config.toml ./target/release/sniper-suite
# or the full stack:  cp .env.template .env && $EDITOR .env && docker compose up --build -d
```

Paper mode simulates fills against live market data with seeded balances
(10 SOL / 1000 USDC). Nothing is sent on-chain or to Polymarket. Enable
modules via `config.toml`, the API, or Telegram.

> Docker caveat: the image build + container smoke test were **NOT EXECUTED**
> in the delivery sandbox (no daemon) — they are wired into the CI `docker`
> job. Run `docker compose up --build` yourself and treat the first successful
> build + `/health` response as your verification of that path.

## 11. Verify health / readiness

```bash
curl -s localhost:8080/health | jq   # liveness: always 200 while HTTP is served
curl -s localhost:8080/ready  | jq   # readiness: 200 only when every component is ready, else 503 + report
```

`/ready` reports per-component (`rpc`, `sniper`, `copy`, `polymarket`)
health with detail strings that never contain error payloads, URLs or key
material. A degraded dependency must fail readiness, not liveness (so an
outage never gets the process restart-looped).

## 12. Verify metrics

```bash
curl -s localhost:8080/metrics | head -40
```

Expect `bot_*` series (build info, mode, kill switch, module counters, RPC
retry/failover counters, execution latency histograms, HTTP request
metrics). Names/labels are enumerated in README §Observability and must match
`docs/OPERATIONS.md`. Point Prometheus at the endpoint (scrape example in
README).

## 13. Verify Telegram

With `TELEGRAM_BOT_TOKEN` set and your chat/user IDs allow-listed: `/status`,
`/positions`, `/pnl` (readonly), then `/on`/`/off`/`/kill`/`/resume`
(operator/owner). An unauthorized request must produce an explicit refusal —
never a silent no-op. Confirm error messages/alerts never contain the bot
token (guaranteed by the redaction regression test, but verify live output
once).

## 14. Verify recovery

Prove the crash path before trusting it with money:

1. Start in paper mode, let it record intents/positions (journal + PG).
2. `kill -9` the process; restart it.
3. Check startup reconciliation ran (logs), no duplicate executions occurred
   (dedup + claims), `/api/audit/verify` still reports a valid chain, and
   `/ready` returns to 200.

Procedures and expected behavior: `docs/RECONCILIATION.md`,
`docs/BACKUP-RESTORE.md` (incl. the pg_dump → restore drill you should also
perform once on your own infrastructure).

## 15. Only then consider live mode — safety gates

Live trading requires **all** of the following, deliberately:

1. Everything above passed on your infrastructure, in paper (and optionally
   `simulate` — which builds and RPC-simulates real transactions without
   sending).
2. `[execution] mode = "live"` **and** `allow_live_trading = true` (two
   independent gates; with the second off, live requests are downgraded and
   never broadcast). At runtime, `/mode live` (REST or Telegram) is
   owner-only.
3. Real key material loaded through the signer registry; risk limits sized
   for the capital you are actually willing to lose; daily-loss auto-disable
   configured.
4. Funded live-trading validation performed **gradually, under operator
   supervision** — this was never executed in the delivery environment
   (documented NOT EXECUTED item) and is your responsibility.
5. For the staking program specifically: program deployed under a finalized
   id **and an independent external security audit passed** — until then,
   mainnet deployment is blocked by documentation and should be blocked by
   your process (`docs/STAKING.md`, `docs/SECURITY.md`).

This document does not encourage live deployment; it documents the gates that
make an unsafe live deployment require multiple deliberate acts.
`````


----- COMPLETE FILE: docs/ACCEPTANCE-CHECKLIST.md (7354 bytes, 133 lines) -----

`````markdown
# Handover acceptance checklist

The buyer signs off the handover by working through this list. Each item is
marked with its status at delivery:

- **[VERIFIED]** — evidence exists in the repository and was executed in the
  final freeze pass (or is a delivered fact); the buyer can re-verify with the
  stated command/procedure.
- **[PREVIOUSLY VERIFIED]** — executed on identical source in an earlier
  session; not re-executed in the final freeze sandbox.
- **[BUYER ACTION]** — must be completed by the buyer (or jointly with the
  seller) after transfer. Not a software defect.
- **[EXTERNAL]** — requires external infrastructure/services.

Statuses mirror `release-manifest.json` and `docs/HANDOVER.md` §3/§5.

## Source & identity

- [ ] **[VERIFIED]** Source received — 146 tracked files, 2.80 MB / 77,980
      lines at freeze; layout matches README §"Project layout"
      (re-check: `git ls-files | wc -l`, size table in
      `docs/BUYER-DUE-DILIGENCE.md` §A).
- [ ] **[BUYER ACTION]** Commit verified on receipt — `git log` shows freeze
      commit `0e139c3` on release commit `9c677cd`, `git status --short`
      empty (buyer-side step 2 of `docs/BUYER-DEPLOYMENT.md`).
- [ ] **[BUYER ACTION]** License assigned — insert the legal copyright holder
      into `LICENSE` (currently the deliberate placeholder "sniper-suite
      authors"; `docs/HANDOVER.md` §5.1).
- [ ] **[BUYER ACTION]** Repository URL assigned — set `repository` in the
      workspace `Cargo.toml` when publishing (intentionally absent; no fake
      URL ships).
- [ ] **[BUYER ACTION]** Security contact assigned — publish a real address
      in root `SECURITY.md` (currently points at "the repository owner's
      contact").
- [ ] **[BUYER ACTION]** Staking program ID finalized — deploy
      `programs/staking-suite` under the declared placeholder id (or change
      id + keypair and rebuild) and set `[contract] program_id`
      (`docs/STAKING.md`).

## Secrets & configuration

- [ ] **[VERIFIED]** No secrets in the delivered tree — release-check
      secret-scan gate passed; `.gitignore`/`.dockerignore` enforce; secrets
      are env-var indirections only (`docs/SECURITY.md`).
- [ ] **[BUYER ACTION]** Secrets configured outside the repo — keypair(s),
      Polymarket key, Telegram token, `API_KEY`, DB password
      (`docs/BUYER-DEPLOYMENT.md` §7).

## Infrastructure

- [ ] **[EXTERNAL]** PostgreSQL ≥ 16 configured (verified against 16.4;
      compose template delivered).
- [ ] **[EXTERNAL]** Redis 7 configured (verified against 7.2.10; compose
      template delivered).
- [ ] **[EXTERNAL]** RPC provider configured (`RPC_URL`/`WS_URL`; retry,
      failover and fan-out implemented and mock-verified).
- [ ] **[EXTERNAL]** WS/Geyser provider configured (optional
      `GEYSER_WS_URL`; poll fallback exists; real-provider e2e not executed
      in delivery).

## Functional acceptance (re-run by buyer per docs/BUYER-DEPLOYMENT.md)

- [ ] **[VERIFIED at delivery / BUYER RE-RUN]** Full local gate passed on the
      frozen tree: `./scripts/release-check.sh` → 20 PASS / 0 FAIL / 0 SKIP
      (521/521 workspace, db 23/23, redis 10/10, distributed 4/4,
      two-replica 1/1, staking 48/48 host, fmt/clippy/audit/deny clean).
- [ ] **[BUYER ACTION]** Paper test completed on buyer infrastructure
      (modules enabled, simulated fills observed, dashboard live).
- [ ] **[BUYER ACTION]** Simulate test completed (`EXECUTION_MODE=simulate`:
      real transactions built + RPC-simulated, nothing broadcast).
- [ ] **[BUYER ACTION]** Health verified (`GET /health` → 200 liveness).
- [ ] **[BUYER ACTION]** Readiness verified (`GET /ready` → 200, and 503 +
      component report when a dependency is down).
- [ ] **[BUYER ACTION]** Metrics verified (`GET /metrics` → `bot_*` series
      matching README §Observability / `docs/OPERATIONS.md`).
- [ ] **[VERIFIED at delivery / BUYER RE-RUN]** Backup verified — pg_dump
      procedure documented (`docs/BACKUP-RESTORE.md`); dump → restore → full
      db_integration suite green on the restored database was executed in the
      final freeze pass.
- [ ] **[VERIFIED at delivery / BUYER RE-RUN]** Restore verified — same
      round-trip as above; buyer should repeat once on their own PG instance.
- [ ] **[VERIFIED at delivery / BUYER RE-RUN]** Audit verification verified —
      `GET /api/audit/verify` recomputes the hash chain; tamper detection
      (modification/reorder/missing/duplicate) + 8-concurrent-appender
      linearity tested against real PG.
- [ ] **[VERIFIED at delivery / BUYER RE-RUN]** Recovery verified — restart
      replay + reconciliation + dedup tested (`db_integration`,
      `storage_lifecycle`); `recon_crash_e2e` against a local validator is
      PREVIOUSLY VERIFIED; buyer drill: `docs/BUYER-DEPLOYMENT.md` §14.
- [ ] **[VERIFIED at delivery / BUYER RE-RUN]** RBAC verified — API-key
      gating, non-loopback-bind refusal without auth, Telegram
      owner/operator/readonly roles, live-mode owner-only, readonly-cannot-
      mutate regression tests (part of the 521).
- [ ] **[BUYER ACTION]** Telegram verified live — real bot token, allow-lists
      populated, refusals observed for unauthorized ids
      (`docs/BUYER-DEPLOYMENT.md` §13).

## On-chain program

- [ ] **[PREVIOUSLY VERIFIED]** Staking program compiles to BPF
      (`cargo build-sbf`, 5,440-byte `.so`, agave 2.1.21) and passed 2/2
      validator e2e (stake lifecycle, timelock, two-step admin transfer,
      genesis-mint latch) on identical source in an earlier session; CI
      `program` job re-runs both on every push once CI is active.
- [ ] **[BUYER ACTION]** External audit completed — **no external security
      audit exists**; mandatory before any mainnet deployment
      (README, `docs/SECURITY.md`, `docs/STAKING.md`).
- [ ] **[BUYER ACTION]** Genesis/admin policy set — multisig admin (Squads/
      Realms PDA recommended), timelock ≥ 24h for production, one-shot
      `GenesisMint` recipient decided (`docs/STAKING.md`).

## Operations & sign-off

- [ ] **[EXTERNAL]** Docker build + smoke executed on a real daemon (NOT
      EXECUTED in delivery sandbox; CI `docker` job covers it; compose file
      passed the `docker compose config` syntax gate statically).
- [ ] **[EXTERNAL]** CI executed on buyer's GitHub — `.github/workflows/ci.yml`
      delivered (4 jobs); no run exists from the delivery environment.
- [ ] **[EXTERNAL]** Monitoring/alerting wired — Prometheus scrape +
      `docs/OPERATIONS.md` runbook adopted.
- [ ] **[BUYER ACTION]** Production sign-off — funded live-trading validation
      under operator supervision, gradual size ramp, kill-switch drill;
      live mode requires both gates (`mode = "live"` +
      `allow_live_trading = true`) and owner-only runtime switching.

## Joint transfer actions

- [ ] **[BUYER ACTION]** Ownership transfer executed — repository, domain(s),
      provider accounts (RPC/Geyser/PumpPortal/Polymarket/Telegram bot) moved
      or re-contracted in the buyer's name (`docs/SUPPORT-HANDOVER.md`).
- [ ] **[BUYER ACTION]** Credential rotation completed — every credential
      that ever existed on either side (keys, tokens, DB passwords, API keys)
      rotated at transfer (`docs/SUPPORT-HANDOVER.md` §"Credential rotation").
`````


----- COMPLETE FILE: docs/RELEASE-NOTES-0.1.0.md (6011 bytes, 107 lines) -----

`````markdown
# Release notes — sniper-suite 0.1.0 (buyer edition)

Factual release summary for the receiving party. The full engineering history
is in `CHANGELOG.md`; the evidence trail is in `AUDIT.md`; this document does
not duplicate either — it states what the release *is*, what was proven, and
what remains open.

## Release identity

| Field | Value |
|---|---|
| Product | sniper-suite — modular crypto trading system (5 modules + control plane) |
| Version | `0.1.0` (initial handover release; `VERSION`, root `Cargo.toml`, `release-manifest.json` agree — gated by `scripts/release-check.sh`) |
| License | MIT (`LICENSE`; copyright holder is a documented handover placeholder) |
| Release commit | `9c677cd` |
| Engineering-freeze commit | `0e139c3` (the frozen tree this package describes) |
| Tree at freeze | 146 tracked files, 2,801,590 bytes (2.80 MB), 77,980 lines |
| Toolchain | Rust 1.98.1 pinned (`rust-toolchain.toml`, Dockerfile, CI); MSRV 1.82 (app) / 1.79 (staking program); agave 2.1.21 for `build-sbf` |

## Test results (final freeze gate, executed on commit `0e139c3`)

| Suite | Result |
|---|---|
| `scripts/release-check.sh` | **20 PASS / 0 FAIL / 0 SKIP**, exit 0 |
| Workspace tests (`--test-threads=1`) | **521 / 521** passed (incl. 38 gated integration tests executed against real services) |
| `db_integration` (PostgreSQL 16.4) | 23 / 23 (fresh + rerun + pg_dump→restore→suite-green round-trip) |
| `redis_integration` (Redis 7.2.10) | 10 / 10 |
| `distributed_integration` | 4 / 4 |
| `two_replica_mirror` (two real processes) | 1 / 1 |
| Staking host tests | 48 / 48 (+2 validator e2e gated-skipped in the freeze sandbox) |
| fmt / `clippy -D warnings` (both projects) | clean |
| `cargo audit` (both lockfiles) | 0 findings |
| `cargo deny` (advisories/bans/licenses/sources) | ok |
| Total test executions across the gate script | 609, 0 failures |

Machine-readable copy: `release-manifest.json` → `test_counts`.

## Major components delivered

- **Modules:** sniper (pump.fun launch detection + PumpSwap/Raydium/Jupiter
  exits), copy trading, Polymarket (Gamma/CLOB, EIP-712 v2 signing), native
  Solana staking program (timelock, caps, two-step admin, latched genesis
  mint), Telegram control (deny-by-default RBAC).
- **Control plane:** Axum REST (23 endpoints over 21 `/api` routes) +
  WebSocket feed + 4 infra routes (28 documented in `docs/API.md`), embedded
  dashboard, liveness/readiness probes, bounded-label Prometheus metrics,
  request-ID correlation, rate limits, non-loopback-bind refusal without auth.
- **Core guarantees:** global pre-trade risk engine, OMS with idempotency,
  three-level restart-safe dedup, intent journal + startup reconciliation,
  hash-chained append-only audit trail, distributed execution ownership
  (claims/leases/epochs/fencing, tighten-only `GlobalRiskOracle`), PostgreSQL
  as durable truth with 11 forward-only migrations, Redis strictly
  non-authoritative.
- **Packaging:** Dockerfile (multi-stage, non-root, healthchecked) +
  compose stack, CI workflow (4 jobs), `deny.toml`, `release-check.sh`,
  `release-manifest.json`, 13 engineering docs + 14 buyer-package docs.

## Major security fixes (made before the release cut)

1. **Telegram bot-token leak into error strings** (found and fixed in the
   engineering-freeze pass): the Bot API embeds the token in request URLs and
   `reqwest::Error`'s `Display` appends ` for url (…)`, so failed Telegram
   calls could put the token into logs/audit/alert text. All 10 error-mapping
   sites now strip the URL (`Error::without_url()`); regression test
   `error_strings_never_contain_the_bot_token` fails if the token ever
   reappears in an error string.
2. **Audit-chain append serialization** (release-engineering pass): concurrent
   appends could fork the hash chain under READ COMMITTED; appends are now
   serialized by a transaction-scoped advisory lock, with regression tests
   for concurrent linearization and reordered/missing/duplicate detection.

## Major engineering fixes (made before the release cut)

- Toolchain-pin drift removed (Dockerfile `rust:1.82` → `rust:1.98.1-bookworm`,
  CI program job `stable` → pinned 1.98.1); three-way pin consistency is now a
  release gate.
- Unused direct dependencies removed after proof of zero code references
  (`tokio-util`, `sha3`, `serde_with`); full gate re-run afterwards.
- Placeholder `repository` URL removed rather than faked; stale test counts
  and route-count documentation corrected; `release-manifest.json` introduced
  and wired into the gate.
- Full source-freeze audit (21 items) passed: no debug hacks, no dead code
  beyond one justified `allow(dead_code)`, no production `unwrap()`, no
  secret leakage paths, docs matched to source (details: `AUDIT.md` §26–27).

## Known limitations & external blockers at release

- **No external security audit** of any component; the staking program's
  mainnet deployment is documentation-blocked until one passes.
- Staking `declare_id!` is a **pre-deploy placeholder**; the program is not
  deployed to any cluster.
- **NOT EXECUTED (environment-blocked, wired into CI or requiring external
  resources):** Docker image build + container smoke (no daemon in the build
  sandbox), GitHub Actions CI run (no runner), funded live-trading /
  mainnet landing-rate validation, SBOM tool run (lockfiles are the
  authoritative dependency record), `build-sbf`/validator-e2e re-execution in
  the final sandbox (PREVIOUSLY VERIFIED on identical source).
- Legal/identity fill-ins: LICENSE copyright holder, repository URL, security
  contact (`docs/HANDOVER.md` §5, `docs/ACCEPTANCE-CHECKLIST.md`).

## What is explicitly NOT claimed

No market-value or sale-price claim; no "guaranteed 1-second execution"
claim (the ~1s sniper figure is a design target measured only by the
PREVIOUSLY VERIFIED local `latency_bench`); no live-trading profit claim
(funded live operation was never executed); no external-audit claim; no
customer/reference claim.
`````


----- COMPLETE FILE: docs/BUYER-FAQ.md (8652 bytes, 154 lines) -----

`````markdown
# Technical FAQ (buyer edition)

Every answer cites the source or document that backs it. If an answer says
"not executed" or "previously verified", that is the honest status per the
taxonomy in `docs/HANDOVER.md` §3.

**Is this paper-trading safe by default?**
Yes. The execution mode defaults to `paper` and `Config::default()` disables
every module, so a freshly started instance trades nothing at all until
modules are explicitly enabled. In paper mode fills are simulated against
live market data with seeded balances (10 SOL / 1000 USDC); nothing is sent
on-chain or to Polymarket (README §Quick start, `crates/core/src/config.rs`).

**How does live trading get enabled?**
Two independent configuration gates must both be true —
`[execution] mode = "live"` **and** `allow_live_trading = true` — plus real
key material for the relevant venue (`SOLANA_KEYPAIR`,
`POLYMARKET_PRIVATE_KEY`). With `allow_live_trading = false`, live requests
are downgraded and never broadcast. At runtime, switching to live
(`POST /api/mode`, Telegram `/mode live`) is owner-only. An intermediate
`simulate` mode builds and RPC-simulates real transactions without sending
them (README §"Going live", authz tests in the 521-test suite).

**Can one replica execute the same intent twice?**
Not under the tested model. The invariant is *one logical execution ⇒ at most
one active owner ⇒ at most one money-moving submission*, enforced by claim
stores (Postgres authoritative), leases + epochs + fencing tokens, and
three-level dedup (memory/Redis/Postgres) with deterministic idempotency keys.
This is tested by `distributed_integration` (4/4) and `two_replica_mirror`
(1/1 — two real processes against shared PG+Redis) in the final freeze pass.
Ambiguous outcomes (e.g. a broadcast whose result is unknown) enter a
handoff-grace path instead of blind retry (`docs/DISTRIBUTED.md`,
`docs/RECONCILIATION.md`). Scale beyond the tested 2-replica topology is
untested — the mechanism is designed for N replicas, but only 2 were
exercised.

**What happens after a crash?**
On restart: the JSONL intent journal and Postgres intents are replayed,
unresolved intents are reconciled against venue truth (Solana transaction
status / Polymarket order state) **before** new work begins, dedup state
prevents re-execution, and ambiguous outcomes respect handoff grace.
Corrupt journal lines are tolerated (skipped + logged, not fatal). Verified
by `storage_lifecycle`, `db_integration` restart-recovery tests, and
(PREVIOUSLY VERIFIED) `recon_crash_e2e` against a local validator
(`crates/core/src/recovery.rs`, `docs/RECONCILIATION.md`).

**Where is financial state stored?**
PostgreSQL is the durable source of truth: orders, executions, positions,
trades, intent journal, execution claims, runtime flags, and the audit chain
(11 forward-only migrations). Redis is explicitly non-authoritative
(coordination, dedup L2, cache). The local JSONL journal is a crash-recovery
aid, not the truth (`docs/BACKUP-RESTORE.md`, `docs/RECONCILIATION.md`).

**What happens if Redis dies?**
Money-relevant truth is unaffected — Redis holds no authoritative state.
Dedup falls back to the Postgres/memory levels and claim coordination falls
back to the Postgres claim store; readiness reports degradation. This is a
design guarantee documented in `docs/BACKUP-RESTORE.md` and exercised by the
gated integration suites.

**What happens if RPC dies?**
The RPC chokepoint retries, tracks consecutive failures, and fails over to
fallback endpoints (3 consecutive failures mark the `rpc` component
unhealthy in `/ready`; `BROADCAST_FANOUT` can race sends across providers).
Modules degrade visibly (readiness 503, `bot_rpc_requests_total{outcome=
"fatal"|"exhausted"}`) rather than silently guessing. Recovery of in-flight
intents goes through reconciliation, not re-broadcast
(`crates/solana-kit/src/rpc.rs`, README §Observability).

**How are unknown transactions handled?**
Transaction attribution (migration `0006`) matches observed on-chain
transactions back to recorded intents via signatures/ids; anything that
cannot be attributed is surfaced for reconciliation rather than acted on
automatically. Reconciliation resolves recorded intents against venue truth;
foreign wallet activity is out of scope (`docs/RECONCILIATION.md`).

**What happens if a Telegram request is unauthorized?**
Explicit refusal, never a silent no-op. Authorization is deny-by-default:
with empty allow-lists no commands are accepted; `readonly` ids get read
commands only and refusals on mutations; operators cannot do owner-only
actions (live-mode switch, key/journal mutations). Unknown chat/user ids are
rejected and can trigger alerts (`crates/module-telegram/src/commands.rs`,
RBAC tests in the suite).

**How are secrets protected?**
Secrets live only in the environment (config stores env-var *names*,
`*_env` fields); the repo is secret-scanned at every release gate;
`/api/config` is redacted; metric labels are bounded and never contain
wallets/signatures/secrets; readiness detail strings never contain error
payloads or URLs; and every Telegram API error path strips the request URL so
the bot token cannot leak into error strings (regression-tested). Signing
keys never reach trading modules — they sit behind the `TransactionSigner` /
`SignerRegistry` boundary (`docs/SECURITY.md`, CHANGELOG freeze-pass fix).

**Is the staking contract externally audited?**
**No.** No external security audit, penetration test, or formal verification
exists for any component. The staking program is host-tested (48/48),
PREVIOUSLY VERIFIED end-to-end on a local validator (2/2, incl. funded
stake→reward→unstake), and compiles to BPF — but mainnet deployment is
documentation-blocked until an independent audit passes (root `SECURITY.md`,
`docs/STAKING.md`).

**Is Docker verified?**
Partially. The `Dockerfile` (multi-stage, non-root, healthchecked, pinned
`rust:1.98.1-bookworm`) and `docker-compose.yml` are delivered and passed
static inspection; `docker compose config` is a CI syntax gate. The image
build and container smoke test were **NOT EXECUTED** in the delivery sandbox
(no Docker daemon) — they run in the CI `docker` job, which itself has not
run on a real GitHub runner yet. The buyer must execute this path once
(`docs/BUYER-RISK-REGISTER.md`).

**What is "previously verified" vs "latest verified"?**
VERIFIED = executed successfully in the final freeze pass on the frozen tree
(521/521, gate 20/20, real PG 16.4 + Redis 7.2.10). PREVIOUSLY VERIFIED =
executed successfully in an earlier build session **on identical source** but
not re-executed in the final sandbox (build-sbf, validator e2e,
recon_crash_e2e, devnet e2e, latency bench). Both labels are listed
machine-readably in `release-manifest.json` → `verification_status`
(`docs/HANDOVER.md` §3).

**Can this be extended with another DEX?**
Yes, mechanically: exit routing already spans three venues
(PumpSwap/Raydium/Jupiter) behind the executor, and instruction builders are
per-venue modules in `crates/solana-kit/src/` (`pump.rs`, `pumpswap.rs`,
`raydium.rs`, `jupiter.rs`). A new DEX = a new builder module + routing
config; the money-path invariants (risk → claim → journal → execute →
reconcile) are venue-agnostic. No such extension is included or claimed.

**Can another execution venue be added?**
Same pattern as Polymarket: `module-polymarket` is a self-contained crate
talking to an external venue (REST+WS+EIP-712 signing) registered with the
same core (events, risk, OMS, persistence). A new venue module would follow
that shape. Not included; this is an architecture fact, not a roadmap
promise.

**Is the system multi-replica?**
Yes — designed and tested for it: claims/leases/epochs/fencing, cross-replica
kill-switch and module-flag sync, position-book sync, cluster-wide
tighten-only `GlobalRiskOracle`, and the `execution_claim_events` lineage
table. Tested with two real replica processes (`two_replica_mirror`,
`distributed_integration`); larger topologies are untested
(`docs/DISTRIBUTED.md`).

**Is it multi-tenant?**
No. One deployment = one operator/owner with one config, one key set, one
set of modules. Authorization separates *roles* (owner/operator/readonly),
not tenants. Multi-tenancy would mean multiple deployments.

**What is still buyer-owned infrastructure?**
Everything external: production PostgreSQL and Redis, RPC/WS (and optional
Geyser/PumpPortal) providers, Polymarket access and Polygon key, Telegram
bot token, hosting/Docker/CI runners, monitoring stack, secret store,
domains/reverse proxy, funded trading keys — itemized in
`docs/SCOPE-BOUNDARY.md`.
`````


----- COMPLETE FILE: docs/SCOPE-BOUNDARY.md (5393 bytes, 99 lines) -----

`````markdown
# Commercial scope boundary

What transfers with sniper-suite 0.1.0, and what does not. Four categories;
every item is factual. This document exists so that neither party has to
guess where the software ends.

## 1. DELIVERED SOFTWARE (transfers with the repository)

- **Rust application source** — 7-crate workspace: `bot-core`, `solana-kit`,
  `module-sniper`, `module-copy`, `module-polymarket`, `module-telegram`,
  `sniper-suite` server binary (`crates/`).
- **Staking program source** — `programs/staking-suite` (native Solana
  program, own lockfile; BPF build PREVIOUSLY VERIFIED, not deployed).
- **Tests** — 521 workspace tests (incl. 38 gated integration tests),
  48 + 2 staking host/e2e tests, all mock harnesses (`tests/` dirs across
  crates).
- **Database migrations** — 11 forward-only PostgreSQL migrations
  (`crates/core/migrations/`).
- **Deployment configuration** — `Dockerfile`, `docker-compose.yml`,
  `.dockerignore`, `.env.template`, `config.toml.example`,
  `rust-toolchain.toml`, `deny.toml`, `.gitignore`.
- **CI** — `.github/workflows/ci.yml` (4 jobs; requires the buyer's runner to
  execute).
- **Release tooling** — `scripts/release-check.sh` (20-gate local release
  validation), `release-manifest.json`.
- **Documentation** — 13 engineering docs (`docs/`: ARCHITECTURE, API,
  SECURITY, DEPLOYMENT, OPERATIONS, MODULES, STAKING, TESTING,
  RECONCILIATION, DISTRIBUTED, RELEASE, HANDOVER, BACKUP-RESTORE) + 14
  buyer-package docs (this set; index in `docs/DELIVERY-MANIFEST.md`) +
  README, CHANGELOG, AUDIT.md (historical evidence trail), root SECURITY.md,
  LICENSE (MIT, holder placeholder), VERSION.

**Not delivered inside this category:** no compiled binaries, no Docker
images, no deployed on-chain program, no populated databases — the buyer
builds everything from source (procedure: `docs/BUYER-DEPLOYMENT.md`).

## 2. BUYER-PROVIDED INFRASTRUCTURE (buyer must supply & operate)

- Production **PostgreSQL ≥ 16** instance (+ backups per
  `docs/BACKUP-RESTORE.md`).
- Production **Redis 7** instance.
- Hosting for the bot process(es)/containers (single or multi-replica),
  including any orchestrator.
- **Monitoring stack** — Prometheus (or compatible) scraping `/metrics`,
  log pipeline consuming the JSON log format, alert routing per
  `docs/OPERATIONS.md`.
- **Secret store / env injection** for keys and tokens (the software reads
  secrets from the environment only).
- **Domains / reverse proxy / TLS** if the API is exposed beyond loopback
  (the server refuses non-loopback binding without API auth; exposing it
  publicly is the buyer's decision and responsibility).
- **CI runners** (GitHub Actions or an adapted equivalent).
- **Funded trading keys** and the capital-risk policy around them.

## 3. EXTERNAL SERVICES (third parties; buyer contracts & pays)

- **Solana RPC + WebSocket providers** (mainnet/devnet access, rate limits,
  failover endpoints).
- **Geyser provider** (Yellowstone-compatible `transactionSubscribe`) —
  optional; poll fallback exists.
- **PumpPortal API** — optional feed for pump.fun launch/trade detection.
- **Polymarket** — Gamma + CLOB APIs and the Polygon-hosted CTF Exchange
  contracts; buyer must satisfy Polymarket's terms of service and applicable
  law in their jurisdiction.
- **Telegram** — Bot API access and the buyer's own bot (BotFather token).
- **Cloud/infrastructure vendors** of the buyer's choice.
- The on-chain protocols themselves (pump.fun, PumpSwap, Raydium, Jupiter,
  Solana) are public infrastructure — no contract needed, but their programs
  can change without notice (`docs/BUYER-RISK-REGISTER.md`).

## 4. HUMAN / LEGAL RESPONSIBILITIES (not software; not transferable by code)

- **License ownership & copyright assignment** — inserting the legal entity
  into `LICENSE`; any IP assignment agreement between the parties.
- **Regulatory review** — trading crypto-assets and prediction markets is
  regulated unevenly across jurisdictions; compliance (including whether
  Polymarket access is lawful for the buyer) is solely the buyer's
  responsibility (README §Disclaimer).
- **External security audit** — none exists; commissioning one (especially
  before any staking-program mainnet deployment) is a buyer decision/cost.
- **Custody policy** — how keys are generated, stored, rotated, and who may
  use them; the software provides the signer boundary, not the policy.
- **Live-trading approval & supervision** — flipping both live gates, sizing
  risk limits, gradual funded validation, and kill-switch drills are operator
  acts (`docs/BUYER-DEPLOYMENT.md` §15).
- **Security contact & incident response ownership** — publishing a real
  contact in `SECURITY.md` and staffing it (`docs/SUPPORT-HANDOVER.md`).
- **Venue terms of service** — accepting/complying with the ToS of every
  venue connected.

## Boundary rules of thumb

1. If it is a file in this repository, it is delivered software (category 1).
2. If it must run somewhere the seller does not control, it is buyer
   infrastructure (category 2) or an external service (category 3).
3. If it requires a human decision, signature, payment, or legal judgment,
   it is category 4 — no code in this repository makes those decisions, and
   the software deliberately blocks several of them behind explicit gates
   (live mode, mainnet staking deployment).
`````


----- COMPLETE FILE: docs/SUPPORT-HANDOVER.md (6571 bytes, 133 lines) -----

`````markdown
# Support & handover model

What handover concretely consists of, category by category. This document
promises no SLA, no support duration, and no availability commitment — those
are contractual matters between the parties, not properties of the software.
What follows are the factual transfer categories and the artifacts that make
each one self-serviceable.

## 1. Source handover

- **Artifact:** the git repository at freeze commit `0e139c3` (on release
  commit `9c677cd`), 146 tracked files, clean tree, no secrets, no build
  artifacts (`.gitignore`/`.dockerignore` enforced; secret-scan gate passed).
- **Buyer verification:** `docs/BUYER-DEPLOYMENT.md` §1–2 (commit, file
  count, version identity).
- **Completeness rule:** everything needed to build, test and run is in the
  tree — proven by the from-zero rebuild on a wiped machine
  (`docs/HANDOVER.md` §2).

## 2. Deployment handover

- **Artifacts:** `Dockerfile`, `docker-compose.yml`, `.env.template`,
  `config.toml.example` (annotated reference for every key),
  `docs/DEPLOYMENT.md`, `docs/BUYER-DEPLOYMENT.md` (15-step sequence).
- **Boundary:** the seller delivers the procedure and templates; the buyer
  executes them on buyer infrastructure. Docker image build/smoke and CI were
  NOT EXECUTED in the delivery environment — the buyer's first successful run
  of each is their verification of those paths.

## 3. Configuration handover

- **Artifacts:** `config.toml.example` (all sections annotated),
  README §Configuration (precedence: defaults → TOML → .env → env overrides;
  unknown keys rejected), `docs/OPERATIONS.md`.
- **Safety property:** the defaults are safe — paper mode, all modules
  disabled, strict config parsing (typos fail startup instead of silently
  changing behavior), `[signing] provider` values that are not implemented
  fail startup rather than falling back.

## 4. Incident handover

- **Artifacts:** `docs/OPERATIONS.md` runbook (alerts, degradation matrix,
  emergency stop, journal inspection, audit verification), kill-switch
  surfaces (`POST /api/kill`, Telegram `/kill`, cross-replica flag sync),
  `docs/RECONCILIATION.md` (ambiguity matrix), `docs/BACKUP-RESTORE.md`
  (Redis-loss and DB-restore behavior).
- **Expectation:** incident response after transfer is staffed by the buyer
  using these runbooks; nothing in the software phones home or depends on the
  seller at runtime.

## 5. Security contact handover

- **State at delivery:** root `SECURITY.md` deliberately points at "the
  security contact of the current repository owner" — no real address ships,
  because publishing a fake one would misroute vulnerability reports.
- **Action:** at transfer, the receiving party publishes their own contact
  in `SECURITY.md` (tracked item: `docs/HANDOVER.md` §5.4,
  `docs/ACCEPTANCE-CHECKLIST.md`).

## 6. Credential rotation

At transfer, **every credential that ever existed on either side should be
considered compromised-by-default and rotated**, regardless of the secret-scan
evidence (which is clean). Checklist:

- Solana keypair(s) — generate new keys for production; never reuse keys that
  existed in any test environment.
- Polymarket/Polygon private key — new key, new API credentials (L1/L2
  headers derive from it).
- Telegram bot token — revoke & reissue via BotFather; update allow-lists.
- `API_KEY` — new value; it gates all mutating routes.
- PostgreSQL / Redis credentials — new; PG is reachable only from the compose
  network by default, but rotate anyway.
- RPC/Geyser/PumpPortal provider API keys — re-contract in the buyer's name.

The software stores secrets only in the environment, so rotation requires no
code change — restart with new env values.

## 7. Ownership transfer

- **Legal:** copyright holder insertion in `LICENSE` (currently the
  placeholder "sniper-suite authors"); any IP assignment paperwork is between
  the parties (`docs/SCOPE-BOUNDARY.md` §4).
- **Technical:** repository ownership/remote transfer; set the real
  `repository` URL in the workspace `Cargo.toml` at publish time.
- **What ownership does NOT include:** third-party protocols, APIs, brands or
  services (itemized in `docs/IP-COMPONENTS.md` §Summary and
  `docs/THIRD-PARTY.md` §3).

## 8. Repository transfer

- Transfer mechanism is buyer/seller-agreed (git bundle, hosted-repo
  transfer, or fresh private remote). Verify on receipt: commit hashes
  `9c677cd` + `0e139c3` present, `git status` clean, `release-check.sh`
  green on the buyer's machine (`docs/BUYER-DEPLOYMENT.md` §9).
- History note: the repository carries the full commit history through the
  freeze; `AUDIT.md` preserves the per-pass engineering evidence independent
  of git.

## 9. Staking program ownership

- **State at delivery:** source + host tests + PREVIOUSLY VERIFIED BPF build
  and validator e2e. **Not deployed to any cluster**; `declare_id!` is a
  pre-deploy placeholder.
- **Transfer actions (buyer):** deploy under the placeholder id with the
  matching keypair (or change id + keypair and rebuild); initialize with the
  admin set to a **multisig PDA** (Squads/Realms recommended — the program
  deliberately does not embed M-of-N logic); set production timelock ≥ 24h;
  plan the one-shot `GenesisMint`; commission the external audit **before**
  mainnet (`docs/STAKING.md`, README §"Deploying the staking program").
- Whoever holds the admin key(s) controls governance within the program's
  caps and timelock — custody of that multisig is the real ownership
  question, and it is a buyer-side responsibility from deployment onward.

## 10. Production sign-off

A factual definition of "done" for the transfer, matching
`docs/ACCEPTANCE-CHECKLIST.md`:

1. Full local gate (`release-check.sh`) green on buyer infrastructure.
2. Paper-mode run stable on buyer infra; health/readiness/metrics verified;
   Telegram verified live; crash-recovery drill passed; backup→restore drill
   passed on the buyer's PG.
3. (If simulating) `simulate` mode exercised — real transactions built and
   RPC-simulated, nothing broadcast.
4. (If trading live) both live gates deliberately enabled, risk limits sized,
   gradual funded validation under operator supervision.
5. (If deploying staking) external audit passed, multisig admin in place,
   program id finalized, genesis plan executed once.
6. Legal fill-ins complete: LICENSE holder, repository URL, security contact.

Until every applicable line is checked, the system is "delivered" but not
"signed off" — the distinction matters and is intentional.
`````


----- COMPLETE FILE: docs/BUYER-RISK-REGISTER.md (7634 bytes, 35 lines) -----

`````markdown
# Buyer risk register

Every known remaining risk at handover of sniper-suite 0.1.0, stated
factually: what it is, what it would impact, what mitigation already exists
in the delivered asset, where the evidence lives, and what the buyer must do.
Nothing here is sensationalized; nothing is hidden. Ordered roughly by
severity of consequence.

| # | Risk | Impact | Current mitigation (delivered) | Evidence | Buyer action |
|---|------|--------|-------------------------------|----------|--------------|
| 1 | **No external security audit** (any component; especially the on-chain staking program) | Undiscovered vulnerabilities could lead to loss of funds (staking vault) or exploitable behavior in trading paths | Extensive internal test matrix (521 + 48/2 tests, tamper/concurrency/dedup/recovery suites); clippy `-D warnings`; cargo-audit/deny gates; documentation explicitly blocks staking mainnet deployment until an audit passes; paper-by-default + dual live gates limit blast radius | Root `SECURITY.md`, `docs/SECURITY.md`, `docs/STAKING.md`, `release-manifest.json` `not_executed_environment_blocked` | Commission an independent audit before mainnet staking deployment; consider a code review of trading paths before funded live operation |
| 2 | **Funded live execution never generally tested** (no funded keypair existed; requires explicit approval) | Real-money behavior (slippage, landing rate, partial fills, venue edge cases) is unproven at scale | `simulate` mode RPC-simulates real transactions without sending; paper mode simulates fills against live data; reconcile-before-restart; handoff grace for ambiguous outcomes; kill switch; daily-loss auto-disable | `release-manifest.json`, `docs/TESTING.md` §Known gaps, `docs/RECONCILIATION.md` | Gradual funded validation under operator supervision, small sizes first (`docs/BUYER-DEPLOYMENT.md` §15) |
| 3 | **Staking program deployment identity not finalized** (`declare_id!` is a pre-deploy placeholder; program deployed nowhere) | Module 4 cannot run live; deploying under a wrong/lost keypair would strand governance | Two-step admin transfer prevents key-typo lock-in; parameter timelock; permissionless `ApplyParams` prevents admin griefing; deployment sequence fully documented incl. genesis latch | `programs/staking-suite/src/lib.rs`, `docs/STAKING.md`, `docs/HANDOVER.md` §5.2 | Deploy under the placeholder id with the matching keypair (or change id+keypair and rebuild); initialize with a multisig admin; timelock ≥ 24h |
| 4 | **Production RPC dependency & provider rate limits** | Degraded detection latency, failed broadcasts, reconciliation delays during provider outages | Retry/failover chokepoint with consecutive-failure tracking; optional broadcast fan-out across providers; readiness degradation is visible (503 + `bot_rpc_*` metrics); intents are reconciled, never blindly re-broadcast | `crates/solana-kit/src/rpc.rs`, `docs/OPERATIONS.md` degradation matrix | Contract ≥ 1 quality provider (preferably 2 for fan-out/failover); monitor `bot_rpc_requests_total{outcome}` |
| 5 | **Third-party API/protocol changes** (pump.fun, PumpSwap, Raydium, Jupiter, Polymarket CLOB/Gamma, PumpPortal, Telegram Bot API) | Integration breakage: wrong instruction layouts, changed endpoints/auth, rejected orders | Account-layout parsing and builders are isolated per venue (`crates/solana-kit/src/*.rs`, `module-polymarket`); strict deserialization fails loudly rather than trading on garbage; poll fallbacks for feeds; CI + release-check catch regressions on upgrade | `docs/IP-COMPONENTS.md`, mock harnesses in `tests/` | Watch venue changelogs; re-run the gate after any dependency bump; budget maintenance for venue drift |
| 6 | **Docker path not executed in delivery sandbox** (no daemon) | Image/compose defects would surface only at buyer's first build | Dockerfile is multi-stage, non-root, healthchecked, toolchain-pinned; `docker compose config` syntax gate in CI; static inspection done | `release-manifest.json`, `.github/workflows/ci.yml` docker job | Run `docker compose up --build` + health smoke as an early acceptance step |
| 7 | **CI has never run on a real GitHub runner** (workflow delivered, unexecuted here) | CI-specific breakage (runner images, service containers) possible | Every CI step has a local equivalent that passed in the freeze gate (release-check 20/20 mirrors the workflow); workflow pins toolchain + service images | `.github/workflows/ci.yml`, `docs/HANDOVER.md` §3 | Trigger CI on the buyer's fork/remote and fix runner-specific issues before relying on it |
| 8 | **Secret infrastructure is buyer-side** (env/secret store quality, key custody) | Key compromise ⇒ direct fund loss; the software can only refuse to embed secrets | Env-var indirection only; secret-scan release gate; signer boundary (modules never see keys); token-redaction regression test; non-loopback bind refused without API auth; credential-rotation checklist | `docs/SECURITY.md`, `docs/SUPPORT-HANDOVER.md` §6 | Use a real secret store; rotate all credentials at transfer; restrict who can read prod env; consider `vault`/`kms` provider implementation (extension point exists, fails startup unimplemented) |
| 9 | **Regulatory responsibility** (crypto trading, prediction markets, token issuance via staking program) | Legal exposure varies by jurisdiction; Polymarket access is restricted in some | Software defaults to paper; nothing broadcasts without explicit dual gates; README disclaimer assigns venue-ToS/legal compliance to the operator | README §Disclaimer, `docs/SCOPE-BOUNDARY.md` §4 | Obtain jurisdiction-appropriate legal review before funded operation or token distribution |
| 10 | **Multi-replica behavior tested at 2 replicas only** | Unforeseen coordination edge cases at larger N | Claims/leases/epochs/fencing are backend-atomic (PG authoritative); `execution_claim_events` lineage gives post-hoc forensics; tighten-only risk oracle cannot be loosened by a rogue replica | `distributed_integration` 4/4, `two_replica_mirror` 1/1, `docs/DISTRIBUTED.md` | Stay at ≤ 2 active replicas until the buyer has exercised larger topologies in staging |
| 11 | **Ownership/legal metadata placeholders** (LICENSE holder, repo URL, security contact) | Ambiguous copyright attribution; misrouted vulnerability reports | Placeholders are deliberate and documented — no fake values ship; release-check verifies version identity consistency | `docs/HANDOVER.md` §5, `docs/ACCEPTANCE-CHECKLIST.md` | Complete the three fill-ins at transfer (legal action, not code) |
| 12 | **Point-in-time supply-chain scans** (audit/deny passed at freeze) | Advisories published after the freeze are not reflected | Both lockfiles committed; audit/deny are CI hard gates going forward | `docs/THIRD-PARTY.md` §5, `release-manifest.json` | Re-run `cargo audit` / `cargo deny check` periodically and on every dependency change |

## Explicitly NOT risks (documented design properties)

- **Redis failure** — non-authoritative by design; no money-relevant truth is
  lost (`docs/BACKUP-RESTORE.md`).
- **Process crash mid-execution** — intent journal + startup reconciliation +
  dedup make restarts safe; tested (`docs/RECONCILIATION.md`).
- **Audit tampering via app APIs** — impossible: append-only from all APIs,
  hash-chained, verified by `GET /api/audit/verify`; DB-superuser tampering
  plus rehash is outside the threat model and stated as such
  (`docs/SECURITY.md`).
- **Accidental live trading** — requires two config gates + owner-only runtime
  switch + real keys; a single misconfiguration cannot enable broadcasts.
`````


----- COMPLETE FILE: docs/TECHNICAL-DIFFERENTIATORS.md (7747 bytes, 139 lines) -----

`````markdown
# Technical differentiation — factual characteristics only

Concrete engineering characteristics that exist in sniper-suite 0.1.0, each
with its location in the tree. This document does **not** rank the project,
compare it to competitors, call it "best"/"enterprise"/"production-proven",
or make performance-superiority claims. Whether these characteristics matter
is the buyer's judgment; that they exist is verifiable.

## Language & build discipline

1. **All-Rust implementation** — memory-safe systems language for every
   component: trading modules, control plane, and the on-chain program
   (91 `.rs` files; no second production language).
2. **Hard-pinned toolchain, three-way enforced** — 1.98.1 in
   `rust-toolchain.toml`, `Dockerfile` (`rust:1.98.1-bookworm`) and CI
   (`dtolnay/rust-toolchain@1.98.1`); `scripts/release-check.sh` fails the
   release if they drift.
3. **`clippy -D warnings` as a hard gate** on both cargo projects (app
   workspace + standalone program), in CI and in the local release gate —
   zero-warning codebase at delivery.
4. **Two committed lockfiles** with MSRV-aware resolution for the on-chain
   crate (`rust-version = 1.79` + `.cargo/config.toml` resolver policy so the
   agave platform-tools compiler can always build the lock).
5. **On-chain release profile hardened** — `overflow-checks = true`,
   `lto = "fat"`, `codegen-units = 1`, `panic = "abort"`
   (`programs/staking-suite/Cargo.toml`).

## Architecture

6. **Modular crate architecture** — modules are independent crates behind a
   shared core (events, risk, OMS, persistence); a venue/module can be
   replaced without touching the money-path invariants.
7. **Single supervised binary** — one process supervises all modules with
   defined startup/shutdown ordering (`crates/server/src/main.rs`), rather
   than a sprawl of services.
8. **Strict configuration contract** — `deny_unknown_fields`: a typo in
   `config.toml` fails startup instead of silently changing behavior;
   unimplemented signing providers (`vault`/`kms`/`hsm`) fail startup rather
   than falling back (`crates/core/src/config.rs`).

## Solana-specific engineering

9. **Protocol-specific instruction builders, hand-rolled** — pump.fun
   bonding curve (incl. v2 variants), PumpSwap AMM, Raydium AMM
   (`SwapBaseIn` + `SwapBaseInV2`), Jupiter routing, from parsed account
   layouts (`crates/solana-kit/src/{pump,pumpswap,raydium,jupiter,layout}.rs`)
   — no heavyweight third-party "sniper SDK" dependency.
10. **Geyser support** — Yellowstone-style `transactionSubscribe` push feeds
    with failed-tx skipping and automatic poll fallback
    (`crates/solana-kit/src/events.rs`; mock-tested).
11. **Warm account cache** — TTL + FIFO-bounded caching of semi-static
    accounts (`crates/solana-kit/src/cache.rs`; env-tunable, `0` disables).
12. **Multi-RPC fan-out** — `BROADCAST_FANOUT` races the same signed
    transaction across primary + fallback RPCs, first accept wins
    (mock-JSON-RPC-pair tested); retry chokepoint with failover after bounded
    consecutive failures and per-method metrics.
13. **Signer abstraction** — `TransactionSigner` + named `SignerRegistry`;
    multi-signer transactions fail structurally if any required signer is
    undeclared/unresolvable; trading modules never touch key material
    (`crates/solana-kit/src/signer.rs`).
14. **Simulate-first execution policy** — transactions are RPC-simulated
    before broadcast by default (`SIMULATE_FIRST`,
    `ABORT_ON_SIMULATION_FAILURE`).

## Correctness / money-path engineering

15. **Deterministic idempotency** — idempotency keys + three-level
    restart-safe dedup (memory/Redis/Postgres) (`crates/core/src/{oms,dedup}.rs`).
16. **Intent journal + reconciliation-before-trust** — intents are recorded
    before send; on restart, unresolved intents are reconciled against venue
    truth before new work; ambiguous outcomes get handoff grace instead of
    blind retry (`docs/RECONCILIATION.md`).
17. **Distributed claims with leases, epochs and fencing tokens** — the
    invariant *one logical execution ⇒ ≤1 active owner ⇒ ≤1 money-moving
    submission*, with an append-only `execution_claim_events` lineage table
    for forensics (`crates/core/src/ownership.rs`, migrations `0009`–`0011`).
18. **Global risk oracle, tighten-only** — cluster-wide risk limits can
    propagate in one direction only; a rogue replica cannot loosen risk
    (`docs/DISTRIBUTED.md`).
19. **Hash-chained append-only audit trail** — no app API can mutate or
    delete audit rows; `GET /api/audit/verify` recomputes the chain; appends
    serialized by advisory lock, tested linear under 8 concurrent appenders,
    with modification/reorder/missing/duplicate tamper-detection tests
    (`crates/core/src/audit.rs`).
20. **PostgreSQL as sole financial truth** — Redis is architecturally
    forbidden from holding authoritative state; forward-only migrations
    (0001–0011) embedded in the binary.

## Staking program characteristics

21. **Parameter timelock with permissionless apply** — queued changes can be
    applied by anyone after the delay (unresponsive admin cannot grief),
    cancellable before; changing the delay itself waits out the old delay
    (OpenZeppelin `TimelockController` rule).
22. **Pause that cannot trap funds** — pause halts deposits only; `Unstake`
    and `Claim` are never gated.
23. **Hard parameter caps on-chain** — fee ≤ 10%, reward rate ≤ 100% APR,
    enforced in-program, so a compromised admin cannot set confiscatory or
    inflationary parameters.
24. **One-shot latched genesis mint** — `GenesisMint` latches
    `genesis_done`; any second attempt fails `GenesisAlreadyDone` (6026) —
    supply cannot be silently inflated post-launch.
25. **Two-step admin transfer** — `TransferAdmin` → `AcceptAdmin`; zero pubkey
    rejected; multisig-admin-by-CPI is the documented production pattern
    without embedding M-of-N logic in-program.

## Cross-venue capability

26. **Polymarket V2 order signing implemented from the spec** — EIP-712
    11-field v2 `Order`, correct domain separator, type-3 signature wrapping,
    L1/L2 CLOB auth headers, CTF ERC-1155 balance reads
    (`crates/module-polymarket/src/eip712.rs` et al.) — wire format
    mock-verified.
27. **Three Solana exit venues behind one executor** — PumpSwap, Raydium,
    Jupiter routing for sniper exits (`crates/module-sniper/src/exit.rs`).

## Verification culture

28. **Extensive test matrix with honest labels** — 521 workspace tests
    (offline-deterministic core + protocol mocks + real-service gated
    integration), 48+2 staking tests, and a VERIFIED / PREVIOUSLY VERIFIED /
    GATED / NOT-EXECUTED taxonomy applied consistently across
    `release-manifest.json`, `docs/HANDOVER.md`, `docs/TESTING.md`,
    `AUDIT.md` — unexecuted things are never labeled as passing.
29. **One-command reproducible release gate** —
    `scripts/release-check.sh`: 20 gates (files, version identity, toolchain
    pin, migration monotonicity, TODO/stub marker scan, secret scan, fmt,
    check, clippy, full test matrix against real PG/Redis, staking suite,
    cargo-audit ×2, cargo-deny), exit-code honest, gated suites announce
    skips instead of faking passes.
30. **Crash/recovery and tamper scenarios are tested, not just described** —
    restart fidelity, corrupt journal lines, OMS restart-recovery, audit
    concurrency, two-process replica mirroring, all in the delivered suite.

---

Each numbered item is checkable at the cited path. If a claim here and the
source ever disagree, the source wins — and `docs/HANDOVER.md` §6 lists the
invariants a maintainer must not regress.
`````


----- COMPLETE FILE: docs/DELIVERY-MANIFEST.md (7548 bytes, 80 lines) -----

`````markdown
# Commercial artifact index (delivery manifest — human-readable)

Index of every artifact in the sniper-suite 0.1.0 buyer package and what it
is for. The machine-readable manifest is `release-manifest.json` (version,
components, toolchain, test counts, verification status, external blockers);
this document is the human map and does not duplicate source code.

## Delivery identity

- Product: **sniper-suite** — modular crypto trading system (5 modules +
  control plane), version **0.1.0**, MIT license (holder placeholder pending
  transfer).
- Frozen engineering tree: release commit `9c677cd`, freeze commit
  `0e139c3`; 146 tracked files / 2,801,590 bytes / 77,980 lines at freeze;
  final gate 20/20 PASS, 521/521 workspace tests, 0 failures.

## Root artifacts

| Artifact | What it is |
|---|---|
| [`release-manifest.json`](../release-manifest.json) | Machine-readable delivery manifest — version identity, components, migration high-water mark, toolchain pins, exact test counts, verification taxonomy, external handover blockers. Gated by `scripts/release-check.sh`. |
| [`README.md`](../README.md) | Product overview, quick start, configuration reference, API/observability summary, staking deployment, testing, project layout — plus the "Buyer / engineering handover" index section. |
| [`CHANGELOG.md`](../CHANGELOG.md) | Keep-a-Changelog history of 0.1.0 incl. both pre-tag fix passes (release-engineering + engineering-freeze). |
| [`AUDIT.md`](../AUDIT.md) | Full historical audit & build trail with per-pass evidence (27 sections). Historical sections are preserved as-is; later passes append. |
| [`SECURITY.md`](../SECURITY.md) | Vulnerability-reporting policy, supported versions, posture statement (incl. the explicit "no external audit" disclosure). |
| [`LICENSE`](../LICENSE) | MIT text with the documented copyright-holder placeholder + handover note. |
| [`VERSION`](../VERSION) | Release identity (`0.1.0`), gated for consistency with `Cargo.toml` + manifest. |
| [`scripts/release-check.sh`](../scripts/release-check.sh) | One-command, 20-gate local release validation (fmt → tests against real PG/Redis → staking → audit/deny → consistency). |
| [`rust-toolchain.toml`](../rust-toolchain.toml), [`deny.toml`](../deny.toml), [`Cargo.lock`](../Cargo.lock) | Pinned toolchain, supply-chain policy, locked app dependency graph (706 packages). |
| [`Dockerfile`](../Dockerfile), [`docker-compose.yml`](../docker-compose.yml), [`.env.template`](../.env.template), [`config.toml.example`](../config.toml.example) | Deployment assets (image build NOT EXECUTED in delivery sandbox — CI covers it). |
| [`.github/workflows/ci.yml`](../.github/workflows/ci.yml) | 4-job CI: app workspace (services: PG16/Redis7), staking program (build-sbf + gated validator e2e), security (audit/deny), docker (build + smoke). |

## Engineering documentation (13 docs, delivered at freeze)

| Doc | What it is |
|---|---|
| [`docs/HANDOVER.md`](HANDOVER.md) | Verify-from-zero procedure, verification-status taxonomy, handover fill-ins, maintenance invariants. **Start here.** |
| [`docs/ARCHITECTURE.md`](ARCHITECTURE.md) | Crate map, data-flow guarantees, startup/shutdown ordering. |
| [`docs/API.md`](API.md) | REST + WebSocket reference (28 endpoints), RBAC matrix, degradation contract. |
| [`docs/SECURITY.md`](SECURITY.md) | Threat model, key management, signer boundary, honest limitations. |
| [`docs/DEPLOYMENT.md`](DEPLOYMENT.md) | Compose + bare-metal setup, production checklist. |
| [`docs/OPERATIONS.md`](OPERATIONS.md) | Day-two runbook: alerts, incidents, journal, audit, backups. |
| [`docs/MODULES.md`](MODULES.md) | Per-module trading guide (feeds, sizing, exits, strategies). |
| [`docs/STAKING.md`](STAKING.md) | Program economics, governance, deploy + genesis sequence. |
| [`docs/TESTING.md`](TESTING.md) | Test layers, what runs where, known gaps. |
| [`docs/RECONCILIATION.md`](RECONCILIATION.md) | Source-of-truth model, ambiguity matrix, crash/startup recovery, PnL replay. |
| [`docs/DISTRIBUTED.md`](DISTRIBUTED.md) | Multi-replica operation: ownership, claims/leases/fencing, flag & book sync. |
| [`docs/RELEASE.md`](RELEASE.md) | Versioning, reproducible-build analysis, release manifest, cut-a-release checklist. |
| [`docs/BACKUP-RESTORE.md`](BACKUP-RESTORE.md) | Durable vs ephemeral data, backup/restore procedures, Redis-loss behavior. |

## Buyer package (14 docs, added in the commercialization pass)

| Doc | What it is |
|---|---|
| [`docs/BUYER-OVERVIEW.md`](BUYER-OVERVIEW.md) | Technical overview: product, modules, architecture, execution path, risk, persistence, reconciliation, distributed ownership, observability, staking, deployment/security/testing/recovery models. |
| [`docs/CAPABILITY-MATRIX.md`](CAPABILITY-MATRIX.md) | Per-capability matrix: implemented / evidence / tested / environment / known limitation (23 capabilities). |
| [`docs/BUYER-DUE-DILIGENCE.md`](BUYER-DUE-DILIGENCE.md) | Independent verification checklist: source, build, testing, security, infrastructure, operations, ownership/IP, open external actions. |
| [`docs/IP-COMPONENTS.md`](IP-COMPONENTS.md) | IP/component inventory with provenance (original code vs external protocol integration) and licensing notes. |
| [`docs/THIRD-PARTY.md`](THIRD-PARTY.md) | Third-party/license inventory: lockfile provenance, major deps, deny policy, advisory scanning, SBOM status, reproduction commands. |
| [`docs/BUYER-DEPLOYMENT.md`](BUYER-DEPLOYMENT.md) | 15-step deployment handover with safety gates; ends in paper mode; live mode gated separately. |
| [`docs/ACCEPTANCE-CHECKLIST.md`](ACCEPTANCE-CHECKLIST.md) | Sign-off checklist with per-item status: VERIFIED / PREVIOUSLY VERIFIED / BUYER ACTION / EXTERNAL. |
| [`docs/RELEASE-NOTES-0.1.0.md`](RELEASE-NOTES-0.1.0.md) | Buyer release notes: identity, test results, components, security/engineering fixes, blockers, explicit non-claims. |
| [`docs/BUYER-FAQ.md`](BUYER-FAQ.md) | Technical FAQ — every answer source-backed (safety defaults, double-execution, crash behavior, Redis/RPC loss, audits, extensibility, multi-replica/tenancy). |
| [`docs/SCOPE-BOUNDARY.md`](SCOPE-BOUNDARY.md) | Commercial boundary: delivered software vs buyer infrastructure vs external services vs human/legal responsibilities. |
| [`docs/SUPPORT-HANDOVER.md`](SUPPORT-HANDOVER.md) | Handover model: source/deployment/config/incident/security-contact/credential-rotation/ownership/repository/staking/production sign-off. No SLA is promised or implied. |
| [`docs/BUYER-RISK-REGISTER.md`](BUYER-RISK-REGISTER.md) | 12 remaining risks with impact, delivered mitigation, evidence, buyer action — plus documented non-risks. |
| [`docs/TECHNICAL-DIFFERENTIATORS.md`](TECHNICAL-DIFFERENTIATORS.md) | 30 concrete engineering characteristics, each with a path. No rankings or superiority claims. |
| [`docs/DELIVERY-MANIFEST.md`](DELIVERY-MANIFEST.md) | This index. |

## Suggested reading order for a technical buyer

1. `docs/BUYER-OVERVIEW.md` — what the system is.
2. `docs/CAPABILITY-MATRIX.md` — what is implemented and how it was tested.
3. `docs/BUYER-DUE-DILIGENCE.md` + `docs/ACCEPTANCE-CHECKLIST.md` — how to
   verify everything independently.
4. `docs/BUYER-RISK-REGISTER.md` + `docs/SCOPE-BOUNDARY.md` — what remains
   open and who owns what.
5. `docs/BUYER-DEPLOYMENT.md` — how to stand it up (paper mode first).
6. Deep dives as needed: the 13 engineering docs, `AUDIT.md` for evidence
   history, `release-manifest.json` for machine-readable facts.
`````


== SECTION 5: COMPLETE CONTENT OF EVERY MODIFIED FILE ==


----- COMPLETE FILE: README.md (29395 bytes, 571 lines) -----

`````markdown
# sniper-suite

A modular crypto trading system written in **Rust**. It bundles five cooperating
modules behind one control plane (Axum REST + WebSocket + an embedded HTML
dashboard), with a Telegram bot for remote on/off control.

| # | Module | Crate | What it does |
|---|--------|-------|--------------|
| 1 | **Sniper** | `module-sniper` | Detects new pump.fun launches and buys within ~1s, with PumpSwap/Raydium/Jupiter exit routing. |
| 2 | **Copy trading** | `module-copy` | Mirrors buys (and optionally exits) of tracked "smart money" wallets. |
| 3 | **Polymarket** | `module-polymarket` | Automated prediction-market betting via Gamma + CLOB REST + WebSocket, with EIP-712 v2 order signing. |
| 4 | **Staking contract** | `programs/staking-suite` | On-chain Solana program: reward token, staking vault, deposit fees, per-second APY accrual, parameter timelock, one-time latched genesis mint. |
| 5 | **Telegram control** | `module-telegram` | Long-polling bot to turn modules on/off, kill-switch, and receive alerts. |

Shared plumbing lives in `bot-core` (config, state, event bus, risk engine,
models) and `solana-kit` (RPC, tx executor, wallet, pump/raydium instruction
builders, swap decoding). The `sniper-suite` crate is the runnable binary that
supervises every module.

> **Safety first.** The suite defaults to **paper** trading. Nothing is sent
> on-chain or to Polymarket until you flip *both* gates (see
> [Going live](#going-live)). Run at your own risk; this is not financial advice.

---

## Documentation

| Doc | Contents |
|---|---|
| [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) | crate map, data-flow guarantees, startup/shutdown ordering |
| [docs/API.md](docs/API.md) | REST + WebSocket reference, RBAC matrix, degradation contract |
| [docs/SECURITY.md](docs/SECURITY.md) | threat model, key management, honest limitations list |
| [docs/DEPLOYMENT.md](docs/DEPLOYMENT.md) | compose + bare-metal setup, production checklist |
| [docs/OPERATIONS.md](docs/OPERATIONS.md) | runbook: alerts, incidents, journal, audit, backups |
| [docs/MODULES.md](docs/MODULES.md) | per-module trading guide (feeds, sizing, exits, strategies) |
| [docs/STAKING.md](docs/STAKING.md) | program economics, governance, deploy + genesis sequence |
| [docs/TESTING.md](docs/TESTING.md) | test layers, what runs where, known gaps |
| [docs/RECONCILIATION.md](docs/RECONCILIATION.md) | source-of-truth model, ambiguity matrix, crash & startup recovery, PnL replay |
| [docs/DISTRIBUTED.md](docs/DISTRIBUTED.md) | multi-replica operation: single active logical execution owner, claims/leases/fencing, flag & book sync |
| [docs/RELEASE.md](docs/RELEASE.md) | versioning, reproducible-build analysis, release manifest, cut-a-release checklist |
| [docs/HANDOVER.md](docs/HANDOVER.md) | engineering handover: verify from zero, verification-status taxonomy, maintenance invariants |
| [docs/BACKUP-RESTORE.md](docs/BACKUP-RESTORE.md) | durable vs ephemeral data, backup/restore procedures, Redis-loss behavior |
| [AUDIT.md](AUDIT.md) | full pre-build audit + build-plan execution status |

### Buyer / engineering handover

A 14-document buyer package (added after the 0.1.0 engineering freeze; no
source code changed) covers commercial due diligence. Index:
[docs/DELIVERY-MANIFEST.md](docs/DELIVERY-MANIFEST.md). Start with
[docs/BUYER-OVERVIEW.md](docs/BUYER-OVERVIEW.md) (what the system is),
[docs/CAPABILITY-MATRIX.md](docs/CAPABILITY-MATRIX.md) (what is implemented
and how it was tested), [docs/BUYER-DUE-DILIGENCE.md](docs/BUYER-DUE-DILIGENCE.md)
(how to verify everything independently),
[docs/ACCEPTANCE-CHECKLIST.md](docs/ACCEPTANCE-CHECKLIST.md) (sign-off list),
[docs/BUYER-DEPLOYMENT.md](docs/BUYER-DEPLOYMENT.md) (15-step deployment,
paper mode first), [docs/BUYER-RISK-REGISTER.md](docs/BUYER-RISK-REGISTER.md)
(remaining risks), and [docs/SCOPE-BOUNDARY.md](docs/SCOPE-BOUNDARY.md)
(what transfers vs what the buyer provides). The machine-readable facts live
in [release-manifest.json](release-manifest.json).

---

## Requirements

- **Rust** — declared MSRV is **1.82** (`rust-version` in `Cargo.toml`); the
  pinned, verified toolchain is **1.98.1** (`rust-toolchain.toml` — rustup
  selects it automatically; the full test suite and CI gate on that exact
  version). Install via [rustup](https://rustup.rs).
- For building Module 4 only: the **Solana CLI / cargo-build-sbf** toolchain.
- Optional native deps for Solana builds: `pkg-config`, `libudev-dev`,
  `protobuf-compiler`, `cmake`, a C toolchain.

The workspace uses a committed `Cargo.lock`; a normal `cargo build` will fetch
the pinned crates.

---

## Quick start (paper mode)

```bash
# 1. Configure
cp config.toml.example config.toml
$EDITOR config.toml            # enable the modules you want, set sizes

# 2. Build
cargo build --release

# 3. Run (CONFIG_PATH defaults to ./config.toml)
cargo run --release -p sniper-suite
# or:  CONFIG_PATH=./config.toml ./target/release/sniper-suite

# 4. Open the dashboard
xdg-open http://localhost:8080/     # live status, positions, trades, events
```

**Full stack with Docker** (bot + PostgreSQL + Redis — the durability stack
for orders/trades/positions/audit and dedup L2):

```bash
cp .env.template .env && $EDITOR .env    # set POSTGRES_PASSWORD etc.
docker compose up --build -d
curl -s localhost:8080/ready | jq
```

Enable modules in `config.toml` (`[sniper] enabled = true`, etc.) or at runtime
through the API / Telegram. In paper mode fills are simulated against live
market data with seeded balances (10 SOL / 1000 USDC).

---

## Configuration

All settings live in a single TOML file (`config.toml`). Every key is optional —
unknown keys are **rejected** (`deny_unknown_fields`), so keep names exactly as
in [`config.toml.example`](config.toml.example). See that file for the full,
annotated reference of every section:

`[network]` `[execution]` `[risk]` `[sniper]` `[copy]` `[polymarket]`
`[contract]` `[telegram]` `[api]` `[storage]` `[secrets]`

### Precedence

1. Built-in defaults
2. `config.toml` (path from `CONFIG_PATH`, else `./config.toml`)
3. `.env` (loaded via `dotenvy`)
4. Environment-variable overrides (highest priority)

### Environment overrides

| Variable | Effect |
|----------|--------|
| `CONFIG_PATH` | Path to the TOML config (default `./config.toml`). |
| `EXECUTION_MODE` | `paper` \| `simulate` \| `live`. |
| `ALLOW_LIVE_TRADING` | `true` to permit live broadcasts (second gate). |
| `RPC_URL` / `WS_URL` | Override Solana RPC / WebSocket endpoints. |
| `SOLANA_KEYPAIR` | Path, base58 secret, or JSON byte array for the Solana wallet. |
| `COPY_WALLETS` | Comma-separated pubkeys appended to `[copy].wallets`. |
| `POLYMARKET_PRIVATE_KEY` (or `POLYGON_PRIVATE_KEY`) | Polygon key for CLOB order signing. |
| `TELEGRAM_BOT_TOKEN` | Token for Module 5 (the *name* of this var is `[telegram].bot_token_env`). |
| `API_KEY` | Shared secret for mutating REST routes (`[api].api_key_env`). |
| `RUST_LOG` | Overrides `[observability].log_level` (full `EnvFilter` syntax, e.g. `info,solana_client=warn`). |
| `LOG_LEVEL` / `LOG_FORMAT` | Base level / `text` \| `json` (see `[observability]`). |
| `METRICS_ENABLED` | `true`/`false` — serves or hides `GET /metrics`. |
| `SAMPLE_INTERVAL_MS` | State-sampler period (≥ 100). |
| `GEYSER_WS_URL` | Yellowstone/Geyser websocket for the `transactionSubscribe` push feeds. |
| `ACCOUNT_CACHE_TTL_MS` | Warm-cache age for semi-static accounts (`0` disables; default 30 000). |
| `ACCOUNT_CACHE_MAX_ENTRIES` | Warm-cache capacity (FIFO eviction; default 5 000). |
| `SIMULATE_FIRST` / `ABORT_ON_SIMULATION_FAILURE` | Execution simulate policy (default `true`/`true`). |
| `BROADCAST_FANOUT` | Race sends across primary + fallback RPCs, first accept wins (default `false`). |

Secret-bearing config fields store the **name** of an env var (e.g.
`bot_token_env = "TELEGRAM_BOT_TOKEN"`), so keys never have to sit in the file.
You may also inline them under `[secrets]`, which the server re-exports into the
environment for the modules.

### Going live

Live execution requires **both** of these to be true:

```toml
[execution]
mode = "live"
allow_live_trading = true
```

…and, for the relevant modules, real key material (`SOLANA_KEYPAIR` for Solana,
`POLYMARKET_PRIVATE_KEY` for Polymarket). With `allow_live_trading = false`,
`live` requests are downgraded and never broadcast. `simulate` mode still builds
and RPC-simulates real transactions without sending them.

---

## Control-plane API

Served by `[api]` (default `0.0.0.0:8080`). Mutating routes require the
`x-api-key` header when `API_KEY` is set.

| Method | Path | Description |
|--------|------|-------------|
| GET | `/` | Embedded HTML dashboard. |
| GET | `/health` | **Liveness** probe: `{status, version, uptime_s}`. Always 200 while the process serves HTTP; checks no external dependency. |
| GET | `/ready` | **Readiness** probe: 200 when every component is ready, 503 otherwise; body is the full component report. |
| GET | `/metrics` | Prometheus text exposition (0.0.4). 404 when `metrics_enabled = false`. |
| GET | `/api/health` | Legacy compatibility alias (`{"ok":true}`). |
| GET | `/api/status` | Global summary: mode, kill switch, balances, PnL, per-module state. |
| GET | `/api/modules` | Enabled/running/detail for each module. |
| GET | `/api/positions` | Open positions. |
| GET | `/api/trades?limit=N` | Recent fills. |
| GET | `/api/config` | Redacted effective config snapshot. |
| GET | `/api/events` | **WebSocket** live event feed. |
| POST | `/api/kill` | Engage the kill switch (halt everything). |
| POST | `/api/resume` | Clear the kill switch. |
| POST | `/api/mode` | Body `{"mode":"paper\|simulate\|live"}`. |
| POST | `/api/modules/:name/enable` | Enable `sniper` \| `copy` \| `polymarket` \| `contract` \| `telegram`. |
| POST | `/api/modules/:name/disable` | Disable a module. |

The table lists the core routes; the complete reference (orders, audit +
hash-chain verify, API-key management, wallets, journal, recovery, db status)
is in [docs/API.md](docs/API.md).

The WebSocket (`/api/events`) streams every `AppEvent` as JSON tagged by
`kind`: `lifecycle`, `module_status`, `launch`, `signal`, `risk_rejected`,
`order_sent`, `fill`, `position_update`, `position_closed`, `wallet_trade`,
`polymarket`, `error`, `info`, `command`.

Every HTTP response carries an `x-request-id` header. An inbound
`x-request-id` is honoured when it is ≤ 128 chars of `[A-Za-z0-9-_]` and
replaced with a generated ID otherwise; the same ID appears in the request's
structured log line, so client, log and response always correlate.

---

## Observability

Configured by `[observability]` (see `config.toml.example`). Three pieces:

### Logs

* `log_format = "text"` — human-readable, for development.
* `log_format = "json"` — one JSON object per event (target/module, level,
  timestamp, span fields incl. `request_id`), for production log pipelines.
* Level: `RUST_LOG` env wins; otherwise `log_level` from config; invalid
  filters fall back to `info` (with a stderr notice).
* Exactly one `info` line per HTTP request (`method`, `route` pattern,
  `status`, `duration_ms`, `request_id`) — handlers stay quiet.

### Health & readiness

`GET /health` is **liveness**: process-only, always 200 while HTTP is served,
never reflects dependency state (a downstream outage must not get the process
restarted). `GET /ready` is **readiness**: 200 only when every component is
ready, else 503 with a JSON report:

```json
{
  "status": "degraded",
  "ready": false,
  "healthy": false,
  "uptime_secs": 123,
  "components": [
    { "name": "rpc",    "healthy": true,  "ready": true,  "detail": "consecutive_failures=0" },
    { "name": "sniper", "healthy": false, "ready": false, "detail": "running=false heartbeat_age_secs=none" }
  ]
}
```

Components: `rpc` (below the 3-consecutive-failure failover threshold) and the
three trading modules (`sniper`, `copy`, `polymarket`). A module is ready when
disabled (nothing to wait for) or when its loop is running **and** heartbeated
within the last 90 s. Telegram and the on-chain contract module do not gate
readiness. `detail` strings only ever contain booleans/counts/enum names —
never error payloads, URLs or key material.

### Metrics (Prometheus)

`GET /metrics`, text format 0.0.4, served by the same Axum server. All series
use stable `bot_*` names and **bounded label sets** (module names, execution
modes, matched route patterns, fixed outcome literals — never symbols,
wallets, signatures or paths). Recorded from the real execution paths:

| Metric | Type | Labels | Source |
|--------|------|--------|--------|
| `bot_build_info` | gauge=1 | `version` | sampler |
| `bot_uptime_seconds`, `bot_kill_switch`, `bot_open_positions`, `bot_event_subscribers`, `bot_execution_mode` (0=paper/1=simulate/2=live), `bot_health_ready`, `bot_rpc_consecutive_failures` | gauge | — | sampler |
| `bot_module_{enabled,running,connected,healthy,consecutive_errors}` | gauge | `module` | sampler |
| `bot_module_{events_seen,signals,orders_sent,orders_filled,orders_failed,risk_rejections}_total` | counter | `module` | sampler (mirrors authoritative `AppState` counters) |
| `bot_module_queue_depth` | gauge | `module` | decision-queue consumers (sniper launch feed, copy trade feed) |
| `bot_rpc_requests_total` | counter | `method`, `outcome` (`ok`/`fatal`/`exhausted`) | RPC retry chokepoint |
| `bot_rpc_attempt_duration_ms` | histogram | `method` | per attempt |
| `bot_ws_reconnects_total`, `bot_ws_connection_failures_total` | counter | — | WS supervisor |
| `bot_launches_total` | counter | `accepted` | event bus |
| `bot_execution_latency_ms` | histogram | `module`, `mode` | `OrderSent.latency_ms` |
| `bot_whale_trades_total`, `bot_polymarket_events_total` | counter | — | event bus |
| `bot_telegram_commands_total` | counter | `accepted` | event bus |
| `bot_app_errors_total` | counter | `module` (`none` if global), `fatal` | event bus |
| `bot_events_dropped_total` | counter | — | metrics pump lag |
| `bot_http_requests_total` | counter | `route`, `method`, `status` | middleware |
| `bot_http_request_duration_ms` | histogram | `route` | middleware |

Histogram buckets (ms): 5, 10, 25, 50, 100, 250, 500, 1000, 2500, 5000,
10000, 30000. `route` is the matched pattern (e.g. `/api/modules/:name/enable`),
so 404 probing cannot inflate cardinality. `metrics_enabled = false` removes
the `/metrics` surface (404) and skips HTTP instrumentation; the registry
itself is a set of atomics and stays live.

Prometheus scrape example:

```yaml
scrape_configs:
  - job_name: sniper-suite
    static_configs: [{ targets: ["localhost:8080"] }]
```

---

## Telegram control (Module 5)

Set `TELEGRAM_BOT_TOKEN`, add your chat/user IDs to `[telegram]`, and enable the
module. Authorization is **deny-by-default**: with empty allow-lists no commands
are accepted, and insufficient rights get an explicit refusal (never a silent
no-op). Roles mirror the API RBAC: `owner_user_ids` (full control incl.
`/mode live`), `allowed_user_ids`/`allowed_chat_ids` (operators — or owners
when no owner list exists, for backward compatibility), `readonly_user_ids`
(read commands only). Commands (an `@botname` suffix is stripped):

```
/help                     list commands
/status                   modules, PnL, kill switch
/on  <module|all>         enable  (sniper, copy, polymarket, contract, telegram)
/off <module|all>         disable
/kill                     engage kill switch
/resume                   clear kill switch
/positions                open positions
/trades                   recent fills
/pnl                      realized/unrealized + today
/balance                  wallet balances
/mode [paper|simulate|live]  show or set execution mode
/config                   key configuration
```

Alerts (fills, risk rejections, disconnects, daily-loss limit, hourly summary)
are configurable under `[telegram]` with cooldown and per-minute caps.

---

## Deploying the staking program (Module 4)

`programs/staking-suite` is a **native Solana program** (pure Rust, excluded
from the app workspace). It mints a reward token, holds a staking vault + fee
treasury (both ATAs), charges a deposit fee, and accrues rewards per second
(`reward_apy_bps`). Mint authority is the config PDA, so only the program can
mint rewards.

Build with the Solana toolchain (from inside the program dir — it is a
standalone crate with its own lockfile):

```bash
cd programs/staking-suite
cargo build-sbf                 # produces target/deploy/staking_suite.so
```

Deploy, then record the program id:

```bash
solana program deploy target/deploy/staking_suite.so
# => Program Id: <YOUR_PROGRAM_ID>
```

1. The program declares a fixed id in `lib.rs`
   (`declare_id!("3vEEMMFmdA88n8ApgZ3b9L3BXEh75yCeMbHbmUjR9mfy")`). Deploy
   under it with `solana program deploy target/deploy/staking_suite.so
   --program-id target/deploy/staking_suite-keypair.json`, or change the
   declared id + keypair to your own and rebuild.
2. Set `[contract] program_id = "<YOUR_PROGRAM_ID>"` in `config.toml`.
3. Call the `Initialize` instruction once (admin-signed) to create the mint,
   vault, treasury, and config with your `fee_bps`, `reward_rate_bps`,
   `min_stake`, `unstake_delay`, `decimals`, `timelock_secs`. The fee and
   reward rate are checked against hard caps (below) and the timelock against
   `[0, 30 days]`; a production deployment should use ≥ 24h.
4. Perform the **one-time genesis distribution**: `GenesisMint{amount}`
   (admin-only) mints the initial supply to a recipient token account and
   latches `Config::genesis_done` — any second attempt fails with
   `GenesisAlreadyDone` (6026), so supply can never be silently inflated
   after launch. Distribute from that wallet through your own sale/airdrop
   process; the program deliberately knows nothing about off-chain sales.
5. Users then `Stake` / `Unstake` / `Claim`. The admin can queue parameter
   changes with `UpdateParams` (applied by anyone via `ApplyParams` after the
   timelock, cancellable via `CancelParams`), `Pause` / `Unpause` deposits
   (withdrawals can never be paused), and hand over control with the
   two-step `TransferAdmin{new_admin}` → `AcceptAdmin`.

Instructions (borsh-encoded): `Initialize`, `Stake{amount}`, `Unstake`,
`Claim`, `UpdateParams{...}`, `ApplyParams`, `CancelParams`, `Pause`,
`Unpause`, `TransferAdmin{new_admin}`, `AcceptAdmin`, `GenesisMint{amount}`.
PDAs: config `["staking-config"]`, stake `["staking-stake", staker]`. Errors
map to `ProgramError::Custom(6000+)`. Client builders for every instruction
live in `staking_suite::instruction`. The full launch sequence and the
end-to-end test evidence are in [docs/STAKING.md](docs/STAKING.md).

### Security model

* **Account validation** — every trusted account is checked before use: the
  config must be the program's `["staking-config"]` PDA owned by the program
  and flagged initialized; a stake account must be the staker's
  `["staking-stake", staker]` PDA owned by the program and owned by the staker;
  the vault / mint / treasury must equal the addresses pinned in the config;
  the token / system / associated-token programs must be the canonical ids; and
  the staker's token account must be an SPL account of the config mint owned by
  the staker. Program PDAs sign via `invoke_signed` with their derivation seeds.
* **Parameter caps** — the deposit fee is capped at `MAX_FEE_BPS` (10%) and the
  annual reward rate at `MAX_REWARD_RATE_BPS` (100% APR); both `Initialize` and
  `UpdateParams` reject anything above, so a compromised admin cannot set a
  confiscatory fee or an inflationary mint rate.
* **Pause that cannot trap funds** — `Pause` halts *new deposits* only;
  `Unstake` and `Claim` are never gated, so the admin can stop inflow during an
  incident but can never freeze user funds.
* **Two-step admin transfer** — `TransferAdmin` records a `pending_admin`;
  control only moves when that key signs `AcceptAdmin`. This prevents losing
  the contract to a typo'd or unowned key. The zero pubkey is rejected.
* **Parameter timelock** — `UpdateParams` no longer changes anything
  immediately: it *queues* the resolved new values on-chain for the full
  `timelock_secs` window. Once the delay elapses, **anyone** may call
  `ApplyParams` (so a queued change can't be griefed by an unresponsive
  admin), and the admin may `CancelParams` before then. Changing the delay
  itself is queued like any other parameter and waits out the *old* delay
  (the OpenZeppelin `TimelockController` rule), so the timelock cannot be
  dropped instantly. Combined with never-gated withdrawals, users always get
  an exit window before any parameter change takes effect.
* **Multisig admin (external)** — `admin` is any signer, including one that
  signs via CPI, so the intended production setup is to initialize with the
  admin set to a **Squads or Realms multisig PDA** (M-of-N). The program
  deliberately does *not* embed its own M-of-N logic: reusing audited
  multisig infrastructure is the standard pattern and keeps this program's
  attack surface small.

> The program ships with host-side unit tests covering the validation layer
> (every rejection path), the parameter caps, pause, the two-step admin
> transfer, the full timelock flow (queue → wait → permissionless apply,
> cancel, delay-change semantics), state math, and instruction
> (de)serialization — **and** it is compiled to BPF (`cargo build-sbf`,
> agave 2.1.21 / platform-tools v1.43) and exercised end-to-end on a local
> `solana-test-validator` (`STAKING_E2E=1 cargo test --test validator_e2e`):
> initialize, guards, pause, timelock governance, and admin transfer all run
> on the BPF VM. Initial supply is distributed through the one-shot,
> admin-only `GenesisMint` instruction (latched by `genesis_done`), and the
> funded stake→reward→unstake money flow is proven end-to-end on the local
> validator. *(Verification context: the build-sbf + validator-e2e evidence
> was executed in earlier build sessions with agave 2.1.21 on this exact
> program source; it is **not** re-executed in every environment — the
> latest restored sandbox re-ran the 48 host tests, fmt, clippy and audit,
> while build-sbf/validator e2e run in the CI `program` job on every push.
> See docs/HANDOVER.md §3 for the full status taxonomy.)* The program has
> **not** had an external audit — **do not deploy to mainnet until an
> independent audit passes**.
* **Wallet & signer boundary (bot side)** — trading modules never touch key
  material: signing goes through the `TransactionSigner` abstraction and a
  named `SignerRegistry` (`primary_trading` plus optional configured
  identities). Multi-signer transactions are fully supported — every required
  signer must be declared (`extra_signers`) and resolvable, or the build
  fails with a structured error; nothing is silently skipped. `[signing]
  provider` selects the custody backend: `local` is implemented; `vault` /
  `kms` / `hsm` are configuration-level extension points that **fail
  startup** in this build (no silent fallback). See `docs/SECURITY.md`.

---

## Testing

```bash
# Application workspace (bot-core, solana-kit, all modules, server)
cargo test --workspace

# Real Postgres/Redis integration (skipped when the env vars are absent;
# CI runs them against service containers; --test-threads=1: shared stores):
POSTGRES_URL=postgres://user:pass@localhost:5432/db   cargo test -p bot-core --test db_integration -- --test-threads=1
REDIS_URL=redis://localhost:6379   cargo test -p bot-core --test redis_integration -- --test-threads=1
POSTGRES_URL=… REDIS_URL=…   cargo test -p bot-core --test distributed_integration -- --test-threads=1
POSTGRES_URL=…   cargo test -p module-copy --test two_replica_mirror -- --test-threads=1

# Module 4 (standalone crate, its own lockfile + target dir)
cd programs/staking-suite && cargo test
```

The default suite is fully offline and deterministic: instruction encoding,
EIP-712 digests, risk decisions, config parsing, state transitions,
observability (health/readiness, metrics registry, correlation IDs), plus
**integration tests against local mocks** of the external protocols —
PumpPortal WebSocket (sniper + copy feeds, reconnect/resubscribe), a
Yellowstone-style **Geyser `transactionSubscribe`** websocket (sniper launch
push + copy-trade push, incl. failed-tx skipping and poll fallback), a mock
JSON-RPC HTTP pair for the broadcast **fan-out** race, the Polymarket
CLOB/Gamma HTTP APIs (incl. L1/L2 auth headers and the signed order wire
format), and the storage journal (restart fidelity, corrupt-line recovery,
rotation) — **521 application workspace tests** (incl. 38 gated
Postgres/Redis/distributed/two-replica integration tests that skip cleanly
without `POSTGRES_URL`/`REDIS_URL` and run against real service containers in
CI), **48 program host tests + 2 validator e2e (gated `STAKING_E2E`)**.

### Network-gated end-to-end tests (off by default; CI never runs the devnet ones)

```bash
# Executor + RPC e2e against public devnet (read-only + paper; simulate/live
# skip gracefully when the public faucet rate-limits). E2E_URL overrides the
# cluster — point it at a local `solana-test-validator` to run everything,
# including the live broadcast → Confirmed loop, with no public side effects:
E2E_NETWORK=1 cargo test -p solana-kit --test devnet_e2e
E2E_NETWORK=1 E2E_LIVE=1 E2E_URL=http://127.0.0.1:8899 \
    cargo test -p solana-kit --test devnet_e2e

# Latency benchmarks (BUILD PLAN §5): p50/p95 for getSlot /
# getLatestBlockhash / simulateTransaction, plus the landing rate through the
# real executor (sequential vs fan-out). E2E_LIVE broadcasts valueless
# self-transfers from an ephemeral key — point E2E_URL at a local validator
# to keep it side-effect-free:
E2E_NETWORK=1 cargo test -p solana-kit --test latency_bench          # read-only benchmarks
E2E_NETWORK=1 E2E_LIVE=1 E2E_URL=http://127.0.0.1:8899 \
    cargo test -p solana-kit --test latency_bench                    # + landing rate

# Module 4 on-chain lifecycle: needs `cargo build-sbf` first and
# solana-test-validator (agave 2.1.x) on PATH — spawns its own validator:
cd programs/staking-suite
cargo build-sbf
STAKING_E2E=1 cargo test --test validator_e2e -- --test-threads=1
```

---

## Project layout

```
sniper-suite/
├─ Cargo.toml / Cargo.lock      workspace root (7 members; programs/ excluded)
├─ config.toml.example          annotated reference config
├─ docker-compose.yml           bot + Postgres 16 + Redis 7 stack
├─ .env.template                compose env template (copy to .env)
├─ Dockerfile / .dockerignore   multi-stage image for the server binary
├─ deny.toml                    cargo-deny policy (advisories/bans/sources)
├─ rust-toolchain.toml          pinned toolchain (1.98.1) — local + CI + image
├─ VERSION / CHANGELOG.md       release identity + history (LICENSE = MIT)
├─ SECURITY.md                  vulnerability-reporting policy
├─ scripts/release-check.sh     local release validation gate
├─ docs/                        27 docs: 13 engineering (architecture, API,
│                               security, ops, release, handover, backup/
│                               restore, testing…) + 14 buyer-package docs
│                               (index: docs/DELIVERY-MANIFEST.md)
├─ .github/workflows/ci.yml     fmt/clippy/build/test + services + sbf + docker
├─ crates/
│  ├─ core/            bot-core: config, state, events, risk, OMS, dedup,
│  │                   auth (RBAC), audit (hash chain), recovery, storage
│  │                   (JSONL journal), db/ (sqlx repos + migrations),
│  │                   redis_kv, obs/ (metrics + health registries)
│  ├─ solana-kit/      RPC, executor, wallet, pump/ray builders, decode
│  ├─ module-sniper/   Module 1
│  ├─ module-copy/     Module 2
│  ├─ module-polymarket/ Module 3
│  ├─ module-telegram/ Module 5
│  └─ server/          sniper-suite binary (Axum API + WS + dashboard;
│                      main.rs orchestration, persist.rs pumps, recon.rs
│                      truth sources, obs.rs probes/metrics, ws.rs feed)
└─ programs/
   └─ staking-suite/   Module 4 (on-chain BPF program)
```

### Docker

```bash
# Full stack (recommended): bot + postgres + redis, healthchecked, volumes
cp .env.template .env && $EDITOR .env
docker compose up --build -d

# Image alone
docker build -t sniper-suite .
docker run --rm -p 8080:8080 --env-file .env \
  -v "$PWD/config.toml:/app/config.toml:ro" \
  -v "$PWD/data:/app/data" \
  sniper-suite
```

The image builds only the server binary; Module 4 is compiled separately with
`cargo build-sbf` (above). Compose publishes the API on 127.0.0.1 by default
and keeps Postgres/Redis internal to the compose network.

---

## Disclaimer

This software is provided "as is", without warranty of any kind. Trading
crypto-assets and prediction markets carries substantial risk of loss. You are
solely responsible for compliance with the laws and terms of service of every
venue you connect to, and for the security of your keys. Test in paper mode
first. Nothing here is financial advice.
`````


----- COMPLETE FILE: CHANGELOG.md (7348 bytes, 126 lines) -----

`````markdown
# Changelog

All notable changes to this project are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project
adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

The canonical version lives in `[workspace.package].version` in the root
`Cargo.toml`; the `VERSION` file mirrors it and `scripts/release-check.sh`
fails the release if they ever disagree.

## [Unreleased]

### Added (buyer / due-diligence documentation pass — no source code changed)

- 14 buyer-package documents under `docs/`: BUYER-OVERVIEW,
  CAPABILITY-MATRIX, BUYER-DUE-DILIGENCE, IP-COMPONENTS, THIRD-PARTY,
  BUYER-DEPLOYMENT, ACCEPTANCE-CHECKLIST, RELEASE-NOTES-0.1.0, BUYER-FAQ,
  SCOPE-BOUNDARY, SUPPORT-HANDOVER, BUYER-RISK-REGISTER,
  TECHNICAL-DIFFERENTIATORS, DELIVERY-MANIFEST (index).
- README "Buyer / engineering handover" section; `docs/HANDOVER.md` §1 and
  `release-manifest.json` `docs_count` updated for the new document set
  (13 → 27 files under `docs/`). The frozen engineering tree (commit
  `0e139c3`) is untouched: no Rust, SQL, config or CI file changed.

## [0.1.0] — initial handover release

First complete, internally verified release of the suite. Delivered state
(full evidence trail in `AUDIT.md`, test inventory in `docs/TESTING.md`):

### Added

- **Module 1 — Sniper** (`module-sniper`): pump.fun launch detection
  (PumpPortal WS, Yellowstone-style Geyser `transactionSubscribe`, poll
  fallback) and entry execution with PumpSwap/Raydium/Jupiter exit routing.
- **Module 2 — Copy trading** (`module-copy`): tracked-wallet mirroring with
  per-wallet rules, sizing, staleness guards and mirrored exits.
- **Module 3 — Polymarket** (`module-polymarket`): Gamma + CLOB REST/WS
  integration with EIP-712 v2 order signing and CTF ERC-1155 balance reads.
- **Module 4 — Staking program** (`programs/staking-suite`): native Solana
  program — reward mint, vault + fee treasury, per-second APY accrual,
  parameter timelock (queue/apply/cancel), two-step admin transfer,
  pause-deposits-only, hard parameter caps, one-time latched `GenesisMint`.
- **Module 5 — Telegram control** (`module-telegram`): deny-by-default RBAC,
  kill switch, module on/off, rate-limited alerts.
- **Control plane** (`server`): Axum REST (23 REST endpoints over 21
  `/api` routes) + WebSocket event feed (`/api/events`) + 4 infra routes —
  28 endpoints documented route-by-route in `docs/API.md` +
  embedded dashboard; liveness/readiness probes; Prometheus metrics with
  bounded labels; request-ID correlation; per-IP and per-principal rate
  limits; refusal to bind non-loopback without API auth.
- **Core** (`bot-core`): typed config with validation and env overrides,
  global risk engine (capacity, exposure, daily-loss auto-disable), OMS state
  machine with idempotency keys, restart-safe dedup (memory/Redis/Postgres),
  hash-chained append-only audit trail, JSONL journal with rotation and
  corrupt-line tolerance, intent journal + startup reconciliation,
  Postgres repositories with 11 forward-only migrations.
- **Distributed execution ownership** (`docs/DISTRIBUTED.md`): one logical
  execution ⇒ at most one active owner ⇒ at most one money-moving submission.
  Claim stores (Postgres authoritative, Redis, memory), leases + epochs +
  fencing, handoff grace for ambiguous outcomes, cross-replica kill-switch /
  module-flag sync, position-book sync, cluster-wide `GlobalRiskOracle`
  (tighten-only), and the append-only `execution_claim_events` lineage table.
- **Solana kit** (`solana-kit`): RPC retry/failover/fan-out, WS supervision
  with resubscribe, account cache (TTL + FIFO bounds), pump/raydium/pumpswap
  instruction builders, transaction executor with simulate-first policy and
  signer registry (multi-signer safe).
- **Operations**: Dockerfile (multi-stage, non-root, healthchecked),
  docker-compose stack (Postgres 16 + Redis 7, healthcheck-gated),
  `.env.template`, single-workflow CI (fmt/clippy `-D warnings`/build/test
  with real service containers, staking `build-sbf` + validator e2e,
  cargo-audit + cargo-deny hard gates, docker image build + smoke test),
  `scripts/release-check.sh` local release gate, machine-readable
  `release-manifest.json`, and thirteen docs under `docs/`.

### Fixed (during the release-engineering pass, pre-tag)

- **Audit chain append serialization** — `AuditRepo::append` previously read
  the chain head with `SELECT … ORDER BY id DESC LIMIT 1 FOR UPDATE`, which
  does not serialize concurrent writers under READ COMMITTED (a blocked
  writer's snapshot never sees the winner's new head row → the chain forks
  and `/api/audit/verify` reports a false break). Appends are now serialized
  by a transaction-scoped advisory lock
  (`pg_advisory_xact_lock(hashtext('audit_events_chain'))`). Regression
  tests: concurrent-append linearization + reordered/missing/duplicate row
  detection (`db_integration`).
- Toolchain-pin drift: the Dockerfile built on `rust:1.82` and the CI
  `program` job on unpinned `stable`, contradicting the `rust-toolchain.toml`
  pin (1.98.1). Both now use 1.98.1; `scripts/release-check.sh` gates the
  three-way consistency.
- Removed the placeholder `repository` URL (`example.com/...`) from the
  workspace manifest; stale test counts and an undocumented route-subset
  table in README corrected.

### Fixed (engineering-freeze pass, pre-tag)

- **Telegram bot-token leak into error strings** — the Bot API embeds the
  token in every request URL and `reqwest::Error`'s `Display` appends
  ` for url (…)` on send errors, so failed Telegram calls put the token into
  tracing logs / audit detail / alert text. Every reqwest error mapping in
  `module-telegram` now strips the URL (`Error::without_url()`); regression
  test `error_strings_never_contain_the_bot_token` exercises all four API
  methods against a closed loopback port and fails if the token ever appears
  in an error string.
- **Unused dependencies removed** (verified zero code references before
  removal, `cargo check` + full gate re-run after): `tokio-util` (core,
  solana-kit, server), `sha3` (module-polymarket — EIP-712 uses
  `tiny-keccak`), `serde_with` (workspace entry no crate referenced). They
  remain in `Cargo.lock` only where still required transitively.
- **Release metadata drift corrected:** control-plane route count and docs
  count in this file now match the source (26 `.route()` registrations /
  28 documented endpoints; 13 docs); `release-manifest.json` added as the
  machine-readable delivery manifest and wired into `release-check.sh`
  (required file + version consistency).

### Verification status at cut

- 521 application workspace tests (incl. 38 gated Postgres/Redis/
  distributed/two-replica integration tests), 48+2 staking host/e2e-gated
  tests — 0 failures; `scripts/release-check.sh` 20/20 gates PASS; fmt, clippy `-D warnings`, cargo-audit, cargo-deny
  clean. Per-pass evidence and the honest NOT-EXECUTED /
  ENVIRONMENT-BLOCKED list: `docs/HANDOVER.md` and `docs/TESTING.md`.
- The staking program has **not** had an external security audit; the
  declared program id is a pre-deploy placeholder. Do not deploy to mainnet
  until an independent audit passes (see `docs/SECURITY.md`).

[0.1.0]: initial release — no previous tags exist.
`````


----- COMPLETE FILE: docs/HANDOVER.md (7812 bytes, 145 lines) -----

`````markdown
# Engineering handover

This document lets a receiving engineering team verify, run and maintain the
repository from a cold machine. It states exactly what was verified where,
and what was not.

## 1. What is being handed over

The complete source of a modular crypto trading system (5 modules + control
plane) at version `0.1.0` (see `VERSION`, `CHANGELOG.md`):

- `crates/` — 7-crate cargo workspace (bot-core, solana-kit, module-sniper,
  module-copy, module-polymarket, module-telegram, server).
- `programs/staking-suite/` — standalone native Solana program (own
  lockfile; built with `cargo build-sbf`, agave 2.1.21).
- `crates/core/migrations/` — 11 forward-only Postgres migrations (embedded
  in the binary; applied at startup when `auto_migrate` is on).
- `docs/` — 13 engineering documents: ARCHITECTURE, API, SECURITY,
  DEPLOYMENT, OPERATIONS, MODULES, STAKING, TESTING, RECONCILIATION,
  DISTRIBUTED, RELEASE, HANDOVER (this file), BACKUP-RESTORE — plus 14
  buyer/due-diligence documents added after the engineering freeze
  (BUYER-OVERVIEW, CAPABILITY-MATRIX, BUYER-DUE-DILIGENCE, IP-COMPONENTS,
  THIRD-PARTY, BUYER-DEPLOYMENT, ACCEPTANCE-CHECKLIST,
  RELEASE-NOTES-0.1.0, BUYER-FAQ, SCOPE-BOUNDARY, SUPPORT-HANDOVER,
  BUYER-RISK-REGISTER, TECHNICAL-DIFFERENTIATORS, DELIVERY-MANIFEST;
  index: `docs/DELIVERY-MANIFEST.md`). No source code changed in that pass.
- Deployment assets: `Dockerfile`, `docker-compose.yml`, `.dockerignore`,
  `.env.template`, `config.toml.example`, `.github/workflows/ci.yml`,
  `deny.toml`, `rust-toolchain.toml`, `scripts/release-check.sh`,
  `release-manifest.json` (machine-readable delivery manifest).
- `AUDIT.md` — the full historical audit/build trail with per-pass evidence.

No secrets, keys, credentials, private databases or build artifacts are part
of the repository (`.gitignore`/`.dockerignore` enforce; the tree was
scanned at release — see `scripts/release-check.sh`).

## 2. Verifying from zero (exact steps)

Requirements: Linux x86-64, ~2 GB RAM, ~20 GB disk, network access.

```bash
# Toolchain (rustup honors rust-toolchain.toml automatically):
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal
source ~/.cargo/bin/env          # or export PATH="$HOME/.cargo/bin:$PATH"

# Datastores (or use the compose stack / CI service containers):
#   PostgreSQL >= 16 on some port, Redis 7 on some port.
export POSTGRES_URL=postgres://postgres@127.0.0.1:5433/postgres
export REDIS_URL=redis://127.0.0.1:6379

# Full local release gate (fmt, check, clippy -D warnings, workspace tests,
# gated integration suites, staking host tests, audit, deny, consistency):
./scripts/release-check.sh
```

Equivalent manual matrix — the exact commands and their last known results
are in `docs/TESTING.md` §Layers and §"What is covered where".

Reproducibility evidence: the whole suite has been rebuilt and re-verified
**from source alone on a wiped machine** (fresh rustup install, PostgreSQL
16.4 compiled from the official tarball, Redis 7.2.10 compiled from source,
empty target directory, empty database), and re-gated on the final tree by
`scripts/release-check.sh` (**20/20 gates PASS**): workspace 521/521,
db_integration 23/23 (fresh + rerun + `pg_dump`→restore round-trip with the
full suite green on the restored database), redis 10/10, distributed 4/4,
two-replica 1/1, staking 50/50 host (gated e2e skipped — no validator),
fmt/clippy/audit/deny all clean. The earlier 518/518, db 21/21 figures
predate the two audit-chain regression tests added in the release pass
(see CHANGELOG "Fixed").

## 3. Verification status taxonomy (honest labeling)

| Label | Meaning |
|---|---|
| VERIFIED | Executed successfully in the most recent full pass in the handover environment (results in `AUDIT.md` final sections + `docs/TESTING.md`) |
| PREVIOUSLY VERIFIED | Executed successfully in an earlier build session on identical source, not re-executed in the latest restored environment |
| GATED | Runs automatically when its env var/dependency is present; skips cleanly otherwise |
| NOT EXECUTED / ENVIRONMENT-BLOCKED | Cannot run in the build sandbox; wired into CI or requires external resources |

Current classification:

- **VERIFIED (latest pass — release gate `scripts/release-check.sh`
  20/20):** all 521 workspace tests (incl. the 38 gated integration tests
  against real PG 16.4 + Redis 7.2.10), 50 staking host tests, fmt,
  `clippy -D warnings` (both cargo projects), `cargo check`, cargo-audit
  (both lockfiles), cargo-deny, migrations 0001–0011 applied on a fresh
  database, and a `pg_dump`→restore→full-suite round-trip on the
  restored database.
- **PREVIOUSLY VERIFIED (earlier sessions, identical source, agave 2.1.21
  toolchain):** `cargo build-sbf` of the staking program; the
  `STAKING_E2E=1` validator e2e (full on-chain lifecycle incl. funded
  stake→reward→unstake); `recon_crash_e2e` against a local
  solana-test-validator; `devnet_e2e` read-only against public devnet;
  `latency_bench` local-pipeline benchmarks.
- **NOT EXECUTED / ENVIRONMENT-BLOCKED:** Docker image build + container
  smoke (no daemon in sandbox — CI `docker` job executes both); CI itself
  (needs GitHub runners); funded/mainnet landing-rate runs (needs funded
  keys + explicit approval); external security audit (none exists —
  `docs/SECURITY.md`).

## 4. Operating it

- Quick start, configuration precedence, env overrides, going-live gates:
  `README.md`.
- Day-two runbook (incidents, degradation matrix, emergency stop, journal,
  audit trail): `docs/OPERATIONS.md`.
- Backup/restore and Redis-loss behavior: `docs/BACKUP-RESTORE.md`.
- Multi-replica deployment (claims/leases/fencing/flag sync + honest
  limits): `docs/DISTRIBUTED.md`.
- Reconciliation/source-of-truth model: `docs/RECONCILIATION.md`.

## 5. Handover fill-ins (deliberate placeholders — act before production)

These are the only intentional open items; each is labeled in-place:

1. **LICENSE copyright holder** — replace "sniper-suite authors" with the
   legal entity transferring/receiving the rights (note at the bottom of
   `LICENSE`).
2. **Staking program id** — `programs/staking-suite/src/lib.rs`
   `declare_id!` is a pre-deploy placeholder; deploy under it with the
   matching keypair or change id+keypair and rebuild (README §"Deploying the
   staking program"). Until deployed, Module 4 cannot run live.
3. **`repository` metadata** — the workspace `Cargo.toml` intentionally has
   no `repository` URL (the previous placeholder was removed); set it to the
   real remote when published.
4. **Security contact** — root `SECURITY.md` points at "the current
   repository owner's security contact"; publish a real address.
5. **External audit of the staking program** — mandatory before mainnet
   (stated in README, `docs/SECURITY.md`, `docs/STAKING.md`).

## 6. Maintenance invariants (do not regress)

- One logical execution ⇒ at most one active owner ⇒ at most one money-moving
  submission (`docs/DISTRIBUTED.md` §1). Any new money path must go through
  risk → claim → fence → intent journal → execute → finish(ambiguous?).
- Risk checks run before execution and no module may bypass the global risk
  engine; the `GlobalRiskOracle` may only tighten limits.
- Durable financial state lives in Postgres; Redis is coordination/cache
  only and may die without losing money-relevant truth.
- The audit trail is append-only from all app APIs; hash-chain verification
  is `GET /api/audit/verify`.
- `cargo clippy --workspace --all-targets -- -D warnings` is a hard gate, as
  are fmt, audit and deny. Keep it that way in CI.
- Never weaken or delete a test to make a gate pass; skipped-by-env tests
  must announce themselves (rule in `docs/TESTING.md`).
`````


----- COMPLETE FILE: release-manifest.json (5420 bytes, 91 lines) -----

`````json
{
  "manifest_version": 1,
  "product": "sniper-suite",
  "description": "Modular crypto trading suite: 5 modules (sniper, copy, polymarket, staking program, telegram control) + Axum control plane + distributed execution ownership",
  "version": "0.1.0",
  "license": "MIT",
  "notes": [
    "Machine-readable delivery manifest. Authoritative sources: VERSION (version), Cargo.lock + programs/staking-suite/Cargo.lock (dependency graph), AUDIT.md (evidence trail), docs/HANDOVER.md (verification taxonomy).",
    "No build timestamp is included (reproducibility). The release commit hash is deliberately NOT embedded: this file is part of the commit it would describe; the authoritative commit/tag is recorded in git history and the release notes.",
    "scripts/release-check.sh fails the release if this file is missing or its version disagrees with VERSION / Cargo.toml."
  ],
  "components": {
    "workspace_members": [
      "crates/core (bot-core)",
      "crates/solana-kit",
      "crates/module-sniper",
      "crates/module-copy",
      "crates/module-polymarket",
      "crates/module-telegram",
      "crates/server (sniper-suite binary)"
    ],
    "standalone_programs": [
      "programs/staking-suite (native Solana program, own lockfile, built with cargo build-sbf / agave 2.1.21)"
    ],
    "database_migrations": {
      "count": 11,
      "high_water_mark": "0011",
      "policy": "forward-only; no down migrations by design (docs/BACKUP-RESTORE.md)"
    },
    "api_endpoints_documented": 28,
    "docs_count": 27
  },
  "toolchain": {
    "rust": "1.98.1",
    "rust_pin_enforced_by": ["rust-toolchain.toml", "Dockerfile (rust:1.98.1-bookworm)", ".github/workflows/ci.yml (program job dtolnay/rust-toolchain@1.98.1)", "scripts/release-check.sh gate"],
    "solana_program_toolchain": "agave 2.1.21 (build-sbf) — PREVIOUSLY VERIFIED, not re-executed in the final sandbox"
  },
  "test_counts": {
    "workspace_total": 521,
    "workspace_gated_integration_executed": 38,
    "db_integration": 23,
    "redis_integration": 10,
    "distributed_integration": 4,
    "two_replica_mirror": 1,
    "staking_host": 48,
    "staking_validator_e2e_gated_skipped": 2,
    "release_check_gates": { "pass": 20, "fail": 0, "skip": 0 },
    "failures": 0
  },
  "verification_status": {
    "verified_final_pass": [
      "cargo fmt / cargo check / cargo clippy --workspace --all-targets -D warnings",
      "cargo test --workspace -- --test-threads=1 (521/521, gated suites executed against real PostgreSQL 16.4 + Redis 7.2.10)",
      "db_integration 23/23, redis_integration 10/10, distributed_integration 4/4, two_replica_mirror 1/1",
      "staking fmt + clippy -D warnings + host tests 48/48",
      "cargo audit (both lockfiles, 0 findings), cargo deny check (advisories/bans/licenses/sources ok)",
      "pg_dump -> restore -> full db_integration suite green on the restored database",
      "audit-chain tamper evidence: modification, reorder, missing, duplicate detection + linear chain under 8 concurrent appenders (advisory-lock serialization)",
      "telegram bot-token redaction in all API error paths (closed-port regression test)",
      "secret scan + TODO/stub-marker scan clean; migrations monotonic 0001-0011; version + toolchain-pin consistency"
    ],
    "previously_verified_identical_source": [
      "cargo build-sbf -> 5440-byte program binary (agave 2.1.21)",
      "STAKING_E2E=1 validator e2e 2/2 (stake lifecycle, timelock, two-step admin transfer, genesis-mint latch)",
      "recon_crash_e2e against a local solana-test-validator",
      "devnet_e2e read-only against public devnet",
      "latency_bench local-pipeline benchmarks",
      "deterministic ledger replay"
    ],
    "not_executed_environment_blocked": [
      "cargo build-sbf in the final sandbox (no Solana toolchain installed)",
      "validator e2e in the final sandbox (no solana-test-validator)",
      "devnet_e2e / funded live trading (no funded keypair; requires explicit approval)",
      "latency_bench (requires co-located measurement infrastructure)",
      "docker build + container smoke (no Docker daemon; Dockerfile/compose verified by static inspection only)",
      "GitHub Actions CI run (no CI runner; equivalent steps executed locally via scripts/release-check.sh)",
      "SBOM generation (cargo cyclonedx / cargo spdx not installed; command documented in docs/RELEASE.md; both Cargo.lock files are the authoritative dependency record)",
      "external security audit / penetration test / formal verification (none performed)"
    ]
  },
  "external_handover_blockers": [
    "Insert the legal copyright holder into LICENSE (currently the generic 'sniper-suite authors')",
    "Publish a real security contact (root SECURITY.md points at the repository owner's contact)",
    "Set the real repository URL in Cargo.toml when published (placeholder was removed)",
    "Deploy the staking program and replace the pre-deploy placeholder declare_id! (programs/staking-suite/src/lib.rs)",
    "Commission an independent external security audit before any mainnet deployment of the staking program",
    "Provide production infrastructure: PostgreSQL >= 16, Redis 7, funded keys, RPC/WS providers",
    "Execute Docker image build + CI on real runners (docker job, build-sbf, validator e2e)",
    "Funded live-trading validation under operator supervision (paper mode is the default)"
  ]
}
`````

---

## 6. DOCUMENTATION CONSISTENCY RESULTS

Checker: `/home/user/doccheck.py` (kept outside the repository — no tooling added to the delivered tree). It validates, across all 31 markdown files + manifest:

1. **Link resolution** — every relative markdown link target in the repo exists (checked all `.md` files, not just new ones): **0 broken links**.
2. **Invisible/bidi characters** — none (one zero-width space introduced during authoring of SUPPORT-HANDOVER.md was caught and fixed before the final run).
3. **Version consistency** — `release-manifest.json` version == `VERSION` file == `0.1.0`; no `sniper-suite X.Y.Z` string disagrees.
4. **Test-count consistency** — canonical freeze numbers present where referenced (521/521, 23/23, 10/10, 4/4, 1/1, 48/48, 20 PASS / 0 FAIL / 0 SKIP, commits 9c677cd + 0e139c3, 146 files, 2,801,590 bytes); stale counts (520/520, 518, 21/21) appear nowhere in the new docs (HANDOVER.md's *historical* "earlier 518/518 … predate" sentence is intentionally exempt).
5. **Manifest sanity** — `docs_count` (27) == actual files in `docs/`; `workspace_total` == 521.
6. **Referenced source paths** — all 50 backticked `crates/…`, `programs/…`, `scripts/…`, `.github/…` paths cited in the new docs exist on disk.
7. **File-count arithmetic** — repo now holds exactly 160 files = 146 frozen + 14 new.

**Final result: `DOC CONSISTENCY: CLEAN` (exit 0), 0 findings.**

## 7. VERIFICATION RESULTS

| Check | Result |
|---|---|
| Documentation consistency script | **PASS** — clean, per §6 |
| `cargo fmt --all --check` | **PASS** — executed with the pinned toolchain (rustup reinstalled 1.98.1 this session; rustfmt 1.9.0-stable); zero diff, confirming no Rust file changed |
| Code/config unchanged proof (byte-exact) | **PASS** — Rust files total 2,051,936 B == frozen 1,826,288 + 225,648 exactly; SQL total 23,443 B == frozen exactly; the 14 untouched engineering/root docs total 251,616 B == frozen docs category 293,952 − (README 28,311 + CHANGELOG 6,645 + HANDOVER 7,380) exactly; total growth 118,359 B == 116,140 (new docs) + 1,084 (README) + 703 (CHANGELOG) + 432 (HANDOVER) + 0 (manifest) exactly |
| `git diff --check` | **NOT EXECUTED** — the sandbox re-provision destroyed the `.git` metadata directory (file tree survived; verified above). No git state was invented. The buyer-side acceptance step "commit verified" (`docs/ACCEPTANCE-CHECKLIST.md`) must be performed against the seller's authoritative repository/bundle, which retains history `9c677cd` → `0e139c3`. |
| `./scripts/release-check.sh` | **NOT EXECUTED (deliberately)** — §21 of the directive: expensive engineering tests are not re-run when no code/config changed; byte-proof above shows none did. Additionally the sandbox lost the toolchain build cache, PostgreSQL 16.4 and Redis 7.2.10 instances (hours of rebuild). Last executed result on the identical source tree: **20 PASS / 0 FAIL / 0 SKIP, exit 0** (freeze run 7, `AUDIT.md` §27). The 4 modified files are markdown/JSON only; the release-check `docs_count`-adjacent gates (required-files, version consistency) are unaffected — `docs_count` is not gate-checked, version identity is and remains `0.1.0` in all three places. |
| Full workspace test suite | **NOT RE-RUN (deliberately)** — same justification; last result on identical source: 521/521 + all gated suites, 0 failures. |

Final tree: **160 files / 2,919,949 bytes (2.92 MB) / 79,813 lines** (documentation-only growth of 118,359 B / 1,833 lines over the frozen 146 / 2,801,590 / 77,980).

## 8. BUYER-ACTION ITEMS (post-transfer; none is a software defect)

1. Verify the delivered repository/bundle against commits `9c677cd` + `0e139c3` on receipt (`docs/BUYER-DEPLOYMENT.md` §2).
2. Insert the legal copyright holder into `LICENSE`.
3. Publish a real security contact in root `SECURITY.md`.
4. Set the real `repository` URL when publishing.
5. Deploy the staking program; finalize its program id; initialize with multisig admin + timelock ≥ 24h; plan the one-shot GenesisMint.
6. Commission the independent external security audit before any staking mainnet deployment.
7. Configure all secrets outside the repo; rotate every credential that existed on either side.
8. Run `./scripts/release-check.sh` green on buyer infrastructure.
9. Complete the paper → simulate → (optional, gradual, supervised) live validation sequence and the crash-recovery + backup/restore drills (`docs/ACCEPTANCE-CHECKLIST.md`).
10. Execute the Docker build + smoke and trigger CI on the buyer's runner (first real executions of those paths).

## 9. EXTERNAL INFRASTRUCTURE ITEMS

- PostgreSQL ≥ 16 production instance (+ backup pipeline).
- Redis 7 production instance.
- Solana RPC + WS provider(s) (fan-out/failover supported); optional Yellowstone-compatible Geyser provider; optional PumpPortal access.
- Polymarket API access + Polygon key (buyer must satisfy Polymarket ToS and applicable law).
- Telegram bot (BotFather token) + allow-listed chats/users.
- Hosting/orchestration, Prometheus-compatible monitoring, log pipeline, secret store, domains/reverse proxy/TLS.
- GitHub (or equivalent) CI runners; Docker daemon.
- Funded trading keys and capital-risk policy.

## 10. FINAL HANDOVER PACKAGE INDEX

Delivered tree (160 files):

- **Software:** 7-crate Rust workspace + standalone native Solana staking program + 11 migrations + 521/48+2 tests (frozen, untouched).
- **Release engineering:** `release-manifest.json`, `scripts/release-check.sh`, `rust-toolchain.toml`, `deny.toml`, both lockfiles, `Dockerfile`, `docker-compose.yml`, `.env.template`, `config.toml.example`, `.github/workflows/ci.yml`, `VERSION`, `LICENSE`, `SECURITY.md`.
- **Engineering docs (13):** ARCHITECTURE, API, SECURITY, DEPLOYMENT, OPERATIONS, MODULES, STAKING, TESTING, RECONCILIATION, DISTRIBUTED, RELEASE, HANDOVER, BACKUP-RESTORE + README, CHANGELOG, AUDIT.md.
- **Buyer package (14, this pass):** BUYER-OVERVIEW, CAPABILITY-MATRIX, BUYER-DUE-DILIGENCE, IP-COMPONENTS, THIRD-PARTY, BUYER-DEPLOYMENT, ACCEPTANCE-CHECKLIST, RELEASE-NOTES-0.1.0, BUYER-FAQ, SCOPE-BOUNDARY, SUPPORT-HANDOVER, BUYER-RISK-REGISTER, TECHNICAL-DIFFERENTIATORS, DELIVERY-MANIFEST (index of everything).

Classification of the delivered state:

- **VERIFIED (final freeze gate on identical source):** 521/521 workspace, db 23/23, redis 10/10, distributed 4/4, two-replica 1/1, staking host 48/48, release-check 20/20, fmt/clippy `-D warnings`/audit ×2/deny clean, pg_dump→restore round-trip, audit-chain tamper suite, telegram token-redaction regression, documentation consistency (this pass), cargo fmt (this pass).
- **PREVIOUSLY VERIFIED (identical source, earlier sessions):** build-sbf 5,440-byte .so (agave 2.1.21), validator e2e 2/2, recon_crash_e2e, devnet_e2e read-only, latency_bench, deterministic ledger replay.
- **BUYER ACTION REQUIRED:** §8 items 1–10.
- **EXTERNAL INFRASTRUCTURE REQUIRED:** §9 items.

**STOP CONDITION MET:** commercial docs complete, links valid, facts consistent, no software code changed (byte-proven), documentation consistency clean, fmt clean. `git diff --check` and `release-check.sh` re-execution are blocked/unnecessary for the stated reasons — both are recorded as NOT EXECUTED with justification, not silently skipped.
