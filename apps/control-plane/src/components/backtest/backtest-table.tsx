"use client";

import { BacktestRecord } from "@/lib/api/backtest-api";

interface BacktestTableProps {
  items: BacktestRecord[];
  loading: boolean;
  onRefresh: () => void;
}

export default function BacktestTable({ items, loading, onRefresh }: BacktestTableProps) {
  if (loading) {
    return <div className="card">Loading backtests...</div>;
  }

  if (items.length === 0) {
    return (
      <div className="card" style={{ textAlign: "center", padding: "2rem" }}>
        <p style={{ color: "var(--muted)", margin: 0 }}>
          No backtest simulations executed yet. Run a historical simulation to analyze Sharpe ratio and max drawdown.
        </p>
      </div>
    );
  }

  return (
    <div className="card" style={{ padding: 0, overflow: "hidden" }}>
      <div style={{ display: "flex", justifyContent: "space-between", alignItems: "center", padding: "1rem" }}>
        <h3 style={{ margin: 0 }}>Historical Backtest Runs ({items.length})</h3>
        <button onClick={onRefresh} className="btn btn-secondary" style={{ fontSize: "0.8rem", padding: "0.25rem 0.5rem" }}>
          Refresh
        </button>
      </div>

      <div style={{ overflowX: "auto" }}>
        <table className="table" style={{ width: "100%", borderCollapse: "collapse" }}>
          <thead>
            <tr style={{ background: "rgba(255,255,255,0.02)", textAlign: "left" }}>
              <th style={{ padding: "0.75rem 1rem" }}>Strategy / Venue</th>
              <th style={{ padding: "0.75rem 1rem" }}>Period</th>
              <th style={{ padding: "0.75rem 1rem" }}>Initial / Final</th>
              <th style={{ padding: "0.75rem 1rem" }}>Net PnL</th>
              <th style={{ padding: "0.75rem 1rem" }}>Max DD</th>
              <th style={{ padding: "0.75rem 1rem" }}>Win Rate</th>
              <th style={{ padding: "0.75rem 1rem" }}>Sharpe</th>
              <th style={{ padding: "0.75rem 1rem" }}>Status</th>
            </tr>
          </thead>
          <tbody>
            {items.map((item) => {
              const pnl = item.net_pnl_usd ?? 0;
              const isProfit = pnl >= 0;
              return (
                <tr key={item.id} style={{ borderTop: "1px solid var(--line)" }}>
                  <td style={{ padding: "0.75rem 1rem" }}>
                    <strong>{item.strategy_name}</strong>
                    <div style={{ fontSize: "0.75rem", color: "var(--muted)" }}>{item.venue}</div>
                  </td>
                  <td style={{ padding: "0.75rem 1rem", fontSize: "0.8rem" }}>
                    <div>{new Date(item.period_start).toLocaleDateString()}</div>
                    <div style={{ color: "var(--muted)" }}>to {new Date(item.period_end).toLocaleDateString()}</div>
                  </td>
                  <td style={{ padding: "0.75rem 1rem", fontSize: "0.85rem" }}>
                    <div>${item.initial_balance_usd.toLocaleString()}</div>
                    <div style={{ color: "var(--muted)" }}>
                      ${(item.final_balance_usd ?? item.initial_balance_usd).toLocaleString()}
                    </div>
                  </td>
                  <td style={{ padding: "0.75rem 1rem" }}>
                    <span style={{ color: isProfit ? "var(--ok)" : "var(--bad)", fontWeight: 600 }}>
                      {isProfit ? "+" : ""}${pnl.toFixed(2)} ({item.net_roi_pct ? `${item.net_roi_pct}%` : "0%"})
                    </span>
                  </td>
                  <td style={{ padding: "0.75rem 1rem", color: "var(--bad)" }}>
                    {item.max_drawdown_pct ? `${item.max_drawdown_pct}%` : "0.0%"}
                  </td>
                  <td style={{ padding: "0.75rem 1rem" }}>
                    {item.win_rate_pct ? `${item.win_rate_pct}%` : "—"} ({item.total_trades ?? 0} trades)
                  </td>
                  <td style={{ padding: "0.75rem 1rem", fontWeight: 600 }}>
                    {item.sharpe_ratio ?? "—"}
                  </td>
                  <td style={{ padding: "0.75rem 1rem" }}>
                    <span
                      className={`badge ${
                        item.status === "completed"
                          ? "badge-ok"
                          : item.status === "running"
                          ? "badge-warn"
                          : "badge-bad"
                      }`}
                    >
                      {item.status.toUpperCase()}
                    </span>
                  </td>
                </tr>
              );
            })}
          </tbody>
        </table>
      </div>
    </div>
  );
}
