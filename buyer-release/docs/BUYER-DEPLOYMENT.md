# Buyer deployment handover

The actual deployment procedure for sniper-suite 0.1.0, in the order it must
be performed. It combines `docs/DEPLOYMENT.md` (reference) and
`docs/HANDOVER.md` §2 (verify-from-zero) into a single buyer-facing sequence
with explicit safety gates. **The sequence ends in paper mode.** Moving to
live trading is a separate, deliberate decision gated at step 15 — do not
shortcut it.

Requirements: Linux x86-64, ~2 GB RAM, ~20 GB disk for a full verification
build (a release-only build needs less), network access.

## 1. Obtain the source

Receive the repository (git bundle/archive) from the seller. The delivered
tree is the frozen engineering state: release commit `9c677cd`, freeze commit
`0e139c3`, 146 tracked files (+ the 14 buyer-package documents added after
the freeze).

## 2. Verify the commit / tree integrity

```bash
git log --oneline -3          # expect 0e139c3 (freeze) on 9c677cd (release)
git status --short            # expect empty (clean tree)
git ls-files | wc -l          # expect 146 (+14 once buyer docs are committed)
```

Cross-check the version identity: `VERSION` = `0.1.0`,
`release-manifest.json` `version` = `0.1.0`, root `Cargo.toml`
`[workspace.package].version` = `0.1.0`. `scripts/release-check.sh` gates
this three-way consistency.

## 3. Install the pinned toolchain

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal
source ~/.cargo/bin/env
rustc --version               # rust-toolchain.toml forces 1.98.1 automatically
```

Do not substitute a different Rust version: 1.98.1 is the version the whole
verification record (521 tests at freeze / 537 in the audit pass, clippy
`-D warnings`, audit/deny) was produced with. For the staking program build you additionally need the Solana
CLI (`cargo build-sbf`, agave 2.1.21 generation) — only for step 15's
on-chain deployment, not for the bot.

## 4. Configure PostgreSQL

- PostgreSQL **≥ 16** (verified on 16.4; compose uses `postgres:16-alpine`).
- Create a dedicated database and role. Export:
  `POSTGRES_URL=postgres://user:pass@host:5432/db`.
- Migrations `0001`–`0011` are embedded in the binary and applied at startup
  when `auto_migrate` is on (`[storage]` in `config.toml.example`). They are
  forward-only by design — there are no down migrations
  (`docs/BACKUP-RESTORE.md`).
- Alternatively use the delivered compose stack (step 10) which provisions
  PG + Redis healthcheck-gated.

## 5. Configure Redis

- Redis **7.x** (verified on 7.2.10; compose uses `redis:7-alpine`).
- Export `REDIS_URL=redis://host:6379`.
- Redis is **non-authoritative** (dedup L2, coordination, cache). It may die
  without losing money-relevant truth, but configure persistence/monitoring
  anyway to avoid needless claim-coordination churn (`docs/BACKUP-RESTORE.md`).

## 6. Configure RPC / WS (and optionally Geyser)

- `RPC_URL` / `WS_URL` (or `[network]` in `config.toml`): a Solana JSON-RPC
  + WebSocket provider you contract for. The retry/failover chokepoint
  tolerates flaky providers; `BROADCAST_FANOUT=true` races sends across
  primary + fallback RPCs when you have more than one.
- Optional `GEYSER_WS_URL`: a Yellowstone-compatible `transactionSubscribe`
  endpoint for push-based launch/trade detection. Without it, the modules
  fall back to PumpPortal WS and polling — functionality is preserved,
  latency is not.

## 7. Configure secrets (outside the repository)

Secrets come from the environment (or your secret store injecting env vars);
config files store only the **names** of env vars (`*_env` fields). Never
commit secrets; the release-check secret-scan gate would fail the build.

| Secret | Used by |
|---|---|
| `SOLANA_KEYPAIR` (path / base58 / JSON array) | Solana execution (live/simulate only) |
| `POLYMARKET_PRIVATE_KEY` (or `POLYGON_PRIVATE_KEY`) | Polymarket CLOB order signing |
| `TELEGRAM_BOT_TOKEN` | Module 5 |
| `API_KEY` | Mutating REST routes |
| `POSTGRES_PASSWORD` etc. | compose stack (`.env` from `.env.template`) |

`.env` is loaded via `dotenvy` and is gitignored; in production prefer real
secret management over `.env` files.

## 8. Configure authentication / RBAC

- Set `API_KEY`; the server **refuses to bind a non-loopback address without
  API auth** — this is a hard startup gate, not a warning.
- Telegram: fill `[telegram]` `owner_user_ids` (full control incl.
  `/mode live`), `allowed_user_ids`/`allowed_chat_ids` (operators),
  `readonly_user_ids`. Authorization is deny-by-default: with empty
  allow-lists, no commands are accepted.
- Review the RBAC matrix in `docs/API.md` before exposing anything.

## 9. Run migrations + the verification gate

```bash
export POSTGRES_URL=... REDIS_URL=...
./scripts/release-check.sh    # 20 gates; last known result 20/0/0 on the frozen tree
```

This applies migrations (via the test suites), runs fmt/check/clippy
`-D warnings`, all 537 workspace tests incl. the 38 gated integration tests
against your real PG/Redis, staking host tests, cargo-audit ×2 and
cargo-deny. **If this does not pass on your machine, stop and resolve before
deploying** — it is the same gate the delivery evidence was produced with.

## 10. Start in paper mode

```bash
cp config.toml.example config.toml && $EDITOR config.toml
# [execution] mode = "paper"  (the default; Config::default disables all modules)
cargo build --release
CONFIG_PATH=./config.toml ./target/release/sniper-suite
# or the full stack:  cp .env.template .env && $EDITOR .env && docker compose up --build -d
```

Paper mode simulates fills against live market data with seeded balances
(10 SOL / 1000 USDC). Nothing is sent on-chain or to Polymarket. Enable
modules via `config.toml`, the API, or Telegram.

> Docker caveat: the image build + container smoke test were **NOT EXECUTED**
> in the delivery sandbox (no daemon) — they are wired into the CI `docker`
> job. Run `docker compose up --build` yourself and treat the first successful
> build + `/health` response as your verification of that path.

## 11. Verify health / readiness

```bash
curl -s localhost:8080/health | jq   # liveness: always 200 while HTTP is served
curl -s localhost:8080/ready  | jq   # readiness: 200 only when every component is ready, else 503 + report
```

`/ready` reports per-component (`rpc`, `sniper`, `copy`, `polymarket`)
health with detail strings that never contain error payloads, URLs or key
material. A degraded dependency must fail readiness, not liveness (so an
outage never gets the process restart-looped).

## 12. Verify metrics

```bash
curl -s localhost:8080/metrics | head -40
```

Expect `bot_*` series (build info, mode, kill switch, module counters, RPC
retry/failover counters, execution latency histograms, HTTP request
metrics). Names/labels are enumerated in README §Observability and must match
`docs/OPERATIONS.md`. Point Prometheus at the endpoint (scrape example in
README).

## 13. Verify Telegram

With `TELEGRAM_BOT_TOKEN` set and your chat/user IDs allow-listed: `/status`,
`/positions`, `/pnl` (readonly), then `/on`/`/off`/`/kill`/`/resume`
(operator/owner). An unauthorized request must produce an explicit refusal —
never a silent no-op. Confirm error messages/alerts never contain the bot
token (guaranteed by the redaction regression test, but verify live output
once).

## 14. Verify recovery

Prove the crash path before trusting it with money:

1. Start in paper mode, let it record intents/positions (journal + PG).
2. `kill -9` the process; restart it.
3. Check startup reconciliation ran (logs), no duplicate executions occurred
   (dedup + claims), `/api/audit/verify` still reports a valid chain, and
   `/ready` returns to 200.

Procedures and expected behavior: `docs/RECONCILIATION.md`,
`docs/BACKUP-RESTORE.md` (incl. the pg_dump → restore drill you should also
perform once on your own infrastructure).

## 15. Only then consider live mode — safety gates

Live trading requires **all** of the following, deliberately:

1. Everything above passed on your infrastructure, in paper (and optionally
   `simulate` — which builds and RPC-simulates real transactions without
   sending).
2. `[execution] mode = "live"` **and** `allow_live_trading = true` (two
   independent gates; with the second off, live requests are downgraded and
   never broadcast). At runtime, `/mode live` (REST or Telegram) is
   owner-only.
3. Real key material loaded through the signer registry; risk limits sized
   for the capital you are actually willing to lose; daily-loss auto-disable
   configured.
4. Funded live-trading validation performed **gradually, under operator
   supervision** — this was never executed in the delivery environment
   (documented NOT EXECUTED item) and is your responsibility.
5. For the staking program specifically: program deployed under a finalized
   id **and an independent external security audit passed** — until then,
   mainnet deployment is blocked by documentation and should be blocked by
   your process (`docs/STAKING.md`, `docs/SECURITY.md`).

This document does not encourage live deployment; it documents the gates that
make an unsafe live deployment require multiple deliberate acts.
