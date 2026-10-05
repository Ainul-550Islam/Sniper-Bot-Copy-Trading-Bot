"use client";

/**
 * Historical Backtesting & Strategy Simulation Console (SECOND.md §52).
 *
 * Deterministic backtesting harness against Raydium, Pump.fun, and Polymarket
 * historical tick feeds with Sharpe ratio and drawdown analytics.
 */

import { useCallback, useEffect, useState } from "react";
import AppShell from "@/components/AppShell";
import BacktestRunner from "@/components/backtest/backtest-runner";
import BacktestTable from "@/components/backtest/backtest-table";
import { BacktestRecord, listBacktests } from "@/lib/api/backtest-api";
import { StrategyRecord, listStrategies } from "@/lib/api/strategy-api";

export default function BacktestsPage() {
  const [backtests, setBacktests] = useState<BacktestRecord[]>([]);
  const [strategies, setStrategies] = useState<StrategyRecord[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [showRunner, setShowRunner] = useState(false);

  const loadData = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      const [bts, strats] = await Promise.all([listBacktests(), listStrategies()]);
      setBacktests(bts);
      setStrategies(strats);
    } catch (err: unknown) {
      setError(err instanceof Error ? err.message : "Failed to load backtest data");
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    void loadData();
  }, [loadData]);

  return (
    <AppShell title="Strategy Backtesting">
      <div style={{ display: "flex", justifyContent: "space-between", alignItems: "center", marginBottom: "1.5rem" }}>
        <div>
          <h1 style={{ margin: 0 }}>Strategy Backtesting &amp; Simulation</h1>
          <p style={{ margin: "0.25rem 0 0", color: "var(--muted)", fontSize: "0.9rem" }}>
            Queue tenant-owned backtest jobs for worker execution over an available authoritative historical dataset.
          </p>
        </div>
        <button onClick={() => setShowRunner(!showRunner)} className="btn btn-primary">
          {showRunner ? "Close Runner" : "+ New Simulation"}
        </button>
      </div>

      {showRunner && (
        <BacktestRunner
          strategies={strategies}
          onComplete={() => {
            setShowRunner(false);
            void loadData();
          }}
          onCancel={() => setShowRunner(false)}
        />
      )}

      {error && (
        <div className="card" style={{ color: "var(--bad)", background: "var(--bad-glow)", marginBottom: "1.5rem" }}>
          {error}
        </div>
      )}

      <BacktestTable items={backtests} loading={loading} onRefresh={() => void loadData()} />
    </AppShell>
  );
}
