"use client";

/**
 * Tenant analytics view.
 *
 * All values come from the authenticated tenant analytics endpoint. Missing
 * authoritative metrics are rendered as unavailable rather than replaced with
 * sample numbers.
 */

import { useCallback, useEffect, useState } from "react";
import AppShell from "@/components/AppShell";
import { toDisplayError } from "@/lib/api";
import { customerTrading, type AnalyticsSummary } from "@/lib/customer-trading-api";

type Timeframe = "24h" | "7d" | "30d" | "all";

function formatMoney(value: number | null): string {
  return value === null ? "Unavailable" : `$${value.toLocaleString(undefined, { minimumFractionDigits: 2, maximumFractionDigits: 2 })}`;
}

function formatPercent(value: number | null): string {
  return value === null ? "Unavailable" : `${value.toFixed(2)}%`;
}

function formatNumber(value: number | null): string {
  return value === null ? "Unavailable" : value.toLocaleString(undefined, { maximumFractionDigits: 2 });
}

export default function AnalyticsPage() {
  const [timeframe, setTimeframe] = useState<Timeframe>("7d");
  const [analytics, setAnalytics] = useState<AnalyticsSummary | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  const loadAnalytics = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      setAnalytics(await customerTrading.analytics(timeframe));
    } catch (err: unknown) {
      setAnalytics(null);
      setError(toDisplayError(err));
    } finally {
      setLoading(false);
    }
  }, [timeframe]);

  useEffect(() => {
    void loadAnalytics();
  }, [loadAnalytics]);

  const metrics = analytics
    ? [
        { title: "Executed Volume", value: formatMoney(analytics.total_volume_usd), detail: timeframe },
        { title: "Realized PnL", value: formatMoney(analytics.realized_pnl_usd), detail: "Authoritative accounting" },
        { title: "Unrealized PnL", value: formatMoney(analytics.unrealized_pnl_usd), detail: "Open positions" },
        { title: "Trades", value: formatNumber(analytics.total_trades), detail: "Recorded trade rows" },
        { title: "Win Rate", value: formatPercent(analytics.win_rate_pct), detail: "Only shown when sourced" },
        { title: "Max Drawdown", value: formatPercent(analytics.max_drawdown_pct), detail: "Portfolio snapshots" },
      ]
    : [];

  return (
    <AppShell title="Analytics">
      <div style={{ display: "flex", justifyContent: "space-between", alignItems: "flex-start", gap: "1rem", flexWrap: "wrap", marginBottom: "1.5rem" }}>
        <div>
          <h1 style={{ margin: 0 }}>Trading Analytics</h1>
          <p style={{ margin: "0.25rem 0 0", color: "var(--muted)", fontSize: "0.9rem" }}>
            Tenant-scoped metrics calculated from persisted accounting, trade, execution, position, and portfolio data.
          </p>
        </div>
        <label style={{ color: "var(--muted)", fontSize: "0.85rem" }}>
          Timeframe{" "}
          <select
            value={timeframe}
            onChange={(event) => setTimeframe(event.target.value as Timeframe)}
            className="select"
            style={{ marginLeft: "0.5rem" }}
          >
            <option value="24h">24 hours</option>
            <option value="7d">7 days</option>
            <option value="30d">30 days</option>
            <option value="all">All time</option>
          </select>
        </label>
      </div>

      {error && (
        <div className="card" style={{ color: "var(--bad)", background: "var(--bad-glow)", marginBottom: "1.5rem" }}>
          <div>{error}</div>
          <button type="button" onClick={() => void loadAnalytics()} className="btn btn-secondary" style={{ marginTop: "0.75rem" }}>
            Retry
          </button>
        </div>
      )}

      {loading ? (
        <div className="card">Loading authoritative analytics...</div>
      ) : !analytics ? (
        <div className="card" style={{ color: "var(--muted)" }}>
          Analytics are unavailable for the selected tenant and timeframe.
        </div>
      ) : (
        <>
          <div style={{ display: "grid", gridTemplateColumns: "repeat(auto-fill, minmax(220px, 1fr))", gap: "1rem", marginBottom: "1.5rem" }}>
            {metrics.map((metric) => (
              <div key={metric.title} className="card">
                <div style={{ fontSize: "0.8rem", color: "var(--muted)" }}>{metric.title}</div>
                <div style={{ fontSize: "1.35rem", fontWeight: 700, margin: "0.5rem 0 0.25rem", fontFamily: "var(--mono)" }}>
                  {metric.value}
                </div>
                <div style={{ fontSize: "0.75rem", color: "var(--muted)" }}>{metric.detail}</div>
              </div>
            ))}
          </div>

          <div className="card" style={{ marginBottom: "1.5rem" }}>
            <h3 style={{ marginTop: 0 }}>Execution Quality</h3>
            <div style={{ display: "grid", gridTemplateColumns: "repeat(auto-fit, minmax(180px, 1fr))", gap: "1rem" }}>
              <div><span className="muted small">Average fill time</span><br /><strong>{analytics.execution_quality.avg_fill_time_ms === null ? "Unavailable" : `${formatNumber(analytics.execution_quality.avg_fill_time_ms)} ms`}</strong></div>
              <div><span className="muted small">Average slippage</span><br /><strong>{analytics.execution_quality.avg_slippage_bps === null ? "Unavailable" : `${analytics.execution_quality.avg_slippage_bps.toFixed(2)} bps`}</strong></div>
              <div><span className="muted small">Failed attempts</span><br /><strong>{formatPercent(analytics.execution_quality.failed_attempts_pct)}</strong></div>
              <div><span className="muted small">Failed or reverted records</span><br /><strong>{analytics.execution_quality.reverted_txs.toLocaleString()}</strong></div>
            </div>
          </div>

          <div className="card" style={{ padding: 0, overflow: "hidden", marginBottom: "1.5rem" }}>
            <div style={{ padding: "1rem", borderBottom: "1px solid var(--line)" }}>
              <h3 style={{ margin: 0 }}>Module Performance Breakdown</h3>
            </div>
            {analytics.module_breakdown.length === 0 ? (
              <div style={{ padding: "2rem", textAlign: "center", color: "var(--muted)" }}>No module trade data exists for this timeframe.</div>
            ) : (
              <table className="table" style={{ width: "100%", borderCollapse: "collapse" }}>
                <thead>
                  <tr style={{ background: "rgba(255,255,255,0.02)", textAlign: "left" }}>
                    <th style={{ padding: "0.75rem 1rem" }}>Module</th>
                    <th style={{ padding: "0.75rem 1rem" }}>Trades</th>
                    <th style={{ padding: "0.75rem 1rem" }}>Volume</th>
                    <th style={{ padding: "0.75rem 1rem" }}>PnL</th>
                    <th style={{ padding: "0.75rem 1rem" }}>Win Rate</th>
                  </tr>
                </thead>
                <tbody>
                  {analytics.module_breakdown.map((item) => (
                    <tr key={item.module} style={{ borderTop: "1px solid var(--line)" }}>
                      <td style={{ padding: "0.75rem 1rem" }}><strong>{item.module}</strong></td>
                      <td style={{ padding: "0.75rem 1rem" }}>{item.trades_count.toLocaleString()}</td>
                      <td style={{ padding: "0.75rem 1rem" }}>{formatMoney(item.volume_usd)}</td>
                      <td style={{ padding: "0.75rem 1rem" }}>{formatMoney(item.pnl_usd)}</td>
                      <td style={{ padding: "0.75rem 1rem" }}>{formatPercent(item.win_rate_pct)}</td>
                    </tr>
                  ))}
                </tbody>
              </table>
            )}
          </div>

          <div className="card" style={{ padding: 0, overflow: "hidden" }}>
            <div style={{ padding: "1rem", borderBottom: "1px solid var(--line)" }}>
              <h3 style={{ margin: 0 }}>Daily PnL Series</h3>
            </div>
            {analytics.pnl_series.length === 0 ? (
              <div style={{ padding: "2rem", textAlign: "center", color: "var(--muted)" }}>No accounting rows exist for this timeframe.</div>
            ) : (
              <table className="table" style={{ width: "100%", borderCollapse: "collapse" }}>
                <thead>
                  <tr style={{ background: "rgba(255,255,255,0.02)", textAlign: "left" }}>
                    <th style={{ padding: "0.75rem 1rem" }}>Date</th>
                    <th style={{ padding: "0.75rem 1rem" }}>Daily PnL</th>
                    <th style={{ padding: "0.75rem 1rem" }}>Cumulative PnL</th>
                  </tr>
                </thead>
                <tbody>
                  {analytics.pnl_series.map((point) => (
                    <tr key={point.date} style={{ borderTop: "1px solid var(--line)" }}>
                      <td style={{ padding: "0.75rem 1rem" }}>{point.date}</td>
                      <td style={{ padding: "0.75rem 1rem" }}>{formatMoney(point.pnl_usd)}</td>
                      <td style={{ padding: "0.75rem 1rem" }}>{formatMoney(point.cumulative_usd)}</td>
                    </tr>
                  ))}
                </tbody>
              </table>
            )}
          </div>
        </>
      )}
    </AppShell>
  );
}
