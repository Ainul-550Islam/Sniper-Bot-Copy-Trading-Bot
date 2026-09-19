# Buyer acceptance test — independent verification procedure

Follow these 22 steps IN ORDER on your own machine to independently verify
the delivery. Every step states the exact command, the expected observable
result, and what the result proves. Values in `<ANGLE_BRACKETS>` are
operator-supplied. Nothing here requires contacting the vendor.

**Prerequisites:** Linux x86_64 (or macOS for steps not needing Docker),
≥ 8 GB RAM, ≥ 30 GB free disk, internet access, `curl`, `git` (optional),
Python 3. Toolchain installs are part of the steps.

**Evidence labels used below:** PASS (you observed it), BLOCKED (needs a
resource only you have), HUMAN ACTION (decision/asset only you can supply).
The vendor's own recorded results live in `docs/EVIDENCE-INDEX.md` and the
`evidence/` directory of the release package — this document is for
producing YOUR OWN evidence.

---

## 1. Verify source hash

```bash
cd <REPO_ROOT>
find . -type f -not -path './target/*' -not -path './programs/staking-suite/target/*' | sort | xargs sha256sum | sha256sum
```

Expected: a single tree hash. Compare per-file hashes against
`evidence/source-inventory.csv` from the release package (columns:
path,bytes,lines,sha256). The vendor's recorded inventory at handover:
175 files / see `evidence/source-inventory.json` `tree_sha256` and the
release manifest `H. FINAL RELEASE HASH MANIFEST`.
Any mismatch = stop and diff the file list.

## 2. Verify toolchain

```bash
rustup toolchain install 1.98.1 --profile minimal -c rustfmt -c clippy
cd <REPO_ROOT> && cargo --version && rustc --version
```

Expected: `cargo 1.98.1`, `rustc 1.98.1 (48a229cea 2026-09-01)` — the repo's
`rust-toolchain.toml` selects this automatically. Cross-check the pin in
`Dockerfile` (`rust:1.98.1-bookworm`) and `.github/workflows/ci.yml`
(program job `dtolnay/rust-toolchain@1.98.1`). Or run
`./scripts/release-check.sh` — its "toolchain pin consistency" gate fails on
drift.

## 3. Build

```bash
cargo check --workspace --all-targets     # fast structural pass
cargo build --workspace --all-targets     # full build incl. bins/tests
```

Expected: exit 0, no errors. Warnings: at most the 6 tracked deprecation
allows documented in `docs/HANDOVER.md`.

## 4. Run tests

Start real services first (step 8 can be done earlier if you prefer Docker):

```bash
# bare metal example (or use the compose stack, step 19)
pg_ctlcluster 17 main start 2>/dev/null || sudo service postgresql start
redis-server --daemonize yes 2>/dev/null || sudo service redis-server start
export POSTGRES_URL=postgres://postgres@127.0.0.1:5432/postgres
export REDIS_URL=redis://127.0.0.1:6379
cargo test --workspace -- --test-threads=1
cargo test --workspace --all-features -- --test-threads=1
```

Expected: **537 passed / 0 failed** twice (the gated db/redis/distributed/
two-replica suites EXECUTE when the env vars are present; without them they
skip loudly and the totals drop — that is visible in the output, never
silent). `--test-threads=1` is required (the db suite shares one database).

## 5. Build staking program

```bash
# Agave 2.1.21 exactly (newer CLIs are documented-incompatible in ci.yml)
sh -c "$(curl -sSfL https://release.anza.xyz/v2.1.21/install)" || \
  curl -sSL -o /tmp/agave.tar.bz2 https://github.com/anza-xyz/agave/releases/download/v2.1.21/solana-release-x86_64-unknown-linux-gnu.tar.bz2
export PATH="<AGAVE_BIN_DIR>:$PATH"
solana --version    # expected: solana-cli 2.1.21
cd programs/staking-suite && cargo build-sbf
sha256sum target/deploy/staking_suite.so
```

Expected: exit 0; `target/deploy/staking_suite.so` exists (187,504 bytes on
Linux x86_64 with platform-tools v1.43). Vendor's recorded SHA-256:
`57a890fae273f2c569fc814c43f0645311b6983dd30782126a9844ee193b5564`.
Same OS/toolchain ⇒ expect the identical hash (vendor verified a
byte-identical rebuild). Different OS ⇒ hash may differ; re-run the e2e
(step 7) instead of trusting the hash.

## 6. Verify program ID

```bash
./scripts/staking-identity.sh verify
```

Expected output: the declared id
`3vEEMMFmdA88n8ApgZ3b9L3BXEh75yCeMbHbmUjR9mfy`, the notice that it is the
PRE-DEPLOYMENT PLACEHOLDER, and `verify: all tracked references agree`.
This proves: single source of truth (`declare_id!` in
`programs/staking-suite/src/lib.rs`), README/STAKING/BUYER-DUE-DILIGENCE
quote the same id, and no stale ids exist elsewhere.

**HUMAN ACTION (before any deployment):** generate YOUR program keypair
(`solana-keygen new -o program-keypair.json`), then
`./scripts/staking-identity.sh set-id program-keypair.json` (updates
source + docs atomically and re-verifies), rebuild (step 5), and deploy via
`./scripts/staking-identity.sh deploy --keypair program-keypair.json --url <RPC>`.
The script REFUSES: keypair ≠ declare_id, placeholder id on a public
cluster, stale binary.

## 7. Run validator e2e

```bash
cd programs/staking-suite
STAKING_E2E=1 cargo test --test validator_e2e -- --test-threads=1
```

Expected: **3 passed / 0 failed** (vendor record: 160.72 s). Requires
`solana-test-validator` on PATH (step 5 toolchain) and internet access (the
metadata test clones the real mpl-token-metadata program from mainnet-beta
with `--clone-upgradeable-program`). Covers: governance lifecycle, funded
stake→reward→claim→unstake money flow, max-supply cap enforcement at the
exact/over-cap boundaries, reward clamping at zero headroom, metadata
creation against the REAL mpl program + replay rejection.

## 8. Start PostgreSQL / Redis

Compose way: `docker compose up -d postgres redis` (see step 19). Bare-metal
way: any PostgreSQL ≥ 16 and Redis ≥ 7 reachable at `POSTGRES_URL` /
`REDIS_URL`. Verify: `pg_isready -h 127.0.0.1` and `redis-cli ping` → PONG.

## 9. Run migrations

Migrations apply AUTOMATICALLY at application startup (`auto_migrate`,
forward-only, `crates/core/migrations/0001…0011`). Verify:

```bash
psql "$POSTGRES_URL" -c 'SELECT version, description, success FROM _sqlx_migrations ORDER BY version'
```

Expected: 11 rows, versions 1–11, all `success = true` (after step 10 has
run once). A checksum mismatch fails startup loudly — never edit applied
migration files.

## 10. Start application

```bash
cp config.toml.example config.toml      # then edit as needed
export API_KEY=<YOUR_OPERATOR_KEY>
export CONFIG_PATH=$PWD/config.toml POSTGRES_URL=... REDIS_URL=... RUST_LOG=info
cargo run --bin sniper-suite            # or ./target/release/sniper-suite
```

Expected: startup log lines ending in the HTTP listener on `:8080`;
no panics; modules disabled by default (enable via API/config).

## 11. Verify health / readiness

```bash
curl -s localhost:8080/health        # {"status":"ok",...}
curl -si localhost:8080/ready | head # 200 + component list
curl -s localhost:8080/api/health    # {"ok":true}
curl -s localhost:8080/metrics | grep '^bot_' | head
```

Expected: all 200; `/ready` lists components with healthy/ready flags;
`bot_health_ready 1` in metrics. Vendor record: `evidence/phase8b-endpoints.log`.

## 12. Verify Telegram authorization

Without a bot token the Telegram module stays disabled (expected here).
Full path (HUMAN ACTION: supply a real bot token + your chat id in
config/env per `.env.template`): verify that (a) unknown chat ids are
rejected, (b) mutating commands require the owner role, (c) live-mode
commands are owner-only, (d) the bot token never appears in any error
string (regression-tested: `error_strings_never_contain_the_bot_token`).
RBAC matrix + tests: `docs/OPERATIONS.md`, module tests within step 4's 537.

## 13. Verify paper mode

```bash
curl -s localhost:8080/api/status | python3 -m json.tool | head -20
```

Expected: `"execution_mode": "paper"`, `"live_allowed": false`,
`"kill_switch": false`. Paper mode seeds demo balances (1,000 USDC / demo
SOL) — those seeds exist ONLY in paper/simulate by design and are rejected
as sizing inputs in live mode (audit-pass regression tests pin this).

## 14. Verify live-mode safety gate

`EXECUTION_MODE=live` requires an owner-role API key AND passes every
preflight: signer registry build (local signer only — vault/kms/hsm FAIL
startup by design), RPC reachability, risk limits loaded. Verify the
rejections without funding anything:

```bash
EXECUTION_MODE=live cargo run --bin sniper-suite   # with a config whose signer backend is "vault"
# expected: startup FAILURE with the typed 'unsupported signer backend' error
```

Live mode additionally refuses to switch on while `kill_switch` is set, and
Polymarket live entries require the funding check of step 15. Evidence:
`crates/solana-kit/src/signer.rs` tests + `module-polymarket` separation
tests (inside step 4).

## 15. Verify Polymarket balance / allowance (live reads, no orders)

Follow `docs/LIVE-VALIDATION.md` §1: raw `eth_call` reads (balanceOf /
decimals / allowance with the exact selectors) against YOUR Polygon RPC and
wallet, then compare with what the app reports in simulate mode. The app's
live path rejects orders on stale (>15 s), implausible-decimals, or
insufficient balance/allowance — typed errors `BalanceUnavailable` /
`InsufficientFunding`, never a paper fallback.
**FUNDED STEP = HUMAN ACTION** (stage a small dedicated amount first).

## 16. Verify Solana RPC

```bash
solana --url <YOUR_RPC> health-check
solana --url <YOUR_RPC> balance <WALLET>
EXECUTION_MODE=simulate cargo run --bin sniper-suite   # real RPC, zero broadcasts
```

Expected: simulate mode builds + simulates transactions against the real
RPC and never broadcasts; RPC failures in simulate/live surface as typed
errors (no cached-balance fallback outside paper mode).

## 17. Verify recovery

Kill the app mid-run (`kill -9`), restart it. Expected: startup recovery
reloads open positions from Postgres, re-registers unfinished orders as
`Unknown`, sweeps unresolved transactions into the reconciliation queue
(`RECOVERY_STARTUP_RECONCILE_SECS` gates module spawn). Automated proof:
the recovery/startup-reconcile tests inside step 4 (db_integration) and
`recon_crash_e2e` (gated, `docs/RECONCILIATION.md` §14).

## 18. Verify backup / restore

Follow `docs/BACKUP-RESTORE.md` §2–§3 (pg_dump -Fc → fresh DB → pg_restore
--no-owner), then:

```bash
psql <RESTORED_URL> -c 'SELECT count(*) FROM _sqlx_migrations'   # 11
curl -s localhost:8080/api/audit/verify -H "x-api-key: $API_KEY" # intact | not_chained (empty chain)
```

Vendor record (PG 17.11): dump SHA-256 `5989ecf1…`, tables/migrations/
rowcounts identical, db_integration 23/23 re-run green ON the restored DB,
app started against it and shut down cleanly — `evidence/phase8-*`.

## 19. Verify Docker

```bash
docker compose config -q
docker build -t sniper-suite:local .
docker run -d --name smoke -p 127.0.0.1:8080:8080 -e RUST_LOG=info sniper-suite:local
curl -fsS http://127.0.0.1:8080/api/health   # expected {"ok":true} — degraded-mode smoke: no DB/Redis/keys required
docker stop smoke   # graceful shutdown
```

Vendor status: **BLOCKED in the delivery sandbox (no Docker daemon)** — the
native-equivalent smoke (binary + example config + health/ready/shutdown)
PASSED; container execution is yours to run. The Dockerfile is multi-stage,
non-root, healthchecked, pinned to `rust:1.98.1-bookworm`.

## 20. Verify CI

Push to your GitHub repo → Actions runs `.github/workflows/ci.yml`
(4 jobs: app / program / security / docker). Every gate has a recorded
local-equivalent execution — `docs/CI-LOCAL-EQUIVALENCE.md` maps them 1:1
with evidence paths. Record the Actions run URL as YOUR evidence. Vendor
status: no GitHub run exists (BLOCKED — no runner); local equivalents PASS.

## 21. Verify security evidence

```bash
cargo audit                     # app lockfile   (cargo-audit 0.22.2)
cd programs/staking-suite && cargo audit
cd <REPO_ROOT> && cargo deny check
grep -rn 'unsafe {' crates programs --include='*.rs' | wc -l   # expected: 0
./scripts/release-check.sh      # includes secret-literal + TODO/stub marker scans
```

Expected: audit 0 errors (9 allow-listed warnings per `.cargo/audit.toml`),
deny ok (advisories/bans/licenses/sources), zero `unsafe {` blocks (4 crates
additionally `#![forbid(unsafe_code)]`), marker/secret scans clean, release
gate 20/0/0. Then read the security model docs: `SECURITY.md`,
`docs/SECURITY.md`, `docs/ARCHITECTURE.md` (trust boundaries),
`docs/OPERATIONS.md` (RBAC, kill switch). **No external security audit
exists — the vendor claims none, and you should commission one before
mainnet deployment of the staking program.**

## 22. Verify final release hashes

The release package contains `checksums/SHA256SUMS` covering every packaged
artifact (source snapshot, .so, evidence logs, manifests). Verify:

```bash
cd <RELEASE_PACKAGE_ROOT> && sha256sum -c checksums/SHA256SUMS
```

Expected: all `OK`. Cross-check the headline values against
`release-manifest.json` (version 0.1.0, test counts, .so hash) and
`evidence/source-inventory.json` (tree hash of the source snapshot).

---

## Result sheet (fill in as you go)

| Step | Result (PASS/FAIL/BLOCKED) | Your evidence path / note |
|---|---|---|
| 1 source hash | | |
| 2 toolchain | | |
| 3 build | | |
| 4 tests 537/537 ×2 | | |
| 5 build-sbf + .so hash | | |
| 6 identity verify | | |
| 7 validator e2e 3/3 | | |
| 8 services | | |
| 9 migrations (11) | | |
| 10 app start | | |
| 11 health/ready | | |
| 12 telegram authz | | |
| 13 paper mode | | |
| 14 live safety gate | | |
| 15 polymarket reads | | |
| 16 solana rpc/simulate | | |
| 17 recovery | | |
| 18 backup/restore | | |
| 19 docker | | |
| 20 CI run URL | | |
| 21 security evidence | | |
| 22 release hashes | | |
