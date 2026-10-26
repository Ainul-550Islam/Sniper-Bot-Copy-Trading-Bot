"use client";

import type { TransactionDetail } from "@/lib/api/trading-detail-api";

interface TransactionDetailProps {
  transaction: TransactionDetail;
}

/**
 * On-chain transaction detail: the signature is public on-chain data, shown
 * for display only — it is never a secret and never transmitted back to the
 * server from this page.
 */
export function TransactionDetail({ transaction }: TransactionDetailProps) {
  return (
    <section className="card" aria-label="Transaction detail">
      <h2>Transaction Detail</h2>
      <p className="muted small">
        Transaction <code>{transaction.signature.slice(0, 14)}…</code>
      </p>
      <table className="table" style={{ width: "100%", borderCollapse: "collapse", marginTop: "1rem" }}>
        <tbody>
          <tr style={{ borderTop: "1px solid var(--line)" }}>
            <td style={{ padding: "0.75rem 1rem" }}><strong>Signature</strong></td>
            <td style={{ padding: "0.75rem 1rem" }}><code>{transaction.signature}</code></td>
          </tr>
          <tr style={{ borderTop: "1px solid var(--line)" }}>
            <td style={{ padding: "0.75rem 1rem" }}><strong>Status</strong></td>
            <td style={{ padding: "0.75rem 1rem" }}><span className="badge">{transaction.status}</span></td>
          </tr>
          <tr style={{ borderTop: "1px solid var(--line)" }}>
            <td style={{ padding: "0.75rem 1rem" }}><strong>Slot</strong></td>
            <td style={{ padding: "0.75rem 1rem" }}>{transaction.slot != null ? String(transaction.slot) : "—"}</td>
          </tr>
          <tr style={{ borderTop: "1px solid var(--line)" }}>
            <td style={{ padding: "0.75rem 1rem" }}><strong>Fee (Lamports)</strong></td>
            <td style={{ padding: "0.75rem 1rem" }}>{transaction.fee_lamports != null ? String(transaction.fee_lamports) : "—"}</td>
          </tr>
          <tr style={{ borderTop: "1px solid var(--line)" }}>
            <td style={{ padding: "0.75rem 1rem" }}><strong>Confirmed At</strong></td>
            <td style={{ padding: "0.75rem 1rem" }}>{transaction.confirmed_at ?? "—"}</td>
          </tr>
          <tr style={{ borderTop: "1px solid var(--line)" }}>
            <td style={{ padding: "0.75rem 1rem" }}><strong>Memo</strong></td>
            <td style={{ padding: "0.75rem 1rem" }}>{transaction.memo ?? "—"}</td>
          </tr>
        </tbody>
      </table>
      <p style={{ marginTop: "1rem", fontSize: "0.8rem", color: "var(--muted)" }}>
        The transaction signature is public on-chain data; this page only
        displays it and never sends it as a credential.
      </p>
    </section>
  );
}

export default TransactionDetail;
