"use client";

/**
 * Historical Backtesting & Strategy Simulation Console (SECOND.md §52).
 *
 * Deterministic backtesting harness against Raydium, Pump.fun, and Polymarket
 * historical tick feeds with Sharpe ratio and drawdown analytics.
 */

import { useCallback, useEffect, useRef, useState } from "react";
import AppShell from "@/components/AppShell";
import BacktestRunner from "@/components/backtest/backtest-runner";
import BacktestTable from "@/components/backtest/backtest-table";
import { BacktestRecord, listBacktests } from "@/lib/api/backtest-api";
import { StrategyRecord, listStrategies } from "@/lib/api/strategy-api";
import { sessionStore } from "@/lib/auth";

export default function BacktestsPage() {
  const [backtests, setBacktests] = useState<BacktestRecord[]>([]);
  const [strategies, setStrategies] = useState<StrategyRecord[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [showRunner, setShowRunner] = useState(false);
  const [sessionVersion, setSessionVersion] = useState(0);
  const requestVersion = useRef(0);

  useEffect(() => sessionStore.subscribe(() => {
    requestVersion.current += 1;
    setSessionVersion((version) => version + 1);
    setBacktests([]);
    setStrategies([]);
    setShowRunner(false);
    setLoading(true);
    setError(null);
  }), []);

  const loadData = useCallback(async () => {
    const requestId = ++requestVersion.current;
    setBacktests([]);
    setStrategies([]);
    setLoading(true);
    setError(null);
    try {
      const [bts, strats] = await Promise.all([listBacktests(), listStrategies()]);
      if (requestId === requestVersion.current) {
        setBacktests(bts);
        setStrategies(strats);
      }
    } catch (err: unknown) {
      if (requestId === requestVersion.current) {
        setBacktests([]);
        setStrategies([]);
        setError(err instanceof Error ? err.message : "Failed to load backtest data");
      }
    } finally {
      if (requestId === requestVersion.current) setLoading(false);
    }
  }, []);

  useEffect(() => {
    let active = true;
    void Promise.resolve().then(() => {
      if (active) void loadData();
    });
    return () => {
      active = false;
      requestVersion.current += 1;
    };
  }, [loadData, sessionVersion]);

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
