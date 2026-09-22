# Capability matrix — sniper-suite 0.1.0 (final)

Every row maps to actual repository evidence. Column semantics:

- **Implementation** — what exists, in one phrase.
- **Source path** — where it lives in the tree.
- **Latest verification** — the most recent execution evidence and its result.
- **Verification type** — per the taxonomy in `docs/HANDOVER.md` §3:
  VERIFIED (final freeze gate on the frozen tree, 2026-09-18), PREVIOUSLY
  VERIFIED (earlier session, identical source), VERIFIED (static) for
  inspection/policy facts, NOT EXECUTED (never run anywhere; reason given).
- **Known limitation** — honest boundary of the claim.

Nothing is marked VERIFIED unless repository evidence supports it
(`AUDIT.md` §26–27, `docs/TESTING.md`, `release-manifest.json`,
`docs/EVIDENCE-INDEX.md`).

| Capability | Implementation | Source path | Latest verification | Verification type | Known limitation |
|---|---|---|---|---|---|
| Sniper (pump.fun launch detection + entry) | Launch feeds (PumpPortal WS, Geyser `transactionSubscribe`, poll fallback) + bonding-curve entry + PumpSwap/Raydium/Jupiter exit routing | `crates/module-sniper/src/{detect,entry,exit}.rs`; `crates/solana-kit/src/{pumpportal,events,ws,cache}.rs` | `detect_feed` + `geyser_detect` mock suites green within 521/521 workspace run (freeze gate) | VERIFIED | "~1s" launch-to-buy is a design target, not a guarantee; funded mainnet landing rate NOT EXECUTED (needs funded keys + explicit approval); `latency_bench` read-only + simulate legs EXECUTED vs public devnet in the hardening pass (`evidence/benchmarks-2026-09-18.json`); landing-rate leg still NOT EXECUTED (funded) |
| Copy trading | Tracked-wallet mirroring with per-wallet rules, sizing, staleness guards, mirrored exits | `crates/module-copy/src/{feeds,mirror,exit}.rs` | `copy_feed` + `geyser_feed` green in workspace run; `two_replica_mirror` 1/1 vs real PG+Redis | VERIFIED | Mirrors only operator-configured wallets; strategy quality is operator-owned |
| Polymarket (CLOB/Gamma, EIP-712 v2) | Gamma discovery, CLOB REST/WS, EIP-712 v2 11-field order signing, L1/L2 auth, CTF ERC-1155 balances; staged order pipeline with one risk decision, live order lifecycle (status poll + user websocket + cancel/TTL/reprice), local-vs-venue reconciliation, journaled restart recovery (migration 0014) | `crates/module-polymarket/src/{gamma,clob,ws,eip712,orders,auth,ctf,strategy,lib}.rs`; `crates/core/src/db/polymarket.rs`; `docs/POLYMARKET-ENGINE.md` | `mock_clob_gamma` (incl. auth headers + signed order wire format), `order_pipeline`, `order_lifecycle`, `user_ws`, `idempotency_concurrency`, `reconciliation`, `crash_recovery`, `strategy_sizing` — all against an in-process mock venue, green in workspace run | VERIFIED (against the mock venue) | Live order placement vs real Polymarket NOT EXECUTED (needs funded Polygon key); third-party API drift is an external risk |
| Telegram control | Deny-by-default RBAC bot: on/off, kill switch, mode, alerts; token-redacted error paths | `crates/module-telegram/src/{commands,alerts,api}.rs` | RBAC/command tests + `error_strings_never_contain_the_bot_token` regression green in workspace run | VERIFIED | Live round-trip needs the buyer's BotFather token; long-polling only (no webhook mode) |
| Staking program (on-chain) | Native Solana program: reward mint, vault + fee treasury, per-second APY, caps, timelock, two-step admin, pause-deposits-only, latched genesis mint, IMMUTABLE max-supply cap on every mint, one-shot immutable mpl token metadata | `programs/staking-suite/src/{lib,processor,state,instruction,error}.rs` | Host tests 71/71 (48/48 at freeze). Hardening pass 2026-09-18 ON THIS SOURCE: `build-sbf` EXECUTED (187,504-byte .so, SHA-256 `57a890fa…`, byte-identical rebuild) + **validator e2e 3/3 EXECUTED/PASSED** (160.72 s, agave 2.1.21 BPF VM, real mpl-token-metadata clone from mainnet-beta; funded stake→reward→claim→unstake, exact/over-cap boundaries, metadata + replay rejection) — `evidence/tests/phase5-full-batch.log`, `evidence/build/phase3-sbf-rebuild.log` | Host + build-sbf + all 3 e2e: VERIFIED BY EXECUTION (hardening pass) | `declare_id!` is a pre-deploy placeholder; NO external audit; mainnet deployment documentation-blocked until audit passes |
| Execution engine | Tx assembly, simulate-first policy, retry/failover chokepoint, optional broadcast fan-out, confirm tracking | `crates/solana-kit/src/{execute,tx,rpc}.rs` | Executor unit + fan-out mock-pair tests green in workspace run; `devnet_e2e` read-only vs public devnet; `recon_crash_e2e` vs local validator | Unit/mock: VERIFIED. devnet/crash e2e: PREVIOUSLY VERIFIED | Funded live broadcast landing-rate NOT EXECUTED |
| Risk engine | Global pre-trade checks (capacity, exposure, daily-loss auto-disable); rejection events/metrics; no module bypass | `crates/core/src/risk.rs` | Risk decision + rejection-path tests green in workspace run; bypass-freedom asserted by adversarial audit (`AUDIT.md` 2026-09-17) | VERIFIED | Limits are configuration; the engine enforces, it does not advise |
| Persistence (PostgreSQL truth) | sqlx repositories; 11 forward-only migrations embedded in binary; orders/executions/positions/trades/intents/claims/flags/audit | `crates/core/src/db/{repo,claims,mod}.rs`; `crates/core/migrations/0001`–`0011` | `db_integration` 23/23 vs real PostgreSQL 16.4 (fresh + rerun + pg_dump→restore→suite-green round-trip) | VERIFIED | No down migrations by design; PG ≥ 16 required |
| Redis (non-authoritative) | Dedup L2, claim coordination, runtime flags/cache; death loses no money-relevant truth | `crates/core/src/{redis_kv,redis_ownership,dedup}.rs` | `redis_integration` 10/10 vs real Redis 7.2.10 | VERIFIED | Redis loss degrades coordination to PG/memory paths (documented, tested) |
| Reconciliation | Record-before-send intents; startup replay; venue-truth resolution; ambiguity matrix; handoff grace; tx attribution | `crates/core/src/reconciliation.rs`; `crates/server/src/recon.rs`; migrations `0005`–`0008` | Reconciliation + OMS restart-recovery tests green (workspace + db_integration); `recon_crash_e2e` vs local validator | Tests: VERIFIED. crash e2e: PREVIOUSLY VERIFIED | Venue-truth resolution depends on RPC/Polymarket availability at reconcile time |
| Distributed execution ownership | Claims/leases/epochs/fencing; PG-authoritative claim store; kill-switch/flag sync; position-book sync; tighten-only GlobalRiskOracle; claim-event lineage | `crates/core/src/{ownership,redis_ownership}.rs`; `db/claims.rs`; migrations `0009`–`0011` | `distributed_integration` 4/4 + `two_replica_mirror` 1/1 (two real processes, shared PG+Redis) | VERIFIED | Tested at 2 replicas; larger topologies NOT EXECUTED |
| Observability | Structured logs + request-ID correlation; liveness/readiness probes; bounded-label `bot_*` Prometheus metrics | `crates/core/src/obs/{health,metrics}.rs`; `crates/server/src/obs.rs` | Health/readiness/metrics/correlation tests green in workspace run; 11 documented metric names verified against source (freeze audit) | VERIFIED | Prometheus text exposition only; no push-gateway/OTLP exporter |
| Control-plane API | 23 REST endpoints over 21 `/api` routes + WS feed + 4 infra routes (28 documented); embedded dashboard; rate limits; non-loopback bind refusal without auth | `crates/server/src/{api,ws,dashboard}.rs`; `docs/API.md` | Route/RBAC/rate-limit/WS tests green in workspace run | VERIFIED | Dashboard is a single embedded HTML file; no separate frontend build |
| RBAC / authorization | API-key gating on mutations; Telegram owner/operator/readonly; live-mode + key/journal mutations owner-only; readonly cannot mutate | `crates/core/src/auth.rs`; `crates/module-telegram/src/commands.rs` | Authz regression tests (owner-only live, operator≠owner, readonly refusal) green in workspace run | VERIFIED | API key is a shared secret; per-user API identities not implemented |
| Backup / restore | pg_dump/restore procedure + journal file handling; Redis disposable | `docs/BACKUP-RESTORE.md` | pg_dump → restore → full `db_integration` suite green on restored database | VERIFIED | Procedure, not automation: no scheduled-backup tooling included |
| CI pipeline | 4-job GitHub workflow: app (fmt/clippy/build/test vs PG16+Redis7 services), staking (host + build-sbf + gated e2e), security (audit×2/deny×2), docker (build + smoke) | `.github/workflows/ci.yml` | No runner execution from the delivery environment; every step has a local equivalent that passed in the freeze gate (release-check 20/20), and the hardening pass mapped every CI step 1:1 to executed local runs (`docs/CI-LOCAL-EQUIVALENCE.md`) | NOT EXECUTED (run); equivalent steps VERIFIED locally | Buyer must run CI on their own GitHub/adapted runner |
| Security scanning | cargo-audit (both lockfiles) + cargo-deny (advisories/bans/licenses/sources) as release + CI hard gates | `deny.toml`; `scripts/release-check.sh`; `.github/workflows/ci.yml` | cargo-audit 0.22.2: 0 findings ×2; cargo-deny 0.18.9: ok — in freeze gate | VERIFIED | RustSec DB is a point-in-time snapshot; buyer re-runs periodically |
| Docker packaging | Multi-stage non-root healthchecked image (rust:1.98.1-bookworm); compose stack (bot + postgres:16-alpine + redis:7-alpine), loopback API by default | `Dockerfile`, `docker-compose.yml`, `.dockerignore` | No Docker daemon in delivery sandbox; static inspection + `docker compose config` syntax gate (CI) only | NOT EXECUTED (build+smoke) | Buyer executes first image build + container smoke |
| Signer abstraction / registry | `TransactionSigner` + named `SignerRegistry`; multi-signer completeness enforced; `local` implemented; `vault`/`kms`/`hsm` fail startup | `crates/solana-kit/src/signer.rs` | Registry + missing-signer structured-failure tests green in workspace run | VERIFIED | Only `local` custody backend implemented; others are documented extension points |
| Geyser feeds | Yellowstone-style `transactionSubscribe` client with failed-tx skipping + poll fallback | `crates/solana-kit/src/events.rs`; module feed tests | `geyser_detect` + `geyser_feed` mock suites green in workspace run | VERIFIED (mocks) | Real Geyser provider is buyer-supplied; provider e2e NOT EXECUTED |
| Account cache | TTL (default 30 s) + FIFO-bounded (default 5 000) warm cache for semi-static accounts | `crates/solana-kit/src/cache.rs` | Cache unit tests green in workspace run | VERIFIED | Best-effort by design; staleness bounds are configuration |
| RPC fan-out / failover | Retry chokepoint, failover after 3 consecutive failures, `BROADCAST_FANOUT` race-send (first accept wins), per-method metrics | `crates/solana-kit/src/rpc.rs` | Mock JSON-RPC pair fan-out race + retry/failover tests green in workspace run | VERIFIED (mocks) | Production provider quality/rate limits are buyer-side |
| Recovery (crash/restart) | JSONL journal (rotation, corrupt-line tolerance) + startup replay + reconcile-before-new-work + 3-level dedup | `crates/core/src/{recovery,storage,dedup}.rs` | `storage_lifecycle` + db_integration restart-recovery green; `recon_crash_e2e` vs local validator | Tests: VERIFIED. crash e2e: PREVIOUSLY VERIFIED | Recovery resolves recorded intents; foreign-wallet activity out of scope |
| Audit chain | Append-only hash-chained trail; advisory-lock serialized appends; `GET /api/audit/verify`; no mutating app API | `crates/core/src/audit.rs`; `crates/core/src/db/repo.rs`; migration `0004` | Tamper detection (modification/reorder/missing/duplicate) + linear chain under 8 concurrent appenders green in db_integration | VERIFIED | App-level hash chain, not a blockchain; DB-superuser tamper + rehash is out of the stated threat model (`docs/SECURITY.md`) |

## Test-count summary (final freeze gate, frozen tree `0e139c3`, 2026-09-18)

Audit-pass update (current tree, 2026-09-18): workspace **537 / 537**,
staking host **71 / 71**, all other suites unchanged and green vs real
PostgreSQL 17.11 + Redis 8.0.2 — see `AUDIT.md` §28 / CHANGELOG
[Unreleased].

| Suite | Result |
|---|---|
| Workspace (`cargo test --workspace -- --test-threads=1`) | 521 / 521 passed (incl. 38 gated integration tests executed) |
| `db_integration` | 23 / 23 |
| `redis_integration` | 10 / 10 |
| `distributed_integration` | 4 / 4 |
| `two_replica_mirror` | 1 / 1 |
| **Buyer-hardening pass (2026-09-18, current tree)** | **537 / 537** workspace (and 537 / 537 with `--all-features`), staking host **71 / 71**, validator e2e **3 / 3 EXECUTED**, db backup→restore round-trip + app startup on restored DB |
| Staking host | 48 / 48 (+2 validator e2e gated-skipped in freeze sandbox; 2/2 PREVIOUSLY VERIFIED). Hardening pass 2026-09-18 on the audit-pass source: 71 / 71 host + **3 / 3 validator e2e EXECUTED and PASSED** (solana-test-validator 2.1.21, real mainnet-cloned mpl program) |
| `scripts/release-check.sh` | 20 PASS / 0 FAIL / 0 SKIP, exit 0 |
| fmt / clippy `-D warnings` / cargo-audit ×2 / cargo-deny | all clean |

Machine-readable copy: `release-manifest.json`. Claim → evidence map:
`docs/EVIDENCE-INDEX.md`. Per-suite detail: `docs/TESTING.md`. Historical
trail: `AUDIT.md`.
