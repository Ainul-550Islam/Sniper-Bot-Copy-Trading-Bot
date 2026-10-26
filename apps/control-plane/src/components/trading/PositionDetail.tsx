"use client";

import type { TenantPosition } from "@/lib/customer-trading-api";

interface PositionDetailProps {
  position: TenantPosition;
}

/**
 * Single position detail: every figure renders exactly what the server
 * returned; a null stays "—" and is never recomputed client-side.
 */
export function PositionDetail({ position }: PositionDetailProps) {
  return (
    <section className="card" aria-label="Position detail">
      <h2>Position Detail</h2>
      <p className="muted small">
        Authoritative inventory row for position <code>{position.id}</code>.
      </p>
      <table className="table" style={{ width: "100%", borderCollapse: "collapse", marginTop: "1rem" }}>
        <tbody>
          <tr style={{ borderTop: "1px solid var(--line)" }}>
            <td style={{ padding: "0.75rem 1rem" }}><strong>Position ID</strong></td>
            <td style={{ padding: "0.75rem 1rem" }}><code>{position.id}</code></td>
          </tr>
          <tr style={{ borderTop: "1px solid var(--line)" }}>
            <td style={{ padding: "0.75rem 1rem" }}><strong>Symbol</strong></td>
            <td style={{ padding: "0.75rem 1rem" }}>{position.symbol}</td>
          </tr>
          <tr style={{ borderTop: "1px solid var(--line)" }}>
            <td style={{ padding: "0.75rem 1rem" }}><strong>Side</strong></td>
            <td style={{ padding: "0.75rem 1rem" }}><span className="badge">{position.side}</span></td>
          </tr>
          <tr style={{ borderTop: "1px solid var(--line)" }}>
            <td style={{ padding: "0.75rem 1rem" }}><strong>Qty</strong></td>
            <td style={{ padding: "0.75rem 1rem" }}>{String(position.qty)}</td>
          </tr>
          <tr style={{ borderTop: "1px solid var(--line)" }}>
            <td style={{ padding: "0.75rem 1rem" }}><strong>Entry Price</strong></td>
            <td style={{ padding: "0.75rem 1rem" }}>{position.entry_price != null ? `$${position.entry_price.toFixed(4)}` : "—"}</td>
          </tr>
          <tr style={{ borderTop: "1px solid var(--line)" }}>
            <td style={{ padding: "0.75rem 1rem" }}><strong>Mark Price</strong></td>
            <td style={{ padding: "0.75rem 1rem" }}>{position.mark_price != null ? `$${position.mark_price.toFixed(4)}` : "—"}</td>
          </tr>
          <tr style={{ borderTop: "1px solid var(--line)" }}>
            <td style={{ padding: "0.75rem 1rem" }}><strong>Realized PnL</strong></td>
            <td style={{ padding: "0.75rem 1rem" }}>{position.realized_pnl != null ? `$${position.realized_pnl.toFixed(4)}` : "—"}</td>
          </tr>
          <tr style={{ borderTop: "1px solid var(--line)" }}>
            <td style={{ padding: "0.75rem 1rem" }}><strong>Unrealized PnL</strong></td>
            <td style={{ padding: "0.75rem 1rem" }}>{position.unrealized_pnl != null ? `$${position.unrealized_pnl.toFixed(4)}` : "—"}</td>
          </tr>
          <tr style={{ borderTop: "1px solid var(--line)" }}>
            <td style={{ padding: "0.75rem 1rem" }}><strong>Status</strong></td>
            <td style={{ padding: "0.75rem 1rem" }}><span className="badge">{position.status}</span></td>
          </tr>
          <tr style={{ borderTop: "1px solid var(--line)" }}>
            <td style={{ padding: "0.75rem 1rem" }}><strong>Opened At</strong></td>
            <td style={{ padding: "0.75rem 1rem" }}>{position.opened_at ?? "—"}</td>
          </tr>
          <tr style={{ borderTop: "1px solid var(--line)" }}>
            <td style={{ padding: "0.75rem 1rem" }}><strong>Closed At</strong></td>
            <td style={{ padding: "0.75rem 1rem" }}>{position.closed_at ?? "—"}</td>
          </tr>
        </tbody>
      </table>
      <p style={{ marginTop: "1rem", fontSize: "0.8rem", color: "var(--muted)" }}>
        Values are server truth. PnL is never estimated or recomputed in the browser.
      </p>
    </section>
  );
}

export default PositionDetail;
