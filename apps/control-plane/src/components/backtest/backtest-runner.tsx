"use client";

import { useState } from "react";
import { StrategyRecord } from "@/lib/api/strategy-api";
import { BacktestRunInput, runBacktest } from "@/lib/api/backtest-api";

interface BacktestRunnerProps {
  strategies: StrategyRecord[];
  onComplete: () => void;
  onCancel: () => void;
}

export default function BacktestRunner({ strategies, onComplete, onCancel }: BacktestRunnerProps) {
  const [strategyId, setStrategyId] = useState(strategies[0]?.id || "");
  const [venue, setVenue] = useState("raydium_v4");
  const [initialBalanceUsd, setInitialBalanceUsd] = useState(10000);
  const [feeRateBps, setFeeRateBps] = useState(25);
  const [slippageBps, setSlippageBps] = useState(50);
  const [daysBack, setDaysBack] = useState(30);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const handleSubmit = async (e: React.FormEvent) => {
    e.preventDefault();
    if (!strategyId) {
      setError("Please select a strategy to backtest");
      return;
    }

    setLoading(true);
    setError(null);

    const now = new Date();
    const periodEnd = now.toISOString();
    const periodStart = new Date(now.getTime() - daysBack * 24 * 60 * 60 * 1000).toISOString();

    try {
      const payload: BacktestRunInput = {
        strategy_id: strategyId,
        period_start: periodStart,
        period_end: periodEnd,
        venue,
        initial_balance_usd: initialBalanceUsd,
        fee_rate_bps: feeRateBps,
        slippage_bps: slippageBps,
      };

      await runBacktest(payload);
      onComplete();
    } catch (err: unknown) {
      setError(err instanceof Error ? err.message : "Failed to trigger backtest simulation");
    } finally {
      setLoading(false);
    }
  };

  return (
    <div className="card" style={{ marginBottom: "1.5rem" }}>
      <h3 style={{ marginTop: 0 }}>Configure Backtest Simulation</h3>
      {error && (
        <div style={{ color: "var(--bad)", background: "var(--bad-glow)", padding: "0.5rem", borderRadius: "4px", marginBottom: "1rem" }}>
          {error}
        </div>
      )}

      <form onSubmit={handleSubmit}>
        <div style={{ display: "grid", gridTemplateColumns: "1fr 1fr", gap: "1rem", marginBottom: "1rem" }}>
          <div>
            <label style={{ display: "block", fontSize: "0.85rem", marginBottom: "0.25rem", color: "var(--muted)" }}>
              Target Strategy
            </label>
            <select
              value={strategyId}
              onChange={(e) => setStrategyId(e.target.value)}
              className="select"
              style={{ width: "100%" }}
              required
            >
              {strategies.map((s) => (
                <option key={s.id} value={s.id}>
                  {s.name} ({s.module_family})
                </option>
              ))}
              {strategies.length === 0 && <option value="">No strategies available</option>}
            </select>
          </div>

          <div>
            <label style={{ display: "block", fontSize: "0.85rem", marginBottom: "0.25rem", color: "var(--muted)" }}>
              Execution Venue
            </label>
            <select
              value={venue}
              onChange={(e) => setVenue(e.target.value)}
              className="select"
              style={{ width: "100%" }}
            >
              <option value="raydium_v4">Raydium v4 AMM</option>
              <option value="raydium_clmm">Raydium Concentrated Liquidity (CLMM)</option>
              <option value="pumpfun">Pump.fun Bonding Curve</option>
              <option value="orca_whirlpools">Orca Whirlpools</option>
              <option value="polymarket_clob">Polymarket CTF CLOB</option>
            </select>
          </div>
        </div>

        <div style={{ display: "grid", gridTemplateColumns: "1fr 1fr 1fr 1fr", gap: "1rem", marginBottom: "1rem" }}>
          <div>
            <label style={{ display: "block", fontSize: "0.85rem", marginBottom: "0.25rem", color: "var(--muted)" }}>
              Initial Capital (USD)
            </label>
            <input
              type="number"
              min="100"
              value={initialBalanceUsd}
              onChange={(e) => setInitialBalanceUsd(Number(e.target.value))}
              className="input"
              style={{ width: "100%" }}
            />
          </div>

          <div>
            <label style={{ display: "block", fontSize: "0.85rem", marginBottom: "0.25rem", color: "var(--muted)" }}>
              Historical Window
            </label>
            <select
              value={daysBack}
              onChange={(e) => setDaysBack(Number(e.target.value))}
              className="select"
              style={{ width: "100%" }}
            >
              <option value={7}>Last 7 Days</option>
              <option value={30}>Last 30 Days</option>
              <option value={90}>Last 90 Days</option>
              <option value={180}>Last 180 Days</option>
            </select>
          </div>

          <div>
            <label style={{ display: "block", fontSize: "0.85rem", marginBottom: "0.25rem", color: "var(--muted)" }}>
              Fee Rate (BPS)
            </label>
            <input
              type="number"
              min="0"
              max="1000"
              value={feeRateBps}
              onChange={(e) => setFeeRateBps(Number(e.target.value))}
              className="input"
              style={{ width: "100%" }}
            />
          </div>

          <div>
            <label style={{ display: "block", fontSize: "0.85rem", marginBottom: "0.25rem", color: "var(--muted)" }}>
              Slippage (BPS)
            </label>
            <input
              type="number"
              min="0"
              max="2000"
              value={slippageBps}
              onChange={(e) => setSlippageBps(Number(e.target.value))}
              className="input"
              style={{ width: "100%" }}
            />
          </div>
        </div>

        <div style={{ display: "flex", justifyContent: "flex-end", gap: "0.5rem" }}>
          <button type="button" onClick={onCancel} className="btn btn-secondary">
            Cancel
          </button>
          <button type="submit" disabled={loading || strategies.length === 0} className="btn btn-primary">
            {loading ? "Running Simulation..." : "Execute Simulation"}
          </button>
        </div>
      </form>
    </div>
  );
}
