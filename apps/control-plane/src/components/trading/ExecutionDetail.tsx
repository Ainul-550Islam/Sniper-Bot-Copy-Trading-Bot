"use client";

import type { TenantExecution } from "@/lib/customer-trading-api";

interface ExecutionDetailProps {
  execution: TenantExecution;
}

/**
 * Single execution detail: every figure renders exactly what the server
 * returned; the custody signature column reports what the server stored and
 * is never inferred client-side.
 */
export function ExecutionDetail({ execution }: ExecutionDetailProps) {
  return (
    <section className="card" aria-label="Execution detail">
      <h2>Execution Detail</h2>
      <p className="muted small">
        Execution record <code>{execution.id}</code> for order{" "}
        <code>{execution.order_id.slice(0, 12)}…</code>.
      </p>
      <table className="table" style={{ width: "100%", borderCollapse: "collapse", marginTop: "1rem" }}>
        <tbody>
          <tr style={{ borderTop: "1px solid var(--line)" }}>
            <td style={{ padding: "0.75rem 1rem" }}><strong>Execution ID</strong></td>
            <td style={{ padding: "0.75rem 1rem" }}><code>{execution.id}</code></td>
          </tr>
          <tr style={{ borderTop: "1px solid var(--line)" }}>
            <td style={{ padding: "0.75rem 1rem" }}><strong>Order ID</strong></td>
            <td style={{ padding: "0.75rem 1rem" }}><code>{execution.order_id}</code></td>
          </tr>
          <tr style={{ borderTop: "1px solid var(--line)" }}>
            <td style={{ padding: "0.75rem 1rem" }}><strong>Kind</strong></td>
            <td style={{ padding: "0.75rem 1rem" }}><span className="badge">{execution.kind}</span></td>
          </tr>
          <tr style={{ borderTop: "1px solid var(--line)" }}>
            <td style={{ padding: "0.75rem 1rem" }}><strong>Status</strong></td>
            <td style={{ padding: "0.75rem 1rem" }}>
              {execution.status} {execution.error ? "⚠" : ""}
            </td>
          </tr>
          <tr style={{ borderTop: "1px solid var(--line)" }}>
            <td style={{ padding: "0.75rem 1rem" }}><strong>Qty</strong></td>
            <td style={{ padding: "0.75rem 1rem" }}>{execution.qty != null ? String(execution.qty) : "—"}</td>
          </tr>
          <tr style={{ borderTop: "1px solid var(--line)" }}>
            <td style={{ padding: "0.75rem 1rem" }}><strong>Price</strong></td>
            <td style={{ padding: "0.75rem 1rem" }}>{execution.price != null ? `$${execution.price.toFixed(4)}` : "—"}</td>
          </tr>
          <tr style={{ borderTop: "1px solid var(--line)" }}>
            <td style={{ padding: "0.75rem 1rem" }}><strong>At</strong></td>
            <td style={{ padding: "0.75rem 1rem" }}>{execution.at}</td>
          </tr>
          <tr style={{ borderTop: "1px solid var(--line)" }}>
            <td style={{ padding: "0.75rem 1rem" }}><strong>Custody Signature</strong></td>
            <td style={{ padding: "0.75rem 1rem" }}>
              {execution.signature ? <code>{execution.signature.slice(0, 16)}…</code> : "none"}
            </td>
          </tr>
          <tr style={{ borderTop: "1px solid var(--line)" }}>
            <td style={{ padding: "0.75rem 1rem" }}><strong>Error</strong></td>
            <td style={{ padding: "0.75rem 1rem" }}>{execution.error ?? "—"}</td>
          </tr>
        </tbody>
      </table>
      <p style={{ marginTop: "1rem", fontSize: "0.8rem", color: "var(--muted)"}}>
        The signature column reports server-stored custody metadata only; no key
        material is ever present in this view.
      </p>
    </section>
  );
}

export default ExecutionDetail;
