# Seller fact sheet — sniper-suite 0.1.0

**Purpose:** a factual source document from which a seller can compose a
listing (Fiverr/Upwork/direct) or answer buyer questions. It is **not** an
advertisement and contains no price, revenue, ROI, client-count, user-count,
production-volume, latency-guarantee, or "enterprise" claim. Every line is
supported by the repository; the evidence pointer is given per section.
Companion source-material file: `docs/SELLING-LISTING-SOURCE.md`.

## Project name & version

- **sniper-suite**, version **0.1.0**, MIT license (copyright-holder line is
  a documented transfer placeholder). Evidence: `VERSION`, `Cargo.toml`,
  `LICENSE`, `release-manifest.json`.

## Architecture (facts)

- Rust, 7-crate cargo workspace + 1 standalone native Solana program
  (no Anchor). Evidence: root `Cargo.toml`, `programs/staking-suite/`.
- 5 functional modules (sniper, copy trading, Polymarket, staking program,
  Telegram control) behind one Axum control plane (REST + WebSocket +
  embedded dashboard). Evidence: `crates/`, `docs/ARCHITECTURE.md`.
- PostgreSQL = durable financial truth (11 forward-only migrations);
  Redis = non-authoritative coordination/cache; JSONL intent journal for
  crash recovery. Evidence: `crates/core/migrations/`,
  `docs/BACKUP-RESTORE.md`.
- Distributed execution ownership: claims/leases/epochs/fencing enforce
  "one logical execution ⇒ ≤1 owner ⇒ ≤1 money-moving submission".
  Evidence: `crates/core/src/ownership.rs`, `docs/DISTRIBUTED.md`.
- Frozen software tree: 146 files / 2,801,590 bytes / 77,980 lines
  (measured). Evidence: `docs/FINAL-DELIVERY.md` §3.

## Modules (facts)

1. **Sniper** — pump.fun launch detection via PumpPortal WS, Geyser
   `transactionSubscribe`, or polling fallback; entries on the bonding curve;
   exits routed via PumpSwap/Raydium/Jupiter. Evidence:
   `crates/module-sniper/`, `crates/solana-kit/src/{pump,pumpswap,raydium,jupiter}.rs`.
2. **Copy trading** — mirrors tracked wallets with per-wallet rules, sizing,
   staleness guards, optional mirrored exits. Evidence: `crates/module-copy/`.
3. **Polymarket** — Gamma + CLOB REST/WS, EIP-712 v2 order signing (11-field
   Order), CTF ERC-1155 balance reads, L1/L2 auth. Evidence:
   `crates/module-polymarket/`.
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
- OMS with deterministic idempotency keys; 3-level restart-safe dedup.
- Intent journal + startup reconciliation with ambiguity handling and
  handoff grace.
- Append-only hash-chained audit trail with `GET /api/audit/verify` and
  tamper-detection tests (incl. 8 concurrent appenders).
- Signer abstraction: trading modules never touch key material; multi-signer
  completeness enforced; unimplemented custody backends fail startup.
- RPC retry/failover + optional broadcast fan-out; WS supervision with
  resubscribe; TTL/FIFO-bounded account cache.
- Observability: liveness/readiness probes, bounded-label Prometheus
  metrics, request-ID correlated logs.
- Safety defaults: paper mode, all modules disabled by default, strict
  config parsing, dual live gates, owner-only live switching.

## Test evidence (facts; labels matter)

- VERIFIED (final freeze gate, 2026-09-18, on the frozen tree): 521/521
  workspace tests incl. 38 gated integration tests against real PostgreSQL
  16.4 + Redis 7.2.10; db 23/23; redis 10/10; distributed 4/4; two-replica
  1/1; staking host 48/48; pg_dump→restore→suite-green round-trip; fmt;
  clippy `-D warnings`; cargo-audit ×2 (0 findings); cargo-deny;
  `release-check.sh` 20/20.
- VERIFIED BY EXECUTION (buyer-hardening pass 2026-09-18, current tree):
  `build-sbf` 187,504-byte .so (SHA-256 `57a890fa…`, byte-identical
  rebuild); validator e2e 3/3 incl. funded stake→reward→claim→unstake and
  cap/metadata vs the real mpl clone; workspace 537/537 (also
  `--all-features`); db backup→restore round-trip + app startup on the
  restored DB; latency-bench read-only + simulate legs.
- PREVIOUSLY VERIFIED (earlier sessions, pre-hardening source):
  crash-recovery e2e vs local validator; read-only devnet e2e.
- NOT EXECUTED: Docker build+smoke (no daemon), CI run (no runner), funded
  live trading, external audit (none exists), dedicated SBOM-generator
  output (cargo metadata/tree + lockfile hashes provided instead,
  `evidence/sbom/`).
- Evidence: `AUDIT.md` §26–27, `docs/TESTING.md`, `release-manifest.json`,
  `docs/EVIDENCE-INDEX.md`.

## Documentation (facts)

- 13 engineering docs (architecture, API, security, deployment, operations,
  modules, staking, testing, reconciliation, distributed, release, handover,
  backup/restore) + 23 buyer/delivery docs (overview, capability matrix,
  due-diligence, IP inventory, third-party inventory, deployment handover,
  acceptance checklist, release notes, FAQ, scope boundary, support model,
  risk register, differentiators, fact sheets, demo runbook, evidence index,
  repository map, archive checklist, delivery index/manifest, quickstart,
  listing source) + README + CHANGELOG + AUDIT.md (evidence trail) +
  SECURITY.md + LICENSE. Evidence: `docs/DELIVERY-MANIFEST.md`.

## Deployment (facts)

- Docker: multi-stage non-root image + compose stack (bot + PG16 + Redis7),
  healthcheck-gated. Bare metal: pinned toolchain build, external PG/Redis.
  Multi-replica: same binary, N processes, ownership per `docs/DISTRIBUTED.md`.
- One-command local release gate (`scripts/release-check.sh`, 20 gates) and
  one-command bundle check (`scripts/verify-delivery.sh`, no toolchain
  needed).
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
- Single-operator, single-tenant: one deployment = one owner/config/keyset;
  RBAC separates roles, not tenants.
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

Repository + Git hosting (authoritative history `9c677cd` → `0e139c3`),
LICENSE holder, staking program authority (deploy keypair, multisig admin,
timelock, genesis plan), deployment credentials, RPC/Geyser/PumpPortal
accounts, Polymarket credentials, Telegram bot, monitoring, domains, CI
secrets, Docker registry, backups. Full checklist:
`docs/IP-COMPONENTS.md` §"Ownership transfer checklist",
`docs/SUPPORT-HANDOVER.md`.

## What this fact sheet deliberately does NOT say

No sale price or valuation; no revenue/ROI projection; no client, user, or
volume counts (there are none); no latency or profit guarantees; no
"enterprise-grade"/"production-proven" labels (no production deployment
evidence exists); no claim that any external audit passed (none exists); no
claim of ownership of any third-party protocol, API, or brand.
