//! Trusted backtest worker (GAP-MAP P1 NEW).
//!
//! `backtests.rs` persists tenant requests as `queued` rows in
//! `backtest_runs`; without this worker they stay queued forever. The
//! worker:
//!
//! 1. **Claims** one queued run at a time with `FOR UPDATE SKIP LOCKED`
//!    (extra replicas are harmless);
//! 2. **Runs** the deterministic `module_sniper::backtest` simulator inside
//!    `spawn_blocking` (it is CPU-bound and pure);
//! 3. **Writes** the authoritative `result_json` and terminal status.
//!
//! ## Honesty rules
//!
//! * **No historical dataset exists in this deployment**, so the simulator
//!   runs over SYNTHETIC scenarios derived deterministically from the run
//!   id (seeded PRNG). `result_json` carries `"label": "synthetic"` and a
//!   note — the API surface and the UI must surface that label next to any
//!   number. If a real recorded dataset pipeline lands later, it becomes a
//!   new branch here; the synthetic path never pretends otherwise.
//! * Request parameters are honoured where the model supports them:
//!   `slippage_bps` → entry slippage tolerance, `fee_rate_bps` → exit
//!   cost, `period` → scenario count, `initial_balance` → per-trade size
//!   (1% of balance). Parameters the model cannot honour are echoed into
//!   `result_json.notes` instead of being silently dropped.
//! * USD conversion uses an operator-pinned reference price
//!   (`BACKTEST_SOL_USD_CENTS`, default 15000 = $150), NOT a live price:
//!   it is reported in the result so nobody mistakes the basis.
//! * A run that crashes lands as `failed` with the error text — never a
//!   fabricated success.

use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use serde_json::{json, Value};
use sqlx::Row;
use uuid::Uuid;

use bot_core::db::Database;
use bot_core::lifecycle::Shutdown;
use module_sniper::backtest::dataset::synthetic_scenarios;
use module_sniper::backtest::exit_sim::ExitConfig;
use module_sniper::backtest::fill_model::FillModelConfig;
use module_sniper::backtest::{run_backtest, BacktestReport};

/// Environment knob for the synthetic→USD reference price (cents per SOL).
pub const REFERENCE_PRICE_ENV: &str = "BACKTEST_SOL_USD_CENTS";
/// Fallback reference price: $150 per SOL.
pub const DEFAULT_REFERENCE_SOL_USD_CENTS: u64 = 15_000;

/// Fraction of the initial balance risked per synthetic entry.
pub const PER_TRADE_BALANCE_FRACTION: f64 = 0.01;

#[derive(Debug, Clone)]
pub struct BacktestWorkerConfig {
    /// Poll cadence for queued runs.
    pub poll_interval: Duration,
    /// Synthetic scenario budget bounds (derived from the run period).
    pub min_scenarios: usize,
    pub max_scenarios: usize,
}

impl Default for BacktestWorkerConfig {
    fn default() -> Self {
        BacktestWorkerConfig {
            poll_interval: Duration::from_secs(5),
            min_scenarios: 8,
            max_scenarios: 200,
        }
    }
}

/// One claimed run, exactly as stored.
#[derive(Debug)]
struct ClaimedRun {
    id: Uuid,
    period_start: DateTime<Utc>,
    period_end: DateTime<Utc>,
    initial_balance_usd_cents: i64,
    fee_rate_bps: i32,
    slippage_bps: i32,
}

pub struct BacktestWorker {
    db: Arc<Database>,
    cfg: BacktestWorkerConfig,
}

impl BacktestWorker {
    pub fn new(db: Arc<Database>, cfg: BacktestWorkerConfig) -> Self {
        BacktestWorker { db, cfg }
    }

    /// Background loop: matches the `WebhookRetryDispatcher` idiom.
    pub async fn run(self: Arc<Self>, shutdown: Arc<Shutdown>) {
        // Single-server requeue: if we are starting up while rows are
        // marked `running`, the process that owned them is dead (this
        // deployment runs one worker). Put them back in the queue instead
        // of leaking them forever.
        match sqlx::query("UPDATE backtest_runs SET status = 'queued' WHERE status = 'running'")
            .execute(self.db.pool())
            .await
        {
            Ok(result) if result.rows_affected() > 0 => {
                tracing::info!(
                    requeued = result.rows_affected(),
                    "backtest worker: requeued runs orphaned by a previous process"
                );
            }
            Ok(_) => {}
            Err(error) => {
                tracing::warn!(error = %error, "backtest worker: startup requeue failed");
            }
        }

        let mut ticker = tokio::time::interval(self.cfg.poll_interval);
        loop {
            tokio::select! {
                _ = shutdown.wait() => break,
                _ = ticker.tick() => {
                    match self.run_once().await {
                        Ok(0) => {}
                        Ok(processed) => {
                            tracing::info!(processed, "backtest worker: processed runs");
                        }
                        Err(error) => {
                            tracing::warn!(error = %error, "backtest worker: poll failed");
                        }
                    }
                }
            }
        }
        tracing::info!("backtest worker stopped");
    }

    /// Claim and process runs until the queue drains (bounded per call).
    /// Returns how many runs reached a terminal state.
    pub async fn run_once(&self) -> Result<usize, sqlx::Error> {
        let mut processed = 0usize;
        while let Some(run) = self.claim_one().await? {
            self.execute(run).await;
            processed += 1;
        }
        Ok(processed)
    }

    /// Atomic single-row claim. `SKIP LOCKED` keeps concurrent workers
    /// from ever taking the same row.
    async fn claim_one(&self) -> Result<Option<ClaimedRun>, sqlx::Error> {
        let row = sqlx::query(
            "UPDATE backtest_runs SET status = 'running'
              WHERE id = (SELECT id FROM backtest_runs
                           WHERE status = 'queued'
                           ORDER BY created_at ASC
                           LIMIT 1
                           FOR UPDATE SKIP LOCKED)
              RETURNING id, period_start, period_end,
                        initial_balance_usd_cents, fee_rate_bps, slippage_bps",
        )
        .fetch_optional(self.db.pool())
        .await?;
        Ok(row.map(|row| ClaimedRun {
            id: row.get("id"),
            period_start: row.get("period_start"),
            period_end: row.get("period_end"),
            initial_balance_usd_cents: row.get("initial_balance_usd_cents"),
            fee_rate_bps: row.get("fee_rate_bps"),
            slippage_bps: row.get("slippage_bps"),
        }))
    }

    async fn execute(&self, run: ClaimedRun) {
        let result = self.simulate(&run).await;
        let (status, result_json, error) = match result {
            Ok(json) => ("completed", Some(json), None),
            Err(message) => ("failed", None, Some(message)),
        };
        let update = sqlx::query(
            "UPDATE backtest_runs
                SET status = $2, result_json = $3, error = $4, completed_at = NOW()
              WHERE id = $1",
        )
        .bind(run.id)
        .bind(status)
        .bind(result_json)
        .bind(error)
        .execute(self.db.pool())
        .await;
        match update {
            Ok(_) => {}
            Err(error) => {
                tracing::error!(run = %run.id, error = %error, "backtest worker: failed to write terminal state");
            }
        }
    }

    /// Build the deterministic synthetic dataset from the run identity and
    /// execute the pure simulator off the async runtime.
    async fn simulate(&self, run: &ClaimedRun) -> Result<Value, String> {
        let scenarios = scenario_budget(&self.cfg, run.period_start, run.period_end);
        let seed = seed_of(run.id);
        let reference_cents = reference_sol_usd_cents();

        let initial_usd = run.initial_balance_usd_cents as f64 / 100.0;
        let initial_sol = run.initial_balance_usd_cents as f64 / reference_cents as f64;
        let entry_sol = (initial_sol * PER_TRADE_BALANCE_FRACTION).max(0.001);
        let entry_quote_lamports = (entry_sol * 1e9) as u64;

        let fill_cfg = FillModelConfig {
            entry_latency_ms: FillModelConfig::default().entry_latency_ms,
            slippage_pct: (run.slippage_bps.max(0) as f64) / 100.0,
            tip_lamports: FillModelConfig::default().tip_lamports,
        };
        let exit_cfg = ExitConfig {
            take_profit_pct: ExitConfig::default().take_profit_pct,
            stop_loss_pct: ExitConfig::default().stop_loss_pct,
            max_hold_ms: ExitConfig::default().max_hold_ms,
            // Venue fee plus the quarter-slippage allowance the exit model
            // uses for its own side of the trade.
            exit_cost_bps: (run.fee_rate_bps.max(0) as f64)
                + (run.slippage_bps.max(0) as f64) * 0.25,
        };

        // The simulator is pure and CPU-bound: keep it off the executor's
        // async threads.
        let report: BacktestReport = tokio::task::spawn_blocking(move || {
            let scenarios = synthetic_scenarios(seed, scenarios);
            // Every scenario gets the same entry size: the run's per-trade
            // budget. Dataset generation owns the price paths; the run
            // parameters own sizing and costs.
            let scenarios = scenarios
                .into_iter()
                .map(|mut s| {
                    s.entry_quote_lamports = entry_quote_lamports;
                    s
                })
                .collect::<Vec<_>>();
            run_backtest(&scenarios, &fill_cfg, &exit_cfg)
        })
        .await
        .map_err(|e| format!("backtest simulator panicked or was cancelled: {e}"))?;

        Ok(result_json_for(run, &report, initial_usd, initial_sol, reference_cents))
    }
}

/// `result_json` exactly in the shape `backtests.rs::run_json` reads, plus
/// the honesty fields (label, reference price, caveats).
fn result_json_for(
    run: &ClaimedRun,
    report: &BacktestReport,
    initial_usd: f64,
    initial_sol: f64,
    reference_cents: u64,
) -> Value {
    let sol_price_usd = reference_cents as f64 / 100.0;
    let net_pnl_sol = report.metrics.net_pnl_sol;
    let net_pnl_usd = net_pnl_sol * sol_price_usd;
    let net_roi_pct = if initial_usd > 0.0 {
        net_pnl_usd / initial_usd * 100.0
    } else {
        0.0
    };
    let max_drawdown_pct = if initial_sol > 0.0 {
        report.metrics.max_drawdown_sol / initial_sol * 100.0
    } else {
        0.0
    };
    json!({
        "label": "synthetic",
        "note": "SYNTHETIC backtest: scenarios are generated from the run id by a seeded PRNG; no recorded market data exists for this period. The USD basis is an operator-pinned reference price, not a live market price.",
        "reference_sol_usd_cents": reference_cents,
        "final_balance_usd": initial_usd + net_pnl_usd,
        "net_pnl_usd": net_pnl_usd,
        "net_pnl_sol": net_pnl_sol,
        "net_roi_pct": net_roi_pct,
        "max_drawdown_pct": max_drawdown_pct,
        "total_trades": report.metrics.trades,
        "missed_entries": report.missed,
        "win_rate_pct": report.metrics.win_rate * 100.0,
        "sharpe_ratio": report.metrics.sharpe,
        "best_pnl_sol": report.metrics.best_pnl_sol,
        "worst_pnl_sol": report.metrics.worst_pnl_sol,
        "model_caveats": report.model_caveats,
        "simulated_scenarios": report.scenarios,
    })
}

/// Deterministic seed from the run id — the same queued row always
/// reproduces the same synthetic dataset.
fn seed_of(run_id: Uuid) -> u64 {
    let bytes = run_id.as_bytes();
    u64::from_le_bytes(bytes[0..8].try_into().expect("uuid is 16 bytes")).max(1)
}

/// Period length → scenario count, clamped to the configured budget.
fn scenario_budget(cfg: &BacktestWorkerConfig, start: DateTime<Utc>, end: DateTime<Utc>) -> usize {
    // Ceiling division by hand: `i64::div_ceil` is still unstable.
    let seconds = (end - start).num_seconds().max(0);
    let days = (seconds.saturating_add(86_399) / 86_400) as usize;
    let want = days.saturating_mul(4).max(cfg.min_scenarios);
    want.min(cfg.max_scenarios)
}

/// The operator-pinned SOL→USD reference price for synthetic results.
fn reference_sol_usd_cents() -> u64 {
    match std::env::var(REFERENCE_PRICE_ENV) {
        Ok(raw) => match raw.trim().parse::<u64>() {
            Ok(v) if v > 0 => v,
            _ => {
                tracing::warn!(
                    value = %raw,
                    "{REFERENCE_PRICE_ENV} is not a positive integer cents value — using default"
                );
                DEFAULT_REFERENCE_SOL_USD_CENTS
            }
        },
        Err(_) => DEFAULT_REFERENCE_SOL_USD_CENTS,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seed_is_deterministic_and_nonzero() {
        let id = Uuid::parse_str("00000000-0000-0000-0000-000000000001").unwrap();
        assert_eq!(seed_of(id), seed_of(id));
        assert_eq!(seed_of(id), 1);
        let zero = Uuid::nil();
        assert_eq!(seed_of(zero), 1, "nil uuid must not seed the PRNG with 0");
    }

    #[test]
    fn budget_scales_with_period_and_clamps() {
        let cfg = BacktestWorkerConfig::default();
        let start = Utc::now();
        // One day → 4 scenarios, raised to the floor.
        assert_eq!(scenario_budget(&cfg, start, start + chrono::Duration::days(1)), 8);
        // Thirty days → 120.
        assert_eq!(scenario_budget(&cfg, start, start + chrono::Duration::days(30)), 120);
        // A decade clamps to the ceiling.
        assert_eq!(scenario_budget(&cfg, start, start + chrono::Duration::days(3650)), 200);
        // Inverted periods floor, never panic.
        assert_eq!(scenario_budget(&cfg, start, start - chrono::Duration::days(5)), 8);
    }

    #[test]
    fn result_json_carries_the_contract_keys_and_the_label() {
        let scenarios = synthetic_scenarios(7, 12);
        let report = run_backtest(
            &scenarios,
            &FillModelConfig::default(),
            &ExitConfig::default(),
        );
        let run = ClaimedRun {
            id: Uuid::new_v4(),
            period_start: Utc::now(),
            period_end: Utc::now() + chrono::Duration::days(3),
            initial_balance_usd_cents: 100_000,
            fee_rate_bps: 25,
            slippage_bps: 50,
        };
        let value = result_json_for(&run, &report, 1000.0, 1000.0 / 150.0, 15_000);
        assert_eq!(value["label"], "synthetic");
        for key in [
            "final_balance_usd",
            "net_pnl_usd",
            "net_roi_pct",
            "max_drawdown_pct",
            "total_trades",
            "win_rate_pct",
            "sharpe_ratio",
        ] {
            assert!(value.get(key).is_some(), "missing {key}");
        }
        assert_eq!(value["reference_sol_usd_cents"], 15_000);
        assert!(value["note"].as_str().unwrap().contains("SYNTHETIC"));
    }
}
