# Selling listing source material — sniper-suite 0.1.0

**Source document only.** Factual, reusable material for composing a listing
or buyer communication. It is not itself a listing, and nothing here may be
turned into deceptive marketing copy: no price/revenue/ROI claims, no fake
customers or volumes, no latency or profit guarantees, no "enterprise" or
"production-proven" labels, no external-audit claims. Every statement below
is repository-backed (pointers in `docs/SELLER-FACT-SHEET.md` and
`docs/EVIDENCE-INDEX.md`).

## Title candidates (factual, non-deceptive)

1. "sniper-suite 0.1.0 — Rust modular crypto trading system (5 modules +
   control plane) with Solana staking program source, 537 tests, full
   handover documentation"
2. "Rust trading suite: pump.fun sniper, copy trading, Polymarket CLOB,
   Telegram control, native Solana staking program — verified test evidence,
   MIT source transfer"
3. "Complete Rust crypto-trading codebase (7 crates + on-chain program) —
   paper-safe defaults, distributed execution ownership, audit chain, 20/20
   release gates, buyer due-diligence package included"

Rules for any derived title: it may state component names, test counts, and
delivered artifacts; it may not state performance guarantees, profitability,
audit status, or deployment status that the repository does not evidence.

## One-paragraph technical summary

sniper-suite is a Rust implementation of a modular crypto trading system:
five cooperating modules (pump.fun launch sniper with PumpSwap/Raydium/
Jupiter exit routing, wallet copy trading, Polymarket CLOB trading with
EIP-712 v2 order signing, a native Solana staking/reward program, and
deny-by-default Telegram remote control) behind a single Axum control plane
with REST, WebSocket, dashboard, Prometheus metrics, and liveness/readiness
probes. Financial truth lives in PostgreSQL (11 forward-only migrations);
Redis is non-authoritative. Money paths follow one invariant pipeline —
risk → ownership claim → idempotency → intent journal → execution →
persistence → reconciliation → append-only hash-chained audit — and
multi-process deployments enforce "one logical execution, at most one owner,
at most one money-moving submission" via claims, leases, epochs and fencing
tokens. The system defaults to paper trading and requires two explicit
configuration gates plus real keys before anything broadcasts. Delivered
state: 537/537 workspace tests (hardening-pass re-execution against real
PostgreSQL/Redis; 521/521 at freeze), staking program built with
cargo build-sbf and all 3 validator e2e executed/passed on
solana-test-validator 2.1.21, a 20-gate release check, cargo-audit/deny
clean, and a complete buyer due-diligence documentation package. No external security
audit exists and the staking program is undeployed — both are documented,
not hidden.

## Feature facts

- 5 modules + control plane; 7 workspace crates + 1 standalone on-chain
  program crate.
- Sniper feeds: PumpPortal WS, Yellowstone-style Geyser
  `transactionSubscribe`, poll fallback; exits: PumpSwap, Raydium (v1+v2
  swap instructions), Jupiter.
- Copy trading: per-wallet rules, sizing, staleness guards, mirrored exits;
  two-replica mirroring tested with real processes.
- Polymarket: Gamma discovery, CLOB REST/WS, EIP-712 v2 signing (11-field
  Order, type-3 signature wrap), L1/L2 auth headers, CTF ERC-1155 balances.
- Staking program: per-second APY accrual, deposit fee, hard caps
  (fee ≤ 10%, reward ≤ 100% APR), queue/apply/cancel parameter timelock
  with permissionless apply, pause-deposits-only, two-step admin transfer,
  one-shot latched genesis mint.
- Telegram: owner/operator/readonly roles, kill switch, module toggles,
  rate-limited alerts, provable bot-token redaction in error paths.
- Control plane: 28 documented endpoints; API-key auth on mutations;
  refusal to bind non-loopback without auth; per-IP and per-principal rate
  limits; request-ID correlation; embedded dashboard.
- Safety architecture: paper default, dual live gates, simulate mode
  (real tx build + RPC simulation, no broadcast), signer registry boundary,
  strict config parsing, fail-startup for unimplemented custody backends.

## Architecture facts

- Event-bus core; supervised modules in one binary; ordered startup/
  shutdown; graceful degradation surfaced through readiness.
- PostgreSQL authoritative (orders, executions, positions, trades, intents,
  claims, flags, audit chain); Redis coordination-only; JSONL journal for
  crash recovery; 3-level dedup.
- Reconciliation: record-before-send intents, startup replay, venue-truth
  resolution, ambiguity matrix, handoff grace, transaction attribution.
- Distributed ownership: PG/Redis/memory claim stores, leases + epochs +
  fencing, tighten-only global risk oracle, claim-event lineage table.
- Observability: `bot_*` Prometheus metrics with bounded labels, JSON/text
  structured logs, one log line per HTTP request.

## Testing facts

- 537/537 workspace tests at hardening, 521/521 at freeze (offline-deterministic core + protocol mocks:
  PumpPortal WS, Geyser WS, JSON-RPC pair, CLOB/Gamma HTTP, journal).
- Gated integration vs real services: db 23/23, redis 10/10, distributed
  4/4, two-replica mirror 1/1 (all executed in the final freeze gate).
- Staking: 48/48 host tests; build-sbf (5,440-byte .so) + validator e2e 2/2
  incl. funded stake→reward→unstake — previously verified on identical
  source (agave 2.1.21).
- Audit chain: tamper detection (modify/reorder/missing/duplicate) + linear
  chain under 8 concurrent appenders.
- Recovery: journal restart fidelity, corrupt-line tolerance, OMS
  restart-recovery, pg_dump→restore→full-suite-green.
- Gates: fmt, clippy `-D warnings` (both projects), cargo-audit ×2
  (0 findings), cargo-deny, `release-check.sh` 20/20.
- Honest gaps: Docker build/smoke and CI runs not executed in the build
  environment; funded live trading never executed; >2-replica topology
  untested.

## Deployment facts

- Dockerfile: multi-stage, non-root, healthchecked, pinned
  `rust:1.98.1-bookworm`; compose: bot + postgres:16-alpine +
  redis:7-alpine, healthcheck-gated, loopback API by default.
- Bare metal: rustup honors `rust-toolchain.toml`; external PG ≥ 16 +
  Redis 7; single `config.toml` + env secrets.
- CI: 4-job GitHub workflow (app tests with service containers, staking
  program incl. build-sbf + gated validator e2e, security audit/deny,
  docker build + smoke).
- Ops: one-command release gate (`scripts/release-check.sh`), one-command
  bundle check (`scripts/verify-delivery.sh`), runbook
  (`docs/OPERATIONS.md`), backup/restore (`docs/BACKUP-RESTORE.md`),
  demo scripts for buyer walkthroughs (`docs/DEMO-RUNBOOK.md`).

## Security facts

- Paper-by-default; two independent live gates; owner-only live switching.
- Secrets env-only; secret-scan release gate; redacted config endpoint;
  bounded metric labels; Telegram token redaction regression test.
- Signer boundary: modules never hold keys; multi-signer completeness
  enforced; unimplemented custody backends fail startup.
- Append-only hash-chained audit; app APIs cannot mutate audit rows.
- Supply chain: two committed lockfiles; deny policy (14 permissive license
  allow-list, crates.io-only sources, no git deps); audit 0 findings at
  freeze.
- **No external audit exists**; staking mainnet deployment documentation-
  blocked until one passes.

## Buyer deliverables (what transfers)

- Full source (workspace + staking program + migrations + tests).
- Deployment assets (Docker, compose, env/config templates, CI workflow).
- Release engineering (release-check, verify-delivery, manifest, toolchain
  pin, deny policy, lockfiles).
- Documentation set: 13 engineering docs + 23 buyer/delivery docs +
  README/CHANGELOG/AUDIT/SECURITY/LICENSE/VERSION.
- Authoritative git history (`9c677cd` → `0e139c3` + documentation commits)
  via the seller's repository/bundle.

## Limitations (must appear in any honest listing)

- No external security audit / pentest / formal verification (any component).
- Staking program undeployed; placeholder program id.
- Funded live trading never executed; no profit/latency guarantee.
- Docker/CI not executed from the delivery environment.
- Two-replica testing ceiling; single-tenant single-operator design.
- Third-party venue/API drift is an ongoing maintenance cost.
- Placeholders requiring legal action: LICENSE holder, repo URL, security
  contact.

## Transfer requirements

1. Legal: IP assignment + LICENSE holder insertion; security contact
   publication.
2. Repository: transfer of the authoritative repo/hosting with real history.
3. Credentials: rotation of everything (keys, tokens, DB passwords, API
   keys, provider accounts) per `docs/SUPPORT-HANDOVER.md` §6.
4. Accounts: RPC/Geyser/PumpPortal/Polymarket/Telegram/registry/monitoring/
   domains re-contracted in the buyer's name.
5. Technical acceptance: buyer runs `verify-delivery.sh` +
   `release-check.sh` green, then the `docs/ACCEPTANCE-CHECKLIST.md`
   sequence (paper → simulate → optional supervised live; staking gated on
   external audit).
