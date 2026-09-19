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
(521 tests at freeze / 537 in the audit pass, clippy `-D warnings`,
audit/deny) was produced with 1.98.1.

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
/ 0 SKIP** (521/521 workspace at freeze — 537/537 on the current audit-pass
tree — incl. the 38 gated integration tests, staking 48/48 host at freeze —
71/71 current, fmt/clippy/audit/deny clean). If anything fails on your machine,
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
