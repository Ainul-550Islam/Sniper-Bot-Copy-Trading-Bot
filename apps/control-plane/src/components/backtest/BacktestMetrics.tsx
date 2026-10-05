"use client";

import { BacktestRecord } from "@/lib/api/backtest-api";
import { formatPercentage, formatUsdCents } from "@/lib/formatters/financial";

interface BacktestMetricsProps {
  backtest: BacktestRecord;
}

export function BacktestMetrics({ backtest }: BacktestMetricsProps) {
  const pnl = backtest.net_pnl_usd;
  const isUnavailable = pnl === null;
  const isProfitable = !isUnavailable && pnl >= 0;
  const metricColor = isUnavailable ? "var(--muted)" : isProfitable ? "var(--ok)" : "var(--bad)";

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
            color: metricColor,
          }}
        >
          {isUnavailable ? "Unavailable" : formatUsdCents((pnl * 100).toFixed(0), { showSign: true })}
        </div>
        <div style={{ fontSize: "0.8rem", color: metricColor }}>
          ROI: {backtest.net_roi_pct === null ? "Unavailable" : formatPercentage(backtest.net_roi_pct, { showSign: true })}
        </div>
      </div>

      <div className="card">
        <div style={{ fontSize: "0.8rem", color: "var(--muted)" }}>Sharpe Ratio</div>
        <div style={{ fontSize: "1.5rem", fontWeight: 700, margin: "0.4rem 0 0.2rem", fontFamily: "var(--mono)" }}>
          {backtest.sharpe_ratio ?? "Unavailable"}
        </div>
        <div style={{ fontSize: "0.8rem", color: "var(--muted)" }}>Only available after a worker result</div>
      </div>

      <div className="card">
        <div style={{ fontSize: "0.8rem", color: "var(--muted)" }}>Max Drawdown</div>
        <div style={{ fontSize: "1.5rem", fontWeight: 700, margin: "0.4rem 0 0.2rem", fontFamily: "var(--mono)", color: "var(--bad)" }}>
          {backtest.max_drawdown_pct === null ? "Unavailable" : formatPercentage(backtest.max_drawdown_pct)}
        </div>
        <div style={{ fontSize: "0.8rem", color: "var(--muted)" }}>Only available after a worker result</div>
      </div>

      <div className="card">
        <div style={{ fontSize: "0.8rem", color: "var(--muted)" }}>Trade Win Rate</div>
        <div style={{ fontSize: "1.5rem", fontWeight: 700, margin: "0.4rem 0 0.2rem", fontFamily: "var(--mono)", color: "var(--ok)" }}>
          {backtest.win_rate_pct === null ? "Unavailable" : `${backtest.win_rate_pct}%`}
        </div>
        <div style={{ fontSize: "0.8rem", color: "var(--muted)" }}>
          {backtest.total_trades === null ? "Trade count unavailable" : `${backtest.total_trades} simulated round-trips`}
        </div>
      </div>
    </div>
  );
}

export default BacktestMetrics;
