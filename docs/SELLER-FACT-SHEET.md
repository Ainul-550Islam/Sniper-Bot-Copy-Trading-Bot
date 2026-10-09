# Seller fact sheet — sniper-suite 0.1.0

**Purpose:** a factual source document from which a seller can compose a
listing (Fiverr/Upwork/direct) or answer buyer questions. It is **not** an
advertisement and contains no price, revenue, ROI, client-count, user-count,
production-volume, latency-promise, or "enterprise" claim. Every line is
supported by the repository; the evidence pointer is given per section.
Companion source-material file: `docs/SELLING-LISTING-SOURCE.md`.
Fact base synchronized to the 2026-09-29 tree (see `archive/AUDIT.md`, the 2026-09-29
re-audit, and `PROMPT-3-RESULT.md`).

## Project name & version

- **sniper-suite**, version **0.1.0**, proprietary license (`LICENSE`, all rights reserved; earlier MIT wording is withdrawn). Evidence: `VERSION`, `Cargo.toml`,
  `LICENSE`, `release-manifest.json`.

## Architecture (facts)

- Rust, 8-crate cargo workspace + 1 standalone native Solana program
  (no Anchor). Evidence: root `Cargo.toml`, `programs/staking-suite/`.
- 5 functional modules (sniper, copy trading, Polymarket, staking program,
  Telegram control) behind one Axum control plane (REST + WebSocket +
  embedded dashboard) plus a Next.js 16 tenant console
  (`apps/control-plane`). Evidence: `crates/`, `apps/`,
  `docs/ARCHITECTURE.md`.
- PostgreSQL = durable financial truth (**<!-- stat:migrations -->54<!-- /stat --> forward-only migrations**, high water `<!-- stat:migrations_high_water -->0054<!-- /stat -->`); Redis = non-authoritative coordination/cache; JSONL intent
  journal for crash recovery. Evidence: `crates/core/migrations/`,
  `docs/BACKUP-RESTORE.md`, `release-manifest.json`.
- Tenant-scoped trading data plane: 17 trading-truth PK/arbiter surfaces are
  tenant-composite (migrations 0026–0034), every `/api/tenant/*` handler
  authenticates and scopes to one organization, and the repository layer
  carries `organization_id` in every SQL predicate. Evidence:
  `crates/core/src/trading_repository/`, `crates/server/src/trading_data_plane/`,
  `PROMPT-3-RESULT.md`.
- Distributed execution ownership: claims/leases/epochs/fencing enforce
  "one logical execution ⇒ ≤1 owner ⇒ ≤1 money-moving submission"
  (now tenant-composite since migration 0028). Evidence:
  `crates/core/src/ownership.rs`, `docs/DISTRIBUTED.md`.
- Current software tree (measured 2026-09-29): **744 files** (excluding
  `target/`, `.git/`, `node_modules/`, `buyer-release/`), **512 Rust files**
  (506 under `crates/` + 6 under `programs/staking-suite/`), **~196,048 Rust
  LOC**, 101 Markdown docs under `docs/`. Evidence: `release-manifest.json`
  (counts verified by `release_manifest_counts_and_version_are_current`).

## Modules (facts)

1. **Sniper** — pump.fun launch detection via PumpPortal WS, Geyser
   `transactionSubscribe`, or polling fallback; entries on the bonding curve;
   exits routed via PumpSwap/Raydium AMM v4/Jupiter. Evidence:
   `crates/module-sniper/`, `crates/solana-kit/src/{pump,pumpswap,raydium,jupiter}.rs`.
2. **Copy trading** — mirrors tracked wallets with per-wallet rules, sizing,
   staleness guards, optional mirrored exits; leader/event/link state is
   tenant-local since migration 0029. Evidence: `crates/module-copy/`,
   `crates/core/src/trading_repository/copy/`.
3. **Polymarket** — Gamma + CLOB REST/WS, EIP-712 v2 order signing (11-field
   Order), CTF ERC-1155 balance reads, L1/L2 auth, tenant-scoped mirror
   book/fills/reconciliation since migration 0030. Evidence:
   `crates/module-polymarket/`,
   `crates/core/src/trading_repository/polymarket/`.
4. **Staking program** — on-chain: reward mint, vault + fee treasury,
   per-second APY, deposit fee, hard caps (fee ≤ 10%, reward ≤ 100% APR),
   parameter timelock, two-step admin transfer, pause-deposits-only,
   one-shot latched genesis mint. Evidence: `programs/staking-suite/`,
   `docs/STAKING.md`.
5. **Telegram control** — deny-by-default RBAC (owner/operator/readonly),
   kill switch, module toggles, rate-limited alerts, token-redacted error
   paths. Evidence: `crates/module-telegram/`.

## Major engineering capabilities (facts)

- Global pre-trade risk engine, no module bypass (invariant + tests).
- OMS with deterministic idempotency keys; 3-level restart-safe dedup
  (idempotency namespaces are tenant-local since migration 0027).
- Intent journal + startup reconciliation with ambiguity handling and
  handoff grace (tenant-scoped since migration 0031).
- Append-only hash-chained audit trail with `GET /api/audit/verify` and
  tamper-detection tests (incl. 8 concurrent appenders).
- Signer abstraction: trading modules never touch key material; multi-signer
  completeness enforced; unimplemented `[signing] provider` backends fail
  startup; the multi-tenant custody boundary ships real Vault-transit and
  AWS-KMS (SigV4, Ed25519) adapters — unit-tested, fail-closed, not
  live-proven (`docs/CUSTODY-STATUS-2026.md`).
- RPC retry/failover + optional broadcast fan-out; WS supervision with
  resubscribe; TTL/FIFO-bounded account cache.
- Observability: liveness/readiness probes, bounded-label Prometheus
  metrics, request-ID correlated logs.
- Tenant worker lanes (migration 0032): per-organization leadership with
  fencing generations for the tenant pipelines.
- Safety defaults: paper mode, all modules disabled by default, strict
  config parsing, dual live gates, owner-only live switching.

## Test evidence (facts; labels matter)

- Static test inventory: <!-- stat:test_attrs_plain -->2061<!-- /stat --> #[test] and <!-- stat:test_attrs_tokio -->888<!-- /stat --> #[tokio::test] functions (a count, not a pass/fail result). External validations: <!-- stat:evidence_passed -->1<!-- /stat --> PASSED / <!-- stat:evidence_not_run -->18<!-- /stat --> NOT_RUN.
- No test-run logs ship in this repository. Earlier pass/fail figures
  (workspace, db_integration, saas-sdk, staking validator e2e) were quoted
  without logs and have been removed; run `cargo test --workspace` yourself
  or see `evidence/` for the validation harnesses and their NOT_RUN state.
- VERIFIED (2026-09-29): buyer package regenerated from the canonical tree
  and `verify-buyer-package.sh` PASS (the 2026-09-29 external re-audit had
  flagged the previously shipped package as stale — `archive/AUDIT.md` §4; the
  regeneration closes that finding).
- NOT EXECUTED: Docker build+smoke (no daemon), CI run (no runner), funded
  live trading, external audit (none exists).
- Evidence: `archive/AUDIT.md` (seller's internal engineering log),
  `docs/TESTING.md`, `docs/STATS.md`, `evidence/`, `docs/EVIDENCE-INDEX.md`.

## Documentation (facts)

- 101 Markdown documents under `docs/` (engineering docs, buyer/delivery
  docs, evidence index, registers) + README + CHANGELOG + archive/AUDIT.md (the
  2026-09-29 re-audit, installed verbatim) + PROMPT-2-RESULT.md +
  PROMPT-3-RESULT.md + SECURITY.md + LICENSE. Evidence:
  `docs/DELIVERY-MANIFEST.md`, `release-manifest.json` (`docs_files 101`).

## Deployment (facts)

- Docker: multi-stage non-root image + compose stack (bot + PG + Redis),
  healthcheck-gated. Bare metal: pinned toolchain build, external PG/Redis.
  Multi-replica: same binary, N processes, ownership per `docs/DISTRIBUTED.md`.
- One-command local release gate (`scripts/release-check.sh`) and
  one-command bundle checks (`scripts/verify-delivery.sh`,
  `scripts/verify-buyer-package.sh`, `scripts/final-release-check.sh`).
- CI workflow delivered (4 jobs) — runs on the buyer's runner.

## Current limitations (facts — state these in any listing)

- No external security audit of any component; staking mainnet deployment is
  documentation-blocked until one passes.
- Staking program not deployed anywhere; `declare_id!` is a placeholder.
- Funded live trading never executed; the ~1s sniper figure is a design
  target, not a measured guarantee (local latency benchmarks are PREVIOUSLY
  VERIFIED only).
- Docker/CI paths never executed from the delivery environment.
- Multi-replica tested at 2 replicas only.
- Tenant architecture is two-layer complete but not three-layer complete:
  the SaaS control plane and the tenant-scoped trading data plane
  (repositories + `/api/tenant/*` routes + PostgreSQL schema) are
  implemented and isolation-tested against a live database, but the five
  trading module crates are not yet tenant-context-wired — module startup
  is still process-level (one deployment runs one module set; per-tenant
  isolated runtimes are not yet built). Evidence: `archive/AUDIT.md` §11.
- Remote custody: the Vault-transit and AWS-KMS adapters are REAL
  implemented code (unit-tested wire protocols; KMS SigV4 verified against
  the AWS-documented test vector), fail-closed, with no local fallback —
  but no live round-trip has been performed (LIVE_TEST remains buyer-side,
  `LIVE_CUSTODY=1`). HSM remains an explicitly fail-closed unimplemented
  provider. Evidence: `docs/CUSTODY-STATUS-2026.md`; `archive/AUDIT.md` §13.8
  remediation note (the 2026-09-29 audit predates the adapters).
- Billing: manual provider implemented; Stripe/Paddle adapters return 501
  until configured; no self-service checkout/invoicing/tax. Evidence:
  `docs/SAAS-PRODUCT.md`.
- Polymarket integration is EIP-712 V2-centered; Exchange V3
  position-backed orders and async `tradeIDs` response handling are not
  implemented. Evidence: `archive/AUDIT.md` §8/§16.
- Third-party venues (pump.fun, PumpSwap, Raydium, Jupiter, Polymarket,
  PumpPortal, Telegram) can change their protocols/APIs; integration
  maintenance is an ongoing cost.

## Buyer responsibilities (facts)

- Infrastructure: PostgreSQL ≥ 16, Redis 7, RPC/WS (+ optional Geyser/
  PumpPortal) providers, hosting, monitoring, secret store, CI runners,
  Docker daemon, domains/TLS.
- Credentials & accounts: funded keys, Polymarket access + Polygon key,
  Telegram bot, provider accounts — all re-contracted and rotated at
  transfer.
- Legal: copyright-holder insertion, regulatory review for their
  jurisdiction(s), venue ToS compliance, external audit commissioning,
  custody policy, live-trading approval.
- Validation: run the release gate, then paper → simulate → gradual
  supervised live validation + recovery/backup drills.
- Evidence: `docs/SCOPE-BOUNDARY.md`, `docs/ACCEPTANCE-CHECKLIST.md`,
  `docs/BUYER-RISK-REGISTER.md`.

## Ownership-transfer items (facts)

Repository + Git hosting, LICENSE holder, staking program authority (deploy
keypair, multisig admin, timelock, genesis plan), deployment credentials,
RPC/Geyser/PumpPortal accounts, Polymarket credentials, Telegram bot,
monitoring, domains, CI secrets, Docker registry, backups. Full checklist:
`docs/IP-COMPONENTS.md` §"Ownership transfer checklist",
`docs/SUPPORT-HANDOVER.md`.

## What this fact sheet deliberately does NOT say

No sale price or valuation; no revenue/ROI projection; no client, user, or
volume counts (there are none); no latency or profit guarantees; no
"enterprise-grade"/"production-proven" labels (no production deployment
evidence exists); no claim that any external audit passed (none exists); no
claim of ownership of any third-party protocol, API, or brand; no claim of
full multi-tenant runtime isolation (the module layer is not yet
tenant-wired — see limitations).
