# Control API reference

One Axum server (the `sniper-suite` binary) serves REST, a WebSocket event
feed, Prometheus metrics and the embedded dashboard. Default bind:
`127.0.0.1:8080` (`[api] bind` in config.toml).

**Fail-closed binding:** the server refuses to start with a non-loopback bind
unless API auth is configured (keys or legacy key), so an accidentally
exposed instance cannot be an open control plane.

## Authentication

Present a key with either header:

```
x-api-key: <key>
Authorization: Bearer <key>
```

Keys are declared in config as `[auth] [[auth.keys]]` entries
(`{label, key_env, role}`) — the plaintext lives in the named environment
variable and is only ever handled as a sha256 digest in memory/DB — or added
at runtime via `POST /api/keys` (owner only). Each key carries a role:

| Role | Powers |
|---|---|
| `readonly` | all GET endpoints |
| `operator` | + kill/resume, mode (non-live), module enable/disable |
| `owner` | + key management, switching mode to `live`, journal rotate |

The legacy single `api_key` config value maps to `owner`. Denials are
rate-limited per IP (bucket refills per `[api] rate_limit_rpm`, 0 disables)
and **audited** (`outcome: denied`). When no authenticator is configured and
the bind is loopback, the API is open for local development (dashboard).

## Degradation contract

Endpoints backed by optional infrastructure never 500 because the dependency
is absent — they report it: DB-backed routes answer `{"available": false}`
when Postgres is not attached; `/api/journal` answers
`{"available": false}` when no journal is configured. `/health` is pure
liveness and never degrades; `/ready` reports dependency readiness.

## REST endpoints

### Unauthenticated infrastructure
| Endpoint | Method | Purpose |
|---|---|---|
| `/` | GET | embedded dashboard (HTML) |
| `/health` | GET | liveness (`{"status":"ok"}`) |
| `/ready` | GET | readiness incl. DB/Redis/feed state |
| `/metrics` | GET | Prometheus text exposition |

### Read (any role)
| Endpoint | Method | Purpose |
|---|---|---|
| `/api/health` | GET | uptime, mode, kill-switch, module states |
| `/api/status` | GET | PnL summary, positions count, risk snapshot |
| `/api/modules` | GET | per-module enabled/running/last-activity |
| `/api/positions` | GET | open + recent closed positions |
| `/api/trades` | GET | recent fills (`?limit=`) |
| `/api/config` | GET | redacted effective config (secrets stripped) |
| `/api/orders` | GET | OMS orders; `?status=pending|submitted|filled|cancelled|failed|unknown` (invalid → 400); `?limit=` |
| `/api/orders/:id` | GET | one order + status history |
| `/api/executions` | GET | execution lifecycle ledger (every tx intent: state, attempts, signature, fee, failure class); `?limit=` (1–500, default 100); `?state=created|validated|submitted|pending|confirmed|failed|expired|reconciled` (invalid → 400); `?open=true` (non-terminal intents only). Response: `{count, states: {state: n}, executions: [...]}` |
| `/api/executions/:id` | GET | one intent by **intent id or transaction signature**. Served from the in-memory ledger; falls back to the durable `execution_lifecycle` table and then returns `{record, events}` (last 100 transitions). 404 when unknown |
| `/api/audit` | GET | audit trail (`?limit=`) — append-only |
| `/api/audit/verify` | GET | hash-chain verification: `intact` / `broken(at_id)` / `not_chained` |
| `/api/keys` | GET | listed key digests + roles + last-used (owner) |
| `/api/recovery/failed` | GET | recon items marked `failed` (exhausted retries) |
| `/api/db` | GET | pool stats, migration count, lag |
| `/api/wallets` | GET | registered wallets + on-chain balances (best-effort) |
| `/api/journal` | GET | JSONL journal sizes/paths or `{"available":false}` |
| `/api/accounting/portfolio` | GET | TASK 5 aggregated portfolio: exposure / realized / unrealized / fees / net PnL / utilization in reference units, per venue / wallet / strategy / asset / module, native per quote asset, `missing_rates` |
| `/api/accounting/events` | GET | TASK 5 ledger events (newest first, `?limit=` 1–1000), counts by kind, pending (not yet journaled) ids |
| `/api/accounting/findings` | GET | TASK 5 accounting reconciliation findings journal (`?limit=`); 503 when the journal is unavailable |
| `/api/ha` | GET | TASK 6 distributed state: this worker's identity / generation / state / readiness (with reasons) and held roles, the cluster registry with heartbeat ages and health, every lease with holder / fencing generation / expiry, durable feed cursors with lag, unresolved feed gaps, recent recovery records (`?limit=` 1–500) |
| `/api/risk/global` | GET | TASK 5 global risk: `[global_risk]` in force, process kill switch, active venue / strategy switches, portfolio totals, recent decisions with their snapshots (`?limit=` 1–256) |

### Write
| Endpoint | Method | Role | Purpose |
|---|---|---|---|
| `/api/kill` | POST | operator | engage kill switch (halts broadcasting; risk flattens per policy) |
| `/api/resume` | POST | operator | clear kill switch |
| `/api/mode` | POST | operator (paper/simulate), **owner (live)** | switch execution mode |
| `/api/modules/:name/enable` | POST | operator | start a module at runtime |
| `/api/modules/:name/disable` | POST | operator | stop a module at runtime |
| `/api/keys` | POST | owner | add key `{label, key, role}` (key ≥ 24 chars; only the sha256 digest is retained, and persisted to the `api_keys` table when the DB is attached) |
| `/api/keys/:hash` | DELETE | owner | revoke key by digest |
| `/api/journal` | POST | owner | rotate journal files |
| `/api/accounting/events` | POST | operator | TASK 5: book one operator-entered ledger event `{kind: deposit\|withdrawal\|transfer\|funding_adjustment\|fee\|correction, wallet, quote_asset, quote_amount, reference_id, …}` through the global ledger (idempotent on `reference_id`; fills / settlements refused — 400; corrections need `correlation_id`) |
| `/api/risk/kill-switch` | POST | operator | TASK 5: `{scope: "venue:<venue>" \| "strategy:<label>", engaged, reason}` — engage / release a venue or strategy kill switch (durable, audited; config-pinned scopes answer 409 `pinned_by_config`) |

Every mutating call is written to the audit chain (actor digest, action,
target, outcome, detail) — the application has no API to alter or delete
audit rows.

## WebSocket event feed

`GET /api/events` upgrades to a WebSocket streaming every `AppEvent` as one
JSON frame, tagged by `kind`:

`lifecycle`, `module_status`, `launch`, `signal`, `risk_rejected`,
`order_sent`, `fill`, `position_update`, `position_closed`, `wallet_trade`,
`polymarket`, `command`, `error`, `info`, `audit`

Serialization failures become an `error` frame rather than dropping the
socket. Slow consumers are lagged (broadcast channel) — reconnect to
resync via the REST endpoints.

## Telegram control (module-telegram)

Mirrors the API RBAC with `owner_user_ids` / `allowed_user_ids` (operator
when owners exist; full control when they don't — backward compatible) /
`readonly_user_ids` / `allowed_chat_ids`. Commands: `/status /positions
/trades /pnl /balance /config` (readonly), `/on /off /kill /resume /mode
[paper|simulate]` (operator), `/mode live` (owner). Insufficient rights get
an explicit refusal message; attempts are published as `command` events.
