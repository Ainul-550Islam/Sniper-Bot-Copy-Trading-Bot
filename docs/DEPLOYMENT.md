# Deployment guide

> **Production deployments: read these first.** This guide covers the
> single-host development/evaluation path. The hardened production path
> is documented separately and is gated in CI (`deploy-config` job):
>
> | Topic | Document | Gate |
> |---|---|---|
> | TLS termination, reverse proxy, certificate renewal | [`TLS-REVERSE-PROXY.md`](TLS-REVERSE-PROXY.md) | `./scripts/verify-tls-config.sh` |
> | Staging vs production isolation, gated migrations | [`ENVIRONMENT-SEPARATION.md`](ENVIRONMENT-SEPARATION.md) | `./scripts/verify-environment-separation.sh` |
> | Digest-pinned deploy, rollback, deployment ledger | [`IMAGE-DIGEST-DEPLOY.md`](IMAGE-DIGEST-DEPLOY.md) | `./scripts/verify-image-digests.sh` |
>
> Configuration lives in [`deploy/`](../deploy/README.md).

## Multi-replica

Running more than one replica is safe **only** from migration
`0036_durable_tenant_controls_and_rotations.sql` onward, with a
0036-or-later application build. Before 0036 the tenant kill-switch,
custody rotation state and the WebSocket replay set were process-local:
a second replica ignored tenant pauses, returned 404 for rotations
created elsewhere, and admitted replayed WebSocket tickets.

Prove it on your own infrastructure before scaling out:

```bash
POSTGRES_URL=postgres://user:pass@host/db \
  cargo test -p sniper-suite --test multi_replica_durable_state -- --nocapture
```

Production must also set `DATABASE_AUTO_MIGRATE=false` and let
`scripts/deploy-release.sh` apply the schema once, before new replicas
start — see `ENVIRONMENT-SEPARATION.md`.

## Prerequisites

* Docker + docker compose (recommended), **or** Rust stable (pinned in
  `rust-toolchain.toml`) for a bare-metal build.
* PostgreSQL 14+ and Redis 7 (compose provides both). The bot runs without
  them in degraded mode (JSONL journal only, in-process dedup) — fine for
  paper trading, not recommended for live.
* A funded Solana keypair JSON only for live/simulate trading.

## 1. Configure

```bash
cp config.toml.example config.toml   # edit: RPC URLs, module flags, risk caps
cp .env.template .env                # edit: POSTGRES_PASSWORD, keypair path
```

Key decisions in `config.toml`:

* `[execution] mode` — keep `"paper"` until you have watched the system in
  paper/simulate for a meaningful period. Live requires additionally
  `allow_live_trading = true`.
* `[api] bind` — leave `127.0.0.1:8080` and front it with a TLS reverse
  proxy (caddy/nginx/traefik) if remote access is needed. A non-loopback
  bind without API auth **refuses to start** (fail-closed).
* `[auth] [[auth.keys]]` — declare API principals as
  `{ label, key_env, role }` where `role` is `owner` / `operator` /
  `readonly` and `key_env` names the environment variable holding the
  plaintext key (the app only ever keeps its sha256 digest). The legacy
  `[api] api_key_env` single key still works and maps to `owner`.
* `[database] enabled = true`, `[redis] enabled = true` for the full
  durability stack (URLs come from `POSTGRES_URL` / `REDIS_URL` env, which
  compose sets automatically).

The loader rejects unknown keys — typos fail fast at startup instead of
silently trading on defaults.

## 2. Run with compose

```bash
docker compose up --build -d
docker compose logs -f bot
curl -s http://127.0.0.1:8080/ready | jq
```

* Postgres data, Redis AOF and the JSONL journal live in named volumes —
  `docker compose down` (without `-v`) preserves all state.
* The bot starts only after both datastores are healthy.
* The control API is published on `127.0.0.1:8080` by default
  (`API_BIND_HOST` / `API_PORT` in `.env`).

## 3. Bare metal (alternative)

```bash
cargo build --release -p sniper-suite
POSTGRES_URL=postgres://... REDIS_URL=redis://... \
SOLANA_KEYPAIR=/secure/path/id.json \
./target/release/sniper-suite            # reads ./config.toml or $CONFIG_PATH
```

Run under systemd/supervisor with `Restart=on-failure`. Startup is
crash-safe: recovery reloads positions/orders and sweeps unresolved
transactions before modules spawn.

## 4. Staking program (optional, separate lifecycle)

See docs/STAKING.md — build with `cargo build-sbf` (agave 2.1.21 toolchain),
deploy with `solana program deploy`, initialize once, then perform the
one-time `GenesisMint`. The bot process does not depend on the program.

## Production checklist

- [ ] `config.toml`: paper → simulate → live progression actually exercised
- [ ] API auth keys configured via `[auth]` key_env principals (or runtime
      `POST /api/keys`), legacy single `API_KEY` removed or intentionally kept
- [ ] Bind is loopback + TLS proxy, or (if directly exposed) auth + firewall
- [ ] `POSTGRES_PASSWORD` strong; DB not published to the internet
- [ ] Risk caps sized to your bankroll (`max_position_quote` /
      `max_position_fraction`, `daily_loss_limit_quote`,
      `max_consecutive_failures`) — defaults are conservative placeholders
- [ ] Wallet keypair file mode 0400, owned by the service user
- [ ] Backups: pg_dump of the Postgres volume + the journal files
- [ ] Monitoring scraping `/metrics`; alerting on `/ready` failures,
      `bot_kill_switch == 1` (kill switch / daily-loss halt), and recon
      `failed` items (`/api/recovery/failed`)
- [ ] `TELEGRAM_BOT_TOKEN` + allowlists set if remote control is wanted
- [ ] Time sync (NTP) — audit chain + recon timestamps assume sane clocks

## Upgrades

1. `docker compose pull` / rebuild the image.
2. Migrations run automatically at startup (`auto_migrate`), additive only.
3. Rolling restart is safe: shutdown drains HTTP → modules → pumps → DB in
   bounded phases, and recovery reconciles anything in flight on boot.
4. The staking program upgrades separately (`solana program deploy`
   --upgradeable) — config accounts persist across upgrades; note that the
   `Config` borsh layout must match the deployed program version.
