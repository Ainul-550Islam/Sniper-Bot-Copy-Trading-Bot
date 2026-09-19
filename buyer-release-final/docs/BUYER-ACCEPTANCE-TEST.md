# Buyer acceptance test — formal record (final handover edition)

Supersedes the 22-step edition of the hardening pass (kept in git-less
history via `AUDIT.md` §29 references and the package `docs/` copy of that
edition is replaced BY THIS FILE; the older vendor package
`buyer-release/` retains the earlier snapshot — labeled, never deleted).

**How to use this document.** Each test has fixed fields. The BUYER fills
ACTUAL RESULT, PASS/FAIL, INITIALS, DATE after executing the COMMAND under
the PRECONDITIONS on the buyer's own machine. "Vendor-recorded evidence" is
REFERENCE ONLY — it proves what the vendor executed and where the artifact
lives; it is never a substitute for the buyer's own PASS. **Nothing in this
document is pre-marked PASS.** A test may only be marked PASS if the buyer
observed the EXPECTED RESULT.

Status vocabulary used in vendor-evidence references: PASS (executed,
artifact exists) / NOT RUN / BLOCKED (vendor environment lacked the
resource) / HUMAN ACTION (only the buyer can perform it).

Placeholders: `<REPO>` = unpacked source root; `<PKG>` = release package
root; `<ANGLE>` = operator-supplied value.

---

## A. Source verification

### A1 — Tree hash
- PRECONDITIONS: package extracted; `sha256sum` available.
- COMMAND: `cd <REPO> && find . -type f -not -path './target/*' -not -path './programs/staking-suite/target/*' | sort | xargs sha256sum | sha256sum`
- EXPECTED: equals `source_tree_sha256` in `<PKG>/BUYER-FINAL-RELEASE-MANIFEST.json`.
- Vendor-recorded evidence: PASS — `<PKG>/provenance/SOURCE-PROVENANCE.json` (per-file hashes + tree hash of the delivered tree).
- ACTUAL RESULT: ______  PASS / FAIL: ____  INITIALS: ____  DATE: ______

### A2 — Per-file checksums
- PRECONDITIONS: A1 done.
- COMMAND: `cd <PKG> && sha256sum -c checksums/SHA256SUMS`
- EXPECTED: every line `OK`, 0 failures.
- Vendor-recorded evidence: PASS — vendor self-check recorded in `<PKG>/checksums/SHA256SUMS.json` (`failures: 0` at packaging time).
- ACTUAL RESULT: ______  PASS / FAIL: ____  INITIALS: ____  DATE: ______

### A3 — Archive reproduction
- PRECONDITIONS: `<PKG>/source/sniper-suite-FINAL-src.tar.gz` present.
- COMMAND: `sha256sum <PKG>/source/sniper-suite-FINAL-src.tar.gz` then extract into an empty dir and diff against `<PKG>/source-tree/`.
- EXPECTED: archive hash equals manifest `archive_sha256`; diff shows 0 missing / 0 extra / 0 changed.
- Vendor-recorded evidence: PASS — double-extraction comparison recorded in `docs/FINAL-RELEASE-AUDIT.md`.
- ACTUAL RESULT: ______  PASS / FAIL: ____  INITIALS: ____  DATE: ______

## B. Build

### B1 — Toolchain
- PRECONDITIONS: rustup installed (reproduction guide step 2).
- COMMAND: `cd <REPO> && cargo --version && rustc --version`
- EXPECTED: `cargo 1.98.1`, `rustc 1.98.1 (48a229cea 2026-09-01)` (selected by `rust-toolchain.toml`).
- Vendor-recorded evidence: PASS — `evidence/gates/release-check-final.log` (toolchain-pin gate).
- ACTUAL RESULT: ______  PASS / FAIL: ____  INITIALS: ____  DATE: ______

### B2 — Compile + gates
- PRECONDITIONS: B1; system deps (reproduction guide step 3).
- COMMAND: `cargo check --workspace --all-targets && cargo fmt --all --check && cargo clippy --workspace --all-targets -- -D warnings`
- EXPECTED: all exit 0; no warnings under `-D warnings`.
- Vendor-recorded evidence: PASS — `evidence/gates/release-check-final.log` gates 7–9.
- ACTUAL RESULT: ______  PASS / FAIL: ____  INITIALS: ____  DATE: ______

### B3 — Full build
- COMMAND: `cargo build --workspace --all-targets`
- EXPECTED: exit 0; `target/debug/sniper-suite` exists.
- Vendor-recorded evidence: PASS — binary built, run, and smoke-tested in `evidence/app-startup/phase8b-*.log`.
- ACTUAL RESULT: ______  PASS / FAIL: ____  INITIALS: ____  DATE: ______

## C. Unit tests

### C1 — Workspace suite (offline-deterministic core)
- PRECONDITIONS: B3. Services NOT required for the offline majority, but totals only reach 537 with services (see D1).
- COMMAND: `cargo test --workspace -- --test-threads=1`
- EXPECTED: 0 failed everywhere; per-suite `test result: ok` lines.
- Vendor-recorded evidence: PASS — 537/537 (`evidence/tests/phase2-test-workspace.log`; re-run inside `release-check-final.log` gate 10).
- ACTUAL RESULT: ______  PASS / FAIL: ____  INITIALS: ____  DATE: ______

### C2 — All-features parity
- COMMAND: `cargo test --workspace --all-features -- --test-threads=1`
- EXPECTED: same totals, 0 failed (no feature-gated delta).
- Vendor-recorded evidence: PASS — 537/537 (`evidence/tests/phase2-test-workspace-allfeat.log`).
- ACTUAL RESULT: ______  PASS / FAIL: ____  INITIALS: ____  DATE: ______

## D. Integration tests (real services)

### D1 — Gated suites execute
- PRECONDITIONS: PostgreSQL ≥16 + Redis ≥7 running; `POSTGRES_URL`, `REDIS_URL` exported.
- COMMAND: `cargo test --workspace -- --test-threads=1` and check the four gated suites ran (not skipped): `db_integration`, `redis_integration`, `distributed_integration`, `two_replica_mirror`.
- EXPECTED: 23 + 10 + 4 + 1 passed inside the 537 total; skip notices ABSENT.
- Vendor-recorded evidence: PASS — PG 17.11 + Redis 8.0.2 (`release-check-final.log` gates 11–14).
- ACTUAL RESULT: ______  PASS / FAIL: ____  INITIALS: ____  DATE: ______

### D2 — Staking host suite
- COMMAND: `cd programs/staking-suite && cargo test`
- EXPECTED: 71 passed / 0 failed (3 e2e compile but stay gated without `STAKING_E2E`).
- Vendor-recorded evidence: PASS — `release-check-final.log` gate 17 (executed on the FINAL source after the D1 discriminant fix).
- ACTUAL RESULT: ______  PASS / FAIL: ____  INITIALS: ____  DATE: ______

## E. Staking build + program identity

### E1 — build-sbf
- PRECONDITIONS: Agave 2.1.21 CLI on PATH (reproduction guide step 2); ~2 GB disk for platform-tools.
- COMMAND: `cd programs/staking-suite && cargo build-sbf && sha256sum target/deploy/staking_suite.so && stat -c %s target/deploy/staking_suite.so`
- EXPECTED: exit 0; 187,504 bytes; SHA-256 `57a890fae273f2c569fc814c43f0645311b6983dd30782126a9844ee193b5564` on Linux x86_64 + platform-tools v1.43 (other platforms: rely on E2 instead of the hash).
- Vendor-recorded evidence: PASS — original build + incremental AND cold byte-identical rebuilds (`evidence/build/…`, `evidence/compile-logs/phase3-sbf-determinism-rerun.log`).
- ACTUAL RESULT: ______  PASS / FAIL: ____  INITIALS: ____  DATE: ______

### E2 — Identity verify (placeholder status)
- COMMAND: `cd <REPO> && ./scripts/staking-identity.sh verify`
- EXPECTED: prints declared id `3vEEMMFmdA88n8ApgZ3b9L3BXEh75yCeMbHbmUjR9mfy`, states it is the PRE-DEPLOYMENT PLACEHOLDER, and reports all tracked references agree (README, BUYER-DUE-DILIGENCE, STAKING, BUYER-ACCEPTANCE-TEST); exit 0.
- Vendor-recorded evidence: PASS — `evidence/phase7-identity-verify.log` (re-run at handover).
- ACTUAL RESULT: ______  PASS / FAIL: ____  INITIALS: ____  DATE: ______

### E3 — Final identity (HUMAN ACTION)
- PRECONDITIONS: buyer generated `program-keypair.json` (`solana-keygen new`), decided cluster + upgrade authority + payer.
- COMMAND: `./scripts/staking-identity.sh set-id program-keypair.json && cargo build-sbf && ./scripts/staking-identity.sh deploy --keypair program-keypair.json --url <RPC>`
- EXPECTED: set-id updates declaration + tracked docs and re-verifies; deploy refuses any keypair≠declare_id mismatch and refuses the placeholder on public clusters; on success prints the deploy tx + on-chain account.
- Vendor-recorded evidence: NOT RUN — no real program id exists; the vendor never deployed and invented nothing. Guards themselves: PASS (script logic + E2).
- ACTUAL RESULT: ______  PASS / FAIL: ____  INITIALS: ____  DATE: ______

## F. Validator e2e

### F1 — All three e2e tests on a real BPF VM
- PRECONDITIONS: E1 toolchain; internet to `api.mainnet-beta.solana.com`.
- COMMAND: `cd programs/staking-suite && STAKING_E2E=1 cargo test --test validator_e2e -- --test-threads=1`
- EXPECTED: `3 passed; 0 failed` (~160 s): governance lifecycle; funded stake→reward→claim→unstake; max-supply cap boundaries + metadata against the REAL mainnet-cloned mpl program + replay rejection.
- Vendor-recorded evidence: PASS — 160.72 s (`evidence/tests/phase5-full-batch.log`).
- ACTUAL RESULT: ______  PASS / FAIL: ____  INITIALS: ____  DATE: ______

## G. Docker

### G1 — Compose config + image build + container smoke
- PRECONDITIONS: Docker Engine installed.
- COMMAND: `docker compose config -q && docker build -t sniper-suite:local . && docker run -d --name smoke -p 127.0.0.1:8080:8080 -e RUST_LOG=info sniper-suite:local && sleep 5 && curl -fsS http://127.0.0.1:8080/api/health && docker stop smoke`
- EXPECTED: config quiet-exit 0; build succeeds; `{"ok":true}` from the container with NO DB/Redis/keys attached (degraded-mode smoke); graceful stop.
- Vendor-recorded evidence: BLOCKED — no Docker daemon in the vendor sandbox (`docker/BLOCKED-NOTE.md`). Native-equivalent binary smoke: PASS (`evidence/app-startup/phase8b-*.log`) — labeled as NOT a container run.
- ACTUAL RESULT: ______  PASS / FAIL: ____  INITIALS: ____  DATE: ______

## H. Database

### H1 — Migrations
- PRECONDITIONS: app or test suite ran once against the DB (C1/D1).
- COMMAND: `psql "$POSTGRES_URL" -c 'SELECT version, description, success FROM _sqlx_migrations ORDER BY version'`
- EXPECTED: 11 rows, versions 1–11, all `success = true`.
- Vendor-recorded evidence: PASS — incl. on a RESTORED database (`evidence/db/phase8-rst-migrations.txt`).
- ACTUAL RESULT: ______  PASS / FAIL: ____  INITIALS: ____  DATE: ______

### H2 — App start + health/ready/status
- COMMAND: reproduction guide steps 8–9 (start binary; curl `/health`, `/ready`, `/api/health`, `/api/status`, `/metrics`).
- EXPECTED: 200s; `/ready` component list; `/api/status` shows `execution_mode "paper"`, `live_allowed false`, `kill_switch false`; `bot_*` metrics present.
- Vendor-recorded evidence: PASS — `evidence/app-startup/phase8b-endpoints.log`.
- ACTUAL RESULT: ______  PASS / FAIL: ____  INITIALS: ____  DATE: ______

## I. Recovery

### I1 — Crash + restart
- COMMAND: reproduction guide step 12 (`kill -9`, restart).
- EXPECTED: startup recovery reloads positions, re-registers unfinished orders as `Unknown`, gates module spawn on the reconcile pass; no data loss.
- Vendor-recorded evidence: PASS (tests inside C1/D1: recovery + startup-reconcile; `recon_crash_e2e` executed on pre-hardening source — labeled historical in `docs/HANDOVER.md` §3).
- ACTUAL RESULT: ______  PASS / FAIL: ____  INITIALS: ____  DATE: ______

### I2 — Clean shutdown
- COMMAND: reproduction guide step 13 (`kill -TERM`).
- EXPECTED: ordered drain; final log line `sniper-suite stopped cleanly`; exit 0.
- Vendor-recorded evidence: PASS — `evidence/app-startup/phase8b-shutdown.log`.
- ACTUAL RESULT: ______  PASS / FAIL: ____  INITIALS: ____  DATE: ______

## J. Telegram

### J1 — Authorization model (live bot)
- PRECONDITIONS: HUMAN ACTION — real bot token + owner chat id configured per `.env.template`.
- COMMAND: from a non-allowlisted chat send any command; from the owner chat send a mutating command and a live-mode command.
- EXPECTED: unknown chat rejected; mutating command requires owner role; live-mode command owner-only; token never appears in any error string.
- Vendor-recorded evidence: PASS for all four behaviors as regression tests inside C1 (mock API); live-bot execution: NOT RUN (no vendor token — HUMAN ACTION).
- ACTUAL RESULT: ______  PASS / FAIL: ____  INITIALS: ____  DATE: ______

## K. Sniper paper mode

### K1 — Paper trading end-to-end (no funds)
- PRECONDITIONS: H2 running in paper mode; sniper module enabled via API/config with a public RPC.
- COMMAND: enable module; feed a launch (or wait for devnet/mainnet feed events per config); observe order lifecycle via `/api` endpoints + logs.
- EXPECTED: entries sized against the paper demo balance; nothing broadcast (paper); fills simulated; positions/orders persisted in PG; kill switch stops new entries immediately.
- Vendor-recorded evidence: PASS for the underlying mechanics (feed mocks + executor paper path inside C1; `/api/status` paper observed in H2's vendor run). Full paper session on buyer feeds: NOT RUN by vendor.
- ACTUAL RESULT: ______  PASS / FAIL: ____  INITIALS: ____  DATE: ______

## L. Copy trading simulation

### L1 — Simulate-mode mirror against a real RPC
- PRECONDITIONS: `EXECUTION_MODE=simulate`; real Solana RPC configured; tracked wallet set.
- COMMAND: enable copy module; observe a tracked wallet's swap; watch the mirror build + SIMULATE (never broadcast).
- EXPECTED: transaction constructed and simulated against the real RPC; broadcast count stays 0; RPC failures surface as typed errors (no cached-balance fallback outside paper).
- Vendor-recorded evidence: PASS for simulate mechanics (executed devnet simulate legs — `evidence/benchmarks/`; two-replica safety in D1). Buyer-RPC session: NOT RUN by vendor.
- ACTUAL RESULT: ______  PASS / FAIL: ____  INITIALS: ____  DATE: ______

## M. Polymarket paper/simulation

### M1 — Live reads, no orders (balance/decimals/allowance)
- PRECONDITIONS: Polygon RPC + funder address configured (read-only; no key needed).
- COMMAND: `docs/LIVE-VALIDATION.md` §1 raw `eth_call` reads (selectors 70a08231/313ce567/dd62ed3e) and compare with the app's reported figures in simulate mode.
- EXPECTED: on-chain reads match the app's sizing inputs; stale (>15 s) or failed reads REJECT orders with typed `BalanceUnavailable`/`InsufficientFunding` — never a paper fallback.
- Vendor-recorded evidence: PASS for wire format + reject-over-fallback (mock-RPC tests inside C1, incl. `rpc_errors_are_errors_never_zero`); real-network funded path: HUMAN ACTION (§O).
- ACTUAL RESULT: ______  PASS / FAIL: ____  INITIALS: ____  DATE: ______

## N. Live safety checks

### N1 — Live mode refuses unsafe startup
- COMMAND: reproduction guide step 10 (signer_backend="vault" scratch config; also verify `allow_live_trading=false` blocks broadcast even in live mode).
- EXPECTED: typed startup failure for unsupported signer backends; `may_broadcast` gate keeps broadcasts off without the explicit flag; kill switch set → live switch refused.
- Vendor-recorded evidence: PASS — gate tests inside C1; paper/`live_allowed:false` observed in H2 vendor run.
- ACTUAL RESULT: ______  PASS / FAIL: ____  INITIALS: ____  DATE: ______

## O. Buyer-controlled live validation (HUMAN ACTION — real funds)

### O1 — Funded procedure
- PRECONDITIONS: buyer decision + dedicated small amounts + own keys; NEVER vendor-executed.
- COMMAND: follow `docs/LIVE-VALIDATION.md` stages in order (Polymarket: RPC/contract/balance/decimals/allowance/spender/live sizing/rejections/order lifecycle; Solana: RPC/wallet/tx construction/simulation/operator-controlled canary/confirmation/reconciliation).
- EXPECTED: each stage's documented observable; any deviation → stop and reconcile before continuing.
- Vendor-recorded evidence: NOT RUN — funded live trading was never executed by the vendor; simulation ≠ funded, paper ≠ live.
- ACTUAL RESULT: ______  PASS / FAIL: ____  INITIALS: ____  DATE: ______

## P. Backup / restore

### P1 — Round-trip
- COMMAND: `docs/BACKUP-RESTORE.md` §2–§3 (pg_dump -Fc → fresh DB → pg_restore --no-owner), then H1 + audit-verify against the restored DB, then start the app against it.
- EXPECTED: tables/migrations/rowcounts identical; audit chain verifies (`intact`, or `not_chained` on an empty chain); app starts and serves H2 endpoints; clean shutdown.
- Vendor-recorded evidence: PASS — dump `5989ecf1…`, 24 tables/68 rows/11 migrations identical ×3, 23/23 suite ON the restored DB, app ran against it (`evidence/db/`, `evidence/app-startup/`).
- ACTUAL RESULT: ______  PASS / FAIL: ____  INITIALS: ____  DATE: ______

## Q. Security evidence

### Q1 — Dependency + static scans
- COMMAND: `cargo audit` (both lockfiles), `cargo deny check`, `grep -rn 'unsafe {' crates programs --include='*.rs' | wc -l`, `./scripts/release-check.sh` (includes marker + secret scans).
- EXPECTED: audit 0 errors (9 allow-listed warnings per `.cargo/audit.toml`); deny advisories/bans/licenses/sources ok; 0 unsafe blocks; scans clean; release-check 20/0/0.
- Vendor-recorded evidence: PASS — `evidence/security/*` + `release-check-final.log`; 18-area map in `evidence/security/SECURITY-EVIDENCE-INDEX.md`. NO external security audit exists — commission one before mainnet staking deployment.
- ACTUAL RESULT: ______  PASS / FAIL: ____  INITIALS: ____  DATE: ______

## R. Release hashes + CI

### R1 — Release hash manifest
- COMMAND: A2 + verify headline values against `<PKG>/BUYER-FINAL-RELEASE-MANIFEST.json` (version, counts, .so hash, lockfile hashes).
- EXPECTED: all consistent; 0 mismatches.
- Vendor-recorded evidence: PASS — manifest + `checksums/SHA256SUMS{,.json}`.
- ACTUAL RESULT: ______  PASS / FAIL: ____  INITIALS: ____  DATE: ______

### R2 — CI on buyer infrastructure
- PRECONDITIONS: repository pushed to the buyer's GitHub (or adapted runner).
- COMMAND: push → Actions runs `.github/workflows/ci.yml` (4 jobs).
- EXPECTED: all jobs green; record the run URL as evidence.
- Vendor-recorded evidence: BLOCKED — no runner in the vendor environment; every CI step has an executed local 1:1 equivalent (`docs/CI-LOCAL-EQUIVALENCE.md`). Local ≠ GitHub run.
- ACTUAL RESULT: ______  PASS / FAIL: ____  INITIALS: ____  DATE: ______

---

## Sign-off

| Role | Name | Signature | Date |
|---|---|---|---|
| Buyer engineer | | | |
| Buyer approver | | | |
| Seller representative | | | |

Completion rule: every section A–R has a filled PASS/FAIL + initials. Any
FAIL stops acceptance until resolved or explicitly waived in writing by the
buyer approver. BLOCKED/HUMAN-ACTION vendor references never count as buyer
PASS entries.
