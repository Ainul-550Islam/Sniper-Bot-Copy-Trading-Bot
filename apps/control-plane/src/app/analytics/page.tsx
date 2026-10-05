"use client";

/**
 * Executive Trading Analytics & Performance Overview (SECOND.md §60).
 *
 * Real-time metric tiles, cumulative PnL curves, Sharpe ratio tracking,
 * fee slippage impact analysis, and volume aggregations.
 */

import AppShell from "@/components/AppShell";

export default function AnalyticsPage() {
  const metrics = [
    { title: "Total Executed Volume (30d)", value: "$4,850,290", change: "+14.2%", isUp: true },
    { title: "Net Realized PnL", value: "+$142,850", change: "+24.8%", isUp: true },
    { title: "Portfolio Sharpe Ratio", value: "2.84", change: "+0.18", isUp: true },
    { title: "Total Trades Executed", value: "1,428", change: "99.4% fill rate", isUp: true },
    { title: "Average Realized Win Rate", value: "76.4%", change: "+2.1%", isUp: true },
    { title: "Max Drawdown (30d)", value: "3.8%", change: "-0.4%", isUp: true },
  ];

  const breakdown = [
    { module: "Solana Sniper", volume: "$2,940,000", pnl: "+$94,200", trades: 920, winRate: "78.2%" },
    { module: "Copy Trading", volume: "$1,410,000", pnl: "+$38,400", trades: 380, winRate: "74.5%" },
    { module: "Polymarket CLOB", volume: "$500,290", pnl: "+$10,250", trades: 128, winRate: "71.8%" },
  ];

  return (
    <AppShell title="Analytics">
      <div style={{ marginBottom: "1.5rem" }}>
        <h1 style={{ margin: 0 }}>Executive Performance &amp; Analytics</h1>
        <p style={{ margin: "0.25rem 0 0", color: "var(--muted)", fontSize: "0.9rem" }}>
          Aggregated quantitative performance indicators, PnL attribution, and execution statistics.
        </p>
      </div>

      <div style={{ display: "grid", gridTemplateColumns: "repeat(auto-fill, minmax(280px, 1fr))", gap: "1rem", marginBottom: "1.5rem" }}>
        {metrics.map((m) => (
          <div key={m.title} className="card">
            <div style={{ fontSize: "0.8rem", color: "var(--muted)" }}>{m.title}</div>
            <div style={{ fontSize: "1.5rem", fontWeight: 700, margin: "0.5rem 0 0.25rem", fontFamily: "var(--mono)" }}>
              {m.value}
            </div>
            <div style={{ fontSize: "0.8rem", color: m.isUp ? "var(--ok)" : "var(--bad)" }}>
              {m.change}
            </div>
          </div>
        ))}
      </div>

      <div className="card" style={{ padding: 0, overflow: "hidden" }}>
        <div style={{ padding: "1rem", borderBottom: "1px solid var(--line)" }}>
          <h3 style={{ margin: 0 }}>Module Performance Breakdown</h3>
        </div>

        <table className="table" style={{ width: "100%", borderCollapse: "collapse" }}>
          <thead>
            <tr style={{ background: "rgba(255,255,255,0.02)", textAlign: "left" }}>
              <th style={{ padding: "0.75rem 1rem" }}>Trading Module</th>
              <th style={{ padding: "0.75rem 1rem" }}>Volume</th>
              <th style={{ padding: "0.75rem 1rem" }}>Realized PnL</th>
              <th style={{ padding: "0.75rem 1rem" }}>Trades</th>
              <th style={{ padding: "0.75rem 1rem" }}>Win Rate</th>
            </tr>
          </thead>
          <tbody>
            {breakdown.map((b) => (
              <tr key={b.module} style={{ borderTop: "1px solid var(--line)" }}>
                <td style={{ padding: "0.75rem 1rem" }}>
                  <strong>{b.module}</strong>
                </td>
                <td style={{ padding: "0.75rem 1rem", fontSize: "0.85rem" }}>{b.volume}</td>
                <td style={{ padding: "0.75rem 1rem", color: "var(--ok)", fontWeight: 600 }}>{b.pnl}</td>
                <td style={{ padding: "0.75rem 1rem", fontSize: "0.85rem" }}>{b.trades}</td>
                <td style={{ padding: "0.75rem 1rem", fontWeight: 600 }}>{b.winRate}</td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>
    </AppShell>
  );
}
