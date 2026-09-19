# sniper-suite 0.1.0 — FINAL DELIVERY BUNDLE & BUYER PRESENTATION PACKAGE Report

Prepared on the completed buyer-handover tree (frozen software: release commit `9c677cd` → freeze commit `0e139c3`; 146 files / 2,801,590 B at freeze). This pass completed the delivery bundle: 9 new documents + 1 bundle-integrity script created; 8 documents finalized/updated. **No trading, risk, staking, schema, migration, auth, signer or execution code was touched — byte-proven in §6.** No git history was fabricated; `.git` remains absent from this sandbox (see §8 and `docs/ARCHIVE-CHECKLIST.md`).

---

## 1. FINAL DELIVERY AUDIT

1. **Starting state verified before edits:** 160 files / 2,919,949 B (post buyer-package), version identity 0.1.0 in all three locations, docs/ = 27.
2. **§17 script justification:** `scripts/verify-delivery.sh` was created because it covers a real gap — `release-check.sh` needs the full toolchain + PostgreSQL + Redis and runs the whole engineering gate, while a received bundle needs a seconds-fast, dependency-light integrity check (required files, version identity, docs count, hygiene, markdown links, invisible characters). It builds/tests nothing and duplicates no release-check gate. It fails closed (missing python3 ⇒ FAIL, any missing file ⇒ FAIL, exit 1). First execution: **7 PASS / 0 FAIL, exit 0**.
3. **§4/§5 finalization:** CAPABILITY-MATRIX rebuilt to the required schema (Capability / Implementation / Source path / Latest verification / Verification type / Known limitation) with no claim upgraded — build-sbf/validator-e2e/devnet/crash-e2e remain PREVIOUSLY VERIFIED, Docker/CI remain NOT EXECUTED, mocks are labeled "VERIFIED (mocks)". BUYER-RISK-REGISTER rebuilt to (Risk / Evidence / Current mitigation / Operational consequence / Buyer action), expanded to 14 rows so every required category is explicit (Geyser/feed-provider and production-infrastructure rows added) + 5 documented non-risks.
4. **§6:** IP-COMPONENTS gained the 15-item Ownership transfer checklist (no credentials or identities invented — every line names an artifact and its state).
5. **§7:** THIRD-PARTY.md reviewed against the required topics (protocol integrations §3, major libraries §2, lockfiles §1, license policy §4, cargo-audit §5, cargo-deny §4, SBOM §6, ownership boundary §3+summary) — **no change needed; left untouched**.
6. **§8:** DELIVERY-MANIFEST now indexes all 23 buyer/delivery docs + the new script, with updated identity counts and reading order starting at FINAL-DELIVERY.md.
7. **§9/§10:** README handover section compacted around the required six links; CHANGELOG gained a second `[Unreleased]` sub-entry; no historical entry rewritten; no technical release fact changed.
8. **§20 git rule honored:** no history recreated, no hashes invented; the authoritative-history requirement (`9c677cd` → `0e139c3`) is documented in FINAL-DELIVERY §2, EVIDENCE-INDEX, ARCHIVE-CHECKLIST, and §8 below.
9. **§21 size rule honored:** growth is documentation + one 7 KB shell script only; no attempt toward 20–30 MB.
10. **Seller-facts discipline:** SELLER-FACT-SHEET and SELLING-LISTING-SOURCE contain explicit non-claim lists (no price/revenue/ROI/clients/volumes/latency guarantees/"enterprise"/audit claims) and mandatory limitation sections.

## 2. FILES CREATED (10 — complete content in §4)

| # | File | Bytes | Lines |
|---|------|-------|-------|
| 1 | docs/FINAL-DELIVERY.md | 13,674 | 220 |
| 2 | docs/BUYER-QUICKSTART.md | 7,762 | 219 |
| 3 | docs/TECHNICAL-FACT-SHEET.md | 8,984 | 151 |
| 4 | docs/SELLER-FACT-SHEET.md | 8,241 | 157 |
| 5 | docs/SELLING-LISTING-SOURCE.md | 9,008 | 169 |
| 6 | docs/DEMO-RUNBOOK.md | 10,831 | 219 |
| 7 | docs/EVIDENCE-INDEX.md | 7,517 | 66 |
| 8 | docs/REPOSITORY-MAP.md | 12,702 | 193 |
| 9 | docs/ARCHIVE-CHECKLIST.md | 5,216 | 106 |
| 10 | scripts/verify-delivery.sh | 7,062 | 155 |

Total created: 90,997 bytes / 1,655 lines.

## 3. FILES MODIFIED (8 — complete final content in §5)

| File | Before → After | Change |
|---|---|---|
| docs/CAPABILITY-MATRIX.md | 9,922 → 11,428 B | Full rewrite to final column schema; 24 capability rows + test-count summary; no status upgraded |
| docs/BUYER-RISK-REGISTER.md | 7,634 → 9,050 B | Full rewrite to final field schema; 12 → 14 risks + 5 non-risks |
| docs/IP-COMPONENTS.md | 10,543 → 13,801 B | Appended "Ownership transfer checklist" (15 items); inventory sections untouched |
| docs/DELIVERY-MANIFEST.md | 7,548 → 10,470 B | Buyer-package table extended with the 9 new docs + script note; identity counts + reading order updated |
| README.md | 29,395 → 29,539 B | Handover section compacted around FINAL-DELIVERY/QUICKSTART/DUE-DILIGENCE/ACCEPTANCE/RISK-REGISTER/DELIVERY-MANIFEST; layout comment 27 → 36 docs; scripts line lists both scripts |
| docs/HANDOVER.md | 7,812 → 8,190 B | §1 bullet: 14 → 23 buyer/delivery docs (+ script disclosure) |
| CHANGELOG.md | 7,348 → 8,442 B | Second `[Unreleased]` sub-entry (final delivery package); historical entries untouched |
| release-manifest.json | 5,420 → 5,420 B | `components.docs_count`: 27 → 36 (single value, same byte length) |

Total modification delta: +10,718 bytes. Created + deltas = 101,715 B = exact tree growth (2,919,949 → 3,021,664).

---

== SECTION 4: COMPLETE CONTENT OF EVERY CREATED FILE (10) ==


----- COMPLETE FILE: docs/FINAL-DELIVERY.md (13674 bytes, 220 lines) -----

`````markdown
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
  e2e tests, all protocol-mock harnesses.
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
| Staking host tests | 48 / 48 | VERIFIED |
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
`````


----- COMPLETE FILE: docs/BUYER-QUICKSTART.md (7762 bytes, 219 lines) -----

`````markdown
# Buyer quick start (technical)

The shortest honest path from "received the bundle" to "verified the system
in paper/simulate mode". This walkthrough ends **before** live trading —
going live is a separate, deliberate, multi-gate decision
(`docs/BUYER-DEPLOYMENT.md` §15). Companion documents:
`docs/DEMO-RUNBOOK.md` (structured demos), `docs/HANDOVER.md` §2
(verify-from-zero).

Requirements: Linux x86-64, ~2 GB RAM, ~20 GB disk (full verification
build), network access, PostgreSQL ≥ 16 and Redis 7 (or Docker for the
compose stack).

## 1. Verify the received bundle

```bash
git log --oneline -5      # authoritative history must contain 0e139c3 (freeze) on 9c677cd (release)
git status --short        # expect: clean
./scripts/verify-delivery.sh   # delivery integrity: docs, versions, counts, no secrets/artifacts
```

If you received an archive instead of a git checkout, unpack it and run
`./scripts/verify-delivery.sh`; then import it into the authoritative
repository history (the archive itself carries no `.git` — see
`docs/ARCHIVE-CHECKLIST.md`).

## 2. Inspect VERSION

```bash
cat VERSION                       # 0.1.0
grep '^version' Cargo.toml        # 0.1.0 (workspace.package)
python3 -c "import json;print(json.load(open('release-manifest.json'))['version'])"   # 0.1.0
```

All three must agree; `scripts/release-check.sh` gates this automatically.

## 3. Verify the file inventory

```bash
git ls-files | wc -l              # frozen software tree: 146 (+ documentation-pass files)
find . -type f -not -path './.git/*' | wc -l
find . -type f -not -path './.git/*' -print0 | du -cb --files0-from=- | tail -1
```

Cross-check against the measured table in `docs/FINAL-DELIVERY.md` §3 and
the layout in `docs/REPOSITORY-MAP.md`.

## 4. Install the pinned Rust toolchain

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal
source ~/.cargo/bin/env
rustc --version                   # rust-toolchain.toml forces 1.98.1 automatically
```

Do not substitute another version — the entire verification record
(521 tests, clippy `-D warnings`, audit/deny) was produced with 1.98.1.

## 5. Configure PostgreSQL

```bash
# Any PostgreSQL >= 16 (verified on 16.4; compose uses postgres:16-alpine)
export POSTGRES_URL=postgres://user:pass@host:5432/sniper
```

Migrations `0001`–`0011` are embedded in the binary and apply at startup
(`auto_migrate`, see `config.toml.example` `[storage]`). Forward-only by
design — no down migrations (`docs/BACKUP-RESTORE.md`).

## 6. Configure Redis

```bash
export REDIS_URL=redis://host:6379    # Redis 7.x (verified on 7.2.10)
```

Redis is non-authoritative (dedup L2, coordination, cache); it may die
without losing money-relevant truth.

## 7. Configure secrets externally

Never place secrets in the repository. Config stores env-var *names*
(`*_env` fields); values come from the environment / your secret store
(`docs/BUYER-DEPLOYMENT.md` §7):

```bash
export API_KEY=...                    # gates all mutating REST routes
export TELEGRAM_BOT_TOKEN=...         # only if using Module 5
# SOLANA_KEYPAIR / POLYMARKET_PRIVATE_KEY: only needed for simulate/live
```

The server refuses to bind a non-loopback address without API auth — a hard
startup gate.

## 8. Run the release gate

```bash
./scripts/release-check.sh
```

Expected on the delivered source with PG+Redis reachable: **20 PASS / 0 FAIL
/ 0 SKIP** (521/521 workspace incl. the 38 gated integration tests, staking
48/48 host, fmt/clippy/audit/deny clean). If anything fails on your machine,
stop and resolve before proceeding — this is the same gate the delivery
evidence was produced with.

## 9. Start in paper mode

```bash
cp config.toml.example config.toml && $EDITOR config.toml
# defaults are safe: [execution] mode = "paper", all modules disabled
cargo build --release
CONFIG_PATH=./config.toml ./target/release/sniper-suite
# or the full stack: cp .env.template .env && $EDITOR .env && docker compose up --build -d
```

Enable modules in `config.toml` or via API/Telegram. Paper mode simulates
fills against live market data with seeded balances (10 SOL / 1000 USDC).
Nothing is sent on-chain or to Polymarket.

## 10. Verify `/health`

```bash
curl -s localhost:8080/health
# {"status":"ok","version":"0.1.0","uptime_s":...}  → HTTP 200 always while serving
```

Liveness only — never reflects dependency state (an outage must not get the
process restart-looped).

## 11. Verify `/ready`

```bash
curl -si localhost:8080/ready | head -20
```

200 with `"ready": true` when every component is ready; 503 + per-component
JSON report otherwise (`rpc`, `sniper`, `copy`, `polymarket`). Detail strings
contain only booleans/counts/enum names — never errors, URLs, or secrets.
Try stopping Redis or blocking RPC to watch it degrade to 503 and recover.

## 12. Verify `/metrics`

```bash
curl -s localhost:8080/metrics | grep '^bot_' | head -20
```

Expect `bot_*` series (build info, execution mode, kill switch, module
counters, RPC outcomes, latency histograms, HTTP metrics). Names/labels:
README §Observability; they must match `docs/OPERATIONS.md`.
`metrics_enabled = false` removes the surface (404).

## 13. Verify the dashboard

Open `http://localhost:8080/` — embedded single-file HTML dashboard: live
status, positions, trades, events (WebSocket feed `/api/events`).

## 14. Verify Telegram authorization

With `TELEGRAM_BOT_TOKEN` set and `[telegram]` allow-lists populated:

- From an allow-listed readonly id: `/status` works, `/kill` is **refused
  explicitly** (never a silent no-op).
- From an unknown id: refused.
- From an owner id: `/on`, `/off`, `/kill`, `/resume`, `/mode` accepted.
- Empty allow-lists ⇒ nothing accepted (deny-by-default).
- Check that no bot output/error ever contains the token (regression-tested,
  but verify live once).

## 15. Perform a simulate-mode run

```bash
EXECUTION_MODE=simulate ./target/release/sniper-suite   # + SOLANA_KEYPAIR for real tx building
```

Simulate builds **real transactions** and RPC-simulates them without
broadcasting. Observe `order_sent`/simulation outcomes in the dashboard and
logs; confirm nothing lands on-chain (no signature exists). Note the
documented simulate semantics: synthesized success stays `Sent` and is
resolved by reconciliation, never treated as confirmed
(`docs/RECONCILIATION.md`).

## 16. Perform a backup

```bash
pg_dump "$POSTGRES_URL" -Fc -f sniper_backup.dump
# + copy the JSONL journal directory (see [storage] config / docs/BACKUP-RESTORE.md)
```

## 17. Perform a restore

```bash
createdb sniper_restored
pg_restore -d sniper_restored sniper_backup.dump
POSTGRES_URL=postgres://user:pass@host:5432/sniper_restored \
  cargo test -p bot-core --test db_integration -- --test-threads=1
```

Expected: the full db_integration suite (23 tests) green **on the restored
database** — this exact round-trip was VERIFIED in the freeze pass.

## 18. Review audit-chain verification

```bash
curl -s -H "x-api-key: $API_KEY" localhost:8080/api/audit/verify
```

Recomputes the hash chain over `audit_events`; expect a valid-chain result.
Tamper behavior (modification/reorder/missing/duplicate detection, linear
chain under 8 concurrent appenders) is regression-tested in `db_integration`
— see `docs/EVIDENCE-INDEX.md` for pointers.

---

**Stop here.** Live trading additionally requires: everything above green on
your infrastructure, both live gates deliberately enabled
(`mode = "live"` + `allow_live_trading = true`), owner-only runtime
switching, real funded keys, sized risk limits, and gradual supervised
funded validation (`docs/BUYER-DEPLOYMENT.md` §15). For the staking program:
external audit passed and program id finalized first (`docs/STAKING.md`).
`````


----- COMPLETE FILE: docs/TECHNICAL-FACT-SHEET.md (8984 bytes, 151 lines) -----

`````markdown
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
| Staking host / validator e2e | 48 passed / 2 gated-skipped in freeze sandbox (e2e 2/2 PREVIOUSLY VERIFIED) |
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
`````


----- COMPLETE FILE: docs/SELLER-FACT-SHEET.md (8241 bytes, 157 lines) -----

`````markdown
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
- PREVIOUSLY VERIFIED (earlier sessions, identical source): `build-sbf`
  5,440-byte binary; validator e2e 2/2 incl. funded stake→reward→unstake;
  crash-recovery e2e vs local validator; read-only devnet e2e; latency
  benchmarks.
- NOT EXECUTED: Docker build+smoke (no daemon), CI run (no runner), funded
  live trading, external audit (none exists), SBOM generator run.
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
`````


----- COMPLETE FILE: docs/SELLING-LISTING-SOURCE.md (9008 bytes, 169 lines) -----

`````markdown
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
   control plane) with Solana staking program source, 521 tests, full
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
state: 521/521 workspace tests plus gated integration suites against real
PostgreSQL/Redis, a 20-gate release check, cargo-audit/deny clean, and a
complete buyer due-diligence documentation package. No external security
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

- 521/521 workspace tests (offline-deterministic core + protocol mocks:
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
`````


----- COMPLETE FILE: docs/DEMO-RUNBOOK.md (10831 bytes, 219 lines) -----

`````markdown
# Demo runbook — deterministic buyer demonstration

Ten demonstrations a seller can run for a buyer (or a buyer can run alone)
against the delivered source. Each demo states: prerequisites, exact
commands, expected observable result, what it proves, and its verification
status. **No demo involves live trading or real funds.** Demos 1–7 and 9–10
run on one machine with PostgreSQL + Redis; Demo 8 needs two processes
(still one machine). Status labels: VERIFIED = the underlying behavior is
covered by tests executed in the final freeze gate; PREVIOUSLY VERIFIED =
covered by tests executed on identical source in an earlier session;
the demo itself is a live re-demonstration on the buyer's/seller's machine.

Global prerequisites: pinned toolchain installed (`rust-toolchain.toml`
auto-selects 1.98.1), `POSTGRES_URL` + `REDIS_URL` exported, migrations
applied (automatic at startup with `auto_migrate`), release build:
`cargo build --release`. Config: `cp config.toml.example config.toml`
(defaults are paper mode, modules disabled).

## Demo 1 — Paper mode end-to-end

- **Prerequisites:** global; a module enabled in `config.toml`
  (e.g. `[sniper] enabled = true`) or enabled at runtime via API/Telegram.
- **Command:**
  ```bash
  CONFIG_PATH=./config.toml ./target/release/sniper-suite
  # in another shell:
  curl -s -X POST localhost:8080/api/modules/sniper/enable -H "x-api-key: $API_KEY"
  curl -s localhost:8080/api/status
  ```
- **Expected:** `mode: paper` in status; the dashboard (`http://localhost:8080/`)
  shows simulated fills against live market data with seeded balances
  (10 SOL / 1000 USDC); positions/trades accumulate; no signature or
  on-chain transaction exists anywhere.
- **Proves:** default-safe operation; the full decision → risk → simulated
  fill → persistence → UI pipeline works without touching a venue.
- **Status:** behavior covered by VERIFIED workspace tests (state machine,
  paper fills, API); live demonstration on the demo machine.

## Demo 2 — Simulate mode (real transactions, zero broadcast)

- **Prerequisites:** Demo 1 setup + `SOLANA_KEYPAIR` (use a fresh empty
  keypair — simulate never sends).
- **Command:**
  ```bash
  EXECUTION_MODE=simulate CONFIG_PATH=./config.toml ./target/release/sniper-suite
  curl -s localhost:8080/api/status   # mode: simulate
  ```
- **Expected:** orders are built as real Solana transactions and sent
  through `simulateTransaction`; logs show simulation outcomes; nothing is
  broadcast (no signatures on-chain); ambiguous synthesized outcomes stay
  `Sent` and are resolved by reconciliation, never shown as confirmed.
- **Proves:** the real execution path (build → sign via signer registry →
  simulate) minus broadcast; honest simulate semantics.
- **Status:** simulate-policy behavior VERIFIED in unit tests; the
  documented simulate status semantics are in `docs/RECONCILIATION.md`.

## Demo 3 — Health / readiness / metrics

- **Prerequisites:** server running (any mode).
- **Command:**
  ```bash
  curl -s localhost:8080/health
  curl -si localhost:8080/ready | head -20
  curl -s localhost:8080/metrics | grep '^bot_' | head -20
  # degrade it: stop Redis (or block RPC), then re-run /ready
  ```
- **Expected:** `/health` always 200 while serving (never reflects
  dependencies); `/ready` 200 with component report, flipping to 503 with
  the affected component marked when Redis/RPC/module degrades, and back to
  200 on recovery; `/metrics` exposes `bot_*` series with bounded labels
  matching README §Observability; every response carries `x-request-id`.
- **Proves:** liveness/readiness separation, visible degradation, metric
  surface, request correlation.
- **Status:** VERIFIED (health/readiness/metrics/correlation tests in the
  521; metric names verified against source).

## Demo 4 — Risk rejection

- **Prerequisites:** server running in paper mode with a module enabled;
  tight risk limits in `config.toml` (`[risk]` — e.g. max positions 1,
  small exposure cap, or a low daily-loss limit).
- **Command:**
  ```bash
  # let it take one position, then watch the next signal get rejected:
  curl -s localhost:8080/api/status          # risk counters
  curl -s localhost:8080/metrics | grep risk_rejections
  # dashboard/events feed shows risk_rejected events
  ```
- **Expected:** signals exceeding a limit produce `risk_rejected` events
  (dashboard + WS feed), `bot_module_risk_rejections_total` increments, and
  **no order is created** — rejection happens before execution.
- **Proves:** the global pre-trade risk engine gates every module.
- **Status:** VERIFIED (risk decision tests + rejection event/metric paths).

## Demo 5 — Kill switch

- **Prerequisites:** server running with a module enabled.
- **Command:**
  ```bash
  curl -s -X POST localhost:8080/api/kill -H "x-api-key: $API_KEY"
  curl -s localhost:8080/api/status            # kill_switch engaged
  curl -s localhost:8080/metrics | grep bot_kill_switch
  # via Telegram (owner id): /kill then /resume
  curl -s -X POST localhost:8080/api/resume -H "x-api-key: $API_KEY"
  ```
- **Expected:** all trading halts immediately on `/api/kill` (or Telegram
  `/kill` from an authorized id; explicit refusal from unauthorized ids);
  `bot_kill_switch` gauge flips; `/resume` clears it. In a multi-replica
  setup the flag syncs to all replicas (Demo 8).
- **Proves:** emergency stop works from both control surfaces and is
  observable.
- **Status:** VERIFIED (kill-switch tests; cross-replica flag sync tested in
  `distributed_integration`).

## Demo 6 — Restart / recovery

- **Prerequisites:** paper (or simulate) run with recorded intents/positions.
- **Command:**
  ```bash
  kill -9 $(pgrep -f 'target/release/sniper-suite')
  CONFIG_PATH=./config.toml ./target/release/sniper-suite   # restart
  # watch startup logs: journal replay + reconciliation before new work
  curl -s localhost:8080/api/status
  ```
- **Expected:** startup replays the JSONL intent journal + Postgres intents,
  reconciles unresolved intents against venue truth **before** accepting new
  work; no duplicate executions (dedup + claims); positions/PnL consistent;
  `/ready` returns to 200.
- **Proves:** crash safety: restart neither loses nor double-executes
  recorded intents.
- **Status:** VERIFIED (restart-fidelity, corrupt-line, OMS restart-recovery
  tests); `recon_crash_e2e` against a local validator is PREVIOUSLY
  VERIFIED.

## Demo 7 — Audit-chain verification

- **Prerequisites:** server has run and recorded audit events.
- **Command:**
  ```bash
  curl -s -H "x-api-key: $API_KEY" localhost:8080/api/audit/verify
  # tamper demonstration (on a THROWAWAY database only):
  psql "$POSTGRES_URL" -c "UPDATE audit_events SET detail='x' WHERE id=(SELECT max(id) FROM audit_events);"
  curl -s -H "x-api-key: $API_KEY" localhost:8080/api/audit/verify   # now reports the break
  ```
- **Expected:** valid chain before tampering; after modifying any row the
  verifier reports the exact break. No app API can delete/modify audit rows
  (append-only); the tamper step requires direct DB access and exists only
  to demonstrate detection.
- **Proves:** tamper-evident, hash-chained, append-only audit trail.
- **Status:** VERIFIED (modification/reorder/missing/duplicate detection +
  linear chain under 8 concurrent appenders, `db_integration`).

## Demo 8 — Distributed claim behavior (where local infrastructure permits)

- **Prerequisites:** one machine, shared PG + Redis; two server processes
  with distinct data/HTTP ports (or run the test harness directly).
- **Command (harness — deterministic):**
  ```bash
  POSTGRES_URL=... REDIS_URL=... cargo test -p module-copy --test two_replica_mirror -- --test-threads=1
  POSTGRES_URL=... REDIS_URL=... cargo test -p bot-core --test distributed_integration -- --test-threads=1
  # live variant: start two processes, enable the same module on both,
  # watch claims/leases/fencing in logs + execution_claim_events table
  ```
- **Expected:** for each logical execution exactly one replica holds the
  claim (lease + epoch + fencing token); the mirror test shows one tracked
  trade produces one mirrored execution across two racing replicas;
  `execution_claim_events` records the lineage; kill-switch/module flags
  sync across replicas.
- **Proves:** the invariant one execution ⇒ ≤1 owner ⇒ ≤1 money-moving
  submission under real contention.
- **Status:** VERIFIED (4/4 + 1/1 against real PG/Redis in the freeze gate).

## Demo 9 — Staking tests / documented historical validator evidence

- **Prerequisites:** Rust toolchain (host tests need nothing else).
- **Command:**
  ```bash
  cd programs/staking-suite && cargo test        # 48 host tests
  # full on-chain lifecycle (requires Solana CLI/agave 2.1.21 toolchain):
  cargo build-sbf && STAKING_E2E=1 cargo test --test validator_e2e -- --test-threads=1
  ```
- **Expected:** 48/48 host tests pass anywhere (validation layer, caps,
  pause, timelock queue/apply/cancel, two-step admin, genesis latch, state
  math, instruction (de)serialization). With the Solana toolchain present:
  5,440-byte `.so` builds and the 2 e2e tests run the full lifecycle on a
  local `solana-test-validator`, including funded stake → reward → unstake.
- **Proves:** program logic on host; on-chain behavior where the toolchain
  exists.
- **Status:** host 48/48 VERIFIED (freeze gate); build-sbf + validator e2e
  2/2 PREVIOUSLY VERIFIED (agave 2.1.21, identical source; CI `program` job
  re-runs both on every push once CI is active). **Reminder for the demo
  audience: no external audit exists; mainnet deployment is blocked until
  one passes.**

## Demo 10 — Backup / restore

- **Prerequisites:** a database with recorded state (from Demos 1–7).
- **Command:**
  ```bash
  pg_dump "$POSTGRES_URL" -Fc -f sniper_backup.dump
  createdb sniper_restored && pg_restore -d sniper_restored sniper_backup.dump
  POSTGRES_URL=postgres://user:pass@host:5432/sniper_restored \
    cargo test -p bot-core --test db_integration -- --test-threads=1
  ```
- **Expected:** restore succeeds; the full 23-test db_integration suite
  passes **against the restored database** (including audit-chain
  verification of restored rows). Redis needs no restore (non-authoritative).
- **Proves:** the durable-truth model: PostgreSQL + journal files are the
  system of record; backups are complete and functional.
- **Status:** VERIFIED (this exact round-trip ran green in the freeze gate).

---

## Demo sequencing advice

Run 1 → 3 → 4 → 5 first (pure paper, zero setup risk), then 6 → 7 → 10
(recovery/tamper/backup), then 2 (needs a keypair), then 8 (two processes),
then 9 (staking). Total time on a warm build: roughly 1–2 hours. Nothing in
this runbook requires funded keys, mainnet access, or live trading.
`````


----- COMPLETE FILE: docs/EVIDENCE-INDEX.md (7517 bytes, 66 lines) -----

`````markdown
# Evidence index — claim → source map

Every major claim made anywhere in the buyer package, mapped to the file and
section that evidences it, with its verification status and date. Purpose: a
buyer's due-diligence team can check any claim in one hop. Status labels per
`docs/HANDOVER.md` §3. Dates: engineering evidence was produced 2026-09-17
(build sessions) and 2026-09-18 (release + freeze passes), as recorded in
`AUDIT.md`'s dated sections.

## Test & gate claims

| Claim | Evidence file | Evidence section | Status / date |
|---|---|---|---|
| 521/521 workspace tests, 0 failures (incl. 38 gated executed) | `AUDIT.md`; `docs/TESTING.md`; `release-manifest.json` | §27 (freeze gate run); §"What is covered where"; `test_counts.workspace_total` | VERIFIED — 2026-09-18 |
| db_integration 23/23 vs real PostgreSQL 16.4 (fresh + rerun) | `AUDIT.md`; `docs/TESTING.md` | §27; db_integration bullet (line ~49) | VERIFIED — 2026-09-18 |
| redis_integration 10/10 vs real Redis 7.2.10 | `AUDIT.md`; `docs/TESTING.md` | §27; redis bullet (line ~75) | VERIFIED — 2026-09-18 |
| distributed_integration 4/4 | `AUDIT.md`; `docs/TESTING.md` | §27; distributed bullet (line ~82) | VERIFIED — 2026-09-18 |
| two_replica_mirror 1/1 (two real processes) | `AUDIT.md`; `docs/TESTING.md`; `crates/module-copy/tests/two_replica_mirror.rs` | §27; test source | VERIFIED — 2026-09-18 |
| Staking host 48/48 | `AUDIT.md`; `docs/STAKING.md`; `release-manifest.json` | §27; `test_counts.staking_host` | VERIFIED — 2026-09-18 |
| Staking validator e2e 2/2 (funded stake→reward→unstake, local validator) | `AUDIT.md`; `docs/STAKING.md`; `programs/staking-suite/tests/validator_e2e.rs` | earlier dated sections; e2e source | PREVIOUSLY VERIFIED — 2026-09-17 era (agave 2.1.21) |
| `cargo build-sbf` → 5,440-byte `staking_suite.so` | `AUDIT.md`; `release-manifest.json` | earlier sections; `previously_verified_identical_source` | PREVIOUSLY VERIFIED |
| release-check 20 PASS / 0 FAIL / 0 SKIP, exit 0 | `AUDIT.md`; `release-manifest.json`; `scripts/release-check.sh` | §27 (definitive run on frozen tree); `test_counts.release_check_gates` | VERIFIED — 2026-09-18 |
| 609 whole-script test executions / 0 failures | `AUDIT.md` | §27 | VERIFIED — 2026-09-18 |
| fmt + clippy `-D warnings` clean (both cargo projects) | `AUDIT.md`; `release-manifest.json` | §26–27; `verified_final_pass` | VERIFIED — 2026-09-18 |
| cargo-audit ×2 = 0 findings (cargo-audit 0.22.2) | `AUDIT.md`; `release-manifest.json`; `docs/THIRD-PARTY.md` | §27; `verified_final_pass`; §5 | VERIFIED — 2026-09-18 |
| cargo-deny ok (advisories/bans/licenses/sources; 0.18.9) | `AUDIT.md`; `deny.toml`; `docs/THIRD-PARTY.md` | §27; policy file; §4 | VERIFIED — 2026-09-18 |
| recon_crash_e2e vs local solana-test-validator | `AUDIT.md`; `crates/solana-kit/tests/recon_crash_e2e.rs` | earlier dated sections | PREVIOUSLY VERIFIED |
| devnet_e2e read-only vs public devnet; latency_bench | `AUDIT.md`; `crates/solana-kit/tests/{devnet_e2e,latency_bench}.rs` | earlier dated sections | PREVIOUSLY VERIFIED |
| Deterministic ledger replay | `AUDIT.md` | earlier dated sections | PREVIOUSLY VERIFIED |

## Security & correctness claims

| Claim | Evidence file | Evidence section | Status / date |
|---|---|---|---|
| pg_dump → restore → full db_integration suite green on restored DB | `AUDIT.md`; `docs/BACKUP-RESTORE.md`; `docs/HANDOVER.md` | §26–27; procedures; §2 | VERIFIED — 2026-09-18 |
| Audit-chain tamper detection (modify/reorder/missing/duplicate) + linear chain under 8 concurrent appenders | `AUDIT.md`; `crates/core/tests/db_integration.rs`; `crates/core/src/db/repo.rs` | §26 (fix + tests); chain tests; advisory-lock append | VERIFIED — 2026-09-18 |
| Telegram bot-token redaction in all API error paths (+ closed-port regression test) | `AUDIT.md`; `crates/module-telegram/src/api.rs`; `CHANGELOG.md` | §27; `without_url()` sites + `error_strings_never_contain_the_bot_token`; "Fixed (engineering-freeze pass)" | VERIFIED — 2026-09-18 |
| No secrets / no build artifacts in tree; marker scan clean | `scripts/release-check.sh`; `AUDIT.md` | secret_scan + marker_scan gates; §27 | VERIFIED — 2026-09-18 |
| Migrations monotonic 0001–0011; version + toolchain-pin consistency | `scripts/release-check.sh`; `crates/core/migrations/` | migration_check + version_check + toolchain_check gates | VERIFIED — 2026-09-18 |
| RBAC: readonly-cannot-mutate, operator≠owner, live-mode owner-only | `crates/core/src/auth.rs`; `crates/module-telegram/src/commands.rs`; test suite | authz tests within the 521 | VERIFIED — 2026-09-18 |
| No external security audit exists (any component) | root `SECURITY.md`; `docs/SECURITY.md`; `release-manifest.json` | "What this document is not"; `not_executed_environment_blocked` | FACT — current |

## Packaging & metadata claims

| Claim | Evidence file | Evidence section | Status / date |
|---|---|---|---|
| Version 0.1.0 consistent across VERSION / Cargo.toml / manifest | `VERSION`; `Cargo.toml`; `release-manifest.json`; `scripts/release-check.sh` | version_check gate | VERIFIED — 2026-09-18 |
| Frozen tree: 146 files / 2,801,590 B / 77,980 lines; category breakdown | `docs/FINAL-DELIVERY.md`; `docs/BUYER-DUE-DILIGENCE.md` | §3; §A (measurement commands included for re-verification) | VERIFIED measurement — 2026-09-18 |
| Documentation passes changed no code bytes (byte-exact category proofs) | pass reports (`COMMERCIAL_PACKAGE_REPORT.md` §7 in the seller's workspace; re-derivable with the commands in `docs/BUYER-DUE-DILIGENCE.md` §A) | byte arithmetic: Rust 2,051,936 B, SQL 23,443 B unchanged | VERIFIED — 2026-09-18 (buyer package pass) |
| Documentation consistency clean (links, counts, versions, paths, invisible chars) | `scripts/verify-delivery.sh` (in-repo checker) | whole script | VERIFIED — re-runnable at any time |
| Docker image build + smoke not executed in delivery environment | `release-manifest.json`; `.github/workflows/ci.yml` | `not_executed_environment_blocked`; docker job | NOT EXECUTED — fact |
| CI workflow delivered, never run from delivery environment | `.github/workflows/ci.yml`; `release-manifest.json` | 4 jobs; `not_executed_environment_blocked` | NOT EXECUTED — fact |
| Funded live trading never executed | `release-manifest.json`; `docs/TESTING.md` | `not_executed_environment_blocked`; §Known gaps | NOT EXECUTED — fact |
| SBOM generator not run; lockfiles authoritative (706 + 580 packages) | `docs/THIRD-PARTY.md`; `Cargo.lock`; `programs/staking-suite/Cargo.lock` | §1, §6 | NOT EXECUTED (SBOM) / FACT (lockfiles) |
| Authoritative history `9c677cd` → `0e139c3` | `AUDIT.md`; `CHANGELOG.md`; `release-manifest.json` notes; `docs/FINAL-DELIVERY.md` | §26–27 headers; release identity; manifest design note; §2 | FACT — recorded in the authoritative repository (git metadata absent from the packaging sandbox; never fabricated) |

## How to re-verify anything above

1. **Whole gate:** `./scripts/release-check.sh` (needs PG + Redis) — reproduces
   every VERIFIED test/gate claim in one run.
2. **Bundle/docs:** `./scripts/verify-delivery.sh` (no toolchain needed).
3. **Individual suites:** exact commands in README §Testing and
   `docs/TESTING.md`.
4. **Sizes/counts:** commands in `docs/BUYER-DUE-DILIGENCE.md` §A.
5. **Historical narrative:** `AUDIT.md` (dated sections, oldest → newest;
   historical sections are preserved unmodified by policy).
`````


----- COMPLETE FILE: docs/REPOSITORY-MAP.md (12702 bytes, 193 lines) -----

`````markdown
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
│  └─ verify-delivery.sh          delivery-bundle integrity check (docs, versions, counts, hygiene)
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
│  │  │        auth.rs, ctf.rs, strategy.rs, error.rs)
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
│        └─ validator_e2e.rs      STAKING_E2E-gated on-chain lifecycle (2 tests)
│
│  ── documentation (36 files under docs/) ──────────────────────────
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
```

## Counts (exact, at the final delivery package)

| Category | Files |
|---|---|
| Root metadata / release identity (incl. root `Cargo.toml`, `Cargo.lock`, `deny.toml`, `rust-toolchain.toml`, `.cargo/audit.toml`) | 12 |
| Deployment assets (Dockerfile, compose, templates, ignores) | 6 |
| Scripts (`release-check.sh`, `verify-delivery.sh`) | 2 |
| CI workflow | 1 |
| App workspace `crates/` — Rust (71 src + 14 tests) + 7 crate Cargo.tomls + 11 SQL migrations | 103 |
| Staking program `programs/staking-suite/` (5 src + 1 test + Cargo.toml + Cargo.lock + 2 `.cargo/` files) | 10 |
| Docs (`docs/`) | 36 |
| **Total tracked files** | **170** |

Note: the frozen software tree (146 files) plus 14 buyer docs plus 9 final
delivery docs plus `scripts/verify-delivery.sh` = 170. Rust/SQL/config bytes
are unchanged from the freeze — proven in each pass report and re-checkable
with the commands in `docs/BUYER-DUE-DILIGENCE.md` §A.

## Where to look first

- Understand: `docs/FINAL-DELIVERY.md` → `docs/BUYER-OVERVIEW.md`
- Verify: `docs/EVIDENCE-INDEX.md` → `AUDIT.md` → run the scripts
- Deploy: `docs/BUYER-QUICKSTART.md` → `docs/BUYER-DEPLOYMENT.md`
- Accept: `docs/ACCEPTANCE-CHECKLIST.md`
- Transfer: `docs/IP-COMPONENTS.md` §Ownership transfer checklist →
  `docs/SUPPORT-HANDOVER.md` → `docs/ARCHIVE-CHECKLIST.md`
`````


----- COMPLETE FILE: docs/ARCHIVE-CHECKLIST.md (5216 bytes, 106 lines) -----

`````markdown
# Final delivery archive checklist

Defines exactly what the seller's final archive (git bundle + source
snapshot) must contain, and what must never be in it. **The archive itself
is NOT produced in the packaging sandbox**: the sandbox lost its `.git`
metadata in an infrastructure re-provision, and an archive built here could
not carry the authoritative history. The real archive MUST be produced from
the seller's authoritative repository containing `9c677cd` → `0e139c3` (plus
the documentation-pass commits), so that history, tags and hashes survive
intact. No fake history may be substituted.

## How to produce the archive (seller side, authoritative repo)

```bash
# 1. Confirm state:
git log --oneline          # contains 9c677cd (release) and 0e139c3 (freeze)
git status --short         # clean
./scripts/verify-delivery.sh   # bundle integrity (docs, versions, hygiene)

# 2. Full-history bundle (primary artifact — preserves real history):
git bundle create sniper-suite-0.1.0.bundle --all
git bundle verify sniper-suite-0.1.0.bundle

# 3. Optional convenience snapshot (no history; secondary artifact only):
git archive --format=tar.gz --prefix=sniper-suite-0.1.0/ -o sniper-suite-0.1.0-src.tar.gz HEAD

# 4. Record identities for the transfer paperwork:
git rev-parse HEAD
sha256sum sniper-suite-0.1.0.bundle sniper-suite-0.1.0-src.tar.gz
```

The buyer verifies per `docs/BUYER-QUICKSTART.md` §1 (clone from bundle →
`git log` shows `0e139c3` on `9c677cd` → `verify-delivery.sh` green).

## INCLUDE (must be present — all are tracked files)

- [ ] **Source** — `crates/` (7 crates: 71 src `.rs` + 14 test `.rs` +
      7 Cargo.tomls) and `programs/staking-suite/` (5 src + 1 test +
      Cargo.toml + `.cargo/` configs).
- [ ] **Lockfiles** — root `Cargo.lock` (706 packages) **and**
      `programs/staking-suite/Cargo.lock` (580 packages). Both are
      mandatory: they are the authoritative dependency record and the
      reproducibility basis.
- [ ] **Migrations** — `crates/core/migrations/0001`–`0011` (all 11 `.sql`).
- [ ] **Docs** — all 36 files under `docs/` (13 engineering + 14 buyer
      package + 9 final delivery), plus root `README.md`, `CHANGELOG.md`,
      `AUDIT.md` (evidence trail — never strip it), `SECURITY.md`.
- [ ] **Config examples** — `config.toml.example`, `.env.template`,
      `rust-toolchain.toml`, `deny.toml`, root `.cargo/audit.toml`.
- [ ] **Deployment** — `Dockerfile`, `docker-compose.yml`, `.dockerignore`,
      `.github/workflows/ci.yml`.
- [ ] **CI** — the workflow above (part of deployment).
- [ ] **Release assets** — `release-manifest.json`, `VERSION`, `LICENSE`,
      `scripts/release-check.sh`, `scripts/verify-delivery.sh`.
- [ ] **Git history** — via the bundle (`--all`): both release commits and
      every documentation-pass commit.

## EXCLUDE (must never be in the archive)

- [ ] `target/` (any Rust build output — root, crates, or program).
- [ ] `build/` or any scratch/build directories.
- [ ] Local database files (Postgres data dirs, dumps made during testing —
      `*.dump`, `*.sql.gz` snapshots of live data).
- [ ] Redis data (`dump.rdb`, `appendonly.aof`).
- [ ] `.env` (the template `.env.template` IS included; a filled `.env` is
      a secret file).
- [ ] Secret files of any kind: `*.pem`, `id_*`, `*keypair*.json`,
      `*-keypair.json`, wallet files, token files, credential stores.
- [ ] Generated logs (`*.log`), profiling/flamegraph outputs.
- [ ] Private credentials or provider API keys in any format.
- [ ] Temporary installer files (rustup-init, toolchain tarballs, PG/Redis
      source archives).
- [ ] Editor/OS junk (`.DS_Store`, `*.swp`, `Thumbs.db`, `.idea/`, `.vscode/`
      with local settings).

The repository's `.gitignore` already excludes all of the above from
tracking, and `scripts/release-check.sh` (secret + marker scans) and
`scripts/verify-delivery.sh` (hygiene checks) fail if any of it appears in
the tree. A `git archive`/`git bundle` of a clean tree therefore satisfies
the EXCLUDE list by construction — verify anyway with:

```bash
tar -tzf sniper-suite-0.1.0-src.tar.gz | grep -E 'target/|\.env$|\.log$|keypair|\.dump$|dump\.rdb' && echo "CONTAMINATED" || echo "clean"
```

## Integrity requirements

1. The bundle must `git bundle verify` cleanly and contain `9c677cd` and
   `0e139c3` as reachable commits.
2. `git status` in the buyer's clone must be clean after checkout.
3. `./scripts/verify-delivery.sh` must pass in the checked-out tree.
4. SHA-256 sums of both artifacts recorded in the transfer paperwork
   (this checklist intentionally contains no hashes — they are produced at
   packaging time on the authoritative machine).
5. Version identity: `VERSION` = `Cargo.toml` = `release-manifest.json` =
   `0.1.0`.

## What is explicitly NOT part of the archive

- Deployed on-chain program (nothing is deployed; `declare_id!` is a
  placeholder).
- Any live/production database content, keys, tokens, or customer data
  (none exist).
- Docker images (buyer builds from the Dockerfile).
- CI run history (buyer's runners execute the workflow after transfer).
- External-audit reports (none exist — `docs/BUYER-RISK-REGISTER.md` #1).
`````


----- COMPLETE FILE: scripts/verify-delivery.sh (7062 bytes, 155 lines) -----

`````bash
#!/usr/bin/env bash
# ============================================================================
# verify-delivery.sh — delivery-bundle integrity check.
#
# Fast, fail-closed validation of a received sniper-suite delivery tree.
# Complementary to scripts/release-check.sh: release-check runs the FULL
# engineering gate (toolchain + PostgreSQL + Redis + ~all tests); this script
# needs only bash/coreutils/grep/sed and answers "is this bundle complete,
# consistent and hygienic?" in seconds. It does NOT build or test anything.
#
# Checks:
#   1. required release + buyer documentation files exist
#   2. version identity (VERSION == Cargo.toml == release-manifest.json)
#   3. manifest docs_count == actual docs/*.md count
#   4. hygiene: no .env, logs, keypairs, build dirs, DB/Redis dumps
#   5. every relative markdown link in README.md + docs/ resolves
#   6. no zero-width / bidi-override characters in markdown
#
# Usage:   ./scripts/verify-delivery.sh        (run from the repository root)
# Exit:    0 = all checks passed; 1 = at least one FAIL (fail-closed).
# ============================================================================
set -u
cd "$(dirname "$0")/.." || { echo "FAIL: cannot cd to repository root"; exit 1; }

PASS=0; FAIL=0
ok()   { PASS=$((PASS+1)); printf 'PASS  %s\n' "$1"; }
bad()  { FAIL=$((FAIL+1)); printf 'FAIL  %s\n' "$1"; }

# ------------------------------------------------------- 1. required files --
REQUIRED_FILES="
VERSION LICENSE SECURITY.md README.md CHANGELOG.md AUDIT.md
Cargo.toml Cargo.lock rust-toolchain.toml deny.toml release-manifest.json
Dockerfile docker-compose.yml .dockerignore .env.template config.toml.example .gitignore
.github/workflows/ci.yml scripts/release-check.sh scripts/verify-delivery.sh
programs/staking-suite/Cargo.toml programs/staking-suite/Cargo.lock
docs/ARCHITECTURE.md docs/API.md docs/SECURITY.md docs/DEPLOYMENT.md docs/OPERATIONS.md
docs/MODULES.md docs/STAKING.md docs/TESTING.md docs/RECONCILIATION.md docs/DISTRIBUTED.md
docs/RELEASE.md docs/HANDOVER.md docs/BACKUP-RESTORE.md
docs/BUYER-OVERVIEW.md docs/CAPABILITY-MATRIX.md docs/BUYER-DUE-DILIGENCE.md
docs/IP-COMPONENTS.md docs/THIRD-PARTY.md docs/BUYER-DEPLOYMENT.md
docs/ACCEPTANCE-CHECKLIST.md docs/RELEASE-NOTES-0.1.0.md docs/BUYER-FAQ.md
docs/SCOPE-BOUNDARY.md docs/SUPPORT-HANDOVER.md docs/BUYER-RISK-REGISTER.md
docs/TECHNICAL-DIFFERENTIATORS.md docs/DELIVERY-MANIFEST.md
docs/FINAL-DELIVERY.md docs/BUYER-QUICKSTART.md docs/TECHNICAL-FACT-SHEET.md
docs/SELLER-FACT-SHEET.md docs/SELLING-LISTING-SOURCE.md docs/DEMO-RUNBOOK.md
docs/EVIDENCE-INDEX.md docs/REPOSITORY-MAP.md docs/ARCHIVE-CHECKLIST.md
"
missing=""
for f in $REQUIRED_FILES; do
  [ -f "$f" ] || missing="$missing $f"
done
if [ -z "$missing" ]; then
  ok "required files present ($(echo $REQUIRED_FILES | wc -w | tr -d ' ') checked)"
else
  bad "missing required files:$missing"
fi
# migrations 0001-0011
migmissing=""
for i in 0001 0002 0003 0004 0005 0006 0007 0008 0009 0010 0011; do
  ls crates/core/migrations/${i}_*.sql >/dev/null 2>&1 || migmissing="$migmissing $i"
done
[ -z "$migmissing" ] && ok "migrations 0001-0011 present" || bad "missing migrations:$migmissing"

# ---------------------------------------------------------- 2. version id --
V_FILE="$(tr -d '[:space:]' < VERSION)"
V_MANIFEST="$(sed -n 's/.*"version"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' release-manifest.json | head -1)"
V_CARGO="$(sed -n '/\[workspace.package\]/,/^\[/s/^version[[:space:]]*=[[:space:]]*"\([^"]*\)".*/\1/p' Cargo.toml | head -1)"
if [ -n "$V_FILE" ] && [ "$V_FILE" = "$V_MANIFEST" ] && [ "$V_FILE" = "$V_CARGO" ]; then
  ok "version identity: VERSION == release-manifest.json == Cargo.toml ($V_FILE)"
else
  bad "version mismatch: VERSION='$V_FILE' manifest='$V_MANIFEST' Cargo.toml='$V_CARGO'"
fi

# -------------------------------------------------------- 3. docs counter --
DOCS_ACTUAL="$(ls docs/*.md 2>/dev/null | wc -l | tr -d ' ')"
DOCS_MANIFEST="$(sed -n 's/.*"docs_count"[[:space:]]*:[[:space:]]*\([0-9]*\).*/\1/p' release-manifest.json | head -1)"
if [ "$DOCS_ACTUAL" = "$DOCS_MANIFEST" ]; then
  ok "manifest docs_count ($DOCS_MANIFEST) == actual docs/*.md ($DOCS_ACTUAL)"
else
  bad "docs_count mismatch: manifest=$DOCS_MANIFEST actual=$DOCS_ACTUAL"
fi

# ------------------------------------------------------------ 4. hygiene ---
dirty=""
[ -e .env ] && dirty="$dirty .env"
[ -d target ] && dirty="$dirty target/"
[ -d build ] && dirty="$dirty build/"
[ -d programs/staking-suite/target ] && dirty="$dirty programs/staking-suite/target/"
for pat in '*.log' '*.dump' 'dump.rdb' 'appendonly.aof' '*keypair*.json' '*.pem'; do
  hits="$(find . -path ./.git -prune -o -type f -name "$pat" -print 2>/dev/null | head -3)"
  [ -n "$hits" ] && dirty="$dirty $hits"
done
if [ -z "$dirty" ]; then
  ok "hygiene: no .env / target/ / build/ / logs / dumps / keypairs / pem files"
else
  bad "hygiene violations:$dirty"
fi

# ------------------------------------------------------- 5. markdown links --
if ! command -v python3 >/dev/null 2>&1; then
  bad "python3 not available — cannot verify markdown links (fail-closed)"
else
  broken="$(python3 - <<'PY'
import os, re
link_re = re.compile(r"\[[^\]]*\]\(([^)\s]+)\)")
bad = []
files = ["README.md"] + ["docs/"+f for f in sorted(os.listdir("docs")) if f.endswith(".md")]
for path in files:
    text = open(path, encoding="utf-8").read()
    for m in link_re.finditer(text):
        t = m.group(1)
        if t.startswith(("http://","https://","mailto:","#")): continue
        t = t.split("#")[0]
        if not t: continue
        if not os.path.exists(os.path.normpath(os.path.join(os.path.dirname(path), t))):
            bad.append(f"{path}: {m.group(1)}")
print("\n".join(bad))
PY
)"
  if [ -z "$broken" ]; then
    ok "all relative markdown links resolve (README.md + docs/*.md)"
  else
    bad "broken markdown links:
$broken"
  fi

# ------------------------------------------- 6. invisible/bidi characters --
  zw="$(python3 - <<'PY'
import os
bad_chars = {"\u200b":"ZWSP","\u200e":"LRM","\u200f":"RLM","\u202a":"LRE","\u202b":"RLE","\u202e":"RLO","\ufeff":"BOM"}
files = ["README.md"] + ["docs/"+f for f in sorted(os.listdir("docs")) if f.endswith(".md")]
out=[]
for path in files:
    t=open(path,encoding="utf-8").read()
    for ch,name in bad_chars.items():
        if ch in t: out.append(f"{path}: {name}")
print("\n".join(out))
PY
)"
  if [ -z "$zw" ]; then
    ok "no zero-width / bidi-override characters in markdown"
  else
    bad "invisible characters found:
$zw"
  fi
fi

# ------------------------------------------------------------- summary ----
TOTAL_FILES="$(find . -path ./.git -prune -o -type f -print | wc -l | tr -d ' ')"
TOTAL_BYTES="$(find . -path ./.git -prune -o -type f -print0 | du -cb --files0-from=- 2>/dev/null | tail -1 | cut -f1)"
echo "----------------------------------------"
echo "tree: ${TOTAL_FILES} files (excluding .git), ${TOTAL_BYTES:-unknown} bytes"
echo "verify-delivery: ${PASS} PASS / ${FAIL} FAIL"
[ "$FAIL" -eq 0 ] || exit 1
exit 0
`````


== SECTION 5: COMPLETE CONTENT OF EVERY MODIFIED FILE (8) ==

(CAPABILITY-MATRIX.md and BUYER-RISK-REGISTER.md were fully rewritten to the final column schema; the other six received targeted edits. All are reproduced complete below.)


----- COMPLETE FILE: docs/CAPABILITY-MATRIX.md (11428 bytes, 60 lines) -----

`````markdown
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
| Sniper (pump.fun launch detection + entry) | Launch feeds (PumpPortal WS, Geyser `transactionSubscribe`, poll fallback) + bonding-curve entry + PumpSwap/Raydium/Jupiter exit routing | `crates/module-sniper/src/{detect,entry,exit}.rs`; `crates/solana-kit/src/{pumpportal,events,ws,cache}.rs` | `detect_feed` + `geyser_detect` mock suites green within 521/521 workspace run (freeze gate) | VERIFIED | "~1s" launch-to-buy is a design target, not a guarantee; funded mainnet landing rate NOT EXECUTED (needs funded keys + explicit approval); `latency_bench` only PREVIOUSLY VERIFIED |
| Copy trading | Tracked-wallet mirroring with per-wallet rules, sizing, staleness guards, mirrored exits | `crates/module-copy/src/{feeds,mirror,exit}.rs` | `copy_feed` + `geyser_feed` green in workspace run; `two_replica_mirror` 1/1 vs real PG+Redis | VERIFIED | Mirrors only operator-configured wallets; strategy quality is operator-owned |
| Polymarket (CLOB/Gamma, EIP-712 v2) | Gamma discovery, CLOB REST/WS, EIP-712 v2 11-field order signing, L1/L2 auth, CTF ERC-1155 balances | `crates/module-polymarket/src/{gamma,clob,ws,eip712,orders,auth,ctf,strategy}.rs` | `mock_clob_gamma` (incl. auth headers + signed order wire format) green in workspace run | VERIFIED | Live order placement vs real Polymarket NOT EXECUTED (needs funded Polygon key); third-party API drift is an external risk |
| Telegram control | Deny-by-default RBAC bot: on/off, kill switch, mode, alerts; token-redacted error paths | `crates/module-telegram/src/{commands,alerts,api}.rs` | RBAC/command tests + `error_strings_never_contain_the_bot_token` regression green in workspace run | VERIFIED | Live round-trip needs the buyer's BotFather token; long-polling only (no webhook mode) |
| Staking program (on-chain) | Native Solana program: reward mint, vault + fee treasury, per-second APY, caps, timelock, two-step admin, pause-deposits-only, latched genesis mint | `programs/staking-suite/src/{lib,processor,state,instruction,error}.rs` | Host tests 48/48 green in freeze gate; `build-sbf` (5,440-byte .so) + validator e2e 2/2 incl. funded stake→reward→unstake on local validator (agave 2.1.21) | Host: VERIFIED. build-sbf + e2e: PREVIOUSLY VERIFIED | `declare_id!` is a pre-deploy placeholder; NO external audit; mainnet deployment documentation-blocked until audit passes |
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
| CI pipeline | 4-job GitHub workflow: app (fmt/clippy/build/test vs PG16+Redis7 services), staking (host + build-sbf + gated e2e), security (audit×2/deny×2), docker (build + smoke) | `.github/workflows/ci.yml` | No runner execution from the delivery environment; every step has a local equivalent that passed in the freeze gate (release-check 20/20) | NOT EXECUTED (run); equivalent steps VERIFIED locally | Buyer must run CI on their own GitHub/adapted runner |
| Security scanning | cargo-audit (both lockfiles) + cargo-deny (advisories/bans/licenses/sources) as release + CI hard gates | `deny.toml`; `scripts/release-check.sh`; `.github/workflows/ci.yml` | cargo-audit 0.22.2: 0 findings ×2; cargo-deny 0.18.9: ok — in freeze gate | VERIFIED | RustSec DB is a point-in-time snapshot; buyer re-runs periodically |
| Docker packaging | Multi-stage non-root healthchecked image (rust:1.98.1-bookworm); compose stack (bot + postgres:16-alpine + redis:7-alpine), loopback API by default | `Dockerfile`, `docker-compose.yml`, `.dockerignore` | No Docker daemon in delivery sandbox; static inspection + `docker compose config` syntax gate (CI) only | NOT EXECUTED (build+smoke) | Buyer executes first image build + container smoke |
| Signer abstraction / registry | `TransactionSigner` + named `SignerRegistry`; multi-signer completeness enforced; `local` implemented; `vault`/`kms`/`hsm` fail startup | `crates/solana-kit/src/signer.rs` | Registry + missing-signer structured-failure tests green in workspace run | VERIFIED | Only `local` custody backend implemented; others are documented extension points |
| Geyser feeds | Yellowstone-style `transactionSubscribe` client with failed-tx skipping + poll fallback | `crates/solana-kit/src/events.rs`; module feed tests | `geyser_detect` + `geyser_feed` mock suites green in workspace run | VERIFIED (mocks) | Real Geyser provider is buyer-supplied; provider e2e NOT EXECUTED |
| Account cache | TTL (default 30 s) + FIFO-bounded (default 5 000) warm cache for semi-static accounts | `crates/solana-kit/src/cache.rs` | Cache unit tests green in workspace run | VERIFIED | Best-effort by design; staleness bounds are configuration |
| RPC fan-out / failover | Retry chokepoint, failover after 3 consecutive failures, `BROADCAST_FANOUT` race-send (first accept wins), per-method metrics | `crates/solana-kit/src/rpc.rs` | Mock JSON-RPC pair fan-out race + retry/failover tests green in workspace run | VERIFIED (mocks) | Production provider quality/rate limits are buyer-side |
| Recovery (crash/restart) | JSONL journal (rotation, corrupt-line tolerance) + startup replay + reconcile-before-new-work + 3-level dedup | `crates/core/src/{recovery,storage,dedup}.rs` | `storage_lifecycle` + db_integration restart-recovery green; `recon_crash_e2e` vs local validator | Tests: VERIFIED. crash e2e: PREVIOUSLY VERIFIED | Recovery resolves recorded intents; foreign-wallet activity out of scope |
| Audit chain | Append-only hash-chained trail; advisory-lock serialized appends; `GET /api/audit/verify`; no mutating app API | `crates/core/src/audit.rs`; `crates/core/src/db/repo.rs`; migration `0004` | Tamper detection (modification/reorder/missing/duplicate) + linear chain under 8 concurrent appenders green in db_integration | VERIFIED | App-level hash chain, not a blockchain; DB-superuser tamper + rehash is out of the stated threat model (`docs/SECURITY.md`) |

## Test-count summary (final freeze gate, frozen tree `0e139c3`, 2026-09-18)

| Suite | Result |
|---|---|
| Workspace (`cargo test --workspace -- --test-threads=1`) | 521 / 521 passed (incl. 38 gated integration tests executed) |
| `db_integration` | 23 / 23 |
| `redis_integration` | 10 / 10 |
| `distributed_integration` | 4 / 4 |
| `two_replica_mirror` | 1 / 1 |
| Staking host | 48 / 48 (+2 validator e2e gated-skipped in freeze sandbox; 2/2 PREVIOUSLY VERIFIED) |
| `scripts/release-check.sh` | 20 PASS / 0 FAIL / 0 SKIP, exit 0 |
| fmt / clippy `-D warnings` / cargo-audit ×2 / cargo-deny | all clean |

Machine-readable copy: `release-manifest.json`. Claim → evidence map:
`docs/EVIDENCE-INDEX.md`. Per-suite detail: `docs/TESTING.md`. Historical
trail: `AUDIT.md`.
`````


----- COMPLETE FILE: docs/BUYER-RISK-REGISTER.md (9050 bytes, 42 lines) -----

`````markdown
# Buyer risk register (final)

Every known remaining risk at handover of sniper-suite 0.1.0, stated
factually. Fields: **Risk** → **Evidence** (where the fact is recorded) →
**Current mitigation** (what the delivered asset already does) →
**Operational consequence** (what happens in operation if unaddressed) →
**Buyer action**. None of these is a software defect; buyer-owned
infrastructure and human/legal items are labeled as such. Ordered roughly by
severity of consequence.

| # | Risk | Evidence | Current mitigation (delivered) | Operational consequence | Buyer action |
|---|------|----------|-------------------------------|------------------------|--------------|
| 1 | **No external security audit** — any component; especially the on-chain staking program | Root `SECURITY.md`; `docs/SECURITY.md`; `release-manifest.json` → `not_executed_environment_blocked` | 521+48/2 test matrix incl. tamper/concurrency/dedup/recovery suites; clippy `-D warnings`; audit/deny gates; docs block staking mainnet deployment until an audit passes; paper default + dual live gates limit blast radius | An undiscovered vulnerability could be exploited: staking vault funds at risk on-chain; trading-path edge cases could misbehave under adversarial conditions | Commission an independent audit before staking mainnet deployment; consider a trading-path review before funded live operation |
| 2 | **Funded live execution never generally tested** (no funded keypair existed; requires explicit approval) | `release-manifest.json`; `docs/TESTING.md` §Known gaps; `AUDIT.md` | `simulate` mode RPC-simulates real transactions; paper mode vs live data; reconcile-before-restart; handoff grace; kill switch; daily-loss auto-disable | Real-money behavior (slippage, landing rate, partial fills, venue edge cases) unproven at scale; first funded runs carry discovery risk | Gradual funded validation under operator supervision, smallest sizes first (`docs/BUYER-DEPLOYMENT.md` §15, `docs/DEMO-RUNBOOK.md`) |
| 3 | **Staking deployment identity not finalized** — `declare_id!` is a pre-deploy placeholder; program deployed nowhere | `programs/staking-suite/src/lib.rs`; `docs/STAKING.md`; `docs/HANDOVER.md` §5.2 | Two-step admin transfer prevents key-typo lock-in; parameter timelock with permissionless apply; hard caps; full deploy+genesis sequence documented; genesis mint latched one-shot | Module 4 cannot run live until deployed; deploying under a lost/mismatched keypair would strand governance or require id change + rebuild | Deploy under the placeholder id with the matching keypair (or change id+keypair and rebuild); initialize with multisig admin (Squads/Realms); timelock ≥ 24h; plan the one-shot GenesisMint |
| 4 | **Production RPC dependency & provider rate limits** | `crates/solana-kit/src/rpc.rs`; `docs/OPERATIONS.md` degradation matrix | Retry/failover chokepoint with consecutive-failure tracking; optional broadcast fan-out; visible degradation (readiness 503, `bot_rpc_*` metrics); intents reconciled, never blindly re-broadcast | Provider outage/throttling ⇒ slower detection, failed broadcasts, delayed reconciliation; degraded but safe (no double-spend path) | Contract ≥ 1 quality provider (2 enables fan-out/failover); alert on `bot_rpc_requests_total{outcome="fatal"|"exhausted"}` |
| 5 | **Geyser / feed-provider dependency** (Yellowstone-compatible Geyser, PumpPortal) | `crates/solana-kit/src/{events,pumpportal}.rs`; `docs/THIRD-PARTY.md` §3 | Both feeds are optional: poll fallback preserves functionality; mock-tested resubscribe/reconnect; failed-tx skipping | Without a provider: higher detection latency (polling); provider outage: feed gaps until reconnect/fallback | Decide provider strategy (Geyser and/or PumpPortal vs polling); monitor `bot_ws_*` counters; contract providers in the buyer's name |
| 6 | **Third-party API/protocol changes** (pump.fun, PumpSwap, Raydium, Jupiter, Polymarket, Telegram) | `docs/IP-COMPONENTS.md`; `docs/BUYER-RISK-REGISTER.md` history; mock harnesses in `tests/` | Per-venue builders isolated; strict deserialization fails loudly rather than trading on garbage; CI + release-check catch regressions on upgrade | A venue change can break entries/exits/orders until the integration is updated; failure mode is loud (errors/rejections), not silent | Watch venue changelogs; re-run the gate after dependency bumps; budget maintenance for venue drift |
| 7 | **Docker path not executed in delivery sandbox** (no daemon) | `release-manifest.json`; `.github/workflows/ci.yml` docker job | Multi-stage non-root healthchecked image, pinned base; `docker compose config` syntax gate; static inspection done | Image/compose defects would surface only at the buyer's first build | Run `docker compose up --build` + health smoke early in acceptance (`docs/BUYER-QUICKSTART.md` §9) |
| 8 | **CI never run on a real runner** (workflow delivered, unexecuted from delivery environment) | `.github/workflows/ci.yml`; `docs/HANDOVER.md` §3 | Every CI step has a local equivalent that passed in the freeze gate (release-check 20/20 mirrors the workflow); toolchain + service images pinned | Runner-image or service-container quirks possible on first run | Trigger CI on the buyer's remote; fix runner-specific issues before relying on it as a gate |
| 9 | **Secret infrastructure is buyer-side** (env/secret-store quality, key custody) | `docs/SECURITY.md`; `docs/SUPPORT-HANDOVER.md` §6 | Env-var indirection only; secret-scan release gate; signer boundary (modules never see keys); token-redaction regression; non-loopback bind refused without auth | Weak custody ⇒ key compromise ⇒ direct fund loss; the software refuses to embed secrets but cannot protect a leaked environment | Real secret store; rotate every credential at transfer; restrict prod-env readers; consider implementing the `vault`/`kms` signer extension points |
| 10 | **Production infrastructure is buyer-provided** (PostgreSQL ≥ 16, Redis 7, hosting, monitoring) | `docs/SCOPE-BOUNDARY.md` §2; `docs/DEPLOYMENT.md` | PG is the single durable truth with forward-only migrations; Redis non-authoritative by design; probes + metrics ready to wire; backup/restore procedure verified | Undersized/unmonitored PG or hosting ⇒ availability and recovery risk; Redis loss is tolerable by design, PG loss is not | Size and monitor PG (with backups per `docs/BACKUP-RESTORE.md`); run the restore drill once on real infra |
| 11 | **Regulatory / legal responsibility** (crypto trading, prediction markets, token issuance) | README §Disclaimer; `docs/SCOPE-BOUNDARY.md` §4 | Paper default; dual live gates; no automated legal decisions anywhere | Operating in a restricted jurisdiction or against venue ToS ⇒ legal exposure independent of software quality | Jurisdiction-appropriate legal review before funded operation or token distribution; verify Polymarket access legality |
| 12 | **Multi-replica behavior tested at 2 replicas only** | `distributed_integration` 4/4, `two_replica_mirror` 1/1; `docs/DISTRIBUTED.md` | Backend-atomic claims (PG authoritative); fencing tokens; `execution_claim_events` lineage for forensics; tighten-only risk oracle | Unforeseen coordination edge cases possible at larger N (invariant mechanisms are N-general, but untested beyond 2) | Stay ≤ 2 active replicas until larger topologies are exercised in buyer staging |
| 13 | **Ownership/legal metadata placeholders** (LICENSE holder, repo URL, security contact) | `docs/HANDOVER.md` §5; `LICENSE`; root `SECURITY.md` | Placeholders are deliberate and documented; no fake values ship; version identity gated | Ambiguous copyright attribution; misrouted vulnerability reports | Complete the three fill-ins at transfer (legal action, not code) |
| 14 | **Point-in-time supply-chain scans** (audit/deny passed at freeze, 2026-09-18) | `docs/THIRD-PARTY.md` §5; `release-manifest.json` | Both lockfiles committed; audit/deny are release + CI hard gates going forward | Advisories published after the freeze are not reflected until re-run | Re-run `cargo audit` / `cargo deny check` periodically and on every dependency change |

## Explicitly NOT risks (documented design properties)

- **Redis failure** — non-authoritative by design; no money-relevant truth
  is lost (`docs/BACKUP-RESTORE.md`).
- **Process crash mid-execution** — intent journal + startup reconciliation +
  dedup make restarts safe; tested (`docs/RECONCILIATION.md`,
  `docs/DEMO-RUNBOOK.md` Demo 6).
- **Audit tampering via app APIs** — impossible: append-only from all APIs,
  hash-chained, `GET /api/audit/verify`; DB-superuser tamper + rehash is
  outside the stated threat model (`docs/SECURITY.md`).
- **Accidental live trading** — requires two config gates + owner-only
  runtime switch + real keys; one misconfiguration cannot enable broadcasts.
- **Buyer-owned infrastructure gaps (rows 4, 5, 7, 8, 10)** — these are
  external-infrastructure items, not code defects; the software degrades
  visibly and safely when they are absent.
`````


----- COMPLETE FILE: docs/IP-COMPONENTS.md (13801 bytes, 259 lines) -----

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

## Ownership transfer checklist

Every item that must change hands (or be created) for the buyer to own and
operate the system outright. No credentials or identities are invented here;
each line names the artifact and its state at delivery. Procedural detail:
`docs/SUPPORT-HANDOVER.md`; sign-off format: `docs/ACCEPTANCE-CHECKLIST.md`.

- [ ] **Repository ownership** — the authoritative git repository (history
      `9c677cd` → `0e139c3` + documentation commits) transferred via bundle
      or hosting transfer (`docs/ARCHIVE-CHECKLIST.md`); buyer verifies per
      `docs/BUYER-QUICKSTART.md` §1.
- [ ] **Git hosting ownership** — remote/hosting account (GitHub org or
      equivalent) created or transferred by the buyer; set the real
      `repository` URL in the workspace `Cargo.toml` at that point
      (deliberately absent until then).
- [ ] **License holder** — insert the legal copyright entity into `LICENSE`
      (currently the documented placeholder "sniper-suite authors"); execute
      any IP assignment paperwork between the parties.
- [ ] **Staking program authority** — deploy-keypair custody decided;
      program deployed under the finalized id; `Initialize` called with the
      admin set to the buyer's **multisig PDA** (Squads/Realms recommended);
      timelock ≥ 24h; one-shot `GenesisMint` recipient and distribution plan
      owned by the buyer (`docs/STAKING.md`).
- [ ] **Deployment credentials** — host/container credentials, SSH keys,
      orchestrator tokens: created by the buyer; nothing ships.
- [ ] **RPC provider accounts** — Solana RPC/WS (and failover) provider
      contracts in the buyer's name; API keys issued by the provider.
- [ ] **Geyser / PumpPortal accounts** — optional feed-provider contracts in
      the buyer's name (poll fallback exists without them).
- [ ] **Polymarket credentials** — Polymarket API access + Polygon key
      generated and held by the buyer; buyer satisfies Polymarket ToS and
      applicable law.
- [ ] **Telegram bot ownership** — bot created (or transferred) via
      BotFather under the buyer's account; token rotated; allow-lists set to
      the buyer's owner/operator/readonly ids.
- [ ] **Monitoring ownership** — Prometheus/log/alerting stack stood up by
      the buyer; scrape target `/metrics`; runbook `docs/OPERATIONS.md`.
- [ ] **Domain ownership** — any domains/reverse-proxy/TLS certificates for
      API exposure: registered and held by the buyer (the server refuses
      non-loopback binds without API auth until then).
- [ ] **CI secrets** — GitHub (or equivalent) environment secrets/tokens for
      CI runs: created by the buyer in their own CI environment.
- [ ] **Docker registry ownership** — image registry namespace/account:
      buyer's; images are built by the buyer from the delivered Dockerfile
      (no images ship).
- [ ] **Backup ownership** — backup destination, schedule and restore
      responsibility: buyer's, per `docs/BACKUP-RESTORE.md`; perform one
      dump→restore drill at acceptance.
- [ ] **Credential rotation** — every credential that ever existed on either
      side rotated at transfer (`docs/SUPPORT-HANDOVER.md` §6), regardless of
      the clean secret-scan evidence.
`````


----- COMPLETE FILE: docs/DELIVERY-MANIFEST.md (10470 bytes, 105 lines) -----

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
- Documentation passes after the freeze (no source bytes changed — proven in
  each pass report): +14 buyer-package docs (160 files / 2,919,949 bytes),
  then +9 final-delivery docs + `scripts/verify-delivery.sh` (170 files).

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

## Buyer package (14 docs, commercialization pass) + final delivery package (9 docs)

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
| [`docs/FINAL-DELIVERY.md`](FINAL-DELIVERY.md) | **Single human-readable starting point**: contents, version/commits, sizes, test & release evidence, taxonomy, components, doc map, infrastructure, buyer actions, limitations, ownership checklist. |
| [`docs/BUYER-QUICKSTART.md`](BUYER-QUICKSTART.md) | 18-step technical quick start: bundle verification → toolchain → PG/Redis → secrets → release-check → paper → probes/metrics/dashboard → Telegram authz → simulate → backup/restore → audit-chain review. |
| [`docs/TECHNICAL-FACT-SHEET.md`](TECHNICAL-FACT-SHEET.md) | One-page-per-topic fact sheet: language, architecture, crates, API, execution, persistence, reconciliation, distributed, observability, staking, tests, CI, scanning, Docker, security, audit status. |
| [`docs/SELLER-FACT-SHEET.md`](SELLER-FACT-SHEET.md) | Factual source document for seller use (listing composition, buyer Q&A). Not an advertisement; explicit non-claims list. |
| [`docs/SELLING-LISTING-SOURCE.md`](SELLING-LISTING-SOURCE.md) | Reusable factual listing material: title candidates, technical summary, feature/architecture/testing/deployment/security facts, deliverables, limitations, transfer requirements. |
| [`docs/DEMO-RUNBOOK.md`](DEMO-RUNBOOK.md) | 10 deterministic buyer demos (paper, simulate, probes/metrics, risk rejection, kill switch, restart/recovery, audit chain, distributed claims, staking, backup/restore) with commands, expected results, verification status. |
| [`docs/EVIDENCE-INDEX.md`](EVIDENCE-INDEX.md) | Claim → evidence file/section/status map for every major assertion in the package, with re-verification instructions. |
| [`docs/REPOSITORY-MAP.md`](REPOSITORY-MAP.md) | Annotated file/folder map of the actual delivered tree with exact counts. |
| [`docs/ARCHIVE-CHECKLIST.md`](ARCHIVE-CHECKLIST.md) | Final seller-archive specification: INCLUDE/EXCLUDE lists, bundle production procedure, integrity requirements. |

Delivery tooling added in the final pass: [`scripts/verify-delivery.sh`](../scripts/verify-delivery.sh)
(fast, fail-closed bundle-integrity check — required files, version identity,
docs count, hygiene, markdown links, invisible characters; complements, does
not duplicate, `scripts/release-check.sh`).

## Suggested reading order for a technical buyer

1. `docs/FINAL-DELIVERY.md` — the single starting point (identity, evidence,
   statuses, actions).
2. `docs/BUYER-QUICKSTART.md` — hands-on verification walkthrough.
3. `docs/BUYER-OVERVIEW.md` + `docs/TECHNICAL-FACT-SHEET.md` — what the
   system is.
4. `docs/CAPABILITY-MATRIX.md` + `docs/EVIDENCE-INDEX.md` — what is
   implemented, how it was tested, and where each claim is evidenced.
5. `docs/BUYER-DUE-DILIGENCE.md` + `docs/ACCEPTANCE-CHECKLIST.md` — how to
   verify everything independently and sign off.
6. `docs/BUYER-RISK-REGISTER.md` + `docs/SCOPE-BOUNDARY.md` — what remains
   open and who owns what.
7. `docs/BUYER-DEPLOYMENT.md` + `docs/DEMO-RUNBOOK.md` — how to stand it up
   (paper mode first) and demonstrate it.
8. Deep dives as needed: the 13 engineering docs, `AUDIT.md` for evidence
   history, `release-manifest.json` for machine-readable facts,
   `docs/REPOSITORY-MAP.md` + `docs/ARCHIVE-CHECKLIST.md` for the physical
   bundle.
`````


----- COMPLETE FILE: README.md (29539 bytes, 576 lines) -----

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

Start at **[docs/FINAL-DELIVERY.md](docs/FINAL-DELIVERY.md)** — the single
delivery index (version, commits, evidence, statuses, buyer actions). Then:

- [docs/BUYER-QUICKSTART.md](docs/BUYER-QUICKSTART.md) — 18-step hands-on
  verification (paper/simulate only; no live trading).
- [docs/BUYER-DUE-DILIGENCE.md](docs/BUYER-DUE-DILIGENCE.md) — independent
  verification checklist.
- [docs/ACCEPTANCE-CHECKLIST.md](docs/ACCEPTANCE-CHECKLIST.md) — sign-off list.
- [docs/BUYER-RISK-REGISTER.md](docs/BUYER-RISK-REGISTER.md) — remaining risks.
- [docs/DELIVERY-MANIFEST.md](docs/DELIVERY-MANIFEST.md) — index of the whole
  23-document buyer/delivery package.

These documents were added after the 0.1.0 engineering freeze; no source
code changed. Machine-readable facts:
[release-manifest.json](release-manifest.json). Bundle integrity check:
`./scripts/verify-delivery.sh`.

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
├─ scripts/                     release-check.sh (20-gate release validation)
│                               + verify-delivery.sh (bundle integrity check)
├─ docs/                        36 docs: 13 engineering (architecture, API,
│                               security, ops, release, handover, backup/
│                               restore, testing…) + 23 buyer/delivery docs
│                               (index: docs/DELIVERY-MANIFEST.md; start:
│                               docs/FINAL-DELIVERY.md)
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


----- COMPLETE FILE: docs/HANDOVER.md (8190 bytes, 151 lines) -----

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
  DISTRIBUTED, RELEASE, HANDOVER (this file), BACKUP-RESTORE — plus 23
  buyer/delivery documents added after the engineering freeze (buyer
  package: BUYER-OVERVIEW, CAPABILITY-MATRIX, BUYER-DUE-DILIGENCE,
  IP-COMPONENTS, THIRD-PARTY, BUYER-DEPLOYMENT, ACCEPTANCE-CHECKLIST,
  RELEASE-NOTES-0.1.0, BUYER-FAQ, SCOPE-BOUNDARY, SUPPORT-HANDOVER,
  BUYER-RISK-REGISTER, TECHNICAL-DIFFERENTIATORS, DELIVERY-MANIFEST;
  final delivery package: FINAL-DELIVERY, BUYER-QUICKSTART,
  TECHNICAL-FACT-SHEET, SELLER-FACT-SHEET, SELLING-LISTING-SOURCE,
  DEMO-RUNBOOK, EVIDENCE-INDEX, REPOSITORY-MAP, ARCHIVE-CHECKLIST;
  index: `docs/DELIVERY-MANIFEST.md`, start: `docs/FINAL-DELIVERY.md`).
  No source code changed in those passes; the only executable added is
  `scripts/verify-delivery.sh` (documentation/bundle integrity checker —
  it builds and tests nothing).
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


----- COMPLETE FILE: CHANGELOG.md (8442 bytes, 144 lines) -----

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

### Added (final delivery package — documentation + bundle tooling only)

- 9 final-delivery documents under `docs/`: FINAL-DELIVERY (single starting
  point), BUYER-QUICKSTART (18-step walkthrough), TECHNICAL-FACT-SHEET,
  SELLER-FACT-SHEET, SELLING-LISTING-SOURCE (factual listing source
  material — not advertising), DEMO-RUNBOOK (10 deterministic demos),
  EVIDENCE-INDEX (claim → evidence map), REPOSITORY-MAP (annotated tree),
  ARCHIVE-CHECKLIST (seller archive INCLUDE/EXCLUDE spec).
- `scripts/verify-delivery.sh` — fast, fail-closed bundle-integrity check
  (required files, version identity, docs count, hygiene, markdown links,
  invisible characters). Complements `release-check.sh`; builds/tests
  nothing.
- CAPABILITY-MATRIX and BUYER-RISK-REGISTER finalized to the delivery column
  schema; IP-COMPONENTS gained the ownership-transfer checklist;
  DELIVERY-MANIFEST indexes all 23 buyer/delivery docs; README/HANDOVER
  pointers and `release-manifest.json` `docs_count` (27 → 36) updated.
  Still zero changes to Rust, SQL, migrations, configs, Docker or CI files.

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
    "docs_count": 36
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

## 6. CODE-INTEGRITY / BYTE-INTEGRITY RESULT

Frozen baseline (commit `0e139c3`) vs current tree, by measurement:

| Category | Frozen baseline | Current | Verdict |
|---|---|---|---|
| Rust (91 `.rs`: production + tests, app + program) | 2,051,936 B (= 1,826,288 prod + 225,648 tests) | **2,051,936 B** | UNCHANGED (byte-exact) |
| SQL migrations (11 files) | 23,443 B | **23,443 B** | UNCHANGED (byte-exact) |
| All non-markdown files (configs, lockfiles, Docker, CI, scripts, manifest) | 129 files / 2,507,638 B | 130 files / **2,514,700 B = 2,507,638 + 7,062** (the new `verify-delivery.sh`) | UNCHANGED except the one new script (exact arithmetic) |
| Both Cargo.lock files | 201,147 + 160,243 B | identical | UNCHANGED |
| 14 frozen engineering/root docs never edited in any doc pass | 251,616 B | **251,616 B** | UNCHANGED (byte-exact) |
| `cargo fmt --all --check` (pinned 1.98.1) | clean | **clean** | confirms zero Rust drift |

Conclusion: **Rust unchanged, SQL unchanged, migrations unchanged, configs unchanged, deployment unchanged.** The only executable added in any documentation pass is `scripts/verify-delivery.sh`, which builds/tests nothing and is scanned by no release-check gate pattern (gates target `.rs/.sql/.toml/.yml/Dockerfile/.json`).

## 7. DOCUMENTATION CONSISTENCY RESULT

Two independent checkers, both green:

1. **`scripts/verify-delivery.sh` (in-repo, delivered):** 7 PASS / 0 FAIL, exit 0 — required files (58 checked) + migrations 0001–0011 present; version identity `0.1.0` three-way; manifest `docs_count` 36 == actual 36; hygiene clean (no .env/target/build/logs/dumps/keypairs/pem); all relative markdown links across README + 36 docs resolve; no zero-width/bidi characters.
2. **Session checker (`/home/user/doccheck.py`, extended for this pass; deliberately not shipped in the tree):** 40 markdown files, 60 referenced source paths, 170 repo files — `DOC CONSISTENCY: CLEAN`, 0 findings. Adds: stale-count detection (520/518/21 absent from all new docs; HANDOVER's legitimate historical "earlier 518/518" sentence exempt per §18), canonical-count presence (521/521, 23/23, 10/10, 4/4, 1/1, 48/48, 2/2, 20 PASS/0 FAIL/0 SKIP, `9c677cd`, `0e139c3`, 146, 2,801,590), manifest sanity, referenced-path existence, exact file count (170 = 146 + 14 + 9 + 1).

No historical evidence was modified to eliminate a legitimate historical count.

## 8. VERIFICATION RESULTS

| Check | Result |
|---|---|
| `scripts/verify-delivery.sh` | **PASS** — 7/7, exit 0 |
| Documentation consistency checker | **PASS** — CLEAN, 0 findings |
| `cargo fmt --all --check` | **PASS** — pinned toolchain 1.98.1, zero diff |
| `git diff --check` | **NOT EXECUTED** — `.git` absent in this sandbox (infrastructure re-provision). Per §20: no history recreated, no hashes invented. Authoritative history `9c677cd` → `0e139c3` (+ documentation commits) must be preserved in the seller's real repository; the archive procedure is specified in `docs/ARCHIVE-CHECKLIST.md` and explicitly must be run there. |
| `./scripts/release-check.sh` | **NOT RE-RUN (per §22)** — no code/config changed (byte-proven §6); PG/Redis/build cache absent in this sandbox. Authoritative result on identical software bytes: **20 PASS / 0 FAIL / 0 SKIP, exit 0** (freeze run, `AUDIT.md` §27). The new script adds no gate interference (verified against release-check's scan patterns). |
| Full test suite (521 etc.) | **NOT RE-RUN (per §22)** — previous engineering evidence remains authoritative while software bytes are proven unchanged. |

Final delivery tree: **170 files / 3,021,664 bytes (3.02 MB) / 81,583 lines** (frozen software 146 / 2,801,590 / 77,980 + 23 buyer/delivery docs + 1 script + doc edits).

## 9. BUYER-DELIVERY CHECKLIST

- [x] FINAL-DELIVERY.md — single starting point (identity, evidence, taxonomy, actions).
- [x] BUYER-QUICKSTART.md — 18-step hands-on verification, ends before live trading.
- [x] TECHNICAL-FACT-SHEET.md — repository-backed facts only, no adjectives.
- [x] CAPABILITY-MATRIX.md — final schema, 24 capabilities, honest statuses.
- [x] BUYER-RISK-REGISTER.md — 14 risks + 5 non-risks, all required categories.
- [x] IP-COMPONENTS.md — inventory + 15-item ownership transfer checklist.
- [x] THIRD-PARTY.md — reviewed; complete without changes.
- [x] DELIVERY-MANIFEST.md — indexes all 20 required artifacts/docs + script.
- [x] README entry point + CHANGELOG package entries (history untouched).
- [x] SELLER-FACT-SHEET.md + SELLING-LISTING-SOURCE.md — factual, non-deceptive, explicit non-claims.
- [x] DEMO-RUNBOOK.md — 10 deterministic demos, no live trading.
- [x] EVIDENCE-INDEX.md — claim → evidence → status map + re-verification guide.
- [x] REPOSITORY-MAP.md — actual tree, exact counts, nothing invented.
- [x] ARCHIVE-CHECKLIST.md — INCLUDE/EXCLUDE + bundle procedure (to run on the authoritative repo).
- [x] verify-delivery.sh — fail-closed bundle check, executed green.
- [x] Consistency + byte-integrity + fmt verification (§6–8).

## 10. FINAL ARCHIVE CONTENTS (specification — archive produced on the authoritative repo, not here)

INCLUDE: full source (`crates/` + `programs/staking-suite/`), both Cargo.lock files, all 11 migrations, all 36 docs + README/CHANGELOG/AUDIT/SECURITY/LICENSE/VERSION, config examples (`config.toml.example`, `.env.template`, `rust-toolchain.toml`, `deny.toml`, `.cargo/audit.toml`), deployment assets (Dockerfile, compose, .dockerignore, ci.yml), release assets (release-manifest.json, both scripts), and the **real git history** (`git bundle --all` containing `9c677cd` → `0e139c3` + documentation commits).
EXCLUDE: target/, build/, DB dumps/data, Redis data, `.env`, secrets/keypairs/pem, logs, installer temporaries, editor/OS junk.
Full specification + production commands + contamination check: `docs/ARCHIVE-CHECKLIST.md`.

## 11. EXTERNAL/HUMAN REQUIREMENTS (unchanged, authoritative list)

1. Legal copyright holder → `LICENSE`. 2. Real security contact → root `SECURITY.md`. 3. Real repository URL on publish. 4. Staking program deployment + program-id finalization (placeholder today). 5. Independent external security audit before staking mainnet. 6. Production infrastructure (PG ≥ 16, Redis 7, RPC/WS ± Geyser/PumpPortal, hosting, monitoring, secret store, CI runners, Docker daemon, funded keys). 7. First real Docker build+smoke and CI run. 8. Gradual supervised funded live-trading validation. 9. Archive production from the authoritative repository (§10). 10. Credential rotation at transfer (`docs/SUPPORT-HANDOVER.md` §6).

## 12. FINAL HANDOVER INDEX

`docs/FINAL-DELIVERY.md` (start) → `docs/BUYER-QUICKSTART.md` (hands-on) → `docs/DELIVERY-MANIFEST.md` (index of all 36 docs + assets) → `docs/EVIDENCE-INDEX.md` (claim→evidence) → `docs/ACCEPTANCE-CHECKLIST.md` (sign-off) → `docs/ARCHIVE-CHECKLIST.md` (physical bundle) → `release-manifest.json` (machine-readable facts).

Classification at stop: **VERIFIED** — freeze-gate evidence on identical software bytes (521/521, 23/23, 10/10, 4/4, 1/1, 48/48, 20/20, audit/deny/fmt/clippy), byte-integrity proof, both documentation checkers, fmt. **PREVIOUSLY VERIFIED** — build-sbf, validator e2e 2/2, recon_crash_e2e, devnet e2e, latency_bench, ledger replay. **BUYER ACTION REQUIRED** — §11 items 1–5, 8, 10 + acceptance checklist. **EXTERNAL INFRASTRUCTURE REQUIRED** — §11 items 6–7, 9.

**STOP CONDITION MET.** The delivery package is complete; the next stage is the seller/buyer transfer process using the real authoritative repository history — not additional engineering.
