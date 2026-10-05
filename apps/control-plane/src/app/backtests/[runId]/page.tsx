"use client";

/**
 * Backtest Simulation Detail Page (THIRD.md §105).
 */

import { useCallback, useEffect, useState } from "react";
import { useParams } from "next/navigation";
import AppShell from "@/components/AppShell";
import BacktestMetrics from "@/components/backtest/BacktestMetrics";
import { ErrorState } from "@/components/common/ErrorState";
import { BacktestRecord, getBacktest } from "@/lib/api/backtest-api";

export default function BacktestDetailPage() {
  const params = useParams();
  const runId = typeof params?.runId === "string" ? params.runId : "";
  const [backtest, setBacktest] = useState<BacktestRecord | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  const load = useCallback(async () => {
    if (!runId) return;
    setLoading(true);
    setError(null);
    try {
      const data = await getBacktest(runId);
      setBacktest(data);
    } catch (err: unknown) {
      setError(err instanceof Error ? err.message : "Failed to load backtest result");
    } finally {
      setLoading(false);
    }
  }, [runId]);

  useEffect(() => {
    void load();
  }, [load]);

  return (
    <AppShell title="Backtest Run Details">
      <div style={{ marginBottom: "1.5rem" }}>
        <h1 style={{ margin: 0 }}>Simulation Run: {backtest?.strategy_name || runId}</h1>
        <p style={{ margin: "0.25rem 0 0", color: "var(--muted)", fontSize: "0.9rem" }}>
          Deterministic historical replay metrics, slippage assumptions, and Sharpe analysis.
        </p>
      </div>

      {error && <ErrorState error={error} onRetry={() => void load()} />}

      {loading ? (
        <div className="card">Loading simulation data...</div>
      ) : backtest ? (
        <div>
          <BacktestMetrics backtest={backtest} />

          <div className="card">
            <h3 style={{ marginTop: 0 }}>Simulation Assumptions &amp; Dataset</h3>
            <div style={{ display: "grid", gridTemplateColumns: "repeat(auto-fill, minmax(200px, 1fr))", gap: "1rem" }}>
              <div>
                <div style={{ fontSize: "0.75rem", color: "var(--muted)" }}>Venue</div>
                <div style={{ fontWeight: 600, marginTop: "0.2rem" }}>{backtest.venue}</div>
              </div>
              <div>
                <div style={{ fontSize: "0.75rem", color: "var(--muted)" }}>Execution Fee Rate</div>
                <div style={{ fontWeight: 600, marginTop: "0.2rem" }}>{backtest.fee_rate_bps} BPS</div>
              </div>
              <div>
                <div style={{ fontSize: "0.75rem", color: "var(--muted)" }}>Slippage Model</div>
                <div style={{ fontWeight: 600, marginTop: "0.2rem" }}>{backtest.slippage_bps} BPS</div>
              </div>
              <div>
                <div style={{ fontSize: "0.75rem", color: "var(--muted)" }}>Status</div>
                <div style={{ fontWeight: 600, marginTop: "0.2rem", textTransform: "uppercase" }}>{backtest.status}</div>
              </div>
            </div>
          </div>
        </div>
      ) : null}
    </AppShell>
  );
}
