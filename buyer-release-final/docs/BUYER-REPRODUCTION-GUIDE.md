# Buyer reproduction guide — clean machine to running system

Goal: starting from a CLEAN machine with nothing but internet access,
reproduce the delivery: verify source → install toolchain → build → test →
database → run → verify safety → shut down. Every command is exact; every
step states what you should observe. Nothing assumes prior knowledge of
this codebase. Companion: `docs/BUYER-ACCEPTANCE-TEST.md` (formal
acceptance record with PASS/FAIL fields).

Estimated time: 60–120 minutes (mostly compiling), ≥ 8 GB RAM, ≥ 30 GB
disk, Linux x86_64 (WSL2 works; macOS works except Docker/validator notes).

## 0. Required network access (nothing else)

| Destination | Why |
|---|---|
| `static.rust-lang.org` | rustup toolchain install |
| `crates.io` + `static.crates.io` | dependency download (lockfile-pinned) |
| `github.com/anza-xyz/agave/releases` | Solana CLI v2.1.21 tarball |
| `release.anza.xyz` / GitHub (platform-tools) | `cargo build-sbf` toolchain (auto-fetched, ~1.2 GB) |
| `api.mainnet-beta.solana.com` | validator e2e clones the real mpl-token-metadata program |
| `api.devnet.solana.com` | optional latency benchmark legs |
| Your Polygon RPC / Polymarket APIs / Telegram API | only for live-mode operation later (not for reproduction) |

## 1. Get the source and verify its hash

```bash
mkdir -p ~/sniper && cd ~/sniper
# unpack the delivered archive (or clone your repository copy):
tar xzf sniper-suite-FINAL-src.tar.gz        # produces sniper-suite/
cd sniper-suite
find . -type f -not -path './target/*' -not -path './programs/staking-suite/target/*' \
  | sort | xargs sha256sum | sha256sum
```

Expected: the tree hash equals the value in `BUYER-FINAL-RELEASE-MANIFEST.json`
(`source_tree_sha256`) and `provenance/SOURCE-PROVENANCE.json`. Per-file
hashes: `sha256sum -c` against the package `checksums/SHA256SUMS` (files
under `source-tree/`) or the provenance file list. Any mismatch → stop;
the source is not the delivered source.

## 2. Install the pinned toolchain

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal
source "$HOME/.cargo/env"
rustup toolchain install 1.98.1 --profile minimal -c rustfmt -c clippy
rustup default 1.98.1
cargo --version && rustc --version
```

Expected: `cargo 1.98.1 (797e8a9bc 2026-08-05)`, `rustc 1.98.1 (48a229cea
2026-09-01)`. The repo's `rust-toolchain.toml` enforces 1.98.1 for every
cargo invocation inside the tree, so a wrong global default cannot silently
build with another version.

Solana CLI (needed for steps 7 and the staking build):

```bash
curl -sSL -o /tmp/agave.tar.bz2 \
  https://github.com/anza-xyz/agave/releases/download/v2.1.21/solana-release-x86_64-unknown-linux-gnu.tar.bz2
echo "5da3359e296f1e6c13522b874317cb4cfe33e73301a9dc60b1e1435cc133b397  /tmp/agave.tar.bz2" | sha256sum -c -
mkdir -p ~/agave-2.1.21 && tar xjf /tmp/agave.tar.bz2 -C ~/agave-2.1.21 --strip-components=1
export PATH="$HOME/agave-2.1.21/bin:$PATH"
solana --version
```

Expected: hash check `OK`; `solana-cli 2.1.21 (src:8a085eeb; feat:…)`.

## 3. System dependencies (services)

Bare metal (Debian/Ubuntu example):

```bash
sudo apt-get update && sudo apt-get install -y postgresql redis-server pkg-config libudev-dev clang libprotobuf-dev protobuf-compiler
sudo service postgresql start && sudo service redis-server start
sudo -u postgres psql -c "ALTER USER postgres PASSWORD 'postgres';"   # or use trust auth locally
```

Or Docker (equivalent):

```bash
docker compose up -d postgres redis
```

Verify:

```bash
pg_isready -h 127.0.0.1        # accepting connections
redis-cli ping                  # PONG
```

(`libudev-dev/clang/protobuf` are only needed for the Solana toolchain; the
app workspace itself needs just `pkg-config` + a C linker.)

## 4. Build

```bash
cd ~/sniper/sniper-suite
cargo check --workspace --all-targets
cargo build --workspace --all-targets
```

Expected: both exit 0. First build downloads the pinned dependency graph
(706 packages) and takes 15–40 minutes on modest hardware. At most 6
deprecation `allow` attributes are present (documented in
`docs/HANDOVER.md`); no errors, no other warnings with `-D warnings` in
step 5's clippy.

## 5. Quality gates (same commands CI runs)

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
```

Expected: both exit 0 with no output (fmt) / no warnings (clippy).

## 6. Full test suite against real services

```bash
export POSTGRES_URL=postgres://postgres:postgres@127.0.0.1:5432/postgres
export REDIS_URL=redis://127.0.0.1:6379
cargo test --workspace -- --test-threads=1
```

Expected: **537 passed; 0 failed** summed across all suite lines. The gated
suites (db_integration 23, redis_integration 10, distributed_integration 4,
two_replica_mirror 1) EXECUTE because the env vars are set; without them
they print skip notices and totals drop — skips are always loud, never
silent. `--test-threads=1` is REQUIRED (db suites share one database).

Migrations: applied automatically by the test suite/app startup (sqlx,
forward-only). Verify:

```bash
psql "$POSTGRES_URL" -c 'SELECT version, success FROM _sqlx_migrations ORDER BY version'
# 11 rows, versions 1..11, success = true
```

## 7. Staking program: host tests, SBF build, validator e2e

```bash
cd programs/staking-suite
cargo test                       # host: 71 passed; 0 failed (3 e2e compile, gated)
cargo fmt --check && cargo clippy --all-targets -- -D warnings
cargo build-sbf                  # first run auto-downloads platform-tools (~1.2 GB)
sha256sum target/deploy/staking_suite.so
```

Expected `.so` SHA-256 on Linux x86_64 with platform-tools v1.43:
`57a890fae273f2c569fc814c43f0645311b6983dd30782126a9844ee193b5564`
(187,504 bytes). Vendor verified byte-identical rebuilds (incremental AND
cold). A different OS/toolchain build may differ — then rely on the e2e
below, not the hash.

```bash
STAKING_E2E=1 cargo test --test validator_e2e -- --test-threads=1
```

Expected: **3 passed; 0 failed** (~160 s). Starts a real
`solana-test-validator` (Agave BPF VM) and clones the REAL
mpl-token-metadata program from mainnet-beta — requires the network access
from step 0. Covers governance, funded stake→reward→claim→unstake, supply
cap boundaries, metadata creation + replay rejection, authority checks.

## 8. Run the application (paper mode — safe default)

```bash
cd ~/sniper/sniper-suite
cp config.toml.example config.toml        # defaults are paper-safe
cp .env.template .env                     # fill ONLY what you need for paper mode
export API_KEY=$(openssl rand -hex 32)    # your operator API key
export CONFIG_PATH=$PWD/config.toml RUST_LOG=info
cargo run --bin sniper-suite
```

Expected startup: migrations already applied → module init → HTTP listener
on `:8080`; modules stay disabled until enabled via API/config; no panics.

## 9. Verify health, readiness, paper mode, metrics

```bash
curl -s localhost:8080/health          # {"status":"ok",...}
curl -si localhost:8080/ready | head -1 # HTTP/1.1 200
curl -s localhost:8080/api/health       # {"ok":true}
curl -s localhost:8080/api/status | python3 -m json.tool | head
curl -s localhost:8080/metrics | grep '^bot_' | head
```

Expected in `/api/status`: `"execution_mode": "paper"`, `"live_allowed":
false`, `"kill_switch": false`. Paper mode seeds demo balances (1,000 USDC /
demo SOL) — by design they exist only in paper/simulate and are rejected as
live sizing inputs (regression-tested).

## 10. Verify live-mode safety gates (without funding anything)

```bash
# a) live mode still refuses to broadcast without allow_live_trading:
grep -n 'allow_live_trading' config.toml.example   # default false
# b) unsupported signer backends fail startup (vault/kms/hsm are local-only by design):
#    set [solana].signer_backend = "vault" in a scratch config copy, start, expect
#    a typed 'unsupported signer backend' startup FAILURE:
sed 's/^signer_backend.*/signer_backend = "vault"/' config.toml.example > /tmp/bad.toml
CONFIG_PATH=/tmp/bad.toml cargo run --bin sniper-suite   # expected: startup error, non-zero exit
```

Expected: the app FAILS to start with the typed signer error — proof that
live mode cannot silently fall back to an unheld key backend. Restore the
real config afterwards.

## 11. Telegram control (optional at reproduction time)

Without a bot token the Telegram module stays disabled (expected). For the
full path: create a bot via @BotFather, put the token + your chat id in
`.env` per `.env.template`, restart, and verify: unknown chat ids rejected;
mutating commands owner-only; live-mode commands owner-only; the token never
appears in any error string (all four behaviors are regression-tested inside
step 6's 537).

## 12. Recovery check

```bash
# with the app running from step 8:
kill -9 $(pgrep -f 'target/debug/sniper-suite' | head -1)
cargo run --bin sniper-suite    # restart
```

Expected: startup recovery reloads open positions from Postgres,
re-registers unfinished orders as `Unknown`, and module spawn waits for the
startup reconcile pass (`RECOVERY_STARTUP_RECONCILE_SECS`). Logs show the
recovery sequence; no data loss (state lives in PG + journal).

## 13. Clean shutdown

```bash
kill -TERM $(pgrep -f 'target/debug/sniper-suite' | head -1)
```

Expected ordered drain in the logs: runtime-flag/journal-pump stop →
http-drain → module-drain → pump-flush → final line
`sniper-suite stopped cleanly`, exit 0.

## 14. One-shot full gate (optional, reproduces the vendor's final gate)

```bash
export POSTGRES_URL=... REDIS_URL=...     # as in step 6
./scripts/release-check.sh
```

Expected: `release-check summary: 20 PASS, 0 FAIL, 0 SKIP` (requires the
services + toolchain above; ~30–60 min from warm caches). And for bundle
integrity without any toolchain: `./scripts/verify-delivery.sh` → 7 PASS /
0 FAIL.

## Troubleshooting

| Symptom | Cause / fix |
|---|---|
| db suites skipped (totals < 537) | `POSTGRES_URL`/`REDIS_URL` not exported into the cargo shell |
| `solana-test-validator` not found | step 2 PATH not exported; e2e requires the Agave 2.1.21 CLI |
| e2e metadata test fails to clone mpl | no internet to `api.mainnet-beta.solana.com`; retry or use `--clone-upgradeable-program` reachable RPC |
| build-sbf hash differs | different OS/platform-tools version — expected; verify via e2e instead |
| link failures on Linux | missing `pkg-config`/`libudev-dev`/`clang` (step 3) |
| clippy/fmt not found | `rustup toolchain install 1.98.1 -c rustfmt -c clippy` (step 2) |
