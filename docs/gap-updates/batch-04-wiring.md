# Batch 04 wiring — how the Telegram trading console plugs in

This file exists so the integration step does not have to re-derive the wiring
from the source. It lists the seams, who owns each one, and the exact three
lines that connect the console to a running `TelegramBot`.

## 1. The seam that Batch 04 provides

```rust
// crates/module-telegram/src/trade_port.rs
#[async_trait]
pub trait TradeCommandPort: Send + Sync {
    async fn place_buy(&self, req: &OrderRequest) -> Result<ExecutionReceipt, PortError>;
    async fn place_sell(&self, req: &OrderRequest) -> Result<ExecutionReceipt, PortError>;
    async fn is_halted(&self) -> bool;
    async fn org_risk_limits(&self, organization_id: &str) -> Result<OrgRiskLimits, PortError>;
    async fn current_mode(&self) -> Result<PortMode, PortError>;
}
```

`OrderRequest` carries `command_id` / `idempotency_key` (the same UUID — a
duplicate `/confirm` must not place a second order), `organization_id`,
`chat_id`, `user_id`, `action`, `mint`, `amount_lamports` (buy), `percent_bps`
(sell) and `slippage_bps`.

`FakeTradePort` is a complete in-memory implementation (orders accepted,
`fail_with`, `set_halted`, `set_mode`, `set_org_limits`, call accounting) used by
every flow test. It is **not** a stub: it enforces the same contracts and is the
reason the whole console is testable without a chain.

## 2. The wiring, in full

```rust
use std::sync::Arc;
use module_telegram::{TradeConsole, TradeConsoleConfig, TradeStores};
use sniper_suite::telegram_trade_store_pg::PgTradeStore;

// (1) stores: Postgres in production, `TradeStores::in_memory()` in tests.
let stores: TradeStores = PgTradeStore::new(db.pool().clone()).trade_stores();

// (2) the executing side (Batch 07): halt flag, org risk limits, mode.
let port: Arc<dyn module_telegram::TradeCommandPort> = Arc::new(my_port);

// (3) the console, attached to the bot that is already running.
let console = Arc::new(TradeConsole::new(
    state.clone(),          // bot_core::state::Shared — kill switch and mode
    port,
    stores,
    TradeConsoleConfig::default(),
));
let bot = Some(TelegramBot::from_api(&cfg).with_trade_console(console));
```

`TelegramBot::trade_console()` returns the handle for chat binding
(`console.link(chat_id, ChatLink { organization_id, user_id, trading_enabled })`)
and for the operator surface (`/consent status`).

## 3. Routing rules (important for the integration)

* A `TradeConsole` is **optional**. Without one, `/consent`, `/confirm`,
  `/cancel` and `/pending` answer "not configured" and `/buy` `/sell` keep the
  pre-existing behaviour (`trading is not configured` or the legacy desk).
  `tests/existing_commands_unchanged.rs` pins this.
* With one, `/buy` `/sell` `/consent` `/confirm` `/cancel` `/pending` are routed
  to the console; every other command (`/status`, `/on`, `/off`, `/kill`,
  `/resume`, `/positions`, …) is untouched.
* Role requirements are unchanged for existing commands and are
  `owner`/`operator` for trading, `owner` for `/consent grant|revoke`.

## 4. Ordering inside the console (never reorder)

`link → consent → validate → caps → rate → kill/halt → mode → pending → confirm
→ port`. Each step denies and audits; nothing is clamped. See
`docs/TELEGRAM-TRADING.md` §6.

## 5. Files this batch owns

```
crates/core/migrations/0066_telegram_trade_consents.sql
crates/core/migrations/0067_telegram_trade_commands.sql
crates/module-telegram/Cargo.toml                       (+ bs58, uuid — both already in the lock)
crates/module-telegram/src/{lib.rs, commands.rs, trade_port.rs}
crates/module-telegram/src/consent/{mod,model,policy,store}.rs
crates/module-telegram/src/commands/{parse,nonce,pending,limits,audit,reply,
                                     buy,sell,consent_cmd,confirm}.rs
crates/module-telegram/tests/{support/mod.rs, parse_and_validation.rs,
                              consent_policy.rs, nonce_and_pending.rs,
                              limits_and_rate.rs, buy_sell_flow.rs,
                              audit_redaction.rs,
                              existing_commands_unchanged.rs}
crates/server/src/telegram_trade_store_pg.rs
crates/server/src/lib.rs                                (one `pub mod` line)
crates/server/tests/{telegram_trade_pg.rs, telegram_trade_migrations.rs}
scripts/{check-batch04-ownership.sh, telegram-trade-smoke.sh}
.github/workflows/telegram-trade.yml
docs/TELEGRAM-TRADING.md
docs/gap-updates/{batch-04-rows.md, batch-04-wiring.md}
```

`commands.rs` stays the router (it is the pre-existing file); the new submodules
live beside it in `src/commands/`. That is a Rust module layout change, not a
behavioural one.
