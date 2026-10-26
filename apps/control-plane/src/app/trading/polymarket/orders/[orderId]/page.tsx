"use client";

import { useCallback, useEffect, useState } from "react";
import { useParams } from "next/navigation";
import AppShell from "@/components/AppShell";
import { ErrorState } from "@/components/common/ErrorState";
import { ApiError, toDisplayError } from "@/lib/api";
import { tradingDetailApi, type PolymarketOrderDetail } from "@/lib/api/trading-detail-api";

export default function PolymarketOrderDetailPage() {
  const params = useParams();
  const venueOrderId =
    typeof params?.orderId === "string" ? params.orderId : "";
  const [order, setOrder] = useState<PolymarketOrderDetail | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  const load = useCallback(async () => {
    if (!venueOrderId) return;
    setLoading(true);
    setError(null);
    try {
      const next = await tradingDetailApi.polymarketOrder(venueOrderId);
      // A body without a venue order id is not a record; surface it honestly.
      if (next && typeof next.venue_order_id === "string") {
        setOrder(next);
      } else {
        setError(`The server returned no Polymarket order record for ${venueOrderId}.`);
      }
    } catch (err: unknown) {
      if (err instanceof ApiError && err.status === 404) {
        setError(`Polymarket order ${venueOrderId.slice(0, 14)}… not found`);
      } else {
        setError(toDisplayError(err));
      }
    } finally {
      setLoading(false);
    }
  }, [venueOrderId]);

  useEffect(() => {
    void Promise.resolve().then(() => load());
  }, [load]);

  return (
    <AppShell title="Polymarket Order Details">
      <div style={{ marginBottom: "1.5rem" }}>
        <h1 style={{ margin: 0 }}>Polymarket Order: {venueOrderId.slice(0, 14)}…</h1>
        <p style={{ margin: "0.25rem 0 0", color: "var(--muted)", fontSize: "0.9rem" }}>
          Authoritative CLOB order row from the Polymarket data plane.
        </p>
      </div>

      {error && <ErrorState error={error} onRetry={() => void load()} />}

      {loading ? (
        <div className="card">Loading order…</div>
      ) : order ? (
        <section className="card" aria-label="Polymarket order detail">
          <h2>Order Detail</h2>
          <p className="muted small">
            Venue order <code>{order.venue_order_id}</code>.
          </p>
          <table className="table" style={{ width: "100%", borderCollapse: "collapse", marginTop: "1rem" }}>
            <tbody>
              <tr style={{ borderTop: "1px solid var(--line)" }}>
                <td style={{ padding: "0.75rem 1rem" }}><strong>Venue Order ID</strong></td>
                <td style={{ padding: "0.75rem 1rem" }}><code>{order.venue_order_id}</code></td>
              </tr>
              <tr style={{ borderTop: "1px solid var(--line)" }}>
                <td style={{ padding: "0.75rem 1rem" }}><strong>Market</strong></td>
                <td style={{ padding: "0.75rem 1rem" }}>{order.market ?? "—"}</td>
              </tr>
              <tr style={{ borderTop: "1px solid var(--line)" }}>
                <td style={{ padding: "0.75rem 1rem" }}><strong>Side</strong></td>
                <td style={{ padding: "0.75rem 1rem" }}><span className="badge">{order.side ?? "—"}</span></td>
              </tr>
              <tr style={{ borderTop: "1px solid var(--line)" }}>
                <td style={{ padding: "0.75rem 1rem" }}><strong>Status</strong></td>
                <td style={{ padding: "0.75rem 1rem" }}><span className="badge">{order.status ?? "—"}</span></td>
              </tr>
              <tr style={{ borderTop: "1px solid var(--line)" }}>
                <td style={{ padding: "0.75rem 1rem" }}><strong>Price</strong></td>
                <td style={{ padding: "0.75rem 1rem" }}>{order.price != null ? `$${order.price.toFixed(4)}` : "—"}</td>
              </tr>
              <tr style={{ borderTop: "1px solid var(--line)" }}>
                <td style={{ padding: "0.75rem 1rem" }}><strong>Size</strong></td>
                <td style={{ padding: "0.75rem 1rem" }}>{order.size != null ? String(order.size) : "—"}</td>
              </tr>
              <tr style={{ borderTop: "1px solid var(--line)" }}>
                <td style={{ padding: "0.75rem 1rem" }}><strong>Created At</strong></td>
                <td style={{ padding: "0.75rem 1rem" }}>{order.created_at ?? "—"}</td>
              </tr>
            </tbody>
          </table>
          <p style={{ marginTop: "1rem", fontSize: "0.8rem", color: "var(--muted)" }}>
            Figures are server truth from the Polymarket data plane.
          </p>
        </section>
      ) : null}
    </AppShell>
  );
}
