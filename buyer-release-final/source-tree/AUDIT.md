# sniper-suite — Commercial Software Due-Diligence Audit

**Auditor posture:** senior Rust / blockchain / quant-trading architect + security reviewer.
**Method:** direct inspection of the uploaded source on disk, plus real build/test execution.
**Date of execution:** toolchain rustc/cargo **1.98.1**, edition 2021, `rust-version = 1.82`.

> Evidence markers used throughout:
> **MISSING** = does not exist. **NOT VERIFIED** = cannot be confirmed without external/live resources. **NOT EXECUTED** = could not be run in this environment.

---

## PHASE 1 — PROJECT INVENTORY

### Totals (verified via `find`/`wc`)
| Metric | Value |
|---|---|
| Source files (excl. `target/`, `.git/`) | **69** |
| Rust files (`.rs`) | **54** |
| Rust lines of code | **27,119** |
| Rust + TOML + MD lines | 27,748 |
| `Cargo.toml` manifests | 9 (1 workspace + 7 crates + 1 program) |
| Lockfiles | 2 (`Cargo.lock` workspace + program) |
| Languages | **Rust** (all logic); embedded **HTML/CSS/JS** (single-file dashboard inside `dashboard.rs`); **TOML** (config); **Dockerfile** |
| Workspace member crates | 7 |
| On-chain programs | 1 (`programs/staking-suite`, native, `cdylib`+`lib`) |
| Binaries | 1 (`sniper-suite`) |
| Libraries | 7 |
| Tests | **279** unit tests (262 workspace + 17 program), all in `#[cfg(test)]` modules |
| Integration tests (`tests/`) | **MISSING** |
| CI / `.github/workflows` | **MISSING** |
| Scripts (sh/py/ts) | **MISSING** |
| Database / migrations | **MISSING** (persistence = JSONL append files) |
| Message queue / Redis / Postgres | **MISSING** |
| Docker / deploy | `Dockerfile` + `.dockerignore` present; no compose/k8s/IaC |
| Documentation | `README.md` (10 KB) + extensive `///` doc comments (a `missing_docs` lint is active) |

### Crate / module inventory

**`bot-core` (`crates/core`, 16 tests)** — shared kernel.
- Purpose: config loading, shared state, event bus, risk engine, models, math, storage.
- Key files: `config.rs` (1231 L), `risk.rs` (832 L), `state.rs` (793 L), `models.rs` (656 L), `maths.rs` (456 L), `events.rs` (339 L), `storage.rs` (186 L), `error.rs` (137 L).
- Important types: `Config` (+ per-section structs, all `deny_unknown_fields`), `AppState`/`Shared = Arc<AppState>`, `RiskEngine`, `RiskDecision`/`ExitDecision`, `Position`, `Trade`, `AppEvent`, `EventBus`, `ExecutionMode`, `BotModule`, `Storage`.
- External APIs: none directly (config + state). Blockchain: none directly. DB: JSONL files.
- Auth/security: secrets are **env-only** with a `redacted()` masker (`config.rs:652`); `validate()` (`config.rs:1131`) downgrades live→simulate when the gate is closed and warns on missing keys.

**`solana-kit` (`crates/solana-kit`, 164 tests)** — Solana plumbing (largest, most mature crate).
- Purpose: RPC client, transaction build/sign/execute, pump.fun/PumpSwap/Raydium/Jupiter instruction builders, WebSocket client, transaction decoding, PumpPortal feed.
- Key files: `pumpswap.rs` (1520 L), `pump.rs` (1332 L), `events.rs` (1223 L), `raydium.rs` (1210 L), `decode.rs` (1063 L), `pumpportal.rs` (1002 L), `execute.rs` (951 L), `rpc.rs` (920 L), `jupiter.rs` (876 L), `tx.rs` (624 L), `consts.rs` (584 L), `ws.rs` (1098 L), `tokens.rs` (397 L), `layout.rs` (361 L).
- Important types/traits: `Rpc`, `Executor`/`ExecPolicy`/`ExecStatus`/`BroadcastMode`, `TxBuilder`/`TxRequest`/`BuiltTx`, `Wallet`, `WsClient`, `PumpContext`, `LayoutStore`, `DecodedSwap`.
- Blockchain integrations: pump.fun `6EF8rrecth…` (+ global `4wTV1…`), PumpSwap `pAMMBay6…`, Raydium v4 `675kPX9…`, Jupiter REST, WSOL/SPL/ATA/system/compute-budget — all in `consts.rs`, discriminators IDL-verified by tests.
- Auth/security: `Wallet` loads keypairs (path/base58/JSON), **never logs the secret** (only pubkey+source, `tokens.rs:121`).

**`module-sniper` (`crates/module-sniper`, 14 tests)** — Module 1.
- Purpose: detect new pump.fun launches; buy on-curve / via Jupiter; manage exits.
- Files: `entry.rs` (527 L), `detect.rs` (433 L), `exit.rs` (397 L), `lib.rs` (374 L).
- Entry point: `Sniper::new(...).run()` (consumes self, spawns detector + sweeper).
- Detection: `LaunchDetector::spawn` merges **PumpPortal `subscribeNewToken`** + **Solana `logsSubscribe`** into one `mpsc<TokenLaunch>` (`detect.rs:44`).

**`module-copy` (`crates/module-copy`, 7 tests)** — Module 2.
- Purpose: mirror tracked wallets' buys (and optionally exits).
- Files: `mirror.rs` (616 L), `feeds.rs` (453 L), `exit.rs` (396 L), `lib.rs` (158 L).
- Feeds: `pumpportal` (`subscribeAccountTrade`) or `logs_poll`; `transaction_subscribe` **falls back to polling** (`feeds.rs:96`).

**`module-polymarket` (`crates/module-polymarket`, 44 tests)** — Module 3.
- Purpose: Gamma discovery + CLOB v2 trading + EIP-712 order signing.
- Files: `lib.rs` (588 L), `clob.rs` (487 L), `eip712.rs` (438 L), `orders.rs` (322 L), `gamma.rs` (296 L), `auth.rs` (215 L), `ws.rs` (180 L), `strategy.rs` (297 L), `error.rs` (106 L).
- Auth: L1 `ClobAuth` EIP-712 → derive L2; L2 HMAC-SHA256 (`POLY_*` headers).

**`module-telegram` (`crates/module-telegram`, 16 tests)** — Module 5.
- Purpose: long-poll control bot + alerts.
- Files: `commands.rs` (421 L), `api.rs` (336 L), `lib.rs` (203 L), `alerts.rs` (173 L).
- Auth: `is_authorized` deny-by-default (`commands.rs:40`), enforced in the run loop (`lib.rs:121–141`).

**`sniper-suite` (`crates/server`, 1 test)** — control plane binary.
- Files: `main.rs` (264 L), `api.rs` (250 L), `dashboard.rs` (236 L), `ws.rs` (31 L).
- Axum REST (13 routes) + WebSocket event feed + embedded HTML dashboard + module supervisor.

**`staking-suite` (`programs/staking-suite`, 17 tests)** — Module 4, native Solana program.
- Files: `processor.rs` (460 L), `state.rs` (237 L), `instruction.rs` (227 L), `error.rs` (72 L), `lib.rs` (49 L).
- `declare_id!("3vEEMMFmdA88n8ApgZ3b9L3BXEh75yCeMbHbmUjR9mfy")` — **placeholder program id** (`lib.rs:30`).

---

## PHASE 2 — BUILD / COMPILE AUDIT

**EXECUTED.**
- `cargo check --workspace --offline --all-targets` → **exit 0, 0 errors, 110 warnings**, finished in ~86 s.
- `cargo test --workspace` → **262 passed, 0 failed**.
- `cargo test` (staking program, host target) → **17 passed, 0 failed**.
- `cargo build-sbf` for the on-chain program → **NOT EXECUTED** (Solana/BPF toolchain not installed). The program is only proven to compile as a **host** library; BPF compilation and on-chain behaviour are **NOT VERIFIED**.

**Warning breakdown (110):**
| Count | Warning | Severity |
|---|---|---|
| 91 | `missing_docs` (struct fields/variants/statics/const) | LOW (lint noise; docs discipline is on) |
| 4 | deprecated `solana_sdk::system_instruction`/`system_program` → use `solana_system_interface` | LOW/MEDIUM (dependency drift) |
| 3 | deprecated `Keypair::from_bytes` → `try_from(&[u8])` (`tokens.rs`) | LOW |
| 1 | dead code: function `err` never used (`server/api.rs:242`) | LOW |
| 1 | dead code: field `chain_id` never read | LOW |

**Dependency review:** `solana-sdk/client/program 2.1` (caret → resolves 2.3.13), `spl-token 6`, `spl-associated-token-account 4`, `axum 0.7`, `tokio 1`, `reqwest 0.12` (rustls, no OpenSSL), `tokio-tungstenite 0.24`, `k256 0.13`, `ed25519-dalek 2`, `tiny-keccak 2`, `borsh 1.5`, `thiserror/anyhow/tracing`. No invalid deps, **no version/feature conflicts** (it compiles), no obviously deprecated crates beyond the `solana_sdk` re-export notices above. Solana SDK usage is correct (`MessageV0::try_compile`, `VersionedTransaction`, default features intentionally enabled — documented in root `Cargo.toml`).

**Findings:**
- **CRITICAL:** none in the application build.
- **HIGH:** on-chain program BPF build **NOT EXECUTED / NOT VERIFIED** (see Phase 6 — it also has a critical logic flaw).
- **MEDIUM:** deprecated Solana re-exports indicate the code targets solana-sdk 2.x APIs that are being migrated out; a future 2.x/3.x bump will need `solana_system_interface`.
- **LOW:** 91 missing-docs warnings, 2 dead-code items.

---

## PHASE 3 — SNIPER BOT AUDIT

**Detection (`module-sniper/src/detect.rs`).** Two independent feeds merged into one channel:
1. **PumpPortal `subscribeNewToken`** (third-party WS; PumpPortal runs its own Geyser and pushes on creation) — `detect.rs:52–67`.
2. **Solana `logsSubscribe`** on the pump program (redundancy) — `detect.rs:70–80`, served by `solana-kit/src/ws.rs:457`.

**Entry (`entry.rs:41 consider_launch`).** Pipeline: dedup `mark_launch_seen` (`entry.rs:47`) → load `PumpContext` (RPC `getMultipleAccounts` for bonding-curve+global) → `risk.check_entry` (`entry.rs:111`) → size → `buy_on_curve` (`pump::plan_buy`+`build_buy_ix`, `entry.rs:168`) or `buy_graduated_via_jupiter` (`entry.rs:233`) → `executor.run(req)` (`entry.rs:216`). Priority fee + compute budget + optional Jito tip are attached (`entry.rs:205–210`). Latency is instrumented (`entry_latency_ms`, `observe_age_ms`).

**Transaction construction/signing (`tx.rs`).** `MessageV0::try_compile` with address-lookup-table support; `VersionedTransaction::try_new(msg, &[wallet.keypair()])` (`tx.rs:212`). Blockhash override or cached `latest_blockhash(false)` (`tx.rs:172–174`). Size/headroom check present. **`extra_signers` is not actually supported** (`tx.rs:203–210` logs and ignores) — single-signer only.

**Execution / priority fees / compute (`execute.rs`, `tokens.rs:248`).** `ExecPolicy` supports `Rpc`, `Jito`, `JitoThenRpc` broadcast (`execute.rs:327`). `set_compute_unit_limit` + `set_compute_unit_price` are prepended (`tokens.rs:252–263`). Retry loop rebuilds only on stale-blockhash/transient errors (`execute.rs:195–228`). RPC has its own retry/backoff + failover chain (`rpc.rs:179–208`), blockhash caching with invalidation (`rpc.rs:322–347`), and `send_transaction` with `max_retries:0` (`rpc.rs:466–475`) — a correct low-latency pattern.

**Duplicate-event protection.** `seen_launches` (`mark_launch_seen`) + `seen_signatures` (`mark_signature_seen`) HashSets, plus risk-engine duplicate-symbol and re-entry cooldown. **However these sets are never pruned** (`state.rs:652–666`; no `retain`/cap anywhere) → **unbounded memory growth** on a long-running sniper.

**Race conditions / concurrency.** Shared state is `Arc<AppState>` with **per-field `tokio::sync::RwLock`** (`state.rs:23–45`) — fine-grained, low contention. No `unsafe`. Config is a hot-reloaded snapshot; modules call `set_policy` each loop, so runtime `/mode` changes propagate (`module-sniper/src/lib.rs:126`).

**Realistic architecture-level latency (NOT "1 s guaranteed").** Critical path = detection + `PumpContext::load` (an on-demand RPC round trip) + `check_entry` + build/sign + **`simulate_first` (hard-coded `true`, `execute.rs:783`)** + broadcast + landing.
- Detection via PumpPortal/public `logsSubscribe`: ~100 ms–1 s+ (third-party/public, variable).
- Context load (RPC): ~50–200 ms.
- Simulate round trip: ~100–400 ms.
- Send: ~50–200 ms; landing: ~0.4–2 s+ (slot time + congestion).
- **Net: first buy *submission* realistically ~0.3–1.3 s; *landing* within 1 s is NOT guaranteed.**

**What sub-second/near-real-time actually requires (MISSING here):**
- Self-hosted/co-located **Yellowstone/Triton Geyser `transactionSubscribe`/`accountSubscribe`** wired into the feed (the WS primitive exists at `ws.rs:473`, but the sniper uses `logsSubscribe` and the copy feed falls back to polling — **Geyser path NOT wired**). Detection in tens of ms.
- **Pre-fetch/warm-cache** bonding-curve+global accounts (or keep them via `accountSubscribe`) so `PumpContext::load` is off the critical path.
- Option to **skip/parallelise `simulate`** on the snipe path (current default adds a round trip).
- **Multi-RPC fan-out** (first-to-land) + proximity networking; Jito bundles/tips (supported ✓).

**MISSING for professional deployment:** Geyser integration, warm account cache, simulation-bypass switch, multi-endpoint fan-out, pruning of dedup sets, BPF/testnet-proven execution.

---

## PHASE 4 — COPY TRADING AUDIT

**Wallet monitoring (`feeds.rs`).** `copy.feed` selects `pumpportal` (`subscribeAccountTrade`) or `logs_poll`; `transaction_subscribe` is **accepted but downgraded to polling** (`feeds.rs:93–96`). Wallet list is hot-reloaded from config (`lib.rs:129`), and `COPY_WALLETS` env appends (`config.rs:999`).

**Transaction / instruction parsing (`solana-kit/decode.rs`, 1063 L).** Decodes swaps from balance deltas; classifies venue (pump/PumpSwap/Raydium/Jupiter); handles versioned + loaded addresses; rejects wrong encodings with clear errors. Well tested (18 decode tests).

**Buy/sell detection + copy execution (`mirror.rs:34 mirror_trade`).** Per-wallet staleness gate (`max_staleness_secs`, `mirror.rs:119–124`), already-holding check (`mirror.rs:140`), copy-specific preflight+cooldown (`mirror.rs:147`), `risk.check_entry` (`mirror.rs:168`), then `buy_on_curve`/`buy_via_jupiter` (`mirror.rs:221–239`). Exits mirrored via `exit.rs` when `mirror_exits`/`full_exit_on_their_exit`.

**Position sizing (`mirror.rs:529 size_for`).** `fixed_sol` wins; else `their_sol × fraction_of_their_size`, capped by `max_sol`. Per-wallet slippage override. Correct, configurable.

**Duplicate prevention.** Layered: `mark_signature_seen` + `mark_copied(wallet,mint)` cooldown (`mirror.rs:513`) + risk-engine duplicate-symbol. Good.

**Failure recovery / partial execution / confirmation.** Shares `Executor` retry/confirm with the sniper (blockhash rebuild, transient retries, confirm polling). **Partial-fill handling is NOT VERIFIED** — Solana swaps are atomic, but Jupiter multi-hop partial outcomes and "sent-but-not-landed" reconciliation rely on `confirm` + `signatures_for_address`; there is no explicit partial-fill state machine.

**Rate limits.** PumpPortal tier via optional API key; RPC retry/backoff. No explicit per-wallet rate limiter beyond cooldowns.

**MISSING for production:** real Geyser `transactionSubscribe` feed (currently polling/PumpPortal-dependent), wallet-state reconciliation against on-chain truth, explicit partial/failed-fill recovery, pruning of `seen_signatures`/`last_copy_at` maps (unbounded), backtesting harness.

---

## PHASE 5 — POLYMARKET AUDIT

**Authentication (`auth.rs`).** L1 = EIP-712 over `ClobAuth(address,string timestamp,uint256 nonce,string message)` (`auth.rs:33`) → `derive_api_key` (`clob.rs:357`). L2 = HMAC-SHA256 over `timestamp+method+path+body`, base64 secret, headers `POLY_ADDRESS/POLY_SIGNATURE/POLY_TIMESTAMP/POLY_API_KEY/POLY_PASSPHRASE` (`auth.rs:53–85`). Matches the documented CLOB two-layer scheme. Tested (5 auth tests incl. determinism + body-sensitivity).

**CLOB API (`clob.rs`).** `server_time`, `order_book`, `order_books` (POST `/books`), `price`, `midpoint`, `market`, `tick_size`, `post_order` (POST `/order`, `clob.rs:311`), `cancel_order` (DELETE `/order`), `cancel_all` (DELETE `/cancel-all`), `heartbeat` (POST `/heartbeat`, dead-man's switch). Complete order lifecycle.

**Order construction (`orders.rs`).** Faithfully mirrors `py-clob-client` `get_order_amounts`: per-tick `RoundConfig::for_tick` (0.1/0.01/0.001/0.0001), truncate/round-up/down to 6-decimal token amounts (`orders.rs:32–124`). Tested.

**EIP-712 signing (`eip712.rs`).** **V2** `Order` with the exact 11-field typehash (`eip712.rs:33`), domain `name="Polymarket CTF Exchange"`, `version="2"`, `chainId=137`, `verifyingContract=exchange`; digest `keccak256(0x1901‖domainSep‖structHash)`; keccak (not SHA3); type-3 deposit-wallet wrapping. Tested against known vectors (keccak-of-empty constant, EIP-55 checksum, sign/recover roundtrip). The doc comments explicitly note the V1→V2 migration and `order_version_mismatch` rejection — genuine, current understanding.

**Contract addresses — externally verified.** The configured addresses match the **official Polymarket docs (docs.polymarket.com/resources/contracts)** and the **`Polymarket/ctf-exchange-v2` GitHub**: CTF Exchange V2 `0xE111180000d2663C0091e4f400237545B87B996B`, NegRisk V2 `0xe2222d279d744050d28e00520010520000310F59`, pUSD collateral proxy `0xC011a7E12a19f7B1f670d46F03B03f3342E82DFB`, CTF `0x4D97DCd97eC945f40cF65F87097ACe5EA0476045`. These are the **current V2** contracts (older sources cite the legacy V1 `0x4bFb41d5…` + USDC.e `0x2791Bca1…`). The code targets V2 correctly.

**Strategy (`strategy.rs`).** `value` (basket-edge) and `search` (keyword) strategies; min-edge gate; closed-market skip. Tested (6 strategy tests).

**WebSocket (`ws.rs`).** Book event parsing, quote map, pong/garbage tolerance. Tested.

**Live gate (`lib.rs:347`).** `will_send = mode==Live && signer.is_some() && api_key.is_some()`. Without `POLYMARKET_PRIVATE_KEY` → read-only/paper. Correct.

**MUST be verified externally (NOT VERIFIED here, needs live API + keys):**
- That the **CLOB REST/WS endpoints and JSON schemas** (`clob.polymarket.com`, `gamma-api.polymarket.com`, `ws-subscriptions-clob…`) are current and unchanged for V2/pUSD.
- That **pUSD allowances/approvals** to the V2 exchange are handled (order signing is off-chain; the operator settles — the user must have set token allowances; the bot does not appear to submit approvals).
- End-to-end **order acceptance/match/cancel** against the live CLOB (no integration test exists).
- Gamma market-discovery schema currency.

**Verdict:** architecturally complete and correctly targets the **current V2** API/contracts; signing logic is correct and vector-tested. Live behaviour is **NOT VERIFIED** (no integration/e2e test; requires keys + network).

---

## PHASE 6 — SMART CONTRACT SECURITY AUDIT

Program: native Solana (no Anchor), `programs/staking-suite`. Host unit tests pass (17), but **BPF build NOT EXECUTED** and **no `solana-program-test`/on-chain test** exists (acknowledged in its `Cargo.toml`). On-chain behaviour is **NOT VERIFIED**.

### 🔴 CRITICAL — Missing account ownership/address validation → vault drain + infinite mint
`processor.rs` validates almost nothing about the accounts it is handed:

- **`process_unstake`/`process_claim` (`processor.rs:330–419`):**
  - `config_acc` is deserialised (`:348`) with **no check that `config_acc.key == config_pda(program_id)` and no `config_acc.owner == program_id`**.
  - `stake_acc` is deserialised (`:349`) and only checked for `sa.owner == staker` (`:350`) — **no check that `stake_acc.key == stake_pda(program_id, staker)` and no `owner == program_id`**.
  - `token_program` (`:342`), `mint`, `vault` are **not checked** against `spl_token::id()` / `config.mint` / `config.vault`.
  - It then `invoke_signed`s a token transfer of `sa.amount` **out of `vault`** (`:364–380`) and `mint_to` of `sa.accrued_rewards(...)` (`:386–402`), both signed by the **correctly derived** config PDA (`:354`) using `config.config_bump` read from the *unvalidated* config account.
  - **Exploit:** anyone passes a **fabricated `stake_acc`** (bytes deserialising to `StakeAccount{owner: attacker, amount: huge, reward_from: 0, staked_at: 0}`), a **fabricated `config_acc`** (`initialized:true`, canonical `config_bump`, `unstake_delay:0`, `reward_rate_bps: huge`), the **real `vault`/`mint`/`token_program`**, and their own `staker_token`. Result: **drain the entire staking vault** and **mint unbounded reward tokens**. No privilege required. This is a total-loss vulnerability.

- **`process_stake` (`processor.rs:219–327`):** `config_acc` is again **not validated** (`:234`) → attacker-controlled `fee_bps`/`min_stake`/`reward_rate_bps`. `vault`/`treasury` are the **passed accounts**, not checked against `config.vault`/`config.treasury`. `token_program` not checked. (`stake_acc.key` *is* checked at `:282–285`, but `stake_acc.owner` is not.)

- **`process_initialize` (`processor.rs:76–217`):** better — checks `payer.is_signer`, `mint_acc.is_signer`, and `config_key == config_acc.key` (`:104–107`), with a re-init guard (`:109–114`). But it still does **not** verify `token_program == spl_token::id()`, `assoc_program`, or `system_program` ids, nor that `vault`/`treasury` are the canonical ATAs.

**Root cause:** hand-rolled native program with **no account-validation layer** (the thing Anchor gives you for free). Every CPI authority is derived correctly, but the *input accounts* are trusted.

### Other classifications
- **HIGH — No program-id checks on CPI targets.** `token_program`/`system_program`/`assoc_program` are used as invoke targets without asserting their ids; a fake `token_program` receives the config-PDA signature via `invoke_signed` and can act as the vault/mint authority.
- **MEDIUM — Upgrade/admin centralisation.** `admin = payer` of `initialize` (`:187`); `process_update` (`:421`) lets admin change `fee_bps`/`reward_rate_bps`/`min_stake`/`unstake_delay` at any time with **no timelock, no caps, no multisig, no event emission**. A malicious/compromised admin can set `reward_rate_bps` extreme or `fee_bps=10_000` (100%). There is **no upgrade-authority/immutable decision recorded** and **no emergency pause**.
- **MEDIUM — Reward inflation model.** Rewards are **minted** (inflationary), not paid from a funded pool; combined with the missing validation this is the drain vector. Even when fixed, unlimited minting needs a supply cap / rewards-vault accounting.
- **LOW — `decimals` is informational only**; no validation against the created mint.
- **LOW — Rent/`data_len` assumptions.** `config_acc.data.borrow_mut()[..serialized.len()]` (`:213`, `:458`) assumes the account was sized exactly; fine given create flow, but brittle if `Config` grows (no migration path / discriminator versioning).
- **INFORMATIONAL — `overflow-checks = true`** in release profile (good) and saturating/checked arithmetic in `compute_reward`/`compute_fee` (good).

**Do NOT deploy this program.** It requires a full validation layer (assert PDAs, owners, program ids, and that `vault==config.vault`, `mint==config.mint`, `treasury==config.treasury`), an admin timelock/multisig, a reward-supply model, pausability, and **real `solana-program-test` coverage + a professional audit**. Claiming it is secure would be false.

---

## PHASE 7 — TELEGRAM CONTROL AUDIT

- **Authentication/authorization (`commands.rs:40 is_authorized`, enforced `lib.rs:121–141`).** **Deny-by-default**: empty allow-lists ⇒ all control commands refused; unauthorized attempts are logged, replied "⛔ not authorized", and skipped (never reach `handle`). Allow by `allowed_user_ids` or `allowed_chat_ids`. Strong default.
- **Roles.** Single tier (allowed vs not). **No granular admin/operator roles** (e.g., read-only vs kill-only) — **MISSING**.
- **Command validation (`commands.rs:57 parse_command`).** Whitelist parse; `@botname` suffix stripped; optional prefix; unknown → `Command::Unknown`. Targets parsed for on/off. Tested (9 command tests).
- **Dangerous-command protection.** `/kill`, `/resume`, `/mode live`, `/on|/off` all require authorization. `/mode live` still cannot broadcast unless `allow_live_trading` is true (defence in depth via `exec_policy_from_config`). Reasonable.
- **Secret handling.** Token read from env var named by `bot_token_env` (`lib.rs:47`); never logged. Good.
- **Notifications (`alerts.rs`).** Classified by `alert_on_*` with cooldown + per-minute cap; message chunking ≤4096 with UTF-8-boundary safety (`api.rs`, tested). Good.
- **Concurrency / rate limiting.** Long-poll loop with `offset` tracking (`lib.rs:93–108`); alert rate cap. No per-user command rate limit — **minor**.
- **Wallet management via Telegram.** **MISSING** (no key/withdrawal commands — which is *good* for safety; control is on/off + mode + status only).

**Can Telegram safely control trading?** Yes for start/stop/kill/status given deny-by-default + the live gate. Gaps: no role separation, no command rate-limit, and alerts render untrusted strings (see Phase 8 XSS — the same symbols flow to the dashboard).

---

## PHASE 8 — SECURITY AUDIT (application)

- **Private keys / seeds / API keys.** Solana keypair and Polygon key are **env-only** (`config.rs:1117–1128`), with `SecretConfig::redacted()` masking (`config.rs:652`) and `/api/config` returning `"<redacted>"` (`api.rs:115`). `Wallet::load` never logs the secret (`tokens.rs:121`). Telegram token via env-name indirection. **Good baseline.**
- **Secrets in source.** None found (only env names + placeholder program id). **Good.**
- **Logging of secrets.** Not observed; pubkey/address only. **Good.**
- **`unsafe`.** None anywhere (`#![forbid(unsafe_code)]` in the program). **Good.**
- **🟠 HIGH — Stored XSS → control-plane takeover (`dashboard.rs:185,194,213`).** Untrusted on-chain strings (token `symbol`/`symbol_display`, launch `symbol`, wallet/mint slices, RPC `error.message`, risk `reason`) are concatenated into `innerHTML` with **no escaping** (no `escapeHtml` helper exists). The API key is read from `#apiKey` in the DOM (`dashboard.rs:138`), so injected JS can read it and call `/api/mode`, `/api/kill`, module toggles. A pump.fun creator fully controls the symbol ⇒ realistic remote attack against an operator who has the dashboard open.
- **🟠 HIGH — Open-by-default control API.** `require_auth` returns `Ok(())` when no key is configured (`api.rs:61–62`); default `bind_host = "0.0.0.0"`, `bind_port = 8080`, `cors_origins = ["*"]` ⇒ `CorsLayer::new().allow_origin(Any)` (`main.rs:180–184`). With no `API_KEY` set, **any host that can reach the port can kill/resume/switch mode/enable modules**, and all read routes + the `/api/events` WS are unauthenticated (info disclosure of positions/trades/fills). No TLS, no rate limiting.
- **MEDIUM — `std::env::set_var` inside async `main` (`main.rs:218,226,231`).** Mutating the process environment while the tokio runtime's threads exist is a data race (it is `unsafe` in Rust 2024). Called before modules spawn, so it works in practice, but it is a latent soundness issue.
- **MEDIUM — Unbounded in-memory growth.** `seen_launches`, `seen_signatures` (`state.rs:38–39`), `last_exit_at`, `last_copy_at` are never pruned ⇒ memory-exhaustion DoS on long runs. (`trades`/events buffers *are* capped.)
- **MEDIUM — No `zeroize`.** Key material lives in heap memory for process lifetime with no wiping on drop.
- **Deserialization.** `serde_json`/`borsh`/`bincode` with explicit types; `deny_unknown_fields` on config; decode paths reject bad encodings with errors (tested). No `serde` gadget risk. **OK.**
- **SSRF.** Outbound URLs come from config (operator-controlled), not user input. **Low risk.**
- **Path traversal.** Storage paths from config only (`storage.rs:30–45`); keypair path operator-controlled. **Low risk.**
- **Command execution.** None (no `std::process::Command`). **Good.**
- **Replay / duplicate execution.** Transaction dedup via `seen_signatures`; Polymarket orders use salt + timestamp; blockhash invalidation on retry. Reasonable, but **replay safety across restarts is NOT VERIFIED** (dedup sets are in-memory only; a restart clears them).
- **Supply chain.** Pinned `Cargo.lock`; mainstream crates; no vendoring/`cargo-audit`/`cargo-deny` (**MISSING**). No SBOM.

**Commercially disqualifying as-is:** the dashboard XSS + open control API + the CRITICAL contract flaw. All are fixable, but none should ship.

---

## PHASE 9 — PERFORMANCE AUDIT

- **Async architecture.** Clean tokio design: each module is a task; `mpsc` for feeds; `broadcast` for events; per-field `RwLock` state. No blocking calls on async paths observed (file IO uses `tokio::fs`).
- **CPU-bound work.** Crypto (keccak/ed25519/HMAC), borsh/bincode (de)serialisation, base58, JSON parsing — all light per event. Instruction building does small allocations (`Vec<AccountMeta>`), acceptable.
- **Lock contention.** Fine-grained locks minimise contention; hot path takes several short read locks in `check_entry`. Fine for single-operator scale.
- **Caching / pooling.** Blockhash cache with invalidation (`rpc.rs:322`); `reqwest::Client` reuse (connection pooling); prebuilt-tx cache with expiry (`execute.rs:733,748`). Good.
- **Allocations on the hot path.** `PumpContext::load` performs an RPC fetch per launch (network, not alloc-bound) — the main latency cost, not CPU.
- **Is Rust sufficient?** **Yes.** The bottleneck is **network/infrastructure latency**, not language speed. 
- **Would C++ help?** **No meaningful benefit.** The hot path is IO-bound (WS ingest, RPC, signing). C++ would add risk and cost without measurable latency gain. **Do not add C++.**
- **Python/TypeScript?** None present. The embedded dashboard JS is minimal and appropriate; no separate TS/Python services exist, so nothing to remove/isolate. **Do not add another language for marketing.**
- **Real perf gaps:** Geyser ingest (vs polling/PumpPortal), warm account cache, simulate-bypass, multi-RPC fan-out, and pruning of dedup maps. These are architecture/infra, not language, issues.

---

## PHASE 10 — PRODUCTION ARCHITECTURE (target, based on existing code)

The current code already implements the user's intended shape (single Axum control plane supervising module tasks over shared state + event bus). Recommended refinements (do **not** rewrite the working cores):

```
Operator/Buyer
   │
   ├─ Telegram bot (module-telegram) ── deny-by-default auth, roles(TODO)
   └─ Web dashboard / Admin UI ──────── FIX XSS, add authN/authZ, TLS
   │
   ▼
Axum API (server/api.rs) ── ADD: mandatory API key/JWT, per-route authz, rate limit, TLS/reverse-proxy
   │
   ▼
Trading Orchestrator (server/main.rs spawn_modules) ── keep
   │
   ├─ Sniper (module-sniper) ── ADD Geyser feed + warm cache + sim-bypass
   ├─ Copy   (module-copy)   ── ADD real transactionSubscribe + reconciliation
   └─ Polymarket (module-polymarket) ── keep (verify live)
   │
   ▼
Risk Engine (bot-core/risk.rs) ── keep (strong); ADD persistent limits across restart
   │
   ▼
Execution Engine (solana-kit/execute.rs,tx.rs,rpc.rs) ── keep; ADD multi-RPC fan-out, Jito (have)
   │
   ▼
Blockchain / Trading APIs (Solana RPC/WS/Jito; Polymarket CLOB/Gamma)
   │
   ▼
Persistence ── REPLACE JSONL-only with Postgres (orders/fills/positions/audit) + Redis (dedup/cache/rate-limit)
   │
   ▼
Observability ── ADD Prometheus metrics, structured logs (have tracing), alerting, health/ready, tracing IDs
```

Key changes vs. the user's diagram: (1) auth is **mandatory** at the API, not optional; (2) add a **persistence tier** (the current JSONL/in-memory store is single-process and loses dedup on restart); (3) add **observability**; (4) the **Geyser** ingest belongs between feeds and the orchestrator for latency.

---

## PHASE 11 — MISSING FEATURES (P0 must / P1 important / P2 nice)

| # | Feature | Why required | Status | Difficulty | Effort | Priority | Commercial importance |
|---|---|---|---|---|---|---|---|
| 1 | Contract account-validation layer | Prevents total fund loss | **MISSING** | Medium | 2–4 d + audit | **P0** | Critical |
| 2 | Dashboard XSS escaping + API authN | Remote takeover prevention | **MISSING/partial** | Low | 1–2 d | **P0** | Critical |
| 3 | Mandatory API auth + TLS + rate limit | Safe remote control | Partial (key optional) | Low/Med | 2–3 d | **P0** | Critical |
| 4 | `solana-program-test` + integration/e2e tests | Prove it works | **MISSING** | High | 1–2 w | **P0** | Critical |
| 5 | Testnet/mainnet live verification (all 3 trading modules) | "Works" claim | **NOT VERIFIED** | High | 1–2 w | **P0** | Critical |
| 6 | Dedup/state pruning + bounded memory | Long-run stability | **MISSING** | Low | 1 d | **P0** | High |
| 7 | Postgres + Redis persistence (multi-restart safe) | Commercial durability | **MISSING** (JSONL) | High | 1–2 w | P1 | High |
| 8 | Geyser (`transactionSubscribe`) feed + warm cache | Sub-second sniping | Scaffold only | High | 1–2 w | P1 | High |
| 9 | Observability (metrics/health/alerting) | Operability | Partial (tracing) | Medium | 3–5 d | P1 | High |
| 10 | CI/CD + `cargo-audit`/`cargo-deny` + SBOM | Supply-chain assurance | **MISSING** | Low | 1–2 d | P1 | Medium |
| 11 | Admin timelock/multisig + pause for contract | Governance safety | **MISSING** | Medium | 3–5 d | P1 | High |
| 12 | Multi-RPC fan-out + Jito tuning | Landing rate | Partial (Jito yes) | Medium | 3–5 d | P1 | Medium |
| 13 | Telegram roles + command rate limit | Least privilege | **MISSING** | Low | 1–2 d | P2 | Medium |
| 14 | Backtesting / paper PnL analytics | Buyer confidence | **MISSING** | High | 1–2 w | P2 | Medium |
| 15 | Multi-tenancy / per-user accounts | SaaS licensing | **MISSING** | High | 2–4 w | P2 | High (for SaaS) |
| 16 | Key management (KMS/HSM/zeroize) | Custody safety | **MISSING** | Medium | 3–5 d | P1 | High |

---

## PHASE 12 — CODE QUALITY (scores 0–10)

| Module | Score | Notes |
|---|---|---|
| `solana-kit` | **8/10** | Best crate. Correct modern Solana APIs, IDL-verified discriminators, 164 meaningful tests, retries/failover/caching. Minor: deprecated re-exports, `extra_signers` stub. |
| `bot-core` | **8/10** | Strong risk engine, clean state model, `deny_unknown_fields`, redaction, validation. Minor: unbounded dedup maps, no persistence abstraction. |
| `module-polymarket` | **8/10** | Correct V2 EIP-712 + L1/L2 auth, mirrors `py-clob-client`, vector-tested. Minor: live unverified, no integration test. |
| `module-sniper` | **7/10** | Clear pipeline, latency instrumentation, redundancy. Minor: no Geyser, on-demand context load, dedup leak. |
| `module-copy` | **7/10** | Good sizing/dedup/staleness. Minor: polling fallback, no reconciliation/partial-fill state machine. |
| `module-telegram` | **7/10** | Deny-by-default, chunking, alert caps. Minor: no roles/rate-limit. |
| `server` (control plane) | **6/10** | Clean Axum routing + graceful shutdown. **Lower** due to open-by-default auth, wildcard CORS, dashboard XSS, `set_var` in async. |
| `staking-suite` (contract) | **3/10** | Good borsh math + tests, **but** the missing account-validation layer is a critical, fund-losing defect; no on-chain tests. |

Cross-cutting: consistent naming, good module boundaries, `thiserror`/`anyhow` error handling, async patterns idiomatic, `tracing` logging, doc comments widespread. **No integration tests, no CI** are the main process gaps.

---

## PHASE 13 — TESTING

**Current:** 279 **unit** tests (pure logic: math, EIP-712 vectors, IDL discriminators, borsh roundtrips, risk gates, command parsing, decode, rounding). They are **meaningful**, not smoke tests.
**MISSING:** integration tests (`tests/`), end-to-end, mock-server/HTTP tests, **blockchain/`solana-program-test`**, transaction lifecycle tests, failure/chaos tests, load tests, security tests (fuzz/property), CI gating.

**Required production test plan:**
- **Unit (keep + raise):** target **≥80 %** line coverage on `bot-core`, `solana-kit`, `module-polymarket`; property tests for `compute_reward`/rounding/size math.
- **Contract:** `solana-program-test` for every instruction incl. **negative tests** (wrong owner/PDA/program-id must fail), re-init, cooldown, fee split, reward accrual, admin update; **fuzz** the processor; third-party audit.
- **Integration:** spin a local validator (`solana-test-validator`) + mock PumpPortal/CLOB/Gamma (e.g. `wiremock`); assert full launch→buy→exit and copy→mirror→exit lifecycles in **paper and simulate**.
- **E2E (testnet/devnet):** real RPC + real PumpPortal + Polymarket testnet/Amoy; verify detection latency, landing rate, order acceptance, confirm/retry.
- **Failure scenarios:** RPC failover, WS disconnect/reconnect, stale blockhash, simulation failure, partial/failed fill, restart-dedup persistence, kill-switch under load.
- **Load:** sustained launch firehose; assert bounded memory (catches the dedup leak) and p50/p95 latency.
- **Security:** `cargo-audit`/`cargo-deny`, dependency scanning, XSS regression test for the dashboard, authz matrix test for API/Telegram.

---

## PHASE 14 — COMMERCIAL VALUE

Valued as a software asset (not LOC). Assumptions: single-tenant, self-hosted, buyer is technical, no live track record provided.

**A) Current codebase value: ≈ $10,000 – $22,000.**
Rationale: a large (~27 k LOC), **compiling**, **279-unit-tested**, well-architected Rust suite with **genuine, current domain knowledge** (pump.fun IDL/discriminators, PumpSwap/Raydium layouts, Polymarket **V2** EIP-712 + CLOB auth verified against official docs). That is months of skilled work and real IP. **Discounters:** a **CRITICAL** fund-losing contract flaw, dashboard XSS + open API, **no integration/e2e/program tests**, **no CI**, **no DB/multi-tenancy**, memory leak, and **live behaviour NOT VERIFIED**. A buyer inherits significant hardening before any real-money use.

**B) After minimum production hardening: ≈ $28,000 – $48,000.**
Assumes: fix contract validation (+ `solana-program-test` + audit), fix XSS + mandatory API auth/TLS/rate-limit, prune memory, add integration + testnet-verified e2e for all three trading modules, CI + `cargo-audit`, observability basics, deployment docs. Result: a credible **single-operator MVP/pilot** that demonstrably runs in paper/simulate and cautiously live.

**C) After professional productionisation/security/testing/docs: ≈ $60,000 – $120,000+.**
Assumes: full security audit + remediation, Geyser low-latency path, Postgres/Redis persistence + restart-safe dedup, multi-RPC fan-out, comprehensive test suite (unit/integration/e2e/load/failure/security), CI/CD, monitoring/alerting/dashboards, KMS-grade key handling, hardened deployment (containers/IaC), user + ops documentation, licensing framework, and (optionally) multi-tenancy for SaaS. This is a defensible commercial product.

---

## PHASE 15 — $20K / $40K / $60K ROADMAPS

### TARGET A — $20,000 (credible, hardened MVP; single operator)
- **Features:** all 5 modules running in paper/simulate **and** verified on **devnet/testnet**; contract validation layer fixed; dashboard usable.
- **Security:** fix CRITICAL contract flaw + XSS; **mandatory** API key; bind localhost/TLS-by-proxy; prune memory; secrets via env (have) + `zeroize`.
- **Testing:** `solana-program-test` (incl. negative), integration tests vs local validator + mocked feeds, testnet e2e smoke.
- **Docs:** README (have) + deploy + operator runbook. **Deployment:** Docker (have) + compose. **Monitoring:** health endpoint + structured logs.
- **Contract:** validation + admin basics; **audit not yet required** but program-test green.
- **Effort:** ~3–5 engineer-weeks. **Buyer:** individual trader / small dev shop / IP acquirer. **Risks:** live edge unproven; single-tenant.

### TARGET B — $40,000 (production candidate; small firm)
- **Everything in A, plus:**
- **Features:** Geyser `transactionSubscribe` feed + warm account cache + simulate-bypass switch; copy reconciliation; Polymarket live-verified.
- **Security:** professional **smart-contract audit** + remediation; admin timelock/multisig + pause; `cargo-audit`/`cargo-deny` + SBOM; rate limiting; CORS locked.
- **Testing:** failure/chaos + load tests; coverage gates; CI pipeline.
- **Persistence:** Postgres (orders/fills/positions/audit) + Redis (dedup/cache/limits), restart-safe.
- **Monitoring:** Prometheus metrics + alerting + tracing IDs. **Deployment:** hardened container + IaC + secrets manager.
- **Effort:** ~8–12 engineer-weeks. **Buyer:** prop team / web3 dev company / crypto automation firm. **Risks:** infra cost; operational burden.

### TARGET C — $60,000 (enterprise / licensable product)
- **Everything in B, plus:**
- **Enterprise:** multi-tenancy + per-user auth (JWT/RBAC), KMS/HSM key custody, full observability + SLOs, DR/backup, config management, admin UI.
- **Performance:** multi-region/low-latency networking, multi-RPC fan-out, Jito/Shredstream tuning, benchmarked p50/p95 latency + landing-rate dashboards.
- **Testing:** comprehensive suite + independent security audit (app + contract) + pen test; documented test evidence.
- **Contract:** audited, immutable-or-governed, reward-supply accounting, emergency controls.
- **Docs/licensing:** full API docs, SLA, licensing/entitlement, white-glove deploy.
- **Effort:** ~4–6 engineer-months. **Buyer:** trading firm / DeFi infra company / investor acquiring IP. **Risks:** scope, compliance, support commitments.

---

## PHASE 16 — BUYER PROFILE

- **Crypto trading firms / prop teams:** care about **latency, landing rate, risk controls, live track record, key custody**. Will discount heavily without testnet/mainnet proof and an audit. Most demanding.
- **Web3 development companies:** care about **code quality, architecture, extensibility, docs** — they will harden it themselves for a client. Best fit for Target A/B; they value the correct V2/IDL knowledge.
- **Crypto automation / bot SaaS companies:** care about **multi-tenancy, persistence, observability, licensing**. Need Target C.
- **Blockchain startups / DeFi infra:** care about the **contract** (must be audited) + modular Rust core. The contract flaw is a dealbreaker until fixed.
- **Investors acquiring software/IP:** care about **defensibility, uniqueness, time-to-market saved**. The ~27 k LOC compiling, tested, protocol-accurate core is the asset; they price in the hardening backlog.

---

## PHASE 17 — FINAL VERDICT

**CURRENT STATUS:** **Advanced prototype** (compiles, 279 unit tests, real protocol knowledge, good architecture) — **not yet MVP**, because no integration/e2e/program tests and live behaviour is unverified, and the contract has a critical defect.

**CURRENT ESTIMATED VALUE:** **$10,000 – $22,000**

**REALISTIC $20K POTENTIAL:** **YES** (achievable with the Target-A hardening; largely a security+test effort on an already-solid base).
**REALISTIC $40K POTENTIAL:** **POSSIBLE AFTER HARDENING** (requires Geyser latency path, persistence, audit, CI, observability).
**REALISTIC $60K POTENTIAL:** **POSSIBLE AFTER HARDENING** (requires full enterprise productionisation + independent audits; multi-tenancy for SaaS).

**BIGGEST 10 PROBLEMS**
1. **CRITICAL contract flaw:** no account ownership/PDA/program-id validation in `process_unstake`/`process_stake` → vault drain + infinite mint (`processor.rs:330–419`, `:219–327`).
2. **Dashboard stored XSS** from untrusted token symbols via `innerHTML` (`dashboard.rs:185,194,213`).
3. **Open-by-default control API** (no key ⇒ mutating routes open; `0.0.0.0`; wildcard CORS; unauth WS) (`api.rs:61`, `main.rs:180`).
4. **No integration/e2e/`solana-program-test`** — nothing proves it works end-to-end.
5. **Live behaviour NOT VERIFIED** (never run on testnet/mainnet; BPF build NOT EXECUTED).
6. **No low-latency Geyser path** wired into feeds (PumpPortal/polling/`logsSubscribe` only) ⇒ sub-second sniping not achievable as-is.
7. **Unbounded in-memory dedup/state** (`seen_launches`/`seen_signatures`/maps) ⇒ memory-exhaustion on long runs; dedup lost on restart.
8. **No persistence tier** (JSONL/in-memory only) ⇒ single-process, not restart-durable, not multi-tenant.
9. **Contract admin centralisation** (no timelock/multisig/pause/caps; inflationary mint) (`processor.rs:421`).
10. **No CI / supply-chain scanning** (`cargo-audit`/`cargo-deny`), deprecated Solana re-exports, `set_var` in async `main`.

**TOP 10 THINGS TO FIX (ordered)**
1. Add a strict account-validation layer to the staking processor (assert PDAs, `owner==program_id`, `token_program==spl_token::id()`, `vault==config.vault`, `mint==config.mint`, `treasury==config.treasury`); add `solana-program-test` negative tests; get an audit.
2. Escape all untrusted strings in the dashboard (add `escapeHtml`, or build rows with `textContent`/`createElement`).
3. Make API auth mandatory (fail closed if no key), default-bind to localhost, lock CORS, add TLS/reverse-proxy + rate limiting; require auth on `/api/events`.
4. Build `solana-program-test` + integration tests (local validator + mocked feeds) and a testnet e2e for sniper/copy/polymarket.
5. Prune/bound dedup + state maps (TTL/LRU); persist dedup across restarts.
6. Wire a Geyser `transactionSubscribe`/`accountSubscribe` feed + warm account cache; add a simulate-bypass option for the snipe path.
7. Add Postgres (orders/fills/positions/audit) + Redis (cache/dedup/limits).
8. Add admin timelock/multisig + pause + reward-supply cap to the contract.
9. Add CI (build/test/clippy/fmt), `cargo-audit`/`cargo-deny`, SBOM; replace deprecated `solana_sdk::system_*` and `Keypair::from_bytes`; remove `set_var` (pass secrets explicitly).
10. Add observability (Prometheus metrics, health/ready, tracing IDs, alerting) + key custody hardening (`zeroize`, optional KMS).

**MOST VALUABLE EXISTING COMPONENTS (do NOT rewrite without evidence)**
1. **`solana-kit`** (164 tests) — IDL-verified pump/PumpSwap/Raydium instruction builders + decode, modern `MessageV0`/`VersionedTransaction` signing, RPC retry/failover/blockhash cache, Jito. Genuinely hard to reproduce.
2. **`module-polymarket` EIP-712 V2 + CLOB auth** (44 tests) — correct, vector-tested, matches official V2 contracts/`py-clob-client`.
3. **`bot-core` risk engine + state/event bus** — comprehensive entry/exit gates and a clean concurrency model.
4. **Execution engine (`execute.rs`/`tx.rs`)** — sound simulate→broadcast→confirm with the live-gate downgrade.
5. **Control plane (`server`)** — working Axum REST+WS+dashboard supervisor (needs security hardening, not replacement).

---

## BUILD PLAN AFTER AUDIT (exact order)

1. **Freeze & baseline:** add CI (fmt/clippy/build/test), `cargo-audit`/`cargo-deny`; pin toolchain; confirm `cargo build-sbf` for the program (currently NOT EXECUTED).
2. **Stop the bleeding (security P0):**
   a. Rewrite the staking **account-validation layer**; add `solana-program-test` incl. negative/attack tests; do not deploy until an external audit passes.
   b. Fix **dashboard XSS** (escaping).
   c. Make **API auth mandatory**, localhost-default, CORS locked, TLS via proxy, rate limit; auth on WS.
   d. Bound/prune dedup + state maps; add `zeroize`.
3. **Prove it works (P0):** integration tests vs `solana-test-validator` + mocked PumpPortal/CLOB/Gamma; **devnet/testnet e2e** for sniper, copy, polymarket (paper→simulate→cautious live). Record evidence.
4. **Durability (P1):** Postgres + Redis persistence; restart-safe dedup; admin timelock/multisig + pause for the contract.
5. **Latency (P1):** Geyser `transactionSubscribe` feed + warm account cache + simulate-bypass + multi-RPC fan-out; benchmark p50/p95 + landing rate.
6. **Operability (P1):** Prometheus metrics, health/ready, tracing IDs, alerting, runbooks, hardened deploy (compose/IaC + secrets manager).
7. **Commercial layer (P2):** observability dashboards, backtest/PnL analytics, Telegram roles, then multi-tenancy/RBAC + KMS for SaaS licensing.
8. **Independent audits (gate to $40k/$60k):** smart-contract audit + application pen test; publish remediation.

**Bottom line:** the application half is a **high-quality advanced prototype** with real, current protocol knowledge and a sound architecture — worth buying and hardening, **not** rewriting. The **smart contract is the one component that is genuinely dangerous as written** and must be re-engineered (validation + tests + audit) before it has any commercial value. With the P0/P1 work above, the **$20k target is readily achievable**, **$40k is achievable**, and **$60k is achievable** as a professionally productionised, audited, multi-tenant product.

---

## REMEDIATION PROGRESS (post-audit fixes — executed & verified)

Test totals below are from actual `cargo test` runs in this workspace
(rustc/cargo 1.98.1). Baseline at audit time was **262 workspace + 17 staking**.

| # | BUILD PLAN item | Status | What changed | Verification |
|---|---|---|---|---|
| 2a | Staking **account-validation layer** (🔴 CRITICAL) | **DONE / VERIFIED (code)** | `programs/staking-suite/src/{processor.rs,error.rs}` rewritten: `require_signer` / `require_address` / `require_owner` / `load_config` / `require_staker_token`; every trusted account checked (config PDA + owner + initialized; vault/mint/treasury == config; staker token = SPL, config mint, staker-owned); PDAs sign via `invoke_signed`; 18 unique `Custom(6000+)` error codes. | `cargo build` exit 0; **10 new processor tests** assert each rejection path (wrong address / wrong owner / unallocated / uninitialized flag / bad token mint / bad token owner / wrong token program / missing signer). Staking suite now **28 passed, 0 failed**. ⚠️ On-chain (`build-sbf` + `solana-program-test`) still **NOT EXECUTED** — no Solana toolchain in this sandbox. |
| 2b | **Dashboard XSS** (🟠 HIGH) | **DONE / VERIFIED** | `crates/server/src/dashboard.rs`: added `esc()` helper; every untrusted `innerHTML` concat now escaped (module / position / trade rows, mode / cluster, push-feed summary / kind / time). | Raw-string intact; zero unescaped untrusted concats; `cargo check -p sniper-suite` exit 0. |
| 2c | **API auth fail-closed + WS auth + loopback default** (🟠 HIGH) | **DONE / VERIFIED** | `config.rs`: `ApiConfig.bind_host` default `0.0.0.0`→`127.0.0.1`. `main.rs`: `serve_api` **fails closed** (`anyhow::bail!`) if non-loopback bind + no API key; `is_loopback()` gate. `api.rs`: event WS enforces key via `x-api-key` header **or** `?key=` (browsers can't set WS headers), 401 on mismatch, open only on keyless loopback dev. `dashboard.rs`: `connectWs` appends `?key=`, closes prior socket, reconnects on key change. `config.toml.example` `[api]` updated. | `cargo check --workspace --all-targets` exit 0 (0 errors); **2 new `is_loopback` tests** (loopback set vs reachable set incl. `0.0.0.0`/`::`/RFC1918) + **1 config test** asserting `bind_host=="127.0.0.1"` and `config.toml.example` parses. |
| 2d-i | **Bound / prune dedup + state maps** (🟠 HIGH — memory exhaustion) | **DONE / VERIFIED** | `state.rs`: `seen_launches` / `seen_signatures` now FIFO-capped `BoundedSet` (evict oldest past `max_dedup_entries`); `last_exit_at` / `last_copy_at` pruned on insert via `prune_timestamps` (drop cooldown-expired + hard size cap, evict oldest). New `StorageConfig.max_dedup_entries` (default 100 000), documented in `config.toml.example`. | `cargo check -p bot-core --all-targets` exit 0 (no unused warnings); **6 new state tests** (dedup novelty, cap eviction, cap-of-1, remove, prune expired+cap, zero-cooldown floor TTL) + **1 config test** (default round-trips with the new field). |
| 2d-ii | `zeroize` on secrets | **NOT DONE** | — | deferred (low marginal value: `solana-sdk` `Keypair` already zeroizes; config secrets are cloned `String`s — needs a broader `Zeroizing<String>` refactor). |
| 1 | CI + `cargo-audit`/`cargo-deny` + `build-sbf` | **DONE (CI files) / VERIFIED (local gates)** | Added `rust-toolchain.toml` (pinned 1.98.1 + rustfmt/clippy), `deny.toml`, `.github/workflows/ci.yml` (3 jobs: **app** fmt/clippy/build/test, **program** fmt/clippy/test/**build-sbf**, **security** cargo-audit + cargo-deny). Whole repo run through `cargo fmt` (now format-clean). | Locally verified: `cargo fmt --all --check` exit 0 (both workspaces); `cargo clippy --all-targets -- -D clippy::correctness` exit 0 / 0 errors (both); `cargo test` **272 + 28 = 300 green** after reformat. ⚠️ `build-sbf`, `cargo-audit`, `cargo-deny` **run in CI only** — NOT EXECUTED locally (no Solana toolchain / tools not installed in sandbox). `deny.toml` licence allow-list is enforced **non-blockingly** on first runs until the transitive set is confirmed. |
| 1b | Deprecation-warning policy (Top-10 #10) | **DECIDED (non-blocking)** | 6 harmless deprecations remain (`Keypair::from_bytes` ×3, `solana_sdk::system_instruction`/`system_program` re-exports ×3). Chose a **correctness-only clippy hard gate** over fixing them now: the `system_*` fix needs a new `solana-system-interface` dep + call-site changes (build risk), so deferred as a tracked TODO rather than a partial fix. | `clippy -D clippy::correctness` exit 0 confirms no correctness lints; the ~165 style/doc/deprecated warnings are reported but non-blocking. |
| 4-i | Contract **admin hardening** — pause + two-step transfer + caps (Top-10 #9) | **DONE / VERIFIED (host)** | `state.rs`: `Config` gains `paused` + `pending_admin`; new `MAX_FEE_BPS` (10%) / `MAX_REWARD_RATE_BPS` (100% APR) caps. `instruction.rs`: `Pause`/`Unpause`/`TransferAdmin{new_admin}`/`AcceptAdmin` + `admin_ix` builder. `processor.rs`: `validate_params` (caps on init **and** update — reward rate was previously uncapped, fee cap tightened 100%→10%), `save_config`, `process_set_paused`/`process_transfer_admin`/`process_accept_admin`; `stake` rejects when paused (withdrawals never gated → cannot trap funds). `error.rs`: +`Paused`/`FeeTooHigh`/`RewardRateTooHigh`/`NotPendingAdmin` (now 22 codes). README security-model section added. | **7 new processor tests** (caps, pause toggle + non-admin reject, two-step transfer happy path + wrong-acceptor reject + non-admin reject + zero-key reject + accept-without-pending reject). Staking suite **35 passed, 0 failed**; `clippy -D clippy::correctness` exit 0. ⚠️ Still host-only — on-chain (`build-sbf` + `solana-program-test`) **NOT EXECUTED**. |
| 4-ii | Contract **parameter timelock + multisig path** (Top-10 #9, completes admin story) | **DONE / VERIFIED (host)** | `state.rs`: `Config` gains `timelock_secs` + `pending: PendingParams` (fixed-size borsh struct, values resolved at queue time); `MAX_TIMELOCK_SECS` = 30 days. `instruction.rs`: `Initialize` takes `timelock_secs`; `UpdateParams` gains `timelock_secs: Option` and now QUEUES; new `ApplyParams` (permissionless) + `CancelParams`; `update_params_ix`/`apply_params_ix` builders. `processor.rs`: `process_queue_update` / `process_apply_update` / `process_cancel_update` replace direct `process_update`; caps re-checked at apply; delay changes wait out the OLD delay (OZ `TimelockController` rule). `error.rs`: +`UpdateAlreadyQueued`/`NoPendingUpdate`/`TimelockNotElapsed`/`TimelockOutOfRange` (26 codes). Multisig: `admin` is any signer incl. CPI → deploy with a **Squads/Realms multisig PDA** as admin (documented in README; deliberately no in-program M-of-N — reuse audited infra). | **8 new tests** (timelock range, queue resolves-Nones/changes-nothing, queue rejects non-admin + double-queue + over-cap fee/rate/timelock, apply permissionless + boundary `t0+delay-1` reject / `t0+delay` accept, delay-shortening waits old delay, cancel auth). Staking suite **43 passed, 0 failed**; fmt + `clippy -D clippy::correctness` clean, no new warnings. ⚠️ Host-only; on-chain **NOT EXECUTED**. |
| 6 | **Observability** — structured tracing, health/readiness probes, Prometheus metrics (BUILD PLAN §6) | **DONE / VERIFIED (host)** | `crates/core/src/obs/` (NEW): `metrics.rs` — dependency-free Prometheus registry (Counter/Gauge/Histogram as `Arc<Atomic…>` handles, get-or-create registration → no dup panics, deterministic text-0.0.4 `encode()` with escaping, process-wide `global()`, `LATENCY_BUCKETS_MS`); `health.rs` — `HealthRegistry`/`ComponentStatus`/`HealthReport` (liveness vs readiness split, safe-detail contract). `config.rs`: new `[observability]` (`log_level`/`log_format` text\|json/`metrics_enabled`/`sample_interval_ms`) + `LOG_LEVEL`/`LOG_FORMAT`/`METRICS_ENABLED`/`SAMPLE_INTERVAL_MS` env overrides + validation. `solana-kit`: `rpc.rs` `retry`/`retry_raw` instrumented (`bot_rpc_requests_total{method,outcome=ok\|fatal\|exhausted}`, `bot_rpc_attempt_duration_ms{method}`); `ws.rs` `supervise` instrumented (`bot_ws_reconnects_total`, `bot_ws_connection_failures_total`). `crates/server/src/obs.rs` (NEW): `/health` (liveness — process-only, fixed 3-field body), `/ready` (200/503 + component report; components = rpc + 3 trading modules, heartbeat freshness 90 s, disabled ⇒ ready, telegram/contract excluded), `/metrics` (404 when disabled), `request_context` route-layer middleware (sanitized inbound `x-request-id` ≤128 `[A-Za-z0-9-_]` else generated, echoed; `info_span("request")`; exactly one info log/request; `bot_http_requests_total{route,method,status}` on **matched route patterns** + duration histogram), `record_event` + `spawn_event_pump` (EventBus → `bot_launches_total{accepted}`, `bot_execution_latency_ms{module,mode}`, `bot_whale_trades_total`, `bot_polymarket_events_total`, `bot_telegram_commands_total{accepted}`, `bot_app_errors_total{module,fatal}`, lag ⇒ `bot_events_dropped_total`), `sample_once` + `spawn_state_sampler` (build/uptime/kill-switch/positions/subscribers/mode gauges; per-module enabled/running/connected/healthy/errors gauges + authoritative counters mirrored via `Counter::set`; decision-queue depth `bot_module_queue_depth{module}` recorded by the sniper/copy consumers; rpc consecutive failures; `bot_health_ready`). `main.rs`: config now loads **before** tracing; `init_tracing(&ObservabilityConfig)` — `RUST_LOG` wins, json vs text arms, invalid filter → stderr + info fallback; pump + sampler spawned at startup. `api.rs`: `ApiState` +`health`/`metrics_enabled`; routes + `route_layer` (MatchedPath available post-routing); legacy `/api/health` kept. Root `Cargo.toml`: tower +`util`; server dev-dep `http-body-util`. **Secret-safety boundaries:** label values only from closed code-defined sets (mode sanitizer collapses unknowns to `other`); health details are counts/booleans only (never error payloads — RPC errors can embed key-bearing URLs); no WS URL labels. | **30 new tests** — metrics 9 (increments, get-or-create + label-order invariance, negative gauges, cumulative-inclusive buckets, deterministic encode + escaping, 8-thread × 1 000-inc concurrency, first-registration-wins buckets, global singleton), health 6 (empty-ready, degraded aggregation, healthy≠ready, overwrite/remove, safe JSON, 8-thread concurrency), config 2 (invalid format/interval/level rejected; unknown level warns), server obs 7 (event → counters incl. no-message-leak assertion, latency + mode sanitization, request-id accept/reject/generate boundaries incl. 128/129, sampler mirrors state, enabled-but-stopped ⇒ not-ready ⇒ running ⇒ ready, `module_component` readiness rules incl. 90 s boundary, sanitize closed set), api routes 6 (liveness stays 200 with all components down + 3-field body, /ready 200↔503 + degraded report + no secrets, /metrics content-type `text/plain; version=0.0.4`, 404 when disabled, x-request-id echo/replace/generate, legacy /api/health unchanged). `cargo test --workspace` **302 passed, 0 failed** (272→302); `cargo fmt --all --check` exit 0; `cargo clippy --workspace --all-targets -- -D clippy::correctness` exit 0, **no new warnings** from §6 code. Staking suite untouched (43 green, verified this session). ⚠️ Scrape/probe behaviour against a live orchestrator **NOT EXECUTED** (no Prometheus/k8s in sandbox); JSON log pipeline shape verified by code path, not by an external collector. |
| 3 | **Prove it works** — integration tests + devnet e2e (BUILD PLAN §3, P0) | **DONE / VERIFIED (host + local validator + public devnet)** | 7 new test files: `solana-kit/tests/mock_pumpportal.rs` (mock PumpPortal WS server: new-token/trade/migration subscribe frames, txType classification, garbage tolerance, reconnect+resubscribe), `module-sniper/tests/detect_feed.rs` (full Sniper detect feed over the mock WS → `launch_from_pumpportal` mapping), `module-copy/tests/copy_feed.rs` (CopyFeed wallet subscription → whale-trade mapping, side/venue derivation), `module-polymarket/tests/mock_clob_gamma.rs` (axum mock of CLOB+Gamma: query building, public endpoints, L1 derive-api-key headers, L2 signed `post_order` bundle wire-format assertions, unauthenticated local rejection), `core/tests/storage_lifecycle.rs` (journal append→restart fidelity, corrupt torn-line resilience, rotate/truncate), `solana-kit/tests/devnet_e2e.rs` (env-gated `E2E_NETWORK`/`E2E_LIVE`/`E2E_URL`: RPC basics, paper build+sign, simulate verdict, cautious live self-transfer), `programs/staking-suite/tests/validator_e2e.rs` (gated `STAKING_E2E`: spawns `solana-test-validator` with the compiled `.so` and drives the full governance lifecycle on the BPF VM). **Source fix proven by tests:** `PostOrderResponse` had no serde aliases — the live CLOB answers camelCase (`orderID`/`errorMsg`/…), so `order_id` was always `None` on the live order path; aliases added (`clob.rs`). **CI fix:** the program job pinned Solana 2.1.0 while the lockfile had drifted to the 2.3 generation (edition2024 crates) → CI `build-sbf` would have failed; lockfile now pinned to the 2.1.21 family (`rust-version = "1.79"` + MSRV-aware resolver in `programs/staking-suite/.cargo/config.toml`), CI pins 2.1.21 and runs the validator e2e. | **`cargo build-sbf` EXECUTED**: agave 2.1.21 / platform-tools v1.43 → `target/deploy/staking_suite.so` (163 288 bytes). Toolchain matrix: Agave 2.3 (v1.48 / Rust 1.84) cannot parse the edition2024 manifests an unpinned 2.3 lock pulls in; Agave 4.x (v1.54) fails `solana-zk-token-sdk` on BPF (`Pedersen` undeclared) — only the 2.1.21 family + v1.43 combination builds. `STAKING_E2E=1 cargo test --test validator_e2e` → **1 passed (9.4 s)**: initialize (CPI mint/vault/treasury/config creation), on-chain borsh config round-trip, mint authority == config PDA, re-init guard, stake guards (BelowMinimum → SPL-token InsufficientFunds → Paused ordering → InvalidStakeAccount), pause authorisation, timelock queue → **permissionless** apply → cancel + hard-cap rejection at queue time, two-step admin transfer + former-admin lockout. `cargo test --workspace` → **319 passed, 0 failed** (302 → +17 integration); staking host **43** + gated e2e **1**; `cargo fmt` + `clippy --all-targets -- -D clippy::correctness` clean in both workspaces. **Public devnet** (`E2E_NETWORK=1` vs `api.devnet.solana.com`): `devnet_rpc_basics` + `executor_paper` **PASS**; simulate skipped (public faucet rate-limited — graceful skip by design), live gated. **Local validator** (`E2E_URL=http://127.0.0.1:…`): **all 4 PASS incl. `executor_live`** — 0-lamport self-transfer reached `Confirmed` + `get_signature_status` ok: the full build→broadcast→confirm loop, no public-network side effects. ⚠️ **NOT EXECUTED:** live broadcast on public devnet (needs explicit approval + funded key per standing rule); real PumpPortal/CLOB/Gamma endpoints (mocked here); a funded end-to-end swap (needs real token + funded keys). 🟠 **NEW FINDING (functional gap, NOT FIXED):** the staking program has **no genesis distribution path** — the mint authority is the config PDA and the only `mint_to` mints rewards against an existing stake, so on a fresh deployment nobody can ever fund the first stake; the positive stake→reward→unstake money flow is blocked until an initial-mint (or authority hand-off) mechanism is designed. Contract change → needs user approval; flagged for §4/§8. Also noted: `Pubkey::new_unique()` is deterministic and its first value holds real devnet SOL — tests must not treat it as an unfunded fresh key. |
| 5 | **Latency** — Geyser push feeds + warm account cache + simulate policy + multi-RPC fan-out (BUILD PLAN §5, P1) | **DONE / VERIFIED (host + local validator + public devnet reads)** | `solana-kit/src/cache.rs` (NEW): `AccountCache` — per-lookup TTL (`Duration::ZERO` never hits), positives-only, FIFO eviction, hit/miss/stale counters → `bot_account_cache_total{outcome}`. `rpc.rs`: `with_account_cache`, `get_account_cached` / `get_multiple_accounts_cached` (misses batched into one `getMultipleAccounts`) / `account_exists_cached`; `token_program_of` warm-cached (mint owner immutable); `failover()` clones share the cache Arc. `pump.rs`: pump **Global** account served from cache (hit ⇒ only the curve round trip remains), bonding curve **always fresh**, ATA existence cached-positive. `execute.rs`: `ExecPolicy.fanout` + `broadcast_fanout` — races the same signed tx across primary + every fallback (first accept wins, dupes deduped by the leader), metered `bot_broadcast_fanout_total{outcome}`; `exec_policy_from_config` now maps `simulate_first` / `abort_on_simulation_failure` (previously hardcoded `true`). `config.rs`: `[network] account_cache_ttl_ms=30000` / `account_cache_max_entries=5000`, `[execution] simulate_first` / `abort_on_simulation_failure` / `broadcast_fanout`, `[sniper] use_transaction_subscribe` + 5 env overrides; `config.toml.example` updated. `decode.rs`: `parse_transaction_notification` + `TxNotification{succeeded, log_messages}` — provider schema drift degrades the feed instead of crashing it. `module-copy/feeds.rs`: **real `run_transaction_subscribe`** replacing the polling stub — Geyser push → dedup → `decode_swap` (same pipeline as polling), falls back to `run_poll` when the endpoint is missing/rejects/ends. `module-sniper/detect.rs`: third launch feed `start_geyser_subscription` (accountInclude = pump program, processed commitment, `Create` event from pushed meta logs → `LaunchFeed::TransactionSubscribe` + push slot). **TWO REAL BUGS FOUND BY THE NEW TESTS AND FIXED:** (1) `ws.rs subscribe()` inserted the `by_request` mapping even when disconnected and no frame was written — `register_outgoing` then skipped the subscription as "in flight" forever, so **any subscription registered before the socket came up was silently never sent** (affects the production logsSubscribe feed at startup); fixed: map only what is written, fail fast when the supervisor is gone, and `on_disconnect` clears in-flight mappings so reconnects re-send. (2) `module-copy run_poll` had the **dedup check inverted** (`mark_signature_seen` returns `true` = newly added): the polling copy feed skipped every NEW trade — it could never emit anything; fixed at both poll and geyser call sites (consumer in `lib.rs` and sniper `mark_launch_seen` were already correct — audited all 4 call sites). | **18 new tests, `cargo test --workspace` 337 passed / 0 failed** (319 → 337); `cargo fmt --all --check` exit 0; `cargo clippy --workspace --all-targets -- -A clippy::all -D clippy::correctness` exit 0. New: `cache.rs` 6 unit tests (fresh/stale/zero-TTL, overwrite, FIFO eviction, disabled, invalidate/clear, 8×50 concurrent accounting); `decode.rs` 3 (base64 notification → `decode_swap` end-to-end, failed-tx flagging, junk rejection — wire shape verified against a **live devnet `getBlock` response**: base64 txs serialize as the untagged `["<b64>","base64"]` form); `module-copy/tests/geyser_feed.rs` 2 (mock Yellowstone WS: subscribe frame shape `accountInclude`/`encoding:base64`/`transactionDetails:full`, pushed whale buy → `WalletTrade{side,venue,mint,25.0 tok,1.5 SOL,slot,block_time}`, failed tx skipped, no-URL ⇒ poll fallback); `module-sniper/tests/geyser_detect.rs` 2 (pushed `Create` event → `TokenLaunch{feed=TransactionSubscribe, slot, sig, mcap, supply}`, failed create skipped, subscribe filters pump program; missing URL + no other feed ⇒ loud config error); `solana-kit/tests/latency_bench.rs` 5 — **offline**: warm cache serves mint/ATA/token-program reads with the network dead (and no phantom hits), fan-out beats a rejecting primary via the healthy fallback (mock JSON-RPC HTTP, signature echoed from the wire tx, meter asserted); **gated live** (`E2E_NETWORK`/`E2E_LIVE`, deterministic CI): local validator — `getSlot` / `getLatestBlockhash` / `simulateTransaction` all p50 ≤ 1 ms, p95 ≤ 4 ms, **landing rate 3/3 sequential + 3/3 fan-out Confirmed** (~1.0 s build→broadcast→confirm each, ephemeral keys, no public side effects); public devnet (read-only, two independent runs) — p50 66–67 ms for all three calls, p95 67–289 ms with sporadic ~10.3 s outliers = shared-IP rate-limit retries (the bench reports percentiles and never asserts wall-clock bounds, so CI stays deterministic). `devnet_e2e.rs` re-run vs local validator: **4/4 incl. `executor_live`** — no regressions. Entire evidence set re-established from a clean toolchain + from-scratch rebuild (337/337, fmt, clippy, validator runs) after a mid-session sandbox re-provision. ⚠️ **NOT EXECUTED:** feed against a live Geyser provider (Yellowstone/Triton/Helius — none reachable from the sandbox; covered by protocol-faithful mocks), funded mainnet/devnet landing-rate at volume, fan-out across two real independent RPC providers. 🟡 **NOTED, NOT CHANGED:** `rpc.get_account` classifies any error containing the method name (incl. transport failures) as `Ok(None)` — pre-existing behaviour, documented in `latency_bench.rs`; devnet now serves version-1 txs in blocks — `maxSupportedTransactionVersion=0` stays for pump-era txs. |
| 4-iii, 7-8 | Postgres/Redis + restart-safe dedup · commercial layer · external audits | **NOT STARTED** | — | per BUILD PLAN. (§4 admin timelock/multisig delivered as 4-i/4-ii; genesis-gap still needs a contract change → user approval.) |

**Current verified green: 337 workspace + 44 staking (43 host + 1 validator e2e) = 381 tests, 0 failures**
(workspace baseline 262 → +2 `is_loopback` +6 state +2 config +30 observability +17 integration/e2e +18 latency/geyser (6 cache, 3 notification-decode, 2 copy geyser, 2 sniper geyser, 5 latency bench); staking 17 → +1 error-uniqueness +10 validation +7 admin-hardening +8 timelock/governance +1 on-chain validator lifecycle). Gated network evidence recorded separately: public devnet 2/4 executed + 2 designed skips (§3) and read-only latency percentiles (§5); local validator 4/4 incl. live confirm (§3, re-run clean after §5) + landing rate 6/6 sequential+fanout (§5).

**Net effect on the verdict:** the single 🔴 CRITICAL (contract validation) and
both 🟠 HIGH app-security issues (XSS, open control API) plus the 🟠 HIGH
memory-exhaustion issue are **fixed in code and covered by tests**, and the
contract's admin-centralisation blocker (Top-10 #9) is now **architecturally
addressed**: deposit-only pause that cannot trap funds, two-step admin
transfer, hard fee/reward caps, and a published parameter timelock with
permissionless apply — with the documented production setup being a
Squads/Realms multisig as `admin`. The contract **compiles to BPF and passes a
full on-chain lifecycle test** on `solana-test-validator` (BUILD PLAN §3), but
is still **NOT** deploy-ready: no external audit, and the 🟠 genesis-
distribution gap above must be designed out before any real deployment.
BUILD PLAN §6 (observability) is now **complete on the host**: the process
exposes liveness/readiness probes and a bounded-cardinality Prometheus surface
from the real execution paths (RPC retries, WS reconnects, event bus, state
counters, HTTP middleware), with request correlation IDs and structured
JSON/text logging driven by `[observability]` config — verified by 30 new
deterministic tests; scraping by an external Prometheus and orchestrator probe
behaviour are **NOT EXECUTED** in this sandbox.
Application half moves from "advanced prototype" toward "hardened MVP" — but
**live trading remains NOT VERIFIED** and **sub-1s landing is NOT achievable**
until the Geyser path (BUILD PLAN §5) is wired.
BUILD PLAN §3 ("prove it works") is now **complete to the extent the sandbox
allows**: every feed-facing module is integration-tested against protocol-
faithful local mocks (PumpPortal WS, Polymarket CLOB/Gamma HTTP incl. the
full L1/L2 auth + signed-order wire format), the storage layer is
restart- and corruption-tested, the execution engine is proven
paper → simulate → **confirmed live broadcast** against a real cluster
(local `solana-test-validator`; public devnet for the read-only/paper paths),
and Module 4 is compiled with `cargo build-sbf` and exercised end-to-end on
the BPF VM. The mocks also paid for themselves immediately: they exposed a
real wire-format bug in the live Polymarket order path (`PostOrderResponse`
camelCase) and a CI `build-sbf` pin/lockfile drift that would have failed the
program job. What §3 has **not** proven: behaviour against the real
third-party endpoints (rate limits, schema drift), a funded real-token swap,
and public-devnet broadcast — the first belongs to soak-testing in staging,
the latter two need explicit approval + funded keys.

---

## MASTER-DIRECTIVE EXECUTION (production-grade completion pass — 2026-09-17)

Status of every gate run in THIS environment (cargo 1.98.1, rustfmt/clippy
1.98.1, agave 2.1.21 tools, cargo-audit 0.22.2, cargo-deny 0.18.9):

| Gate | Result |
|---|---|
| `cargo fmt --all --check` (workspace + program) | **VERIFIED clean** |
| `cargo clippy --workspace --all-targets -- -D clippy::correctness` | **VERIFIED 0 errors** (style/deprecated warnings remain non-blocking per the tracked-TODO policy: 6 `solana_sdk` re-export deprecations + missing_docs backlog) |
| `cargo test --workspace` | **VERIFIED 404 passed / 0 failed** (incl. 8 db + 5 redis gated integration tests skipping cleanly without services) |
| `cargo test` (program, host) | **VERIFIED 48 passed / 0 failed** |
| `cargo build-sbf` (program, agave 2.1.21) | **VERIFIED — 166 272-byte .so, byte-size-identical to the pre-update build after the lockfile patch bumps (host-only deps)** |
| Validator e2e (`STAKING_E2E=1`, real BPF VM) | **VERIFIED 2/2 in 89.5 s** — governance lifecycle + funded money flow (genesis → stake → claim → unstake) |
| `cargo audit` (app + program, `.cargo/audit.toml`) | **VERIFIED exit 0** — 0 un-ignored vulnerabilities; 6 ignore IDs with written justification (upstream-pinned dalek chain via solana-keypair; no-patch-exists webpki 0.101.7 via solana-pubsub-client; phantom sqlx-mysql→rsa); 9 unmaintained/unsound warnings allowed by policy |
| `cargo deny check advisories bans sources licenses` | **VERIFIED all ok** — licenses now a BLOCKING gate (allow-list confirmed; added CDLA-Permissive-2.0 for webpki root-cert data; dropped never-encountered OpenSSL/Unicode-DFS-2016) |
| Devnet e2e (read-only + valueless live) | **VERIFIED 4/4** incl. a landed devnet self-transfer |
| Latency bench (public devnet, read-only) | **VERIFIED 5/5** — getSlot/getLatestBlockhash p50 66 ms, p95 72–274 ms (shared-IP rate-limit outliers up to 10.3 s, reported not asserted) |
| Landing-rate sequential-vs-fanout on public devnet | **NOT EXECUTED this pass** — faucet rate-limited (designed skip); previously VERIFIED on a local validator (3/3 + 3/3, §5) and one live devnet landing confirmed via devnet_e2e above |
| Docker image build / compose up | **BLOCKED in sandbox** (no docker daemon) — CI `docker` job builds the image and smoke-tests `/api/health`; `docker compose config -q` gate added to the app job |
| Live Geyser provider feed | **NOT EXECUTED** (no reachable provider) — mock-WS coverage + devnet wire-shape verification stand |
| Mainnet live trading | **NOT EXECUTED** (requires funded keys + explicit approval) |

### Delivered in this pass (all compiled + tested)

* **Persistence (§4-iii):** sqlx Postgres layer (5 migrations), repos for
  every durable entity, `PersistencePump` (events → DB materialization) +
  `JournalPump` (JSONL), startup `restore()` before modules spawn,
  Redis KV (locks/NX-TTL/INCR) as cache-only L2. **13 gated integration
  tests** execute against real Postgres 16 / Redis 7 in CI (service
  containers added) and skip cleanly offline.
* **Reconciliation:** `recon_queue` (SKIP LOCKED claim, exp backoff → 1 h
  cap, failed after exhaustion) + 3 truth sources in `recon.rs` (Solana tx
  confirm, Polymarket order status, Solana position drift FLAG — never
  auto-corrects).
* **OMS:** idempotent order intents (DB-backed), status history, recovery to
  `Unknown`, external-id/signature attach; `/api/orders` + `/api/orders/:id`.
* **RBAC:** role-bearing API keys (`[auth]` key_env principals, sha256
  digests only, runtime add/revoke by owner), per-IP rate limiting, audited
  denials; **Telegram roles** (owner/operator/readonly, backward compatible:
  no owner list ⇒ legacy allowlist keeps full control) with loud refusals —
  4 new tests.
* **Audit chain:** sha256 hash-chained append-only trail, `/api/audit` +
  `/api/audit/verify` (tamper detection tested via direct SQL in the gated
  suite).
* **Journal API:** `GET /api/journal` (sizes/paths or `available:false`) +
  `POST /api/journal` owner-only rotate.
* **Lifecycle:** 4-phase ordered shutdown (http-drain → module-drain →
  pump-flush → db-close) with deadlines; every module loop selects on the
  shutdown signal; fail-closed non-loopback bind.
* **Staking genesis (contract change, explicitly authorized):**
  `GenesisMint{amount}` — admin-only, ONE-TIME latch (`Config::genesis_done`),
  mints initial supply via config-PDA `mint_to`; errors 6026
  `GenesisAlreadyDone` / 6027 `InvalidAmount`; builder `genesis_mint_ix`.
  Closes the §5 KNOWN GAP: the funded stake→reward→claim→unstake flow is now
  exercised END-TO-END on the BPF VM (rewards proven to be minted — supply
  grows by exactly the payout; vault drains on unstake; replay + non-admin
  rejected). 5 new host tests + e2e test #2. ⚠️ `Config` borsh layout grew
  by 1 byte (`genesis_done`) — redeploy + re-initialize required for any
  existing deployment (none exists beyond test validators).
* **Deployment:** `docker-compose.yml` (bot + postgres:16 + redis:7,
  healthcheck-gated startup, loopback-published API, named volumes),
  `.env.template`, `.gitignore` (secrets/keys/journal excluded).
* **CI:** app job gains Postgres+Redis service containers (gated tests now
  EXECUTE in CI), `--test-threads=1`, `docker compose config` gate; new
  `docker` job (build + container health smoke test); program e2e pinned
  single-threaded; licenses gate now blocking.
* **Docs tree (new):** `docs/{ARCHITECTURE,API,SECURITY,DEPLOYMENT,
  OPERATIONS,MODULES,STAKING,TESTING}.md`; README restructured (docs index,
  compose quick start, genesis launch step, telegram roles, layout);
  `config.toml.example` + `[database] [redis] [auth]` sections + telegram
  role keys (parse-tested).
* **Supply chain:** app lockfile refreshed (`cargo update`: rustls 0.23.44
  RUSTSEC-2026-0285 fixed, stale entries re-resolved); program lockfile
  patched (quinn-proto 0.11.15 fixing RUSTSEC-2026-0185, time 0.3.47 fixing
  RUSTSEC-2026-0009) without touching the solana pins; `.cargo/audit.toml`
  (root + program) with per-ID justification; deny.toml ignore list +
  unmaintained scope documented. Full test suites re-run green AFTER the
  lockfile changes (404 ws + 48 prog + build-sbf + 2/2 e2e).

**Current verified green: 404 workspace + 50 program (48 host + 2 validator
e2e) = 454 tests, 0 failures.** Sandbox re-provisioning wiped the toolchain
mid-pass; everything above was re-verified from a clean toolchain install
afterwards (rustup minimal + agave tarball + fetched crates).

### Clippy `-D warnings` reconciliation (post-directive cleanup, 2026-09-17)

The master directive's final-verification gate requires `clippy -D warnings`;
the interim policy (row 1b above) had deferred the style/deprecation backlog
to a correctness-only gate. **The backlog is now fully cleared and the gate
upgraded** — CI runs `cargo clippy --workspace --all-targets -- -D warnings`
(app) and `cargo clippy --all-targets -- -D warnings` (program) as hard gates.

What was fixed (185 warnings → 0, behaviour-preserving):

* **Deprecations (11 app + 3 program + 1 e2e):** `solana_sdk::system_instruction`
  / `system_program` → `solana_system_interface::{instruction, program}` via
  aliased imports (crate already in both lockfiles transitively — no new
  code in the BPF object); `Keypair::from_bytes` → `Keypair::try_from(&[u8])`
  (same validation). Program **rebuilt with `cargo build-sbf`** (166 272-byte
  .so) and **validator e2e re-run 2/2 green (99.1 s)** after the swap.
* **missing_docs (96 items, module-polymarket + module-telegram):** real
  documentation written for every undocumented field/variant/static —
  Polymarket CLOB/Gamma/EIP-712 wire structs (incl. the V2 `Order` 11-field
  semantics: maker/taker amounts per side, signature types 0–3, GTD expiry
  carried in `timestamp` per this implementation's mapping) and the Telegram
  Bot API DTOs + `Command`/`TgRole` enums. One doc claim was corrected
  against the code during writing (GTD expiry lives in `timestamp`, not
  `metadata` — `orders.rs` maps `expiration_timestamp` there).
* **Mechanical lints (~74):** 40 machine-applicable fixes via `cargo clippy
  --fix` (useless conversions, clone-on-copy, redundant closures,
  manual saturating arithmetic, derivable Default impls, map_or/and_then
  simplifications…); the rest by hand: NaN-explicit `matches!(partial_cmp…)`
  rewrites (maths/risk — semantics preserved: NaN still rejects),
  `sort_by_key(Reverse(…))`, struct-literal test configs, merged identical
  `if` arms in the sniper exit-status mapping, format-in-format flattening,
  `clamp` for the position-fraction cap (NaN unreachable via TOML),
  `PrebuiltCache::is_empty`, `ClobClient::chain_id()` getter (field was
  dead), deleted an unused `err()` helper.
* **Justified `#[allow]`s (4, each with a written reason in-code):**
  `too_many_arguments` ×2 (instruction-builder convention; test fixture),
  `result_large_err` ×2 (axum `Response` denial pattern; solana `ClientError`
  in the e2e helper).
* **Program `unexpected_cfgs`:** `[lints.rust] check-cfg` declarations for
  the `entrypoint!` macro's `custom-heap`/`custom-panic`/`target_os="solana"`
  gates (lint stays active for everything else).

Verification after the cleanup (all re-run, this environment):
`cargo fmt --all --check` clean (both workspaces) · `clippy -D warnings`
exit 0 (both) · **404/404 workspace tests** · **48/48 program host tests** ·
`build-sbf` OK · **validator e2e 2/2** (single-threaded; a parallel-threads
run of the same binary failed on resource contention — two validators +
disk exhaustion — which is why the suite is pinned `--test-threads=1`).

Security re-verification after the dependency change (`solana-system-interface`
became a direct program dep; both lockfiles re-resolved), replicating the CI
security job exactly: `cargo audit` **exit 0 on both lockfiles** (9 allowed
warnings each — the documented ignore sets; one non-ignored *warning-level*
advisory remains, RUSTSEC-2026-0097 `rand 0.7.3` unsound-with-custom-logger,
transitive via the solana-sdk 2.1 family — not applicable here, no custom
rand logger exists in either workspace, and cargo-audit treats unsound as
non-blocking warn). `cargo deny check advisories bans sources` and
`cargo deny check licenses` both **exit 0** (duplicate-version entries are
`multiple-versions = "warn"` by policy). `cargo build --workspace
--all-targets` **exit 0 / 0 warnings**. Every CI step is now either locally
re-executed green or blocked only by sandbox environment (docker job — no
daemon; covered by CI).

### Adversarial audit: "no module may bypass global risk" (2026-09-17)

Trace of **every money-moving call site** in the workspace (grep-complete:
`Executor::run` ×4, `post_order` ×1; no other callers exist — `api.rs:136`
`next.run` is tower middleware, `main.rs` runs are loops/workers):

| # | Path | Gates proven (by source inspection) |
|---|------|--------------------------------------|
| 1 | Sniper entry (`entry.rs:221`) | `check_launch_with_lists` → `check_entry` → reject honored (`inc_risk_rejected` + `RiskRejected` event + early return) → execute |
| 2 | Copy mirror (`mirror.rs:287`) | `check_copy` (itself calls `preflight(Copy)`) → `check_entry` → reject honored → execute |
| 3 | Polymarket order (`lib.rs:323` → `submit_live` → `post_order:447`) | `check_entry` → reject honored (event + return) → paper fill **or** live submit; risk applies in paper mode too |
| 4 | Sniper exit (`exit.rs:341`) | `check_exit` — kill-switch flatten is **rule #1**; exits deliberately NOT preflight-gated (risk-reducing must never be blocked by kill/disable/loss-latch) |
| 5 | Copy exit (`exit.rs:330`) | same as #4 |

Global gate contents (`risk.rs:189 preflight`, re-read live on every check):
kill switch → module enabled → `loss_limit_tripped` latch → live daily-loss
recompute. `check_entry` adds: NaN/negative/size sanity, slippage cap,
max open positions, duplicate symbol, sizing caps.

Bypass vectors specifically probed and closed:
* **API**: no order-placement endpoints exist (reads + kill/resume/mode/
  enable/disable only, RBAC-gated — `Mode("live")` requires Owner).
* **Telegram**: no trading commands in the `Command` enum; RBAC hierarchy.
* **Re-enable after daily-loss trip**: `enable_module` flips `enabled`, but
  preflight still rejects on the `loss_limit_tripped` latch — the only clear
  paths are the UTC rollover (`daily_stats` resets on day change, verified)
  or `state::clear_loss_limit`, which has **zero control-plane callers**
  (tests only).
* **Sweeper starvation**: exit sweepers have no `is_enabled` gate, so
  disabling a module cannot strand open positions; under kill the sweeper
  skips mark-fetch and flattens at any price.
* **Hot config**: `risk_config().await` is read inside every check — TOML
  reloads take effect immediately, no stale-config window.

**Result: no bypass found; the directive claim holds by construction.**

### Adversarial audit: staking program processor (2026-09-17, source-level)

Fresh-eyes pass over all 11 instruction handlers (`processor.rs`, 2 126 lines)
against the classic Solana vulnerability classes. **No vulnerabilities found;
no code changes required.** Evidence:

* **Account validation** — every handler loads config via `load_config`
  (PDA address + program owner + non-empty + `initialized`); stake accounts
  must be the `stake_pda(program, staker)` address, program-owned, with
  `owner == staker`; `require_staker_token` unpacks the SPL account and
  enforces token-program owner + config-mint + staker ownership, so deposits
  can only come from, and withdrawals only go to, the staker's own account
  of the right mint.
* **Address pinning** — vault/treasury/mint are checked against the
  on-chain config (which itself is a validated PDA); token/system program
  IDs pinned to the real IDs.
* **Signer/authority** — `staker`, `admin`, `pending_admin` all
  `require_signer`; vault transfers and reward/genesis mints are
  `invoke_signed` by the config PDA with the canonical seeds+bump.
* **Arithmetic** — checked add/sub at every balance mutation; reward math
  in u128 with `elapsed <= 0` short-circuit (clock-regression safe) and
  round-down (favours the vault). The saturating `u64::MAX` overflow branch
  is provably unreachable: with `reward_rate_bps <= MAX_REWARD_RATE_BPS`
  (10 000) and supply-bounded amounts it would require
  `elapsed > i64::MAX`. Release profile keeps `overflow-checks = true`.
* **Replay/re-init** — `initialize` rejects when already initialized
  (`:348`, e2e-proven); `apply_update` and `accept_admin` clear their
  pending slots after use; genesis mint is one-shot via `genesis_done`.
* **Permissionless surface** — only `apply_update`, gated by active
  pending + `saturating_add` timelock (old-delay semantics, OZ
  TimelockController rule) + cap re-validation at apply time.
* **Fund-trap resistance** — pause gates deposits only; claim ignores the
  unstake cooldown; a pre-created account squatting the stake PDA makes
  `create_account` fail atomically (no partial state).
* **Panic safety** — `save_config` bounds-checks instead of panicking;
  stake-account writes are size-guaranteed by a successful fixed-size
  borsh deserialize precondition.

Noted, not defects: `GenesisMint` checks only `is_writable` on the
recipient — mint-membership is enforced downstream by spl-token's `mint_to`
(mismatch fails the CPI); recipient choice is admin-trusted by design.

### Prompt 1 — Enterprise signer abstraction + key-custody foundation (2026-09-17)

Objective: remove the direct wallet-keypair dependency from the transaction
layer, make `extra_signers` real, and establish the Vault/KMS/HSM extension
boundary — without changing trading behaviour.

**New file:** `crates/solana-kit/src/signer.rs` — `TransactionSigner` trait
(async `sign_message` / `sign_versioned_message`, `pubkey`; `Debug` as a
secret-free supertrait), `LocalKeypairSigner` (wraps the existing `Wallet`
loading path; redacted `Debug`), `SignerRegistry` (named identities,
deterministic lookup, duplicate rejection, `find_by_pubkey`, no default
fallback), `build_signer_registry` (startup validation: unsupported provider
→ hard `UnsupportedBackend` error, never a silent Local fallback; every
configured identity must resolve). Identity constants: `primary_trading`
(always the loaded wallet) + conventional `sniper` / `copy_trading` /
`treasury` / `staking_admin`. 16 tests.

**Extra-signer fix (`tx.rs`):** the builder no longer *warns* about
`extra_signers` — the compiled message's required-signer set must exactly
equal {wallet} ∪ dedup(`extra_signers`); every extra must be resolvable via
the registry; signatures are collected in message order (wallet local,
others through the abstraction). Structured failures: `ExtraSignerNotRequired`,
`SignerMismatch` (undeclared required signer), `MissingSigner` (unresolvable),
`SigningFailed` (backend error, label-annotated). New `required_signer_keys`
helper (v0 + legacy). Wallet-only transactions assemble exactly as before
(same message bytes, same signature order). 9 new tests incl. a mock
"outage" signer and a 3-signer build with per-index signature verification.

**Wallet boundary (`tokens.rs`):** `keypair()` accessor **removed**;
`sign_message_sync` is now the single local signing choke point; `Wallet`
implements `TransactionSigner`; hand-written `Debug` (pubkey + source only).
`jupiter.rs` limit-order signing routed through the choke point; its stray
`Signer` import moved into tests.

**Wiring:** `TxBuilder::with_registry`; `Executor::with_signer_registry`
(`Executor::new` signature unchanged); `Sniper::new` / `CopyBot::new` /
`ExitSweeper::new` take `Option<Arc<SignerRegistry>>` and thread it into
every executor (entry + sweeper paths); `main.rs` builds + validates the
registry right after wallet load and logs identity→pubkey pairs (public
information only). Polymarket EVM signing deliberately untouched — the two
signing models are not merged.

**Config boundary (`config.rs`):** `[signing] provider = local|vault|kms|hsm`
(only `local` implemented; others parse but fail startup) +
`[[signing.identities]] {name, alias|keypair_env|keypair_path}` with
validation: non-empty unique names, `primary_trading` reserved, exactly one
source, aliases may only reference earlier identities (deterministic
registration order). `SIGNING_PROVIDER` env override. `config.toml.example`
+ `.env.template` documented. 7 new tests.

**Error model (`error.rs`):** structured `SignerError` (10 variants:
NotFound, DuplicateIdentity, MissingSigner, ExtraSignerNotRequired,
SignerMismatch, SigningFailed, InvalidSigner, UnsupportedBackend, SecretLoad,
UnsafeConfiguration) wired as `BotError::Signer` (`#[from]`, alertable).
All variants secret-free by construction; `from_spec` errors verified by
test to not echo the spec.

**Redaction (G):** `SecretConfig` derived `Debug` **replaced** with a
hand-written `<set>`/`<unset>` impl (closes the latent `{:?}`-on-Config leak
through `AppConfig`/journal paths); `/api/config`, config-version recording,
health, metrics and Telegram verified secret-free (pre-existing + re-audited:
`/api/wallets` is the copy-tracking address registry, no key material).

**Verification (executed):** `cargo fmt --all --check` clean · `cargo clippy
--workspace --all-targets -- -D warnings` exit 0 · `cargo test --workspace`
**436 passed / 0 failed** (404 → 436; +32 new tests, none removed). Program
crate untouched by this change. Docker/CI unchanged.

**Post-Prompt-1 live re-verification (same day, gap #5 closed):** after
re-downloading the agave 2.1.21 toolchain, all live legs re-executed against
the NEW signing path: `devnet_e2e` vs local validator **4/4** (incl.
`executor_live` — build→sign→broadcast→`Confirmed` in 2.05 s), `latency_bench`
**5/5** (incl. landing-rate sequential + fan-out legs through the refactored
`TxBuilder`), staking `validator_e2e` **2/2** (97.4 s, artifact `.so` proven
loadable on-chain), security gates re-run post-dependency-change: `cargo
audit` app+program **exit 0**, `cargo deny` advisories/bans/sources +
licenses **exit 0**. Honest incident log: the first two attempts hit sandbox
resource limits — attempt 1 ran the validator concurrently with a 24-minute
test-binary rebuild (OOM kill → 2 transport-error failures while
`executor_live` still passed); attempt 2 started tests before fee
stabilization (1 flaky `executor_live` failure) and a solo re-run against a
stalled validator hung the sandbox into OOM thrash (~8 min recovery). The
clean final runs above were against a fully stabilized validator with
prebuilt binaries — the failures were environmental, not signer-code
regressions (evidence: identical code passed both when the validator was
healthy and in the 436-test workspace run).

**Remaining gaps after Prompt 1:** Vault/KMS/HSM backends are configuration +
trait boundaries only (deliberately not implemented — no fake backends);
remote-signer latency/timeout/retry policies land with the first real
backend; `extra_signers` has no production caller yet (sniper/copy/poly flows
are single-signer today — the capability is proven by tests, not yet
exercised by a live multi-sig flow); per-identity key rotation is manual
(restart); no hardware-backed integration test possible in this sandbox.

## PROMPT 2 — ON-CHAIN RECONCILIATION & CRASH RECOVERY (2026-09-17)

**Objective:** make the chain/venue the final source of truth for money
movement: every ambiguous execution (timeout, transport failure, crash
between broadcast and persistence) becomes a durable claim that a
reconciliation worker resolves against external truth before the affected
module may trade again; positions and PnL converge deterministically after
any crash point; the same logical execution can never double-trade.

**New code:**
* `crates/core/src/reconciliation.rs` — pure comparison engine: typed
  `ReconOutcome` (13 verdicts incl. InSync/ExternalAhead/LocalAhead/
  QuantityMismatch/MissingPosition/UnexpectedPosition/UnknownExecution/
  MissingTransaction/DuplicateExecution/StaleLocalState/
  ExternalStateUnavailable/RecoveryRequired), `compare_position` (tolerance
  + dust + in-flight guard), `classify_execution` (local×external decision
  matrix), `reconstruct_pnl` (average-cost replay, defensive against
  over-sell/garbage), `ExternalState` (observed vs UNREADABLE — never
  conflated). 19 unit tests.
* `crates/core/migrations/0006_transaction_attribution.sql` — additive,
  restart-safe: `transactions.signer/venue/attempts` + signer index.
* `crates/solana-kit/tests/recon_crash_e2e.rs` — §W validator-gated proof:
  intent→persist→broadcast→crash-before-state-update→restart→chain-truth
  discovery→converged Filled, EXACTLY ONE on-chain transfer, retry
  collapses onto the terminal order; plus transport-black-hole →
  `SendUnknown` (never a definite failure), inconclusive classification,
  zero lamports moved.
* `docs/RECONCILIATION.md` — full model: state boundaries, claim lifecycle,
  outcome matrix, the 8 §F ambiguity cases, crash points A–K table, startup
  sequence, RPC/commitment behaviour, dedup layers, position/PnL policy,
  Redis/Postgres authority, metrics/alert catalogue, honest limitations.

**Modified code (extend, never compete):**
* `execute.rs` — `ExecStatus::SendUnknown`; `classify_send_error`
  (conservative: only node-produced rejections are definite); ambiguous
  sends fall through to on-chain confirmation of the ALREADY-SIGNED tx
  instead of blind retry; `succeeded()` includes SendUnknown;
  `ExecutionResult.attempts` provenance; 3 new tests (2 with mock
  endpoints).
* `oms.rs` — `bot_duplicate_execution_prevented_total{where}` on both
  idempotency-hit paths (+ test).
* `recovery.rs` — `startup_reconcile` gate (window+batch, shutdown-aware,
  unresolved report), `run(&self)`, per-kind duration histogram.
* `db/repo.rs` — `record_submitted(chain,…,signer,venue,attempts)`,
  `TransactionRepo::get_status`, `TradeRepo::list_for_position`,
  `ReconRepo::{is_active,reopen_resolved,unresolved_counts(active_only)}`.
* `state.rs` — `recon_unresolved` snapshot (+ `Summary` field → `/api/status`,
  WS hello and Telegram `/status` automatically).
* `config.rs`/`config.toml.example`/`.env.template` — `[recovery]`
  (startup_reconcile_secs=30, startup_batch=64,
  block_modules_on_unresolved=true, position_recheck_interval_secs=300)
  + env overrides.
* `server/recon.rs` — adapters rewired through the engine: TxTruth keeps
  slot/fee capture, distinguishes unreadable-RPC from not-found, parks
  DB-says-success/chain-says-failure as `RecoveryRequired` (never silently
  flipped); PositionTruth uses the aggregated identity-validated reader,
  in-flight-claim guard, typed outcomes, and ONE deterministic correction
  (chain-zero + confirmed exit fill → close with PnL RECOMPUTED from
  persisted fills, audited); everything else flags/parks. Outcome +
  read-error metrics.
* `server/persist.rs` — Polymarket OrderSent branch (claim kind
  `polymarket_order`, chain `polymarket`); exit/fill signatures now always
  claimed (previously exits published Fill without any tx claim); signer +
  attempts attribution.
* `server/main.rs` — startup order is now restore → worker build →
  `startup_reconcile` GATE → block affected modules (kind→module map,
  audited `denied` records + Error events) → worker loop + 60 s backlog
  sampler + periodic position recheck ticker → modules. 3 gate tests.
* `tokens.rs` — `token_balances_for_owner`: multi-account aggregation with
  per-account mint/owner identity validation, jsonParsed AND raw-base64
  wire shapes, chain decimals; `Ok(zero)` vs `Err` contract documented.
* `module-polymarket` — `PolyError::SubmitUnknown`; `derived_order_id()`
  (CLOB orderID = local EIP-712 struct hash); submit-unknown publishes
  OrderSent with the derived id (claim survives restart); DETERMINISTIC
  salt from intent semantics → re-signed identical intent = same order id =
  venue-level dedup (+ test).
* `module-sniper`/`module-copy` — SendUnknown rides the filled/claim path
  (signature never dropped); OrderSent carries `signer` + `attempts`.
* `events.rs` — `OrderSent{signer,attempts}` (serde-default, wire-additive).
* `docs/TESTING.md`, `README.md` — counts + pointers.

**Verification (executed, this sandbox):** `cargo fmt --check` clean ·
`cargo clippy --workspace --all-targets -- -D warnings` exit 0 (re-run
after disk-pressure incident) · `cargo test --workspace` **467 passed /
0 failed** (436 → 467; +31 net new, none removed) · `cargo audit` app +
program exit 0 (only the known warning-level RUSTSEC-2026-0097 rand 0.7.3,
config-allowed) · `cargo deny check advisories bans licenses sources`
exit 0 · live legs vs local solana-test-validator (agave 2.1.21):
**`recon_crash_e2e` 2/2** (16.6 s — crash→restart→convergence with
exactly-one-transfer assertion, and the ambiguity proof), `devnet_e2e`
**4/4** (4.2 s), `latency_bench` **5/5** (9.4 s), staking `validator_e2e`
**2/2** (114 s, against the byte-identical Prompt-1 `.so` — program
untouched by this prompt). DB/Redis-gated suites skip offline by design and
execute in CI (Postgres 16 / Redis 7 services); the 3 new db_integration
tests are NOT EXECUTED here (no Postgres in sandbox) — their repo SQL
follows the exact patterns of the executed ones.

**Honest incident log:** the sandbox re-provisioned mid-prompt (5th time;
toolchain + target dir lost, workspace files intact — one repo method lost
to a snapshot race was detected by the compiler and re-applied); one test
run started while the validator was up and a rebuild kicked off
concurrently → OOM thrash (~5 min recovery; the known
never-compile-with-validator lesson); the first crash-e2e attempt
legitimately failed on rent-exemption (1000-lamport transfer to a fresh
account) — fixed to 1 000 000 lamports and re-run green; disk filled twice
from test-binary bloat (cleaned per the established >20 MB-binary purge).

**Remaining gaps after Prompt 2:** crash point C with TOTAL event loss
(dies between broadcast and any durable trace) is only discoverable via the
position recheck → operator queue, not auto-corrected (needs pre-signing
intent journaling; deliberate latency trade-off, documented §13);
Polymarket reconciliation is order-status-based — no on-chain CTF balance
reader (matched-but-fill-event-missed surfaces as Filled order without
position, not auto-created); startup gate blocks whole modules, not
per-symbol subsets; `transactions.attempts` is per-publishing-replica
(cross-replica retry truth lives in `reconciliation_state.attempts`);
drift correction stays intentionally narrow (only the
zero-balance-with-exit-fill case); Telegram surfaces the reconciliation
backlog read-only (no command may mutate claims — by design).

---

## Gap Closure — 2026-09-17 (same session, post-Prompt-2): gated integration suites REALLY EXECUTED

**Context.** The Prompt-2 report marked db_integration (11 tests) and redis_integration (5 tests)
NOT EXECUTED (no Postgres/Redis available offline; repo never pushed so CI never ran them).
This pass provisioned real servers inside the sandbox and executed both suites — which exposed
FOUR latent pre-existing bugs in never-executed code paths. All four are now FIXED and verified.

**Infrastructure provisioned (sandbox-local, not part of the repo):**
- PostgreSQL 16.4.0 from the zonky embedded-postgres-binaries jar (Maven Central) →
  initdb + pg_ctl on 127.0.0.1:5433, trust auth, fsync off. Migrations 0001–0006 apply cleanly.
- Redis 7.2.10 compiled from source (download.redis.io, `make redis-server redis-cli`) →
  127.0.0.1:6379, persistence off.

**Latent bugs found and fixed (all pre-existing, none introduced by Prompt 2):**
1. `ReconRepo::claim_due` (crates/core/src/db/repo.rs): the claim UPDATE set
   `status='in_progress', attempts+1` WITHOUT advancing `next_attempt_at`, so a second
   sequential claim immediately re-claimed the same in-progress row (attempts inflated;
   give-up after 2 attempts could double-count). FIX: the claim now leases the row
   (`next_attempt_at = now() + interval '60 seconds'`); `fail()` still overwrites with the
   real backoff, and lease expiry re-enables claims whose worker crashed.
2. `OrderManager::recover_from_db` (crates/core/src/oms.rs): skipped rows already present in
   the in-memory mirror. A duplicate-create after restart re-inserts the stale non-terminal DB
   row into the mirror, so recovery then skipped exactly the orders that most needed it
   (test: recovered==0 / status stayed Submitted). FIX: transition ALL non-terminal DB rows to
   Unknown and overwrite the mirror (list_incomplete returns only non-terminal rows; terminal
   orders are immutable and never listed; runs before modules spawn).
3. `AuditRepo::append` (crates/core/src/db/repo.rs): hashed `Utc::now()` at ns precision, but
   the timestamptz column truncates to µs — on clocks with ns granularity the recomputed hash
   never matched and `verify_chain` reported the chain broken at the FIRST row even with zero
   tampering. FIX: canonicalize ts to microseconds
   (`DateTime::from_timestamp_micros(ts.timestamp_micros())`) before hashing AND storing, so
   hash input == stored value on any clock.
4. `RedisKv::incr_expire` (crates/core/src/redis_kv.rs): the atomic pipeline returns one reply
   per command (INCR, EXPIRE) but destructured a 1-tuple → redis TypeError
   "Array response of wrong dimension" on every call. FIX: destructure `(u64, i64)`.

**Test-harness fix (test-only):** `audit_chain_verifies_and_detects_tampering` deliberately
corrupts a row, which poisons the GLOBAL chain for any later run against the same database
(CI is unaffected — fresh container per job). The test now deletes its own rows afterwards and
asserts the chain is intact again. Direct SQL only; the app still cannot mutate audit rows.

**Executed results (evidence, not claims):**
- `POSTGRES_URL=postgres://postgres@127.0.0.1:5433/postgres cargo test -p bot-core --test
  db_integration -- --test-threads=1` → **11 passed / 0 failed** on a FRESH cluster, and
  **11 passed / 0 failed** on a SECOND run against the same cluster (cross-run isolation).
  First-ever real run before the fixes: 8 passed / 3 failed (the latent bugs above).
- `REDIS_URL=redis://127.0.0.1:6379 cargo test -p bot-core --test redis_integration
  -- --test-threads=1` → **5 passed / 0 failed** (first run before fix 4: 4/1).
- Regression gate after the four source fixes: `cargo fmt --all --check` clean;
  `cargo clippy --workspace --all-targets -- -D warnings` exit 0;
  `cargo test --workspace` → **467 passed / 0 failed across 25 suites**;
  `cargo test -p bot-core --lib` → 104/104.

**Honest limits of this evidence:** sandbox-local servers with durability off (fsync=off,
redis save off) — proof of SQL/protocol/logic correctness, NOT of crash-durability under real
disk failure (that class is covered by the validator e2e + the reconciliation design, and by
CI's service containers with default durability). The pre-fix tampered rows in one local
cluster correctly made later verify_chain runs fail — tamper evidence is permanent by design.

---

## Gap Closure II — 2026-09-17: the six documented "remaining gaps" — five IMPLEMENTED, one confirmed by-design

The Prompt-2 entry listed six remaining gaps. Directive: "check missing add don't skip".
A full sweep (todo!/unimplemented!/stub grep: clean; config/env/docs cross-check: clean)
confirmed the six gaps were the only MISSING items. Status after this pass:

**1. Crash point C — write-ahead intent journal: IMPLEMENTED.**
- `crates/core/migrations/0007_intent_journal.sql` — `execution_intents` table (pending →
  submitted/abandoned, partial index on pending).
- `crates/core/migrations/0008_intent_claim_kind.sql` — extends the `reconciliation_state`
  kind CHECK with `'intent'` (strict superset; restart-safe revalidation).
- `IntentRepo` (record idempotent / link / abandon / get / list_orphaned) in `db/repo.rs`.
- `IntentSink` trait + `with_intent()` wrapper + `sweep_orphan_intents()` in `recovery.rs`.
- ALL EIGHT Solana broadcast sites wrapped (sniper pump-buy, sniper Jupiter-buy, sniper
  pump-sell, sniper Jupiter-sell, copy pump-buy, copy Jupiter-buy, copy pump-sell, copy
  Jupiter-sell) via `ExecutionResult::broadcast_signature()` (paper/empty → abandon, never
  a phantom link). Server injects `DbIntentSink` when `[recovery] intent_journal` (default
  ON, env `RECOVERY_INTENT_JOURNAL`); journal write failures are logged+metered
  (`bot_intent_journal_errors_total`) and never fail a trade.
- `IntentTruth` (kind `intent`): pending orphan → Retry (late link can still land) → parks
  for operators after max attempts; ambiguous forever by design — NEVER resubmitted.
- Startup: orphan sweep (30 s age) runs BEFORE the gate so orphans gate their symbols;
  60 s sampler re-sweeps (120 s age) at runtime.
- Polymarket intentionally excluded: deterministic salt already makes its submission
  idempotent at the venue (documented in RECONCILIATION.md §6).

**2. Per-symbol gating: IMPLEMENTED.** `AppState` gains a blocked-symbols set
(block/unblock/set/is_blocked + `Summary.blocked_symbols`); startup gate now attributes
each active claim to a symbol (`symbol_for_claim`: intent→journaled symbol, position→its
symbol, transaction/polymarket_order→via the attributed order row) and entry-gates ONLY
that symbol; unattributable claims (e.g. `balance:<addr>`) keep the conservative
module-wide block. Entries check the gate in sniper (`consider_launch`), copy (mirror
buy) and polymarket (`act_on_decision`); exits/sells are NEVER gated. Meter:
`bot_symbol_gated_entries_total{module}`. The 60 s sampler recomputes the set, so
resolved claims unblock automatically.

**3. Cross-replica `transactions.attempts`: IMPLEMENTED.** `record_submitted` ON CONFLICT
now MAXes `attempts` and COALESCEs missing attribution while `status='submitted'`;
terminal rows are immutable (§Y). Return value still means "first recording" (`xmax = 0`).

**4. Polymarket CTF balance reader: IMPLEMENTED.** `crates/module-polymarket/src/ctf.rs`
— ERC-1155 `balanceOf(address,uint256)` via `eth_call` on Polygon
(`[polymarket].ctf_rpc_url`, default `https://polygon-rpc.com`, empty disables);
77-digit decimal token-id → u256 encoding, no-truncation decode rule, errors are
"could not read", never zero (§O). `PolyBot::ctf_balance()` uses funder_address (or EOA).
`PolymarketOrderTruth` on `matched`+no-local-position verifies settlement on-chain and
flags `poly_settled_no_position` with balance evidence (risk event + Error alert).
Position auto-creation deliberately NOT done: cost basis must come from fills (§Y).

**5. Widened drift correction: IMPLEMENTED.** Beyond the zero-balance-with-exit-fill
case, `LocalAhead/ExternalAhead/QuantityMismatch/BalanceMismatch` now adopt the on-chain
quantity ONLY when `reconstruct_pnl` over durable fills independently reproduces it
within tolerance — deterministic, fill-justified, auditable (`recon_correction` flag);
everything else still flags without rewriting.

**6. Telegram reconciliation control: CONFIRMED BY-DESIGN, visibility improved.** No
command may mutate claims (§Y). `/status` now shows the reconciliation backlog per kind
and the entry-gated symbol list (read-only).

**Executed verification (all green):**
- `cargo fmt --all --check` clean; `cargo clippy --workspace --all-targets -- -D warnings` exit 0.
- `cargo test --workspace`: **481/481** (was 467; +5 core, +2 db, +6 polymarket CTF,
  +1 solana-kit, and the rest renumbered suites unchanged).
- db_integration vs real PostgreSQL 16.4: **13/13** on a fresh cluster AND two reruns on
  the same cluster (includes the new intent-lifecycle and cross-replica-attempts tests;
  migration 0008 applied over live data).
- redis_integration vs real Redis 7.2.10: **5/5**. `cargo audit` exit 0; `cargo deny` exit 0.
- Test-spec update (not a weakening): `recon_attribution_and_queue_lifecycle_work` now
  asserts the NEW documented semantics (attempts MAX to 9, signer attribution immutable).

**Remaining honest limits:** intent INSERT adds sub-ms latency per Solana execution
(disableable); orphan intents can never auto-resolve to Filled/Failed (no signature
exists) — they park for operators with the symbol gated; CTF check verifies settlement
but does not fabricate positions; symbol attribution requires the order row. Details in
docs/RECONCILIATION.md §13.

---

## Prompt 3 — 2026-09-17: distributed execution ownership (multiple concurrent replicas)

Directive: make the platform safe for MULTIPLE concurrent replicas — one logical execution
intent → at most one single active logical execution owner → at most one money-moving
submission until external state proves the outcome. Full reference: `docs/DISTRIBUTED.md`.

**§A audit (18 items, source-inspected before any change):** no distributed claims existed on
any entry/exit path (the core gap); kill switch + module enables were process-local; copy
cooldowns (`mark_copied`, `last_exit_at`) were process-local; launch dedup already routed
through the persistent facade; OMS `idempotency_key` was per-replica (duplicate orders
possible without claims); `claim_due FOR UPDATE SKIP LOCKED`, the audit chain and
`record_submitted` were already replica-safe; Telegram has NO money-moving commands (no
change needed — kill/enable propagate via flag sync); Redis locks existed but were unused in
execution paths; Redis dedup failure was (correctly) fail-open — ownership must be the
opposite.

**Implemented (all files complete, compiled, tested):**
- `crates/core/src/ownership.rs` (NEW) — claim state machine (`claimed → released |
  handed_off`, implicit `expired`, epoch-fenced takeover), `ClaimStore` trait,
  `OwnershipRegistry` (fail-closed), `ClaimGuard` (fence / bounded renew / `run_guarded`
  ticker / release / hand_off / complete), `Permit` module glue (Unmanaged | Owned | Lost),
  `MemoryClaimStore` with injected clock (deterministic expiry tests, no sleeps),
  `RuntimeFlagsWriter/Reader` + `MemoryFlags`, low-cardinality metrics
  (`bot_distributed_claim_*`, `bot_distributed_fencing_rejected_total`).
- `crates/core/migrations/0009_execution_claims.sql` — authoritative claim table
  (execution_id PK, owner_id, claim_epoch, status CHECK, claimed_at, lease_until,
  last_heartbeat, takeover_count, previous_owner, updated_at; never deleted; partial
  indexes). `0010_runtime_flags.sql` — flag/enabled/reason/updated_by/updated_at.
- `crates/core/src/db/claims.rs` (NEW) — `PostgresClaimStore`: acquisition is ONE atomic
  `INSERT … ON CONFLICT DO UPDATE … WHERE (expired | released | grace elapsed) RETURNING` +
  `prev` CTE (takeover classification without a second round trip); renew/verify/release are
  CAS on (owner, epoch, claimed[, unexpired]); expired leases never resurrect via renew.
  `PostgresFlags` upsert/read.
- `crates/core/src/redis_ownership.rs` (NEW) — `RedisClaimStore` over `own:claim:{id}`
  hashes (§U namespace): CLAIM/RENEW/VERIFY/RELEASE as single Lua scripts, ALL timestamps
  from `redis.call('TIME')` (clock-skew safe), terminal hashes expire after 7 days;
  `RedisFlags` over `own:flag:*` (SCAN, never KEYS). Redis is lease-only — Postgres stays
  authoritative whenever configured (durability rule respected: no money state moved into
  Redis).
- `crates/core/src/state.rs` — stable replica id (§C: `[ha].replica_id` else
  `{hostname}-{pid}-{rand8}`, exposed via `Summary.replica_id`); `flags_touched` +
  `attach_flags_writer` + publish hooks inside `set_kill_switch`/`set_enabled`/
  `emergency_stop`; `apply_remote_kill`/`apply_remote_enabled` (never echo back);
  `apply_flag_sync` staleness rules (kill/halt ON immediate; OFF/flags only when the shared
  row is newer than the last local decision); the emergency-halt latch (`halted`) also
  propagates — `emergency_stop` publishes it and `clear_halt` (/resume) releases it
  cluster-wide, so a resume served by any replica clears every replica; `merge_positions` book-sync rules (insert unknown;
  overwrite only when DB row newer AND local non-terminal).
- `crates/core/src/config.rs` — `[ha]` block (`HaConfig`: replica_id, claim_lease_secs=45,
  claim_handoff_grace_secs=900, flag_sync_secs=5, book_sync_secs=30) with `HA_*` env
  overrides and floors; `config.toml.example` + `.env.template` updated.
- `crates/core/src/error.rs` — `ClaimRejected` / `OwnershipUnavailable` (+ constructors).
- `crates/solana-kit/src/execute.rs` — `ExecStatus::is_ambiguous()` (Sent | SendUnknown).
- **Module integration (§F/§P, claim → fence → intent → broadcast → release/hand-off):**
  sniper entry `snipe:{mint}` (after risk, both curve + Jupiter paths, fence before
  `with_intent`); sniper exits `exit:{position.id}:{rule}` per sell decision (sweeper);
  copy entries `copy:{wallet}:{mint}` (cross-replica whale dedup, §H); copy exits (sweeper +
  whale mirror-exit `…:mirror_exit`); polymarket `poly:entry:{token_id}` (POSTed order →
  hand_off: the order may rest on the book, grace ≫ book-sync; `SubmitUnknown` → hand_off;
  definite rejection → release). Jupiter paths now derive `Confirmed` only from an OBSERVED
  `ConfirmOutcome::Confirmed` (unconfirmed broadcasts are honestly `Sent`/ambiguous).
  Telegram unchanged (no money-moving commands — verified in §A).
- `crates/server/src/main.rs` — store selection with logged precedence Postgres > Redis >
  Memory (loud warnings: memory store = run exactly one replica; live + memory = "WILL
  double-execute"); registry on the replica id; flags writer attached to `AppState`;
  runtime-flag sync task (`flag_sync_secs`, keeps local view on store failure, metered);
  position-book sync task (`book_sync_secs`, `list_open` → `merge_positions`); both stop on
  the shutdown coordinator (§O); `bot_replica_info` gauge; ownership injected into all three
  trading modules via `with_ownership`.
- Tests: **+33** (bot-core units 109→127: ownership state machine ×14 incl. fault-injection
  fail-closed §Y, state flag/merge/replica ×4; db_integration 13→19; redis_integration
  5→10; NEW `distributed_integration.rs` ×4 = §X two-context shared PG+Redis).
- Docs: `docs/DISTRIBUTED.md` (NEW, full model + honest limits), README table row,
  `docs/TESTING.md` counts/coverage.

**Verification (all EXECUTED this pass):** `cargo fmt --all --check` clean;
`cargo clippy --workspace --all-targets -- -D warnings` exit 0; `cargo test --workspace`
**514/514** (was 481; +33) with `POSTGRES_URL`/`REDIS_URL` live (0 skips); gated suites
standalone `--test-threads=1`: db_integration **19/19**, redis_integration **10/10**,
distributed_integration **4/4** (fresh + rerun); `cargo audit` exit 0 (9 pre-existing allowed
warnings); `cargo deny check` exit 0 (advisories/bans/licenses/sources ok).

**Remaining honest limits (also in docs/DISTRIBUTED.md §11):** lease-based fencing has a
theoretical pause-window between `fence()` and broadcast (compensated by intent journal +
handoff grace + reconciliation + chain-level balance checks — no epoch-checked storage
endpoint exists on Solana/Polymarket); Redis-only deployments lose claim state on Redis
restart (Postgres removes this); flag/book sync are periodic (bounded staleness: kill
propagation ≤ `flag_sync_secs`, capacity convergence ≤ `book_sync_secs`); claim rows carry
lineage one generation deep (full history in logs); sandbox PG/Redis run with durability off
— tests prove SQL/Lua/protocol/logic, not store crash-durability.

---

## Post-Prompt-3 gap closure (this pass — all EXECUTED, not designed-on-paper)

Closes every REMAINING GAP that is closable in-process; the rest stay documented
as honest limits (docs/DISTRIBUTED.md §11).

**1. Cluster-wide risk view (closes gaps 3+4 — cross-replica capacity & daily-loss).**
`GlobalRiskOracle` trait (bot-core `risk.rs`): `count_open(module) -> Option<usize>`,
`realized_today() -> Option<f64>`; `None` = unknown → local fallback (§K: risk never
depends on store availability). Combine is **tighten-only**: capacity uses
`max(local, global)`, daily-loss uses `min(local, global)` — an oracle can never loosen a
limit (unit-tested incl. the None/no-oracle fallbacks). Hooks: `preflight` daily-loss gate,
`check_entry` capacity step, and `book_pnl` daily-loss trip all consult
`effective_realized`/`effective_open_count`. `PostgresRiskOracle` (db/claims.rs) queries the
shared `positions` table: open count = `status IN ('open','closing') AND source=$1`;
`realized_today` = full lifecycle PnL (`realized_quote - cost_basis`) of positions closed
today UTC — **documented approximation**: partial exits on still-open positions stay local
until close. `main.rs` attaches it whenever Postgres is present. Contract/Telegram sources
return None (venue-agnostic).

**2. Full claim lineage (closes gap 5/6 — one-generation rows).** Migration `0011`
`execution_claim_events` (append-only: execution_id, event, owner_id, claim_epoch,
previous_owner, detail, created_at + indexes). `PostgresClaimStore` now records
`acquired | reacquired | takeover | released | handed_off | fenced | renew_rejected` on
every transition (fence/renew rejections include the current holder in `detail`), plus a
public `events(execution_id)` read API. Event writes are **best-effort**: audit failure
logs a warning and never changes the claim outcome — the claim row stays the authority.
Redis/memory stores unchanged (Postgres is the audit layer). This table immediately proved
its worth: it exposed the cross-run mint collision below from persisted history.

**3. Terminal-loss meter (closes gap 1-lite).** `bot_distributed_claim_terminal_lost_total
{module}` — emitted in `ClaimGuard::transition` when a terminal transition is refused
because ownership was lost mid-work (fenced during execution). Complements the existing
takeover/fencing counters.

**4. Two-replica MODULE-layer election test (closes gap 8 at module level).**
`module-copy/tests/two_replica_mirror.rs` (gated on Postgres): two independent `CopyBot`
instances (own AppState/registry, shared PG, paper mode, dead RPC port) receive the SAME
whale trade via `tokio::join!` — exactly one passes the claim gate (proven by it reaching
the network stage and erroring at curve load), the loser returns `Ok(())` having emitted
zero events, the claim row names the winner (Released, epoch 1), lineage =
`[acquired, released]`. Full two-process-on-live-validator e2e remains a documented limit.

**Bug found & fixed by the new test itself:** first workspace run failed with epoch=2 —
`Pubkey::new_unique()` is a per-process counter (same sequence every run), so the "unique"
mint collided with the previous run's claim row in the shared DB (previous_owner in the
events table named the stale replica: PID 58061 vs current 59256). Mint is now derived
from the run tag (splitmix64 over pid+nanos). TESTING.md rule 1 records the trap.
3× consecutive reruns pass.

**Still open (NOT closable in-process, unchanged):** fencing pause-window (no epoch-fenced
storage endpoint on Solana/Polymarket — compensated by journal+recon+handoff); Redis-only
restart loses claims; flag sync periodic (kill propagation ≤ `flag_sync_secs`); sandbox
durability off; exit claims fail closed per sweep round (deliberate).

**Verification (all EXECUTED this pass, post-fix):** `cargo fmt --all --check` clean;
`cargo clippy --workspace --all-targets -- -D warnings` exit 0; `cargo test --workspace`
**518/518** (was 514; +1 oracle unit, +2 db_integration, +1 two-replica) with
`POSTGRES_URL`/`REDIS_URL` live, 0 ignored; standalone `--test-threads=1`: db_integration
**21/21**, redis_integration **10/10**, distributed_integration **4/4**, two_replica_mirror
**1/1** (+3 consecutive reruns); `cargo audit` exit 0 (9 pre-existing allowed warnings);
`cargo deny check` exit 0 (advisories/bans/licenses/sources ok). Migration 0011 applied to
PostgreSQL 16.4 via the suites' own `migrate()`. Docs updated: DISTRIBUTED.md (§2 events,
§7 oracle, §8 metric, §10 tests, §11 limits 4+6 rewritten), TESTING.md, AUDIT.md (here).

---

## 26. Release-engineering / buyer-handover pass (2026-09-18, this tree)

Scope: the 27-item release pass executed on top of the verified tree from §25 — release
manifest, reproducibility audit, versioning artifacts, config/env audits, Docker static
inspection, DB packaging policy, runbook vs code, backup/restore, audit-system self-audit,
API contract check, security package, release gate script, doc taxonomy, cleanup, size,
license, secret scan, TODO scan, test matrix. Artifacts created: `VERSION`, `LICENSE`,
`CHANGELOG.md`, `SECURITY.md`, `scripts/release-check.sh`, `docs/RELEASE.md`,
`docs/HANDOVER.md`, `docs/BACKUP-RESTORE.md`, `docs/OPERATIONS.md`; modified: `README.md`,
`Cargo.toml`, `docs/TESTING.md`, `Dockerfile`, `.github/workflows/ci.yml`,
`crates/core/src/db/repo.rs`, `crates/core/tests/db_integration.rs`.

**Forensic findings (real defects, not cosmetic):**

1. **Audit-chain fork under concurrent appends (production bug, found by the new release
   tests).** `AuditRepo::append` read the chain head with
   `SELECT hash … ORDER BY id DESC LIMIT 1 FOR UPDATE`. Under READ COMMITTED, two
   concurrent writers both see the same head; the loser blocks, then re-reads its *stale
   snapshot* (EvalPlanQual only rechecks the locked row — the winner's new row is
   invisible), and appends from the wrong `prev_hash`. Result: a genuine chain fork —
   `verify_chain` reported `broken(at_id)=3` from concurrency alone, with no tampering.
   Fix: transaction-scoped advisory lock `pg_advisory_xact_lock(hashtext('audit_events_chain'))`
   acquired before the head read — serializes appends across all connections, processes
   and replicas that share the database, without changing any API or table.
   Regression tests added to `db_integration` (21→23):
   `audit_chain_detects_reorder_missing_and_duplicate` (direct SQL reorder/DELETE/
   duplicate-clone → each mutation must flip `verify_chain` from `None` to a specific
   `broken(at_id)`, with full row-set restore between mutations) and
   `audit_chain_survives_concurrent_appends` (8 concurrent appenders over one shared
   pool → chain stays linear, `verify_chain == None`, no forks).
2. **Toolchain-pin drift.** `Dockerfile` built on `rust:1.82` and CI's `program` job used
   unpinned `stable`, contradicting `rust-toolchain.toml` (1.98.1). Both pinned to
   1.98.1; `release-check.sh` now gates the three-way consistency (fails if any of the
   three drifts).
3. **Placeholder repository URL** (`example.com/...`) removed from `Cargo.toml`.
4. **Stale README claims corrected:** route table listed 14 of the 21 real API routes
   (the table was removed and replaced with a pointer to `docs/API.md`, which was
   verified route-by-route against the source); test counts updated to the numbers
   executed by the final gate.

**Gates executed on the final tree (all against real PostgreSQL 16.4 :5433 and Redis
7.2.10 :6379, `scripts/release-check.sh`, 20/20 PASS, exit 0):** required-file presence;
`VERSION` vs `Cargo.toml` vs lockfile consistency; toolchain pin three-way consistency;
migration monotonicity (0001–0011, no gaps, no down-migrations — forward-only policy per
`docs/BACKUP-RESTORE.md`); no TODO/FIXME/stub/unimplemented markers in shipped source; no
secret-pattern hits; `cargo fmt --all --check`; `cargo check --workspace --all-targets`;
`cargo clippy --workspace --all-targets -- -D warnings` (exit 0);
`cargo test --workspace -- --test-threads=1` **520 passed / 0 failed** (38 gated
integration tests executed, none skipped); standalone `db_integration` **23/23** (fresh +
rerun), `redis_integration` **10/10**, `distributed_integration` **4/4**,
`two_replica_mirror` **1/1**; staking `cargo fmt` + `clippy -D warnings` + `cargo test`
**48/48 host + 2 gated-skipped e2e** (no validator available); `cargo audit` (both
lockfiles, 0 findings); `cargo deny check` (advisories/bans/licenses/sources ok). Log
total across the script's steps: 608 test executions, 0 failures.

**Backup/restore round-trip executed (proves `docs/BACKUP-RESTORE.md` §7):**
`pg_dump` of the live database (11 migrations, 37 claims, 88 claim events, 15 positions,
12 orders) → `CREATE DATABASE restore_test` → plain-SQL restore (sequences `setval`'d) →
row counts identical on every table → **full `db_integration` suite 23/23 against the
restored database** (this includes `verify_chain == None` assertions, i.e. the restored
audit chain verified intact) → smoke DB dropped. Durable-state boundary confirmed by
inspection: Postgres holds all financial/audit/claim state; Redis holds only
leases/flags/caches and the two-replica suite proves a Redis-only restart loses nothing
durable.

**Reproducibility audit (inspection, not a claim):** 0 `build.rs` in any member; no build
timestamps embedded; `/health` and `bot_build_info` expose only `CARGO_PKG_VERSION` and
git metadata resolved at runtime when present; `Cargo.lock` committed for the workspace
and for `programs/staking-suite`; release profile `opt-level=3`, `lto="thin"`,
`codegen-units=1`, `strip=true`; `programs/staking-suite/Cargo.lock` pins
`solana-program =2.1.21` matching the `PREVIOUSLY VERIFIED` build-sbf toolchain.

**Not executed (environment-blocked, unchanged from §25 and documented as such):**
`cargo build-sbf` (no Solana toolchain in sandbox), validator e2e (`STAKING_E2E`, no
`solana-test-validator`), devnet e2e (`E2E_NETWORK`, no funded keypair), `latency_bench`
(requires co-located infra), Docker image build (no daemon — Dockerfile verified by
static inspection only), external security audit (none performed — `SECURITY.md` states
this explicitly).

---

## 27. Final engineering-freeze pass (2026-09-18, on top of release commit 9c677cd)

Scope: the 21-area freeze directive — full source audit, error model, money-path assertion,
authorization assertion, secret/leak assertion, observability, persistence, distributed,
staking, dependencies, runtime config, CI, release-gate false-positive analysis, hygiene,
documentation truth, release metadata, delivery manifest, size measurement, final test
execution, git integrity. Method: source-level inspection (grep + read, per area) →
targeted fixes → full gate re-execution. No subsystem rewrites; no functionality removed.

**Defects found and fixed:**

1. **Telegram bot-token leak into error strings (secret-leak assertion, item 5 — real,
   fixed, regression-tested).** The Bot API embeds the token in every request URL path
   (`https://api.telegram.org/bot<token>/<method>`), and `reqwest::Error`'s `Display`
   appends ` for url ({url})` on send errors (verified in the vendored reqwest 0.12.28
   source, `src/error.rs` Display impl). All ten reqwest error mappings in
   `crates/module-telegram/src/api.rs` interpolated that Display into `BotError::Http` /
   `BotError::Encoding` messages — so any failed Telegram call (timeout, DNS, connection
   refused) put the bot token into tracing logs, and potentially into audit detail strings
   and alert text. Fix: every mapping now calls `.without_url()` on the reqwest error;
   `method_url` carries a comment documenting the invariant. Regression test
   `error_strings_never_contain_the_bot_token` drives all four API methods
   (deleteWebhook, getUpdates, sendMessage, setMyCommands) against closed loopback port 1
   (deterministic, no network) and asserts the token appears in no error string.
   module-telegram tests 20→21; workspace 520→521.
2. **Unused dependencies removed (items 1/10):** `tokio-util` (declared by core,
   solana-kit, server — zero code references anywhere), `sha3` (module-polymarket —
   EIP-712 uses `tiny-keccak`; the only "sha3" hit was a test function name),
   `serde_with` (workspace entry no crate referenced). Removal verified by grep before
   editing and by `cargo check --workspace` + the full release gate after; `Cargo.lock`
   lost exactly the 4 direct-reference lines (the crates remain only where still required
   transitively). No version changes were made anywhere (directive: no cosmetic churn).
3. **Release-metadata drift corrected (item 15):** CHANGELOG claimed "Axum REST (22
   routes) + WebSocket event feed" — the 22 `/api` route paths already include the WS
   feed's path, double-counting it. Actual (counted from `api.rs` `.route()` calls and
   docs/API.md tables): 26 route registrations = 4 infra (`/`, `/health`, `/ready`,
   `/metrics`) + 22 `/api` paths (21 REST + 1 WS) = 28 method-level endpoints, matching
   docs/API.md exactly. CHANGELOG also said "ten docs under docs/" — there are thirteen.
   Both corrected.
4. **`release-manifest.json` added (item 17):** machine-readable delivery manifest —
   version, components, migration high-water mark (0011), toolchain pins, executed test
   counts, verification taxonomy (verified / previously verified / not executed),
   external handover blockers. No build timestamp (reproducibility) and no commit hash
   (self-reference: the file is part of the commit it would describe; git history is
   authoritative). `scripts/release-check.sh` now requires the file and fails on
   manifest-version drift (inside the existing version-consistency gate; still 20 gates).
5. **`docs/SECURITY.md` operator guidance added:** RPC/WS endpoint URLs are logged on
   connect and may appear in wrapped transport errors; providers that embed API keys in
   URLs make those log lines secret-bearing (prefer header auth). The suite's own secrets
   are never part of any URL it logs (enforced for Telegram by fix 1).

**Areas inspected, verdict CLEAN (no changes needed — evidence per area):**

- **Source freeze (item 1):** zero `dbg!`/`println!` in any production source; the only
  two `eprintln!` are in `server/src/main.rs` pre-tracing startup paths (config-load
  fallback + invalid log filter) — both loud, both fail-safe (defaults = paper mode, all
  modules disabled; required for the documented degraded/configless start exercised by
  the CI docker smoke test). All `panic!`/`todo!`/`unimplemented!` hits are inside
  `#[cfg(test)]` modules except `solana-kit/src/consts.rs` lazy-constant validation
  (fail-fast on an invalid hard-coded pubkey — deterministic programming-error trap,
  covered by unit tests). One `#[allow(dead_code)]` with written justification (complete
  borsh reader). A whole-tree scan for `.unwrap()` outside `#[cfg(test)]` sections in
  `src/` returned zero hits. No duplicate implementations found (the one real duplicate —
  two keccak providers — was resolved by fix 2).
- **Error model (item 2):** `BotError` classification consistent: `is_retryable()`
  (Http/WebSocket/Rpc/Timeout/Io = transient), `is_alertable()` (KillSwitch/
  InsufficientBalance/Rpc/Solana/Signing/Signer = page a human), permanent = the rest;
  reconciliation-required ambiguity is a distinct channel: `PolyError::SubmitUnknown` and
  executor `SendUnknown` vs `SendFailed`; `SignerError::is_configuration()` drives
  startup fail-fast. Cross-crate conversions normalized (`#[from]` io/json/toml/signer;
  explicit `From<PolyError> for BotError` preserving order-id + ambiguity in the
  message). `SignerError` is documented and structured secret-free (identities/pubkeys/
  context only). API error responses carry status + message, not internal payloads.
- **Money path (item 3):** every money-moving call site traced: sniper entry/exit, copy
  mirror/exit — `risk check → Permit::acquire(logical id) → proceed → fence() →
  with_intent(write-ahead) → executor.run → finish(ambiguous?)`; polymarket —
  `risk.check_entry → Permit::acquire(poly:entry:{token_id}) → fence → post_order →
  finish(hand-off on SubmitUnknown)`; executor broadcast modes (Rpc/Jito/JitoThenRpc/
  fan-out) sit strictly behind that pipeline. No REST route creates orders or moves
  funds (mutating routes: kill/resume/mode/module toggles/keys/journal only). Staking
  mint/governance is on-chain under program-enforced authority (item 9). No bypass
  found; none needed fixing.
- **Authorization (item 4):** `require_role` gates every route: reads `readonly`;
  kill/resume/module-enable/disable/mode(paper|simulate) `operator`; **mode(live),
  key add/revoke, journal rotate `owner`**; `/api/events` WS honors the same key via
  header or `?key=`; no-auth only on loopback (server refuses non-loopback bind without
  API auth — server tests); Telegram deny-by-default RBAC intact (module tests);
  `set_mode` live additionally gated by `allow_live_trading` config (response says
  "orders will simulate" when the gate is closed). Every mutating decision audited.
- **Secrets/leaks (item 5):** beyond fix 1 — `SecretConfig` hand-written `Debug` emits
  `<set>`/`<unset>` only (unit-tested), `Wallet`/`LocalKeypairSigner` Debug redacted
  (unit-tested), `/api/config` + config-version snapshots replace `secrets` with
  `<redacted>`, polymarket `auth.rs`/`clob.rs` contain zero logging statements (no
  header/token logging), API keys are digest-only at rest, gate secret-scan clean.
- **Observability (item 6):** all 11 metric names documented in `docs/OPERATIONS.md`
  exist verbatim in source (`bot_health_ready`, `bot_kill_switch`, `bot_execution_mode`,
  `bot_app_errors_total`, `bot_module_healthy`, `bot_module_consecutive_errors`,
  `bot_execution_latency_ms`, `bot_dup_total`, `bot_db_pool_active`, `bot_db_pool_idle`,
  `bot_events_dropped_total`); `bot_test_*`/`bot_a_gauge` names are confined to
  `#[cfg(test)]`; labels bounded, no secret/wallet/signature labels; request-ID
  correlation + structured JSON logs + shutdown/recon/risk logging all in place
  (unchanged from the verified §20-phase state).
- **Persistence (item 7):** exactly two explicit transactions in the repos — audit
  append (advisory-lock serialized, §26) and order `set_status` (transition + history
  row atomically); every other financial write is a single atomic statement (claim
  upsert `INSERT … ON CONFLICT … WHERE … RETURNING`, positions/orders upserts). PG =
  durable truth, Redis = coordination/cache, journal = forensic copy, dedup L1/L2/L3,
  startup recovery + reconciliation behavior unchanged and doc-matched (db_integration
  23/23 re-executed in this pass's gate).
- **Distributed (item 8):** invariant (one execution ⇒ ≤1 owner ⇒ ≤1 money submission)
  re-proven by the gate: 8-way claim race, lease/epoch/fencing lineage, handoff grace,
  two-replica mirror, cross-context flag/kill/position convergence — 4/4 + 1/1 executed.
  No new complexity added.
- **Staking (item 9):** untouched this pass; controls (account validation, caps,
  timelock queue/apply/cancel, pause-deposits, two-step admin, genesis latch, overflow
  checks, canonical program checks) remain as verified in earlier sections; docs keep
  the VERIFIED / PREVIOUSLY VERIFIED / NOT EXECUTED split; program id remains a
  pre-deploy placeholder; no external-audit claim anywhere.
- **Runtime config (item 11):** paper default; live requires `allow_live_trading` +
  `live_confirmation` + owner-role switch; dangerous combinations rejected by
  `validate()` (audited line-by-line in the release pass, unchanged); signer backend
  misconfig fails startup (never falls back); no silent unsafe fallback found (the
  config-load fallback is fail-safe: paper, modules off, loud on stderr).
- **CI (item 12):** toolchain pinned three ways (pin file governs the app job via
  rustup; program job explicit `dtolnay/rust-toolchain@1.98.1`; Dockerfile
  `rust:1.98.1-bookworm`); fmt/clippy `-D warnings`/build/test hard gates; real PG16 +
  Redis7 service containers with healthchecks and job-wide env (gated suites EXECUTE,
  never silently skip); staking fmt/clippy/test/build-sbf (solana 2.1.21 pinned) +
  validator e2e; audit ×2 + deny (advisories/bans/sources/licenses) hard gates; docker
  build + container health smoke; default failure propagation (no `continue-on-error`).
  Network-gated checks not run in CI (devnet e2e, latency bench) are explicitly labeled
  gated in `docs/TESTING.md`.
- **Release gate false-positive analysis (item 13):** `set -u`; every step runs through
  `step()` which treats any non-zero (including command-not-found, 127) as FAIL; SKIP is
  only possible for env-gated suites when the env var is unset, is counted separately,
  and is printed in the summary; the marker/secret scans' grep pipelines fail closed on
  hits and cannot silently pass on grep usage errors of the fixed patterns; no `||true`,
  no output swallowing, `--test-threads=1` mirrors CI. Manifest-version check added
  (fix 4). No genuine false-green path remains.
- **Hygiene (item 14):** `git ls-files` contains no logs/dumps/backups/editor/OS/
  credential files; `.gitignore` covers target, .env*, keys, jsonl, ledgers, editor
  noise; `.dockerignore` keeps secrets/data/docs out of the image context; the stale
  714 MB local `target/` cache (pre-`CARGO_TARGET_DIR` residue) was deleted in the
  release pass. During this pass an ENOSPC incident (25 GB sandbox disk, 18 GB build
  cache) was resolved by deleting 210 stale duplicate build artifacts (>10 MB,
  keep-newest-per-name; 12.1 GB freed total) plus consumed installer archives
  (PostgreSQL source tree/tarball, redis tarball, cargo-audit/deny extraction dirs —
  binaries live in `~/.cargo/bin`); the running PG data dir and Redis tree were never
  touched.
- **Metadata (item 16):** `VERSION` = workspace `Cargo.toml` = staking `Cargo.toml` =
  both `Cargo.lock` package entries = `release-manifest.json` = **0.1.0** (now
  gate-enforced across all five); `rust-toolchain.toml` = Dockerfile = CI program job =
  **1.98.1**; no repository URL (placeholder removed); LICENSE holder + security contact
  remain deliberate, labeled fill-ins.

**Final gate (this pass, post-fix tree): `scripts/release-check.sh` **20 PASS / 0 FAIL / 0
SKIP**, `SCRIPT_EXIT=0` (log `release_check6.log`): workspace **521/521** single-threaded
(38 gated integration tests executed; whole-script total 609 test executions, 0 failures),
db_integration **23/23** (7.12 s), redis_integration **10/10**, distributed_integration
**4/4**, two_replica_mirror **1/1**, staking fmt + clippy `-D warnings` + **48/48 host +
2 gated-skipped e2e**, `cargo audit` ×2 lockfiles 0 findings, `cargo deny check` ok,
`cargo fmt --all --check` clean, `cargo check` clean. New in this pass's totals: the
Telegram token-redaction regression test (module-telegram 20→21). An intermediate run
(`release_check5.log`) caught the only process defect of this pass — the un-rustfmt'd
insertion — proving the fmt gate works: 19 PASS / 1 FAIL, then green after `cargo fmt`.

**Freeze verdict:** all internal audits pass; the only open items are the external/human
ones listed in `docs/HANDOVER.md` §5 and `release-manifest.json`
`external_handover_blockers`. STOP CONDITION met — no further engineering work should be
done on this tree without a new directive.

## 28. Post-delivery audit pass (2026-09-18, current tree — directive: full audit + gap fixes)

**Scope of the directive:** re-audit the delivered repository line-by-line (not trusting prior
audits or docs), fix the six named gap areas (A live/paper balance separation, B staking max
supply, C token metadata, D signer backends, E deployment completeness, F test coverage),
preserve all working architecture, and re-run the full gate suite honestly.

### Findings (verified in source, not docs)

* **A — CONFIRMED DEFECT (module 3).** `module-polymarket/src/lib.rs::available_usdc` returned
  the cached dashboard balance when > 0 **in every mode** (a paper start seeds 1,000 USDC via
  `server/src/main.rs`, and that seed survives a runtime paper→live mode switch) and otherwise
  fell back to `PAPER_USDC_BALANCE` **explicitly in non-paper mode** ("fall back to the paper
  figure so sizing never panics"). No on-chain collateral reader existed anywhere in the module
  (`ctf.rs` reads ERC-1155 outcome tokens only). Live orders were therefore sized against a demo
  balance, feeding `EntryRequest.available_quote` into the risk gate (reserve / fraction cap /
  rejection all keyed off it).
* **A2 — CONFIRMED DEFECT (module 1).** `module-sniper/src/lib.rs::available_sol` fell back to
  the cached balance on RPC failure **without checking the execution mode**, contradicting its
  own comment ("Live mode surfaces the real error"). Same poisoned-seed exposure after a mode
  switch. Module 2 (copy) was audited and is correct (paper cache only in paper mode; real RPC
  read otherwise; errors propagate) — left untouched.
* **B — CONFIRMED GAP (module 4).** `Config` had **no max-supply field at all**; `GenesisMint`
  was admin- and latch-gated but unbounded in amount, and reward minting in `claim`/`unstake`
  had no total-supply cap. `ContractConfig.token_supply` (app config, default 1,000,000,000) was
  declared and **never used anywhere** — a config value with no enforcement.
* **C — CONFIRMED GAP (module 4).** No token-metadata implementation existed (repo-wide grep:
  only Polymarket order-metadata fields and unrelated DAS types).
* **D — COMPLIANT, NO CHANGE.** `solana-kit/src/signer.rs::build_signer_registry` hard-fails at
  startup for `vault`/`kms`/`hsm` (typed error, documented "only local implemented in this
  build", regression-tested in `signer.rs` tests: every unsupported provider must fail startup).
  No fake backends exist; the system cannot advertise them as functional.
* **E — SPOT-REVERIFIED.** Deployment surface present and consistent (`.env.template`,
  `config.toml.example`, Dockerfile, compose, CI workflow, release/verify scripts, migrations
  0001–0011). `config.toml.example` comments updated for the new collateral semantics;
  `[contract].token_supply` documented as informational ↔ on-chain `max_supply` binding.
* **Latent test flake found by the gate itself.** `bot-core auth::tests::rate_limiter_refills_over_time`
  asserted `Limited` immediately after draining 6,000 tokens while the limiter refills
  continuously against the wall clock (100 tok/s): under machine load (this pass hit an ENOSPC
  thrash) the burst loop spans > 10 ms and the strict assert flakes. Production code verified
  correct; the TEST was made deterministic (bounded drain loop; refill cannot outpace it).

### Changes (all callers/tests/configs/docs updated in the same pass)

* **New** `crates/module-polymarket/src/collateral.rs` — Polygon ERC-20 reader
  (`balanceOf` 0x70a08231 / `decimals` 0x313ce567 / `allowance` 0xdd62ed3e via `eth_call`),
  u128-no-truncation decoding shared with `ctf.rs` (helpers made `pub(crate)`), `raw_to_usd` /
  `usd_to_raw` conversions with NaN/negative/overflow rejection; mock-RPC wire tests.
* `module-polymarket/src/error.rs` — new typed variants `BalanceUnavailable` and
  `InsufficientFunding` (+ `BotError` mapping).
* `module-polymarket/src/lib.rs` — `available_usdc` replaced by `available_collateral` +
  `read_collateral` (freshness-bounded 15 s snapshot, decimals plausibility 1..=18, mirrors the
  REAL balance into shared state) + `ensure_live_funding` (pre-broadcast: balance covers the
  approved notional; for `signature_type == 0` the settling exchange — neg-risk-aware — must
  hold the ERC-20 allowance; proxy flows 1/2/3 balance-only by design) + pure
  `resolve_sizing_balance` separation rule. `will_send` moved BEFORE the ownership permit so the
  funding gate rejects without consuming a claim. LIVE ignores the cached seed entirely;
  PAPER/SIMULATE keep the demo path. 5 separation-matrix regression tests (poisoned seed,
  failed/missing/implausible reads, demo fallthrough order).
* `module-sniper/src/lib.rs` — `available_sol` fallback is now paper-mode-only via the pure
  `sol_balance_fallback` rule (+ unit test; simulate/live propagate the RPC error).
* `programs/staking-suite` — `Initialize` gained `max_supply: u64` (> 0, stored in `Config`,
  NOT in `PendingParams`/`UpdateParams` ⇒ immutable); `GenesisMint` enforces
  `fits_under_cap(live mint supply, amount, max_supply)` (checked arithmetic, overflow fails
  closed) before the mint CPI; reward minting clamps to `supply_headroom` so
  `claim`/`unstake` can never fail at the cap (shortfall forfeited, `msg!`-logged). Errors
  6028 `MaxSupplyExceeded`, 6029 `InvalidMaxSupply`. New one-shot admin instruction
  `CreateTokenMetadata{name,symbol,uri}` — hand-rolled borsh `CreateMetadataAccountsV3`
  (discriminant 19, byte-layout pinned by test) CPI to the canonical mpl program id
  (`metaqbxxUerdq28cj1RbAWkYQm3ybzjb6a8bt518x1s`, const-asserted), immutable metadata
  (`is_mutable=false`), config PDA as mint/update authority via `invoke_signed`, canonical-PDA +
  program-id + byte-limit (32/10/200, measured in BYTES like mpl) validation, one-shot via
  account-existence check. Errors 6030/6031/6032. Builder `create_token_metadata_ix` +
  `validate_metadata_fields` shared with the processor.
* `programs/staking-suite/tests/validator_e2e.rs` — `initialize_ix` carries `max_supply`; all
  three existing call sites updated; third gated e2e added
  (`validator_e2e_max_supply_cap_and_metadata`: zero-cap rejection, one-over-cap rejection with
  latch untouched, exact-cap genesis, stake→claim at zero headroom (succeeds, supply frozen),
  unstake accounting, metadata against a mainnet-CLONED real mpl program + replay rejection).
  Compiles green here; **NOT EXECUTED** (no build-sbf/validator/internet clone in this sandbox).
* `crates/core/src/auth.rs` — the rate-limiter refill test de-flaked (test-only change).
* Config/docs kept truthful: `config.toml.example`, `release-manifest.json` (counts + honest
  `previously_verified_superseded_source` class for build-sbf/e2e on the changed program
  source), CHANGELOG `[Unreleased]`, README, docs/{STAKING,MODULES,TESTING,REPOSITORY-MAP,
  DELIVERY-MANIFEST,HANDOVER,BUYER-*,CAPABILITY-MATRIX,DEMO-RUNBOOK,EVIDENCE-INDEX,
  FINAL-DELIVERY,ACCEPTANCE-CHECKLIST}.md. Historical freeze-gate figures were preserved as
  historical; "latest count" claims were updated to this pass.

### Gate results (this pass, this sandbox: PostgreSQL 17.11 + Redis 8.0.2 via apt, Rust 1.98.1)

* `cargo fmt --all --check` — clean (both cargo projects).
* `cargo check --workspace --all-targets` — clean.
* `cargo test --workspace -- --test-threads=1` — **537/537, 0 failures** (db_integration 23/23,
  redis_integration 10/10, distributed_integration 4/4, two_replica_mirror 1/1 executed against
  the real services; devnet/latency/recon-crash gated-skipped as designed).
* `cargo clippy --workspace --all-targets --all-features -- -D warnings` — clean.
* staking: fmt clean, `cargo clippy --all-targets -- -D warnings` clean, host tests **71/71**
  (was 48/48; +23: cap math/boundaries, live-supply authority, reward clamping incl. zero
  headroom, metadata guards/layout/PDA), e2e test target compiles.
* `cargo audit` ×2 lockfiles — 0 errors (9 pre-existing allow-listed warnings, unchanged
  `.cargo/audit.toml`); `cargo deny check` — advisories/bans/licenses/sources ok.
* `Cargo.lock` package counts unchanged (706 / 580) — no dependency drift; no Cargo.toml touched.
* `scripts/release-check.sh` (POSTGRES_URL/REDIS_URL exported, real services up) —
  **20 PASS / 0 FAIL / 0 SKIP, exit 0** (log: release_check_audit3.log). Inside the gate:
  fmt/check/clippy `-D warnings` clean; workspace test step 537/537;
  db_integration 23/23, redis_integration 10/10, distributed_integration 4/4,
  two_replica_mirror 1/1 executed AGAINST REAL PostgreSQL 17.11 + Redis 8.0.2;
  staking fmt/clippy clean + host tests 71/71; validator_e2e target compiles and its
  3 tests gate-skip (no STAKING_E2E / no validator in this sandbox);
  migrations monotonic; no TODO/stub markers; no secret-looking literals;
  cargo audit both lockfiles 0 errors (1251 advisories loaded, 9 pre-existing
  allow-listed warnings); cargo deny: advisories/bans/licenses/sources all ok.
* Environment-blocked (NOT executed, labeled): `cargo build-sbf` + all three validator e2e on
  the audit-pass program source (no Solana toolchain/validator here; the metadata e2e also
  needs internet cloning of mpl), Docker build (no daemon), GitHub CI (no runner), pg_dump→
  restore round-trip (not re-run this pass), funded live validation.

### Integrity notes

* release-check needed four attempts in this sandbox, recorded as process evidence per the
  honesty rule: #1 aborted on sandbox ENOSPC (15 PASS / 5 FAIL, all five failures were
  `No space left on device (os error 28)`, not logic); #2 = 19 PASS / 1 FAIL, the sole
  failure being the pre-existing `rate_limiter_refills_over_time` wall-clock flake under
  load (fixed above — test-only change); #3 = 19 PASS / 1 FAIL where `cargo deny` died
  mid-download on ENOSPC again (disk reached 0 bytes; result invalid, cleaned 2.8G of
  incremental artifacts); #4 = the clean **20/0/0** run cited above. No gate result from
  an ENOSPC-corrupted run was ever recorded as a pass.
* The delivered 0.1.0 snapshot archive (`/home/user/delivery/`) intentionally still reflects the
  PRE-audit 170-file tree; the current tree is 171 files / 3,135,466 bytes / 84,108 lines (post-§28-append;
  pre-append 171 / 3,125,115 / 83,977; original baseline 170 / 3,021,664 / 81,583).

## 29. Buyer-hardening pass (2026-09-18, on top of the §28 tree — directive: re-verify everything by execution)

Directive constraints honored throughout: current repo tree as sole source of truth; prior
report numbers re-checked, not trusted; no manufactured evidence (no fake transactions,
program IDs, benchmarks, audits, mainnet claims); every result traceable to a command +
artifact + hash in `evidence/`; strict PASS / NOT RUN / BLOCKED / HUMAN ACTION separation;
working logic preserved (no redesign).

**Environment:** 2-core VM, 2 GB RAM (no swap after phase 5), ~25 GB disk, PostgreSQL 17.11
(@5432, trust) + Redis 8.0.2 (@6379) live, rustc/cargo 1.98.1, agave/solana-cli 2.1.21
(official release tarball, SHA-256 `5da3359e…`), platform-tools v1.43 (sbf rustc 1.79.0),
cargo-audit 0.22.2, cargo-deny 0.18.9. Public mainnet-beta + devnet RPC reachable. No Docker
daemon, no GitHub runner — both recorded BLOCKED, never converted to PASS.

**Executed results (artifacts in `evidence/`, indexed in `docs/EVIDENCE-INDEX.md`):**

1. `cargo metadata --locked` ✓; `cargo check --workspace --all-targets` ✓ (exit 0).
2. `cargo fmt --all --check` ✓; `cargo clippy --all-features -- -D warnings` ✓ (app),
   clippy ✓ + host tests **71/71** (staking program).
3. `cargo test --workspace -- --test-threads=1` **537/537** with live PG+Redis (gated
   db_integration/redis/distributed/two-replica suites EXECUTED, not skipped);
   `--all-features` **537/537** (no delta).
4. `cargo build-sbf`: `target/deploy/staking_suite.so` 187,504 B, SHA-256
   `57a890fae273f2c569fc814c43f0645311b6983dd30782126a9844ee193b5564` — supersedes the
   freeze-era `9e113678…` build (never reused as evidence for this source). Determinism
   proven: `touch src/lib.rs && cargo build-sbf` with freshly-fetched platform-tools →
   byte-identical hash.
5. `STAKING_E2E=1 cargo test --test validator_e2e -- --test-threads=1`: **3/3 passed,
   160.72 s** — real `solana-test-validator` (Agave 2.1.21 BPF VM), metadata test cloning
   the REAL mpl-token-metadata program from mainnet-beta. Coverage now executed, not merely
   compiled: governance lifecycle; funded stake→reward→claim→unstake money flow; zero-cap
   reject, one-over-cap reject, exact-cap mint, no-exceed assertion, zero-headroom safe
   claim, unstake correctness, metadata correctness, metadata replay rejection, authority
   enforcement.
6. DB backup/restore round-trip (PostgreSQL 17.11): populated 24 tables / 68 rows via the
   app's own repositories, `pg_dump -Fc` (dump SHA-256 `5989ecf1…`), restore to a CLEAN
   database, tables/migrations/rowcounts IDENTICAL across three comparisons, `_sqlx_migrations`
   11/11 success, db_integration **23/23 re-run ON the restored DB**, app binary started
   against the restored DB, endpoints verified (`/health` ok; `/ready` 200 with 4 components;
   `/api/status` = paper / live_allowed:false / kill_switch:false; `bot_*` metrics), clean
   SIGTERM shutdown ("sniper-suite stopped cleanly"). Production-equivalent data untouched.
7. `E2E_NETWORK=1 latency_bench` (existing tests only — nothing invented): getSlot n=30,
   getLatestBlockhash n=30, simulateTransaction n=10 against public devnet; landing_rate
   SKIP (requires funded keys — HUMAN ACTION). Machine-readable record
   `evidence/benchmarks-2026-09-18.json` with hardware/toolchain/network class and an
   explicit not-measured list; sandbox egress figures are measurements of this environment,
   not product guarantees.
8. Program identity: `scripts/staking-identity.sh` (show/verify/set-id/deploy) added + tested;
   `verify` confirms the declared placeholder `3vEEMMFmdA88n8ApgZ3b9L3BXEh75yCeMbHbmUjR9mfy`
   agrees across all tracked references. No program ID was invented. Final ID + keypair =
   HUMAN ACTION; deploy refuses keypair≠declare_id and placeholder-on-public-cluster.
   README's previously DANGEROUS deploy hint (deploy with the build-sbf auto-generated
   keypair — which provably mismatches declare_id) replaced with the safe procedure.
9. Static security evidence re-confirmed: 0 `unsafe {` blocks; `#![forbid(unsafe_code)]` ×4;
   0 TODO/FIXME/stub markers; cargo audit ×2 (0 errors, 9 allow-listed warnings) + cargo deny
   (advisories/bans/licenses/sources ok) inside the release gate. No external security audit
   exists; none is claimed.

**Defects found by execution (compile-green ≠ executed, proven):**

* **D1 — staking program (real, would have hit mainnet):** `processor.rs` sent
  `MPL_CREATE_METADATA_ACCOUNTS_V3 = 19`. Discriminant 19 in the deployed mpl-token-metadata
  enum is `Utilize` — the metadata instruction would have been malformed/rejected on-chain
  (verified against the deployed program: `CreateMetadataAccountV3 = 33`; source checked at
  tag `token-metadata@v1.14.0`). Fixed to 33; the fix is proven by the executed e2e (metadata
  created against the real program + replay rejected).
* **D2 — validator e2e harness:** `--clone` cannot clone an upgradeable program
  ("Program is not deployed"); the metadata test could never have run. Fixed to
  `--clone-upgradeable-program`. Both fixes are test/program-side; no application-crate logic
  was changed in this pass.
* **D3 — evidence tooling (not repo):** `curl | head` under `set -o pipefail` SIGPIPEs (exit
  141) and killed the phase-8b startup script before its shutdown step; wrapped as
  `{ …; } || true`. Recorded for reproducibility of the evidence itself.
* **Post-pass evidence-integrity note (2026-09-19):** a file-by-file completeness audit found
  the persisted `phase3-sbf-rebuild.log` TRUNCATED at the platform-tools download stage (the
  hash-comparison tail existed only in the non-persisted live monitor). Response: a full COLD
  determinism re-run (reinstalled rust 1.98.1, agave 2.1.21 tarball hash-verified `5da3359e…`,
  fresh platform-tools, empty sbf cache) executed end-to-end with a complete log —
  `REBUILD_EXIT=0`, rebuilt .so 187,504 B, SHA-256 `57a890fa…`, `BYTE_IDENTICAL=YES` vs the
  preserved first-build artifact (`evidence/phase3-sbf-determinism-rerun.log`, ledger
  EVID-SBF-003). The truncated log is kept as honest history, never deleted. The same audit
  also clarified that `phase8-main.log` step-7's transient compile error (disk pressure) is
  superseded by the phase8b app-startup evidence, and verified all 175 files three-way
  byte-identical across repo / package mirror / tarball (171 line-scan clean + 4 reviewed
  false positives; no placeholders).

**Still BLOCKED / HUMAN ACTION (never converted):** Docker image build + container smoke and
`docker compose config -q` (no daemon — native-equivalent binary smoke PASSED and is labeled
as such); actual GitHub Actions run (no runner — 1:1 local equivalents mapped in
`docs/CI-LOCAL-EQUIVALENCE.md`); funded live validation and landing-rate bench (procedure
written in `docs/LIVE-VALIDATION.md`; execution requires operator funds + approval); final
program keypair/ID and deployment; external security audit (none exists).

**Tree after this pass:** 175 files / 3,196,164 bytes / 85,164 lines (post-§29-append; §28 recorded 171 / 3,135,466 / 84,108 at that time). Added this pass: `scripts/staking-identity.sh`, `docs/LIVE-VALIDATION.md`, `docs/BUYER-ACCEPTANCE-TEST.md`, `docs/CI-LOCAL-EQUIVALENCE.md` (39 docs). Modified this pass: `programs/staking-suite/src/processor.rs`, `programs/staking-suite/tests/validator_e2e.rs`, `README.md`, `CHANGELOG.md`, `release-manifest.json`, `AUDIT.md` (this §29), and docs: EVIDENCE-INDEX, HANDOVER, REPOSITORY-MAP, DELIVERY-MANIFEST, STAKING, BUYER-DUE-DILIGENCE, BUYER-DEPLOYMENT, BUYER-OVERVIEW, TECHNICAL-FACT-SHEET, CAPABILITY-MATRIX, RELEASE-NOTES-0.1.0, SELLER-FACT-SHEET, SELLING-LISTING-SOURCE, TECHNICAL-DIFFERENTIATORS.
