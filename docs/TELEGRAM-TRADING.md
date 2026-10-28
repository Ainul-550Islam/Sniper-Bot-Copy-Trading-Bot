# Telegram trading — `/buy`, `/sell` and trade consent (BATCH 04)

This document describes the trading half of the Telegram control bot: how a
chat is authorised to trade at all, what exactly a `/buy` or `/sell` does
before any money moves, and which invariant is enforced by which piece of code.

Nothing in this document is a claim about live trading readiness. The executor
(`TradeCommandPort`) is implemented by the server; the paper mode is the
default and the live mode needs **two** independent gates to open.

---

## 1. The one-line model

```
chat → linked user → organisation → consent → validation → caps → gates
     → pending command (CSPRNG nonce, TTL 60 s) → /confirm (digest compare)
     → TradeCommandPort → executed | failed
```

Every arrow is a hard stop: the flow either continues or it is **denied and
audited**. There is no path where a rejection is silently downgraded into a
smaller order; nothing is ever clamped.

---

## 2. Commands

| Command | Who may run it | Effect |
|---|---|---|
| `/consent grant [max_order_sol] [daily_cap_sol] [ttl_hours] <phrase>` | owner | creates the consent row (needs the typed phrase) |
| `/consent status` | any authorised user of the chat | shows scope, caps, expiry, time left |
| `/consent revoke` | owner | revokes the consent **and** cancels every pending command of the chat |
| `/buy <mint> <amount_sol> [slippage_bps]` | owner / operator | opens a pending command, replies with the nonce |
| `/sell <mint> <percent\|all> [slippage_bps]` | owner / operator | opens a pending command, replies with the nonce |
| `/confirm <nonce>` | the user who requested it | executes the pending command, once |
| `/cancel <nonce>` | the user who requested it | cancels the pending command |
| `/pending` | any authorised user of the chat | lists the chat's live commands (never nonces) |

The confirmation phrase is `I CONSENT TO TELEGRAM TRADING`, matched
case-insensitively with whitespace collapsed. Only its SHA-256 digest is stored
(`telegram_trade_consents.phrase_sha256`).

---

## 3. Layer map — one owner per rule

| Concern | Sole owner |
|---|---|
| decimal parsing, mint/percent/slippage/amount validation | `commands::parse` |
| nonce generation, digest, constant-time compare | `commands::nonce` |
| pending state machine + `CommandStore` seam | `commands::pending` |
| per-order cap, rolling 24h cap, rate limiting | `commands::limits` |
| audit events, redaction, whitelisted detail | `commands::audit` |
| every user-facing string, MarkdownV2 escaping | `commands::reply` |
| consent rules (phrase, TTL, caps, active/expired/revoked, scope) | `consent::policy` |
| consent row + typed errors | `consent::model` |
| consent persistence seam | `consent::store` |
| routing, ordering of the checks, audit on every transition | `commands` (`TradeConsole`) |
| executing side (order placement, halt, limits, mode) | `trade_port::TradeCommandPort` (server, Batch 07) |

`module-telegram` has no database dependency. The Postgres implementations of
`ConsentStore`, `CommandStore` and `AuditStore` live in the server crate
(`sniper_suite::telegram_trade_store_pg`), which is also where the port will be
implemented.

---

## 4. Validation (reject, never clamp)

| Input | Rule | Rejection |
|---|---|---|
| mint | base58, decodes to exactly 32 bytes, canonical encoding | `mint_not_base58`, `mint_wrong_length`, `mint_not_canonical` |
| amount | `amount > 0`, finite, ≤ 9 decimals, no exponent/sign/underscore, fits `u64` lamports | `amount_not_numeric`, `amount_zero`, `amount_too_many_decimals`, `amount_overflow` |
| percent | `"all"` ⇒ 10000, otherwise `0 < p ≤ 100`, ≤ 2 decimals | `percent_out_of_range`, `percent_not_numeric` |
| slippage | integer basis points, `1..=500`, default 100 | `slippage_out_of_range` |
| ttl | `1..=720` hours, default 24 | `ttl_out_of_range` |

Money is integer lamports (`u64`) everywhere. No `f64` touches an amount, at any
layer.

---

## 5. Caps

```
per_order_cap = min(consent.max_order_lamports, org.max_order_lamports)
daily_cap     = min(consent.daily_cap_lamports, org.daily_cap_lamports)
```

* per-order: `amount ≤ per_order_cap` (inclusive) — buying only; a `/sell` has
  no SOL amount;
* rolling 24h: `spent_last_24h + amount ≤ daily_cap`, where `spent_last_24h` is
  the sum of this chat's **confirmed and executed** buys (money is committed the
  moment the port is called);
* rate limit: a per-`(organisation, chat)` token bucket (5 per 60 s by default)
  **plus** a durable counter of the commands the ledger recorded in the same
  window, so a restart cannot reset the budget. Both halves must pass;
* overflow (`spent + amount > u64::MAX`) is a denial, never a wrap.

A consent can never exceed the organisation's own risk limits: `draft_consent`
compares the requested caps against them and **rejects** (`cap_exceeds_org_limit`)
instead of clamping.

---

## 6. Gates, in the order the console applies them

1. **Chat → user → organisation.** An unlinked or non-allowlisted chat is denied
   and audited (`unlinked_chat`). A user who is not the chat's linked user is
   denied (`wrong_user`).
2. **Consent.** A live consent is required; the scope must cover the action.
   Expired (`consent_expired`) and revoked (`consent_revoked`) are different,
   separately audited refusals.
3. **Input validation** (§4).
4. **Caps and rate limit** (§5).
5. **Kill switch / halt.** `bot_core` state kill switch or `port.is_halted()` ⇒
   `halted`.
6. **Mode.** `paper` and `simulate` are allowed; `live` requires the port to
   report `live_allowed` as well, so two independent switches must be closed-to-
   safe/open before an order can leave. Otherwise `live_gates_closed`.
7. **Pending command.** CSPRNG nonce (16 OS-random bytes, 32 hex chars, ≥ 8
   required), TTL 60 s, bound to `(chat_id, user_id, params)`, only
   `nonce_sha256` stored, one pending command per chat.
8. **`/confirm`.** The nonce is hashed and compared against the stored digest in
   constant time, the row must still be `pending` and unexpired, the chat and
   user must match, and the `pending → confirmed` transition is a compare-and-set
   — so of N simultaneous taps exactly one executes. The port is called with
   `idempotency_key = command_id`.
9. **Duplicate confirms.** Inside the 5 s grace window a second tap is answered
   "in flight" and does **not** call the port; after it, a `confirmed` row is
   replayed with the same idempotency key (crash recovery). In both cases a
   terminal row is final.

---

## 7. Audit

One row per transition: `requested`, `denied`, `confirmed`, `executed`,
`failed`, `expired`, `cancelled`, `revoked` (plus `consent_granted`). Rows carry
only whitelisted detail keys (`reason`, `action`, `mint`, amounts, bps, `mode`,
`status`, `consent_id`, `failure_detail`) built by `AuditDetail`; free-form text
is scrubbed. **No nonce, no phrase, no token and no key material is ever stored
or logged** — a test scans the rendered audit trail and every chat reply for both
the nonce and the bot token.

Audit failures are fail-closed: when the audit write for a state-changing step
fails, the step does not complete.

---

## 8. Storage (migrations 0066 / 0067)

| Table | Purpose | Notable constraints |
|---|---|---|
| `telegram_trade_consents` | one row per grant | scope ⊂ `{buy,sell}`; `max_order > 0`; `daily ≥ order`; `expires_at > granted_at`; digest length 32; **partial unique index** ⇒ at most one non-revoked row per `(org, chat)` |
| `telegram_trade_commands` | the nonce handshake | action/status whitelists (7 states, `denied` included); buy ⇔ amount, sell ⇔ percent ≤ 10000; mint base58 32–44; slippage 1–500; digest 32 bytes; `idempotency_key` **UNIQUE NOT NULL** and always equal to `command_id`; **partial unique index** ⇒ at most one `pending` row per `(org, chat)` |
| `telegram_trade_audit` | append-only trail | event whitelist; `jsonb` detail must be an object |

Both migrations:

* enable **and force** row-level security, with `USING`/`WITH CHECK
  (organization_id = app.current_org_id())`;
* guard that block with `IF to_regprocedure('app.current_org_id()') IS NOT NULL`,
  so they also apply on a database that predates the Batch 01 RLS foundation
  (and stay inert, never half-installed, in that case);
* `GRANT` only when `bot_app` / `bot_system` actually exist;
* are idempotent: `CREATE TABLE IF NOT EXISTS`, `CREATE INDEX IF NOT EXISTS`,
  `DROP POLICY IF EXISTS` before `CREATE POLICY`.

---

## 9. Running it

```bash
# unit + integration tests (no database needed)
cargo test --locked -p module-telegram

# the two migrations on a real server, then the Postgres stores
export POSTGRES_URL=postgres://postgres:postgres@localhost:5432/sniper
REQUIRE_POSTGRES=1 cargo test --locked -p sniper-suite \
    --test telegram_trade_migrations --test telegram_trade_pg

# everything, including the database half
POSTGRES_URL=… REQUIRE_POSTGRES=1 bash scripts/telegram-trade-smoke.sh
```

With `REQUIRE_POSTGRES=1` and no `POSTGRES_URL`, the database tests **panic** —
a missing database can never be reported as a green run. `REQUIRE_RLS=1` does the
same for the tenancy half: a database without `app.current_org_id()` then fails
the cross-tenant test instead of quietly skipping it.

## 9a. Tenancy

Both tables carry `organization_id`, are `ENABLE` **and** `FORCE` row-level
security, and are policied on `app.current_org_id()`. The integration test
`org_a_cannot_read_or_update_org_b_rows` drives the same
`set_config('app.org_id', …, true)` path the server uses and asserts that:

* tenant A sees only A's consents and audit rows;
* a cross-tenant `UPDATE` affects **zero** rows (the `USING` clause filters it);
* a cross-tenant `INSERT` is refused (the `WITH CHECK` clause);
* with **no** tenant context set, a `SELECT` returns nothing — fail closed,
  never "all rows".

Because the policy is `FORCE`d, even the table owner is subject to it: a bug in
the application layer cannot cross tenants.

---

## 10. What is deliberately not here

* The real executor: `TradeCommandPort` is implemented in Batch 07. Until then
  the only implementation is `FakeTradePort` (in-memory, used by every flow
  test), and `TelegramBot` answers "trading is not configured" when no console
  is attached.
* Chat binding management (link/unlink) — owned by the existing
  `trade_session`/`TradeConsole::link` surface, not re-implemented here.
* Any change to the existing control commands. They are byte-for-byte the same
  when no `TradeConsole` is attached; `tests/existing_commands_unchanged.rs`
  pins that.
