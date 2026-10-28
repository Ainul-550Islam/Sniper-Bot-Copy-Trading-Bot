# GAP section 5 — Batch 04 row updates (Telegram `/buy` `/sell` + trade consent)

`GAP-MISSING-FILES.md` is owned by Batch 01 and is **not** edited here. This
file records the replacement rows that the integration step should merge once
Batch 04 lands. It states only what the repository now shows; it is not a
release claim and it does not upgrade any LIVE status.

Rows are grouped by what a due-diligence reader needs: what works end to end
today, what is a seam by design, and what has not been exercised at all.

## Replacement rows — implemented and proved by this batch

| Area | Status | Evidence |
|---|---|---|
| §5 Telegram trading — consent model + grant/revoke | DONE (Batch 04) | `crates/module-telegram/src/consent/{model,policy,store}.rs`; typed confirmation phrase (digest-only), TTL bounds, caps that can never exceed the organisation's limits |
| §5 Telegram trading — `/consent grant\|status\|revoke` | DONE (Batch 04) | `commands/consent_cmd.rs`; `revoke` is immediate and cancels every pending command of the chat |
| §5 Telegram trading — `/buy` `/sell` parsing and validation | DONE (Batch 04) | `commands/parse.rs`: base58 32-byte mint, integer-lamport amounts (exact decimal, no `f64`), percent `(0,100]`, slippage 1–500 bps; **reject, never clamp** |
| §5 Telegram trading — nonce handshake and pending state machine | DONE (Batch 04) | `commands/nonce.rs` (CSPRNG, digest-only storage, constant-time compare), `commands/pending.rs` (`pending→confirmed→executed/failed/expired/cancelled`, 60 s TTL, one pending command per chat, `idempotency_key = command_id`) |
| §5 Telegram trading — caps and rate limiting | DONE (Batch 04) | `commands/limits.rs`: `min(consent, org)` per order, rolling 24 h ledger cap, per-`(org,chat)` token bucket **plus** a durable counter |
| §5 Telegram trading — audit trail | DONE (Batch 04) | `commands/audit.rs` + `telegram_trade_audit`; one row per transition, whitelisted detail keys, nonce/token/key redaction proved by `tests/audit_redaction.rs` |
| §5 Telegram trading — migrations | DONE (Batch 04) | `0066_telegram_trade_consents.sql`, `0067_telegram_trade_commands.sql` (RLS enabled **and** forced, guarded on `app.current_org_id()`, role-guarded GRANTs, idempotent; `CHECK` constraints reject nonsense rows) |
| §5 Telegram trading — Postgres stores | DONE (Batch 04) | `crates/server/src/telegram_trade_store_pg.rs`; `tests/telegram_trade_pg.rs`, `tests/telegram_trade_migrations.rs` (real-PG gated; `REQUIRE_POSTGRES=1` panics without a URL) |
| §5 Telegram trading — tenant isolation of the new tables | DONE (Batch 04) | forced RLS + `tenant_org_isolation`; `telegram_trade_pg::org_a_cannot_read_or_update_org_b_rows` proves cross-tenant SELECT/UPDATE/INSERT are refused, and that an unset tenant context sees nothing (`REQUIRE_RLS=1` makes the RLS half mandatory) |
| §5 Telegram trading — existing commands unaffected | DONE (Batch 04) | `tests/existing_commands_unchanged.rs` pins `/status`, `/on`, `/off`, `/kill`, `/resume`, `/mode`, `/positions`, … byte-for-byte, with and without a console attached |

## Replacement rows — seams, by design

| Area | Status | Evidence |
|---|---|---|
| §5 Telegram trading — executing side (`TradeCommandPort`) | SEAM ONLY (implementation = Batch 07) | `crates/module-telegram/src/trade_port.rs`: the async trait (`place_buy`, `place_sell`, `is_halted`, `org_risk_limits`, `current_mode`) + a complete in-memory `FakeTradePort` used by every flow test. The server-side executor that talks to a wallet and a DEX does **not** exist yet |
| §5 Telegram trading — consent UI | NOT STARTED (Batch 07) | No HTTP route, no OpenAPI path and no frontend change is made by this batch; consent is granted and revoked from Telegram itself |
| §5 Telegram trading — chat binding management | PRE-EXISTING (not re-implemented) | `crates/module-telegram/src/trade_session.rs` (`ChatBinding`) and the `TradeConsole` link surface |

## Replacement rows — not exercised

| Area | Status | Evidence / why |
|---|---|---|
| §5 Telegram trading — LIVE trading evidence | NOT_RUN (correct) | No live order has ever been placed by this code path. Paper mode is the default and `live` needs two independent gates (`PortMode::Live` **and** `live_allowed`) |
| §5 Telegram trading — real Telegram Bot API session | NOT_RUN | Every test drives a local fake Bot API server; no request ever leaves the process to `api.telegram.org` |

## Notes for the merge

* The crate deliberately has **no database dependency**; the durable stores live
  in the server crate behind one `pub mod telegram_trade_store_pg;` line in
  `crates/server/src/lib.rs`. That single line is the only edit this batch makes
  outside its own tree.
* Only two `Cargo.lock` edges changed, both to crates already present (`bs58`,
  `uuid`), and both inside the `[[package]] name = "module-telegram"` stanza.
* `telegram_trade_commands.idempotency_key` is a real `UNIQUE NOT NULL` column
  and is always equal to `command_id`; the store cross-checks the pair on read
  and refuses a row where they disagree.
* Money is integer lamports (`u64`) end to end. `bigint` columns cannot hold
  every `u64`, so the store refuses an out-of-range amount instead of wrapping.
* Filing: `docs/gap-updates/batch-04-wiring.md` documents exactly what Batch 07
  must implement (trait, constructor changes, config keys, UI/OpenAPI surface).
