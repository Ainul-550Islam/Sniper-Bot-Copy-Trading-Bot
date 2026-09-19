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
exists for any component. The staking program is host-tested (71/71 in the
audit pass; 48/48 at freeze), compiled to BPF with `cargo build-sbf`
(Agave 2.1.21 / platform-tools v1.43, hardening pass 2026-09-18), and ALL
THREE validator e2e tests were EXECUTED and PASSED against a real
`solana-test-validator` on the audit-pass source (3/3, 160.72 s batch run;
incl. funded stake→reward→unstake and the max-supply/metadata e2e against
the real mainnet-cloned mpl-token-metadata program). Executing them found
and fixed two real defects (mpl CPI discriminant 19→33; validator
`--clone-upgradeable-program` harness flag) — see AUDIT.md §29. Mainnet deployment is
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
VERIFIED = executed successfully in the latest gate pass on the current tree
(audit pass: 537/537, real PG 17.11 + Redis 8.0.2; freeze pass: 521/521,
gate 20/20, real PG 16.4 + Redis 7.2.10). PREVIOUSLY VERIFIED =
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
