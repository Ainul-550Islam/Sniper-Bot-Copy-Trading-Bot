"use client";

import type { TenantTrade } from "@/lib/api/trading-detail-api";

interface TradeDetailProps {
  trade: TenantTrade;
  positionId: string;
}

/**
 * Trades for one position: every figure renders exactly what the server
 * returned; a null price stays "—".
 */
export function TradeDetail({ trade, positionId }: TradeDetailProps) {
  return (
    <section className="card" aria-label="Trade detail">
      <h2>Trade Detail</h2>
      <p className="muted small">
        Trade <code>{trade.id}</code> recorded against position{" "}
        <code>{positionId}</code>.
      </p>
      <table className="table" style={{ width: "100%", borderCollapse: "collapse", marginTop: "1rem" }}>
        <tbody>
          <tr style={{ borderTop: "1px solid var(--line)" }}>
            <td style={{ padding: "0.75rem 1rem" }}><strong>Trade ID</strong></td>
            <td style={{ padding: "0.75rem 1rem" }}><code>{trade.id}</code></td>
          </tr>
          <tr style={{ borderTop: "1px solid var(--line)" }}>
            <td style={{ padding: "0.75rem 1rem" }}><strong>Position ID</strong></td>
            <td style={{ padding: "0.75rem 1rem" }}><code>{trade.position_id}</code></td>
          </tr>
          <tr style={{ borderTop: "1px solid var(--line)" }}>
            <td style={{ padding: "0.75rem 1rem" }}><strong>Symbol</strong></td>
            <td style={{ padding: "0.75rem 1rem" }}>{trade.symbol}</td>
          </tr>
          <tr style={{ borderTop: "1px solid var(--line)" }}>
            <td style={{ padding: "0.75rem 1rem" }}><strong>Side</strong></td>
            <td style={{ padding: "0.75rem 1rem" }}><span className="badge">{trade.side}</span></td>
          </tr>
          <tr style={{ borderTop: "1px solid var(--line)" }}>
            <td style={{ padding: "0.75rem 1rem" }}><strong>Venue</strong></td>
            <td style={{ padding: "0.75rem 1rem" }}>{trade.venue}</td>
          </tr>
          <tr style={{ borderTop: "1px solid var(--line)" }}>
            <td style={{ padding: "0.75rem 1rem" }}><strong>Qty</strong></td>
            <td style={{ padding: "0.75rem 1rem" }}>{String(trade.qty)}</td>
          </tr>
          <tr style={{ borderTop: "1px solid var(--line)" }}>
            <td style={{ padding: "0.75rem 1rem" }}><strong>Price</strong></td>
            <td style={{ padding: "0.75rem 1rem" }}>{trade.price != null ? `$${trade.price.toFixed(4)}` : "—"}</td>
          </tr>
          <tr style={{ borderTop: "1px solid var(--line)" }}>
            <td style={{ padding: "0.75rem 1rem" }}><strong>Status</strong></td>
            <td style={{ padding: "0.75rem 1rem" }}><span className="badge">{trade.status}</span></td>
          </tr>
          <tr style={{ borderTop: "1px solid var(--line)" }}>
            <td style={{ padding: "0.75rem 1rem" }}><strong>Executed At</strong></td>
            <td style={{ padding: "0.75rem 1rem" }}>{trade.executed_at}</td>
          </tr>
        </tbody>
      </table>
      <p style={{ marginTop: "1rem", fontSize: "0.8rem", color: "var(--muted)"}}>
        Figures are server truth; the browser never recomputes price or qty.
      </p>
    </section>
  );
}

export default TradeDetail;
