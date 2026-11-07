"use client";

import { useCallback, useEffect, useRef, useState } from "react";
import Link from "next/link";
import AppShell from "@/components/AppShell";
import { ErrorState } from "@/components/common/ErrorState";
import { toDisplayError } from "@/lib/api";
import { sessionStore } from "@/lib/auth";
import { tradingDetailApi, type TenantTrade } from "@/lib/api/trading-detail-api";

export default function TradesPage() {
  const [trades, setTrades] = useState<TenantTrade[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [sessionVersion, setSessionVersion] = useState(0);
  const requestVersion = useRef(0);

  useEffect(() => sessionStore.subscribe(() => {
    requestVersion.current += 1;
    setSessionVersion((version) => version + 1);
    setTrades([]);
    setLoading(true);
    setError(null);
  }), []);

  const load = useCallback(async () => {
    const requestId = ++requestVersion.current;
    setTrades([]);
    setLoading(true);
    setError(null);
    try {
      const response = await tradingDetailApi.trades();
      if (requestId === requestVersion.current) {
        setTrades(Array.isArray(response.items) ? response.items : []);
      }
    } catch (err: unknown) {
      if (requestId === requestVersion.current) {
        setTrades([]);
        setError(toDisplayError(err));
      }
    } finally {
      if (requestId === requestVersion.current) setLoading(false);
    }
  }, []);

  useEffect(() => {
    let active = true;
    void Promise.resolve().then(() => {
      if (active) void load();
    });
    return () => {
      active = false;
      requestVersion.current += 1;
    };
  }, [load, sessionVersion]);

  return (
    <AppShell title="Trades">
      <div style={{ display: "flex", justifyContent: "space-between", alignItems: "center", marginBottom: "1.5rem" }}>
        <div>
          <h1 style={{ margin: 0 }}>Trades</h1>
          <p style={{ margin: "0.25rem 0 0", color: "var(--muted)", fontSize: "0.9rem" }}>
            Execution fills recorded against the organization&apos;s positions, as
            returned by the tenant trading data plane.
          </p>
        </div>
        <button onClick={() => void load()} className="btn btn-secondary" style={{ fontSize: "0.85rem" }}>
          Refresh
        </button>
      </div>

      {error && <ErrorState error={error} onRetry={() => void load()} />}

      {loading ? (
        <div className="card">Loading trades…</div>
      ) : (
        <div className="card" style={{ padding: 0, overflow: "hidden" }}>
          <div style={{ padding: "1rem", borderBottom: "1px solid var(--line)" }}>
            <h3 style={{ margin: 0 }}>Trades ({trades.length})</h3>
          </div>
          {trades.length === 0 && (
            <p style={{ padding: "1rem", color: "var(--muted)" }}>
              No trades were returned for this organization.
            </p>
          )}
          {trades.length > 0 && (
            <table className="table" style={{ width: "100%", borderCollapse: "collapse" }}>
              <thead>
                <tr style={{ background: "rgba(255,255,255,0.02)", textAlign: "left" }}>
                  <th style={{ padding: "0.75rem 1rem" }}>Trade ID</th>
                  <th style={{ padding: "0.75rem 1rem" }}>Position ID</th>
                  <th style={{ padding: "0.75rem 1rem" }}>Symbol</th>
                  <th style={{ padding: "0.75rem 1rem" }}>Side</th>
                  <th style={{ padding: "0.75rem 1rem" }}>Venue</th>
                  <th style={{ padding: "0.75rem 1rem" }}>Qty</th>
                  <th style={{ padding: "0.75rem 1rem" }}>Price</th>
                  <th style={{ padding: "0.75rem 1rem" }}>Status</th>
                  <th style={{ padding: "0.75rem 1rem" }}>Executed At</th>
                  <th style={{ padding: "0.75rem 1rem" }}>Details</th>
                </tr>
              </thead>
              <tbody>
                {trades.map((t) => (
                  <tr key={t.id} style={{ borderTop: "1px solid var(--line)" }}>
                    <td style={{ padding: "0.75rem 1rem" }}><code>{t.id.slice(0, 12)}…</code></td>
                    <td style={{ padding: "0.75rem 1rem" }}>
                      <Link href={`/trading/trades/${t.position_id}`}>
                        <code>{t.position_id.slice(0, 12)}…</code>
                      </Link>
                    </td>
                    <td style={{ padding: "0.75rem 1rem" }}>{t.symbol}</td>
                    <td style={{ padding: "0.75rem 1rem" }}><span className="badge">{t.side}</span></td>
                    <td style={{ padding: "0.75rem 1rem" }}>{t.venue}</td>
                    <td style={{ padding: "0.75rem 1rem" }}>{String(t.qty)}</td>
                    <td style={{ padding: "0.75rem 1rem" }}>{t.price != null ? `$${t.price.toFixed(4)}` : "—"}</td>
                    <td style={{ padding: "0.75rem 1rem" }}><span className="badge">{t.status}</span></td>
                    <td style={{ padding: "0.75rem 1rem", fontSize: "0.85rem", color: "var(--muted)" }}>{t.executed_at}</td>
                    <td style={{ padding: "0.75rem 1rem" }}>
                      <Link href={`/trading/trades/${t.position_id}`}>Trades for position</Link>
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          )}
        </div>
      )}
    </AppShell>
  );
}
