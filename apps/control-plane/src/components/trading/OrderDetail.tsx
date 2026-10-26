"use client";

import type { TenantOrder } from "@/lib/customer-trading-api";

interface OrderDetailProps {
  order: TenantOrder;
}

/**
 * Single order detail: every figure renders exactly what the server
 * returned; a null stays "—".
 */
export function OrderDetail({ order }: OrderDetailProps) {
  return (
    <section className="card" aria-label="Order detail">
      <h2>Order Detail</h2>
      <p className="muted small">
        Authoritative order row for order <code>{order.id}</code>.
      </p>
      <table className="table" style={{ width: "100%", borderCollapse: "collapse", marginTop: "1rem" }}>
        <tbody>
          <tr style={{ borderTop: "1px solid var(--line)" }}>
            <td style={{ padding: "0.75rem 1rem" }}><strong>Order ID</strong></td>
            <td style={{ padding: "0.75rem 1rem" }}><code>{order.id}</code></td>
          </tr>
          <tr style={{ borderTop: "1px solid var(--line)" }}>
            <td style={{ padding: "0.75rem 1rem" }}><strong>Idempotency Key</strong></td>
            <td style={{ padding: "0.75rem 1rem" }}><code>{order.idempotency_key}</code></td>
          </tr>
          <tr style={{ borderTop: "1px solid var(--line)" }}>
            <td style={{ padding: "0.75rem 1rem" }}><strong>Module</strong></td>
            <td style={{ padding: "0.75rem 1rem" }}>{order.module}</td>
          </tr>
          <tr style={{ borderTop: "1px solid var(--line)" }}>
            <td style={{ padding: "0.75rem 1rem" }}><strong>Side</strong></td>
            <td style={{ padding: "0.75rem 1rem" }}><span className="badge">{order.side}</span></td>
          </tr>
          <tr style={{ borderTop: "1px solid var(--line)" }}>
            <td style={{ padding: "0.75rem 1rem" }}><strong>Symbol</strong></td>
            <td style={{ padding: "0.75rem 1rem" }}>{order.symbol}</td>
          </tr>
          <tr style={{ borderTop: "1px solid var(--line)" }}>
            <td style={{ padding: "0.75rem 1rem" }}><strong>Venue</strong></td>
            <td style={{ padding: "0.75rem 1rem" }}>{order.venue}</td>
          </tr>
          <tr style={{ borderTop: "1px solid var(--line)" }}>
            <td style={{ padding: "0.75rem 1rem" }}><strong>Mode</strong></td>
            <td style={{ padding: "0.75rem 1rem" }}>{order.mode}</td>
          </tr>
          <tr style={{ borderTop: "1px solid var(--line)" }}>
            <td style={{ padding: "0.75rem 1rem" }}><strong>Status</strong></td>
            <td style={{ padding: "0.75rem 1rem" }}>{order.error ? <span title={order.error}>{order.status} ⚠</span> : order.status}</td>
          </tr>
          <tr style={{ borderTop: "1px solid var(--line)" }}>
            <td style={{ padding: "0.75rem 1rem" }}><strong>Qty</strong></td>
            <td style={{ padding: "0.75rem 1rem" }}>{String(order.qty)}</td>
          </tr>
          <tr style={{ borderTop: "1px solid var(--line)" }}>
            <td style={{ padding: "0.75rem 1rem" }}><strong>Price</strong></td>
            <td style={{ padding: "0.75rem 1rem" }}>{order.price != null ? `$${order.price.toFixed(4)}` : "—"}</td>
          </tr>
          <tr style={{ borderTop: "1px solid var(--line)" }}>
            <td style={{ padding: "0.75rem 1rem" }}><strong>External ID</strong></td>
            <td style={{ padding: "0.75rem 1rem" }}>{order.external_id ?? "—"}</td>
          </tr>
          <tr style={{ borderTop: "1px solid var(--line)" }}>
            <td style={{ padding: "0.75rem 1rem" }}><strong>Error</strong></td>
            <td style={{ padding: "0.75rem 1rem" }}>{order.error ?? "—"}</td>
          </tr>
          <tr style={{ borderTop: "1px solid var(--line)" }}>
            <td style={{ padding: "0.75rem 1rem" }}><strong>Created At</strong></td>
            <td style={{ padding: "0.75rem 1rem" }}>{order.created_at}</td>
          </tr>
          <tr style={{ borderTop: "1px solid var(--line)" }}>
            <td style={{ padding: "0.75rem 1rem" }}><strong>Updated At</strong></td>
            <td style={{ padding: "0.75rem 1rem" }}>{order.updated_at}</td>
          </tr>
          <tr style={{ borderTop: "1px solid var(--line)" }}>
            <td style={{ padding: "0.75rem 1rem" }}><strong>Submitted At</strong></td>
            <td style={{ padding: "0.75rem 1rem" }}>{order.submitted_at ?? "—"}</td>
          </tr>
          <tr style={{ borderTop: "1px solid var(--line)" }}>
            <td style={{ padding: "0.75rem 1rem" }}><strong>Finished At</strong></td>
            <td style={{ padding: "0.75rem 1rem" }}>{order.finished_at ?? "—"}</td>
          </tr>
        </tbody>
      </table>
      <p style={{ marginTop: "1rem", fontSize: "0.8rem", color: "var(--muted)" }}>
        Status and timestamps are server truth. The browser never re-labels them.
      </p>
    </section>
  );
}

export default OrderDetail;
