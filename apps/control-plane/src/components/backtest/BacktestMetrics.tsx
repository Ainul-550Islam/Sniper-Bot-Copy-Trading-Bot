"use client";

import { BacktestRecord } from "@/lib/api/backtest-api";
import { formatPercentage, formatUsdCents } from "@/lib/formatters/financial";

interface BacktestMetricsProps {
  backtest: BacktestRecord;
}

export function BacktestMetrics({ backtest }: BacktestMetricsProps) {
  const pnl = backtest.net_pnl_usd ?? 0;
  const isProfitable = pnl >= 0;

  return (
    <div
      style={{
        display: "grid",
        gridTemplateColumns: "repeat(auto-fill, minmax(220px, 1fr))",
        gap: "1rem",
        marginBottom: "1.5rem",
      }}
    >
      <div className="card">
        <div style={{ fontSize: "0.8rem", color: "var(--muted)" }}>Net Simulated PnL</div>
        <div
          style={{
            fontSize: "1.5rem",
            fontWeight: 700,
            margin: "0.4rem 0 0.2rem",
            fontFamily: "var(--mono)",
            color: isProfitable ? "var(--ok)" : "var(--bad)",
          }}
        >
          {formatUsdCents((pnl * 100).toFixed(0), { showSign: true })}
        </div>
        <div style={{ fontSize: "0.8rem", color: isProfitable ? "var(--ok)" : "var(--bad)" }}>
          ROI: {formatPercentage(backtest.net_roi_pct ?? 0, { showSign: true })}
        </div>
      </div>

      <div className="card">
        <div style={{ fontSize: "0.8rem", color: "var(--muted)" }}>Sharpe Ratio</div>
        <div style={{ fontSize: "1.5rem", fontWeight: 700, margin: "0.4rem 0 0.2rem", fontFamily: "var(--mono)" }}>
          {backtest.sharpe_ratio ?? "—"}
        </div>
        <div style={{ fontSize: "0.8rem", color: "var(--muted)" }}>Risk-adjusted alpha score</div>
      </div>

      <div className="card">
        <div style={{ fontSize: "0.8rem", color: "var(--muted)" }}>Max Drawdown Peak</div>
        <div style={{ fontSize: "1.5rem", fontWeight: 700, margin: "0.4rem 0 0.2rem", fontFamily: "var(--mono)", color: "var(--bad)" }}>
          {formatPercentage(backtest.max_drawdown_pct ?? 0)}
        </div>
        <div style={{ fontSize: "0.8rem", color: "var(--muted)" }}>Max capital drawdown window</div>
      </div>

      <div className="card">
        <div style={{ fontSize: "0.8rem", color: "var(--muted)" }}>Trade Win Rate</div>
        <div style={{ fontSize: "1.5rem", fontWeight: 700, margin: "0.4rem 0 0.2rem", fontFamily: "var(--mono)", color: "var(--ok)" }}>
          {backtest.win_rate_pct ? `${backtest.win_rate_pct}%` : "—"}
        </div>
        <div style={{ fontSize: "0.8rem", color: "var(--muted)" }}>
          {backtest.total_trades ?? 0} simulated round-trips
        </div>
      </div>
    </div>
  );
}

export default BacktestMetrics;
