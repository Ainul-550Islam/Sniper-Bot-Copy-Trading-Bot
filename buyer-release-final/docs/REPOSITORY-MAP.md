# Repository map — sniper-suite 0.1.0

The actual delivered tree (no invented directories). Generated from the real
file listing; counts are exact. Annotated by role.

```
sniper-suite/
│
│  ── root metadata & release identity ──────────────────────────────
├─ VERSION                        release identity: 0.1.0 (gated vs Cargo.toml + manifest)
├─ LICENSE                        MIT (copyright holder = documented transfer placeholder)
├─ SECURITY.md                    vulnerability-reporting policy + explicit "no external audit"
├─ CHANGELOG.md                   Keep-a-Changelog history (0.1.0 + Unreleased doc passes)
├─ AUDIT.md                       historical audit/build evidence trail (27 dated sections)
├─ README.md                      product overview, quick start, config/API/observability reference
├─ release-manifest.json          machine-readable delivery manifest (versions, counts, statuses)
├─ Cargo.toml                     workspace root: 7 members, [workspace.dependencies] pins
├─ Cargo.lock                     app dependency lockfile (706 packages)
├─ rust-toolchain.toml            pinned Rust 1.98.1 + rustfmt + clippy
├─ deny.toml                      cargo-deny policy (advisories/bans/licenses/sources)
├─ .cargo/
│  └─ audit.toml                  cargo-audit config (app workspace)
│
│  ── deployment assets ─────────────────────────────────────────────
├─ Dockerfile                     multi-stage, non-root, healthcheck, rust:1.98.1-bookworm
├─ docker-compose.yml             bot + postgres:16-alpine + redis:7-alpine, healthcheck-gated
├─ .dockerignore                  build-context exclusions
├─ .env.template                  compose env template (copy to .env; .env never committed)
├─ config.toml.example            annotated reference config (every key, every section)
├─ .gitignore                     repo hygiene (target/, .env, data/, logs, keypairs…)
│
│  ── release tooling & CI ──────────────────────────────────────────
├─ scripts/
│  ├─ release-check.sh            20-gate local release validation (fmt→tests→staking→audit/deny)
│  ├─ verify-delivery.sh          delivery-bundle integrity check (docs, versions, counts, hygiene)
│  └─ staking-identity.sh         program-id identity tooling (show/verify/set-id/deploy; refuses
│                                 keypair≠declare_id and placeholder ids on public clusters)
├─ .github/
│  └─ workflows/
│     └─ ci.yml                   4 jobs: app workspace / staking program / security / docker
│
│  ── application workspace (7 crates) ─────────────────────────────
├─ crates/
│  ├─ core/                       bot-core — shared kernel
│  │  ├─ Cargo.toml
│  │  ├─ migrations/              11 forward-only PostgreSQL migrations (0001–0011):
│  │  │                           bootstrap; orders/executions; positions/trades;
│  │  │                           dedup/risk/audit; reconciliation; tx attribution;
│  │  │                           intent journal; intent claim kind; execution claims;
│  │  │                           runtime flags; execution_claim_events lineage
│  │  ├─ src/
│  │  │  ├─ lib.rs                crate surface
│  │  │  ├─ config.rs             typed config, validation, env overrides, deny_unknown_fields
│  │  │  ├─ error.rs              error model (classification, redaction)
│  │  │  ├─ events.rs             in-process event bus (AppEvent kinds)
│  │  │  ├─ state.rs              authoritative AppState + counters
│  │  │  ├─ models.rs             domain models (Position, Trade, Order, …)
│  │  │  ├─ maths.rs              numeric helpers
│  │  │  ├─ lifecycle.rs          module lifecycle/heartbeat
│  │  │  ├─ risk.rs               global pre-trade risk engine
│  │  │  ├─ oms.rs                order state machine + idempotency keys
│  │  │  ├─ dedup.rs              3-level restart-safe dedup (memory/Redis/PG)
│  │  │  ├─ auth.rs               API/RBAC authorization
│  │  │  ├─ audit.rs              hash-chained append-only audit trail
│  │  │  ├─ ownership.rs          claims/leases/epochs/fencing + GlobalRiskOracle
│  │  │  ├─ redis_ownership.rs    Redis claim-store backend
│  │  │  ├─ redis_kv.rs           Redis KV (dedup L2, flags)
│  │  │  ├─ reconciliation.rs     intent → venue-truth resolution, ambiguity matrix
│  │  │  ├─ recovery.rs           startup replay/recovery
│  │  │  ├─ storage.rs            JSONL intent journal (rotation, corrupt-line tolerance)
│  │  │  ├─ db/
│  │  │  │  ├─ mod.rs             sqlx pool + embedded migrate!
│  │  │  │  ├─ repo.rs            repositories (orders/positions/audit append w/ advisory lock…)
│  │  │  │  └─ claims.rs          Postgres claim store (authoritative)
│  │  │  └─ obs/
│  │  │     ├─ mod.rs             observability surface
│  │  │     ├─ health.rs          health/ready registries
│  │  │     └─ metrics.rs         bot_* metrics registry (bounded labels)
│  │  └─ tests/
│  │     ├─ db_integration.rs          23 gated tests vs real PostgreSQL
│  │     ├─ redis_integration.rs       10 gated tests vs real Redis
│  │     ├─ distributed_integration.rs 4 gated multi-context tests
│  │     └─ storage_lifecycle.rs       journal restart/rotation/corruption tests
│  ├─ solana-kit/                 Solana integration kit
│  │  ├─ Cargo.toml
│  │  ├─ src/
│  │  │  ├─ lib.rs, consts.rs     crate surface; program IDs / address constants
│  │  │  ├─ rpc.rs                retry/failover chokepoint + broadcast fan-out
│  │  │  ├─ ws.rs                 WS supervision + resubscribe
│  │  │  ├─ events.rs             Yellowstone-style Geyser transactionSubscribe client
│  │  │  ├─ pumpportal.rs         PumpPortal WS client
│  │  │  ├─ cache.rs              TTL + FIFO-bounded warm account cache
│  │  │  ├─ pump.rs               pump.fun bonding-curve instruction builders (incl. v2)
│  │  │  ├─ pumpswap.rs           PumpSwap AMM builders
│  │  │  ├─ raydium.rs            Raydium AMM SwapBaseIn / SwapBaseInV2 builders
│  │  │  ├─ jupiter.rs            Jupiter exit routing
│  │  │  ├─ layout.rs             on-chain account layout parsing
│  │  │  ├─ tokens.rs             SPL token / ATA helpers
│  │  │  ├─ decode.rs             transaction/swap decoder
│  │  │  ├─ tx.rs                 transaction assembly + blockhash
│  │  │  ├─ execute.rs            executor: simulate-first, send, confirm
│  │  │  └─ signer.rs             TransactionSigner + SignerRegistry (multi-signer safe)
│  │  └─ tests/
│  │     ├─ mock_pumpportal.rs    PumpPortal WS mock harness
│  │     ├─ devnet_e2e.rs         network-gated devnet e2e (E2E_NETWORK)
│  │     ├─ latency_bench.rs      network-gated latency benchmarks
│  │     └─ recon_crash_e2e.rs    crash-recovery e2e vs local validator
│  ├─ module-sniper/              Module 1 — pump.fun sniper
│  │  ├─ Cargo.toml
│  │  ├─ src/ (lib.rs, detect.rs, entry.rs, exit.rs)
│  │  └─ tests/ (detect_feed.rs, geyser_detect.rs)
│  ├─ module-copy/                Module 2 — copy trading
│  │  ├─ Cargo.toml
│  │  ├─ src/ (lib.rs, feeds.rs, mirror.rs, exit.rs)
│  │  └─ tests/ (copy_feed.rs, geyser_feed.rs, two_replica_mirror.rs)
│  ├─ module-polymarket/          Module 3 — Polymarket CLOB/Gamma
│  │  ├─ Cargo.toml
│  │  ├─ src/ (lib.rs, gamma.rs, clob.rs, ws.rs, eip712.rs, orders.rs,
│  │  │        auth.rs, ctf.rs, collateral.rs, strategy.rs, error.rs)
│  │  └─ tests/ (mock_clob_gamma.rs)
│  ├─ module-telegram/            Module 5 — Telegram control
│  │  ├─ Cargo.toml
│  │  └─ src/ (lib.rs, commands.rs, alerts.rs, api.rs — token-redacted Bot API)
│  └─ server/                     sniper-suite binary — control plane + supervision
│     ├─ Cargo.toml
│     └─ src/
│        ├─ main.rs               startup/shutdown orchestration, module supervision
│        ├─ api.rs                REST routes + RBAC + rate limits + request IDs
│        ├─ ws.rs                 /api/events WebSocket feed
│        ├─ dashboard.rs          embedded HTML dashboard
│        ├─ obs.rs                probes + metrics wiring
│        ├─ persist.rs            persistence pumps (state → PostgreSQL)
│        └─ recon.rs              reconciliation tasks (venue truth)
│
│  ── standalone on-chain program (Module 4) ────────────────────────
├─ programs/
│  └─ staking-suite/              native Solana program (own workspace root)
│     ├─ Cargo.toml               hardened release profile; MSRV 1.79 (agave platform-tools)
│     ├─ Cargo.lock               independent lockfile (580 packages)
│     ├─ .cargo/
│     │  ├─ config.toml           MSRV-aware resolver policy
│     │  └─ audit.toml            cargo-audit config (program)
│     ├─ src/
│     │  ├─ lib.rs                entrypoint, declare_id! (pre-deploy placeholder), PDAs
│     │  ├─ processor.rs          instruction processing + account validation
│     │  ├─ state.rs              Config/Stake state, reward/fee math
│     │  ├─ instruction.rs        borsh instruction (de)serialization + client builders
│     │  └─ error.rs              custom errors 6000+ (incl. GenesisAlreadyDone 6026)
│     └─ tests/
│        └─ validator_e2e.rs      STAKING_E2E-gated on-chain lifecycle (3 tests; all 3 executed + passed in the hardening pass)
│
│  ── documentation (47 files under docs/) ──────────────────────────
└─ docs/
   │  # engineering set (13, delivered at freeze):
   ├─ ARCHITECTURE.md  API.md  SECURITY.md  DEPLOYMENT.md  OPERATIONS.md
   ├─ MODULES.md  STAKING.md  TESTING.md  RECONCILIATION.md  DISTRIBUTED.md
   ├─ RELEASE.md  HANDOVER.md  BACKUP-RESTORE.md
   │  # buyer package (14, first documentation pass):
   ├─ BUYER-OVERVIEW.md  CAPABILITY-MATRIX.md  BUYER-DUE-DILIGENCE.md
   ├─ IP-COMPONENTS.md  THIRD-PARTY.md  BUYER-DEPLOYMENT.md
   ├─ ACCEPTANCE-CHECKLIST.md  RELEASE-NOTES-0.1.0.md  BUYER-FAQ.md
   ├─ SCOPE-BOUNDARY.md  SUPPORT-HANDOVER.md  BUYER-RISK-REGISTER.md
   ├─ TECHNICAL-DIFFERENTIATORS.md  DELIVERY-MANIFEST.md
   │  # final delivery package (9, this pass):
   ├─ FINAL-DELIVERY.md  BUYER-QUICKSTART.md  TECHNICAL-FACT-SHEET.md
   ├─ SELLER-FACT-SHEET.md  SELLING-LISTING-SOURCE.md  DEMO-RUNBOOK.md
   ├─ EVIDENCE-INDEX.md  REPOSITORY-MAP.md  ARCHIVE-CHECKLIST.md
   │  # buyer-hardening pass (3):
   ├─ LIVE-VALIDATION.md  BUYER-ACCEPTANCE-TEST.md  CI-LOCAL-EQUIVALENCE.md
   │  # final buyer-handover pass (8; BUYER-ACCEPTANCE-TEST upgraded in place):
   ├─ FEATURE-TRACEABILITY.md  SECURITY-BOUNDARY-MAP.md
   ├─ FINAL-IP-AND-THIRD-PARTY-INVENTORY.md  BUYER-REPRODUCTION-GUIDE.md
   ├─ FINAL-KNOWN-LIMITATIONS.md  FINAL-OPERATIONS-HANDOVER.md
   ├─ FINAL-INCIDENT-RUNBOOK.md  FINAL-RELEASE-AUDIT.md
```

## Counts (exact, at the final delivery package)

| Category | Files |
|---|---|
| Root metadata / release identity (incl. root `Cargo.toml`, `Cargo.lock`, `deny.toml`, `rust-toolchain.toml`, `.cargo/audit.toml`) | 12 |
| Deployment assets (Dockerfile, compose, templates, ignores) | 6 |
| Scripts (`release-check.sh`, `verify-delivery.sh`, `staking-identity.sh`) | 3 |
| CI workflow | 1 |
| App workspace `crates/` — Rust (72 src + 14 tests) + 7 crate Cargo.tomls + 11 SQL migrations | 104 |
| Staking program `programs/staking-suite/` (5 src + 1 test + Cargo.toml + Cargo.lock + 2 `.cargo/` files) | 10 |
| Docs (`docs/`) | 47 |
| **Total tracked files** | **183** |

Note: the frozen software tree (146 files) plus 14 buyer docs plus 9 final
delivery docs plus `scripts/verify-delivery.sh` = 170, plus
`crates/module-polymarket/src/collateral.rs` added by the post-delivery
audit pass = 171; the buyer-hardening pass added `scripts/staking-identity.sh`
plus 3 docs (LIVE-VALIDATION, BUYER-ACCEPTANCE-TEST, CI-LOCAL-EQUIVALENCE)
= 175; the final buyer-handover pass added 8 docs (feature traceability,
security boundary map, IP/third-party inventory, reproduction guide,
known-limitations register, operations handover, incident runbook, release
audit) = 183 and modified `scripts/staking-identity.sh` (identity-tracking
defect fix: BUYER-ACCEPTANCE-TEST.md added to TRACKED_FILES;
release-manifest.json excluded from the post-set-id stale sweep as a
delivery-time record). The audit pass also MODIFIED existing sources (live/paper
balance separation in modules 1+3, staking max-supply cap + token metadata
in module 4), so the frozen-tree byte identity applies to commit `0e139c3`
only; the current tree differs as described in CHANGELOG.md [Unreleased].
Re-check with the commands in `docs/BUYER-DUE-DILIGENCE.md` §A.

## Where to look first

- Understand: `docs/FINAL-DELIVERY.md` → `docs/BUYER-OVERVIEW.md`
- Verify: `docs/EVIDENCE-INDEX.md` → `AUDIT.md` → run the scripts
- Deploy: `docs/BUYER-QUICKSTART.md` → `docs/BUYER-DEPLOYMENT.md`
- Accept: `docs/ACCEPTANCE-CHECKLIST.md`
- Transfer: `docs/IP-COMPONENTS.md` §Ownership transfer checklist →
  `docs/SUPPORT-HANDOVER.md` → `docs/ARCHIVE-CHECKLIST.md`
