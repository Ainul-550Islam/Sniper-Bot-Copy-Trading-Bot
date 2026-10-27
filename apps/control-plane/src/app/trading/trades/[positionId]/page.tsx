"use client";

import { useCallback, useEffect, useState } from "react";
import { useParams } from "next/navigation";
import AppShell from "@/components/AppShell";
import { ErrorState } from "@/components/common/ErrorState";
import { ApiError, toDisplayError } from "@/lib/api";
import { tradingDetailApi } from "@/lib/api/trading-detail-api";
import type { TenantTrade } from "@/lib/api/trading-detail-api";

export default function TradesForPositionPage() {
  const params = useParams();
  const positionId = typeof params?.positionId === "string" ? params.positionId : "";
  const [trades, setTrades] = useState<TenantTrade[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  const load = useCallback(async () => {
    if (!positionId) return;
    setLoading(true);
    setError(null);
    try {
      const response = await tradingDetailApi.tradeForPosition(positionId);
      setTrades(Array.isArray(response.trades) ? response.trades : []);
    } catch (err: unknown) {
      if (err instanceof ApiError && err.status === 404) {
        setError(`No trades found for position ${positionId}`);
      } else {
        setError(toDisplayError(err));
      }
    } finally {
      setLoading(false);
    }
  }, [positionId]);

  useEffect(() => {
    void Promise.resolve().then(() => load());
  }, [load]);

  return (
    <AppShell title="Trades for Position">
      <div style={{ marginBottom: "1.5rem" }}>
        <h1 style={{ margin: 0 }}>Trades for Position: {positionId}</h1>
        <p style={{ margin: "0.25rem 0 0", color: "var(--muted)", fontSize: "0.9rem" }}>
          Trades recorded against this position, as returned by the tenant trading data plane.
        </p>
      </div>

      {error && <ErrorState error={error} onRetry={() => void load()} />}

      {loading ? (
        <div className="card">Loading trades…</div>
      ) : (
        <div className="card" style={{ padding: 0, overflow: "hidden" }}>
          <div style={{ padding: "1rem", borderBottom: "1px solid var(--line)" }}>
            <h3 style={{ margin: 0 }}>{trades.length} Trade{trades.length !== 1 ? "s" : ""}</h3>
          </div>
          {trades.length === 0 && (
            <p style={{ padding: "1rem", color: "var(--muted)" }}>
              No trades were returned for this position.
            </p>
          )}
          {trades.length > 0 && (
            <table className="table" style={{ width: "100%", borderCollapse: "collapse" }}>
              <thead>
                <tr style={{ background: "rgba(255,255,255,0.02)", textAlign: "left" }}>
                  <th style={{ padding: "0.75rem 1rem" }}>Trade ID</th>
                  <th style={{ padding: "0.75rem 1rem" }}>Symbol</th>
                  <th style={{ padding: "0.75rem 1rem" }}>Side</th>
                  <th style={{ padding: "0.75rem 1rem" }}>Venue</th>
                  <th style={{ padding: "0.75rem 1rem" }}>Qty</th>
                  <th style={{ padding: "0.75rem 1rem" }}>Price</th>
                  <th style={{ padding: "0.75rem 1rem" }}>Status</th>
                  <th style={{ padding: "0.75rem 1rem" }}>Executed At</th>
                </tr>
              </thead>
              <tbody>
                {trades.map((t) => (
                  <tr key={t.id} style={{ borderTop: "1px solid var(--line)" }}>
                    <td style={{ padding: "0.75rem 1rem" }}><code>{t.id.slice(0, 12)}…</code></td>
                    <td style={{ padding: "0.75rem 1rem" }}>{t.symbol}</td>
                    <td style={{ padding: "0.75rem 1rem" }}><span className="badge">{t.side}</span></td>
                    <td style={{ padding: "0.75rem 1rem" }}>{t.venue}</td>
                    <td style={{ padding: "0.75rem 1rem" }}>{String(t.qty)}</td>
                    <td style={{ padding: "0.75rem 1rem" }}>{t.price != null ? `$${t.price.toFixed(4)}` : "—"}</td>
                    <td style={{ padding: "0.75rem 1rem" }}><span className="badge">{t.status}</span></td>
                    <td style={{ padding: "0.75rem 1rem", fontSize: "0.85rem", color: "var(--muted)" }}>{t.executed_at}</td>
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
