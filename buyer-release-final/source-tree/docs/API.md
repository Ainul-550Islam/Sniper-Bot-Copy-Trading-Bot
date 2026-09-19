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
| `/api/audit` | GET | audit trail (`?limit=`) — append-only |
| `/api/audit/verify` | GET | hash-chain verification: `intact` / `broken(at_id)` / `not_chained` |
| `/api/keys` | GET | listed key digests + roles + last-used (owner) |
| `/api/recovery/failed` | GET | recon items marked `failed` (exhausted retries) |
| `/api/db` | GET | pool stats, migration count, lag |
| `/api/wallets` | GET | registered wallets + on-chain balances (best-effort) |
| `/api/journal` | GET | JSONL journal sizes/paths or `{"available":false}` |

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
