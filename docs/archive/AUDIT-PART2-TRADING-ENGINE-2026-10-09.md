# Part 2 Audit — solana-kit, module-sniper, module-copy (+ bot-core fixes) — 2026-10-09

## Status: NOT GREEN. Build now compiles; 9 tests fail; clippy not yet run.

## Fixes applied

| ID | Sev | File | Fix |
|---|---|---|---|
| P2-1 | **Critical (build)** | `crates/core/src/execution.rs` (deleted) | `execution.rs` and `execution/mod.rs` both existed, so `bot-core` failed with E0761 and **no crate depending on it compiled** (incl. the Dockerfile build). The 10-line `execution.rs` was a re-export shim of `execution/mod.rs`; removed. Public API is unchanged. |
| P2-2 | High | `crates/core/src/config.rs` `validate_copy_engine` | `copy.slippage_pct` was never validated. Copy exits pass `slippage_pct*100` straight to Jupiter with no clamp, so 150 meant 15,000 bps. Now rejected unless finite, in `[0,100]`, and ≤ `risk.max_slippage_bps`. |
| P2-3 | Medium | `crates/core/src/config.rs` `validate` | `sniper.slippage_pct` used `< 0 || > 100`, so NaN (valid TOML `nan`) passed. Now uses `is_finite`. |
| P2-4 | Test | `crates/core/src/config.rs` tests | Added regression assertions for NaN and out-of-range, plus the copy ceiling. `bot-core` targeted test passes. Shipped example config (copy 20% vs 30% ceiling) still validates. |

## Test results (`cargo test --locked --no-fail-fast`, rustc 1.98.1)

| Binary | Result |
|---|---|
| module-copy (unit 60 + 9 integration binaries) | all pass |
| module-sniper unit (174) + backtest/concurrency/detect/exit_sweeper/... | all pass |
| module-sniper `failure_injection` | **2 failed**, 20 passed |
| module-sniper `gates_risk_intel` | **5 failed**, 6 passed |
| module-sniper `pipeline` | **1 failed**, 14 passed |
| solana-kit `latency_bench` | **1 failed**, 4 passed |
| solana-kit other binaries | pass |

Failures (root cause classified):

1. `failure_injection::missing_liquidity_is_insufficient_liquidity` — zero virtual reserves are treated as graduated (`is_graduated`) and fail closed with `ExecutionUnavailable` before gates run. Safe behavior, but the reason label is wrong for the test's intent. **Decision:** label as InsufficientLiquidity (code change) or update the test.
2. `failure_injection::unreadable_mint_is_skipped_by_default_and_rejected_under_strict_gates` — **real determinism defect.** The default "skip freeze gate" path only works if the mint is in the cache. With a cold cache, `token_program_of` returns NotFound and the launch is rejected as `PoolNotReady` before gates. Behavior depends on cache warmth. Fix: in `market.rs`, when the mint is missing, continue with `mint_info=None` (gates decide skip/fail) instead of failing the load.
3. `gates_risk_intel` ×5 (creator serial-rugger, bundler ratio, holder concentration ×2, sell-simulation unknown → skip) — gate outcome semantics differ from the test expectations (Skip vs Fail/Pass). Needs per-gate review against the documented skip semantics in `docs/SNIPER-ENGINE.md` before deciding which side is wrong. Not changed.
4. `pipeline::graduated_curve_without_a_route_is_invalid_route_and_pool_not_ready_when_missing` — expected `InvalidRoute`, got `ExecutionUnavailable`. Same family as (1): the graduated-curve path is classified upstream in `PumpContext::load`.
5. `latency_bench::warm_cache_serves_hot_path_reads_offline` — **test fixture bug.** The warm mint cache entry has 0 bytes of data. `token_program_of` validates the mint layout and fails closed, which is correct. Fix: seed a valid 82-byte SPL mint in the fixture.

## Static findings (no test failures)

- Float money math: `slippage_bps = (pct*100).round() as u64` is clamped downstream on entries (`hard_max_bps`), but **sell/exit paths do not clamp** to `risk.max_slippage_bps` (module-sniper `exit.rs:962`, module-copy `exit.rs:581`). Now bounded by P2-2 for copy and by the sniper range check; a clamp at the call site is still recommended.
- Tip floor (`extract_tip_floor_value`) is finite-checked and clamped. Good.
- `module-copy/src/recovery.rs` uses `Mutex::lock().unwrap()` in ~6+ places. A poisoned lock panics the recovery path. Recommend `lock().unwrap_or_else(|e| e.into_inner())` or propagating errors.
- Dropped `tokio::spawn` handles in `module-sniper/src/lib.rs:502` and `module-copy/src/lib.rs:818` (exit sweeper). A panic there is silent; recommend supervising the JoinHandle (restart or fail-closed).
- Not reviewed in depth this batch: `execute.rs` (3,278 lines), `ws.rs` (2,210), `provider.rs`, RPC failover/reconnect logic, transaction replay guards, Jito bundle path. These are the largest files and need their own pass.

## Not done in Part 2

- `cargo clippy -D warnings` for these crates: not run.
- Full-file output: `config.rs` is ~3,500 lines. Changed regions are listed above; full file goes in the workspace, not in chat.
- Repo-root `.cargo/config.toml` requires `clang`/`lld`; this sandbox used `RUSTFLAGS=" "` overrides for all runs.
